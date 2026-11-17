//! The simple settings layer: three plain choices — what the stream favours, how much picture
//! quality it may spend, and whether HDR may be used — mapped onto the concrete settings both apps
//! already have. The screen stays this device's own resolution and refresh rate.
//!
//! Everything here is pure: the caller passes what it measured at run time (display size and
//! refresh rate, HDR output, codec support, and what the PC offers), so the mapping follows this
//! device and this PC and never a vendor or model name. A stream keeps its values as concrete
//! settings, so a user who never opens Settings is not touched by this module.
//!
//! The choices are stored by each app (`simple_smoothness`, `simple_picture`, `simple_hdr` on
//! Android, the same fields in desktop Nightjar's `settings.json`). The rows show "Custom" when
//! the current settings match no combination, and "Back to recommended" writes the recommended
//! one: [`chosen`] is that comparison, over exactly the values [`values`] writes.

use serde::{Deserialize, Serialize};

/// What the stream should favour: the measured recommendation, a smooth cadence, a balance, or the
/// lowest delay.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Smoothness {
    /// The recommended pace: the newest picture at each refresh, with variable refresh rate where
    /// the screen offers it. It is what a fresh install and Back to recommended use.
    Auto,
    /// Every picture in turn, held for up to four refreshes: the steadiest cadence, a little more
    /// delay. Variable refresh rate is off, since it replaces the pacing.
    Smoothest,
    /// The newest picture at each refresh, with variable refresh rate where the display offers it.
    Balanced,
    /// The newest picture at each refresh, without waiting for it: the lowest delay.
    LowestDelay,
}

/// How much picture quality the stream may spend for its data and power.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Picture {
    /// The bitrate for the size and frame rate, and the codec the PC offers.
    Auto,
    /// 1.5× that bitrate: sharper, more network. Nothing is turned down behind the player's back.
    Best,
    /// 0.6× that bitrate, with the bitrate and quality lowered when the link or the device struggles.
    SaveData,
}

/// Whether HDR may be asked for when this screen, decoder and PC can all show it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Hdr {
    /// HDR where every part of the chain supports it, SDR otherwise.
    Auto,
    /// SDR always.
    Off,
}

/// The three simple choices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choices {
    pub smoothness: Smoothness,
    pub picture: Picture,
    pub hdr: Hdr,
}

impl Default for Choices {
    fn default() -> Self {
        Self { smoothness: Smoothness::Auto, picture: Picture::Auto, hdr: Hdr::Auto }
    }
}

impl Choices {
    pub fn from_keys(smoothness: &str, picture: &str, hdr: &str) -> Self {
        Self {
            smoothness: Smoothness::from_key(smoothness),
            picture: Picture::from_key(picture),
            hdr: Hdr::from_key(hdr),
        }
    }

    pub fn keys(self) -> (&'static str, &'static str, &'static str) {
        (self.smoothness.key(), self.picture.key(), self.hdr.key())
    }
}

impl Smoothness {
    pub const ALL: [Self; 4] = [Self::Auto, Self::Smoothest, Self::Balanced, Self::LowestDelay];

    pub fn key(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Smoothest => "smoothest",
            Self::Balanced => "balanced",
            Self::LowestDelay => "lowest-delay",
        }
    }

    /// Anything a newer version stored reads as the recommended choice.
    pub fn from_key(key: &str) -> Self {
        Self::ALL.into_iter().find(|choice| choice.key() == key).unwrap_or(Self::Auto)
    }

    /// English label; the apps translate it as they show it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto (recommended)",
            Self::Smoothest => "Smoothest",
            Self::Balanced => "Balanced",
            Self::LowestDelay => "Lowest delay",
        }
    }
}

impl Picture {
    pub const ALL: [Self; 3] = [Self::Auto, Self::Best, Self::SaveData];

