//! Shared-memory ring header, texture slots, exclusive monitor locks,
//! fence/keyed transport selection, and failure tracking for `idd_push`.

use super::{Fences, Remote, VddOwned};
use crate::capture::{Device, Pixel};
use anyhow::{Context, Result, bail};
use rubylight_vdisplay_proto::control::{CAP_FENCE_RING, MonitorStatus};
use rubylight_vdisplay_proto::ring::{self, Counters, Header, Kind, Layout};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use windows::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE, WAIT_TIMEOUT};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    PAGE_READWRITE, UnmapViewOfFile,
};
use windows::core::{HRESULT, Interface, PCWSTR};

/// Shapes a ring may be rebuilt to before the first frame, when the driver reports the
/// surface differs from the guess it was opened with.
pub(super) const RESHAPES_BEFORE_FIRST_FRAME: u32 = 3;
/// The driver stamps its swap-chain loop every pass (at most 125 ms apart while idle). This
/// much silence after frames flowed means the loop stopped.
pub(super) const DRAIN_STALL: Duration = Duration::from_secs(3);
/// How long a ring may wait for its first frame before the capture gives up.
pub(super) const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(10);
/// A live producer that acquires frames but cannot publish through fences gets the
/// standard transport sooner. A display Windows has not started composing keeps its
/// full startup allowance above.
pub(super) const FENCE_HANDOFF_TIMEOUT: Duration = Duration::from_secs(2);
/// How often the driver's frame counters are logged (`idd_push=debug`).
pub(super) const COUNTERS_EVERY: Duration = Duration::from_secs(5);

/// Rings of one monitor that may fail before their first frame before its captures use WGC.
pub(super) const FAILURES_BEFORE_WGC: u32 = 2;

/// The monitors this process captures from. The driver keeps one ring per monitor, so a
/// second capture registering its own would take the frames from the first.
pub(super) static CAPTURING: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// This process's capture of one monitor, released on drop.
pub(super) struct Exclusive(u64);

impl Exclusive {
    pub(super) fn claim(session_id: u64) -> Option<Self> {
        let mut capturing = CAPTURING.lock().unwrap();
        if capturing.contains(&session_id) {
            return None;
        }
        capturing.push(session_id);
        Some(Self(session_id))
    }
}

impl Drop for Exclusive {
    fn drop(&mut self) {
        CAPTURING.lock().unwrap().retain(|&id| id != self.0);
    }
}

/// Monitors whose rings failed before delivering a frame, in which [`FenceScope`], and how
/// often. A failure the driver reports after the ring was accepted (textures that do not
/// open, a surface that never settles, no frame at all) would repeat on every reopen, so after
/// [`FAILURES_BEFORE_WGC`] the monitor's captures use WGC until a ring delivers a frame or the
/// scope changes: the display is recreated, the driver restarts, or it renders on another
/// adapter. Session ids alone are persistent client identities, so a recreated display keeps
/// its id and starts clean only by its new registration.
#[derive(Default)]
pub(super) struct Failures(pub(super) Vec<(FenceScope, u32)>);

impl Failures {
    pub(super) fn failed(&mut self, scope: FenceScope) -> u32 {
        match self
            .0
            .iter_mut()
            .find(|(at, _)| at.session == scope.session)
        {
            Some((at, count)) if *at == scope => {
                *count += 1;
                *count
            }
            Some(entry) => {
                *entry = (scope, 1);
                1
            }
            None => {
                self.0.push((scope, 1));
                1
            }
        }
    }

    pub(super) fn delivered(&mut self, session_id: u64) {
        self.0.retain(|(at, _)| at.session != session_id);
    }

    /// The failures that send this monitor's captures to WGC in `scope`, if it has them.
    pub(super) fn excluded(&self, scope: FenceScope) -> Option<u32> {
        self.0
            .iter()
            .find(|(at, count)| *at == scope && *count >= FAILURES_BEFORE_WGC)
            .map(|(_, count)| *count)
    }
}

pub(super) static FAILURES: Mutex<Failures> = Mutex::new(Failures(Vec::new()));

