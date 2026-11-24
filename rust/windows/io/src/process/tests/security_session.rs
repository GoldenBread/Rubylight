//! Tests for Windows security descriptors, console reattachment, and streaming session checks.
use super::super::security::{
    ADMINISTRATORS_ONLY, CHILD_MOVE, USERS_READ, WRITE, dacl_allowed, open_file_allowed,
};
use super::super::session::{
    UserLookup, disconnected_desktops, ensure_streaming_session_with, reattach_blocker,
    same_disconnected_owner, user_lookup_change, validate_streaming_session,
    verify_disconnected_console_owner,
};
use super::super::tokens::own_session_user;
use super::super::*;

#[test]
fn users_may_read_but_never_change_a_readable_folder() {
    use windows::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    use windows::Win32::Storage::FileSystem::{
        DELETE, FILE_APPEND_DATA, FILE_DELETE_CHILD, FILE_LIST_DIRECTORY, FILE_WRITE_ATTRIBUTES,
        FILE_WRITE_DATA, FILE_WRITE_EA, READ_CONTROL, WRITE_DAC, WRITE_OWNER,
    };
    let users = USERS_READ
        .split('(')
        .find(|ace| ace.ends_with(";;;BU)"))
        .unwrap();
    let mask = u32::from_str_radix(
        users.split(';').nth(2).unwrap().trim_start_matches("0x"),
        16,
    )
    .unwrap();
    let write = DELETE.0
        | FILE_APPEND_DATA.0
        | FILE_DELETE_CHILD.0
        | FILE_WRITE_ATTRIBUTES.0
        | FILE_WRITE_DATA.0
        | FILE_WRITE_EA.0
        | WRITE_DAC.0
        | WRITE_OWNER.0
        | 0xf000_0000;
    assert_eq!(mask & write, 0, "{mask:#x}");
    // Open to check and read, as a host without administrator rights does.
    assert_eq!(
        mask & (FILE_LIST_DIRECTORY.0 | READ_CONTROL.0),
        FILE_LIST_DIRECTORY.0 | READ_CONTROL.0
    );
    // The rest is the administrators-only one's.
    assert!(USERS_READ.starts_with(ADMINISTRATORS_ONLY));
    for sddl in [ADMINISTRATORS_ONLY, USERS_READ] {
        let text = wide(std::ffi::OsStr::new(sddl)).unwrap();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: `text` is NUL-terminated and outlives the call; `descriptor` is freed once.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(text.as_ptr()),
                1,
                &mut descriptor,
                None,
            )
            .unwrap();
            let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        }
    }
}

