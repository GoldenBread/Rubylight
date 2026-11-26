//! Display decisions that do not need Windows: whether a session gets a
//! virtual display, which mode to report, stable virtual display identities,
//! render sizes, letterboxing and display arrangements.
use anyhow::{Result, bail};

mod arrangement;
mod watch;

pub use arrangement::{
    Arrangement, HostDisplay, beside_the_users, chosen_layout, layout_mismatch, recalled_over,
    switched_back_on,
};
pub use watch::{
    HeartbeatThrottle, LayoutAction, LayoutWatch, PrimaryAction, PrimaryChange, PrimaryWatch,
    primary_moved,
};

/// A client request can enable a virtual display; an absent request leaves
/// the host, app and device settings in control, as does `virtualDisplay=0`,
/// which Apollo-compatible clients send whenever their own toggle is off.
#[derive(Default)]
pub struct VirtualDisplayRequest<'a> {
    pub client_requested: bool,
    pub client_forced: bool,
    pub app_requested: bool,
    pub configured: bool,
    pub output_override: Option<&'a str>,
    pub configured_output: &'a str,
    /// The client asked to stream a physical display (`hostDisplay=physical`):
    /// the host, app and per-device settings that would add a virtual display
    /// are overridden for this stream. A headless host and an output that
    /// names the virtual display still use one; there is nothing else to show.
    pub client_physical: bool,
    /// No display is active (a headless host, or every monitor off): there
    /// is no physical display to stream, so a virtual one is used whatever
    /// the settings say, as in Vibepollo.
    pub headless: bool,
}
impl VirtualDisplayRequest<'_> {
    /// Whether the stream gets a virtual display. Without a usable driver a
    /// request from the client, the app or the host settings streams the
    /// physical display (the caller warns), as Apollo and Vibepollo do: an
    /// Artemis client asked to proceed without the driver must still get a
    /// stream. Only an output that names the virtual display itself has no
    /// physical display to fall back to.
    pub fn uses_virtual(
        &self,
        physical_only: bool,
        driver_available: impl FnOnce() -> bool,
    ) -> bool {
        !physical_only && self.requested() && (self.output_virtual() || driver_available())
    }
    pub fn output_virtual(&self) -> bool {
        matches!(
            self.output_override
                .unwrap_or(self.configured_output)
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "sunshine:virtual_display" | "virtual" | "virtual_display" | "virtual-display"
        )
    }
    pub fn requested(&self) -> bool {
        if self.client_physical {
            return self.headless || self.output_virtual();
        }
        self.client_requested
            || self.client_forced
            || self.headless
            || self.output_virtual()
            || (self.output_override.is_none() && (self.app_requested || self.configured))
    }
}

/// The physical display mode to apply for a stream, given the modes the display
/// supports as (width, height, refresh Hz), the requested resolution and refresh
/// (millihertz), and the current mode.
///
/// Windows substitutes an arbitrary mode for one a display does not support:
/// a 2560x1600 request on a 32:9 monitor became 3840x1080 at 60 Hz, halving a
/// 120 fps stream. An unsupported resolution keeps the current one (the stream
/// is scaled on the GPU), and a refresh is applied only where it exists.
pub fn physical_mode(
    supported: &[(u32, u32, u32)],
    requested: (Option<(u32, u32)>, Option<u32>),
    current: (u32, u32, u32),
) -> (u32, u32, u32) {
    let (current_width, current_height, current_rate) = current;
    let hz = |millihertz: u32| millihertz.saturating_add(500) / 1000;
    let (width, height) = requested
        .0
        .filter(|(w, h)| supported.iter().any(|m| m.0 == *w && m.1 == *h))
        .unwrap_or((current_width, current_height));
    let rate = requested
        .1
        .filter(|rate| {
            supported
                .iter()
                .any(|m| m.0 == width && m.1 == height && m.2 == hz(*rate))
        })
        .or_else(|| {
            // Keep the current refresh when it exists at the chosen resolution;
            // otherwise the highest that does.
            let at = |rate: u32| {
                supported
                    .iter()
                    .any(|m| m.0 == width && m.1 == height && m.2 == hz(rate))
            };
            if at(current_rate) {
                Some(current_rate)
            } else {
                supported
                    .iter()
                    .filter(|m| m.0 == width && m.1 == height)
                    .map(|m| m.2 * 1000)
                    .max()
            }
        })
        .unwrap_or(current_rate);
    (width, height, rate)
}
pub fn report_mode(
    warnings: &(impl crate::WarningSink + ?Sized),
    requested: (Option<(u32, u32)>, Option<u32>, Option<bool>),
    actual: (u32, u32, u32, bool),
    stream_rate: u32,
) {
    report_mode_with(warnings, requested, actual, stream_rate, true);
}

