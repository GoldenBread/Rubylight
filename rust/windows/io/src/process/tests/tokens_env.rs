//! Tests for command parsing, stop signals, user tokens, environment blocks, and creation time.
use super::super::control::{HostStop, duplicate_stop_event, parse_stop_source};
use super::super::spawn::{environment, settable};
use super::super::tokens::{
    account_of, own_linked_account, same_account, shell_token, target_token, token,
    token_in_administrators,
};
use super::super::*;
use butterpollo_session::nucleus::elevation::Account;
use std::{collections::BTreeMap, time::Duration};

#[test]
fn urls_and_documents_open_through_their_association() {
    assert_eq!(
        shell_command("steam://open/bigpicture"),
        "start \"\" /wait \"steam://open/bigpicture\""
    );
    assert_eq!(
        shell_command("ms-settings:display"),
        "start \"\" /wait \"ms-settings:display\""
    );
    assert_eq!(
        shell_command("\"C:\\Games\\My Game.lnk\" -windowed"),
        "start \"\" /wait \"C:\\Games\\My Game.lnk\" -windowed"
    );
    for unchanged in [
        "\"C:\\Program Files\\Game\\game.exe\" -fullscreen",
        "C:\\tools\\setup.bat",
        "powershell -NoProfile -Command Get-Date",
        "start \"\" steam://run/570",
        "",
    ] {
        assert_eq!(shell_command(unchanged), unchanged);
    }
    assert_eq!(command_target("\"C:\\a b\\c.exe\" x"), "C:\\a b\\c.exe");
}

