//! Tests for Nucleus message boxes, crashlog quoting, startup timeouts, exit
//! handling, failed desktop passes, and audio build gating.
use super::*;

#[test]
fn a_message_box_nucleus_shows_instead_of_the_profile_is_named() {
    // Nucleus' check of a Documents folder in OneDrive waits for an
    // answer before its window opens; nobody gives it during a
    // background stream (#59).
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    let shown = MessageBox {
        window: 1,
        caption: "OneDrive must be disabled".into(),
        text: "Your Documents folder\r\nis in OneDrive.".into(),
    };
    screen
        .0
        .lock()
        .unwrap()
        .boxes
        .push((NUCLEUS, shown.clone()));
    // Another program's box is not Nucleus'.
    screen.0.lock().unwrap().boxes.push((
        99,
        MessageBox {
            window: 2,
            caption: "Other".into(),
            text: String::new(),
        },
    ));
    assert_eq!(runner.step(t0), None);
    assert_eq!(runner.step(t0), None);
    assert_eq!(runner.logged, [shown]);
    assert_eq!(
        runner.step(t0 + START_TIMEOUT),
        Some(End::Failed(
            "The co-op helper shows \"OneDrive must be disabled: Your Documents folder is in OneDrive.\" on the PC's screen".into()
        ))
    );
}

#[test]
fn a_window_without_the_profile_is_named_with_the_sessions_crash() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    let folder = tempfile::tempdir().unwrap();
    let log = folder.path().join("crashlog.txt");
    std::fs::write(&log, "[01.10.2026 10:00:00] System.Exception: earlier\n").unwrap();
    runner.crashlog = Some(Crashlog::at_launch(folder.path(), None));
    let mut later = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
    std::io::Write::write_all(
        &mut later,
        b"[29.10.2026 12:00:00] System.NullReferenceException: no game\n   at ChangeGame()\n",
    )
    .unwrap();
    screen
        .0
        .lock()
        .unwrap()
        .windows
        .push(window(5, NUCLEUS, ON_SCREEN));
    assert_eq!(runner.step(t0), None);
    assert_eq!(
        runner.step(t0 + START_TIMEOUT),
        Some(End::Failed(format!(
            "The co-op helper shows no profile Rubylight for Game; {HINT}. Its crashlog.txt says: [29.10.2026 12:00:00] System.NullReferenceException: no game"
        )))
    );
}

#[test]
fn a_game_nucleus_does_not_list_is_named_with_its_list() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let folder = tempfile::tempdir().unwrap();
    let exe = folder.path().join("Game.exe");
    std::fs::write(&exe, b"MZ").unwrap();
    let setup =
        butterpollo_session::nucleus::generic::Setup::new(&exe.to_string_lossy(), "Game", None);
    let own = files::write_own(folder.path(), &setup).unwrap();
    screen
        .0
        .lock()
        .unwrap()
        .windows
        .push(window(5, NUCLEUS, ON_SCREEN));
    let timeout = || {
        let mut runner = runner(1, &screen, &probe, t0);
        runner.game = Some((folder.path().to_path_buf(), own.guid.clone()));
        assert_eq!(runner.step(t0), None);
        runner.step(t0 + START_TIMEOUT)
    };
    // Listed: the profile is what's missing.
    assert_eq!(
        timeout(),
        Some(End::Failed(format!(
            "The co-op helper shows no profile Rubylight for Game; {HINT}"
        )))
    );
    // Not in the list Nucleus reads (#59).
    let list = files::list(folder.path());
    std::fs::remove_file(&list).unwrap();
    assert_eq!(
        timeout(),
        Some(End::Failed(format!(
            "The co-op helper does not list Game: {} has no entry for it with its handler and an .exe that is there",
            list.display()
        )))
    );
}

