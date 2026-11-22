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
pub(crate) fn probe_main(profile: &std::path::Path) -> i32 {
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
pub(crate) fn status(h: &Shared) -> serde_json::Value {
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
    wanted: Enhancement,
    warning: Option<String>,
    /// RTX HDR's NGX values, compared against the filter's on every picture.
    parameters: [u32; 4],
    /// The encoder's SDR white and linear scale while RTX HDR converts.
    luminance: (f32, f32),
}

impl Parsed {
    /// Whether this choice answers for the picture `revision`, `device` and
    /// `pixel` describe.
    fn matches(&self, revision: u64, device: usize, pixel: Pixel) -> bool {
        self.revision == revision && self.device == device && self.pixel == pixel
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
        wanted: nvidia_enhance::select(config, runs, stream_hdr, source),
        warning: nvidia_enhance::unavailable(config, offered, stream_hdr, source, elsewhere),
        parameters: nvidia_enhance::rtx_parameters(config),
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
        if !self
            .inner
            .parsed
            .as_ref()
            .is_some_and(|parsed| parsed.matches(revision, device, image.pixel))
        {
            let parsed = parse(
                revision,
                device,
                image.gpu.vendor,
                &image.gpu.display.adapter,
                image.pixel,
                s.hdr_active(),
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
        profile_due: Option<Instant>,
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
                    let parameters = nvidia_enhance::rtx_parameters(config);
                    let filter = match &mut self.rtx_hdr {
                        Some(filter) => filter,
                        none => none.insert(truehdr::Filter::new_gpu(image, parameters)?),
                    };
                    filter.set_parameters(parameters);
                    filter.apply_gpu(image)
                }
                Enhancement::RtxVideo => {
                    let quality = nvidia_enhance::rtx_video_quality(config)
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
            Some(nvidia_enhance::resolve(
                base,
                visible.is_some(),
                profiles.poll(visible.as_deref()),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_map_to_their_sources() {
        assert_eq!(source(Pixel::Bgra8), Source::Sdr);
        assert_eq!(source(Pixel::RgbaF16), Source::ScRgb);
        assert_eq!(source(Pixel::Rgba10Pq), Source::Pq10);
    }

    #[test]
    fn only_a_capture_on_an_nvidia_gpu_runs_the_enhancements() {
        use nvidia_enhance::{ALL, Elsewhere, Enhancement};
        let config = |text: &str| Config::parse(text).unwrap();
        let video = config(
            "rtx_video = true
",
        );
        // Issue #103: the display's Radeon 780M captures beside an RTX 5060 Ti
        // that offers both: nothing runs there, and the warning names the 780M.
        let (runs, elsewhere) = for_capture(&video, ALL, 0x1002, "AMD Radeon 780M Graphics");
        assert_eq!(runs, 0);
        assert_eq!(
            elsewhere,
            Some(Elsewhere {
                gpu: "AMD Radeon 780M Graphics",
                display_bound: false
            })
        );
        assert_eq!(
            nvidia_enhance::select(&video, runs, true, Source::Sdr),
            Enhancement::None
        );
        // On the NVIDIA card everything it offers runs.
        let (runs, elsewhere) = for_capture(&video, ALL, 0x10de, "NVIDIA GeForce RTX 5060 Ti");
        assert_eq!((runs, elsewhere), (ALL, None));
        assert_eq!(
            nvidia_enhance::select(&video, runs, true, Source::Sdr),
            Enhancement::RtxVideo
        );
        // Desktop Duplication and AMD Direct Capture stay on the display's GPU.
        for method in ["ddx", "DXGI", "amd"] {
            let bound = config(&format!(
                "rtx_video = true
capture = {method}
"
            ));
            let (_, elsewhere) = for_capture(&bound, ALL, 0x1002, "AMD Radeon 780M Graphics");
            assert!(elsewhere.unwrap().display_bound, "{method}");
        }
        // Graphics card names the NVIDIA card, and the capture still runs on
        // the 780M: Automatic fell back to Desktop Duplication.
        let chosen = config("rtx_video = true\nadapter_name = NVIDIA GeForce RTX 5060 Ti\n");
        let (_, elsewhere) = for_capture(&chosen, ALL, 0x1002, "AMD Radeon 780M Graphics");
        assert!(elsewhere.unwrap().display_bound);
        // Graphics card names the 780M itself: choosing the NVIDIA card is the advice.
        let igpu = config("rtx_video = true\nadapter_name = AMD Radeon 780M Graphics\n");
        let (_, elsewhere) = for_capture(&igpu, ALL, 0x1002, "AMD Radeon 780M Graphics");
        assert!(!elsewhere.unwrap().display_bound);
        let (_, elsewhere) = for_capture(&video, ALL, 0x8086, "");
        assert_eq!(
            elsewhere.unwrap().gpu,
            "a graphics card that is not NVIDIA's"
        );
    }

    #[test]
    fn the_enhancer_reports_the_generated_peak_only_while_rtx_hdr_runs() {
        let config = Config::parse("hdr_peak_brightness = 900\n").unwrap();
        let mut enhancer = Enhancer::default();
        assert_eq!(enhancer.hdr_peak(&config), None);
        enhancer.inner.selected = Enhancement::RtxHdr;
        assert_eq!(enhancer.hdr_peak(&config), Some(900));
    }

    #[test]
    fn the_choice_is_parsed_again_only_when_its_inputs_change() {
        // RTX HDR asked for and offered, an SDR picture for an HDR stream.
        let mut config = Config::parse("rtx_hdr = true\n").unwrap();
        config
            .values
            .insert(nvidia_enhance::marker("rtx_hdr"), "true".into());
        config
            .values
            .insert(nvidia_enhance::OFFERED_KEY.into(), RTX_HDR.to_string());
        let parsed = parse(
            1,
            0,
            0x10de,
            "NVIDIA GeForce RTX 5060 Ti",
            Pixel::Bgra8,
            true,
            &config,
        );
        assert_eq!(parsed.wanted, Enhancement::RtxHdr);
        assert_eq!(parsed.warning, None);
        // The same revision, capture device and format re-use it.
        assert!(parsed.matches(1, 0, Pixel::Bgra8));
        // A replaced configuration, a new capture device or another format
        // parses again.
        assert!(!parsed.matches(2, 0, Pixel::Bgra8));
        assert!(!parsed.matches(1, 1, Pixel::Bgra8));
        assert!(!parsed.matches(1, 0, Pixel::RgbaF16));
        // What a new revision parses follows the new values, warning
        // included: here RTX HDR is asked for but nothing offers it.
        let mut unoffered = Config::parse("rtx_hdr = true\n").unwrap();
        unoffered
            .values
            .insert(nvidia_enhance::marker("rtx_hdr"), "true".into());
        let reparsed = parse(
            2,
            0,
            0x10de,
            "NVIDIA GeForce RTX 5060 Ti",
            Pixel::Bgra8,
            true,
            &unoffered,
        );
        assert_eq!(reparsed.wanted, Enhancement::None);
        assert!(reparsed.warning.is_some());
    }

    #[test]
    fn the_parsed_choice_answers_what_the_per_picture_reads_answered() {
        // What `Enhancer::apply` read for every picture before: the same
        // helpers over the same values.
        let mut config = Config::parse("rtx_video = true\nrtx_hdr = true\n").unwrap();
        config
            .values
            .insert(nvidia_enhance::marker("rtx_hdr"), "true".into());
        config.values.insert(
            nvidia_enhance::OFFERED_KEY.into(),
            (RTX_HDR | RTX_VIDEO).to_string(),
        );
        let vendor = 0x10de;
        let adapter = "NVIDIA GeForce RTX 5060 Ti";
        let parsed = parse(7, 3, vendor, adapter, Pixel::Bgra8, true, &config);
        let source = Source::Sdr;
        let offered = nvidia_enhance::offered(&config);
        let (runs, elsewhere) = for_capture(&config, offered, vendor, adapter);
        assert_eq!(
            parsed.wanted,
            nvidia_enhance::select(&config, runs, true, source)
        );
        assert_eq!(
            parsed.warning,
            nvidia_enhance::unavailable(&config, offered, true, source, elsewhere)
        );
        assert_eq!(parsed.luminance, nvidia_enhance::rtx_luminance(&config));
        assert_eq!(parsed.parameters, nvidia_enhance::rtx_parameters(&config));
    }

    #[test]
    fn rtx_hdrs_luminance_is_parsed_with_the_choice_and_follows_the_revision() {
        let mut config = Config::parse("rtx_hdr = true\nrtx_hdr_sdr_brightness = 20\n").unwrap();
        config
            .values
            .insert(nvidia_enhance::marker("rtx_hdr"), "true".into());
        config
            .values
            .insert(nvidia_enhance::OFFERED_KEY.into(), RTX_HDR.to_string());
        let mut enhancer = Enhancer::default();
        enhancer.inner.selected = Enhancement::RtxHdr;
        // Nothing parsed yet, so nothing answers.
        assert_eq!(enhancer.luminance(), None);
        // The choice is parsed for revision 1: its luminance answers while
        // it is current, with the values that revision's configuration had.
        enhancer.inner.parsed = Some(parse(
            1,
            0,
            0x10de,
            "NVIDIA GeForce RTX 5060 Ti",
            Pixel::Bgra8,
            true,
            &config,
        ));
        assert_eq!(enhancer.luminance(), Some((120., 1.)));
        // A later revision changed the SDR white; the next parse carries it.
        config
            .values
            .insert("rtx_hdr_sdr_brightness".into(), "40".into());
        enhancer.inner.parsed = Some(parse(
            2,
            0,
            0x10de,
            "NVIDIA GeForce RTX 5060 Ti",
            Pixel::Bgra8,
            true,
            &config,
        ));
        assert_eq!(enhancer.luminance(), Some((140., 1.)));
    }

    /// RTX HDR converts an SDR picture for an HDR stream. The stream then
    /// describes the peak RTX HDR makes, not the SDR monitor's.
    #[test]
    fn rtx_hdr_describes_the_peak_it_generates() {
        let mut config = Config::parse("rtx_hdr = true\nhdr_peak_brightness = 1200\n").unwrap();
        config
            .values
            .insert(nvidia_enhance::marker("rtx_hdr"), "true".into());
        config.values.insert(
            nvidia_enhance::OFFERED_KEY.into(),
            nvidia_enhance::ALL.to_string(),
        );
        assert_eq!(
            generated_peak(&config, true, Pixel::Bgra8),
            Some(1200),
            "an SDR picture for an HDR stream is generated at the chosen peak"
        );
        assert_eq!(
            generated_peak(&config, false, Pixel::Bgra8),
            None,
            "an SDR stream runs no RTX HDR"
        );
        assert_eq!(
            generated_peak(&config, true, Pixel::Rgba10Pq),
            None,
            "a 10-bit HDR picture is not converted"
        );
        let unasked = Config::parse("rtx_hdr = true\n").unwrap();
        assert_eq!(generated_peak(&unasked, true, Pixel::Bgra8), None);
    }

    #[test]
    fn a_host_without_the_component_reports_nothing_offered() {
        let f = crate::state::test_support::Fixture::new();
        let h = &f.host;
        h.nvidia_enhancement_errors
            .lock()
            .unwrap()
            .push(("rtx_video", "the GPU does not support it".into()));
        let status = status(h);
        assert_eq!(status["component"]["state"], "missing");
        assert_eq!(status["rtx_hdr"], false);
        assert_eq!(status["rtx_video"], false);
        assert_eq!(status["errors"]["rtx_video"], "the GPU does not support it");
        h.nvidia_enhancements.store(RTX_VIDEO, Ordering::Release);
        assert!(status_of(h, "rtx_video"));
    }

    fn status_of(h: &Shared, key: &str) -> bool {
        status(h)[key].as_bool().unwrap()
    }
}
