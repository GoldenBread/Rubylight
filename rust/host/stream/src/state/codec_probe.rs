//! Standard video codec capability probing, encoder focus checks, throughput recording,
//! and PyroWave optional probe retry handling.

use super::*;

pub(super) const STANDARD_CODECS: u32 = 0x1 | 0x100 | 0x200 | 0x10000 | 0x20000;
/// Bits from the separate PyroWave (Vulkan) probe.
pub(super) const PYROWAVE_CODECS: u32 = 0x0780_0000;
pub(super) const CODEC_RETRY_MIN: Duration = Duration::from_secs(5);
pub(super) const CODEC_RETRY_MAX: Duration = Duration::from_secs(60);

pub(super) struct CodecRetry {
    pub(super) delay: Duration,
    pub(super) next: Option<Instant>,
    pub(super) on_demand: Instant,
    pub(super) warning: Option<Instant>,
}
impl CodecRetry {
    pub(super) fn new(now: Instant) -> Self {
        Self {
            delay: CODEC_RETRY_MIN,
            next: None,
            on_demand: now,
            warning: None,
        }
    }
    pub(super) fn completed(
        &mut self,
        flags: u32,
        error: Option<&str>,
        warnings: &Warnings,
        now: Instant,
    ) -> bool {
        if flags & !0x40000000 == 0 {
            if self.warn(now) {
                // Clearing makes an unchanged warning log its next reminder.
                warnings.clear("video_encoder");
                warnings.set(
                    "video_encoder",
                    format!(
                        "No video encoder available: {}. Retrying.",
                        error.unwrap_or("the encoder produced no packets")
                    ),
                );
            }
        } else {
            warnings.clear("video_encoder");
            self.warning = None;
        }
        if flags & STANDARD_CODECS != 0 {
            self.next = None;
            self.delay = CODEC_RETRY_MIN;
            return false;
        }
        self.next = Some(now + self.delay);
        self.on_demand = now + CODEC_RETRY_MIN;
        self.delay = (self.delay * 2).min(CODEC_RETRY_MAX);
        true
    }
    pub(super) fn due(&self, now: Instant, requested: bool) -> bool {
        self.next
            .is_some_and(|next| now >= next || (requested && now >= self.on_demand))
    }
    pub(super) fn warn(&mut self, now: Instant) -> bool {
        if self.warning.is_some_and(|next| now < next) {
            return false;
        }
        self.warning = Some(now + CODEC_RETRY_MAX);
        true
    }
}

/// A finished probe pass: the codec flags, the first encoder error, and
/// when to probe PyroWave again if its probe is worth repeating.
type ProbePass = (u32, Option<String>, Option<Duration>);

/// How long the host must have had no stream before an optional probe
/// starts, so a client that reconnects at once finds the encoder free.
pub(super) const PROBE_IDLE: Duration = if cfg!(any(test, feature = "test-support")) {
    Duration::from_millis(300)
} else {
    Duration::from_secs(5)
};

pub(super) const PROBE_POLL: Duration =
    Duration::from_millis(if cfg!(any(test, feature = "test-support")) {
        25
    } else {
        250
    });

/// PyroWave probes whose worker crashed, counting the startup probe, before
/// PyroWave stays off until a restart.
pub(super) const CRASH_ATTEMPTS: u32 = 3;

/// Checks PyroWave on the encoder: `codec_probe::pyrowave`, or a fake in
/// tests. The closure says a stream wants the encoder.
pub(crate) type PyroWaveProbe = fn(&Config, &dyn Fn() -> bool) -> Result<u32>;

/// Checks one packed 4:4:4 mode on the encoder: `codec_probe::packed`, or a
/// fake in tests. The closure says a stream wants the encoder.
pub(crate) type PackedProbe =
    fn(&Config, &butterpollo_media_policy::rtsp::Negotiated, &dyn Fn() -> bool) -> Result<()>;

/// The focus check's verdict. Its error marks only this codec unsupported:
/// the codec itself passed and stays offered.
pub(super) fn focus_verdict(
    probe: Result<butterpollo_media_policy::encoder_focus::Support>,
) -> butterpollo_media_policy::encoder_focus::Support {
    probe.unwrap_or_else(
        |error| butterpollo_media_policy::encoder_focus::Support::Unsupported {
            reason: butterpollo_media_policy::encoder_focus::Reason::CodecUnsupported,
            message: format!("the focus check failed: {error:#}"),
        },
    )
}

