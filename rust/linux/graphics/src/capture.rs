//! Displays, GPUs and screen capture as the host names them. On Linux the GPUs are the AMD and NVIDIA render nodes, and a capture takes the picture of a gamescope session (the target `gamescope`), of a PipeWire node (`pipewire-node:<id>`) or of the user's own desktop through the portal (`desktop`, `desktop:<monitor>`) as DMA-BUFs that VA-API converts into surfaces the encoder reads in place; a desktop falls back to memory buffers uploaded by the CPU. No window is captured yet.
use crate::vaapi::{
    Va,
    node::{self, Node},
    pool::{Pool, Pooled},
    vpp::{Colors, Converter},
};
use anyhow::{Context, Result, bail};
use butterpollo_media_policy::encoder_gpu;
use serde::Serialize;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub mod desktop;
#[cfg(test)]
mod desktop_tests;
pub mod idd_push;
pub mod kms;
pub mod kwin;
mod priority;
mod pw_capture;

pub use priority::Priority;

pub use butterpollo_media_policy::app_window::WindowTarget;
use butterpollo_media_policy::app_window::{Candidate, WINDOW_TARGET_PREFIX};

/// The capture target of the newest gamescope session.
pub const GAMESCOPE_TARGET: &str = "gamescope";

/// How long a gamescope target waits for the session to offer its picture.
const GAMESCOPE_WAIT: Duration = Duration::from_secs(10);

/// How long a stream waits for a capture helper to start.
pub const WGC_START_TIMEOUT: Duration = Duration::from_secs(5);

/// One display of the desktop, on the GPU that drives it.
#[derive(Clone, Debug, Serialize)]
pub struct Display {
    pub device_id: String,
    pub display_name: String,
    pub friendly_name: String,
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub primary: bool,
    pub adapter: String,
    #[serde(skip)]
    pub adapter_index: u32,
    #[serde(skip)]
    pub output_index: u32,
}

/// Process-wide display scaling awareness. Nothing needs setting on Linux.
pub fn enable_dpi_awareness() {}

/// The displays of the desktop session, named `desktop:<index>`; none without a session.
pub fn displays() -> Result<Vec<Display>> {
    Ok(desktop::displays())
}

/// The display `name` names among `choices`: the primary (else the first) for an empty name,
/// otherwise the one with that name or device ID, and an error when it is not there.
fn select_display<'a>(choices: &'a [Display], name: &str) -> Result<Option<&'a Display>> {
    if name.is_empty() {
        return Ok(choices
            .iter()
            .find(|d| d.primary)
            .or_else(|| choices.first()));
    }
    choices
        .iter()
        .find(|d| d.display_name == name || d.device_id == name)
        .map(Some)
        .with_context(|| format!("selected capture display is unavailable: {name}"))
}

/// Whether capture can open this display now. An empty name is the primary
/// display. A game window target always counts as present: a closed window,
/// a missing display or a malformed target is the capture's to report, never
/// a reason to show the primary display in the window's place.
pub fn display_present(name: &str) -> bool {
    name.starts_with(WINDOW_TARGET_PREFIX)
        || name == GAMESCOPE_TARGET
        || desktop::present(name)
        || butterpollo_linux_io::gamescope::node_of(name).is_some()
        || displays().is_ok_and(|choices| select_display(&choices, name).is_ok_and(|d| d.is_some()))
}

/// Whether the window a target names can be streamed. No window can be captured on Linux
/// yet, so every target is refused with that reason.
pub fn check_window(_target: &WindowTarget) -> Result<()> {
    Err(crate::unsupported("Window capture"))
}

/// The primary display's name, which capture opens when the stream's display
/// did not come back. `None` while no display is listed.
pub fn primary_display() -> Option<String> {
    let choices = displays().ok()?;
    select_display(&choices, "")
        .ok()
        .flatten()
        .map(|d| d.display_name.clone())
}

/// Per-thread platform initialisation a capture or encoder thread holds for its lifetime.
/// Linux needs none: the guard holds nothing.
pub struct ComGuard;
/// Ends what the value holds, at the point the host drops it. Linux has no COM, so there is nothing to uninitialize.
impl Drop for ComGuard {
    fn drop(&mut self) {}
}