    pub fn key(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Best => "best",
            Self::SaveData => "save-data",
        }
    }

    pub fn from_key(key: &str) -> Self {
        Self::ALL.into_iter().find(|choice| choice.key() == key).unwrap_or(Self::Auto)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto (recommended)",
            Self::Best => "Best",
            Self::SaveData => "Save data",
        }
    }
}

impl Hdr {
    pub const ALL: [Self; 2] = [Self::Auto, Self::Off];

    pub fn key(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Off => "off",
        }
    }

    pub fn from_key(key: &str) -> Self {
        Self::ALL.into_iter().find(|choice| choice.key() == key).unwrap_or(Self::Auto)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto (recommended)",
            Self::Off => "Off",
        }
    }
}

/// This device's screen, as it reports itself now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Display {
    pub width: u32,
    pub height: u32,
    /// The refresh rate the stream starts at. Rounded, as the hosts take it.
    pub refresh_hz: u32,
    /// The screen shows HDR10 (the format the PC encodes).
    pub hdr: bool,
    /// The screen offers variable refresh rate.
    pub vrr: bool,
}

impl Default for Display {
    fn default() -> Self {
        Self { width: 1920, height: 1080, refresh_hz: 60, hdr: false, vrr: false }
    }
}

impl Display {
    /// The stream frame rate the screen asks for: its own refresh rate, in the hosts' 1–1000 Hz
    /// range. 0 or an unreadable rate is 60 Hz, as the apps start when the screen cannot be read.
    pub fn frame_rate(&self) -> u32 {
        if self.refresh_hz == 0 {
            60
        } else {
            self.refresh_hz.clamp(1, 1000)
        }
    }
}

/// What this device's decoder measured, and which codecs it decodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decoder {
    pub h264: bool,
    pub hevc: bool,
    pub av1: bool,
    /// It decodes 10-bit HDR at all.
    pub hdr: bool,
}

impl Default for Decoder {
    fn default() -> Self {
        Self { h264: true, hevc: false, av1: false, hdr: false }
    }
}

/// What the paired PC offers. `None` is not known yet, which never rules anything out: the
/// stream's own negotiation then picks what the PC really takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Host {
    pub hdr: Option<bool>,
    pub hevc: Option<bool>,
    pub av1: Option<bool>,
}

/// What this device and PC measured: the mapping's whole input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub display: Display,
    pub decoder: Decoder,
    pub host: Host,
}

/// The pacing the simple layer asks for (Android's `frame_pacing` and desktop's `FramePacing`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Pacing {
    Latency,
    Balanced,
    Smoothest,
}

/// The codec the simple layer asks for. `Auto` lets the stream negotiate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Codec {
    #[default]
    Auto,
    H264,
    Hevc,
    Av1,
}

/// The bitrate the simple layer asks for: the app's usual one, or a fraction of it in tenths of a
/// percent (1500 is 1.5×).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Bitrate {
    Default,
    Factor(u32),
}

/// Whether the quality ladder may lower the resolution for this stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AdaptiveQuality {
    Off,
    SavePower,
}

/// The concrete settings the choices mean, for a stream from this device and PC.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Values {
    /// The stream uses this screen's own size and rate (the apps' native resolution).
    pub native_resolution: bool,
    /// `None` is the screen's own rate, which the simple layer always asks for: a decode time is
    /// not a sustained rate, so nothing measured caps it. `Some(rate)` is a lower rate the player
    /// set by hand, which reads as Custom.
    pub fps: Option<u32>,
    pub pacing: Pacing,
    /// Variable refresh rate where the screen offers it.
    pub vrr: bool,
    /// The decoding and input switches that cut latency stay on.
    pub latency_switches: bool,
    /// Balanced drops late pictures (the newest one at each refresh); always on for the simple layer.
    pub drop_late_frames: bool,
    /// The PC's frame times, a pacing the simple layer does not choose.
    pub host_frame_times: bool,
    /// Real VRR (`present_at_host_times`/`checkbox_real_vrr`) when VRR is on; `None` when VRR is off,
    /// where the value does not change anything.
    pub real_vrr: Option<bool>,
    /// The lower refresh rate setting, off: the screen keeps its own rate.
    pub reduce_refresh: bool,
    /// The content-rate vote, off: the screen keeps its own rate.
    pub content_rate: bool,
    pub bitrate: Bitrate,
    pub codec: Codec,
    pub hdr: bool,
    /// The bitrate drops while the link struggles, instead of a stutter.
    pub adaptive_bitrate: bool,
    pub adaptive_quality: AdaptiveQuality,
}

