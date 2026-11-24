//! Child-session state restoration and saved password cleanup during uninstall.
use crate::{log::line, system, ui::Progress};
use anyhow::{Result, bail};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY};

/// The record child-session setup leaves
/// (`butterpollo_session::child_session::SETUP_FILE`), in its folder beside the
/// profile (`SETUP_FOLDER`). One of that name in the profile, which its user
/// can write, is never read.
pub(super) const CHILD_SESSION_SETUP_FILE: &str = "child_session_setup.json";
pub(super) const CHILD_SESSION_SETUP_FOLDER: &str = "windows-settings";
/// The host's flag that puts back what that setup changed
/// (`butterpollo_session::child_session::RESTORE_FLAG`).
pub(super) const CHILD_SESSION_RESTORE_FLAG: &str = "--child-session-restore";
/// Its exit code when the restore failed after putting Remote Desktop back
/// as it was before setup: RDP is the user's own again and stays as it is
/// (`butterpollo_session::child_session::RESTORE_PARTLY_EXIT`).
pub(super) const CHILD_SESSION_RESTORE_PARTLY_EXIT: i32 = 4;
/// The PC's child-session setup record
/// (`butterpollo_session::child_session::setup_record`), beside `profile`.
pub(super) fn child_session_record(profile: &Path) -> PathBuf {
    profile
        .parent()
        .unwrap_or(profile)
        .join(CHILD_SESSION_SETUP_FOLDER)
        .join(CHILD_SESSION_SETUP_FILE)
}
pub(super) fn child_session_recorded(profile: &Path) -> bool {
    child_session_record(profile).is_file()
}
/// The installed host when child-session setup left a record for `profile`
/// for it to restore; None when there is nothing to put back or no host to
/// do it.
pub(super) fn child_session_restore_pending(profile: &Path, install: &Path) -> Option<PathBuf> {
    let program = install.join("butterpollo.exe");
    (child_session_recorded(profile) && program.is_file()).then_some(program)
}

