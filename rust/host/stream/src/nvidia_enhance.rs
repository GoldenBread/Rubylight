//! The NVIDIA image enhancements before encoding: RTX HDR (an SDR capture for
//! an HDR stream) and RTX Video enhancement (NVIDIA's video super resolution
//! at the capture's own size). They need the NVIDIA component
//! ([`crate::nvidia_component`]); without it neither is offered and every
//! picture passes through unchanged. The per-app enhancement design follows
//! Foundation Sunshine's (AlkaidLab/foundation-sunshine, GPL-3.0) DLSS NR
//! settings.
use crate::state::{Session, Shared};
use butterpollo_config::config::Config;
use butterpollo_media_policy::nvidia_enhance::Enhancement;
use butterpollo_media_policy::nvidia_enhance::{self, RTX_HDR, RTX_VIDEO, Source};
use butterpollo_win_graphics::capture::{GpuImage, Pixel};
use std::sync::{Arc, atomic::Ordering};

/// The session warning naming an enhancement that was asked for and does
/// not run.
const WARNING: &str = "nvidia_enhancement";

/// Asks the GPU and driver which enhancements they offer, once at startup.
/// NVIDIA's NGX runs in a process of its own (`--nvidia-probe`): a crash or
/// hang inside it costs the enhancements, never the host.
pub(crate) fn probe(h: &Shared) {
    let started = std::time::Instant::now();
    let (offered, errors) = probe_child(&h.directory);
    h.nvidia_enhancements.store(offered, Ordering::Release);
    for (feature, error) in &errors {
        tracing::warn!(feature, error = %error, "NVIDIA image enhancement unavailable");
    }
    tracing::info!(
        rtx_hdr = offered & RTX_HDR != 0,
        rtx_video = offered & RTX_VIDEO != 0,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "NVIDIA image enhancements checked"
    );
    *h.nvidia_enhancement_errors.lock().unwrap() = errors;
}

/// The `--nvidia-probe` process: NGX's check, printed as one JSON line. A
/// crash in it leaves a report in `profile`'s crashes folder.
pub fn probe_main(profile: &std::path::Path) -> i32 {
    // Without crash reports the check still runs; its exit code tells a crash.
    let _ = butterpollo_win_io::crash::initialize(profile);
    let (offered, errors) = match crate::nvidia_component::component_for(profile) {
        Ok(component) => {
            butterpollo_win_graphics::nvidia_enhance::set_component(Some(component));
            butterpollo_win_graphics::nvidia_enhance::probe()
        }
        Err(error) => (
            0,
            vec![
                ("rtx_hdr", format!("{error:#}")),
                ("rtx_video", format!("{error:#}")),
            ],
        ),
    };
    let errors: serde_json::Map<String, serde_json::Value> = errors
        .into_iter()
        .map(|(feature, error)| (feature.to_owned(), error.into()))
        .collect();
    println!(
        "{}",
        serde_json::json!({"offered": offered, "errors": errors})
    );
    0
}

/// Runs NVIDIA's check in its own process, as this host's user.
fn probe_child(profile: &std::path::Path) -> nvidia_enhance::Offer {
    use nvidia_enhance::{PROBE_TIMEOUT_SECS, ProbeEnd, probe_failed, read_probe};
    use std::{io::BufRead, os::windows::process::CommandExt, process::Stdio, time::Instant};
    let spawned = std::env::current_exe().and_then(|program| {
        std::process::Command::new(program)
            .arg("--nvidia-probe")
            .arg(profile)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .spawn()
    });
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => return probe_failed(format!("NVIDIA's check could not start: {error}")),
    };
    // The answer is one line. Read on its own thread: the crash reporter the
    // check starts holds the pipe open after the check exits.
    let (sender, lines) = std::sync::mpsc::channel();
    if let Some(stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            if let Some(Ok(line)) = std::io::BufReader::new(stdout).lines().next() {
                let _ = sender.send(line);
            }
        });
    }
    let deadline = Instant::now() + std::time::Duration::from_secs(PROBE_TIMEOUT_SECS);
    let ended = loop {
        match child.try_wait() {
            Ok(Some(status)) => break ProbeEnd::Code(status.code().unwrap_or(-1)),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50))
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break ProbeEnd::TimedOut;
            }
            Err(error) => {
                return probe_failed(format!("NVIDIA's check could not be waited for: {error}"));
            }
        }
    };
    let line = lines.recv_timeout(std::time::Duration::from_secs(2)).ok();
    read_probe(line.as_deref(), ended)
}