/// Like [`report_mode`], but leaves `"display_hdr"` untouched when the stream already fell back
/// from HDR to SDR and wrote its own fallback warning.
pub fn report_mode_keeping_hdr(
    warnings: &(impl crate::WarningSink + ?Sized),
    requested: (Option<(u32, u32)>, Option<u32>, Option<bool>),
    actual: (u32, u32, u32, bool),
    stream_rate: u32,
) {
    report_mode_with(warnings, requested, actual, stream_rate, false);
}

fn report_mode_with(
    warnings: &(impl crate::WarningSink + ?Sized),
    requested: (Option<(u32, u32)>, Option<u32>, Option<bool>),
    actual: (u32, u32, u32, bool),
    stream_rate: u32,
    check_hdr: bool,
) {
    if let Some((width, height)) = requested.0
        && (width, height) != (actual.0, actual.1)
    {
        warnings.set("display_resolution", &format!("Display resolution {width}x{height} was not applied; using {}x{} and scaling the stream. The mode may be unsupported or shared with another stream; select a supported mode or a virtual display.", actual.0, actual.1));
    } else {
        warnings.clear("display_resolution");
    }
    if requested
        .1
        .is_some_and(|rate| rate.abs_diff(actual.2) > 500)
        || actual.2.saturating_add(500) < stream_rate
    {
        warnings.set("display_refresh", &format!("Display refresh is {:.3} Hz for a {:.3} fps stream; the requested rate may be unsupported or held by another stream. Fresh frames cannot exceed the display rate. Select a supported refresh or a virtual display.", f64::from(actual.2) / 1000., f64::from(stream_rate) / 1000.));
    } else {
        warnings.clear("display_refresh");
    }
    if check_hdr {
        if requested.2.is_some_and(|hdr| hdr != actual.3) {
            warnings.set("display_hdr", &format!("Requested display HDR state was not applied; capture source HDR is {}. The display may lack HDR or be shared. Check Windows HDR and the display policy, or reconnect with HDR disabled.", if actual.3 { "on" } else { "off" }));
        } else {
            warnings.clear("display_hdr");
        }
    }
}

/// Preserve libdisplaydevice's UUIDv5 identity, including its UTF-16 byte order
/// and removal of the unstable parent portion of Windows' instance ID.
pub fn legacy_device_id(path: &str, instance: Option<&str>, edid: &[u8]) -> String {
    let mut bytes = Vec::new();
    if let Some(instance) = instance {
        let separators: Vec<_> = instance
            .match_indices('&')
            .map(|(at, _)| at)
            .take(3)
            .collect();
        if separators.len() == 3 {
            bytes.extend_from_slice(edid);
            for part in [&instance[..separators[1]], &instance[separators[2]..]] {
                bytes.extend(part.encode_utf16().flat_map(u16::to_le_bytes));
            }
        }
    }
    if bytes.is_empty() {
        bytes.extend(path.encode_utf16().flat_map(u16::to_le_bytes));
    }
    format!("{{{}}}", uuid::Uuid::new_v5(&uuid::Uuid::nil(), &bytes))
}
fn fnv(bytes: impl IntoIterator<Item = u8>) -> u64 {
    bytes.into_iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
}
/// Match uuid_util::parse's Windows GUID byte order and the previous host's
/// deterministic fallback domains. The driver hashes those 16 memory bytes.
pub fn virtual_identity(stable_id: &str) -> [u8; 16] {
    if let Ok(uuid) = uuid::Uuid::parse_str(stable_id) {
        return uuid.to_bytes_le();
    }
    let mut bytes = [0; 16];
    for (at, domain) in [
        (0, "sunshine-virtual-display-a:"),
        (8, "sunshine-virtual-display-b:"),
    ] {
        bytes[at..at + 8]
            .copy_from_slice(&fnv(domain.bytes().chain(stable_id.bytes())).to_le_bytes());
    }
    bytes[6] = (bytes[6] & 15) | 0x50;
    bytes[8] = (bytes[8] & 63) | 0x80;
    bytes
}
pub fn virtual_display_id(stable_id: &str) -> u64 {
    fnv(virtual_identity(stable_id)).max(1)
}
pub fn app_client_identity(app: &str, client: &str) -> String {
    let mut bytes = virtual_identity(app);
    for (byte, client_byte) in bytes.iter_mut().zip(virtual_identity(client)) {
        *byte ^= client_byte;
    }
    uuid::Uuid::from_bytes_le(bytes).to_string()
}