#[test]
fn only_crashes_written_after_the_launch_are_quoted() {
    let folder = tempfile::tempdir().unwrap();
    let log = folder.path().join("crashlog.txt");
    // No file at the launch or after it.
    let none = Crashlog::at_launch(folder.path(), None);
    assert_eq!(none.from, 0);
    assert_eq!(none.newest(None), None);
    std::fs::write(&log, "[1] System.Exception: before\n   at Old()\n").unwrap();
    let crashlog = Crashlog::at_launch(folder.path(), None);
    assert_eq!(crashlog.newest(None), None);
    let append = |text: &str| {
        let mut file = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        std::io::Write::write_all(&mut file, text.as_bytes()).unwrap();
    };
    append("[2] System.Exception: first\n   at A()\n[3] System.Exception: second\n   at B()\n\n");
    assert_eq!(
        crashlog.newest(None).as_deref(),
        Some("[3] System.Exception: second")
    );
    // A file started again since is read whole.
    std::fs::write(&log, "[4] new\n").unwrap();
    assert_eq!(crashlog.newest(None).as_deref(), Some("[4] new"));
    // A long line is cut.
    append(&format!("[5] {}\n", "x".repeat(400)));
    let cut = Crashlog::at_launch(folder.path(), None);
    append(&format!("[6] {}\n", "y".repeat(400)));
    let line = cut.newest(None).unwrap();
    assert!(
        line.starts_with("[6] yyy") && line.ends_with("..."),
        "{line}"
    );
    assert_eq!(line.chars().count(), Crashlog::LIMIT + 3);
    // None written since the launch: nothing is quoted.
    assert_eq!(Crashlog::at_launch(folder.path(), None).newest(None), None);
    // Of a huge one, only its end is read, where the newest is.
    let huge = Crashlog::at_launch(folder.path(), None);
    append(&"   at Deep()\n".repeat(1 << 16));
    append("[7] System.Exception: last\n   at C()\n");
    assert_eq!(
        huge.newest(None).as_deref(),
        Some("[7] System.Exception: last")
    );
    assert!(huge.since_launch().unwrap().len() as u64 <= Crashlog::TAIL);
}

#[test]
fn a_users_crashlog_is_read_as_them() {
    // Standing in for the user: refused what is granted to this account
    // by name.
    let user = crate::user_folder::tests::without_own_sid();
    let sid = crate::process::own_sid().unwrap();
    let data = tempfile::tempdir().unwrap();
    let theirs = data.path().join("theirs");
    crate::process::create_folder(&theirs, "D:P(A;OICI;FA;;;WD)").unwrap();
    let crashlog = Crashlog::at_launch(&theirs, Some(&user));
    std::fs::write(
        theirs.join("crashlog.txt"),
        "[1] System.Exception: theirs\n",
    )
    .unwrap();
    assert_eq!(
        crashlog.newest(Some(&user)).as_deref(),
        Some("[1] System.Exception: theirs")
    );
    // What the user can't read, the host doesn't read for them, though
    // it could.
    let hidden = data.path().join("hidden");
    crate::process::create_folder(&hidden, &format!("D:P(A;OICI;FA;;;{sid})")).unwrap();
    let crashlog = Crashlog::at_launch(&hidden, Some(&user));
    std::fs::write(
        hidden.join("crashlog.txt"),
        "[1] System.Exception: hidden\n",
    )
    .unwrap();
    assert_eq!(crashlog.newest(Some(&user)), None);
    assert_eq!(
        crashlog.newest(None).as_deref(),
        Some("[1] System.Exception: hidden")
    );
}

#[test]
fn a_covered_profile_list_is_named_when_it_times_out() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    label(&screen);
    screen.0.lock().unwrap().covered = true;
    assert_eq!(runner.step(t0), None);
    // Tried again on every pass.
    assert_eq!(runner.step(t0), None);
    assert_eq!(
        runner.step(t0 + START_TIMEOUT),
        Some(End::Failed(
            "The co-op helper did not start Game: covered".into()
        ))
    );
}