impl ComGuard {
    pub fn new() -> Result<Self> {
        Ok(Self)
    }
}

/// One hardware GPU.
#[derive(serde::Serialize)]
pub struct Gpu {
    pub name: String,
    pub vendor: u32,
    pub dedicated_memory: u64,
    /// The PCI device ID (not in the console's GPU list).
    #[serde(skip)]
    pub device: u32,
    /// The adapter's identity for as long as the system runs.
    pub luid: (u32, i32),
    pub pnp_id: Option<String>,
}

/// The GPU the Graphics card setting names, matched as capture and the
/// encoders match it: by the stored PnP identity when there is one, otherwise
/// by name. None on Automatic.
pub fn select_gpu<'a>(
    gpus: &'a [Gpu],
    adapter_name: &str,
    pnp_id: &str,
) -> Result<Option<&'a Gpu>> {
    if adapter_name.is_empty() && pnp_id.is_empty() {
        return Ok(None);
    }
    let mut matches = gpus.iter().filter(|gpu| {
        if pnp_id.is_empty() {
            gpu.name == adapter_name
        } else {
            gpu.pnp_id
                .as_deref()
                .is_some_and(|id| id.eq_ignore_ascii_case(pnp_id))
        }
    });
    let gpu = matches.next();
    if gpu.is_none() || (!pnp_id.is_empty() && matches.next().is_some()) {
        bail!("configured GPU adapter was not resolved uniquely: {adapter_name} {pnp_id}");
    }
    Ok(gpu)
}

/// The hardware GPUs: the AMD and NVIDIA GPUs with a render node. The adapter identity is the
/// node's minor number; the PnP identity names the PCI vendor, device and slot.
pub fn gpus() -> Result<Vec<Gpu>> {
    Ok(node::nodes()
        .iter()
        .map(|node| Gpu {
            name: node.name.clone(),
            vendor: node.vendor,
            dedicated_memory: node.memory,
            device: node.device,
            luid: (node.minor, 0),
            pnp_id: Some(format!(
                "PCI\\VEN_{:04X}&DEV_{:04X}&SLOT_{}",
                node.vendor, node.device, node.slot
            )),
        })
        .collect())
}

/// The hardware GPUs as the encoder GPU check reads them.
pub fn gpu_adapters(gpus: &[Gpu]) -> Vec<encoder_gpu::Adapter> {
    gpus.iter()
        .map(|gpu| encoder_gpu::Adapter {
            name: gpu.name.clone(),
            vendor: gpu.vendor,
        })
        .collect()
}

/// The GPU choice a stream that encodes on the capture device opens it with.
/// A chosen GPU stays. With none chosen on a PC that mixes vendors it is the
/// AMD or NVIDIA GPU with the most video memory, unless the display is already
/// on one (`display_vendor`).
fn pinned_choice(
    gpus: &[Gpu],
    display_vendor: Option<u32>,
    adapter_name: &str,
    pnp_id: &str,
) -> (String, String) {
    if adapter_name.is_empty() && pnp_id.is_empty() {
        let adapters: Vec<(u32, u64)> = gpus
            .iter()
            .map(|gpu| (gpu.vendor, gpu.dedicated_memory))
            .collect();
        if let Some(gpu) = encoder_gpu::pinned(&adapters, display_vendor).and_then(|i| gpus.get(i))
        {
            return (gpu.name.clone(), gpu.pnp_id.clone().unwrap_or_default());
        }
    }
    (adapter_name.to_string(), pnp_id.to_string())
}

/// [`pinned_choice`] for this PC's GPUs; the choice as given when they cannot be listed.
/// Which GPU drives `display` is not known on Linux yet, so no display counts as already
/// on an encoding GPU.
pub fn stream_adapter(
    _display: Option<&str>,
    adapter_name: &str,
    pnp_id: &str,
) -> (String, String) {
    match gpus() {
        Ok(gpus) => pinned_choice(&gpus, None, adapter_name, pnp_id),
        Err(error) => {
            tracing::debug!(%error, "the GPUs could not be listed; the GPU choice stays as configured");
            (adapter_name.to_string(), pnp_id.to_string())
        }
    }
}