/// What a stream's current settings say, for [`matches`]: the apps read their own stored values
/// into this, with a `None` frame rate when it follows the screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Current {
    pub native_resolution: bool,
    pub fps: Option<u32>,
    /// `None` is a pacing the simple layer does not set (the frame rate limit, the PC's frame
    /// times): the settings read as Custom.
    pub pacing: Option<Pacing>,
    pub vrr: bool,
    pub latency_switches: bool,
    pub drop_late_frames: bool,
    pub host_frame_times: bool,
    pub real_vrr: bool,
    pub reduce_refresh: bool,
    pub content_rate: bool,
    /// The bitrate the stream would use, against the app's bitrate for the size and rate.
    pub bitrate_milli: Option<u32>,
    pub codec: Codec,
    pub hdr: bool,
    pub adaptive_bitrate: bool,
    /// `None` is a quality mode the simple layer does not set (Prefer motion, Prefer detail).
    pub adaptive_quality: Option<AdaptiveQuality>,
    /// The choices last stored by the player, when there are any: a choice the settings still match
    /// is shown as itself, so Auto and an equal profile are told apart.
    pub stored_smoothness: Option<Smoothness>,
    pub stored_picture: Option<Picture>,
    pub stored_hdr: Option<Hdr>,
}

/// The profile "Auto (recommended)" starts from: the newest picture at each refresh, with variable
/// refresh rate where the screen offers it, the bitrate for the size and rate, and HDR wherever the
/// whole chain supports it.
pub fn recommended(_capabilities: &Capabilities) -> Choices {
    Choices { smoothness: Smoothness::Auto, picture: Picture::Auto, hdr: Hdr::Auto }
}

/// The concrete settings a set of choices means on this device and PC.
pub fn values(choices: Choices, capabilities: &Capabilities) -> Values {
    let display = &capabilities.display;
    let decoder = &capabilities.decoder;
    let host = &capabilities.host;
    // The codec stays Auto: a stream negotiates the best one both ends decode, and forcing a codec
    // the PC does not offer would fail the start instead of improving it. The decoder's codecs still
    // matter for HDR, which needs a 10-bit one both ends carry.
    let codec = Codec::Auto;
    let bitrate = match choices.picture {
        Picture::Auto => Bitrate::Default,
        Picture::Best => Bitrate::Factor(1500),
        Picture::SaveData => Bitrate::Factor(600),
    };
    // The screen's own rate: a decoder's benchmark time measures how long one frame takes to
    // decode, which says nothing about how many frames a second it sustains, so it never caps the
    // stream. A lower rate is the player's own choice and reads as Custom.
    let fps = None;
    let hdr = choices.hdr == Hdr::Auto
        && display.hdr
        && decoder.hdr
        && (decoder.hevc || decoder.av1)
        && host.hdr != Some(false)
        && (host.hevc != Some(false) || host.av1 != Some(false));
    Values {
        native_resolution: true,
        fps,
        pacing: match choices.smoothness {
            Smoothness::Auto | Smoothness::LowestDelay => Pacing::Latency,
            Smoothness::Balanced => Pacing::Balanced,
            Smoothness::Smoothest => Pacing::Smoothest,
        },
        // Smoothest's pacing replaces variable refresh rate; Auto and Balanced use it where the
        // screen offers it, and Lowest delay does not wait for it.
        vrr: matches!(choices.smoothness, Smoothness::Auto | Smoothness::Balanced) && display.vrr,
        latency_switches: true,
        drop_late_frames: true,
        host_frame_times: false,
        real_vrr: (matches!(choices.smoothness, Smoothness::Auto | Smoothness::Balanced) && display.vrr)
            .then_some(true),
        reduce_refresh: false,
        content_rate: false,
        bitrate,
        codec,
        hdr,
        adaptive_bitrate: choices.picture == Picture::SaveData,
        adaptive_quality: if choices.picture == Picture::SaveData {
            AdaptiveQuality::SavePower
        } else {
            AdaptiveQuality::Off
        },
    }
}