/// The console's view: the NVIDIA component's state, what the GPU and driver
/// offer with it and why the rest is missing.
pub fn status(h: &Shared) -> serde_json::Value {
    let offered = h.nvidia_enhancements.load(Ordering::Acquire);
    let errors: serde_json::Map<String, serde_json::Value> = h
        .nvidia_enhancement_errors
        .lock()
        .unwrap()
        .iter()
        .map(|(feature, error)| ((*feature).to_owned(), error.as_str().into()))
        .collect();
    serde_json::json!({
        "component": crate::nvidia_component::summary(h),
        "rtx_hdr": offered & RTX_HDR != 0,
        "rtx_video": offered & RTX_VIDEO != 0,
        "errors": errors,
    })
}

/// What a capture delivers, for the enhancement choice.
pub(crate) fn source(pixel: Pixel) -> Source {
    match pixel {
        Pixel::Bgra8 => Source::Sdr,
        Pixel::RgbaF16 => Source::ScRgb,
        Pixel::Rgba10Pq => Source::Pq10,
    }
}

/// The peak nits RTX HDR will generate for a stream whose first picture is
/// `pixel`, before its enhancer has run: an SDR picture for an HDR stream,
/// with the component and the user's opt-in present.
pub(crate) fn generated_peak(config: &Config, stream_hdr: bool, pixel: Pixel) -> Option<u16> {
    (stream_hdr && pixel == Pixel::Bgra8 && nvidia_enhance::rtx_hdr_runs(config))
        .then(|| nvidia_enhance::peak_nits(config) as u16)
}

/// What a capture on the GPU with PCI vendor `vendor` and name `adapter` can
/// run: the enhancements the NVIDIA card offers (`offered`) on an NVIDIA
/// capture, none on another GPU, which is then described for the warning.
/// The enhancements run on the capture's own device.
fn for_capture<'a>(
    config: &Config,
    offered: u8,
    vendor: u32,
    adapter: &'a str,
) -> (u8, Option<nvidia_enhance::Elsewhere<'a>>) {
    if butterpollo_media_policy::encoder_gpu::is_nvidia(vendor) {
        return (offered, None);
    }
    let gpu = if adapter.is_empty() {
        "a graphics card that is not NVIDIA's"
    } else {
        adapter
    };
    // Desktop Duplication and AMD Direct Capture (and their old spellings)
    // capture on the display's own GPU whatever Graphics card names; so does
    // Automatic when WGC fails and it falls back to Desktop Duplication,
    // which shows as a capture on another GPU than the one Graphics card names.
    let chosen = config.get("adapter_name", "");
    // Compared in place: this runs for every picture.
    let capture = config.get("capture", "auto");
    let display_bound = ["ddx", "dxgi", "amd"]
        .iter()
        .any(|kind| capture.eq_ignore_ascii_case(kind))
        || (!chosen.is_empty() && chosen != adapter);
    (0, Some(nvidia_enhance::Elsewhere { gpu, display_bound }))
}

/// The enhancement choice for the current runtime configuration, parsed once
/// for the revision, capture device and picture format it was read at: every
/// picture re-uses it, and the first picture after a change parses again.
/// The warning is part of it, so its text is built once per change and not
/// rebuilt for the comparison on every picture.
#[derive(Clone, Debug)]
struct Parsed {
    revision: u64,
    device: usize,
    pixel: Pixel,
    stream_hdr: bool,
    wanted: Enhancement,
    warning: Option<String>,
    /// RTX HDR's NGX values, compared against the filter's on every picture.
    parameters: [u32; 4],
    /// RTX Video's quality level, cached once per revision so `convert` and
    /// repeat-cache invalidation avoid reparsing config strings every frame.
    quality: Option<u32>,
    /// The encoder's SDR white and linear scale while RTX HDR converts.
    luminance: (f32, f32),
}