/// The GPU a stream of `display` captures and encodes on, by name and adapter identity: the
/// one the Graphics card setting names or [`stream_adapter`] pins, else the one with the most
/// video memory (Linux cannot tell which GPU drives a display).
pub fn stream_gpu(display: &str, adapter_name: &str, pnp_id: &str) -> Result<(String, (u32, i32))> {
    let gpus = gpus()?;
    let (name, pnp_id) = stream_adapter(Some(display), adapter_name, pnp_id);
    if let Some(gpu) = select_gpu(&gpus, &name, &pnp_id)? {
        return Ok((gpu.name.clone(), gpu.luid));
    }
    // Nothing chosen and nothing to pin: the GPU with the most video memory, since no desktop
    // display says which GPU drives it.
    gpus.iter()
        .max_by_key(|gpu| gpu.dedicated_memory)
        .map(|gpu| (gpu.name.clone(), gpu.luid))
        .context("no AMD or NVIDIA GPU with a render node was found")
}

/// The PCI vendor of [`stream_gpu`]'s GPU for the configured display and adapter, None when it
/// can't be found.
pub fn stream_gpu_vendor(config: &butterpollo_config::config::Config) -> Option<u32> {
    let [display, adapter_name, pnp_id] =
        ["output_name", "adapter_name", "adapter_pnp_id"].map(|key| config.get(key, ""));
    let (_, luid) = stream_gpu(display, adapter_name, pnp_id)
        .inspect_err(|error| tracing::debug!(%error, "the stream's GPU is unknown"))
        .ok()?;
    gpus()
        .ok()?
        .into_iter()
        .find(|gpu| gpu.luid == luid)
        .map(|gpu| gpu.vendor)
}

/// Whose driver settings the panel controls, for the configured stream GPU.
pub fn panel_driver(config: &butterpollo_config::config::Config) -> crate::radeon::Driver {
    let vendors: Vec<u32> = gpus()
        .map(|gpus| gpus.iter().map(|gpu| gpu.vendor).collect())
        .unwrap_or_default();
    crate::radeon::Driver::for_stream(stream_gpu_vendor(config), &vendors)
}

/// The pixel format of a captured picture.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pixel {
    Bgra8,
    RgbaF16,
    Rgba10Pq,
}

/// A picture in CPU memory: `height` rows of `stride` bytes.
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub bytes: Vec<u8>,
    pub captured: Instant,
    pub pixel: Pixel,
}

/// The identity of a graphics device, to compare two of them: the render node's number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct D3d11Device {
    id: usize,
}

/// What a CPU readback keeps between pictures: a converter to RGB and the surfaces it writes.
pub struct D3d11Texture {
    converter: Converter,
    rgb: Arc<Pool>,
    size: (u32, u32),
}

/// The graphics device a capture and its encoders share, with the display it was opened
/// for: a VA-API display on a render node.
#[derive(Clone)]
pub struct Device {
    pub device: D3d11Device,
    pub display: Arc<Display>,
    /// The PCI vendor of the device's adapter. Streams encode only on AMD and NVIDIA.
    pub vendor: u32,
    va: Arc<Va>,
}

impl Device {
    /// The device's identity, to tell a recreated device from this one.
    pub fn key(&self) -> usize {
        self.device.id
    }
    /// The display's luminance. Linux reads none yet: the default, which names no luminance.
    pub fn hdr_metadata(&self) -> butterpollo_media_policy::hdr::Metadata {
        butterpollo_media_policy::hdr::Metadata::default()
    }
    /// The display's SDR content brightness in nits; `100.0` while Linux reads none.
    pub fn sdr_white_nits(&self) -> f32 {
        100.0
    }
    /// Whether the display is composed as HDR10 right now. `None`: its colour space cannot
    /// be read on Linux yet.
    pub fn display_in_hdr(&self) -> Option<bool> {
        None
    }
    /// [`Self::new_adapter`] with no GPU chosen.
    pub fn new(name: &str) -> Result<Self> {
        Self::new_adapter(name, "", "")
    }
    /// A device for work that needs a GPU, not a particular picture: encoder probes and
    /// tools. The GPU is the one the adapter settings name, else the one with the most video
    /// memory.
    pub fn new_adapter(name: &str, adapter_name: &str, pnp_id: &str) -> Result<Self> {
        let (_, luid) = stream_gpu(name, adapter_name, pnp_id)?;
        let node = node::nodes()
            .iter()
            .find(|node| node.minor == luid.0)
            .context("the GPU's render node is gone")?;
        let va = Va::open(&node.path)
            .with_context(|| format!("opening VA-API on {} ({})", node.path, node.name))?;
        Ok(Self::on(Arc::new(va), node, name, (0, 0)))
    }

