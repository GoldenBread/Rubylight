use super::*;
use std::{
    io::{Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::PathBuf,
};

fn shell(command: &str) -> Process {
    Process::shell(command, None, false, &BTreeMap::new()).unwrap()
}

/// Waits up to five seconds for `done`.
fn eventually(mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    done()
}

/// A new, empty folder for one test.
fn scratch(name: &str) -> PathBuf {
    let folder = std::env::temp_dir().join(format!("rl-process-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    folder
}

#[test]
fn a_command_gives_its_exit_code_or_the_signal_that_ended_it() {
    let exited = shell("exit 3");
    assert_eq!(exited.wait(Duration::from_secs(5)).unwrap(), 3);
    // Still readable: the code is read for good only as the process is dropped.
    assert_eq!(exited.exit_code().unwrap(), Some(3));
    let signalled = shell("kill -TERM $$");
    assert_eq!(
        signalled.wait(Duration::from_secs(5)).unwrap(),
        128 + libc::SIGTERM as u32
    );
    let missing = Process::spawn(
        Path::new("/no/such/program"),
        &[],
        None,
        Target::User { elevated: false },
        &BTreeMap::new(),
        true,
    );
    assert!(missing.is_err());
}

#[test]
fn a_wait_times_out_and_a_stop_ends_the_whole_tree() {
    let command = shell("sleep 30 & sleep 30 & wait");
    assert_eq!(command.exit_code().unwrap(), None);
    let error = command.wait(Duration::from_millis(50)).unwrap_err();
    assert_eq!(error.to_string(), "process timed out");
    assert!(eventually(|| command.active_processes().unwrap() == 3));
    let ids = command.process_ids().unwrap();
    assert!(ids.contains(&command.pid));
    let started: BTreeMap<u32, u64> = ids.iter().map(|pid| (*pid, creation_time(*pid))).collect();
    assert!(started.values().all(|started| *started != 0));
    command.stop().unwrap();
    assert_eq!(
        command.exit_code().unwrap(),
        Some(128 + libc::SIGKILL as u32)
    );
    assert!(eventually(|| command.active_processes().unwrap() == 0));
    assert!(started.keys().all(|pid| creation_time(*pid) == 0));
    // Stopping what has ended already is not an error.
    command.stop().unwrap();
    command.terminate().unwrap();
}

#[test]
fn dropping_a_command_ends_its_tree_unless_it_was_left_running() {
    let command = shell("sleep 30 & wait");
    assert!(eventually(|| command.active_processes().unwrap() == 2));
    let ids = command.process_ids().unwrap();
    drop(command);
    assert!(eventually(|| ids.iter().all(|pid| creation_time(*pid) == 0)));

    let leaving = Process::shell_leaving_children("sleep 30 &", None, false, &BTreeMap::new())
        .unwrap();
    assert_eq!(
        leaving.wait_leaving_children(Duration::from_secs(5)).unwrap(),
        0
    );
    let left = leaving.process_ids().unwrap();
    assert_eq!(left.len(), 1, "{left:?}");
    let left: BTreeMap<u32, u64> = left.iter().map(|pid| (*pid, creation_time(*pid))).collect();
    drop(leaving);
    std::thread::sleep(Duration::from_millis(100));
    assert!(left.iter().all(|(pid, started)| creation_time(*pid) == *started));
    // An ID with another start time is another process, and is left alone.
    let other: BTreeMap<u32, u64> = left.iter().map(|(pid, started)| (*pid, started + 1)).collect();
    stop_processes(&other, Duration::ZERO);
    assert!(left.iter().all(|(pid, started)| creation_time(*pid) == *started));
    stop_processes(&left, Duration::from_secs(5));
    assert!(eventually(|| left.keys().all(|pid| creation_time(*pid) == 0)));
}

#[test]
fn a_graceful_stop_asks_first_and_ends_what_does_not_listen() {
    let folder = scratch("graceful");
    let asked = folder.join("asked");
    // The shell notes the request and ends; its child ignores it.
    let command = shell(&format!(
        "trap 'touch {}; exit 0' TERM; (trap '' TERM; sleep 30) & wait",
        asked.display()
    ));
    assert!(eventually(|| command.active_processes().unwrap() >= 2));
    // Time for the traps to be set.
    std::thread::sleep(Duration::from_millis(200));
    command.stop_graceful(Duration::from_millis(500)).unwrap();
    assert!(asked.exists());
    assert!(eventually(|| command.active_processes().unwrap() == 0));
    std::fs::remove_dir_all(folder).unwrap();
}

#[test]
fn a_cancelled_wait_ends_the_command() {
    let cancel = std::sync::Arc::new(CommandCancellation::new().unwrap());
    assert!(!cancel.requested());
    let command = shell("sleep 30");
    let canceller = {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            cancel.cancel().unwrap();
        })
    };
    let began = Instant::now();
    let waited = command
        .wait_cancellable(Duration::from_secs(20), &cancel)
        .unwrap();
    canceller.join().unwrap();
    assert_eq!(waited, None);
    assert!(began.elapsed() < Duration::from_secs(10));
    assert!(cancel.requested());
    assert!(command.exit_code().unwrap().is_some());
    // Uncancelled, it is a wait like any other.
    let fresh = CommandCancellation::new().unwrap();
    assert_eq!(
        shell("exit 7")
            .wait_cancellable(Duration::from_secs(5), &fresh)
            .unwrap(),
        Some(7)
    );
    let late = shell("sleep 30");
    let error = late
        .wait_cancellable(Duration::from_millis(50), &fresh)
        .unwrap_err();
    assert_eq!(error.to_string(), "process timed out");
    assert!(late.exit_code().unwrap().is_some());
}

#[test]
fn a_piped_program_talks_over_its_standard_streams() {
    let mut piped =
        Process::spawn_piped(Path::new("/bin/cat"), &[], Target::User { elevated: false }).unwrap();
    let mut stdin = piped.stdin.take().unwrap();
    let mut stdout = piped.stdout.take().unwrap();
    stdin.write_all(b"hello\n").unwrap();
    // One of the program's own descriptors, read from this side: its standard output.
    match piped.process.duplicate_resource(1) {
        Ok(theirs) => {
            let mut theirs = std::fs::File::from(theirs);
            theirs.write_all(b"from the host\n").unwrap();
        }
        // A sandbox may forbid looking into another process, a child too.
        Err(error) => {
            let denied = error.chain().any(|cause| {
                cause
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| {
                        matches!(error.raw_os_error(), Some(libc::EPERM) | Some(libc::ENOSYS))
                    })
            });
            assert!(denied, "{error:#}");
        }
    }
    drop(stdin);
    let mut text = String::new();
    stdout.read_to_string(&mut text).unwrap();
    assert!(text.contains("hello\n"), "{text}");
    assert_eq!(piped.process.wait(Duration::from_secs(5)).unwrap(), 0);
    assert!(piped.process.duplicate_into_raw(1).is_err());
}