impl Parsed {
    /// Whether this choice answers for the picture `revision`, `device`,
    /// `pixel` and active HDR state describe.
    fn matches(&self, revision: u64, device: usize, pixel: Pixel, stream_hdr: bool) -> bool {
        self.revision == revision
            && self.device == device
            && self.pixel == pixel
            && self.stream_hdr == stream_hdr
    }
}

/// What runs for a picture, with the warning when what the stream asked for
/// cannot, from the configuration the run loop holds for it.
fn parse(
    revision: u64,
    device: usize,
    vendor: u32,
    adapter: &str,
    pixel: Pixel,
    stream_hdr: bool,
    config: &Config,
) -> Parsed {
    let source = source(pixel);
    let offered = nvidia_enhance::offered(config);
    let (runs, elsewhere) = for_capture(config, offered, vendor, adapter);
    Parsed {
        revision,
        device,
        pixel,
        stream_hdr,
        wanted: nvidia_enhance::select(config, runs, stream_hdr, source),
        warning: nvidia_enhance::unavailable(config, offered, stream_hdr, source, elsewhere),
        parameters: nvidia_enhance::rtx_parameters(config),
        quality: nvidia_enhance::rtx_video_quality(config),
        luminance: nvidia_enhance::rtx_luminance(config),
    }
}

/// A stream's enhancement: chooses it for each picture, keeps its NGX
/// filter, and reports when the encoder must be rebuilt because the picture
/// it receives changed kind (format, or an unshared filter output instead of
/// the capture's texture).
#[derive(Default)]
pub(crate) struct Enhancer {
    inner: active::Inner,
}

impl Enhancer {
    /// The picture to encode for `image`, and whether the encoder must be
    /// rebuilt for it. The choice and its warning are parsed only when the
    /// runtime configuration's `revision`, the capture device or the
    /// picture's format changed; the run loop raises the revision whenever
    /// it replaces the configuration. A failing enhancement is turned off
    /// for this capture with a session warning, and the original picture is
    /// encoded. A capture on a GPU that is not NVIDIA's (the display's AMD
    /// iGPU beside an NVIDIA card, issue #103) runs neither, and the warning
    /// names that GPU.
    pub(crate) fn apply(
        &mut self,
        s: &Session,
        config: &Config,
        revision: u64,
        image: Arc<GpuImage>,
    ) -> (Arc<GpuImage>, bool) {
        let wanted = self.resolve_choice(s, config, revision, &image);
        self.inner.apply(s, config, wanted, image)
    }

    /// Whether the chosen enhancement differs from the one the encoder was
    /// built for, checked without running GPU inference on deferred frames.
    pub(crate) fn will_change(
        &mut self,
        s: &Session,
        config: &Config,
        revision: u64,
        image: &GpuImage,
    ) -> bool {
        self.resolve_choice(s, config, revision, image) != self.inner.selected
    }

    fn resolve_choice(
        &mut self,
        s: &Session,
        config: &Config,
        revision: u64,
        image: &GpuImage,
    ) -> Enhancement {
        let device = image.gpu.key();
        let stream_hdr = s.hdr_active();
        if self
            .inner
            .parsed
            .as_ref()
            .is_some_and(|parsed| parsed.revision != revision)
            && !std::mem::take(&mut self.inner.tuned_revision_pending)
        {
            // External runtime_revision bump (e.g., user config change replacing
            // runtime_config with base_config): re-apply driver profile on next tune.
            self.inner.last_tune = None;
            self.inner.profile_due = None;
        }
        if !self
            .inner
            .parsed
            .as_ref()
            .is_some_and(|parsed| parsed.matches(revision, device, image.pixel, stream_hdr))
        {
            let parsed = parse(
                revision,
                device,
                image.gpu.vendor,
                &image.gpu.display.adapter,
                image.pixel,
                stream_hdr,
                config,
            );
            // Told once per change: with the same text the warning is already
            // current, and with none there is nothing to clear.
            match &parsed.warning {
                Some(reason) => s.launch.warnings.set(WARNING, reason),
                None => s.launch.warnings.clear(WARNING),
            }
            self.inner.parsed = Some(parsed);
        }
        let wanted = self
            .inner
            .parsed
            .as_ref()
            .map_or(Enhancement::None, |parsed| parsed.wanted);
        if self.inner.device == device && self.inner.failed == Some(wanted) {
            Enhancement::None
        } else {
            wanted
        }
    }