    /// A device on `va`, which was opened on `node`, for a display of `size` pixels.
    fn on(va: Arc<Va>, node: &Node, name: &str, size: (u32, u32)) -> Self {
        Self {
            device: D3d11Device {
                id: node.minor as usize,
            },
            display: Arc::new(Display {
                device_id: node.path.clone(),
                display_name: name.to_string(),
                friendly_name: name.to_string(),
                width: size.0,
                height: size.1,
                x: 0,
                y: 0,
                primary: true,
                adapter: node.name.clone(),
                adapter_index: 0,
                output_index: 0,
            }),
            vendor: node.vendor,
            va,
        }
    }

    /// The VA-API display the device's surfaces and encoders live on.
    pub(crate) fn va(&self) -> &Arc<Va> {
        &self.va
    }
}

/// An immutable, owned GPU snapshot of one captured picture. No capture produces one on
/// Linux yet.
#[derive(Clone)]
pub struct GpuImage {
    pub width: u32,
    pub height: u32,
    pub pixel: Pixel,
    /// Capture timestamp used by pacing and encoders.
    pub captured: Instant,
    /// The compositor's own stamp, for diagnostics only.
    pub wgc_stamp: Option<Instant>,
    /// When the host submitted its owned copy, before GPU completion.
    pub acquired: Instant,
    /// When the capture published the picture to the stream threads; until
    /// then, its acquisition. Only timing reports read this.
    pub published: Instant,
    pub gpu: Device,
    /// Whether the picture changed the foreground window: `None` when the capture cannot
    /// tell.
    pub focus: Option<bool>,
    /// This picture repeats the previous one: constant-rate capture published it again
    /// because nothing was composed for the stream's slot. Its stamps are the
    /// publication's, not a composition's.
    pub repeat: bool,
    /// The capture publication that first carried this content, when the capture publishes
    /// repeats: a repeat keeps it. 0 when the capture does not repeat.
    pub origin: u64,
    /// The VA-API surface that holds the picture, back in its pool when the last copy of the
    /// image is dropped.
    surface: Option<Arc<Pooled>>,
    /// The colour space the surface's YCbCr is in.
    colors: Colors,
}

/// A shown pointer a capture supplied beside a picture.
pub struct PointerShape<'a> {
    pub width: u32,
    pub height: u32,
    /// The point that clicks, from the shape's top left.
    pub hotspot: [i32; 2],
    /// Logic alpha (0 transparent, 128 inverts the screen by the pixel's color, 255
    /// opaque) for monochrome and masked shapes; straight alpha otherwise.
    pub logic: bool,
    /// `width * height` RGBA pixels, rows top to bottom; a shape seen again shares them.
    pub pixels: &'a Arc<[u8]>,
}