/// A non-default application scale takes precedence over the client's scale.
/// The previous host rounded down to even dimensions and ignored overflow.
pub fn render_dimensions(width: u32, height: u32, client_scale: i64, app_scale: i64) -> (u32, u32) {
    let scale = if app_scale != 100 {
        app_scale
    } else {
        client_scale
    };
    if scale <= 0 || scale == 100 {
        return (width, height);
    }
    let dimension = |value: u32| {
        u64::from(value)
            .checked_mul(scale as u64)
            .map(|value| (value / 100) & !1)
            .filter(|value| *value > 0 && *value <= i32::MAX as u64)
            .map(|value| value as u32)
    };
    match (dimension(width), dimension(height)) {
        (Some(width), Some(height)) => (width, height),
        _ => (width, height),
    }
}
/// The launch option a client sets the Windows scale (text and interface
/// size) of its virtual display with, in percent. `scaleFactor` is a
/// different thing: it multiplies the size the PC renders at
/// ([`render_dimensions`]).
pub const DISPLAY_SCALE_PARAMETER: &str = "rlDisplayScale";
/// The serverinfo element that says the host reads [`DISPLAY_SCALE_PARAMETER`].
pub const DISPLAY_SCALE_SERVERINFO_FIELD: &str = "DisplayScaleVersion";
/// The serverinfo element that says the host keeps a device's second screen beside its first
/// stream: a launch with `rlSecondScreen=1`.
pub const SECOND_SCREEN_SERVERINFO_FIELD: &str = "SecondScreenVersion";
/// The display scales Windows offers, in percent.
pub const DISPLAY_SCALES: [u32; 12] = [100, 125, 150, 175, 200, 225, 250, 300, 350, 400, 450, 500];

/// The client's `rlDisplayScale` choice: None when it sent none, an error for
/// a scale Windows does not offer. A choice comes before the device, app and
/// host `dd_virtual_display_scale`, as the client's display arrangement does.
pub fn client_display_scale(
    options: &std::collections::BTreeMap<String, String>,
) -> Result<Option<i64>> {
    let Some(value) = options.get(DISPLAY_SCALE_PARAMETER) else {
        return Ok(None);
    };
    match value.trim().parse::<u32>() {
        Ok(percent) if DISPLAY_SCALES.contains(&percent) => Ok(Some(i64::from(percent))),
        _ => bail!(
            "invalid {DISPLAY_SCALE_PARAMETER} {value:?}: Windows offers {DISPLAY_SCALES:?} percent"
        ),
    }
}

