//! The signed-in user's own copy of Nucleus Co-op, on Linux: none is ever made. The host
//! runs as its user here and has no token of another account, so only work "as the host
//! itself" runs.
use anyhow::Result;
use butterpollo_session::nucleus::generic::Setup;
use butterpollo_session::nucleus::handler::Handler;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};

/// What every step that needs the user's copy answers with on Linux.
fn no_copy() -> anyhow::Error {
    crate::unsupported("Nucleus Co-op's copy for the signed-in user")
}

/// The signed-in user's token, which their copy is written and run as. Always an error on
/// Linux, where the file descriptor only stands in for the Windows token.
pub fn user() -> Result<OwnedFd> {
    Err(no_copy())
}

/// The SID of the user of `token`. Always an error on Linux, which has neither.
pub fn sid(_token: &OwnedFd) -> Result<String> {
    Err(no_copy())
}

/// Runs `work` as the user of `token`. Without a token it runs as the host itself, as on
/// Windows; with one it is an error on Linux and `work` does not run.
pub fn as_user<T>(token: Option<&OwnedFd>, work: impl FnOnce() -> T) -> Result<T> {
    match token {
        None => Ok(work()),
        Some(_) => Err(crate::unsupported("Acting as the signed-in user")),
    }
}

/// The user `sid`'s own copy of the host's Nucleus folder `source`. Always an error on
/// Linux: no copy is made.
pub fn ensure(_data: &Path, _source: &Path, _sid: &str, _token: &OwnedFd) -> Result<PathBuf> {
    Err(no_copy())
}

/// Write Rubylight's one-player handler into the host's folder and the user's copy.
/// Always an error on Linux: nothing is written.
pub fn write_own(
    _folder: &Path,
    _copy: &Path,
    _setup: &Setup,
    _token: Option<&OwnedFd>,
) -> Result<Handler> {
    Err(no_copy())
}

/// Remove the game `guid` from the list of the user's copy at `copy`. Always an error on
/// Linux: nothing is removed.
pub fn remove_game(_copy: &Path, _guid: &str, _token: Option<&OwnedFd>) -> Result<()> {
    Err(no_copy())
}