impl GpuImage {
    /// The pointer drawn into this picture, when the capture supplies it apart from the
    /// desktop and it is shown. No capture does on Linux.
    pub fn pointer(&self) -> Option<PointerShape<'_>> {
        None
    }
    /// Leave the pointer out of this picture, for a device that draws it itself. Only a
    /// pointer the capture supplies apart can be left out, and none is.
    pub fn leave_pointer_out(&mut self) {}
    /// The VA-API surface of the picture and the size of the part of it that is the picture.
    pub(crate) fn surface(&self) -> Option<&Arc<Pooled>> {
        self.surface.as_ref()
    }

    /// The picture in CPU memory, converted back to BGRA by video processing. `staging` keeps
    /// the converter between calls.
    pub fn readback(&self, staging: &mut Option<D3d11Texture>) -> Result<Image> {
        let surface = self
            .surface
            .as_ref()
            .context("this picture is not on a VA-API surface")?;
        let size = (self.width, self.height);
        let va = surface.va();
        let made = |texture: &D3d11Texture| (texture.size, texture.converter.colors());
        if staging.as_ref().map(made) != Some((size, self.colors)) {
            *staging = Some(D3d11Texture {
                converter: Converter::new(va, size.0, size.1)?.writing(self.colors),
                rgb: Pool::new(
                    va,
                    crate::vaapi::ffi::VA_RT_FORMAT_RGB32,
                    crate::vaapi::BGRX,
                    size.0,
                    size.1,
                    1,
                    1,
                )?,
                size,
            });
        }
        let texture = staging.as_ref().expect("made above");
        let target = texture.rgb.take().context("no surface for the readback")?;
        texture.converter.to_rgb(surface.id(), size, target.id())?;
        let mut bytes = Vec::new();
        va.download_bgra(target.id(), size.0, size.1, &mut bytes)?;
        Ok(Image {
            width: size.0,
            height: size.1,
            stride: size.0 as usize * 4,
            bytes,
            captured: self.captured,
            pixel: Pixel::Bgra8,
        })
    }
}

/// The grid a capture aligns its publications to: one every `period` from `anchor`.
pub struct ClaimGrid {
    pub anchor: Instant,
    pub period: Duration,
}

/// What a capture needs from the host: the virtual display driver's bridge and the
/// launcher of user-session helpers.
#[derive(Clone, Default)]
pub struct CaptureEnv {
    pub vdd: Option<Arc<dyn idd_push::VddBridge>>,
    pub user_launcher: Option<Arc<dyn crate::user_helper::UserLauncher>>,
}

/// Every top-level window of the desktop the signed-in user sees. None is listed on Linux
/// yet.
pub fn app_windows() -> Result<Vec<Candidate>> {
    Ok(Vec::new())
}

/// The capture target of window `id` of process `pid`. No window can be captured on Linux
/// yet.
pub fn app_window_target(_id: u64, _pid: u32, _client: bool) -> Result<String> {
    Err(crate::unsupported("Window capture"))
}

/// The desktop rectangle a window target's picture shows, as `[left, top, right, bottom]`,
/// for absolute input. `None`: no window is captured on Linux, so input never maps onto
/// one.
pub fn window_target_rect(_name: &str) -> Option<[i32; 4]> {
    None
}

/// Whether the desktop can be captured from a helper in the user's session. No such
/// helper exists on Linux.
pub fn wgc_desktop_available() -> bool {
    false
}

/// The capture helper's entry point, run in a process of its own. Linux has no capture
/// helper.
pub fn run_wgc_worker_with(
    _pipe: &str,
    _parent: u32,
    _launcher: &dyn crate::user_helper::UserLauncher,
) -> Result<()> {
    Err(crate::unsupported("The capture helper"))
}

/// A capture of one display or window: the picture of a gamescope session or of a PipeWire
/// node, or the place a lost capture leaves.
pub enum Capture {
    /// Holds no device: the place of a lost capture while a new one is made.
    Closed,
    /// A PipeWire node's DMA-BUF frames, converted into VA-API surfaces.
    PipeWire(Box<PipeWire>),
}

/// An open PipeWire capture with the device its frames are on.
pub struct PipeWire {
    capture: pw_capture::PipeWireCapture,
    device: Device,
    /// The gamescope session's folder, when the target is a session: its end ends the capture
    /// even when PipeWire does not say so.
    session: Option<std::path::PathBuf>,
    /// The KWin screencast of a `kwin-output:` target, which ends when this drops.
    kwin: Option<kwin::KwinStream>,
    last_check: Instant,
    /// Keeps the readback converter for `next_frame`.
    staging: Option<D3d11Texture>,
}