    /// The encoder's SDR white and linear scale while RTX HDR converts the
    /// picture, as parsed with the choice; `None` leaves the encoder's own
    /// values.
    pub(crate) fn luminance(&self) -> Option<(f32, f32)> {
        (self.inner.selected == Enhancement::RtxHdr)
            .then(|| self.inner.parsed.as_ref().map(|parsed| parsed.luminance))
            .flatten()
    }

    /// The peak of the HDR signal RTX HDR generates, when it runs: the stream
    /// describes that peak, not the SDR panel's.
    pub(crate) fn hdr_peak(&self, config: &Config) -> Option<u16> {
        (self.inner.selected == Enhancement::RtxHdr)
            .then(|| nvidia_enhance::peak_nits(config) as u16)
    }

    /// RTX HDR's values from the NVIDIA driver profile of the game in front,
    /// where the app or device did not choose them (polled every 250 ms
    /// while RTX HDR runs). Returns the stream's new runtime configuration.
    pub(crate) fn tune(&mut self, h: &Shared, image: &GpuImage, base: &Config) -> Option<Config> {
        self.inner.tune(h, image, base)
    }

    /// Drops the filters: the capture is recreated, possibly on a new device.
    pub(crate) fn reset(&mut self) {
        self.inner = active::Inner::default();
    }
}

mod active {
    use super::*;
    use anyhow::Context as _;
    use butterpollo_win_graphics::{rtx_profiles::Profiles, rtx_video, truehdr};
    use std::{
        sync::Weak,
        time::{Duration, Instant},
    };

    #[derive(Default)]
    pub(super) struct Inner {
        pub(super) selected: Enhancement,
        rtx_hdr: Option<truehdr::Filter>,
        rtx_video: Option<rtx_video::Filter>,
        /// An enhancement that failed on this capture: not retried each frame.
        pub(super) failed: Option<Enhancement>,
        /// The capture device the filters were made on.
        pub(super) device: usize,
        /// The choice and warning parsed for the runtime configuration's
        /// revision, this device and the picture's format: re-parsed only
        /// when one of them changed.
        pub(super) parsed: Option<super::Parsed>,
        /// The last source picture, the content identity it carried, and its
        /// enhanced copy: a repeated picture is encoded again without a second
        /// conversion, and the stream's freshness check sees the same picture.
        /// A constant-rate repeat names its content (`GpuImage::origin`), so
        /// the copy made for the fresh picture serves its repeats.
        last: Option<(Weak<GpuImage>, u64, Arc<GpuImage>)>,
        profiles: Option<Profiles>,
        foreground: Option<butterpollo_win_graphics::foreground::Tracker>,
        pub(super) profile_due: Option<Instant>,
        /// Last foreground app + NVIDIA driver profile applied by [`Self::tune`]:
        /// unchanged polls return `None` with no `Config` clone or revision bump.
        pub(super) last_tune: Option<(Option<String>, std::collections::BTreeMap<String, i64>)>,
        /// Set when [`Self::tune`] returned `Some(Config)` so the resulting
        /// `runtime_revision` bump is recognized as self-inflicted.
        pub(super) tuned_revision_pending: bool,
    }