/// A capture reopen shares this identity; a real display recreation, driver restart or
/// render-adapter change does not. Session ids alone are persistent client identities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FenceScope {
    pub(super) session: u64,
    pub(super) registration: u64,
    pub(super) driver_pid: u32,
    pub(super) adapter: (u32, i32),
}

impl FenceScope {
    pub(super) fn new(monitor: &VddOwned, adapter: (u32, i32)) -> Self {
        Self {
            session: monitor.session_id,
            registration: monitor.registration,
            driver_pid: monitor.wudf_pid,
            adapter,
        }
    }
}

/// Remember a rejected transport through capture reopen, independently of the generic
/// driver-to-WGC failures. A successful keyed frame must not enable the rejected path.
#[derive(Default)]
pub(super) struct FenceFailures(pub(super) Vec<FenceScope>);

impl FenceFailures {
    pub(super) const LIMIT: usize = 64;

    pub(super) fn failed(&mut self, scope: FenceScope) {
        self.clear(scope.session);
        if self.0.len() == Self::LIMIT {
            self.0.remove(0);
        }
        self.0.push(scope);
    }

    pub(super) fn excluded(&self, scope: FenceScope) -> bool {
        self.0.contains(&scope)
    }

    pub(super) fn clear(&mut self, session: u64) {
        self.0.retain(|entry| entry.session != session);
    }
}

pub(super) static FENCE_FAILURES: Mutex<FenceFailures> = Mutex::new(FenceFailures(Vec::new()));

#[derive(Debug, PartialEq, Eq)]
pub(super) enum FenceRecovery {
    /// No GPU work has read this ring yet, so its device can still serve a keyed ring.
    Keyed {
        reason: &'static str,
        remember: bool,
    },
    /// A context may already wait behind failed work. Reopening creates a fresh device.
    Reopen(&'static str),
}

pub(super) fn fence_recovery(
    status: u32,
    started: bool,
    elapsed: Duration,
    counters: Counters,
    ordering_failed: bool,
    queue_stuck: bool,
) -> Option<FenceRecovery> {
    if queue_stuck {
        return Some(FenceRecovery::Reopen("compute queue cannot be reused"));
    }
    if ordering_failed {
        return Some(FenceRecovery::Reopen("lost GPU ordering"));
    }
    if status == ring::STATUS_OPEN_FAILED {
        return Some(if started {
            FenceRecovery::Reopen("shared resources stopped opening")
        } else {
            FenceRecovery::Keyed {
                reason: "could not open shared resources",
                remember: true,
            }
        });
    }
    // Shape changes and device/adapter loss follow their existing recovery paths.
    if !started && matches!(status, ring::STATUS_WAITING | ring::STATUS_STREAMING) {
        if elapsed >= FENCE_HANDOFF_TIMEOUT && counters.acquired > 0 && counters.pushed == 0 {
            return Some(FenceRecovery::Keyed {
                reason: "could not publish acquired frames",
                remember: true,
            });
        }
        if elapsed >= FIRST_FRAME_TIMEOUT {
            return Some(FenceRecovery::Keyed {
                reason: "did not deliver a frame",
                // Silence alone does not identify a fence problem: Windows may not have
                // supplied any pixels yet. A later capture may try fences again.
                remember: false,
            });
        }
    }
    None
}

/// Microseconds from QPC `from` to `to`, zero when `to` is earlier or either is missing.
pub(super) fn qpc_micros(from: u64, to: u64, frequency: i64) -> u64 {
    if from == 0 || to <= from || frequency <= 0 {
        return 0;
    }
    ((to - from) as u128 * 1_000_000 / frequency as u128) as u64
}

/// Whether this capture should come from the driver: asked for by name, or automatic on a
/// display this host created. `capture = wgc` or `ddx` keeps those.
pub(in crate::capture) fn wanted(kind: &str, capture_setting: &str, own_display: bool) -> bool {
    kind == "idd-push" || (matches!(capture_setting.trim(), "" | "auto") && own_display)
}

/// The ring to open: the surface the driver last composed when it reported one, otherwise
/// the monitor's mode in the format its colour state suggests.
pub(super) fn ring_shape(status: &MonitorStatus, hdr: bool) -> (u32, u32, u32) {
    if status.surface_width > 0
        && status.surface_height > 0
        && ring::supported_format(status.surface_format)
    {
        return (
            status.surface_width,
            status.surface_height,
            status.surface_format,
        );
    }
    let format = if hdr {
        ring::FORMAT_RGBA16F
    } else {
        ring::FORMAT_BGRA8
    };
    (status.width, status.height, format)
}

/// The generation after `previous`: never 0 and within the publish token's 24 bits, so a
/// frame an earlier ring of the monitor left in its header is never taken from this one.
pub(super) fn next_generation(previous: u32) -> u32 {
    (previous % ring::Token::GENERATION_MASK) + 1
}

/// What the header's driver status means for this capture.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Verdict {
    Fine,
    /// Retire the ring before its sequence or GPU fence could wrap.
    Renew,
    /// Rebuild the ring at the surface's real `(width, height, format)`.
    Reshape(u32, u32, u32),
    Fail(String),
}

