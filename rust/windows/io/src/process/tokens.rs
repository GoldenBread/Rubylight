//! Windows access tokens, elevation detection, and target user selection.
use super::{
    owned, raw,
    security::token_user_sid,
    session::{current_session, own_session_user_token},
};
use anyhow::{Context, Result, bail};
use butterpollo_session::nucleus::elevation::Account;
use std::{mem::size_of, os::windows::io::OwnedHandle};
use windows::Win32::{Foundation::*, Security::*, System::Threading::*};

pub(super) fn token() -> Result<OwnedHandle> {
    // SAFETY: `token` is a valid out-pointer and the handle it receives is owned by the caller.
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_SESSIONID,
            &mut token,
        )?;
        Ok(owned(token))
    }
}

pub fn is_system() -> bool {
    let Ok(token) = token() else { return false };
    // SAFETY: `buffer` is usize-aligned and writable for the size passed, and holds a TOKEN_USER
    // whose SID points into it once the call succeeds.
    unsafe {
        let mut buffer = [0usize; 64];
        let mut needed = 0;
        if GetTokenInformation(
            raw(&token),
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            size_of::<[usize; 64]>() as u32,
            &mut needed,
        )
        .is_err()
        {
            return false;
        }
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        IsWellKnownSid(user.User.Sid, WinLocalSystemSid).as_bool()
    }
}

/// The signed-in user's SID (`S-1-5-21-...`), whose registry hive and
/// folders hold per-user program settings. Without the service this is the
/// current user.
pub fn user_sid() -> Option<String> {
    let user_token = if is_system() {
        target_token(&Target::User { elevated: false }).ok()?
    } else {
        None
    };
    let own = token().ok()?;
    let token = user_token.as_ref().unwrap_or(&own);
    token_user_sid(token).ok()
}

/// For an elevated administrator host that is not SYSTEM: the desktop
/// shell's standard-rights token of the same user, to act as that user
/// without administrator rights. None when this host is not elevated, or
/// when the shell is elevated too (User Account Control is off). An error
/// when the shell is not running or belongs to another user, as after an
/// over-the-shoulder elevation.
pub fn standard_user_token() -> Result<Option<OwnedHandle>> {
    let own = token()?;
    if !token_elevated(&own)? {
        return Ok(None);
    }
    shell_token(&own)
}

/// The SID of the account this process runs as (`S-1-5-18` for SYSTEM), unlike [`user_sid`], which
/// names the signed-in user when this process is SYSTEM.
pub fn own_sid() -> Result<String> {
    token_user_sid(&token()?)
}

pub enum Target {
    User {
        elevated: bool,
    },
    /// The signed-in user with their standard rights, only while they are
    /// the account of this SID: the one whose files a launch checked.
    Account(String),
    SystemSession(u32),
}

/// Whether this process runs with administrator rights (elevated, or as SYSTEM).
pub fn elevated() -> Result<bool> {
    token_elevated(&token()?)
}

/// What Windows gives the user signed in to this host's session to start a
/// program elevated with. As the service: their elevation-linked token when
/// it is elevated and theirs; one of another account is the separate
/// administrator of Windows' Administrator protection, which Rubylight never
/// starts a program as. Outside the service, the host's own rights.
pub fn user_account() -> Result<Account> {
    if !is_system() {
        return Ok(match user_runs_elevated()? {
            true => Account::Elevates,
            false => Account::Standard,
        });
    }
    let user = target_token(&Target::User { elevated: false })?
        .context("no signed-in user in the host session")?;
    let linked = match linked_token(&user) {
        Some(linked) => Some((
            token_elevated(&linked)?,
            token_user_sid(&linked)? == token_user_sid(&user)?,
        )),
        None => None,
    };
    Ok(account_of(
        linked,
        token_elevated(&user)?,
        token_in_administrators(&user)?,
    ))
}

/// The account behind a signed-in user's token: `linked`, its
/// elevation-linked token's elevation and whether it is the same account;
/// `elevated`, the token's own; `administrator`, whether its groups name
/// Administrators (for deny only, in an administrator's filtered token).
pub(super) fn account_of(
    linked: Option<(bool, bool)>,
    elevated: bool,
    administrator: bool,
) -> Account {
    match linked {
        // User Account Control's split token.
        Some((true, true)) => Account::Elevates,
        // Administrator protection's separate administrator.
        Some((true, false)) => Account::Protected,
        // User Account Control off: the token is elevated itself.
        _ if elevated => Account::Elevates,
        // An administrator Windows links no elevated token of their own to.
        _ if administrator => Account::Protected,
        _ => Account::Standard,
    }
}