/// Put back the Windows settings the removed child-session setup changed, through the
/// installed host; the reason when they were not restored and its record
/// is still needed, with what was done about Remote Desktop. None when there
/// was nothing to restore or it worked.
pub(super) fn restore_child_session(install: &Path, progress: &Progress) -> Option<String> {
    restore_child_session_with(
        &crate::install::profile(),
        install,
        &mut NativeRegistry,
        child_session_block_rule_present,
        |program| {
            progress.set("Restoring the Windows settings child sessions changed…");
            // The record is the PC's, not the profile's: no --config-dir.
            system::run(
                &program.display().to_string(),
                &[CHILD_SESSION_RESTORE_FLAG],
                Duration::from_secs(180),
            )
        },
    )
}
/// [`restore_child_session`] for `profile`, running the host with `run`.
/// Whatever keeps the settings from coming back, Remote Desktop is turned
/// off here too: a host that never ran (missing, not starting), timed out
/// or crashed did not turn it off itself. Without a record, so does setup's
/// block rule that `block_rule` finds still there ([`child_session_unrecorded`]).
/// Except after a restore that put RDP back as the user had it and failed on
/// the rest ([`CHILD_SESSION_RESTORE_PARTLY_EXIT`]): RDP is left as it is.
pub(super) fn restore_child_session_with(
    profile: &Path,
    install: &Path,
    registry: &mut dyn MachineRegistry,
    block_rule: impl FnOnce() -> Result<bool>,
    run: impl FnOnce(&Path) -> Result<(i32, String)>,
) -> Option<String> {
    let problem = match child_session_restore_pending(profile, install) {
        None if !child_session_recorded(profile) => {
            child_session_unrecorded(&*registry, block_rule)?
        }
        None => format!("{} is missing", install.join("butterpollo.exe").display()),
        Some(program) => match run(&program) {
            Ok((0, _)) => return None,
            // RDP is back as the user had it, and a later restore skips it: turned off now,
            // it would stay off.
            Ok((CHILD_SESSION_RESTORE_PARTLY_EXIT, output)) => {
                return Some(format!(
                    "The Windows settings the removed child-session feature changed were partly restored: {}. Remote Desktop was left as it is.",
                    output.trim().trim_end_matches('.')
                ));
            }
            Ok((code, output)) => format!("({code}) {}", output.trim()),
            Err(error) => format!("{error:#}"),
        },
    };
    Some(format!(
        "The Windows settings the removed child-session feature changed (Remote Desktop, its firewall rules, child sessions) were not restored: {}. {}.",
        problem.trim().trim_end_matches('.'),
        turn_rdp_off(registry)
    ))
}
/// The block rule child-session setup adds, as `"<name> (TCP)"` and
/// `"<name> (UDP)"` (`butterpollo_session::child_session::BLOCK_RULE`).
pub(super) const CHILD_SESSION_BLOCK_RULE: &str = "Rubylight child session: RDP only on this PC";
/// Without a record, as the host's `enforce_unrecorded`: Remote Desktop on
/// while setup's block rule is still there (`block_rule`) was turned on by a
/// setup whose record is gone, and nothing restores it; the problem to say
/// before RDP is turned off. None with RDP off, or without the rule: RDP is
/// then the user's own. A rule that can't be read changes nothing, said in
/// the log.
pub(super) fn child_session_unrecorded(
    registry: &dyn MachineRegistry,
    block_rule: impl FnOnce() -> Result<bool>,
) -> Option<String> {
    if let Ok(Some(1)) = registry.read_dword(TERMINAL_SERVER, DENY_TS_CONNECTIONS) {
        return None;
    }
    match block_rule() {
        Ok(true) => Some(format!(
            "their record is gone, but the firewall rule \"{CHILD_SESSION_BLOCK_RULE}\" setup adds is still there"
        )),
        Ok(false) => None,
        Err(error) => {
            line(format!(
                "warning: whether the firewall rule \"{CHILD_SESSION_BLOCK_RULE}\" child-session setup adds is still there could not be read, so Remote Desktop was left as it is: {error:#}"
            ));
            None
        }
    }
}
/// Whether either of [`CHILD_SESSION_BLOCK_RULE`]'s rules is on this PC.
pub(super) fn child_session_block_rule_present() -> Result<bool> {
    for protocol in ["TCP", "UDP"] {
        if system::firewall_rule_present(&format!("{CHILD_SESSION_BLOCK_RULE} ({protocol})"))? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Where the removed child-session feature saved the account's Windows
/// password, encrypted with DPAPI, under each `AppData\Local`, and the file a
/// save cut short left (`butterpollo_session::child_session::PASSWORD_FOLDER`
/// and `PASSWORD_FILES`).
pub(super) const CHILD_SESSION_PASSWORD_FOLDER: &str = "Rubylight";
pub(super) const CHILD_SESSION_PASSWORD_FILES: [&str; 2] =
    ["child-session-password.bin", "child-session-password.tmp"];
/// The accounts' profile folders, each a `ProfileImagePath`.
const PROFILE_LIST: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList";
/// Delete the saved child-session passwords in every profile (ProfileList's
/// `S-1-5-21-*` `ProfileImagePath`s) and in this account's own
/// `%LOCALAPPDATA%`; what could not be deleted, each with its path.
pub(super) fn delete_child_session_passwords() -> Vec<String> {
    let profiles: Vec<PathBuf> = system::subkeys(HKEY_LOCAL_MACHINE, PROFILE_LIST, KEY_WOW64_64KEY)
        .into_iter()
        .filter(|sid| sid.starts_with("S-1-5-21-"))
        .filter_map(|sid| {
            system::registry_string(
                HKEY_LOCAL_MACHINE,
                &format!(r"{PROFILE_LIST}\{sid}"),
                "ProfileImagePath",
            )
        })
        .map(|path| PathBuf::from(expand_environment(&path)))
        .collect();
    let local: Vec<PathBuf> = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .into_iter()
        .collect();
    let (deleted, failed) =
        delete_child_session_passwords_in(&profiles, &local, wipe_file, |folder| {
            match std::fs::remove_dir(folder) {
                Err(error) if error.kind() != std::io::ErrorKind::DirectoryNotEmpty => Err(error),
                _ => Ok(()),
            }
        });
    for path in deleted {
        line(format!(
            "deleted the child-session password Rubylight had saved (DPAPI-encrypted) at {}",
            path.display()
        ));
    }
    failed
}
/// `%NAME%` in a REG_EXPAND_SZ value, from this process's environment; a
/// name it doesn't have stays as it is.
pub(super) fn expand_environment(text: &str) -> String {
    let mut expanded = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        let Some(length) = rest[start + 1..].find('%') else {
            break;
        };
        let name = &rest[start + 1..start + 1 + length];
        expanded.push_str(&rest[..start]);
        match std::env::var(name) {
            Ok(value) if !name.is_empty() => expanded.push_str(&value),
            _ => expanded.push_str(&rest[start..start + length + 2]),
        }
        rest = &rest[start + length + 2..];
    }
    expanded.push_str(rest);
    expanded
}
/// [`delete_child_session_passwords`] in each `AppData\Local` under
/// `profiles`, and in each of `local`: a path that is not absolute or holds
/// `..` is skipped, each folder once. Each file through `wipe` (Ok(false)
/// when there is none), then each folder a file went from through
/// `remove_empty`. The files deleted, and each failure with its path; one
/// failure stops none of the others.
pub(super) fn delete_child_session_passwords_in(
    profiles: &[PathBuf],
    local: &[PathBuf],
    mut wipe: impl FnMut(&Path) -> std::io::Result<bool>,
    mut remove_empty: impl FnMut(&Path) -> std::io::Result<()>,
) -> (Vec<PathBuf>, Vec<String>) {
    let usable = |path: &Path| {
        path.is_absolute()
            && !path
                .components()
                .any(|part| part == std::path::Component::ParentDir)
    };
    let mut folders: Vec<PathBuf> = Vec::new();
    for folder in profiles
        .iter()
        .filter(|profile| usable(profile))
        .map(|profile| profile.join("AppData").join("Local"))
        .chain(local.iter().filter(|local| usable(local)).cloned())
        .map(|local| local.join(CHILD_SESSION_PASSWORD_FOLDER))
    {
        let key = folder.to_string_lossy().to_lowercase();
        if !folders
            .iter()
            .any(|known| known.to_string_lossy().to_lowercase() == key)
        {
            folders.push(folder);
        }
    }
    let (mut deleted, mut failed) = (Vec::new(), Vec::new());
    for folder in folders {
        let mut emptied = false;
        for name in CHILD_SESSION_PASSWORD_FILES {
            let path = folder.join(name);
            match wipe(&path) {
                Ok(true) => {
                    deleted.push(path);
                    emptied = true;
                }
                Ok(false) => {}
                Err(error) => failed.push(format!(
                    "The child-session password Rubylight had saved at {} could not be deleted: {error}; delete it by hand.",
                    path.display()
                )),
            }
        }
        if emptied && let Err(error) = remove_empty(&folder) {
            failed.push(format!(
                "The empty folder {} could not be removed: {error}.",
                folder.display()
            ));
        }
    }
    (deleted, failed)
}
/// Overwrite the file at `path` with zeros, flush it to disk and delete it;
/// false when there is none. Opened without following a link, and refused
/// with nothing written when the handle is not `path` itself (a link or
/// junction on the way led elsewhere).
pub(super) fn wipe_file(path: &Path) -> std::io::Result<bool> {
    use std::io::Write;
    use std::os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    };
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_NAME_NORMALIZED, GetFinalPathNameByHandleW,
        },
    };
    let mut file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    if metadata.file_attributes() & 0x400 != 0 || !metadata.is_file() {
        return Err(std::io::Error::other(
            "it is not a plain file; nothing was changed",
        ));
    }
    let mut buffer = vec![0u16; 32768];
    // SAFETY: `file` stays open for the call and `buffer` is writable for its length.
    let length = unsafe {
        GetFinalPathNameByHandleW(
            HANDLE(file.as_raw_handle()),
            &mut buffer,
            FILE_NAME_NORMALIZED,
        )
    } as usize;
    if length == 0 || length > buffer.len() {
        return Err(std::io::Error::last_os_error());
    }
    let opened = String::from_utf16_lossy(&buffer[..length]);
    let opened = opened.strip_prefix(r"\\?\").unwrap_or(&opened);
    if !opened.eq_ignore_ascii_case(&path.display().to_string()) {
        return Err(std::io::Error::other(format!(
            "it opens as {opened}, through a link on the way; nothing was changed"
        )));
    }
    let zeros = [0u8; 64 * 1024];
    let mut left = metadata.len();
    while left > 0 {
        let chunk = left.min(zeros.len() as u64) as usize;
        file.write_all(&zeros[..chunk])?;
        left -= chunk as u64;
    }
    file.sync_all()?;
    drop(file);
    std::fs::remove_file(path)?;
    Ok(true)
}