    impl Inner {
        pub(super) fn apply(
            &mut self,
            s: &Session,
            config: &Config,
            wanted: Enhancement,
            image: Arc<GpuImage>,
        ) -> (Arc<GpuImage>, bool) {
            let device = image.gpu.key();
            if device != self.device {
                *self = Self {
                    selected: self.selected,
                    profiles: self.profiles.take(),
                    foreground: self.foreground.take(),
                    parsed: self.parsed.clone(),
                    device,
                    ..Self::default()
                };
            }
            let wanted = if self.failed == Some(wanted) {
                Enhancement::None
            } else {
                wanted
            };
            let changed = wanted != self.selected;
            if changed {
                tracing::info!(client = %s.launch.client.name, from = ?self.selected, to = ?wanted, "image enhancement changed; rebuilding the encoder");
                self.selected = wanted;
                self.last = None;
                if wanted != Enhancement::RtxHdr {
                    self.rtx_hdr = None;
                }
                if wanted != Enhancement::RtxVideo {
                    self.rtx_video = None;
                }
            }
            if wanted == Enhancement::None {
                return (image, changed);
            }
            // RTX HDR's contrast, saturation, middle gray or peak changed while a still picture
            // repeats: the cached copy was made with the old values, so it is made again rather
            // than shown until the next picture is captured. The values were parsed with the
            // choice, whose revision covers every settings change.
            if wanted == Enhancement::RtxHdr
                && self.parsed.as_ref().is_some_and(|parsed| {
                    self.rtx_hdr
                        .as_ref()
                        .is_some_and(|filter| filter.parameters() != parsed.parameters)
                })
            {
                self.last = None;
            }
            if wanted == Enhancement::RtxVideo
                && self.parsed.as_ref().is_some_and(|parsed| {
                    self.rtx_video
                        .as_ref()
                        .is_some_and(|filter| Some(filter.quality()) != parsed.quality)
                })
            {
                self.last = None;
            }
            if let Some((source, origin, enhanced)) = &self.last
                && (std::ptr::eq(source.as_ptr(), Arc::as_ptr(&image))
                    // A repeat carries the origin of the picture it repeats:
                    // the converted copy is already the same content.
                    || (image.repeat && *origin != 0 && *origin == image.origin))
            {
                if !image.repeat {
                    return (enhanced.clone(), changed);
                }
                // A repeat is a picture of its own to the stream: the same
                // converted texture, its own stamps, and no second pass.
                let mut repeat = (**enhanced).clone();
                repeat.captured = image.captured;
                repeat.acquired = image.acquired;
                repeat.published = image.published;
                repeat.wgc_stamp = image.wgc_stamp;
                repeat.focus = image.focus;
                repeat.repeat = true;
                repeat.origin = image.origin;
                return (Arc::new(repeat), changed);
            }
            match self.convert(config, wanted, &image) {
                Ok(enhanced) => {
                    let enhanced = Arc::new(enhanced);
                    self.last = Some((Arc::downgrade(&image), image.origin, enhanced.clone()));
                    (enhanced, changed)
                }
                Err(error) => {
                    let name = match wanted {
                        Enhancement::RtxHdr => "RTX HDR",
                        _ => "RTX Video enhancement",
                    };
                    tracing::warn!(client = %s.launch.client.name, error = %format!("{error:#}"), "{name} failed; streaming the picture without it");
                    s.launch.warnings.set(
                        "nvidia_enhancement_failed",
                        format!("{name} failed ({error:#}) and is off until the capture restarts; the stream continues without it. Update the NVIDIA driver, or turn {name} off for this app."),
                    );
                    self.failed = Some(wanted);
                    self.selected = Enhancement::None;
                    self.rtx_hdr = None;
                    self.rtx_video = None;
                    self.last = None;
                    // The encoder was made for the enhanced picture.
                    (image, true)
                }
            }
        }

        fn convert(
            &mut self,
            config: &Config,
            wanted: Enhancement,
            image: &GpuImage,
        ) -> anyhow::Result<GpuImage> {
            match wanted {
                Enhancement::RtxHdr => {
                    let parameters = self
                        .parsed
                        .as_ref()
                        .map_or_else(|| nvidia_enhance::rtx_parameters(config), |p| p.parameters);
                    let filter = match &mut self.rtx_hdr {
                        Some(filter) => filter,
                        none => none.insert(truehdr::Filter::new_gpu(image, parameters)?),
                    };
                    filter.set_parameters(parameters);
                    filter.apply_gpu(image)
                }
                Enhancement::RtxVideo => {
                    let quality = self
                        .parsed
                        .as_ref()
                        .map_or_else(|| nvidia_enhance::rtx_video_quality(config), |p| p.quality)
                        .context("RTX Video enhancement has no strength")?;
                    let filter = match &mut self.rtx_video {
                        Some(filter) => filter,
                        none => none.insert(rtx_video::Filter::new_gpu(image, quality)?),
                    };
                    filter.set_quality(quality);
                    filter.apply_gpu(image)
                }
                Enhancement::None => anyhow::bail!("no enhancement"),
            }
        }