pub(super) fn verdict(status: (u32, u32), surface: (u32, u32, u32), started: bool) -> Verdict {
    match status.0 {
        ring::STATUS_WAITING | ring::STATUS_STREAMING => Verdict::Fine,
        ring::STATUS_RENEW_REQUIRED => Verdict::Renew,
        ring::STATUS_SIZE_MISMATCH | ring::STATUS_FORMAT_MISMATCH => {
            let (width, height, format) = surface;
            if started {
                // As with WGC, a size or colour-space change reopens the capture, so the
                // encoder is rebuilt for it.
                Verdict::Fail(format!(
                    "the Rubylight display changed to {width}x{height} format {format}"
                ))
            } else if width > 0 && height > 0 && ring::supported_format(format) {
                Verdict::Reshape(width, height, format)
            } else {
                Verdict::Fail(format!(
                    "the Rubylight display composes an unsupported surface ({width}x{height} format {format})"
                ))
            }
        }
        ring::STATUS_OPEN_FAILED => Verdict::Fail(format!(
            "the Rubylight display driver could not open the shared textures ({:#010x})",
            status.1
        )),
        ring::STATUS_WRONG_ADAPTER => Verdict::Fail(
            "the Rubylight display renders on another GPU than the capture opened on; select that GPU or let Windows choose".into(),
        ),
        ring::STATUS_DEVICE_LOST => {
            Verdict::Fail("the Rubylight display driver lost its GPU device".into())
        }
        other => Verdict::Fail(format!("unknown Rubylight display driver status {other}")),
    }
}

/// Whether the driver's drain loop has gone quiet: `beat` and `now` are QPC values.
pub(super) fn stalled(beat: u64, now: i64, frequency: i64, limit: Duration) -> bool {
    if beat == 0 || now <= 0 || frequency <= 0 {
        return false;
    }
    let age = now.saturating_sub(beat as i64).max(0) as f64 / frequency as f64;
    age > limit.as_secs_f64()
}

pub(super) fn qpc_now() -> i64 {
    let mut now = 0;
    // SAFETY: QueryPerformanceCounter writes only to the live local.
    let _ = unsafe { windows::Win32::System::Performance::QueryPerformanceCounter(&mut now) };
    now
}

/// Take a keyed mutex without waiting. AcquireSync's WAIT_TIMEOUT is a success HRESULT the
/// `Result` binding would report as `Ok`, so the raw code is inspected (as the WGC helper
/// transport does). Keyed-mutex ring only.
pub(super) fn acquire(mutex: &IDXGIKeyedMutex, key: u64) -> Result<bool> {
    // SAFETY: `mutex` is a live keyed mutex; AcquireSync takes (this, key, timeout).
    let status = unsafe { (mutex.vtable().AcquireSync)(mutex.as_raw(), key, 0) };
    if status.0 == WAIT_TIMEOUT.0 as i32 {
        return Ok(false);
    }
    anyhow::ensure!(
        status == HRESULT(0),
        "Rubylight frame ring mutex failed: {status:?}"
    );
    Ok(true)
}

/// The shared header: an unnamed section mapped here, its handle duplicated to the driver.
pub(super) struct Section {
    pub(super) handle: OwnedHandle,
    base: *mut std::ffi::c_void,
}