#[test]
fn extras_reach_the_command_and_bad_ones_are_refused() {
    let extras = BTreeMap::from([("RL_PROCESS_TEST".to_owned(), "yes it is".to_owned())]);
    let command = Process::shell(
        "test \"$RL_PROCESS_TEST\" = 'yes it is' && test \"$PWD\" = /",
        Some(Path::new("/")),
        false,
        &extras,
    )
    .unwrap();
    assert_eq!(command.wait(Duration::from_secs(5)).unwrap(), 0);
    for (name, value) in [("", "x"), ("A=B", "x"), ("A", "x\0y")] {
        let extras = BTreeMap::from([(name.to_owned(), value.to_owned())]);
        let refused = Process::shell("true", None, false, &extras).err().unwrap();
        assert_eq!(refused.to_string(), "invalid process environment variable");
    }
    assert!(Process::shell("true\0", None, false, &BTreeMap::new()).is_err());
    let own = user_environment().unwrap();
    assert_eq!(
        own.get("PATH").cloned(),
        std::env::var_os("PATH").map(|path| path.to_string_lossy().into_owned())
    );
}

#[test]
fn a_detached_command_outlives_the_call() {
    let folder = scratch("detached");
    let done = folder.join("done");
    Process::shell_detached(
        &format!("sleep 0.2; touch {}", done.display()),
        None,
        false,
        &BTreeMap::new(),
    )
    .unwrap();
    assert!(eventually(|| done.exists()));
    Process::spawn_detached(
        Path::new("/bin/sh"),
        &["-c".into(), "exit 0".into()],
        Target::User { elevated: false },
    )
    .unwrap();
    std::fs::remove_dir_all(folder).unwrap();
}

#[test]
fn only_the_hosts_own_user_is_a_target() {
    let own = own_sid().unwrap();
    assert_eq!(user_sid(), Some(own.clone()));
    assert!(check_target(&Target::Account(own.clone())).is_ok());
    assert!(check_target(&Target::Account(format!("{own}0"))).is_err());
    assert!(check_target(&Target::SystemSession(1)).is_err());
    assert_eq!(
        check_target(&Target::User { elevated: true }).is_ok(),
        elevated().unwrap()
    );
    assert_eq!(
        user_account().unwrap(),
        if elevated().unwrap() {
            Account::Elevates
        } else {
            Account::Standard
        }
    );
    assert!(!is_system());
    assert!(user_signed_in());
    assert_eq!(
        process_session(std::process::id()).unwrap(),
        current_session().unwrap()
    );
    assert_eq!(console_session(), current_session().unwrap());
    assert!(process_session(u32::MAX).is_err());
    assert!(ensure_streaming_session().is_ok());
    let recovery = console_recovery().unwrap();
    assert!(!recovery.can_reattach);
    assert!(reattach_console(recovery.session_id, recovery.console_session_id).is_err());
    let stop = StopSignal::new(None).unwrap();
    assert!(!stop.supervised() && !stop.requested());
    stop.wait();
    assert!(StopSignal::new(Some("1:2")).is_err());
}

