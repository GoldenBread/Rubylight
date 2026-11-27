//! KMS capture: the picture a display controller shows, read from the kernel's DRM device
//! whichever compositor owns it (a signed-in user's, the login manager's), as the DMA-BUFs of
//! the framebuffer on the output's planes. Reading another process's framebuffer takes
//! `CAP_SYS_ADMIN`; without it the kernel lists the framebuffer but hands out no buffer.
//!
//! The device is opened read-only and never as DRM master: nothing here changes what is shown.
//! An open card keeps working across a switch to another session's compositor, which is how a
//! capture follows the login screen.
use anyhow::{Context, Result, bail};
use std::{
    fs::File,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::{Path, PathBuf},
};

/// `DRM_CLIENT_CAP_UNIVERSAL_PLANES`: list the primary and cursor planes too.
const CAP_UNIVERSAL_PLANES: u64 = 2;
/// `DRM_MODE_FB_MODIFIERS`: the framebuffer's modifier fields are set.
const FB_MODIFIERS: u32 = 1 << 1;
/// `DRM_FORMAT_MOD_INVALID`.
pub const MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;
/// `DRM_MODE_CONNECTED`.
const CONNECTED: u32 = 1;

const fn iowr(nr: u64, size: usize) -> u64 {
    (3 << 30) | ((size as u64) << 16) | ((b'd' as u64) << 8) | nr
}
const fn iow(nr: u64, size: usize) -> u64 {
    (1 << 30) | ((size as u64) << 16) | ((b'd' as u64) << 8) | nr
}

#[repr(C)]
#[derive(Default)]
struct SetClientCap {
    capability: u64,
    value: u64,
}

#[repr(C)]
#[derive(Default)]
struct CardRes {
    fb_id_ptr: u64,
    crtc_id_ptr: u64,
    connector_id_ptr: u64,
    encoder_id_ptr: u64,
    count_fbs: u32,
    count_crtcs: u32,
    count_connectors: u32,
    count_encoders: u32,
    min_width: u32,
    max_width: u32,
    min_height: u32,
    max_height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ModeInfo {
    clock: u32,
    hdisplay: u16,
    hsync_start: u16,
    hsync_end: u16,
    htotal: u16,
    hskew: u16,
    vdisplay: u16,
    vsync_start: u16,
    vsync_end: u16,
    vtotal: u16,
    vscan: u16,
    vrefresh: u32,
    flags: u32,
    kind: u32,
    name: [u8; 32],
}

#[repr(C)]
#[derive(Default)]
struct GetConnector {
    encoders_ptr: u64,
    modes_ptr: u64,
    props_ptr: u64,
    prop_values_ptr: u64,
    count_modes: u32,
    count_props: u32,
    count_encoders: u32,
    encoder_id: u32,
    connector_id: u32,
    connector_type: u32,
    connector_type_id: u32,
    connection: u32,
    mm_width: u32,
    mm_height: u32,
    subpixel: u32,
    pad: u32,
}

#[repr(C)]
#[derive(Default)]
struct GetEncoder {
    encoder_id: u32,
    encoder_type: u32,
    crtc_id: u32,
    possible_crtcs: u32,
    possible_clones: u32,
}

#[repr(C)]
#[derive(Default)]
struct GetCrtc {
    set_connectors_ptr: u64,
    count_connectors: u32,
    crtc_id: u32,
    fb_id: u32,
    x: u32,
    y: u32,
    gamma_size: u32,
    mode_valid: u32,
    mode: ModeInfo,
}

#[repr(C)]
#[derive(Default)]
struct PlaneRes {
    plane_id_ptr: u64,
    count_planes: u32,
}

#[repr(C)]
#[derive(Default)]
struct GetPlane {
    plane_id: u32,
    crtc_id: u32,
    fb_id: u32,
    possible_crtcs: u32,
    gamma_size: u32,
    count_format_types: u32,
    format_type_ptr: u64,
}

#[repr(C)]
#[derive(Default)]
struct FbCmd2 {
    fb_id: u32,
    width: u32,
    height: u32,
    pixel_format: u32,
    flags: u32,
    handles: [u32; 4],
    pitches: [u32; 4],
    offsets: [u32; 4],
    modifier: [u64; 4],
}

#[repr(C)]
#[derive(Default)]
struct PrimeHandle {
    handle: u32,
    flags: u32,
    fd: i32,
}

#[repr(C)]
#[derive(Default)]
struct GemClose {
    handle: u32,
    pad: u32,
}

const SET_CLIENT_CAP: u64 = iow(0x0d, size_of::<SetClientCap>());
const GEM_CLOSE: u64 = iow(0x09, size_of::<GemClose>());
const PRIME_HANDLE_TO_FD: u64 = iowr(0x2d, size_of::<PrimeHandle>());
const GET_RESOURCES: u64 = iowr(0xa0, size_of::<CardRes>());
const GET_CRTC: u64 = iowr(0xa1, size_of::<GetCrtc>());
const GET_ENCODER: u64 = iowr(0xa6, size_of::<GetEncoder>());
const GET_CONNECTOR: u64 = iowr(0xa7, size_of::<GetConnector>());
const GET_PLANE_RESOURCES: u64 = iowr(0xb5, size_of::<PlaneRes>());
const GET_PLANE: u64 = iowr(0xb6, size_of::<GetPlane>());
const GET_FB2: u64 = iowr(0xce, size_of::<FbCmd2>());

/// The kernel's name for a connector type, as in `DP-1`.
pub fn connector_type_name(kind: u32) -> &'static str {
    match kind {
        1 => "VGA",
        2 => "DVI-I",
        3 => "DVI-D",
        4 => "DVI-A",
        5 => "Composite",
        6 => "SVIDEO",
        7 => "LVDS",
        8 => "Component",
        9 => "DIN",
        10 => "DP",
        11 => "HDMI-A",
        12 => "HDMI-B",
        13 => "TV",
        14 => "eDP",
        15 => "Virtual",
        16 => "DSI",
        17 => "DPI",
        18 => "Writeback",
        19 => "SPI",
        20 => "USB",
        _ => "Unknown",
    }
}