/// Where Remote Desktop is switched (`fDenyTSConnections`), and the group
/// policy that wins over it (`butterpollo_session::child_session`'s
/// `TERMINAL_SERVER` and `TERMINAL_SERVICES_POLICY`).
pub(super) const TERMINAL_SERVER: &str = r"SYSTEM\CurrentControlSet\Control\Terminal Server";
pub(super) const TERMINAL_SERVICES_POLICY: &str =
    r"SOFTWARE\Policies\Microsoft\Windows NT\Terminal Services";
pub(super) const DENY_TS_CONNECTIONS: &str = "fDenyTSConnections";
/// HKLM's DWORD values, all [`turn_rdp_off`] needs: [`NativeRegistry`] on
/// this PC, a fake in tests.
pub(super) trait MachineRegistry {
    fn read_dword(&self, path: &str, name: &str) -> Result<Option<u32>>;
    fn write_dword(&mut self, path: &str, name: &str, value: u32) -> Result<()>;
}
pub(super) struct NativeRegistry;
impl MachineRegistry for NativeRegistry {
    fn read_dword(&self, path: &str, name: &str) -> Result<Option<u32>> {
        system::machine_dword(path, name)
    }
    fn write_dword(&mut self, path: &str, name: &str, value: u32) -> Result<()> {
        system::write_key(
            HKEY_LOCAL_MACHINE,
            path,
            &[(name, system::Value::Number(value))],
        )
    }
}
/// Turn Remote Desktop off (`fDenyTSConnections = 1`), read it back, and
/// check that no group policy keeps it on, as the host does
/// (`butterpollo_session::child_session::turn_rdp_off`). What was done, for
/// the note.
pub(super) fn turn_rdp_off(registry: &mut dyn MachineRegistry) -> String {
    let off = registry
        .write_dword(TERMINAL_SERVER, DENY_TS_CONNECTIONS, 1)
        .and_then(
            |()| match registry.read_dword(TERMINAL_SERVER, DENY_TS_CONNECTIONS)? {
                Some(1) => Ok(()),
                now => bail!("it reads {now:?} after writing 1"),
            },
        )
        .and_then(
            |()| match registry.read_dword(TERMINAL_SERVICES_POLICY, DENY_TS_CONNECTIONS)? {
                Some(0) => bail!(
                    "group policy keeps Remote Desktop on ({DENY_TS_CONNECTIONS} = 0 under HKLM\\{TERMINAL_SERVICES_POLICY}), which wins over this setting"
                ),
                _ => Ok(()),
            },
        );
    match off {
        Ok(()) => format!(
            "Remote Desktop was turned off ({DENY_TS_CONNECTIONS} = 1), so it is not open to the network"
        ),
        Err(error) => format!(
            "Turning Remote Desktop off failed ({DENY_TS_CONNECTIONS}: {error:#}): turn it off in Windows' settings (System, Remote Desktop) now"
        ),
    }
}