impl PipeWire {
    /// Opens the target on a device chosen like an encoder's: the configured adapter, else the
    /// GPU with the most video memory.
    fn open(name: &str, hdr: bool, config: &butterpollo_config::config::Config) -> Result<Self> {
        if hdr {
            return Err(crate::unsupported("HDR capture"));
        }
        let desktop = match desktop::parse(name) {
            Some(index) => {
                let cast = butterpollo_linux_io::portal::ScreenCast::open(
                    index.is_some(),
                    butterpollo_linux_io::portal::DIALOG_WAIT,
                )?;
                let monitor = desktop::pick(&cast.monitors, index)?;
                tracing::info!(
                    node = monitor.node,
                    ?monitor,
                    "desktop capture of a portal monitor"
                );
                Some((cast, monitor.node))
            }
            None => None,
        };
        let (node, session) = if let Some((_, node)) = &desktop {
            (*node, None)
        } else if name == GAMESCOPE_TARGET {
            let session = butterpollo_linux_io::gamescope::wait_for_node(GAMESCOPE_WAIT)?;
            (
                session
                    .node
                    .context("the gamescope session names no node")?,
                Some(session.folder),
            )
        } else if let Some(node) = butterpollo_linux_io::gamescope::node_of(name) {
            (node, None)
        } else if kwin::parse(name).is_some() {
            (0, None)
        } else {
            bail!(
                "this Linux host captures the targets \"{GAMESCOPE_TARGET}\", \"desktop\", \"desktop:<monitor>\" or \"kwin-output:<name>\", \"pipewire-node:<id>\", not {name:?}"
            );
        };
        let probe = Device::new_adapter(
            name,
            config.get("adapter_name", ""),
            config.get("adapter_pnp_id", ""),
        )?;
        // The picture the stream's encoder takes, which the stream set.
        let picture = butterpollo_media_policy::capture_policy::Picture::read(config);
        let mut kwin_stream = None;
        let capture = match (&desktop, kwin::parse(name)) {
            (Some((cast, node)), _) => {
                pw_capture::PipeWireCapture::open_portal(cast, *node, probe.va(), picture)?
            }
            (None, Some(output)) => {
                let (capture, stream) = kwin::open(output, probe.va(), picture)?;
                kwin_stream = Some(stream);
                capture
            }
            (None, None) => pw_capture::PipeWireCapture::open(node, probe.va(), picture)?,
        };
        let size = capture.size;
        let found = node::nodes()
            .iter()
            .find(|candidate| candidate.minor as usize == probe.key())
            .context("the GPU's render node is gone")?;
        let device = Device::on(probe.va().clone(), found, name, size);
        Ok(Self {
            capture,
            device,
            session,
            kwin: kwin_stream,
            last_check: Instant::now(),
            staging: None,
        })
    }

    fn next_gpu(&mut self, origin: u64) -> Result<Option<Arc<GpuImage>>> {
        if let Some(reason) = self.kwin.as_ref().and_then(|stream| stream.ended()) {
            bail!("{reason}");
        }
        let Some(frame) = self.capture.take()? else {
            // PipeWire may not tell when the producer is gone: the session's own record does.
            if let Some(folder) = &self.session
                && self.last_check.elapsed() >= Duration::from_millis(250)
            {
                self.last_check = Instant::now();
                if butterpollo_linux_io::gamescope::read(folder).is_none() {
                    bail!("the game's display closed: gamescope is no longer running");
                }
            }
            return Ok(None);
        };
        let (width, height) = self.capture.size;
        Ok(Some(Arc::new(GpuImage {
            width,
            height,
            pixel: Pixel::Bgra8,
            captured: frame.captured,
            wgc_stamp: None,
            acquired: frame.acquired,
            published: frame.published,
            gpu: self.device.clone(),
            focus: None,
            repeat: false,
            origin,
            surface: Some(frame.surface),
            colors: self.capture.colors(),
        })))
    }
}

/// Ends what the value holds, at the point the host drops it. The PipeWire capture stops its own
/// thread when it drops, so there is nothing more to close here.
impl Drop for Capture {
    fn drop(&mut self) {}
}

/// The desktop pointer a lost capture last saw, for the capture replacing it.
pub struct Pointer(());