/// `h264=verified(+10.0) hevc=... av1_10=not_probed`.
pub(super) fn focus_summary(
    support: &[butterpollo_media_policy::encoder_focus::Support; 5],
) -> String {
    butterpollo_media_policy::encoder_focus::SUPPORT_NAMES
        .iter()
        .zip(support)
        .map(|(name, support)| format!("{name}={}", support.summary()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// When the startup PyroWave probe is worth trying again, and after how
/// long: it ran before the user's session was ready, or it timed out, as
/// under a busy CPU, which a game that keeps running would busy again soon
/// after. A worker that crashed is probed [`CRASH_ATTEMPTS`] times in all:
/// on one RTX 5070 half of the host starts found PyroWave and half crashed
/// (issue #94), and each crash had left it off until a restart. Any other
/// failure, such as a missing DLL, stays one.
pub(super) fn pyrowave_retry_delay(error: &anyhow::Error) -> Option<Duration> {
    use butterpollo_win_graphics::codec_probe::{Crashed, SessionNotReady, TimedOut};
    if error.is::<SessionNotReady>() || error.is::<Crashed>() {
        Some(Duration::from_secs(5))
    } else if error.is::<TimedOut>() {
        Some(Duration::from_secs(60))
    } else {
        None
    }
}

/// The PyroWave flags a codec pass advertises, and when to probe PyroWave
/// again. A stream that stopped the probe keeps `previous`, what the last
/// probe found, and the probe runs again once the stream has ended: the
/// pass still completes, and its standard codecs are not probed again.
pub(super) fn pyrowave_pass(
    result: Result<u32>,
    previous: u32,
    standard: bool,
) -> (u32, Option<Duration>) {
    match result {
        Ok(optional) => (optional, None),
        Err(error) if error.is::<butterpollo_win_graphics::codec_probe::Interrupted>() => {
            tracing::debug!(
                "optional PyroWave probe stopped for a stream; it runs again once no stream runs"
            );
            (previous, Some(Duration::from_secs(5)))
        }
        Err(error) => {
            let retry = pyrowave_retry_delay(&error);
            if standard {
                tracing::warn!(error = %format!("{error:#}"), retry_in_s = retry.map(|delay| delay.as_secs()), "optional PyroWave probe failed; standard codecs remain available");
            } else {
                tracing::debug!(error = %format!("{error:#}"), "optional PyroWave probe failed");
            }
            (0, retry)
        }
    }
}

impl Host {
    pub fn request_codec_probe(&self) {
        if self.codecs.load(std::sync::atomic::Ordering::Acquire) & STANDARD_CODECS == 0 {
            self.codec_probe_requested
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }

    /// Wait up to `limit` for the startup probe's standard codecs. A client
    /// that wakes or starts the host and connects at once otherwise caches no
    /// HEVC, AV1 or HDR, or has its first launch refused. Once that probe has
    /// finished, even without finding an encoder, nothing waits again: a
    /// probe retried while no user is signed in or a driver fails holds back
    /// no request.
    pub async fn wait_for_startup_codecs(&self, limit: Duration) {
        let mut ready = self.startup_codecs.subscribe();
        let _ = tokio::time::timeout(limit, ready.wait_for(|ready| *ready)).await;
    }

    /// Wait until `deadline` for the startup pass's packed 4:4:4 probe, which runs after the
    /// standard codecs are published. `/serverinfo` offers `Packed444Modes` from it; a launch
    /// does not wait, because packed 4:4:4 is negotiated in the RTSP handshake from what
    /// serverinfo offered, never asked for in `/launch`.
    pub async fn wait_for_startup_packed(&self, deadline: Instant) {
        let mut ready = self.startup_packed.subscribe();
        let _ = tokio::time::timeout(
            deadline.saturating_duration_since(Instant::now()),
            ready.wait_for(|ready| *ready),
        )
        .await;
    }

    /// `wait_for_startup_codecs` for a launch, which runs on a blocking thread.
    pub fn wait_for_startup_codecs_blocking(&self, limit: Duration) {
        let deadline = Instant::now() + limit;
        while !*self.startup_codecs.borrow() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    pub fn probe_codecs(self: &Arc<Self>) {
        let h = self.clone();
        std::thread::spawn(move || {
            use std::sync::atomic::Ordering;
            let mut retry = CodecRetry::new(Instant::now());
            let mut retrying = false;
            loop {
                if h.stop.load(Ordering::Acquire) {
                    return;
                }
                h.codec_probe_requested.store(false, Ordering::Release);
                h.probing_codecs.store(true, Ordering::Release);
                let mut config = h.config.read().unwrap().clone();
                // Probe the driver's ability independently of the stream's
                // opt-in: the default asks for long-term references in AV1.
                config.values.remove("amd_ltr_frames");
                let result = butterpollo_win_io::input::on_input_desktop(|| {
                    h.probe_codecs_once(&config, retrying)
                });
                retrying = true;
                let Some((flags, error, retry_pyrowave)) =
                    result.unwrap_or_else(|error| Some((0, Some(format!("{error:#}")), None)))
                else {
                    // A stream is starting: leave the encoder to it and probe
                    // again once it has ended, without judging this attempt.
                    h.probing_codecs.store(false, Ordering::Release);
                    h.startup_codecs.send_replace(true);
                    h.startup_packed.send_replace(true);
                    h.metadata.lock().unwrap().take();
                    tracing::debug!("encoder capability probe paused for a stream");
                    if !h.wait_for_codec_retry(&retry) {
                        return;
                    }
                    continue;
                };
                h.codecs.store(flags, Ordering::Release);
                *h.encoder_probe_error.lock().unwrap() = (flags & !0x40000000 == 0).then(|| {
                    error
                        .clone()
                        .unwrap_or_else(|| "the encoder produced no packets".into())
                });
                h.probing_codecs.store(false, Ordering::Release);
                h.startup_codecs.send_replace(true);
                h.startup_packed.send_replace(true);
                if h.stop.load(Ordering::Acquire) {
                    return;
                }
                let now = Instant::now();
                let again = retry.completed(flags, error.as_deref(), &h.warnings, now);
                if flags & !0x40000000 != 0 {
                    tracing::info!(codec_flags = flags, "encoder capability probe completed");
                }
                h.metadata.lock().unwrap().take();
                // A stream stopped the pass's packed probe: probe those modes
                // once it has ended, not every codec again.
                if h.packed_probe_pending.load(Ordering::Acquire) != 0 {
                    h.start_packed_probe(crate::win_bridge::probe_packed);
                }
                if !again {
                    if let Some(delay) = retry_pyrowave {
                        h.retry_pyrowave_probe(&config, delay, crate::win_bridge::probe_pyrowave);
                    }
                    return;
                }
                if !h.wait_for_codec_retry(&retry) {
                    return;
                }
            }
        });
    }

    /// The encoder focus check's verdict for a codec and bit depth.
    pub fn focus_support(
        &self,
        codec: u8,
        ten_bit: bool,
    ) -> butterpollo_media_policy::encoder_focus::Support {
        butterpollo_media_policy::encoder_focus::support_index(codec, ten_bit).map_or(
            butterpollo_media_policy::encoder_focus::Support::NotProbed,
            |index| self.focus_support.lock().unwrap()[index].clone(),
        )
    }

    /// Keeps the throughput the encoder reports for its codec, for the WebRTC frame-rate
    /// offer. A driver that reports none leaves it 0, which limits the codec to 60 fps.
    fn record_throughput(&self, codec: u8, encoder: &butterpollo_win_graphics::encoder::Encoder) {
        static WARNED: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
        let name = match codec {
            0 => "MaxThroughput",
            1 => "HevcMaxThroughput",
            _ => "Av1CapMaxThroughput",
        };
        let rate = encoder
            .capability(name)
            .and_then(|rate| u64::try_from(rate).ok())
            .unwrap_or(0);
        tracing::info!(
            codec,
            rate,
            cap = name,
            "encoder throughput (16x16 macroblocks per second)"
        );
        let bit = 1 << codec.min(2);
        if rate == 0 && WARNED.fetch_or(bit, std::sync::atomic::Ordering::Relaxed) & bit == 0 {
            tracing::warn!(
                codec,
                cap = name,
                "the encoder reports no throughput; WebRTC offers this codec at 60 fps and below"
            );
        }
        self.encoder_mb_rates[codec as usize].store(rate, std::sync::atomic::Ordering::Release);
    }

    /// One pass over every codec. A retry (`yield_to_streams`) stops before
    /// opening another encoder once a launch or stream appears, returning
    /// None: a probe must not compete with a stream for the encoder. Once the
    /// standard codecs are probed, it completes instead and leaves the
    /// optional probes for after the stream.
    fn probe_codecs_once(
        &self,
        config: &Config,
        yield_to_streams: bool,
    ) -> Result<Option<ProbePass>> {
        let _com = butterpollo_win_graphics::capture::ComGuard::new()?;
        let busy = || yield_to_streams && !self.sessions.lock().unwrap().idle();
        // Until this pass publishes its result, keep advertising what the
        // separate PyroWave probe found last time.
        let pyrowave = self.codecs.load(std::sync::atomic::Ordering::Acquire) & PYROWAVE_CODECS;
        let mut first_error = None;
        let image = butterpollo_win_graphics::capture::Image {
            width: 640,
            height: 480,
            stride: 2560,
            bytes: vec![128; 640 * 480 * 4],
            captured: Instant::now(),
            pixel: butterpollo_win_graphics::capture::Pixel::Bgra8,
        };
        let mut flags = 0u32;
        let mut outcomes: Vec<(
            butterpollo_media_policy::codec_support::Codec,
            butterpollo_media_policy::codec_support::Outcome,
        )> = Vec::new();
        // The GPU the first encoder opened on, which wording a missing codec
        // must name: the probe may encode on a card other than the biggest.
        let mut adapter = String::new();
        let mut focus: [butterpollo_media_policy::encoder_focus::Support; 5] =
            std::array::from_fn(|_| butterpollo_media_policy::encoder_focus::Support::NotProbed);
        // Bits follow moonlight-common-c's SCM_* values. Native 4:4:4 is
        // probed after the 4:2:0 modes, as Sunshine's encoder check does
        // (`src/video.cpp`), and only where its 4:2:0 mode works: a real
        // session opens in 4:4:4 and must return a frame, so NVENC's
        // NV_ENC_CAPS_SUPPORT_YUV444_ENCODE on this GPU decides, never its
        // name. AMF encodes 4:2:0 only and is not opened for it.
        use butterpollo_media_policy::codec_support::{Codec, Outcome};
        for probed in Codec::ALL.into_iter().chain(Codec::NATIVE_444) {
            let (codec, hdr, bit) = (probed.id, probed.hdr, probed.bit());
            let mode = config.integer(if codec == 1 { "hevc_mode" } else { "av1_mode" }, 0);
            if matches!(codec, 1 | 2) && (mode == 1 || (hdr && mode == 2)) {
                outcomes.push((probed, Outcome::Disabled));
                continue;
            }
            if probed.yuv444 {
                // A 4:2:0 gap is explained already.
                if flags & probed.chroma_420().bit() == 0 {
                    continue;
                }
                let refusal = match *self.probed_encoder.lock().unwrap() {
                    "nvenc" => butterpollo_media_policy::nvenc::sdk_444_refusal(codec),
                    _ => Some(butterpollo_media_policy::codec_support::AMF_NATIVE_444),
                };
                if let Some(reason) = refusal {
                    outcomes.push((probed, Outcome::Failed(reason.into())));
                    continue;
                }
            }
            if self.stop.load(std::sync::atomic::Ordering::Acquire) {
                break;
            }
            if busy() {
                return Ok(None);
            }
            let negotiated = butterpollo_media_policy::rtsp::Negotiated {
                width: 640,
                height: 480,
                fps: 30,
                bitrate_kbps: 2000,
                codec,
                hdr,
                yuv444: probed.yuv444,
                ..Default::default()
            };
            match butterpollo_win_graphics::encoder::Encoder::new_options(
                &negotiated,
                config.get("output_name", ""),
                config,
            ) {
                Ok(mut encoder) => {
                    if adapter.is_empty() {
                        adapter = encoder.adapter().unwrap_or_default().to_string();
                    }
                    if codec == 0 && !probed.yuv444 {
                        *self.probed_encoder.lock().unwrap() = encoder.backend();
                    }
                    if !hdr && !probed.yuv444 {
                        self.record_throughput(codec, &encoder);
                    }
                    let mut outcome = butterpollo_media_policy::codec_support::Outcome::Failed(
                        "the encoder produced no packets".into(),
                    );
                    for frame in 0..8 {
                        match encoder.encode(&image, frame == 0, negotiated.bitrate_kbps) {
                            Ok(packets) if !packets.is_empty() => {
                                flags |= bit;
                                outcome =
                                    butterpollo_media_policy::codec_support::Outcome::Available;
                                self.codecs
                                    .store(flags | pyrowave, std::sync::atomic::Ordering::Release);
                                if encoder.supports_invalidation() {
                                    flags |= 0x40000000;
                                }
                                break;
                            }
                            Err(error) => {
                                first_error.get_or_insert_with(|| format!("{error:#}"));
                                tracing::debug!(%error, codec, hdr, "encoder capability probe failed");
                                outcome = butterpollo_media_policy::codec_support::Outcome::Failed(
                                    format!("{error:#}"),
                                );
                                break;
                            }
                            _ => std::thread::sleep(Duration::from_millis(5)),
                        }
                    }
                    // The focus check is a 4:2:0 verdict per codec and depth.
                    if outcome == butterpollo_media_policy::codec_support::Outcome::Available
                        && !probed.yuv444
                        && !busy()
                        && let Some(index) =
                            butterpollo_media_policy::encoder_focus::support_index(codec, hdr)
                    {
                        focus[index] = focus_verdict(encoder.probe_focus());
                    }
                    outcomes.push((probed, outcome));
                }
                Err(error) => {
                    first_error.get_or_insert_with(|| format!("{error:#}"));
                    tracing::debug!(%error, codec, hdr, "encoder capability initialization failed");
                    outcomes.push((
                        probed,
                        butterpollo_media_policy::codec_support::Outcome::Failed(format!(
                            "{error:#}"
                        )),
                    ));
                }
            }
        }
        self.explain_codecs(&adapter, &outcomes, flags & STANDARD_CODECS != 0);
        tracing::info!("encoder focus check: {}", focus_summary(&focus));
        *self.focus_support.lock().unwrap() = focus;
        // Publish standard codecs together before the optional Vulkan
        // probe. A driver/overlay failure in that child cannot kill them.
        self.codecs
            .store(flags | pyrowave, std::sync::atomic::Ordering::Release);
        self.publish_standard_codecs(config, flags, crate::win_bridge::probe_packed);
        let mut retry_pyrowave = None;
        if config.boolean("pyrowave", true) && !self.stop.load(std::sync::atomic::Ordering::Acquire)
        {
            let result = if busy() {
                Err(butterpollo_win_graphics::codec_probe::Interrupted.into())
            } else {
                crate::win_bridge::probe_pyrowave(config, &busy)
            };
            let (optional, retry) = pyrowave_pass(result, pyrowave, flags & STANDARD_CODECS != 0);
            flags |= optional;
            retry_pyrowave = retry;
        }
        Ok(Some((flags, first_error, retry_pyrowave)))
    }

    /// Say why each missing codec is missing, once per pass: a log line per
    /// codec, the reason kept for a refused launch, and a console warning when
    /// another standard codec works. With none working, `CodecRetry` already
    /// warns "No video encoder available".
    pub(super) fn explain_codecs(
        &self,
        adapter: &str,
        outcomes: &[(
            butterpollo_media_policy::codec_support::Codec,
            butterpollo_media_policy::codec_support::Outcome,
        )],
        any_codec: bool,
    ) {
        let missing = butterpollo_media_policy::codec_support::explain_all(adapter, outcomes);
        for m in &missing {
            // A setting is the user's choice, not a fault, and most GPUs
            // encode 4:2:0 only.
            if m.codec.yuv444 || m.reason.contains("turned off in the host settings") {
                tracing::info!(adapter, codec = m.codec.label(), reason = %m.reason, "codec not advertised");
            } else {
                tracing::warn!(adapter, codec = m.codec.label(), reason = %m.reason, "codec not advertised");
            }
        }
        match butterpollo_media_policy::codec_support::warning(&missing).filter(|_| any_codec) {
            Some(message) => self.warnings.set("codec_support", message),
            None => self.warnings.clear("codec_support"),
        }
        *self.codec_missing.lock().unwrap() = missing;
    }

    pub(super) fn wait_for_codec_retry(&self, retry: &CodecRetry) -> bool {
        use std::sync::atomic::Ordering;
        let mut session_check = Instant::now();
        loop {
            if self.stop.load(Ordering::Acquire) {
                return false;
            }
            let now = Instant::now();
            if now >= session_check
                && retry.due(now, self.codec_probe_requested.load(Ordering::Acquire))
            {
                // Standard encoders also serve the sign-in screen. Only the
                // optional user-session helper must wait for a signed-in user.
                if self.sessions.lock().unwrap().idle() {
                    return true;
                }
                session_check = now + CODEC_RETRY_MIN;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// Wait until a user is signed in, the startup probe has finished and no
    /// stream has run or started for `PROBE_IDLE`: a client that reconnects
    /// at once finds the encoder free. False when the host is stopping.
    fn wait_until_idle(&self, probe: &str) -> bool {
        use std::sync::atomic::Ordering;
        let mut idle_since = None;
        let mut waiting = false;
        loop {
            if self.stop.load(Ordering::Acquire) {
                return false;
            }
            if !self.probing_codecs.load(Ordering::Acquire) && self.probe_session_ready() {
                if idle_since.get_or_insert_with(Instant::now).elapsed() >= PROBE_IDLE {
                    return true;
                }
            } else {
                idle_since = None;
                if !std::mem::replace(&mut waiting, true) {
                    tracing::info!(probe, "optional codec probe waits until no stream runs");
                }
            }
            std::thread::sleep(PROBE_POLL);
        }
    }

    /// A user is signed in, and no stream runs or is being launched: a
    /// launch asks for a probe, which must wait until its stream has ended.
    fn probe_session_ready(&self) -> bool {
        !(butterpollo_win_io::process::is_system()
            && !butterpollo_win_io::process::user_signed_in())
            && self.sessions.lock().unwrap().idle()
    }

    /// Wait `delay`, then until a user is signed in and no stream has run or
    /// started for `PROBE_IDLE`. False when the host is stopping.
    pub(super) fn wait_for_session(&self, delay: Duration, probe: &str) -> bool {
        use std::sync::atomic::Ordering;
        let until = Instant::now() + delay;
        while Instant::now() < until {
            if self.stop.load(Ordering::Acquire) {
                return false;
            }
            std::thread::sleep(PROBE_POLL);
        }
        // Wait for a sign-in, and leave the GPU to a running stream.
        self.wait_until_idle(probe)
    }

    /// Probe PyroWave again when the first probe ran before the signed-in
    /// user's session was ready, as when the host starts with Windows, or
    /// timed out, as under a busy CPU. PyroWave otherwise stayed unavailable
    /// until the host restarted. A worker that crashed is tried until
    /// [`CRASH_ATTEMPTS`] probes have run (issue #94). The first try waits
    /// `delay`; a timeout is not tried again, and a stream that starts stops the probe until it has
    /// ended, which counts as no attempt. Returns the attempts made.
    pub(super) fn retry_pyrowave_probe(
        &self,
        config: &Config,
        delay: Duration,
        probe: PyroWaveProbe,
    ) -> u32 {
        use butterpollo_win_graphics::codec_probe;
        use std::sync::atomic::Ordering;
        let mut delay = delay;
        let mut attempts = 0;
        while attempts < 6 {
            if !self.wait_for_session(delay, "PyroWave") {
                return attempts;
            }
            let result = {
                let _probe = self.optional_probe.lock().unwrap();
                probe(config, &|| !self.sessions.lock().unwrap().idle())
            };
            if matches!(&result, Err(error) if error.is::<codec_probe::Interrupted>()) {
                tracing::debug!(
                    "optional PyroWave probe stopped for a stream; it runs again once no stream runs"
                );
                continue;
            }
            attempts += 1;
            match result {
                Ok(optional) => {
                    let flags = self.codecs.fetch_or(optional, Ordering::AcqRel) | optional;
                    self.metadata.lock().unwrap().take();
                    tracing::info!(
                        codec_flags = flags,
                        attempts,
                        "optional PyroWave probe completed when probed again"
                    );
                    return attempts;
                }
                Err(error) if error.is::<codec_probe::SessionNotReady>() && attempts < 6 => {
                    tracing::debug!(error = %format!("{error:#}"), attempts, "optional PyroWave probe waits for the session");
                }
                // The startup probe was the first of these attempts.
                Err(error)
                    if error.is::<codec_probe::Crashed>() && attempts < CRASH_ATTEMPTS - 1 =>
                {
                    tracing::warn!(error = %format!("{error:#}"), attempts, "optional PyroWave probe failed again; it is probed once more");
                }
                Err(error) => {
                    tracing::warn!(error = %format!("{error:#}"), attempts, "optional PyroWave probe failed again; standard codecs remain available");
                    return attempts;
                }
            }
            delay = (delay * 2).min(Duration::from_secs(60));
        }
        attempts
    }
}