fn reminder() -> MessageBox {
    MessageBox {
        window: 3,
        caption: "Nucleus Co-op".into(),
        text: "Press the End key to unlock the input.".into(),
    }
}
const REMINDED: &str = "The co-op helper shows \"Nucleus Co-op: Press the End key to unlock the input.\" on the PC's screen";

#[test]
fn only_a_box_windows_shows_after_the_launch_is_named() {
    use crate::nucleus::instances::tests::CSRSS;
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let other = MessageBox {
        window: 4,
        caption: "Explorer.EXE - Application Error".into(),
        text: "The instruction referenced memory that could not be read.".into(),
    };
    screen.0.lock().unwrap().boxes.push((CSRSS, other.clone()));
    let mut runner = runner(1, &screen, &probe, t0);
    // What start keeps: the boxes up before Nucleus Co-op ran.
    runner.before = screen
        .clone()
        .message_boxes(&BTreeSet::new())
        .unwrap()
        .into_iter()
        .map(|shown| shown.window)
        .collect();
    screen
        .0
        .lock()
        .unwrap()
        .windows
        .push(window(5, NUCLEUS, ON_SCREEN));
    assert_eq!(runner.step(t0), None);
    assert_eq!(runner.logged, []);
    // Windows shows one for Nucleus Co-op (DefaultDesktopOnly) later.
    let unreadable = MessageBox {
        window: 6,
        caption: "Nucleus Co-op".into(),
        text: "Unable to load settings.".into(),
    };
    screen
        .0
        .lock()
        .unwrap()
        .boxes
        .push((CSRSS, unreadable.clone()));
    assert_eq!(runner.step(t0), None);
    assert_eq!(runner.logged, [unreadable]);
    assert_eq!(
        runner.step(t0 + START_TIMEOUT),
        Some(End::Failed(
            "The co-op helper shows \"Nucleus Co-op: Unable to load settings.\" on the PC's screen"
                .into()
        ))
    );
}

#[test]
fn a_start_that_cannot_list_the_boxes_up_before_nucleus_is_refused() {
    let folder = tempfile::tempdir().unwrap();
    let (screen, probe) = (screen(), Probe::default());
    screen.0.lock().unwrap().unlisted = true;
    let (running, _) = start_with(folder.path(), 1, [false; 4], &screen, &probe);
    let error = format!("{:#}", running.err().unwrap());
    assert!(
        error.contains("message boxes on the PC's screen before the co-op helper starts"),
        "{error}"
    );
    assert!(probe.launched.lock().unwrap().is_empty());
}

#[test]
fn a_box_covering_the_profile_list_is_named_instead_of_the_cover() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    label(&screen);
    {
        let mut desktop = screen.0.lock().unwrap();
        desktop.covered = true;
        desktop.boxes.push((NUCLEUS, reminder()));
    }
    assert_eq!(runner.step(t0), None);
    assert_eq!(runner.logged, [reminder()]);
    assert_eq!(
        runner.step(t0 + START_TIMEOUT),
        Some(End::Failed(REMINDED.into()))
    );
}

#[test]
fn a_box_nucleus_shows_after_the_click_is_named() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    label(&screen);
    assert_eq!(runner.step(t0), None);
    assert_eq!(screen.0.lock().unwrap().clicks, vec![7]);
    screen.0.lock().unwrap().boxes.push((NUCLEUS, reminder()));
    assert_eq!(runner.step(t0), None);
    assert_eq!(runner.logged, [reminder()]);
    assert_eq!(
        runner.step(t0 + START_TIMEOUT),
        Some(End::Failed(REMINDED.into()))
    );
}