        pub(super) fn tune(
            &mut self,
            h: &Shared,
            image: &GpuImage,
            base: &Config,
        ) -> Option<Config> {
            if self.selected != Enhancement::RtxHdr {
                self.profile_due = None;
                self.last_tune = None;
                return None;
            }
            let now = Instant::now();
            if self.profile_due.is_some_and(|due| now < due) {
                return None;
            }
            self.profile_due = Some(now + Duration::from_millis(250));
            let (running, owned) = {
                let app = h.current_app.lock().unwrap();
                (
                    app.is_some(),
                    app.as_ref()
                        .and_then(|app| app.child.as_ref())
                        .and_then(|child| child.process_ids().ok())
                        .unwrap_or_default(),
                )
            };
            let visible = running
                .then(|| {
                    self.foreground
                        .get_or_insert_with(butterpollo_win_graphics::foreground::Tracker::default)
                        .poll(&owned, &image.gpu.display)
                })
                .flatten();
            let profiles = self.profiles.get_or_insert_with(Profiles::new);
            let profile = profiles.poll(visible.as_deref());
            resolve_tune(
                &mut self.last_tune,
                &mut self.tuned_revision_pending,
                visible,
                profile,
                base,
            )
        }
    }

    pub(super) fn resolve_tune(
        last_tune: &mut Option<(Option<String>, std::collections::BTreeMap<String, i64>)>,
        tuned_revision_pending: &mut bool,
        visible: Option<String>,
        profile: &std::collections::BTreeMap<String, i64>,
        base: &Config,
    ) -> Option<Config> {
        if let Some((last_app, last_profile)) = last_tune.as_ref()
            && *last_app == visible
            && last_profile == profile
        {
            return None;
        }
        let foreground_matches = visible.is_some();
        *last_tune = Some((visible, profile.clone()));
        *tuned_revision_pending = true;
        Some(nvidia_enhance::resolve(base, foreground_matches, profile))
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod opt_tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn parsed_caches_rtx_video_quality() {
        let low = Config::parse("rtx_video_intensity = 25\n").unwrap();
        let ultra = Config::parse("rtx_video_intensity = 100\n").unwrap();
        let off = Config::parse("rtx_video_intensity = 0\n").unwrap();
        assert_eq!(
            parse(1, 0, 0x10de, "RTX", Pixel::Bgra8, false, &low).quality,
            Some(1)
        );
        assert_eq!(
            parse(2, 0, 0x10de, "RTX", Pixel::Bgra8, false, &ultra).quality,
            Some(4)
        );
        assert_eq!(
            parse(3, 0, 0x10de, "RTX", Pixel::Bgra8, false, &off).quality,
            None
        );
    }

    #[test]
    fn resolve_tune_skips_unchanged_app_and_profile_polls() {
        let base = Config::default();
        let mut last = None;
        let mut pending = false;
        let mut profile = BTreeMap::new();
        profile.insert("rtx_hdr_contrast".to_owned(), 25);

        let first = active::resolve_tune(
            &mut last,
            &mut pending,
            Some("game.exe".into()),
            &profile,
            &base,
        );
        assert!(first.is_some());
        assert!(pending);

        // Identical app and profile returns None (no Config clone, no revision bump).
        pending = false;
        let same = active::resolve_tune(
            &mut last,
            &mut pending,
            Some("game.exe".into()),
            &profile,
            &base,
        );
        assert!(same.is_none());
        assert!(!pending);

        // Profile value change triggers a fresh Config.
        profile.insert("rtx_hdr_contrast".to_owned(), 40);
        let updated = active::resolve_tune(
            &mut last,
            &mut pending,
            Some("game.exe".into()),
            &profile,
            &base,
        );
        assert!(updated.is_some());
        assert!(pending);

        // Foreground app change triggers a fresh Config even with an empty profile.
        pending = false;
        let empty = BTreeMap::new();
        let desktop = active::resolve_tune(&mut last, &mut pending, None, &empty, &base);
        assert!(desktop.is_some());
        assert!(pending);
        pending = false;
        assert!(active::resolve_tune(&mut last, &mut pending, None, &empty, &base).is_none());
    }
}