/// Whether a stream's settings are exactly what these choices write on this device and PC.
pub fn matches(choices: Choices, capabilities: &Capabilities, current: &Current) -> bool {
    let (Some(pacing), Some(adaptive_quality)) = (current.pacing, current.adaptive_quality) else {
        return false;
    };
    let wanted = values(choices, capabilities);
    let expected = Values {
        native_resolution: current.native_resolution,
        fps: current.fps,
        pacing,
        vrr: current.vrr,
        latency_switches: current.latency_switches,
        drop_late_frames: current.drop_late_frames,
        host_frame_times: current.host_frame_times,
        // Real VRR only matters with VRR on; off, its stored value changes nothing.
        real_vrr: wanted.vrr.then_some(current.real_vrr),
        reduce_refresh: current.reduce_refresh,
        content_rate: current.content_rate,
        bitrate: match current.bitrate_milli {
            None => Bitrate::Default,
            Some(milli) => Bitrate::Factor(milli),
        },
        codec: current.codec,
        hdr: current.hdr,
        adaptive_bitrate: current.adaptive_bitrate,
        adaptive_quality,
    };
    wanted == expected
}

/// The choices a stream's settings are, or `None` for Custom. Only the choices the player last stored
/// count: a hand change that happens to equal another profile still reads as Custom. A request from
/// before the stored choices existed reads as the recommended combination when it matches exactly.
pub fn chosen(current: &Current, capabilities: &Capabilities) -> Option<Choices> {
    let stored = match (current.stored_smoothness, current.stored_picture, current.stored_hdr) {
        (Some(smoothness), Some(picture), Some(hdr)) => Choices { smoothness, picture, hdr },
        (None, None, None) => recommended(capabilities),
        _ => return None,
    };
    matches(stored, capabilities, current).then_some(stored)
}

/// The JNI wire format: one JSON request in, one JSON answer out. Android's
/// `SimpleProfileModel` sends these; `protocol/android/src/simple_profile.rs` checks the answers
/// against the shared contract file.
pub mod json {
    use super::*;
    use serde_json::{json, Value};

    #[derive(Deserialize)]
    struct Request {
        op: String,
        #[serde(default)]
        smoothness: Option<String>,
        #[serde(default)]
        picture: Option<String>,
        #[serde(default)]
        hdr: Option<String>,
        #[serde(default)]
        display: Option<Display>,
        #[serde(default)]
        decoder: Option<Decoder>,
        #[serde(default)]
        host: Option<Host>,
        #[serde(default)]
        current: Option<Current>,
    }

    fn choices(request: &Request) -> Choices {
        Choices::from_keys(
            request.smoothness.as_deref().unwrap_or("auto"),
            request.picture.as_deref().unwrap_or("auto"),
            request.hdr.as_deref().unwrap_or("auto"),
        )
    }

    fn capabilities(request: &Request) -> Capabilities {
        Capabilities {
            display: request.display.unwrap_or_default(),
            decoder: request.decoder.unwrap_or_default(),
            host: request.host.unwrap_or_default(),
        }
    }

    fn bitrate(value: Bitrate) -> Value {
        match value {
            Bitrate::Default => json!({"mode": "default"}),
            Bitrate::Factor(milli) => json!({"mode": "factor", "milli": milli}),
        }
    }