#[test]
fn a_commands_arguments_are_what_follows_its_program() {
    assert_eq!(
        command_arguments(r#" "C:\a b\c.exe" -game "x y" "#),
        r#"-game "x y""#
    );
    assert_eq!(command_arguments(r"C:\Ice\Ice.exe  -windowed"), "-windowed");
    assert_eq!(command_arguments(r"C:\Ice\Ice.exe"), "");
    assert_eq!(command_arguments(r#""C:\a b\c.exe""#), "");
}

#[test]
fn supervisor_event_is_wait_only_and_outlives_the_source_handle() -> Result<()> {
    let source = HostStop::new()?;
    let (pid, handle) = parse_stop_source(&source.source())?;
    let signal = StopSignal(Some(duplicate_stop_event(pid, handle)?));
    assert!(!signal.requested());
    let event = raw(signal.0.as_ref().unwrap());
    assert_eq!(
        // SAFETY: `event` is a handle owned by `signal`, which is alive here.
        unsafe { SetEvent(event) }.unwrap_err().code(),
        windows::core::HRESULT::from_win32(ERROR_ACCESS_DENIED.0)
    );
    source.request()?;
    drop(source);
    assert!(signal.requested());
    // SAFETY: `event` is a handle owned by `signal`, which is alive here.
    assert!(unsafe { ResetEvent(event) }.is_err());
    Ok(())
}

#[test]
fn a_stop_signal_wait_returns_when_the_supervisor_asks_and_at_once_without_one() -> Result<()> {
    let unsupervised = StopSignal::new(None)?;
    assert!(!unsupervised.supervised());
    unsupervised.wait();
    let source = HostStop::new()?;
    let (pid, handle) = parse_stop_source(&source.source())?;
    let signal = std::sync::Arc::new(StopSignal(Some(duplicate_stop_event(pid, handle)?)));
    assert!(signal.supervised());
    let (done, waited) = std::sync::mpsc::channel();
    let waiter = signal.clone();
    std::thread::spawn(move || {
        waiter.wait();
        let _ = done.send(());
    });
    assert!(waited.recv_timeout(Duration::from_millis(200)).is_err());
    source.request()?;
    waited.recv_timeout(Duration::from_secs(5))?;
    assert!(signal.requested());
    Ok(())
}

#[test]
fn shutdown_source_rejects_invalid_handles_and_foreign_parents() -> Result<()> {
    for source in [
        "",
        "1",
        "0:1",
        "1:0",
        "1:-1",
        "1:1:2",
        "+1:2",
        "1:xyz",
        "4294967296:1",
        "1:ffffffffffffffffffffffff",
    ] {
        assert!(parse_stop_source(source).is_err(), "{source}");
    }
    assert!(StopSignal::new(Some(&HostStop::new()?.source())).is_err());
    assert!(!StopSignal::new(None)?.requested());
    Ok(())
}

#[test]
fn shell_runs_grouped_commands_with_quoted_output_paths() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("quoted output.log");
    let command = format!("(echo NATIVE_SHELL) 1>\"{}\" 2>&1", output.display());
    let child = Process::shell(
        &command,
        Some(directory.path()),
        target_token(&Target::User { elevated: true }).is_ok(),
        &Default::default(),
    )
    .unwrap();
    assert_eq!(child.wait(std::time::Duration::from_secs(5)).unwrap(), 0);
    assert_eq!(
        std::fs::read_to_string(output).unwrap().trim(),
        "NATIVE_SHELL"
    );
}

#[test]
fn portable_non_elevated_targets_never_reuse_an_elevated_token() -> Result<()> {
    if is_system() {
        return Ok(());
    }
    let selected = match target_token(&Target::User { elevated: false }) {
        Ok(selected) => selected,
        Err(_) => {
            assert!(Process::shell("exit /b 0", None, false, &BTreeMap::new()).is_err());
            return Ok(());
        }
    };
    let current = token()?;
    // With User Account Control off the desktop shell is elevated too and
    // no standard-rights token exists: the program gets the desktop's
    // rights, never more than the user's own desktop has.
    if token_elevated(&current)? && shell_token(&current)?.is_none() {
        assert!(selected.is_none());
        return Ok(());
    }
    let mut info = TOKEN_ELEVATION::default();
    let mut bytes = 0;
    // SAFETY: the selected/current token and TOKEN_ELEVATION output live through the call.
    unsafe {
        GetTokenInformation(
            raw(selected.as_ref().unwrap_or(&current)),
            TokenElevation,
            Some((&mut info as *mut TOKEN_ELEVATION).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut bytes,
        )?;
    }
    assert_eq!(info.TokenIsElevated, 0);
    Ok(())
}

#[test]
fn administrator_hosts_start_non_elevated_programs_with_standard_rights() {
    if is_system() {
        return;
    }
    // An elevated portable host previously failed every non-elevated
    // program with ERROR_BAD_IMPERSONATION_LEVEL.
    let environment = user_environment().unwrap();
    assert!(environment.contains_key("USERPROFILE"));
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("groups.txt");
    // Windows' own whoami: a PATH with Git's usr\bin first finds one without /groups.
    let windir = std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap());
    let whoami = windir.join("System32\\whoami.exe");
    let child = Process::shell(
        // Unquoted: cmd /c would strip the first and last of several quotes.
        &format!(
            "{} /groups 1>\"{}\" 2>&1",
            whoami.display(),
            output.display()
        ),
        None,
        false,
        &BTreeMap::new(),
    )
    .unwrap();
    // whoami /groups took 12 s on a PC busy with parallel builds: the limit only
    // catches a hang, not slowness.
    let code = child.wait(Duration::from_secs(60)).unwrap();
    // Localized output uses the OEM code page; the SIDs are ASCII.
    let groups = String::from_utf8_lossy(&std::fs::read(output).unwrap_or_default()).into_owned();
    assert_eq!(code, 0, "{groups}");
    // Medium mandatory level, never High, unless User Account Control is
    // off (as on CI runners): then the desktop itself is elevated and the
    // program gets what the desktop would give it rather than failing.
    let own = token().unwrap();
    let standard = !token_elevated(&own).unwrap() || shell_token(&own).unwrap().is_some();
    assert_eq!(groups.contains("S-1-16-8192"), standard, "{groups}");
    assert_eq!(groups.contains("S-1-16-12288"), !standard, "{groups}");
}

#[test]
fn programs_started_without_a_token_get_this_hosts_environment() {
    // A non-elevated host gave its programs the system's variables, and the PyroWave probe
    // worker missed the AMD shader cache under C:\Users\Default, compiled every pipeline
    // again and timed out.
    let environment = environment(None).unwrap();
    for name in ["USERPROFILE", "USERNAME", "TEMP", "PATH"] {
        assert_eq!(
            environment.get(name).map(String::as_str),
            std::env::var(name).ok().as_deref(),
            "{name}"
        );
    }
    if !is_system() {
        // Apps and client commands get this one, from whichever token the host uses.
        let user = user_environment().unwrap();
        for name in ["USERPROFILE", "USERNAME"] {
            assert_eq!(user.get(name), environment.get(name), "{name}");
        }
    }
}

#[test]
fn programs_started_for_the_user_keep_the_users_profile() {
    if is_system() {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("user.txt");
    // `/u` writes UTF-16, so a profile name outside the OEM code page reads back intact.
    let child = Process::shell(
        &format!("cmd /d /u /c set USER 1>\"{}\" 2>&1", output.display()),
        None,
        false,
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(child.wait(Duration::from_secs(10)).unwrap(), 0);
    let bytes = std::fs::read(output).unwrap();
    let wide: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    let text = String::from_utf16_lossy(&wide);
    let seen: BTreeMap<_, _> = text
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(name, value)| (name.to_uppercase(), value.to_owned()))
        .collect();
    for name in ["USERPROFILE", "USERNAME"] {
        assert_eq!(
            seen.get(name),
            std::env::var(name).ok().as_ref(),
            "{name}: {text}"
        );
    }
}

#[test]
fn the_signed_in_account_is_told_apart() {
    // User Account Control: an administrator's filtered token, linked
    // to their own elevated one.
    assert_eq!(
        account_of(Some((true, true)), false, true),
        Account::Elevates
    );
    // Administrator protection: linked to another account's, or to none.
    assert_eq!(
        account_of(Some((true, false)), false, true),
        Account::Protected
    );
    assert_eq!(account_of(None, false, true), Account::Protected);
    // User Account Control off: elevated itself.
    assert_eq!(account_of(None, true, true), Account::Elevates);
    // A standard account, linked to nothing or nothing elevated.
    assert_eq!(account_of(None, false, false), Account::Standard);
    assert_eq!(
        account_of(Some((false, true)), false, false),
        Account::Standard
    );
}

#[test]
fn the_program_starts_only_as_the_signed_in_users_own_account() {
    let user = "S-1-5-21-1-2-3-1001";
    let other = "S-1-5-21-1-2-3-1002";
    // User Account Control's elevated token is the same account's.
    own_linked_account(user, user).unwrap();
    // Administrator protection's separate administrator is never started
    // as, and the refusal never advises turning it off.
    let refused = format!("{:#}", own_linked_account(user, other).unwrap_err());
    assert_eq!(
        refused,
        "Starting it elevated needs administrator rights, and Windows' Administrator protection keeps them from Rubylight"
    );
    // A start for the account whose copy a launch checked runs as it,
    // or not at all.
    same_account(user, user).unwrap();
    let changed = format!("{:#}", same_account(user, other).unwrap_err());
    assert!(changed.contains("another account signed in"), "{changed}");
    // Outside the service, a program runs as this process's account.
    if !is_system() && !elevated().unwrap() {
        let own = own_sid().unwrap();
        assert!(target_token(&Target::Account(own)).unwrap().is_none());
        assert!(target_token(&Target::Account(other.into())).is_err());
    }
}

#[test]
fn this_hosts_own_token_names_administrators_when_elevated() {
    let token = token().unwrap();
    let administrator = token_in_administrators(&token).unwrap();
    if elevated().unwrap() {
        assert!(administrator);
    }
    if !is_system() {
        // Outside the service the account is the host's own rights.
        match user_account() {
            Ok(account) => assert_eq!(account, Account::Elevates),
            Err(_) => assert!(!elevated().unwrap()),
        }
    }
}

#[test]
fn user_elevation_is_the_elevated_targets_own() {
    if is_system() {
        return;
    }
    // Outside the service only an elevated host starts elevated programs.
    match user_runs_elevated() {
        Ok(runs) => assert!(runs && elevated().unwrap()),
        Err(error) => {
            assert!(!elevated().unwrap());
            assert!(
                format!("{error:#}").contains("Windows service"),
                "{error:#}"
            );
        }
    }
}

#[test]
fn portable_user_identity_does_not_require_a_non_elevated_token() {
    if !is_system() {
        assert!(user_sid().is_some_and(|sid| !sid.is_empty()));
    }
}

#[test]
fn a_host_started_from_a_shell_can_still_hand_on_its_environment() {
    let inherited = BTreeMap::from([
        ("=C:".to_string(), r"C:\src".to_string()),
        ("=EXITCODE".to_string(), "00000000".to_string()),
        ("PATH".to_string(), r"C:\a=b".to_string()),
    ]);
    assert_eq!(
        settable(inherited),
        BTreeMap::from([("PATH".to_string(), r"C:\a=b".to_string())])
    );
}

#[test]
fn an_exited_process_another_program_still_holds_open_is_not_running() {
    // A launcher or Steam holding the game's handle keeps an exited game openable: its
    // settings files and Playnite's end check must not wait for it (issue 82).
    let ping = std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap())
        .join("System32")
        .join("ping.exe");
    let mut child = std::process::Command::new(&ping)
        .args(["-n", "30", "127.0.0.1"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let pid = child.id();
    assert_ne!(
        creation_time(pid),
        0,
        "a running process has its creation time"
    );
    child.kill().unwrap();
    child.wait().unwrap();
    // `child` still holds its handle: the process can be opened, yet it has exited.
    assert_eq!(creation_time(pid), 0);
    drop(child);
}