// SAFETY: the view is only read and written through atomics (`Header`), from any thread, and
// unmapped once, in Drop; the handle is owned.
unsafe impl Send for Section {}
// SAFETY: as above.
unsafe impl Sync for Section {}

impl Section {
    pub(super) fn new() -> Result<Self> {
        // SAFETY: an unnamed, pagefile-backed section of SECTION_BYTES; the handle is owned
        // below and the view unmapped in Drop.
        unsafe {
            let handle = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                None,
                PAGE_READWRITE,
                0,
                ring::SECTION_BYTES as u32,
                PCWSTR::null(),
            )?;
            let handle = OwnedHandle::from_raw_handle(handle.0);
            let view = MapViewOfFile(
                HANDLE(handle.as_raw_handle()),
                FILE_MAP_ALL_ACCESS,
                0,
                0,
                ring::SECTION_BYTES,
            );
            anyhow::ensure!(!view.Value.is_null(), "map the frame ring header");
            Ok(Self {
                handle,
                base: view.Value,
            })
        }
    }

    pub(super) fn header(&self) -> Header<'_> {
        // SAFETY: the view is page-aligned and SECTION_BYTES long, so it holds HEADER_WORDS
        // aligned u64 words; both processes access them only atomically.
        let words = unsafe {
            std::slice::from_raw_parts(self.base.cast::<AtomicU64>(), ring::HEADER_WORDS)
        };
        Header::new(words).expect("a section holds a whole header")
    }
}

impl Drop for Section {
    fn drop(&mut self) {
        // SAFETY: this value's own single view; the handle closes after, as a field.
        unsafe {
            let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: self.base });
        }
    }
}

pub(super) struct Slot {
    /// Shared with each image lent from this slot without per-frame `Arc` allocation.
    pub(super) texture: Arc<ID3D11Texture2D>,
    /// Keyed-mutex ring only.
    pub(super) mutex: Option<IDXGIKeyedMutex>,
    pub(super) shared: OwnedHandle,
    /// Immutable `(width, height, pixel)` validated once when the ring was created.
    pub(super) shape: (u32, u32, Pixel),
}

pub(super) fn ring_pixel(format: DXGI_FORMAT) -> Result<Pixel> {
    match format {
        DXGI_FORMAT_B8G8R8A8_UNORM => Ok(Pixel::Bgra8),
        DXGI_FORMAT_R16G16B16A16_FLOAT => Ok(Pixel::RgbaF16),
        format => bail!("unsupported Rubylight frame ring format {format:?}"),
    }
}

/// One ring: its header and textures, and a fence ring's fences. The driver holds
/// duplicates of every handle until the ring is replaced or cleared.
pub(super) struct Ring {
    pub(super) section: Arc<Section>,
    pub(super) slots: Vec<Slot>,
    pub(super) layout: Layout,
    /// `None` for a keyed-mutex ring.
    pub(super) fences: Option<Arc<Fences>>,
}

impl Ring {
    /// A fence ring: its textures and fences are created on the compute device's D3D12, the
    /// textures opened on the capture's D3D11 device for its copies and readers.
    pub(super) fn fenced(
        gpu: &Device,
        compute: Arc<crate::compute::Compute>,
        layout: Layout,
        read_in_place: bool,
    ) -> Result<Self> {
        let format = DXGI_FORMAT(layout.format as i32);
        let pixel = ring_pixel(format)?;
        let shape = (layout.width, layout.height, pixel);
        let section = Arc::new(Section::new()?);
        section.header().init(layout);
        let device: ID3D11Device1 = gpu.device.cast()?;
        // Build the owner before importing textures: a later allocation/open failure must
        // remove every preceding import from Compute's cache as this partially built ring
        // drops, just as a complete ring does.
        let fences = Fences::new(gpu, compute.clone(), section.clone(), read_in_place)?;
        let mut ring = Self {
            section,
            slots: Vec::with_capacity(layout.slot_count as usize),
            layout,
            fences: Some(Arc::new(fences)),
        };
        for _ in 0..layout.slot_count {
            let (texture, shared) =
                compute.share_texture(&device, layout.width, layout.height, format)?;
            ring.fences
                .as_ref()
                .unwrap()
                .imports
                .lock()
                .unwrap()
                .push(texture.clone());
            ring.slots.push(Slot {
                texture: Arc::new(texture),
                mutex: None,
                shared,
                shape,
            });
        }
        Ok(ring)
    }

