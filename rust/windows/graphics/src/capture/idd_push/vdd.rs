//! Decoupled Rubylight virtual display driver bridge and WUDFHost handle duplicator.
use anyhow::{Context, Result};
use rubylight_vdisplay_proto::control::{MonitorStatus, SetFenceRingRequest, SetFrameRingRequest};
use windows::Win32::Foundation::{
    CloseHandle, DUPLICATE_CLOSE_SOURCE, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE,
    WAIT_OBJECT_0,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, PROCESS_DUP_HANDLE, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};

/// A monitor owned by the Rubylight virtual display driver, decoupled from `butterpollo-win-display`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VddOwned {
    pub session_id: u64,
    pub adapter: (u32, i32),
    pub target: u32,
    pub wudf_pid: u32,
    pub registration: u64,
}

/// Control handle operations on the Rubylight virtual display driver.
pub trait VddControl: Send + Sync {
    fn caps(&self) -> u32;
    fn query(&self, session_id: u64) -> Result<MonitorStatus>;
    fn set_frame_ring(&self, request: &SetFrameRingRequest) -> Result<()>;
    fn set_fence_ring(&self, request: &SetFenceRingRequest) -> Result<()>;
    fn clear_frame_ring(&self, session_id: u64) -> Result<()>;
    fn clear_frame_ring_generation(&self, session_id: u64, generation: u32) -> Result<()>;
}

/// Lookup and driver control bridge supplied by `rust/host` (backed by `butterpollo-win-display`).
pub trait VddBridge: Send + Sync {
    fn owned_display(&self, name: &str) -> Result<Option<VddOwned>>;
    fn open_control(&self) -> Result<Box<dyn VddControl>>;
}

pub(crate) fn wanted_for(
    name: &str,
    kind: &str,
    config: &butterpollo_config::config::Config,
    vdd: Option<&dyn VddBridge>,
) -> bool {
    let setting = config.get("capture", "auto");
    super::wanted(kind, setting, false)
        || (super::wanted(kind, setting, true)
            && vdd.is_some_and(|b| matches!(b.owned_display(name), Ok(Some(_)))))
}

/// The driver's WUDFHost, opened to duplicate handles into it.
pub(super) struct Remote {
    process: HANDLE,
}
// SAFETY: a process handle is a process-wide token, safe to wait on or duplicate concurrently across threads, closed once in Drop.
unsafe impl Send for Remote {}
// SAFETY: Win32 `WaitForSingleObject` and `DuplicateHandle` take the handle by value concurrently without mutating `Remote`.
unsafe impl Sync for Remote {}

impl Remote {
    pub(super) fn open(pid: u32) -> Result<Self> {
        // SAFETY: plain FFI; the returned handle is owned and closed in Drop.
        let process = unsafe { OpenProcess(PROCESS_DUP_HANDLE | PROCESS_SYNCHRONIZE, false, pid) }
            .with_context(|| format!("open the display driver's host process {pid}"))?;
        Ok(Self { process })
    }
    /// Whether the remote driver process has already terminated.
    pub(super) fn has_exited(&self) -> bool {
        // SAFETY: `self.process` was opened with `PROCESS_SYNCHRONIZE` and stays live until `Drop`.
        unsafe { WaitForSingleObject(self.process, 0) == WAIT_OBJECT_0 }
    }
    pub(super) fn duplicate(&self, handle: HANDLE) -> Result<u64> {
        let mut remote = HANDLE::default();
        // SAFETY: `handle` is live in this process; `self.process` grants PROCESS_DUP_HANDLE.
        unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                handle,
                self.process,
                &mut remote,
                0,
                false,
                DUPLICATE_SAME_ACCESS,
            )?;
        }
        Ok(remote.0 as usize as u64)
    }
    pub(super) fn close(&self, value: u64) {
        // SAFETY: `value` was duplicated into that process by `duplicate` and the driver did
        // not adopt it, so this is its only close; DUPLICATE_CLOSE_SOURCE closes it there.
        unsafe {
            let _ = DuplicateHandle(
                self.process,
                HANDLE(value as usize as *mut _),
                HANDLE::default(),
                std::ptr::null_mut(),
                0,
                false,
                DUPLICATE_CLOSE_SOURCE,
            );
        }
    }
}
impl Drop for Remote {
    fn drop(&mut self) {
        // SAFETY: this value owns the process handle and closes it once.
        unsafe {
            let _ = CloseHandle(self.process);
        }
    }
}
