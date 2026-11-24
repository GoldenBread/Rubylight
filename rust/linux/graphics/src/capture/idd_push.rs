//! The bridge a capture uses to reach Rubylight's virtual display driver, supplied by the host so that graphics does not depend on the display crate. On Linux only the bridge's traits and types are here; no capture takes frames from a display driver yet.
use anyhow::Result;
use rubylight_vdisplay_proto::control::{MonitorStatus, SetFenceRingRequest, SetFrameRingRequest};

/// A monitor owned by the Rubylight virtual display driver.
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

/// Lookup and driver control bridge supplied by the host.
pub trait VddBridge: Send + Sync {
    fn owned_display(&self, name: &str) -> Result<Option<VddOwned>>;
    fn open_control(&self) -> Result<Box<dyn VddControl>>;
}
