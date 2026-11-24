//! The host side of Rubylight's virtual display driver. The driver is a Windows driver: on
//! Linux its control interface never opens and this process owns none of its monitors.
use anyhow::Result;
use rubylight_vdisplay_proto::control::InfoReply;
pub use rubylight_vdisplay_proto::control::{
    MonitorStatus, SetFenceRingRequest, SetFrameRingRequest,
};

const DRIVER: &str = "Rubylight's virtual display driver";

/// The driver's control interface. It cannot be opened on Linux.
pub struct Control {
    pub info: InfoReply,
}

impl Control {
    /// Open the driver's control interface. Fails on Linux, where the driver does not exist.
    pub fn open() -> Result<Self> {
        Err(crate::unsupported(DRIVER))
    }
    /// The state of the monitor `session_id`. Fails: there is no driver to ask.
    pub fn query(&self, _session_id: u64) -> Result<MonitorStatus> {
        Err(crate::unsupported(DRIVER))
    }
    /// Hand the driver a frame ring. Fails: there is no driver to take it.
    pub fn set_frame_ring(&self, _request: &SetFrameRingRequest) -> Result<()> {
        Err(crate::unsupported(DRIVER))
    }
    /// Hand the driver a fence ring. Fails: there is no driver to take it.
    pub fn set_fence_ring(&self, _request: &SetFenceRingRequest) -> Result<()> {
        Err(crate::unsupported(DRIVER))
    }
    /// Take the frame ring of the monitor `session_id` back. Fails: there is no driver.
    pub fn clear_frame_ring(&self, _session_id: u64) -> Result<()> {
        Err(crate::unsupported(DRIVER))
    }
    /// Take one generation of the monitor's frame ring back. Fails: there is no driver.
    pub fn clear_frame_ring_generation(&self, _session_id: u64, _generation: u32) -> Result<()> {
        Err(crate::unsupported(DRIVER))
    }
}

/// A monitor this process created on the driver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Owned {
    pub session_id: u64,
    /// The adapter and target the driver reported.
    pub adapter: (u32, i32),
    pub target: u32,
    /// Where the system lists the display while it is on, when that is another adapter and
    /// target than the driver's.
    pub listed: Option<((u32, i32), u32)>,
    /// The process that hosts the driver.
    pub wudf_pid: u32,
    /// One actual monitor registration.
    pub registration: u64,
}

/// The driver's monitor behind the display `name`, if this process created it. None on
/// Linux, where it creates none.
pub fn owned_display(_name: &str) -> Result<Option<Owned>> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_driver_is_absent() {
        assert!(Control::open().is_err());
        assert_eq!(owned_display("DP-1").unwrap(), None);
    }
}