/// A DRM card opened for reading what it shows.
pub struct Card {
    file: File,
    path: PathBuf,
}

/// An output that shows something: its connector and the controller driving it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    /// As KWin and sysfs name it, `DP-1`.
    pub name: String,
    pub connector_id: u32,
    pub crtc_id: u32,
    /// The controller's mode: width, height, refresh in millihertz.
    pub mode: (u32, u32, u32),
}

/// A plane on an output's controller, with the framebuffer it scans out now (0 for none).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaneState {
    pub plane_id: u32,
    pub fb_id: u32,
}

/// One DMA-BUF plane of a grabbed framebuffer.
#[derive(Debug)]
pub struct GrabPlane {
    pub fd: OwnedFd,
    pub pitch: u32,
    pub offset: u32,
}

/// A framebuffer exported as DMA-BUFs. The buffers stay the compositor's: it may draw into
/// them again once it has flipped away from them.
#[derive(Debug)]
pub struct Grab {
    pub fb_id: u32,
    pub width: u32,
    pub height: u32,
    /// The DRM fourcc, such as `XR24`.
    pub fourcc: u32,
    /// [`MOD_INVALID`] when the framebuffer declares none.
    pub modifier: u64,
    /// One per framebuffer plane; planes that share a buffer object have their own
    /// descriptor for it.
    pub planes: Vec<GrabPlane>,
}

/// A fourcc as its four letters.
pub fn fourcc_text(code: u32) -> String {
    code.to_le_bytes()
        .iter()
        .map(|&b| if b.is_ascii_graphic() { b as char } else { '?' })
        .collect()
}

impl Card {
    /// Opens `path` (`/dev/dri/cardN`) read-only and asks for every plane.
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
        let card = Self {
            file,
            path: path.to_owned(),
        };
        let mut cap = SetClientCap {
            capability: CAP_UNIVERSAL_PLANES,
            value: 1,
        };
        card.ioctl(SET_CLIENT_CAP, &mut cap)
            .context("asking for universal planes")?;
        Ok(card)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn ioctl<T>(&self, request: u64, arg: &mut T) -> std::io::Result<()> {
        loop {
            // SAFETY: `arg` is the `#[repr(C)]` structure this request reads and writes; any
            // pointers in it point at live buffers sized by its counts.
            let status = unsafe {
                libc::ioctl(
                    self.file.as_raw_fd(),
                    request as libc::c_ulong,
                    std::ptr::from_mut(arg),
                )
            };
            if status == 0 {
                return Ok(());
            }
            let error = std::io::Error::last_os_error();
            if !matches!(error.raw_os_error(), Some(libc::EINTR | libc::EAGAIN)) {
                return Err(error);
            }
        }
    }