#[test]
fn a_file_is_checked_and_read_through_one_handle_in_a_folder_that_stays() {
    use std::io::Read;
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let folder = root.join("windows-settings");
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join("record.json");
    std::fs::write(&path, "first").unwrap();
    let held = open_unfollowed(&folder, true).unwrap();
    assert_eq!(
        open_file_owner(&held).unwrap(),
        file_owner(&folder).unwrap()
    );
    // The folder checked is the one the file is opened in.
    assert!(std::fs::rename(&folder, root.join("moved")).is_err());
    let mut file = open_unfollowed(&path, false).unwrap();
    assert_eq!(open_file_owner(&file).unwrap(), file_owner(&path).unwrap());
    // Not written while open; a newer record can still take the name, and what was
    // checked is what is read.
    assert!(std::fs::OpenOptions::new().write(true).open(&path).is_err());
    let newer = folder.join("record.tmp");
    std::fs::write(&newer, "second").unwrap();
    std::fs::rename(&newer, &path).unwrap();
    let mut text = String::new();
    file.read_to_string(&mut text).unwrap();
    assert_eq!(text, "first");
    drop((file, held));
    std::fs::rename(&folder, root.join("moved")).unwrap();
    assert_eq!(
        open_unfollowed(&folder, true).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
}

#[test]
fn the_settings_record_writers_are_read_from_its_permissions() {
    use windows::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    let allowed = |sddl: &str, mask: u32| {
        let text = wide(std::ffi::OsStr::new(sddl)).unwrap();
        let mut descriptor = crate::ipc::Descriptor(PSECURITY_DESCRIPTOR::default());
        // SAFETY: `text` is NUL-terminated and outlives the call, and `dacl` points into
        // `descriptor`, which is freed once it drops, after its last use.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(text.as_ptr()),
                1,
                &mut descriptor.0,
                None,
            )
            .unwrap();
            let (mut present, mut defaulted) = (BOOL(0), BOOL(0));
            let mut dacl: *mut ACL = std::ptr::null_mut();
            GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut dacl, &mut defaulted)
                .unwrap();
            assert!(present.as_bool(), "{sddl}");
            dacl_allowed(dacl, mask).unwrap()
        }
    };
    let writers = |sddl: &str| allowed(sddl, WRITE);
    // Who may move a folder's contents: not those who may only add to it, as
    // Authenticated Users may to C:\.
    assert_eq!(
        allowed("D:P(A;;FA;;;SY)(A;;0x4;;;AU)", CHILD_MOVE),
        ["S-1-5-18"]
    );
    for rights in ["WD", "WO", "GA", "GW", "0x40"] {
        assert_eq!(
            allowed(&format!("D:P(A;;{rights};;;AU)"), CHILD_MOVE),
            ["S-1-5-11"],
            "{rights}"
        );
    }
    assert_eq!(writers(USERS_READ), ["S-1-5-18", "S-1-5-32-544"]);
    assert_eq!(writers(ADMINISTRATORS_ONLY), ["S-1-5-18", "S-1-5-32-544"]);
    assert_eq!(
        writers("D:P(A;;FA;;;SY)(A;;FW;;;BU)"),
        ["S-1-5-18", "S-1-5-32-545"]
    );
    // Permissions, ownership and deletion are write access too.
    for rights in ["WD", "WO", "SD", "GA", "GW", "0x40"] {
        assert_eq!(
            writers(&format!("D:P(A;;{rights};;;AU)")),
            ["S-1-5-11"],
            "{rights}"
        );
    }
    // Reading, denials and what applies only inside it allow no writes.
    assert!(writers("D:P(A;;FRFX;;;WD)(D;;FA;;;BU)(A;OICIIO;FA;;;CO)").is_empty());
    // No permissions at all, or an allowing entry of another kind, fail closed.
    assert_eq!(writers("D:NO_ACCESS_CONTROL"), ["S-1-1-0"]);
    assert_eq!(
        writers("D:P(OA;;CC;bf967aba-0de6-11d0-a285-00aa003049e2;;WD)"),
        ["ACE type 5"]
    );
}

#[test]
fn the_settings_record_files_are_made_with_their_permissions() {
    use std::io::{ErrorKind, Write};
    let temporary = tempfile::tempdir().unwrap();
    let folder = temporary.path().join("windows-settings");
    let path = folder.join("record.tmp");
    let elevated = elevated().unwrap();
    let protected = |open: &std::fs::File| {
        assert_eq!(open_file_owner(open).unwrap(), "S-1-5-32-544");
        assert_eq!(
            open_file_writers(open).unwrap(),
            ["S-1-5-18", "S-1-5-32-544"]
        );
    };
    match create_folder_administrators_readable(&folder) {
        Ok(()) => {
            assert!(elevated);
            protected(&open_unfollowed(&folder, true).unwrap());
        }
        // Only an elevated process can make Administrators the owner; nothing is made.
        Err(error) => {
            assert!(!elevated, "{error}");
            assert!(!folder.exists());
            std::fs::create_dir(&folder).unwrap();
        }
    }
    // Anything with the name is left as it is.
    assert_eq!(
        create_folder_administrators_readable(&folder)
            .unwrap_err()
            .kind(),
        ErrorKind::AlreadyExists
    );
    std::fs::write(&path, "planted").unwrap();
    assert_eq!(
        create_file_administrators_readable(&path)
            .unwrap_err()
            .kind(),
        ErrorKind::AlreadyExists
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "planted");
    std::fs::remove_file(&path).unwrap();
    match create_file_administrators_readable(&path) {
        Ok(mut file) => {
            assert!(elevated);
            // Nothing else opens it while it is written.
            assert!(std::fs::File::open(&path).is_err());
            file.write_all(b"record").unwrap();
            drop(file);
            protected(&open_unfollowed(&path, false).unwrap());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "record");
        }
        Err(error) => {
            assert!(!elevated, "{error}");
            assert!(!path.exists());
        }
    }
}