#[test]
fn a_box_gone_from_the_list_is_not_quoted_and_a_failed_list_is_named() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    screen.0.lock().unwrap().boxes.push((NUCLEUS, reminder()));
    assert_eq!(runner.step(t0), None);
    assert_eq!(runner.showing, [reminder()]);
    // The box may be gone; a list that failed doesn't say it's still up.
    screen.0.lock().unwrap().unlisted = true;
    assert_eq!(runner.step(t0), None);
    assert_eq!(runner.showing, []);
    assert_eq!(
        runner.step(t0 + START_TIMEOUT),
        Some(End::Failed(
            "The co-op helper's windows could not be listed: access denied".into()
        ))
    );
}

#[test]
fn each_step_nucleus_makes_gives_it_the_timeout_again() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(2, &screen, &probe, t0);
    label(&screen);
    let clicked = t0 + START_TIMEOUT / 2;
    assert_eq!(runner.step(clicked), None);
    // Past the first deadline, but the click came since.
    let first = clicked + START_TIMEOUT / 2 + PERIOD;
    instance(&screen, 0);
    assert_eq!(runner.step(first), None);
    assert_eq!(runner.step(first + START_TIMEOUT - PERIOD), None);
    assert_eq!(
        runner.step(first + START_TIMEOUT),
        Some(End::Failed(
            "The co-op helper started 1 of 2 instances of Game".into()
        ))
    );
}

#[test]
fn an_instance_without_a_window_is_named() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    label(&screen);
    instance(&screen, 0);
    screen.0.lock().unwrap().windows.clear();
    assert_eq!(runner.step(t0), None);
    assert_eq!(
        runner.step(t0 + START_TIMEOUT),
        Some(End::Failed(
            "Player 1's instance of Game showed no window".into()
        ))
    );
}

#[test]
fn nucleus_exiting_before_the_game_fails_with_its_code() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    assert_eq!(runner.step(t0), None);
    *probe.exit.lock().unwrap() = Some(3);
    assert_eq!(
        runner.step(t0),
        Some(End::Failed(
            "The co-op helper exited with code 3 before starting Game".into()
        ))
    );
}

#[test]
fn nucleus_exiting_after_the_instances_started_leaves_them_running() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    label(&screen);
    instance(&screen, 0);
    assert_eq!(runner.step(t0), None);
    *probe.exit.lock().unwrap() = Some(0);
    assert_eq!(runner.step(t0), None);
    assert_eq!(runner.step(t0), None);
    assert_eq!(runner.shared.session.lock().unwrap().phase, Phase::Running);
}

#[test]
fn an_instance_that_exits_ends_the_session_and_everything_stops() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(2, &screen, &probe, t0);
    label(&screen);
    instance(&screen, 0);
    instance(&screen, 1);
    assert_eq!(runner.step(t0), None);
    let shared = runner.shared.clone();
    assert_eq!(shared.session.lock().unwrap().phase, Phase::Running);
    screen.0.lock().unwrap().processes.retain(|p| p.pid != 21);
    let end = runner.step(t0).unwrap();
    assert_eq!(
        end,
        End::Crashed("Player 2's instance of Game exited".into())
    );
    runner.finish(end.clone());
    assert_eq!(shared.session.lock().unwrap().phase, Phase::Ended(end));
    // Player 1's instance is stopped with Nucleus.
    assert_eq!(
        *probe.stopped.lock().unwrap(),
        Some(BTreeMap::from([(20, SINCE + 1)]))
    );
}

#[test]
fn an_instance_that_exits_while_starting_fails_the_launch() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(2, &screen, &probe, t0);
    label(&screen);
    instance(&screen, 0);
    assert_eq!(runner.step(t0), None);
    screen.0.lock().unwrap().processes.retain(|p| p.pid != 20);
    assert_eq!(
        runner.step(t0),
        Some(End::Failed("Player 1's instance of Game exited".into()))
    );
}