/// The token Windows links to `user`'s for elevation, if it links one.
fn linked_token(user: &OwnedHandle) -> Option<OwnedHandle> {
    let mut linked = TOKEN_LINKED_TOKEN::default();
    let mut bytes = 0;
    // SAFETY: `linked` is a writable TOKEN_LINKED_TOKEN and the size passed
    // is its size; the handle Windows returns is wrapped exactly once.
    unsafe {
        GetTokenInformation(
            raw(user),
            TokenLinkedToken,
            Some((&mut linked as *mut TOKEN_LINKED_TOKEN).cast()),
            size_of::<TOKEN_LINKED_TOKEN>() as u32,
            &mut bytes,
        )
        .ok()?;
    }
    (!linked.LinkedToken.is_invalid()).then(|| owned(linked.LinkedToken))
}

/// Whether `token`'s groups name the Administrators group, enabled or for
/// deny only.
pub(super) fn token_in_administrators(token: &OwnedHandle) -> Result<bool> {
    let mut needed = 0;
    // SAFETY: the first call only asks for the size. The buffer is
    // usize-aligned and as large as Windows asked; the TOKEN_GROUPS in it,
    // its array and its SIDs are read only while it lives.
    unsafe {
        let _ = GetTokenInformation(raw(token), TokenGroups, None, 0, &mut needed);
        anyhow::ensure!(
            needed as usize >= size_of::<TOKEN_GROUPS>(),
            "Windows returned no token groups"
        );
        let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
        GetTokenInformation(
            raw(token),
            TokenGroups,
            Some(buffer.as_mut_ptr().cast()),
            (buffer.len() * size_of::<usize>()) as u32,
            &mut needed,
        )?;
        let groups = &*buffer.as_ptr().cast::<TOKEN_GROUPS>();
        let groups = std::slice::from_raw_parts(groups.Groups.as_ptr(), groups.GroupCount as usize);
        Ok(groups
            .iter()
            .any(|group| IsWellKnownSid(group.Sid, WinBuiltinAdministratorsSid).as_bool()))
    }
}

/// Whether a program started as `Target::User { elevated: true }` runs
/// elevated. A standard user has no elevated token, so it gets the user's
/// standard rights.
pub fn user_runs_elevated() -> Result<bool> {
    match target_token(&Target::User { elevated: true })? {
        Some(token) => token_elevated(&token),
        None => elevated(),
    }
}

pub(crate) fn token_elevated(token: &OwnedHandle) -> Result<bool> {
    let mut info = TOKEN_ELEVATION::default();
    let mut bytes = 0;
    // SAFETY: `info` is a writable TOKEN_ELEVATION and the size passed is its size.
    unsafe {
        GetTokenInformation(
            raw(token),
            TokenElevation,
            Some((&mut info as *mut TOKEN_ELEVATION).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut bytes,
        )?;
    }
    Ok(info.TokenIsElevated != 0)
}

/// The Windows desktop shell is not running, so an elevated administrator
/// host has no standard-rights token of its user. Callers match it with
/// `downcast_ref` to say what the user can do.
#[derive(Debug)]
pub struct NoShell;
impl std::fmt::Display for NoShell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Cannot start a non-elevated program from this administrator host: the Windows desktop shell is not running. Start Rubylight without administrator rights, or use the installed Windows service")
    }
}
impl std::error::Error for NoShell {}

/// The desktop shell's primary token, for starting a non-elevated program
/// from an elevated administrator process that is not SYSTEM. It must belong
/// to the same user. None when the shell is elevated itself (User Account
/// Control is off): there is no standard-rights token, and the program runs
/// with the rights the user's own desktop gives it.
pub(super) fn shell_token(own: &OwnedHandle) -> Result<Option<OwnedHandle>> {
    use windows::Win32::UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId};
    // SAFETY: the shell window is only used to read its process ID. The
    // process and token handles are owned once and closed on drop, and the
    // out-pointers are live locals.
    let primary = unsafe {
        let shell = GetShellWindow();
        if shell.is_invalid() {
            return Err(NoShell.into());
        }
        let mut pid = 0;
        GetWindowThreadProcessId(shell, Some(&mut pid));
        if pid == 0 {
            return Err(NoShell.into());
        }
        let process = owned(
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
                .context("Cannot open the Windows desktop shell")?,
        );
        let mut token = HANDLE::default();
        OpenProcessToken(raw(&process), TOKEN_DUPLICATE, &mut token)
            .context("Cannot read the Windows desktop shell's user token")?;
        let token = owned(token);
        let mut primary = HANDLE::default();
        DuplicateTokenEx(
            raw(&token),
            TOKEN_QUERY
                | TOKEN_DUPLICATE
                | TOKEN_IMPERSONATE
                | TOKEN_ASSIGN_PRIMARY
                | TOKEN_ADJUST_DEFAULT
                | TOKEN_ADJUST_SESSIONID,
            None,
            SecurityImpersonation,
            TokenPrimary,
            &mut primary,
        )?;
        owned(primary)
    };
    anyhow::ensure!(
        token_user_sid(&primary)? == token_user_sid(own)?,
        "Cannot start a non-elevated program: the Windows desktop shell belongs to another user"
    );
    Ok((!token_elevated(&primary)?).then_some(primary))
}