/// Where a source of another shape sits in the stream, as `(x, y, width,
/// height)`: centred, keeping its aspect ratio, with black bars around it.
/// Sizes and offsets are even for 4:2:0 chroma. A shape within a pixel of
/// the stream's fills it, as in the GPU converter.
pub fn letterbox(source: (u32, u32), target: (u32, u32)) -> (u32, u32, u32, u32) {
    let full = (0, 0, target.0, target.1);
    if source.0 == 0 || source.1 == 0 || source == target {
        return full;
    }
    let (sw, sh) = (f64::from(source.0), f64::from(source.1));
    let (tw, th) = (f64::from(target.0), f64::from(target.1));
    let scale = (tw / sw).min(th / sh);
    let (w, h) = (sw * scale, sh * scale);
    if (w - tw).abs() < 1. && (h - th).abs() < 1. {
        return full;
    }
    let even = |value: f64, limit: u32| ((value.round() as u32) & !1).max(2).min(limit);
    let (w, h) = (even(w, target.0), even(h, target.1));
    (((target.0 - w) / 2) & !1, ((target.1 - h) / 2) & !1, w, h)
}
/// Where the display's picture sits in the stream, or None when it fills the
/// stream: a display of another shape is letterboxed (see `letterbox`).
pub fn picture(display: (u32, u32), stream: (u32, u32)) -> Option<(u32, u32, u32, u32)> {
    if stream.0 == 0 || stream.1 == 0 {
        return None;
    }
    let picture = letterbox(display, stream);
    (picture != (0, 0, stream.0, stream.1)).then_some(picture)
}
/// A point the client gives as fractions of the whole stream, black bars
/// included, as fractions of the picture, that is of the display. A point on
/// a bar moves to the picture's edge.
pub fn stream_to_picture(
    point: (f64, f64),
    stream: (u32, u32),
    picture: (u32, u32, u32, u32),
) -> (f64, f64) {
    let map = |value: f64, total: u32, offset: u32, size: u32| {
        ((value.clamp(0., 1.) * f64::from(total) - f64::from(offset)) / f64::from(size.max(1)))
            .clamp(0., 1.)
    };
    (
        map(point.0, stream.0, picture.0, picture.2),
        map(point.1, stream.1, picture.1, picture.3),
    )
}
/// A device's own display mode, `WIDTHxHEIGHTxREFRESH` (refresh in Hz, up to
/// three decimals, e.g. `1920x1080x59.94`).
pub fn parse_display_mode(text: &str) -> Option<(u32, u32, crate::framegen::Rate)> {
    let mut parts = text.trim().split(['x', 'X']);
    let width = parts.next()?.trim().parse::<u32>().ok()?;
    let height = parts.next()?.trim().parse::<u32>().ok()?;
    let rate = crate::framegen::Rate::parse(parts.next()?).ok()?;
    (parts.next().is_none()
        && (1..=16384).contains(&width)
        && (1..=16384).contains(&height)
        && rate.0 >= 1000)
        .then_some((width, height, rate))
}
pub use butterpollo_config::config::{
    MAX_STREAM_RESOLUTION, MAX_STREAM_RESOLUTION_NATIVE, MAX_STREAM_RESOLUTION_RANGE,
    parse_max_stream_resolution, parse_resolution,
};

pub struct DisplayRequest {
    pub resolution: Option<(u32, u32)>,
    pub refresh: Option<u32>,
    pub prefer_highest: bool,
    pub hdr: Option<bool>,
}