#[test]
fn a_folder_for_administrators_alone_is_made_with_its_permissions() {
    use std::io::ErrorKind;
    // List or read data, all, read.
    const READ: u32 = 0x1 | 0x1000_0000 | 0x8000_0000;
    let temporary = tempfile::tempdir().unwrap();
    let folder = temporary.path().join("nucleus-locks");
    let elevated = elevated().unwrap();
    match create_folder_administrators_only(&folder) {
        Ok(()) => {
            assert!(elevated);
            // Nobody but SYSTEM and Administrators can even open it, from
            // its creation on: the temporary folder lets Users read.
            let open = open_unfollowed(&folder, true).unwrap();
            assert_eq!(open_file_owner(&open).unwrap(), "S-1-5-32-544");
            let mut readers = open_file_allowed(&open, READ).unwrap();
            readers.sort();
            readers.dedup();
            assert_eq!(readers, ["S-1-5-18", "S-1-5-32-544"]);
            assert_eq!(
                open_file_writers(&open).unwrap(),
                ["S-1-5-18", "S-1-5-32-544"]
            );
        }
        // Only an elevated process can make Administrators the owner; nothing is made.
        Err(error) => {
            assert!(!elevated, "{error}");
            assert!(!folder.exists());
            std::fs::create_dir(&folder).unwrap();
        }
    }
    // Anything with the name is left as it is.
    assert_eq!(
        create_folder_administrators_only(&folder)
            .unwrap_err()
            .kind(),
        ErrorKind::AlreadyExists
    );
}

#[test]
fn console_reattachment_refuses_other_users_and_transitional_sessions() {
    assert!(reattach_blocker(4, 1, WTSDisconnected, 2, true, false, false).is_none());
    assert!(reattach_blocker(4, 1, WTSActive, 2, true, false, false).is_none());
    for (source, console, state, protocol, user, occupied) in [
        (4, 1, WTSDisconnected, 2, true, true),
        (4, 1, WTSDisconnected, 2, false, false),
        (4, 1, WTSConnected, 2, true, false),
        (4, 1, WTSDisconnected, 0, true, false),
        (4, u32::MAX, WTSDisconnected, 2, true, false),
        (4, 4, WTSDisconnected, 2, true, false),
        (4, 4, WTSActive, 0, true, false),
        (0, 1, WTSDisconnected, 2, true, false),
    ] {
        assert!(
            reattach_blocker(source, console, state, protocol, user, occupied, false).is_some()
        );
    }
}

#[test]
fn console_sign_in_host_explains_why_return_to_console_is_unavailable() {
    // The installed service follows the console: after Remote Desktop it
    // runs at the sign-in screen, and the reason must say what to do.
    let reason = reattach_blocker(4, 4, WTSConnected, 0, false, false, false).unwrap();
    for text in [
        "sign-in screen",
        "started inside a Remote Desktop session",
        "reconnect Moonlight",
    ] {
        assert!(reason.contains(text), "missing {text} in {reason}");
    }
    // A signed-in console host keeps the existing explanation.
    assert_eq!(
        reattach_blocker(1, 1, WTSActive, 0, true, true, false),
        Some("Rubylight already runs on the physical console.")
    );
}

#[test]
fn occupied_console_requires_a_disconnected_verified_matching_owner() {
    const OWNER: &str = "S-1-5-21-100-200-300-1001";
    const OTHER: &str = "S-1-5-21-100-200-300-1002";
    for (state, source, target, allowed) in [
        (Some(WTSDisconnected), Some(OWNER), Some(OWNER), true),
        (Some(WTSActive), Some(OWNER), Some(OWNER), false),
        (Some(WTSConnected), Some(OWNER), Some(OWNER), false),
        (None, Some(OWNER), Some(OWNER), false),
        (Some(WTSDisconnected), Some(OWNER), Some(OTHER), false),
        (Some(WTSDisconnected), None, Some(OWNER), false),
        (Some(WTSDisconnected), Some(OWNER), None, false),
        (Some(WTSDisconnected), None, None, false),
        (Some(WTSDisconnected), Some(""), Some(""), false),
    ] {
        let verified = same_disconnected_owner(state, source, target);
        for host_state in [WTSActive, WTSDisconnected] {
            assert_eq!(
                reattach_blocker(4, 1, host_state, 2, true, true, verified).is_none(),
                allowed,
                "target state {state:?}, source {source:?}, target {target:?}"
            );
        }
    }
    // An owner match cannot authorize a different source or a changing console.
    for (source, console, state, protocol, signed_in) in [
        (0, 1, WTSDisconnected, 2, true),
        (4, u32::MAX, WTSDisconnected, 2, true),
        (4, 4, WTSDisconnected, 2, true),
        (4, 1, WTSConnected, 2, true),
        (4, 1, WTSDisconnected, 0, true),
        (4, 1, WTSDisconnected, 2, false),
    ] {
        assert!(
            reattach_blocker(source, console, state, protocol, signed_in, true, true).is_some()
        );
    }
}

