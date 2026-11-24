//! What a game's .exe asks of Windows to start, on Linux: its manifest, the compatibility
//! settings and the folder's permissions are Windows' and are not read here. Whether the
//! .exe imports DirectInput is read from the file, as on Windows.
use anyhow::{Context, Result};
use butterpollo_session::nucleus::elevation::Need;
use std::os::fd::OwnedFd;
use std::path::Path;

/// What the game at `exe` needs to start, for the user `sid` of `token`. Always an error
/// on Linux, which reads no Windows manifest or compatibility setting.
pub fn game_need(_exe: &Path, _sid: &str, _token: Option<&OwnedFd>) -> Result<Need> {
    Err(crate::unsupported(
        "Reading what a game asks of Windows to start",
    ))
}

/// Whether the user of `token` may change the files in `folder` as Nucleus Co-op does.
/// Always an error on Linux, where the folder is not walked.
pub fn folder_need(_folder: &Path, _token: Option<&OwnedFd>) -> Result<Need> {
    Err(crate::unsupported(
        "Checking a game folder for Nucleus Co-op",
    ))
}

/// Whether the game at `exe` imports DirectInput
/// ([`butterpollo_session::nucleus::imports`]), read as the user of `token` (None: as the
/// host itself; a token is an error on Linux).
pub fn reads_direct_input(exe: &Path, token: Option<&OwnedFd>) -> Result<bool> {
    super::user_copy::as_user(token, || -> Result<bool> {
        let file = std::fs::File::open(exe)?;
        let dlls = butterpollo_session::nucleus::imports::imported_dlls(
            &mut std::io::BufReader::new(file),
        )?;
        tracing::info!(
            exe = %exe.display(),
            pads = ?butterpollo_session::nucleus::imports::pad_dlls(&dlls),
            "Nucleus Co-op: the DLLs the game imports to read a pad"
        );
        Ok(butterpollo_session::nucleus::imports::direct_input(&dlls))
    })
    .unwrap_or_else(Err)
    .with_context(|| format!("reading the DLLs {} imports", exe.display()))
}
