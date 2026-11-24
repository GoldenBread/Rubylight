//! User token lookup, thread impersonation, and known-folder resolution.
use super::UserFolderError;
use std::{
    ffi::OsString,
    io,
    marker::PhantomData,
    os::windows::{
        ffi::OsStringExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::PathBuf,
};
use windows::{
    Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_NO_TOKEN, HANDLE},
        Security::{ImpersonateLoggedOnUser, RevertToSelf},
        System::{Com::CoTaskMemFree, RemoteDesktop::WTSQueryUserToken},
        UI::Shell::{
            FOLDERID_Downloads, FOLDERID_RoamingAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
        },
    },
    core::{GUID, HRESULT},
};

/// A Win32 failure as the `io::Error` std would report, so callers can
/// match its kind (`StorageFull`, `AlreadyExists`, ...).
pub(crate) fn os_error(error: windows::core::Error) -> io::Error {
    let code = error.code().0 as u32;
    if code & 0xFFFF_0000 == 0x8007_0000 {
        io::Error::from_raw_os_error((code & 0xFFFF) as i32)
    } else {
        io::Error::other(error)
    }
}

/// This thread runs as the user until the guard drops, also when the work
/// fails or panics. Not `Send`: the impersonation belongs to this thread.
pub(crate) struct Impersonation(PhantomData<*const ()>);
impl Impersonation {
    pub(crate) fn begin(token: Option<&OwnedHandle>) -> io::Result<Option<Self>> {
        let Some(token) = token else { return Ok(None) };
        // SAFETY: token is an open primary token of the signed-in user; the
        // returned guard reverts the impersonation on this thread.
        unsafe { ImpersonateLoggedOnUser(HANDLE(token.as_raw_handle())) }.map_err(os_error)?;
        Ok(Some(Self(PhantomData)))
    }
}
impl Drop for Impersonation {
    fn drop(&mut self) {
        // SAFETY: ends the impersonation `begin` started on this thread.
        if let Err(error) = unsafe { RevertToSelf() } {
            tracing::error!("The host could not stop acting as the Windows user: {error}");
            // This thread would go on with the user's rights in place of the
            // host's, or with the host's where the user's were checked.
            std::process::abort();
        }
    }
}

/// The token files are opened with: the user signed in to the host's
/// session when the host runs as SYSTEM, the same user without
/// administrator rights when it runs elevated, and none when the host
/// already is that user.
pub(crate) fn user_token() -> Result<Option<OwnedHandle>, UserFolderError> {
    if !crate::process::is_system() {
        return standard_user(crate::process::standard_user_token());
    }
    let session = crate::process::current_session()
        .map_err(|error| io::Error::other(format!("{error:#}")))?;
    let mut token = HANDLE::default();
    // SAFETY: token is a writable handle slot.
    let queried = unsafe { WTSQueryUserToken(session, &mut token) };
    match queried {
        // SAFETY: WTS returned an open token that only this OwnedHandle closes.
        Ok(()) => Ok(Some(unsafe { OwnedHandle::from_raw_handle(token.0) })),
        Err(error) if no_signed_in_user(error.code()) => Err(UserFolderError::NoConsoleUser),
        Err(error) => Err(os_error(error).into()),
    }
}

/// The elevated host's token of its user without administrator rights, with
/// a missing desktop shell as [`UserFolderError::NoUserShell`].
pub(super) fn standard_user(
    token: anyhow::Result<Option<OwnedHandle>>,
) -> Result<Option<OwnedHandle>, UserFolderError> {
    token.map_err(|error| {
        if error.downcast_ref::<crate::process::NoShell>().is_some() {
            return UserFolderError::NoUserShell;
        }
        io::Error::other(format!(
            "file transfer cannot act as the Windows user without administrator rights: {error:#}"
        ))
        .into()
    })
}

/// Runs `work` on this thread as the user (see [`user_token`]), so Windows
/// checks what it opens, follows and replaces against the user's rights, not
/// the host's: a link the user put in a folder of theirs leads nowhere the
/// user could not reach anyway. With nobody to act as, `work` does not run.
pub fn as_user<T>(work: impl FnOnce() -> T) -> Result<T, UserFolderError> {
    as_token(user_token()?.as_ref(), work)
}

/// [`as_user`] with `token`, or as the host without one. The impersonation
/// ends when `work` returns or panics.
pub(crate) fn as_token<T>(
    token: Option<&OwnedHandle>,
    work: impl FnOnce() -> T,
) -> Result<T, UserFolderError> {
    let _user = Impersonation::begin(token)?;
    Ok(work())
}

/// Runs `work` on this thread as the user signed in to the host's session
/// when the host runs as SYSTEM, for what Windows keeps per user. Otherwise,
/// or with nobody signed in, it runs as the host.
pub fn as_session_user<T>(work: impl FnOnce() -> T) -> io::Result<T> {
    let token = if crate::process::is_system() {
        match user_token() {
            Ok(token) => token,
            Err(UserFolderError::NoConsoleUser) => None,
            Err(UserFolderError::Io(error)) => return Err(error),
            Err(error) => return Err(io::Error::other(error.to_string())),
        }
    } else {
        None
    };
    let _user = Impersonation::begin(token.as_ref())?;
    Ok(work())
}

/// How `WTSQueryUserToken` says the session has no user: ERROR_NO_TOKEN
/// at the sign-in screen, ERROR_FILE_NOT_FOUND when the session is gone.
pub(crate) fn no_signed_in_user(code: HRESULT) -> bool {
    code == HRESULT::from_win32(ERROR_NO_TOKEN.0)
        || code == HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0)
}

/// The user's Downloads folder, wherever they moved it.
pub(super) fn downloads(token: Option<&OwnedHandle>) -> Result<PathBuf, UserFolderError> {
    known_folder(&FOLDERID_Downloads, token)
}

/// The roaming AppData folder of the user signed in to the host's session
/// (see [`user_token`]), where Nucleus Co-op keeps the user's game list.
pub fn roaming_app_data() -> Result<PathBuf, UserFolderError> {
    let token = user_token()?;
    known_folder(&FOLDERID_RoamingAppData, token.as_ref())
}

/// The known folder `id` of the user `token` names, or of the host's user.
pub(crate) fn known_folder(
    id: &GUID,
    token: Option<&OwnedHandle>,
) -> Result<PathBuf, UserFolderError> {
    let token = token.map(|token| HANDLE(token.as_raw_handle()));
    // SAFETY: the token stays open for the call; the string Windows returns
    // is copied, then freed once.
    unsafe {
        let path = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, token).map_err(os_error)?;
        let copy = OsString::from_wide(path.as_wide());
        CoTaskMemFree(Some(path.0 as *const _));
        Ok(PathBuf::from(copy))
    }
}