#[test]
fn console_owner_verification_rechecks_the_target_after_reading_both_tokens() {
    use std::cell::Cell;
    for (final_state, final_console, allowed) in [
        (WTSDisconnected, 1, true),
        (WTSActive, 1, false),
        (WTSDisconnected, 2, false),
    ] {
        let state_reads = Cell::new(0);
        let sid_reads = Cell::new(0);
        let verified = verify_disconnected_console_owner(
            4,
            1,
            |session| {
                assert_eq!(session, 1);
                let read = state_reads.get();
                state_reads.set(read + 1);
                assert_eq!(sid_reads.get(), if read == 0 { 0 } else { 2 });
                Ok(if read == 0 {
                    WTSDisconnected
                } else {
                    final_state
                })
            },
            |session| {
                assert_eq!(state_reads.get(), 1);
                assert_eq!(session, if sid_reads.get() == 0 { 4 } else { 1 });
                sid_reads.set(sid_reads.get() + 1);
                Ok("S-1-5-21-100-200-300-1001".into())
            },
            || {
                assert_eq!(state_reads.get(), 2);
                assert_eq!(sid_reads.get(), 2);
                final_console
            },
        )
        .unwrap();
        assert_eq!(verified, allowed);
        assert_eq!(state_reads.get(), 2);
        assert_eq!(sid_reads.get(), 2);
    }
}