#[test]
fn a_game_process_is_held_while_it_runs() {
    let game = shell("exec sleep 30");
    let identity = butterpollo_session::audio_scope::Identity {
        pid: game.pid,
        started: creation_time(game.pid),
    };
    let held = AudioProcess::open(identity).unwrap();
    assert!(held.alive());
    assert!(
        AudioProcess::open(butterpollo_session::audio_scope::Identity {
            started: identity.started + 1,
            ..identity
        })
        .is_err()
    );
    game.stop().unwrap();
    assert!(!held.clone().alive());
    assert!(AudioProcess::open(identity).is_err());
}

#[test]
fn command_lines_are_split_into_program_and_arguments() {
    assert_eq!(command_target("  steam -bigpicture "), "steam");
    assert_eq!(command_arguments("  steam -bigpicture "), "-bigpicture");
    assert_eq!(
        command_target("\"/opt/My Game/run.sh\" --fullscreen"),
        "/opt/My Game/run.sh"
    );
    assert_eq!(
        command_arguments("\"/opt/My Game/run.sh\"  --fullscreen"),
        "--fullscreen"
    );
    assert_eq!(command_target("'/opt/My Game/run.sh' -x"), "/opt/My Game/run.sh");
    assert_eq!(command_arguments("'/opt/My Game/run.sh' -x"), "-x");
    assert_eq!(command_target(""), "");
    assert_eq!(command_arguments("game"), "");
    assert_eq!(shell_command("game --flag"), "game --flag");
    assert_eq!(shell_command("PATH=/a:/b game"), "PATH=/a:/b game");
    assert_eq!(
        shell_command("steam://rungameid/570"),
        "xdg-open 'steam://rungameid/570'"
    );
    assert_eq!(
        shell_command("\"heroic://launch/it's\" -x"),
        "xdg-open 'heroic://launch/it'\\''s' -x"
    );
}

#[test]
fn links_are_not_followed_and_folders_are_made_with_their_permissions() {
    let folder = scratch("security");
    let file = folder.join("file");
    std::fs::write(&file, b"text").unwrap();
    let link = folder.join("link");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    let folder_link = folder.join("folder-link");
    std::os::unix::fs::symlink(&folder, &folder_link).unwrap();

    let mut text = String::new();
    open_unfollowed(&file, false)
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    assert_eq!(text, "text");
    let refused = open_unfollowed(&link, false).unwrap_err();
    assert!(refused.to_string().contains("is a link"), "{refused}");
    let refused = open_unfollowed(&folder_link, true).unwrap_err();
    assert!(refused.to_string().contains("is a link"), "{refused}");
    assert!(open_unfollowed(&file, true).is_err());
    assert_eq!(
        open_unfollowed(&folder.join("missing"), false)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::NotFound
    );
    assert!(Hold::new([folder.as_path(), Path::new("/")]).is_ok());
    assert!(Hold::new([folder_link.as_path()]).is_err());

    let open = open_unfollowed(&folder, true).unwrap();
    let own = own_sid().unwrap();
    assert_eq!(open_file_owner(&open).unwrap(), own);
    assert!(account_name(&own).is_some());
    assert_eq!(account_name("everyone").as_deref(), Some("everyone"));
    assert_eq!(account_name("S-1-5-32-545"), None);
    let gid = format!("gid:{}", open.metadata().unwrap().gid());
    assert!(account_name(&gid).is_some());

    let mode = |path: &Path| std::fs::metadata(path).unwrap().mode() & 0o777;
    let readable = folder.join("readable");
    create_folder_administrators_readable(&readable).unwrap();
    assert_eq!(mode(&readable), 0o755);
    assert_eq!(
        create_folder_administrators_readable(&readable)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::AlreadyExists
    );
    let private = folder.join("private");
    create_folder_administrators_only(&private).unwrap();
    assert_eq!(mode(&private), 0o700);
    let record = readable.join("record");
    create_file_administrators_readable(&record)
        .unwrap()
        .write_all(b"{}")
        .unwrap();
    assert_eq!(mode(&record), 0o644);
    assert_eq!(
        create_file_administrators_readable(&link).unwrap_err().kind(),
        std::io::ErrorKind::AlreadyExists
    );

    std::fs::set_permissions(&readable, std::fs::Permissions::from_mode(0o777)).unwrap();
    let open = open_unfollowed(&readable, true).unwrap();
    assert_eq!(
        open_file_writers(&open).unwrap(),
        [own.clone(), gid, "everyone".to_owned()]
    );
    restrict_to_administrators(&readable).unwrap();
    assert_eq!(mode(&readable), 0o700);
    assert_eq!(mode(&record), 0o600);
    let open = open_unfollowed(&readable, true).unwrap();
    assert_eq!(open_file_writers(&open).unwrap(), [own.clone()]);
    assert_eq!(open_file_child_movers(&open).unwrap(), [own.clone()]);
    assert_eq!(open_file_movers(&open).unwrap(), [own]);
    assert!(restrict(&readable, "O:BAD:P(A;OICI;FA;;;SY)").is_err());
    std::fs::remove_dir_all(folder).unwrap();
}