    /// The connected outputs that show something, by connector id.
    pub fn outputs(&self) -> Result<Vec<Output>> {
        let mut res = CardRes::default();
        self.ioctl(GET_RESOURCES, &mut res)
            .context("reading the card's resources")?;
        let mut connectors = vec![0u32; res.count_connectors as usize];
        let mut res = CardRes {
            connector_id_ptr: connectors.as_mut_ptr() as u64,
            count_connectors: connectors.len() as u32,
            ..CardRes::default()
        };
        self.ioctl(GET_RESOURCES, &mut res)
            .context("listing the card's connectors")?;
        connectors.truncate(res.count_connectors as usize);
        let mut outputs = Vec::new();
        for id in connectors {
            // Room for one mode: a count of 0 would ask the kernel to probe the monitor.
            let mut mode = ModeInfo::default();
            let mut connector = GetConnector {
                connector_id: id,
                modes_ptr: std::ptr::from_mut(&mut mode) as u64,
                count_modes: 1,
                ..GetConnector::default()
            };
            if self.ioctl(GET_CONNECTOR, &mut connector).is_err()
                || connector.connection != CONNECTED
                || connector.encoder_id == 0
            {
                continue;
            }
            let mut encoder = GetEncoder {
                encoder_id: connector.encoder_id,
                ..GetEncoder::default()
            };
            if self.ioctl(GET_ENCODER, &mut encoder).is_err() || encoder.crtc_id == 0 {
                continue;
            }
            let mut crtc = GetCrtc {
                crtc_id: encoder.crtc_id,
                ..GetCrtc::default()
            };
            if self.ioctl(GET_CRTC, &mut crtc).is_err() || crtc.mode_valid == 0 {
                continue;
            }
            outputs.push(Output {
                name: format!(
                    "{}-{}",
                    connector_type_name(connector.connector_type),
                    connector.connector_type_id
                ),
                connector_id: id,
                crtc_id: encoder.crtc_id,
                mode: (
                    u32::from(crtc.mode.hdisplay),
                    u32::from(crtc.mode.vdisplay),
                    refresh_millihz(&crtc.mode),
                ),
            });
        }
        Ok(outputs)
    }

    /// The planes on `crtc_id` now, primary first as the kernel lists them.
    pub fn planes(&self, crtc_id: u32) -> Result<Vec<PlaneState>> {
        let mut res = PlaneRes::default();
        self.ioctl(GET_PLANE_RESOURCES, &mut res)
            .context("counting the card's planes")?;
        let mut ids = vec![0u32; res.count_planes as usize];
        let mut res = PlaneRes {
            plane_id_ptr: ids.as_mut_ptr() as u64,
            count_planes: ids.len() as u32,
        };
        self.ioctl(GET_PLANE_RESOURCES, &mut res)
            .context("listing the card's planes")?;
        ids.truncate(res.count_planes as usize);
        let mut planes = Vec::new();
        for plane_id in ids {
            let mut plane = GetPlane {
                plane_id,
                ..GetPlane::default()
            };
            self.ioctl(GET_PLANE, &mut plane)
                .with_context(|| format!("reading plane {plane_id}"))?;
            if plane.crtc_id == crtc_id {
                planes.push(PlaneState {
                    plane_id,
                    fb_id: plane.fb_id,
                });
            }
        }
        Ok(planes)
    }

    /// The framebuffer the primary plane of `crtc_id` shows now, or 0.
    pub fn primary_fb(&self, crtc_id: u32) -> Result<u32> {
        let mut crtc = GetCrtc {
            crtc_id,
            ..GetCrtc::default()
        };
        self.ioctl(GET_CRTC, &mut crtc)
            .context("reading the display controller")?;
        Ok(crtc.fb_id)
    }

    /// Exports framebuffer `fb_id` as DMA-BUFs. Fails without `CAP_SYS_ADMIN`, when the
    /// kernel withholds the framebuffer's buffers.
    pub fn grab(&self, fb_id: u32) -> Result<Grab> {
        let mut fb = FbCmd2 {
            fb_id,
            ..FbCmd2::default()
        };
        self.ioctl(GET_FB2, &mut fb)
            .with_context(|| format!("reading framebuffer {fb_id}"))?;
        let count = fb.handles.iter().take_while(|&&h| h != 0).count();
        let close = |handles: &[u32]| {
            let mut closed: Vec<u32> = Vec::new();
            for &handle in handles {
                if handle != 0 && !closed.contains(&handle) {
                    closed.push(handle);
                    let _ = self.ioctl(
                        GEM_CLOSE,
                        &mut GemClose {
                            handle,
                            ..GemClose::default()
                        },
                    );
                }
            }
        };
        if count == 0 {
            bail!(
                "framebuffer {fb_id} came without its buffers: reading another program's picture needs CAP_SYS_ADMIN"
            );
        }
        let mut planes = Vec::with_capacity(count);
        for index in 0..count {
            let mut prime = PrimeHandle {
                handle: fb.handles[index],
                flags: libc::O_CLOEXEC as u32,
                fd: -1,
            };
            if let Err(error) = self.ioctl(PRIME_HANDLE_TO_FD, &mut prime) {
                close(&fb.handles);
                return Err(error).context("exporting the framebuffer as a DMA-BUF");
            }
            planes.push(GrabPlane {
                // SAFETY: the kernel just made this descriptor for us.
                fd: unsafe { OwnedFd::from_raw_fd(prime.fd) },
                pitch: fb.pitches[index],
                offset: fb.offsets[index],
            });
        }
        close(&fb.handles);
        Ok(Grab {
            fb_id,
            width: fb.width,
            height: fb.height,
            fourcc: fb.pixel_format,
            modifier: if fb.flags & FB_MODIFIERS != 0 {
                fb.modifier[0]
            } else {
                MOD_INVALID
            },
            planes,
        })
    }
}