#[test]
fn console_owner_verification_fails_closed_when_either_token_query_fails() {
    use std::cell::Cell;
    for failed_session in [4, 1] {
        let state_reads = Cell::new(0);
        let sid_reads = Cell::new(0);
        let error = verify_disconnected_console_owner(
            4,
            1,
            |session| {
                assert_eq!(session, 1);
                state_reads.set(state_reads.get() + 1);
                Ok(WTSDisconnected)
            },
            |session| {
                sid_reads.set(sid_reads.get() + 1);
                anyhow::ensure!(session != failed_session, "session token unavailable");
                Ok("S-1-5-21-100-200-300-1001".into())
            },
            || panic!("do not accept a console after an identity query failed"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("session token unavailable"));
        assert_eq!(state_reads.get(), 1);
        assert_eq!(sid_reads.get(), if failed_session == 4 { 1 } else { 2 });
    }
}

#[test]
fn streaming_session_rejects_disconnected_rdp_and_session_transitions() {
    for state in [
        WTSConnected,
        WTSDisconnected,
        WTSConnectQuery,
        WTSReset,
        WTSDown,
        WTSInit,
    ] {
        let error = validate_streaming_session(state, false).unwrap_err();
        assert!(error.to_string().contains("disconnected or changing"));
        assert!(error.to_string().contains("RDP"));
    }
    for state in [WTSConnectQuery, WTSReset, WTSDown, WTSInit] {
        assert!(validate_streaming_session(state, true).is_err());
    }
}

#[test]
fn streaming_session_preserves_active_sessions_and_console_sign_in() {
    assert!(validate_streaming_session(WTSActive, false).is_ok());
    for state in [WTSActive, WTSConnected, WTSDisconnected] {
        assert!(validate_streaming_session(state, true).is_ok());
    }
}

#[test]
fn streaming_gate_queries_the_host_session_instead_of_the_console() {
    for (state, allowed) in [(WTSActive, true), (WTSDisconnected, false)] {
        let result = ensure_streaming_session_with(
            || Ok(4),
            || 1,
            |session| {
                assert_eq!(session, 4, "query the host's own RDP session");
                Ok((state, 2))
            },
            |_| panic!("only the console exception lists other sessions"),
        );
        assert_eq!(result.is_ok(), allowed);
    }
    for state in [WTSConnected, WTSDisconnected] {
        assert!(
            ensure_streaming_session_with(|| Ok(1), || 1, |_| Ok((state, 0)), |_| Ok(vec![]))
                .is_ok()
        );
        assert!(
            ensure_streaming_session_with(
                || Ok(1),
                || 1,
                |_| Ok((state, 2)),
                |_| panic!("a refused session needs no session list")
            )
            .is_err()
        );
    }
}

#[test]
fn user_programs_start_in_the_hosts_own_session_not_the_consoles() {
    // A host in session 3 while the console (session 1) is the
    // user's main desktop: its capture worker gets session 3's user.
    let asked = own_session_user(|| Ok(3), Ok).unwrap();
    assert_eq!(asked, 3);
    let error = own_session_user::<u32>(
        || Err(anyhow::anyhow!("no session")),
        |_| panic!("no session, no user token"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("no session"));
}

fn warnings(check: impl FnOnce()) -> String {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("session.log");
    let file = std::fs::File::create(&path).unwrap();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_target(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(move || file.try_clone().unwrap())
        .finish();
    tracing::subscriber::with_default(subscriber, check);
    std::fs::read_to_string(path).unwrap()
}

#[test]
fn refused_streaming_session_is_logged_at_warn_with_the_checked_session() {
    let log = warnings(|| {
        let error = ensure_streaming_session_with(
            || Ok(4),
            || 1,
            |_| Ok((WTSDisconnected, 2)),
            |_| panic!("a refused session needs no session list"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("disconnected or changing"));
    });
    for field in [
        "WARN",
        "session_id=4",
        "console_session_id=1",
        "state=4",
        "protocol=2",
        "allowed=false",
        "decision=\"disconnected_or_changing\"",
    ] {
        assert!(log.contains(field), "missing {field} in {log}");
    }
    // Accepted active sessions stay out of the warning log.
    let log = warnings(|| {
        ensure_streaming_session_with(
            || Ok(1),
            || 1,
            |_| Ok((WTSActive, 0)),
            |_| panic!("an active session needs no session list"),
        )
        .unwrap();
    });
    assert!(log.is_empty(), "{log}");
}

#[test]
fn sign_in_screen_launch_warns_about_a_disconnected_desktop_without_blocking() {
    // Reporter's host: the service runs on console session 4 at the
    // sign-in screen while the RDP desktop waits as disconnected session 1.
    let log = warnings(|| {
        ensure_streaming_session_with(
            || Ok(4),
            || 4,
            |_| Ok((WTSConnected, 0)),
            |host| {
                assert_eq!(host, 4);
                Ok(vec![1])
            },
        )
        .unwrap();
    });
    for field in [
        "WARN",
        "session_id=4",
        "state=1",
        "disconnected_session_ids=[1]",
        "reconnect Moonlight",
    ] {
        assert!(log.contains(field), "missing {field} in {log}");
    }
    // No disconnected desktop, or no session list, keeps the existing
    // sign-in support silent and allowed.
    for list in [Ok(vec![]), Err(anyhow::anyhow!("enumeration failed"))] {
        let log = warnings(|| {
            ensure_streaming_session_with(|| Ok(4), || 4, |_| Ok((WTSConnected, 0)), |_| list)
                .unwrap();
        });
        assert!(log.is_empty(), "{log}");
    }
}

#[test]
fn disconnected_desktops_exclude_services_the_host_and_sessions_without_a_user() {
    let sessions = [
        (0, WTSDisconnected),
        (1, WTSDisconnected),
        (2, WTSDisconnected),
        (3, WTSActive),
        (4, WTSConnected),
        (5, WTSDisconnected),
        (6, WTSDisconnected),
    ];
    let found = disconnected_desktops(6, &sessions, |session| match session {
        1 => Ok(true),
        2 => Ok(false),
        5 => anyhow::bail!("user name unavailable"),
        0 | 6 => panic!("session {session} is never a candidate"),
        _ => panic!("only disconnected sessions are checked, not {session}"),
    });
    assert_eq!(found, vec![1]);
}

#[test]
fn streaming_gate_fails_closed_when_the_host_session_cannot_be_queried() {
    let error = ensure_streaming_session_with(
        || anyhow::bail!("process session unavailable"),
        || panic!("do not fall back to the console session"),
        |_| panic!("do not query a substitute session"),
        |_| panic!("do not list sessions without the host session"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("process session unavailable"));
    let error = ensure_streaming_session_with(
        || Ok(4),
        || 1,
        |session| {
            assert_eq!(session, 4);
            anyhow::bail!("connection state unavailable")
        },
        |_| panic!("do not list sessions without a connection state"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("connection state unavailable"));
}

#[test]
fn the_log_names_a_missing_signed_in_user_once_per_reason() {
    let mut last = None;
    // Found from the start: nothing to say.
    assert_eq!(user_lookup_change(&mut last, None), None);
    let none = "A specified logon session does not exist. (0x80070520)".to_string();
    assert_eq!(
        user_lookup_change(&mut last, Some(none.clone())),
        Some(UserLookup::Missing(none.clone()))
    );
    // The scan every few minutes repeats the same reason: logged once.
    assert_eq!(user_lookup_change(&mut last, Some(none.clone())), None);
    let other = "Access is denied. (0x80070005)".to_string();
    assert_eq!(
        user_lookup_change(&mut last, Some(other.clone())),
        Some(UserLookup::Missing(other))
    );
    assert_eq!(user_lookup_change(&mut last, None), Some(UserLookup::Found));
    assert_eq!(user_lookup_change(&mut last, None), None);
    assert_eq!(
        user_lookup_change(&mut last, Some(none.clone())),
        Some(UserLookup::Missing(none))
    );
}