#[test]
fn a_failed_start_walks_the_game_folder_again() {
    use crate::nucleus::elevation;
    let t0 = Instant::now();
    let game = tempfile::tempdir().unwrap();
    // A walk in the clock tick the folder was made in is not kept.
    elevation::next_tick();
    elevation::folder_need(game.path(), None).unwrap();
    assert!(elevation::is_kept(game.path()));
    let (screen, probe) = (screen(), Probe::default());
    // A session stopped, or one that crashed once it ran: it stays.
    for end in [End::Ended, End::Crashed("kicked".into())] {
        let mut runner = runner(1, &screen, &probe, t0);
        runner.game_folder = Some(game.path().to_path_buf());
        runner.finish(end);
        assert!(elevation::is_kept(game.path()));
    }
    // One that failed to start: walked again next time.
    let mut runner = runner(1, &screen, &probe, t0);
    runner.game_folder = Some(game.path().to_path_buf());
    runner.finish(End::Failed("The co-op helper did not start Game".into()));
    assert!(!elevation::is_kept(game.path()));
    // Nor one Nucleus never started for.
    elevation::folder_need(game.path(), None).unwrap();
    assert!(elevation::is_kept(game.path()));
    let exe = game.path().join("Game.exe");
    let folder = tempfile::tempdir().unwrap();
    *probe.refused.lock().unwrap() = Some("access denied".into());
    let (running, _) = start_steam(
        folder.path(),
        1,
        [false; 4],
        &super::screen(),
        &probe,
        None,
        exe.to_str().unwrap(),
    );
    assert_eq!(running.err().unwrap().to_string(), "access denied");
    assert!(!elevation::is_kept(game.path()));
}

#[test]
fn a_session_ended_by_the_host_keeps_its_reason() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    let shared = runner.shared.clone();
    shared
        .session
        .lock()
        .unwrap()
        .end(End::Crashed("kicked".into()));
    let end = runner.step(t0).unwrap();
    runner.finish(End::Ended);
    assert_eq!(end, End::Crashed("kicked".into()));
    assert_eq!(
        shared.session.lock().unwrap().phase,
        Phase::Ended(End::Crashed("kicked".into()))
    );
    assert!(probe.stopped.lock().unwrap().is_some());
}

#[test]
fn a_window_that_cannot_be_parked_ends_the_session_after_some_passes() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    *runner.shared.sizes.lock().unwrap() = vec![(40000, 720)];
    label(&screen);
    instance(&screen, 0);
    for _ in 1..FAILED_PASSES {
        assert_eq!(runner.step(t0), None);
    }
    let end = runner.step(t0).unwrap();
    assert!(
        matches!(&end, End::Failed(why) if why.contains("does not fit beside the PC's desktop")),
        "{end:?}"
    );
}

#[test]
fn a_good_pass_resets_the_failed_passes() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    label(&screen);
    instance(&screen, 0);
    for _ in 0..2 {
        *runner.shared.sizes.lock().unwrap() = vec![(40000, 720)];
        for _ in 1..FAILED_PASSES {
            assert_eq!(runner.step(t0), None);
        }
        *runner.shared.sizes.lock().unwrap() = vec![(1280, 720)];
        assert_eq!(runner.step(t0), None);
    }
    assert_eq!(runner.shared.session.lock().unwrap().phase, Phase::Running);
}

#[test]
fn several_profile_entries_fail_at_once() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    label(&screen);
    label(&screen);
    for _ in 1..FAILED_PASSES {
        assert_eq!(runner.step(t0), None);
    }
    let end = runner.step(t0);
    assert!(
        matches!(&end, Some(End::Failed(why)) if why.contains("two labels")),
        "{end:?}"
    );
}

#[test]
fn per_player_audio_needs_build_20348() {
    let limit = audio_limit(Some(19045)).unwrap();
    assert!(
        limit.starts_with(
            "Per-player audio needs Windows build 20348 or newer and this PC runs build 19045"
        ),
        "{limit}"
    );
    assert_eq!(audio_limit(Some(20348)), None);
    assert_eq!(audio_limit(None), None);
}