/// The user a `Target::User` program runs as: the one signed in to this
/// host's own session, never the console's. A host in another session
/// starts its capture worker (and games) on that session's desktop, not on
/// the main desktop the user is working on.
pub(super) fn own_session_user<T>(
    current: impl FnOnce() -> Result<u32>,
    user: impl FnOnce(u32) -> Result<T>,
) -> Result<T> {
    user(current()?)
}

/// The token a program started as `target` runs with; None: this process's
/// own. Only ever the signed-in user's own account: a [`Target::Account`]
/// start is refused once another account is signed in.
pub(super) fn target_token(target: &Target) -> Result<Option<OwnedHandle>> {
    let token = session_token(target)?;
    if let Target::Account(expected) = target {
        let found = match &token {
            Some(token) => token_user_sid(token)?,
            None => own_sid()?,
        };
        same_account(expected, &found)?;
    }
    Ok(token)
}

/// A start for the account `expected` ([`Target::Account`]) goes ahead only
/// as that account: `found` is the one whose token it would run with.
pub(super) fn same_account(expected: &str, found: &str) -> Result<()> {
    anyhow::ensure!(
        expected == found,
        "another account signed in on this PC since the launch began: start it again"
    );
    Ok(())
}

/// An elevated start runs only as the signed-in user's own account: `user`
/// is the SID of their token, `linked` that of the elevation-linked token
/// Windows gives for it. Under Windows' Administrator protection that is a
/// separate administrator account, which Rubylight never starts a program
/// as.
pub(super) fn own_linked_account(user: &str, linked: &str) -> Result<()> {
    anyhow::ensure!(
        user == linked,
        "Starting it elevated needs administrator rights, and {}",
        butterpollo_session::nucleus::elevation::PROTECTED
    );
    Ok(())
}

fn session_token(target: &Target) -> Result<Option<OwnedHandle>> {
    if !is_system() {
        if matches!(target, Target::SystemSession(_)) {
            bail!("SYSTEM session creation requires the Windows service");
        }
        let token = token()?;
        let elevated = token_elevated(&token)?;
        if matches!(target, Target::User { elevated: true }) && !elevated {
            bail!("this elevated command requires the installed Windows service");
        }
        if matches!(
            target,
            Target::User { elevated: false } | Target::Account(_)
        ) && elevated
        {
            // The elevation-linked token is only an identification token for
            // a caller without SeTcbPrivilege, which Windows refuses for
            // process creation. The desktop shell holds the usable one.
            return shell_token(&token);
        }
        return Ok(None);
    }
    // SAFETY: every out-pointer is a live local whose size matches the one passed, and each
    // returned token handle is wrapped in OwnedHandle exactly once.
    unsafe {
        let source = match target {
            Target::SystemSession(_) => token()?,
            Target::User { .. } | Target::Account(_) => {
                let mut user = own_session_user(current_session, own_session_user_token)?;
                if matches!(target, Target::User { elevated: true })
                    && let Some(linked) = linked_token(&user)
                {
                    own_linked_account(&token_user_sid(&user)?, &token_user_sid(&linked)?)?;
                    user = linked;
                }
                user
            }
        };
        let mut primary = HANDLE::default();
        DuplicateTokenEx(
            raw(&source),
            TOKEN_ALL_ACCESS,
            None,
            SecurityImpersonation,
            TokenPrimary,
            &mut primary,
        )?;
        let primary = owned(primary);
        if let Target::SystemSession(session) = target {
            SetTokenInformation(
                raw(&primary),
                TokenSessionId,
                (session as *const u32).cast(),
                4,
            )?;
        }
        Ok(Some(primary))
    }
}