pub fn display_request(
    config: &butterpollo_config::config::Config,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<DisplayRequest> {
    let mut request = display_request_rate(
        config,
        width,
        height,
        crate::framegen::Rate(fps.saturating_mul(1000)),
        false,
        false,
    )?;
    request.refresh = request.refresh.map(|r| r.saturating_add(500) / 1000);
    Ok(request)
}

pub fn display_request_rate(
    config: &butterpollo_config::config::Config,
    width: u32,
    height: u32,
    rate: crate::framegen::Rate,
    hdr: bool,
    virtual_display: bool,
) -> Result<DisplayRequest> {
    use anyhow::Context;
    use butterpollo_config::config::{HDR_OPTION, fallback};
    if !virtual_display && config.get("dd_configuration_option", "verify_only") == "disabled" {
        return Ok(DisplayRequest {
            resolution: None,
            refresh: None,
            prefer_highest: false,
            hdr: None,
        });
    }
    let resolution_option = config.get("dd_resolution_option", "auto");
    let refresh_option = config.get("dd_refresh_rate_option", "auto");
    let resolution_option = match resolution_option {
        "disabled" | "auto" | "manual" => resolution_option,
        other => {
            fallback("dd_resolution_option", other, "auto");
            "auto"
        }
    };
    let refresh_option = match refresh_option {
        "disabled" | "auto" | "manual" | "prefer_highest" => refresh_option,
        other => {
            fallback("dd_refresh_rate_option", other, "auto");
            "auto"
        }
    };
    let mut resolution = match resolution_option {
        "disabled" => None,
        "auto" => Some((width, height)),
        "manual" => {
            let (width, height) = config
                .get("dd_manual_resolution", "")
                .split_once('x')
                .context("manual resolution must be WIDTHxHEIGHT")?;
            Some((width.trim().parse()?, height.trim().parse()?))
        }
        _ => bail!("invalid display resolution policy"),
    };
    let mut refresh = match refresh_option {
        "disabled" => None,
        "auto" => Some(rate.0),
        "manual" => {
            let rate = crate::framegen::Rate::parse(config.get("dd_manual_refresh_rate", ""))?;
            if rate.0 < 1000 {
                bail!("invalid manual refresh rate");
            }
            Some(rate.0)
        }
        "prefer_highest" => None,
        _ => bail!("invalid display refresh policy"),
    };
    let remapping_type = match (resolution_option == "auto", refresh_option == "auto") {
        (true, true) => Some("mixed"),
        (true, false) => Some("resolution_only"),
        (false, true) => Some("refresh_rate_only"),
        _ => None,
    };
    if let Some(kind) = remapping_type
        && let Some(value) = config.values.get("dd_mode_remapping")
    {
        let mappings: serde_json::Value =
            serde_json::from_str(value).context("invalid display mode remapping")?;
        if let Some(entries) = mappings.get(kind).and_then(serde_json::Value::as_array) {
            for entry in entries {
                let text = |key: &str| {
                    entry
                        .get(key)
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                        .trim()
                };
                let parse_remapped = |s: &str| -> Result<(u32, u32)> {
                    let (w, h) = s
                        .split_once('x')
                        .context("remapping resolution must be WIDTHxHEIGHT")?;
                    Ok((w.trim().parse()?, h.trim().parse()?))
                };
                if kind != "refresh_rate_only"
                    && !text("requested_resolution").is_empty()
                    && resolution != Some(parse_remapped(text("requested_resolution"))?)
                {
                    continue;
                }
                if kind != "resolution_only"
                    && !text("requested_fps").is_empty()
                    && refresh != Some(crate::framegen::Rate::parse(text("requested_fps"))?.0)
                {
                    continue;
                }
                if kind != "refresh_rate_only" && !text("final_resolution").is_empty() {
                    resolution = Some(parse_remapped(text("final_resolution"))?);
                }
                if kind != "resolution_only" && !text("final_refresh_rate").is_empty() {
                    refresh = Some(crate::framegen::Rate::parse(text("final_refresh_rate"))?.0);
                }
                break;
            }
        }
    }
    if let Some((w, h)) =
        resolution.filter(|&(w, h)| !(320..=7680).contains(&w) || !(200..=4320).contains(&h))
    {
        bail!("display resolution {w}x{h} is outside its limits (320x200..=7680x4320)");
    }
    if let Some(f) = refresh.filter(|&f| !(1000..=1_000_000).contains(&f)) {
        bail!("display refresh rate {f} mHz is outside its limits (1000..=1000000 mHz)");
    }
    Ok(DisplayRequest {
        resolution,
        refresh,
        prefer_highest: refresh_option == "prefer_highest",
        // RTX HDR converts an SDR source for the HDR stream.
        hdr: if hdr && crate::nvidia_enhance::rtx_hdr_runs(config) {
            Some(false)
        } else {
            match config.get(HDR_OPTION, "auto") {
                "disabled" => None,
                "force_on" => Some(true),
                "force_off" => Some(false),
                other => {
                    fallback(HDR_OPTION, other, "auto");
                    Some(hdr)
                }
            }
        },
    })
}
/// The size a stream is encoded at when the client asks for `requested` and
/// the host caps it at `cap`: the largest even size of the same shape that
/// fits inside the cap, never larger than the request. A cap the request fits
/// in changes nothing, and neither does none. The display keeps `requested`;
/// the converter letterboxes its picture inside the coded size when the shapes
/// differ.
pub fn capped_encode_size(requested: (u32, u32), cap: Option<(u32, u32)>) -> (u32, u32) {
    let even = |value: u32| (value & !1).max(2);
    let Some(cap) = cap.map(|(width, height)| (even(width), even(height))) else {
        return requested;
    };
    if requested.0 <= cap.0 && requested.1 <= cap.1 {
        return requested;
    }
    let scale = (f64::from(cap.0) / f64::from(requested.0.max(1)))
        .min(f64::from(cap.1) / f64::from(requested.1.max(1)));
    let fit = |value: u32, limit: u32| {
        let scaled = even((f64::from(value) * scale).round() as u32);
        // A picture keeps the encoder's own floor (64 px) where the cap allows
        // it; an extreme shape is widened rather than failing the stream.
        let floor = 64.min(limit);
        scaled.clamp(floor, limit.max(floor))
    };
    (fit(requested.0, cap.0), fit(requested.1, cap.1))
}
/// A `DISPLAYCONFIG_ROTATION` (1 upright to 4, 270° clockwise) in degrees.
pub fn rotation_degrees(rotation: u32) -> Option<u16> {
    match rotation {
        1 => Some(0),
        2 => Some(90),
        3 => Some(180),
        4 => Some(270),
        _ => None,
    }
}
/// Degrees clockwise as a `DISPLAYCONFIG_ROTATION`.
pub fn rotation_code(degrees: u16) -> Option<u32> {
    match degrees {
        0 => Some(1),
        90 => Some(2),
        180 => Some(3),
        270 => Some(4),
        _ => None,
    }
}
fn portrait((width, height): (u32, u32)) -> bool {
    height > width
}
/// `size` in portrait or landscape: swapped when it has the other shape. A
/// square stays as it is.
pub fn oriented(size: (u32, u32), portrait: bool) -> (u32, u32) {
    if size.0 != size.1 && self::portrait(size) != portrait {
        (size.1, size.0)
    } else {
        size
    }
}
/// The rotation in degrees a stream turns a physical display to under
/// `dd_rotation_option`, or None to leave it as it is. `current` is its
/// rotation now and `desktop` its size as the desktop shows it, turned.
/// Following the stream turns the display only when its shape differs from
/// the stream's: a turned display goes back upright, an upright one to 90°.
pub fn physical_rotation(
    option: butterpollo_config::config::DisplayAdjust,
    manual: u16,
    current: u16,
    desktop: (u32, u32),
    stream: (u32, u32),
) -> Option<u16> {
    use butterpollo_config::config::DisplayAdjust;
    let wanted = match option {
        DisplayAdjust::Disabled => return None,
        DisplayAdjust::Manual => manual,
        DisplayAdjust::Auto => {
            if stream.0 == stream.1
                || desktop.0 == desktop.1
                || portrait(stream) == portrait(desktop)
            {
                return None;
            }
            if current.is_multiple_of(180) { 90 } else { 0 }
        }
    };
    (wanted != current).then_some(wanted)
}
/// The mode a physical display gets before it turns: the requested size in the
/// shape the display shows now (`desktop`), so that it has the shape the turn
/// gives it afterwards. Windows lists a display's modes in its current shape.
pub fn mode_before_turn(requested: (u32, u32), desktop: (u32, u32)) -> (u32, u32) {
    oriented(requested, portrait(desktop))
}
/// The size a stream's own virtual display is made at under
/// `dd_rotation_option`: `size` swapped for 90° and 270°, or in the stream's
/// shape when it follows the stream. A virtual display is shaped rather than
/// turned, since a turned one would reach the encoder sideways; 180° leaves it
/// as it is.
pub fn virtual_shape(
    option: butterpollo_config::config::DisplayAdjust,
    manual: u16,
    size: (u32, u32),
    stream: (u32, u32),
) -> (u32, u32) {
    use butterpollo_config::config::DisplayAdjust;
    match option {
        DisplayAdjust::Disabled => size,
        DisplayAdjust::Manual if manual % 180 == 90 => (size.1, size.0),
        DisplayAdjust::Manual => size,
        DisplayAdjust::Auto if stream.0 == stream.1 => size,
        DisplayAdjust::Auto => oriented(size, portrait(stream)),
    }
}

#[cfg(test)]
mod tests;