/// The mode's refresh in millihertz, from its timings, or its rounded rate.
fn refresh_millihz(mode: &ModeInfo) -> u32 {
    let total = u64::from(mode.htotal) * u64::from(mode.vtotal);
    if total == 0 {
        return mode.vrefresh * 1000;
    }
    // The clock is in kHz.
    (u64::from(mode.clock) * 1_000_000 / total) as u32
}

/// The cards under /dev/dri with their connectors' sysfs names, such as `card1`.
pub fn cards() -> Vec<PathBuf> {
    let mut cards: Vec<_> = std::fs::read_dir("/dev/dri")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("card"))
        })
        .collect();
    cards.sort();
    cards
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structures_match_the_kernels() {
        // Sizes from include/uapi/drm/drm_mode.h on x86_64 and aarch64.
        assert_eq!(size_of::<ModeInfo>(), 68);
        assert_eq!(size_of::<CardRes>(), 64);
        assert_eq!(size_of::<GetConnector>(), 80);
        assert_eq!(size_of::<GetEncoder>(), 20);
        assert_eq!(size_of::<GetCrtc>(), 104);
        assert_eq!(size_of::<PlaneRes>(), 16);
        assert_eq!(size_of::<GetPlane>(), 32);
        assert_eq!(size_of::<FbCmd2>(), 104);
        assert_eq!(size_of::<PrimeHandle>(), 12);
        // The request numbers libdrm uses.
        assert_eq!(GET_FB2, 0xc068_64ce);
        assert_eq!(PRIME_HANDLE_TO_FD, 0xc00c_642d);
        assert_eq!(GET_CRTC, 0xc068_64a1);
        assert_eq!(GET_PLANE, 0xc020_64b6);
    }

    #[test]
    fn names_and_rates_read_as_the_kernel_says() {
        assert_eq!(connector_type_name(10), "DP");
        assert_eq!(connector_type_name(11), "HDMI-A");
        assert_eq!(connector_type_name(15), "Virtual");
        assert_eq!(fourcc_text(0x3432_5258), "XR24");
        // CVT 1920x1080 at 60 Hz: 148.5 MHz over 2200x1125.
        let mode = ModeInfo {
            clock: 148_500,
            htotal: 2200,
            vtotal: 1125,
            vrefresh: 60,
            ..ModeInfo::default()
        };
        assert_eq!(refresh_millihz(&mode), 60_000);
        let unknown = ModeInfo {
            vrefresh: 75,
            ..ModeInfo::default()
        };
        assert_eq!(refresh_millihz(&unknown), 75_000);
    }

    /// Lists what each card shows and tries to export it: without `CAP_SYS_ADMIN` the export
    /// must fail with the reason, not hand out another program's picture.
    #[test]
    #[ignore = "needs a DRM card with a lit output"]
    fn live_outputs_are_listed() {
        for path in cards() {
            let Ok(card) = Card::open(&path) else {
                continue;
            };
            for output in card.outputs().unwrap() {
                let fb = card.primary_fb(output.crtc_id).unwrap();
                eprintln!(
                    "{} {output:?} fb {fb} planes {:?}",
                    path.display(),
                    card.planes(output.crtc_id).unwrap()
                );
                match card.grab(fb) {
                    Ok(grab) => eprintln!(
                        "  {}x{} {} modifier 0x{:x} planes {}",
                        grab.width,
                        grab.height,
                        fourcc_text(grab.fourcc),
                        grab.modifier,
                        grab.planes.len()
                    ),
                    Err(error) => eprintln!("  {error:#}"),
                }
            }
        }
    }
}