    /// The keyed-mutex ring, also compatible with drivers before `CAP_FENCE_RING`.
    pub(super) fn keyed(gpu: &Device, layout: Layout) -> Result<Self> {
        let format = DXGI_FORMAT(layout.format as i32);
        let pixel = ring_pixel(format)?;
        let shape = (layout.width, layout.height, pixel);
        let section = Arc::new(Section::new()?);
        section.header().init(layout);
        let desc = D3D11_TEXTURE2D_DESC {
            Width: layout.width,
            Height: layout.height,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: (D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0
                | D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0) as u32,
        };
        let mut slots = Vec::with_capacity(layout.slot_count as usize);
        for _ in 0..layout.slot_count {
            // SAFETY: `gpu.device` is live and `desc` describes a valid shareable texture; the
            // shared NT handle is owned by the slot.
            unsafe {
                let mut texture = None;
                gpu.device
                    .CreateTexture2D(&desc, None, Some(&mut texture))?;
                let texture = texture.context("frame ring texture missing")?;
                let shared = texture.cast::<IDXGIResource1>()?.CreateSharedHandle(
                    None,
                    DXGI_SHARED_RESOURCE_READ.0 | DXGI_SHARED_RESOURCE_WRITE.0,
                    None,
                )?;
                slots.push(Slot {
                    mutex: Some(texture.cast()?),
                    texture: Arc::new(texture),
                    shared: OwnedHandle::from_raw_handle(shared.0),
                    shape,
                });
            }
        }
        Ok(Self {
            section,
            slots,
            layout,
            fences: None,
        })
    }
}

/// Handle values duplicated into the driver's process for one request, closed there again
/// unless the driver adopted them.
pub(super) struct Delivery<'a> {
    pub(super) remote: &'a Remote,
    pub(super) values: Vec<u64>,
    pub(super) adopted: bool,
}

impl Delivery<'_> {
    pub(super) fn send(&mut self, handle: HANDLE) -> Result<u64> {
        let value = self.remote.duplicate(handle)?;
        self.values.push(value);
        Ok(value)
    }
}

impl Drop for Delivery<'_> {
    fn drop(&mut self) {
        if !self.adopted {
            for value in self.values.drain(..) {
                self.remote.close(value);
            }
        }
    }
}

/// The fence transport is the default, and is limited to the in-place compute path. A
/// capture without it and an old installed driver use the keyed-mutex transport.
pub(super) fn ring_kind(caps: u32, requested: bool, zero_copy: bool) -> Kind {
    if requested && zero_copy && caps & CAP_FENCE_RING != 0 {
        Kind::Fence
    } else {
        Kind::KeyedMutex
    }
}

/// The transport a session opens with, and why a requested fence ring was not used. Turning
/// the fence ring off explicitly is a choice and never has a reason.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct RingChoice {
    pub(super) kind: Kind,
    pub(super) warning: Option<&'static str>,
}

pub(super) fn choose_ring(
    caps: u32,
    requested: bool,
    rejected: bool,
    zero_copy: bool,
) -> RingChoice {
    let kind = ring_kind(caps, requested && !rejected, zero_copy);
    let warning = if !requested || kind == Kind::Fence {
        None
    } else if rejected {
        Some(
            "Using the keyed-mutex driver transport after GPU fences failed on this display. Recreate the display or restart the host to retry.",
        )
    } else if caps & CAP_FENCE_RING == 0 {
        Some(
            "The installed Rubylight display driver does not support the GPU fence transport, so capture uses the keyed-mutex ring. Update the display driver, or set idd_push_fence_ring = false to use the keyed-mutex ring without this warning.",
        )
    } else {
        Some(
            "The GPU fence transport needs compute capture with in-place reads, so capture uses the keyed-mutex ring. Enable gpu_compute_conversion and idd_push_zero_copy, or set idd_push_fence_ring = false to use the keyed-mutex ring without this warning.",
        )
    };
    RingChoice { kind, warning }
}