    fn values_json(values: &Values) -> Value {
        json!({
            "native_resolution": values.native_resolution,
            "fps": values.fps,
            "pacing": values.pacing,
            "vrr": values.vrr,
            "latency_switches": values.latency_switches,
            "drop_late_frames": values.drop_late_frames,
            "host_frame_times": values.host_frame_times,
            "real_vrr": values.real_vrr,
            "reduce_refresh": values.reduce_refresh,
            "content_rate": values.content_rate,
            "bitrate": bitrate(values.bitrate),
            "codec": values.codec,
            "hdr": values.hdr,
            "adaptive_bitrate": values.adaptive_bitrate,
            "adaptive_quality": values.adaptive_quality,
        })
    }

    pub fn call(request: &str) -> String {
        let request: Request = match serde_json::from_str(request) {
            Ok(request) => request,
            Err(error) => return json!({"error": "invalid", "detail": error.to_string()}).to_string(),
        };
        let capabilities = capabilities(&request);
        let answer = match request.op.as_str() {
            "recommended" => json!(recommended(&capabilities)),
            "values" => values_json(&values(choices(&request), &capabilities)),
            "chosen" => match request.current {
                Some(current) => json!({"choices": chosen(&current, &capabilities)}),
                None => return json!({"error": "invalid", "detail": "chosen needs current"}).to_string(),
            },
            other => return json!({"error": "invalid", "detail": format!("unknown op {other}")}).to_string(),
        };
        json!({"ok": answer}).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn hdr_device() -> Capabilities {
        Capabilities {
            display: Display { width: 1920, height: 1080, refresh_hz: 120, hdr: true, vrr: true },
            decoder: Decoder { h264: true, hevc: true, av1: true, hdr: true },
            host: Host { hdr: Some(true), hevc: Some(true), av1: Some(true) },
        }
    }

    fn plain_device() -> Capabilities {
        Capabilities {
            display: Display { width: 1280, height: 720, refresh_hz: 60, hdr: false, vrr: false },
            decoder: Decoder { h264: true, hevc: false, av1: false, hdr: false },
            ..Capabilities::default()
        }
    }

    #[test]
    fn the_recommended_choices_are_the_same_on_every_device() {
        for capabilities in [plain_device(), hdr_device()] {
            assert_eq!(
                recommended(&capabilities),
                Choices { smoothness: Smoothness::Auto, picture: Picture::Auto, hdr: Hdr::Auto }
            );
        }
    }

    #[test]
    fn every_screen_keeps_its_own_size_and_rate() {
        // A decode time is not a sustained rate: nothing in the device's capabilities caps the
        // frame rate, so the layer always leaves the screen's own rate in charge.
        for capabilities in [plain_device(), hdr_device()] {
            let auto = values(Choices::default(), &capabilities);
            assert!(auto.native_resolution);
            assert_eq!(auto.fps, None, "the screen's own rate");
        }
        assert_eq!(hdr_device().display.frame_rate(), 120);
    }

    #[test]
    fn smoother_choices_pace_and_use_vrr_where_the_screen_has_it() {
        let auto = values(Choices::default(), &hdr_device());
        assert_eq!(auto.pacing, Pacing::Latency);
        assert!(auto.vrr, "Auto uses VRR where the screen offers it");
        let fixed = values(Choices::default(), &plain_device());
        assert!(!fixed.vrr, "Auto does not invent VRR on a fixed screen");
        assert_eq!(fixed.pacing, Pacing::Latency);

        let balanced = values(Choices { smoothness: Smoothness::Balanced, ..Choices::default() }, &hdr_device());
        assert_eq!(balanced.pacing, Pacing::Balanced);
        assert!(balanced.vrr);

        let smoothest = values(Choices { smoothness: Smoothness::Smoothest, ..Choices::default() }, &hdr_device());
        assert_eq!(smoothest.pacing, Pacing::Smoothest);
        assert!(!smoothest.vrr, "VRR replaces the pacing, so Smoothest turns it off");

        let latency = values(Choices { smoothness: Smoothness::LowestDelay, ..Choices::default() }, &hdr_device());
        assert_eq!(latency.pacing, Pacing::Latency);
        assert!(!latency.vrr, "Lowest delay does not wait for the screen's refresh");
        for values in [auto, balanced, smoothest, latency] {
            assert!(values.latency_switches, "the low-latency switches stay on");
            assert!(values.drop_late_frames && !values.host_frame_times);
            assert!(!values.reduce_refresh && !values.content_rate);
        }
    }

    #[test]
    fn picture_quality_sets_the_bitrate_and_whether_it_falls() {
        let auto = values(Choices { picture: Picture::Auto, ..Choices::default() }, &hdr_device());
        assert_eq!(auto.bitrate, Bitrate::Default);
        assert!(!auto.adaptive_bitrate, "the player did not ask the bitrate to fall");
        assert_eq!(auto.adaptive_quality, AdaptiveQuality::Off);

        let best = values(Choices { picture: Picture::Best, ..Choices::default() }, &hdr_device());
        assert_eq!(best.bitrate, Bitrate::Factor(1500));
        assert!(!best.adaptive_bitrate);
        assert_eq!(best.adaptive_quality, AdaptiveQuality::Off);
        assert_eq!(best.codec, Codec::Auto, "the stream negotiates the best codec both ends decode");

        let save = values(Choices { picture: Picture::SaveData, ..Choices::default() }, &hdr_device());
        assert_eq!(save.bitrate, Bitrate::Factor(600));
        assert!(save.adaptive_bitrate);
        assert_eq!(save.adaptive_quality, AdaptiveQuality::SavePower);
        assert_eq!(save.codec, Codec::Auto);
    }

    #[test]
    fn picture_quality_never_forces_a_codec_the_pc_may_not_send() {
        let hevc_only = Capabilities {
            display: hdr_device().display,
            decoder: Decoder { av1: false, ..hdr_device().decoder },
            host: Host { av1: Some(false), ..hdr_device().host },
        };
        for capabilities in [hdr_device(), hevc_only, plain_device()] {
            for picture in Picture::ALL {
                assert_eq!(values(Choices { picture, ..Choices::default() }, &capabilities).codec, Codec::Auto);
            }
        }
    }

    #[test]
    fn hdr_auto_needs_the_whole_chain_and_off_always_wins() {
        assert!(values(Choices::default(), &hdr_device()).hdr);
        let cases = [
            ("no HDR screen", Capabilities { display: Display { hdr: false, ..hdr_device().display }, ..hdr_device() }),
            ("no HDR decoder", Capabilities { decoder: Decoder { hdr: false, ..hdr_device().decoder }, ..hdr_device() }),
            ("no HDR PC", Capabilities { host: Host { hdr: Some(false), ..hdr_device().host }, ..hdr_device() }),
            ("no 10-bit codec", Capabilities { decoder: Decoder { hevc: false, av1: false, ..hdr_device().decoder }, ..hdr_device() }),
            ("PC's HDR codec unknown", Capabilities { host: Host { hdr: Some(true), hevc: Some(false), av1: Some(false) }, ..hdr_device() }),
        ];
        for (name, capabilities) in cases {
            assert!(!values(Choices::default(), &capabilities).hdr, "{name}");
        }
        assert!(!values(Choices { hdr: Hdr::Off, ..Choices::default() }, &hdr_device()).hdr);
    }

    #[test]
    fn stored_choices_survive_a_later_version_and_unknown_values_read_as_recommended() {
        let choices = Choices { smoothness: Smoothness::LowestDelay, picture: Picture::SaveData, hdr: Hdr::Off };
        assert_eq!(Choices::from_keys("lowest-delay", "save-data", "off"), choices);
        assert_eq!(Choices::from_keys("teleport", "wat", "maybe"), Choices::default());
        assert_eq!(Choices::default().keys(), ("auto", "auto", "auto"));
    }

    #[test]
    fn the_screen_rate_is_a_sane_stream_rate() {
        assert_eq!(Display { refresh_hz: 119, ..Display::default() }.frame_rate(), 119);
        assert_eq!(Display { refresh_hz: 0, ..Display::default() }.frame_rate(), 60);
        assert_eq!(Display { refresh_hz: 5000, ..Display::default() }.frame_rate(), 1000);
    }

    fn current_of(choices: Choices, capabilities: &Capabilities) -> Current {
        let values = values(choices, capabilities);
        Current {
            native_resolution: values.native_resolution,
            fps: values.fps,
            pacing: Some(values.pacing),
            vrr: values.vrr,
            latency_switches: values.latency_switches,
            drop_late_frames: values.drop_late_frames,
            host_frame_times: values.host_frame_times,
            real_vrr: values.real_vrr.unwrap_or(true),
            reduce_refresh: values.reduce_refresh,
            content_rate: values.content_rate,
            bitrate_milli: match values.bitrate {
                Bitrate::Default => None,
                Bitrate::Factor(milli) => Some(milli),
            },
            codec: values.codec,
            hdr: values.hdr,
            adaptive_bitrate: values.adaptive_bitrate,
            adaptive_quality: Some(values.adaptive_quality),
            stored_smoothness: Some(choices.smoothness),
            stored_picture: Some(choices.picture),
            stored_hdr: Some(choices.hdr),
        }
    }

    #[test]
    fn every_choice_matches_itself_and_reads_back_as_itself() {
        for capabilities in [plain_device(), hdr_device()] {
            for smoothness in Smoothness::ALL {
                for picture in Picture::ALL {
                    for hdr in Hdr::ALL {
                        let choices = Choices { smoothness, picture, hdr };
                        let current = current_of(choices, &capabilities);
                        assert!(matches(choices, &capabilities, &current), "{choices:?}");
                        assert_eq!(chosen(&current, &capabilities), Some(choices), "{choices:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn auto_and_lowest_delay_are_told_apart_by_the_stored_choice() {
        // On a fixed screen Auto and Lowest delay write the same values; the stored choice decides
        // which one the row shows, and a fresh install with no stored choice shows Auto.
        let capabilities = plain_device();
        let lowest = Choices { smoothness: Smoothness::LowestDelay, ..Choices::default() };
        let mut current = current_of(lowest, &capabilities);
        assert_eq!(chosen(&current, &capabilities), Some(lowest));
        (current.stored_smoothness, current.stored_picture, current.stored_hdr) = (None, None, None);
        assert_eq!(chosen(&current, &capabilities), Some(Choices::default()));
    }

    #[test]
    fn one_changed_advanced_setting_reads_as_custom() {
        let choices = Choices::default();
        let capabilities = hdr_device();
        let base = current_of(choices, &capabilities);
        let changes: [fn(&mut Current); 10] = [
            |c| c.native_resolution = false,
            |c| c.fps = Some(60),
            |c| c.pacing = None,
            |c| c.latency_switches = false,
            |c| c.drop_late_frames = false,
            |c| c.host_frame_times = true,
            |c| c.reduce_refresh = true,
            |c| c.content_rate = true,
            |c| c.bitrate_milli = Some(1234),
            |c| c.codec = Codec::H264,
        ];
        for change in changes {
            let mut current = base;
            change(&mut current);
            assert_eq!(chosen(&current, &capabilities), None, "{current:?}");
        }
        let mut real_vrr = base;
        real_vrr.real_vrr = false;
        assert_eq!(chosen(&real_vrr, &capabilities), None, "Real VRR off under VRR");
        let mut quality = base;
        quality.adaptive_quality = Some(AdaptiveQuality::SavePower);
        assert_eq!(chosen(&quality, &capabilities), None);
        let mut adaptive = base;
        adaptive.adaptive_bitrate = true;
        assert_eq!(chosen(&adaptive, &capabilities), None);
        // A hand change that lands on another profile's values is still Custom: the stored choice
        // alone decides. Changing Auto's pacing to Balanced equals the Balanced profile exactly.
        let mut other_profile = base;
        other_profile.pacing = Some(Pacing::Balanced);
        assert_eq!(chosen(&other_profile, &capabilities), None, "another profile's values are Custom");
        // And the reverse: Balanced with Auto's pacing would once have read as Auto.
        let balanced = Choices { smoothness: Smoothness::Balanced, ..Choices::default() };
        let mut from_balanced = current_of(balanced, &capabilities);
        from_balanced.pacing = Some(Pacing::Latency);
        assert_eq!(chosen(&from_balanced, &capabilities), None, "Balanced with Auto's pacing is Custom");
    }

    #[test]
    fn hdr_turned_off_by_the_device_is_also_a_choice() {
        // A player on a plain screen sees "Auto" and the profile writes HDR off, so the two match.
        let capabilities = plain_device();
        let current = current_of(Choices::default(), &capabilities);
        assert!(!current.hdr);
        assert_eq!(chosen(&current, &capabilities), Some(Choices::default()));
        // Best on the same screen still says HDR off.
        let best = Choices { picture: Picture::Best, ..Choices::default() };
        assert!(!values(best, &capabilities).hdr);
    }

    #[test]
    fn json_answers_the_apps_requests() {
        let ask = |request: &str| -> Value { serde_json::from_str(&json::call(request)).unwrap() };
        let caps = json!({
            "display": {"width": 1920, "height": 1080, "refresh_hz": 120, "hdr": true, "vrr": true},
            "decoder": {"h264": true, "hevc": true, "av1": true, "hdr": true},
            "host": {"hdr": true, "hevc": true, "av1": true}
        });
        let request = json!({
            "op": "values", "smoothness": "smoothest", "picture": "best", "hdr": "auto",
            "display": caps["display"], "decoder": caps["decoder"], "host": caps["host"]
        });
        let answer = ask(&request.to_string());
        assert_eq!(answer["ok"]["pacing"], "smoothest");
        assert_eq!(answer["ok"]["vrr"], false);
        assert_eq!(answer["ok"]["bitrate"], json!({"mode": "factor", "milli": 1500}));
        assert_eq!(answer["ok"]["codec"], "auto");
        assert_eq!(answer["ok"]["hdr"], true);
        assert_eq!(answer["ok"]["native_resolution"], true);

        let recommended = ask(&json!({"op": "recommended", "display": caps["display"], "decoder": caps["decoder"], "host": caps["host"]}).to_string());
        assert_eq!(recommended["ok"], json!({"smoothness": "auto", "picture": "auto", "hdr": "auto"}));
        // A values request without a smoothness key reads as the recommended Auto, not the old Balanced.
        let omitted = ask(&json!({"op": "values", "picture": "auto", "hdr": "auto",
            "display": caps["display"], "decoder": caps["decoder"], "host": caps["host"]}).to_string());
        assert_eq!(omitted["ok"]["pacing"], "latency");

        let current = json!({
            "native_resolution": true, "fps": null, "pacing": "balanced", "vrr": true,
            "latency_switches": true, "drop_late_frames": true, "host_frame_times": false,
            "real_vrr": true, "reduce_refresh": false, "content_rate": false,
            "bitrate_milli": null, "codec": "auto", "hdr": true,
            "adaptive_bitrate": false, "adaptive_quality": "off",
            "stored_smoothness": "balanced", "stored_picture": "auto", "stored_hdr": "auto"
        });
        let chosen = ask(&json!({"op": "chosen", "current": current, "display": caps["display"], "decoder": caps["decoder"], "host": caps["host"]}).to_string());
        assert_eq!(chosen["ok"]["choices"]["smoothness"], "balanced");
        assert_eq!(ask(&json!({"op": "chosen", "current": current}).to_string())["ok"]["choices"], Value::Null);
        assert_eq!(ask(r#"{"op":"teleport"}"#)["error"], "invalid");
        assert_eq!(ask(r#"{"op":"chosen"}"#)["error"], "invalid");
        assert_eq!(ask("not json")["error"], "invalid");
    }
}