impl Capture {
    /// The pointer to hand to the capture that replaces this one. A closed capture has
    /// none.
    pub fn pointer(&self) -> Option<Pointer> {
        None
    }
    /// Draw `pointer` until the capture reports the pointer itself. A closed capture draws
    /// nothing.
    pub fn resume_pointer(&mut self, _pointer: Pointer) -> Result<()> {
        Ok(())
    }
    /// How frames reach the GPU: `"dmabuf"` (imported in place), `"shm"` (memory buffers
    /// uploaded by the CPU, a desktop's fallback) or `"none"` yet.
    pub fn buffer_path(&self) -> &'static str {
        match self {
            Self::Closed => "none",
            Self::PipeWire(capture) => capture.capture.path(),
        }
    }
    /// The backend's name, as the capture setting names it.
    pub fn backend(&self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::PipeWire(_) => "pipewire",
        }
    }
    /// The grid a capture that publishes on one aligns to. A closed capture publishes
    /// nothing.
    pub fn set_claim_grid(&mut self, _grid: Arc<Mutex<ClaimGrid>>, _enabled: bool) {}
    /// When a held picture is due for publication. A closed capture holds none.
    pub fn publication_deadline(&self) -> Option<Instant> {
        None
    }
    /// Set when the capture announces a frame. A closed capture announces none.
    pub fn frame_signal(&self) -> Option<&crate::timing::Signal> {
        match self {
            Self::Closed => None,
            Self::PipeWire(capture) => Some(capture.capture.signal()),
        }
    }
    /// How long the caller waits before its next poll: `poll`, since a closed capture has
    /// no announcement to wait for.
    pub fn wait_interval(&self, _idle: Duration, poll: Duration) -> Duration {
        poll
    }
    /// Opens the capture of a stream, with the host's bridges: the target `gamescope`, `desktop`, `desktop:<n>`,
    /// `kwin-output:<name>` or `pipewire-node:<id>`. A window target is refused, as is any other.
    pub fn open_for_stream_with_env(
        name: &str,
        _kind: &str,
        hdr: bool,
        config: &butterpollo_config::config::Config,
        warnings: Arc<dyn butterpollo_media_policy::WarningSink>,
        _env: &CaptureEnv,
    ) -> Result<Self> {
        warnings.clear("capture_compute");
        warnings.clear("capture_helper_compute");
        warnings.clear("capture_adapter");
        warnings.clear("capture_idd_push");
        if WindowTarget::parse(name)?.is_some() {
            return Err(crate::unsupported("Window capture"));
        }
        Ok(Self::PipeWire(Box::new(PipeWire::open(name, hdr, config)?)))
    }
    /// Changes the target on the capture's own device, when it can: `None` when the capture
    /// has to be reopened instead, as every capture on Linux does.
    pub fn switch_on_device_with_vdd(
        &mut self,
        _name: &str,
        _kind: &str,
        _hdr: bool,
        _config: &butterpollo_config::config::Config,
        _warnings: Arc<dyn butterpollo_media_policy::WarningSink>,
        _vdd: Option<&dyn idd_push::VddBridge>,
    ) -> Option<Result<Self>> {
        None
    }
    /// A capture of display `name` with backend `kind`, for probes and tools.
    pub fn new_format(name: &str, _kind: &str, hdr: bool) -> Result<Self> {
        if WindowTarget::parse(name)?.is_some() {
            return Err(crate::unsupported("Window capture"));
        }
        let config = butterpollo_config::config::Config::default();
        Ok(Self::PipeWire(Box::new(PipeWire::open(
            name, hdr, &config,
        )?)))
    }
    /// The newest picture in CPU memory.
    pub fn next_frame(&mut self) -> Result<Option<Image>> {
        match self {
            Self::Closed => bail!("capture is closed"),
            Self::PipeWire(capture) => match capture.next_gpu(0)? {
                Some(image) => image.readback(&mut capture.staging).map(Some),
                None => Ok(None),
            },
        }
    }
    /// The newest picture, wrapped for the stream threads. `origin` is the publication
    /// identity the caller stamps into it when the capture publishes repeats; 0 when the
    /// capture does not repeat.
    pub fn next_gpu(&mut self, origin: u64) -> Result<Option<Arc<GpuImage>>> {
        match self {
            Self::Closed => bail!("capture is closed"),
            Self::PipeWire(capture) => capture.next_gpu(origin),
        }
    }
}
