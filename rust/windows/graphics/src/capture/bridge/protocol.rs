//! IPC messages, shared-texture validation, keyed-mutex helpers, and
//! in-place texture [`Lease`] ownership for the WGC user-session bridge.

use super::super::*;
use butterpollo_config::config::Config;
use serde::{Deserialize, Serialize};
use std::{
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    sync::{Arc, Mutex},
};
use windows::Win32::{Foundation::*, System::StationsAndDesktops::*};
use windows::core::{HRESULT, Interface};

pub(super) const VERSION: u32 = 3;
pub(super) const SLOTS: usize = 4;
/// How long a WGC helper's start may take before it is given up.
pub const START_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const PIPE_PREFIX: &str = r"\\.\pipe\Butterpollo.Wgc.";

pub(super) fn owned(handle: HANDLE) -> OwnedHandle {
    // SAFETY: Callers pass a handle this process owns and closes nowhere else: a fresh
    // CreateSharedHandle result, or the event the pid-checked parent duplicated into this process.
    unsafe { OwnedHandle::from_raw_handle(handle.0) }
}
pub(super) fn raw(handle: &OwnedHandle) -> HANDLE {
    HANDLE(handle.as_raw_handle())
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub(super) enum Request {
    Start {
        version: u32,
        name: String,
        hdr: bool,
        config: Config,
        /// An event of the host's, duplicated into the helper, set after
        /// each frame it announces.
        wake: u64,
        /// A game window (HWND) to capture instead of the display `name`.
        /// Absent for displays, so their message is unchanged.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<u64>,
    },
    Release {
        slot: usize,
    },
    Ping,
    Stop,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub(super) enum Reply {
    Alive,
    Ready {
        version: u32,
        handles: [u64; SLOTS],
        width: u32,
        height: u32,
        format: i32,
        compute: bool,
    },
    Frame {
        slot: usize,
        sequence: u64,
        qpc: i64,
        /// Whether the compositions since the previous frame changed the
        /// foreground window; absent when WGC cannot tell.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        focus: Option<bool>,
    },
    Error {
        message: String,
        #[serde(default)]
        device_removed: Option<i32>,
    },
    ComputeFallback {
        message: String,
    },
}

/// No desktop mutation. A user capture cannot show Winlogon/UAC; the SYSTEM
/// host must temporarily use Desktop Duplication there.
pub(in crate::capture) fn desktop_available() -> bool {
    // SAFETY: `desktop` is closed exactly once below, and `name` is a live buffer whose byte size
    // is passed.
    unsafe {
        let Ok(desktop) = OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS)
        else {
            return false;
        };
        let mut name = [0u16; 256];
        let result = GetUserObjectInformationW(
            HANDLE(desktop.0),
            UOI_NAME,
            Some(name.as_mut_ptr().cast()),
            std::mem::size_of_val(&name) as u32,
            None,
        );
        let _ = CloseDesktop(desktop);
        result.is_ok() && crate::text::from_wide(&name).eq_ignore_ascii_case("default")
    }
}

/// AcquireSync's WAIT_TIMEOUT and WAIT_ABANDONED are *successful HRESULTs*.
/// The generated Result binding loses that distinction, so inspect the raw code.
pub(super) fn acquire(mutex: &IDXGIKeyedMutex, key: u64) -> Result<bool> {
    // SAFETY: `mutex` is a live IDXGIKeyedMutex and AcquireSync takes (this, key, timeout) as
    // called here.
    let status = unsafe { (mutex.vtable().AcquireSync)(mutex.as_raw(), key, 0) };
    acquired_status(status)
}
pub(super) fn acquired_status(status: HRESULT) -> Result<bool> {
    if status.0 == WAIT_TIMEOUT.0 as i32 {
        return Ok(false);
    }
    ensure!(
        status == HRESULT(0),
        "WGC shared texture mutex failed: {status:?}"
    );
    Ok(true)
}

pub(super) fn capture_config(config: &Config) -> Config {
    const KEYS: &[&str] = &[
        "adapter_name",
        "adapter_pnp_id",
        "gpu_compute_conversion",
        "wgc_compute_copy",
        "wgc_high_rate_capture",
        "wgc_drain_to_newest",
        "wgc_cursor_capture",
        "wgc_helper_streaming_scope",
        // Whether a window's capture draws the pointer (`window_cursor`).
        butterpollo_media_policy::app_window::SETTING,
        // What part of a window is captured (`capture_window_frame`).
        butterpollo_media_policy::app_window::FRAME,
    ];
    Config {
        values: config
            .values
            .iter()
            .filter(|(key, _)| KEYS.contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    }
}
pub(super) fn handoff(
    gpu: &Device,
    config: &Config,
    warnings: &dyn butterpollo_media_policy::WarningSink,
) -> Option<crate::compute::Handoff> {
    if !config.boolean("wgc_compute_copy", true) || !crate::compute::enabled(config) {
        return None;
    }
    match crate::compute::Compute::for_device(&gpu.device)
        .and_then(|compute| crate::compute::Handoff::new(compute, gpu))
    {
        Ok(copy) => Some(copy),
        Err(error) => {
            warnings.set("capture_compute", &format!("WGC helper compute copies unavailable ({error:#}); using the graphics queue. Lower game GPU load or update the graphics driver if capture stutters."));
            None
        }
    }
}

#[derive(Default)]
pub(super) struct Slots {
    pub(super) busy: [bool; SLOTS],
    pub(super) sequence: u64,
}
impl Slots {
    pub(super) fn publish(&mut self, slot: usize, sequence: u64) -> Result<()> {
        ensure!(
            slot < SLOTS && !self.busy[slot],
            "invalid or occupied WGC slot"
        );
        ensure!(sequence > self.sequence, "out-of-order WGC frame");
        self.busy[slot] = true;
        self.sequence = sequence;
        Ok(())
    }
    pub(super) fn release(&mut self, slot: usize) -> Result<()> {
        ensure!(
            slot < SLOTS && self.busy[slot],
            "invalid or duplicate WGC release"
        );
        self.busy[slot] = false;
        Ok(())
    }
}

pub(super) struct Texture {
    /// Shared with each image lent from it, without an allocation per frame.
    pub(super) texture: Arc<ID3D11Texture2D>,
    pub(super) mutex: IDXGIKeyedMutex,
}

/// A shared texture an image is read from in place instead of being copied
/// into a host texture first, until the last clone of the image is dropped.
/// A WGC helper texture (on AMD with compute copies, unless
/// `wgc_helper_zero_copy` is false) or a keyed-mutex frame ring's goes back
/// through its keyed mutex once the compute work that read it is done; the
/// capture worker tells the helper on its next poll. A fence ring's slot goes
/// back through the ring's fences ([`idd_push::Lent`]).
pub(crate) struct Lease {
    pub(super) slot: usize,
    pub(super) back: Back,
    /// The WGC helper hears of a returned slot through these. Rubylight's
    /// display driver needs no word: it finds the slot free by its mutex or
    /// its state word.
    pub(super) notify: Option<Notify>,
}
pub(super) enum Back {
    Keyed(crate::compute::KeyedLease),
    Fenced(super::idd_push::Lent),
}
/// The helper's returned slots, and the capture worker's wake.
type Notify = (Arc<Mutex<Vec<usize>>>, Arc<crate::timing::Signal>);
impl Lease {
    /// A keyed-mutex frame-ring texture of Rubylight's display driver, handed
    /// back with key 0.
    pub(in crate::capture) fn ring(slot: usize, keyed: crate::compute::KeyedLease) -> Self {
        Self {
            slot,
            back: Back::Keyed(keyed),
            notify: None,
        }
    }
    /// A fence-ring slot of Rubylight's display driver.
    pub(in crate::capture) fn fenced(slot: usize, lent: super::idd_push::Lent) -> Self {
        Self {
            slot,
            back: Back::Fenced(lent),
            notify: None,
        }
    }
    /// Order D3D11 work submitted after this on the image's device after the
    /// writes the image's `ready` waits for.
    pub(in crate::capture) fn order_d3d11(&self) -> Result<()> {
        match &self.back {
            // The capture's D3D11 device took the keyed mutex itself.
            Back::Keyed { .. } => Ok(()),
            Back::Fenced(lent) => lent.order_d3d11(),
        }
    }
    /// A producer failed to complete its pixels. Quarantine a fence ring so capture can
    /// reopen with the standard transport; keyed ownership is already handled separately.
    pub(in crate::capture) fn reject(&self) {
        if let Back::Fenced(lent) = &self.back {
            lent.reject();
        }
    }
}
// SAFETY: The keyed mutex is used only in Drop, under `returns`' lock on the multithread-protected
// device; a fence-ring slot is handed back under its ring's lock; the rest is locks and an event
// handle.
unsafe impl Send for Lease {}
// SAFETY: Shared references order the multithread-protected D3D11 device behind a fence
// or atomically reject its ring; only Drop, with exclusive access, hands the texture back.
unsafe impl Sync for Lease {}
impl Drop for Lease {
    fn drop(&mut self) {
        let returned = match &self.back {
            Back::Keyed(keyed) => keyed.hand_back(),
            // The slot is freed as `Lent` drops, right after this.
            Back::Fenced(_) => Ok(()),
        };
        if let Err(error) = returned {
            tracing::debug!(error = %format!("{error:#}"), slot = self.slot, "shared capture texture handed back after a GPU error");
        }
        if let Some((freed, wake)) = &self.notify {
            freed.lock().unwrap().push(self.slot);
            // Wake the capture worker so the helper hears of the free texture now.
            let _ = wake.set();
        }
    }
}
pub(super) fn validate_texture(
    desc: &D3D11_TEXTURE2D_DESC,
    width: u32,
    height: u32,
    format: i32,
) -> Result<()> {
    ensure!(
        width > 0 && height > 0 && width <= 16384 && height <= 16384,
        "invalid WGC texture dimensions"
    );
    ensure!(
        matches!(
            DXGI_FORMAT(format),
            DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_R16G16B16A16_FLOAT
        ),
        "unsupported WGC shared format"
    );
    ensure!(
        desc.Width == width
            && desc.Height == height
            && desc.Format.0 == format
            && desc.MipLevels == 1
            && desc.ArraySize == 1
            && desc.SampleDesc.Count == 1
            && desc.Usage == D3D11_USAGE_DEFAULT,
        "WGC shared texture descriptor mismatch"
    );
    let sharing =
        (D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0 | D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0) as u32;
    ensure!(
        desc.MiscFlags & sharing == sharing,
        "WGC texture is not synchronized for sharing"
    );
    Ok(())
}
