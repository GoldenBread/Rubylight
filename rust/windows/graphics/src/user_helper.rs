//! Traits for launching user-session helpers and talking over private local pipes without coupling graphics to `butterpollo-win-io`.
use anyhow::{Result, ensure};
use serde::{Serialize, de::DeserializeOwned};
use std::{ffi::OsString, fs::File, os::windows::io::OwnedHandle, path::Path, time::Duration};

/// Maximum serialized helper IPC message size in bytes.
pub const MESSAGE_LIMIT: usize = 4096;

/// Private local named pipe endpoint for a user-session helper.
pub trait HelperPipe: Send + Sync {
    fn connected(&self, expected_pid: u32) -> Result<bool>;
    fn send_bytes(&self, bytes: &[u8]) -> Result<()>;
    fn receive_bytes(&self) -> Result<Option<Vec<u8>>>;
}

/// Owned child process running in the signed-in user's session.
pub trait UserChild: Send + Sync {
    fn pid(&self) -> u32;
    fn exit_code(&self) -> Result<Option<u32>>;
    fn wait(&self, timeout: Duration) -> Result<u32>;
    fn terminate(&self) -> Result<()>;
    fn duplicate_into(&self, raw_handle: usize) -> Result<u64>;
    fn duplicate_resource(&self, raw_handle: usize) -> Result<OwnedHandle>;
}

/// User-session child started with redirected stdio pipes.
pub struct PipedUserChild {
    pub process: Box<dyn UserChild>,
    pub stdin: Option<File>,
    pub stdout: Option<File>,
    pub stderr: Option<File>,
}

/// Host-supplied launcher for spawning user-session helpers and opening private helper pipes.
pub trait UserLauncher: Send + Sync {
    fn pipe_server(&self, prefix: &str) -> Result<(Box<dyn HelperPipe>, String)>;
    fn pipe_client(&self, name: &str, parent: u32, prefix: &str) -> Result<Box<dyn HelperPipe>>;
    fn spawn_user(&self, program: &Path, args: &[OsString]) -> Result<Box<dyn UserChild>>;
    fn spawn_user_piped(&self, program: &Path, args: &[OsString]) -> Result<PipedUserChild>;
    fn user_sid(&self) -> Option<String>;
}

pub(crate) trait HelperPipeExt {
    fn send_json<T: Serialize>(&self, message: &T) -> Result<()>;
    fn receive_json<T: DeserializeOwned>(&self) -> Result<Option<T>>;
}
impl<P: HelperPipe + ?Sized> HelperPipeExt for P {
    fn send_json<T: Serialize>(&self, message: &T) -> Result<()> {
        let bytes = serde_json::to_vec(message)?;
        ensure!(
            bytes.len() <= MESSAGE_LIMIT,
            "helper message exceeds its bound"
        );
        self.send_bytes(&bytes)
    }
    fn receive_json<T: DeserializeOwned>(&self) -> Result<Option<T>> {
        let Some(bytes) = self.receive_bytes()? else {
            return Ok(None);
        };
        ensure!(bytes.len() <= MESSAGE_LIMIT, "oversized helper message");
        Ok(Some(serde_json::from_slice(&bytes)?))
    }
}
