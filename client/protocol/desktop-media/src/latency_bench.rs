//! Settings → Advanced → Latency benchmark: this device's display, input, timer and decoder timing
//! without a PC, Nightjar Android's `LatencyBenchmarkActivity`, `LatencyBenchmarkStats` and
//! `DecoderBenchmark` on the desktop (the page is `ui::latency_bench`).
//!
//! What each measurement is here:
//! - Frame callback interval and delay: the launcher's frames while the test runs, each drawn and
//!   presented with V-Sync. A frame's time is the display's last vblank where the system reports it
//!   (DXGI frame statistics or DWM on Windows, GLX OML sync on X11: `stream::phase::timing`), else
//!   the moment the previous present returned; its callback is when the main loop starts the next
//!   frame. Android's are Choreographer's vsync time and its callback.
//! - Input dispatch age: an SDL input event's timestamp to the moment the main loop handles it.
//! - Timer wakeup delay: how late a thread asleep for 100 ms wakes. Android times a main-looper
//!   message; the desktop's main loop blocks in V-Sync while the test runs, so its own timer would
//!   only measure the frame.
//! - Reported panel refresh: SDL's current display mode. Reported audio: SDL's preferred format of
//!   the default output device (its rate and buffer in sample frames), as Android's AudioManager
//!   properties.
//! - Decoders: the clips Android bundles (1080p60, 24 frames), each fed one frame at a time and its
//!   picture taken, 3 rounds; the median of rounds 2 and 3 without the key frame, as Android. The
//!   stream's decoder order is tried first, then software, as a stream falls back. The time is
//!   what the stream's overlay counts as decode time: from handing the frame to FFmpeg to getting
//!   its picture.
//!
//! Nothing here runs during a stream: the page stops the test when it is left, and a stream starts
//! only from another page.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::latency_bench_state::{
    Bench as BenchState, Codec, DecoderResult, Device, Measured, Metric, Stats, Summary,
};
use crate::media;
use crate::mp4;

/// The clips Nightjar Android's decoder benchmark decodes (`app/src/main/assets/bench`).
pub const H264_CLIP: &[u8] = include_bytes!("../../../app/src/main/assets/bench/h264.mp4");
pub const HEVC_CLIP: &[u8] = include_bytes!("../../../app/src/main/assets/bench/hevc.mp4");
pub const AV1_CLIP: &[u8] = include_bytes!("../../../app/src/main/assets/bench/av1.mp4");

/// How long the timing test runs.
pub const TEST: Duration = Duration::from_secs(10);
const TIMER_PERIOD: Duration = Duration::from_millis(100);
/// How often the shown values follow the samples while the test runs, as Android's page.
const SHOW_EVERY: Duration = Duration::from_millis(500);
const ROUNDS: usize = 3;
/// How long an asynchronous decoder (V4L2 M2M) may take for a picture, as Android's timeout.
const PICTURE_TIMEOUT: Duration = Duration::from_millis(200);

fn codec_clip(codec: Codec) -> &'static [u8] {
    match codec {
        Codec::H264 => H264_CLIP,
        Codec::Hevc => HEVC_CLIP,
        Codec::Av1 => AV1_CLIP,
    }
}

/// The decoder for a clip's codec.
fn decoder_codec(codec: mp4::Codec) -> media::Codec {
    match codec {
        mp4::Codec::H264 => media::Codec::H264,
        mp4::Codec::Hevc => media::Codec::Hevc,
        mp4::Codec::Av1 => media::Codec::Av1,
    }
}

/// The decoder benchmark's thread.
struct DecoderRun {
    cancel: Arc<AtomicBool>,
    results: Receiver<DecoderResult>,
    thread: Option<JoinHandle<()>>,
}

/// The benchmark the page shows: the timing test, then the decoders.
#[derive(Default)]
pub struct Bench {
    stats: Arc<Mutex<Stats>>,
    /// When the timing test ends; none when it isn't running.
    until: Option<Instant>,
    timer_stop: Option<Arc<AtomicBool>>,
    /// A timing test has run: the results text carries its values.
    measured: bool,
    last_present_ns: Option<i64>,
    shown: [Option<Summary>; 4],
    shown_at: Option<Instant>,
    pub device: Option<Device>,
    /// The stream's decoders, for the benchmark after the timing test.
    hardware: Option<media::Hardware>,
    /// Keeps `hardware`'s device alive until the decoder benchmark, whose thread holds its own.
    gpu: media::DeviceRef,
    wake: Option<crate::hosts::Waker>,
    decoders: Option<DecoderRun>,
    /// A stopped benchmark's thread, until it has seen its cancel flag ([`Self::finish`]).
    stopped: Vec<JoinHandle<()>>,
    results: Vec<DecoderResult>,
    decoders_measured: bool,
}

impl Bench {
    /// Start: the 10-second timing test, then the decoder benchmark on `hardware`'s decoders.
    pub fn start(&mut self, hardware: media::Hardware, wake: crate::hosts::Waker) {
        self.stop();
        self.stats = Arc::default();
        self.shown = [None; 4];
        self.shown_at = None;
        self.last_present_ns = None;
        self.results.clear();
        self.decoders_measured = false;
        self.measured = true;
        self.hardware = Some(hardware);
        self.gpu = media::DeviceRef::of(hardware);
        self.wake = Some(wake);
        let started = Instant::now();
        self.until = Some(started + TEST);
        let stop = Arc::new(AtomicBool::new(false));
        self.timer_stop = Some(stop.clone());
        let stats = self.stats.clone();
        let spawned = std::thread::Builder::new().name("latency-timer".into()).spawn(move || {
            let mut due = Instant::now() + TIMER_PERIOD;
            while !stop.load(Ordering::SeqCst) {
                std::thread::sleep(due.saturating_duration_since(Instant::now()));
                let now = Instant::now();
                let late = now.saturating_duration_since(due).as_nanos().min(i64::MAX as u128) as i64;
                // Checked under the lock: nothing counts once Stop has read the values.
                let mut stats = stats.lock().unwrap_or_else(|e| e.into_inner());
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                stats.add(Metric::TimerDelay, late);
                drop(stats);
                due = now + TIMER_PERIOD;
            }
        });
        if let Err(error) = spawned {
            eprintln!("nightjar: latency benchmark: no timer thread: {error}");
        }
        eprintln!("nightjar: latency benchmark started");
    }

    /// Stop, or the page was left: what was measured stays shown.
    pub fn stop(&mut self) {
        if self.until.take().is_some() {
            self.show();
            eprintln!("nightjar: latency benchmark stopped");
        }
        if let Some(stop) = self.timer_stop.take() {
            stop.store(true, Ordering::SeqCst);
        }
        self.hardware = None;
        self.gpu = media::DeviceRef::default();
        if let Some(mut run) = self.decoders.take() {
            run.cancel.store(true, Ordering::Relaxed);
            self.results.extend(run.results.try_iter());
            self.stopped.extend(run.thread.take());
        }
        self.stopped.retain(|thread| !thread.is_finished());
    }

    /// The page was left: stopped, and the device read again next time.
    pub fn leave(&mut self) {
        self.stop();
        self.device = None;
    }

    /// Stopped, and every decoder thread gone: before the renderer, whose device they may decode
    /// on, goes away. Waits at most for one frame or one decoder opening.
    pub fn finish(&mut self) {
        self.stop();
        for thread in self.stopped.drain(..) {
            let _ = thread.join();
        }
    }

    /// The timing test is running: every launcher frame is a sample.
    pub fn timing(&self) -> bool {
        self.until.is_some()
    }

    /// The decoder benchmark is running.
    pub fn testing_decoders(&self) -> bool {
        self.decoders.is_some()
    }

    pub fn running(&self) -> bool {
        self.timing() || self.testing_decoders()
    }

    /// Once per launcher frame: ends the timing test after [`TEST`] and starts the decoders, and
    /// takes in the decoders' results.
    pub fn step(&mut self, now: Instant) {
        if self.until.is_some_and(|until| now >= until) {
            self.until = None;
            if let Some(stop) = self.timer_stop.take() {
                stop.store(true, Ordering::SeqCst);
            }
            self.show();
            if let Some(hardware) = self.hardware.take() {
                self.decoders = Some(start_decoders(hardware, std::mem::take(&mut self.gpu), self.wake.clone()));
            }
        } else if self.until.is_some() && self.shown_at.is_none_or(|at| now.duration_since(at) >= SHOW_EVERY) {
            self.show();
            self.shown_at = Some(now);
        }
        let mut finished = false;
        if let Some(run) = &mut self.decoders {
            loop {
                match run.results.try_recv() {
                    Ok(result) => self.results.push(result),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        if let Some(thread) = run.thread.take() {
                            let _ = thread.join();
                        }
                        finished = true;
                        break;
                    }
                }
            }
        }
        if finished {
            self.decoders = None;
            self.decoders_measured = true;
            eprintln!("nightjar: latency benchmark finished");
        }
    }

    fn show(&mut self) {
        let stats = self.stats.lock().unwrap_or_else(|e| e.into_inner());
        self.shown = Metric::ALL.map(|metric| stats.summary(metric));
    }

    /// A launcher frame starts at `now_ns` on the session clock; `vblank_ns` is the display's last
    /// vblank when the system reports it.
    pub fn frame(&mut self, vblank_ns: Option<i64>, now_ns: i64) {
        if !self.timing() {
            return;
        }
        if let Some(frame_ns) = vblank_ns.or(self.last_present_ns) {
            self.stats.lock().unwrap_or_else(|e| e.into_inner()).frame(frame_ns, now_ns);
        }
    }

    /// The frame's present returned at `now_ns` (with V-Sync, about its vblank).
    pub fn presented(&mut self, now_ns: i64) {
        if self.timing() {
            self.last_present_ns = Some(now_ns);
        }
    }

    /// An input event was handled `age_ns` after SDL stamped it.
    pub fn input(&mut self, age_ns: i64) {
        if self.timing() {
            self.stats.lock().unwrap_or_else(|e| e.into_inner()).add(Metric::InputAge, age_ns);
        }
    }

    /// Snapshot for `nightjar-desktop-ui::ui::latency_bench`.
    pub fn state(&self) -> BenchState {
        BenchState {
            timing: self.timing(),
            running: self.running(),
            measured: self.measured,
            decoders_measured: self.decoders_measured,
            shown: self.shown,
            device: self.device.clone(),
            results: self.results.clone(),
        }
    }

    /// What the page shows for `metric`: updated twice a second while the test runs.
    pub fn shown(&self, metric: Metric) -> Option<Summary> {
        self.shown[metric as usize]
    }

    /// `codec`'s result: none before it was measured.
    pub fn result(&self, codec: Codec) -> Option<&DecoderResult> {
        self.results.iter().find(|result| result.codec == codec)
    }

    /// The results as Android's `DeviceResults` text, for the user to paste where they like; the
    /// fields only a phone has are null. No PC name, address or account goes in.
    pub fn json(&self, version: &str) -> String {
        self.state().json(version)
    }
}

/// The decoder benchmark on its own thread; each codec's result comes as it is measured.
/// `device` keeps `hardware`'s device alive while the thread decodes on it.
fn start_decoders(hardware: media::Hardware, device: media::DeviceRef, wake: Option<crate::hosts::Waker>) -> DecoderRun {
    let cancel = Arc::new(AtomicBool::new(false));
    let (send, results) = channel();
    let flag = cancel.clone();
    let thread = std::thread::Builder::new()
        .name("latency-decoders".into())
        .spawn(move || {
            let _device = device;
            for codec in Codec::ALL {
                if flag.load(Ordering::Relaxed) {
                    return;
                }
                let outcome = mp4::read(codec_clip(codec)).and_then(|clip| {
                    let measured = measure(hardware, &clip, &flag);
                    if let Ok(m) = &measured {
                        eprintln!("nightjar: decoder benchmark {} {}x{} at {} fps, {}: {} us/frame", codec.name(), clip.width, clip.height, clip.fps, m.hardware.unwrap_or("software"), m.median_us);
                    }
                    measured
                });
                if flag.load(Ordering::Relaxed) {
                    return;
                }
                if let Err(why) = &outcome {
                    eprintln!("nightjar: decoder benchmark {}: not measured: {why}", codec.name());
                }
                if send.send(DecoderResult { codec, outcome }).is_err() {
                    return;
                }
                if let Some(wake) = &wake {
                    wake();
                }
            }
            // The closed channel tells the launcher the benchmark finished.
            drop(send);
            if let Some(wake) = &wake {
                wake();
            }
        })
        .map_err(|error| eprintln!("nightjar: latency benchmark: no decoder thread: {error}"))
        .ok();
    DecoderRun { cancel, results, thread }
}

/// One codec on the stream's decoders, else in software as a stream falls back.
fn measure(hardware: media::Hardware, clip: &mp4::Clip, cancel: &AtomicBool) -> Result<Measured, String> {
    let (codec, samples) = (decoder_codec(clip.codec), clip.samples.as_slice());
    let mut failures = Vec::new();
    if hardware != media::Hardware::None {
        let pictures = media::Pictures { vaapi_node: media::vaapi_node(hardware), ..Default::default() };
        match media::VideoDecoder::open_first(codec, pictures, hardware, true, |_, _| {}) {
            Ok(mut decoder) => {
                // VAAPI pictures leave as the stream's do, without a copy to memory.
                decoder.export_dmabuf();
                let backend = decoder.hardware.label();
                match time(&mut decoder, samples, cancel) {
                    Ok(_) if decoder.refused() => failures.push(format!("{backend}: the pictures came out in memory")),
                    Ok(median_us) => return Ok(Measured { median_us, decoder: decoder.name.clone(), hardware: Some(backend) }),
                    Err(why) => failures.push(format!("{backend}: {why}")),
                }
            }
            Err(why) => failures.push(why),
        }
        if cancel.load(Ordering::Relaxed) {
            return Err("stopped".into());
        }
    }
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get().min(4)) as i32;
    let software = media::VideoDecoder::open(codec, media::Hardware::None, threads, false)
        .and_then(|mut decoder| time(&mut decoder, samples, cancel).map(|median_us| Measured { median_us, decoder: decoder.name.clone(), hardware: None }));
    software.map_err(|why| {
        failures.push(format!("software: {why}"));
        failures.join("; ")
    })
}

/// Feeds the frames one at a time and waits for each picture, as a low-latency stream does:
/// the median time per frame in microseconds, over every round but the first and without the key
/// frame, which a stream rarely has.
fn time(decoder: &mut media::VideoDecoder, samples: &[Vec<u8>], cancel: &AtomicBool) -> Result<u64, String> {
    let mut times = Vec::with_capacity(samples.len() * (ROUNDS - 1));
    let mut pictures = 0;
    let mut pts = 0;
    for round in 0..ROUNDS {
        // Each round starts again at the key frame, which resets the decoder's references.
        for (index, sample) in samples.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Err("stopped".into());
            }
            let started = Instant::now();
            let mut status = decoder.send(sample, pts);
            if status == media::AGAIN {
                // A picture still waits: taken, then the frame again.
                while matches!(decoder.receive(), media::Received::Frame(_)) {}
                status = decoder.send(sample, pts);
            }
            pts += 1;
            if status < 0 {
                return Err(format!("the decoder refused frame {index} ({status})"));
            }
            if take_picture(decoder)? {
                pictures += 1;
                if round > 0 && index > 0 {
                    times.push(started.elapsed().as_micros() as u64);
                }
            }
        }
    }
    if pictures == 0 || times.is_empty() {
        return Err("the decoder made no picture".into());
    }
    times.sort_unstable();
    Ok(times[times.len() / 2])
}

/// Whether the frame just sent gave a picture; any further pictures are taken too.
fn take_picture(decoder: &mut media::VideoDecoder) -> Result<bool, String> {
    let deadline = Instant::now() + PICTURE_TIMEOUT;
    let mut got = false;
    loop {
        match decoder.receive() {
            media::Received::Frame(_) => got = true,
            media::Received::Error => return Err("the decoder failed".into()),
            media::Received::None if !got && decoder.asynchronous() && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_micros(500));
            }
            media::Received::None => return Ok(got),
        }
    }
}

/// The default output device's preferred rate and buffer in sample frames (Android's
/// `PROPERTY_OUTPUT_SAMPLE_RATE` and `PROPERTY_OUTPUT_FRAMES_PER_BUFFER`).
pub fn audio_format() -> Option<(u32, u32)> {
    use sdl3_sys::everything::*;
    // SAFETY: SDL's audio subsystem is up for the launcher (main.rs); both outputs are ours.
    unsafe {
        let mut spec: SDL_AudioSpec = std::mem::zeroed();
        let mut frames = 0;
        if !SDL_GetAudioDeviceFormat(SDL_AUDIO_DEVICE_DEFAULT_PLAYBACK, &mut spec, &mut frames) {
            return None;
        }
        (spec.freq > 0 && frames > 0).then_some((spec.freq as u32, frames as u32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Android's LatencyBenchmarkStatsTest, case for case.

    #[test]
    fn empty_metrics_are_unavailable_rather_than_zero_latency() {
        assert_eq!(Stats::default().summary(Metric::InputAge), None);
    }

    #[test]
    fn callbacks_measure_intervals_and_delivery_delay_separately() {
        let mut stats = Stats::default();
        stats.frame(1_000_000, 3_000_000);
        stats.frame(11_000_000, 14_000_000);
        stats.frame(21_000_000, 25_000_000);
        assert_eq!(stats.summary(Metric::FrameInterval), Some(Summary { samples: 2, mean: 10.0, p95: 10.0, p99: 10.0 }));
        assert_eq!(stats.summary(Metric::FrameDelay), Some(Summary { samples: 3, mean: 3.0, p95: 4.0, p99: 4.0 }));
    }

    #[test]
    fn duplicates_and_mixed_clock_samples_cannot_corrupt_cadence() {
        let mut stats = Stats::default();
        stats.frame(1_000_000, 2_000_000);
        stats.frame(1_000_000, 3_000_000);
        stats.frame(500_000, 3_000_000);
        stats.frame(10_000_000, 9_000_000);
        stats.frame(11_000_000, 12_000_000);
        stats.add(Metric::InputAge, -1);
        assert_eq!(stats.summary(Metric::FrameInterval), Some(Summary { samples: 1, mean: 10.0, p95: 10.0, p99: 10.0 }));
        assert_eq!(stats.summary(Metric::FrameDelay).unwrap().samples, 2);
        assert_eq!(stats.summary(Metric::InputAge), None);
    }

    #[test]
    fn percentiles_and_window_eviction_use_only_actual_recent_samples() {
        let mut stats = Stats::default();
        for i in 1..=100 {
            stats.add(Metric::InputAge, i * 1_000_000);
        }
        assert_eq!(stats.summary(Metric::InputAge), Some(Summary { samples: 100, mean: 50.5, p95: 95.0, p99: 99.0 }));
        for _ in 0..crate::latency_bench_state::WINDOW_SIZE {
            stats.add(Metric::InputAge, 2_000_000);
        }
        assert_eq!(stats.summary(Metric::InputAge), Some(Summary { samples: crate::latency_bench_state::WINDOW_SIZE, mean: 2.0, p95: 2.0, p99: 2.0 }));
    }

    #[test]
    fn values_read_as_android_shows_them() {
        assert_eq!(Summary { samples: 600, mean: 6.944, p95: 7.1, p99: 12.345 }.text(), "6.94 / 7.10 / 12.35 (n=600)");
    }

    fn bench_running() -> Bench {
        let mut bench = Bench::default();
        bench.start(media::Hardware::None, Arc::new(|| {}));
        bench
    }

    #[test]
    fn frames_are_timed_from_the_vblank_or_else_the_last_present() {
        let mut bench = Bench::default();
        bench.frame(Some(1_000_000), 2_000_000);
        bench.input(1_000_000);
        assert!(bench.stats.lock().unwrap().summary(Metric::FrameDelay).is_none(), "nothing counts before Start");
        let mut bench = bench_running();
        bench.frame(None, 1_000_000);
        assert!(bench.stats.lock().unwrap().summary(Metric::FrameDelay).is_none(), "no frame time yet");
        bench.presented(10_000_000);
        bench.frame(None, 10_500_000);
        bench.presented(20_000_000);
        bench.frame(None, 20_250_000);
        bench.frame(Some(30_000_000), 30_100_000);
        bench.input(2_000_000);
        let stats = bench.stats.lock().unwrap();
        assert_eq!(stats.summary(Metric::FrameInterval).unwrap().samples, 2);
        assert_eq!(stats.summary(Metric::FrameDelay).unwrap().samples, 3);
        assert_eq!(stats.summary(Metric::InputAge).unwrap().mean, 2.0);
        drop(stats);
        bench.finish();
    }

    #[test]
    fn the_test_ends_after_ten_seconds_and_shows_what_it_measured() {
        let mut bench = bench_running();
        assert!(bench.timing() && bench.running());
        bench.input(3_000_000);
        bench.step(Instant::now());
        assert_eq!(bench.shown(Metric::InputAge).unwrap().samples, 1, "shown at once");
        bench.input(5_000_000);
        bench.step(Instant::now());
        assert_eq!(bench.shown(Metric::InputAge).unwrap().samples, 1, "then twice a second");
        bench.step(Instant::now() + TEST);
        assert!(!bench.timing(), "ten seconds");
        assert_eq!(bench.shown(Metric::InputAge).unwrap().samples, 2, "the last values are shown");
        bench.input(7_000_000);
        assert_eq!(bench.stats.lock().unwrap().summary(Metric::InputAge).unwrap().samples, 2, "nothing counts after");
        bench.finish();
        assert!(!bench.running());
    }

    #[test]
    fn stop_keeps_the_values_and_leave_forgets_the_device() {
        let mut bench = bench_running();
        bench.device = Some(Device { panel_hz: Some(60.0), ..Device::default() });
        bench.input(4_000_000);
        bench.stop();
        assert!(!bench.running());
        assert_eq!(bench.shown(Metric::InputAge).unwrap().mean, 4.0);
        bench.leave();
        assert!(bench.device.is_none());
        assert_eq!(bench.shown(Metric::InputAge).unwrap().mean, 4.0, "the values stay until the next Start");
    }

    #[test]
    fn the_timer_thread_counts_its_wakeups_until_stopped() {
        let mut bench = bench_running();
        let deadline = Instant::now() + Duration::from_secs(5);
        while bench.stats.lock().unwrap().summary(Metric::TimerDelay).is_none() {
            assert!(Instant::now() < deadline, "the timer never woke");
            std::thread::sleep(Duration::from_millis(10));
        }
        bench.stop();
        let counted = bench.stats.lock().unwrap().summary(Metric::TimerDelay).unwrap().samples;
        std::thread::sleep(TIMER_PERIOD * 3);
        assert_eq!(bench.stats.lock().unwrap().summary(Metric::TimerDelay).unwrap().samples, counted, "stopped");
    }

    #[test]
    fn the_results_text_has_android_s_shape() {
        let json: serde_json::Value = serde_json::from_str(&BenchState::fixture().json("4.13.0")).unwrap();
        assert_eq!(json["format"], "nightjar-device-results");
        assert_eq!(json["version"], 1);
        assert_eq!(json["app"]["version"], "4.13.0");
        assert_eq!(json["device"]["gpu"], "AMD Radeon RX 7900 XT");
        assert_eq!(json["device"]["model"], serde_json::Value::Null);
        assert_eq!(json["decoderBenchmark"]["measured"], true);
        assert_eq!(json["decoderBenchmark"]["clip"], "1920x1080@60");
        let results = json["decoderBenchmark"]["results"].as_array().unwrap();
        assert_eq!(results.len(), 2, "only what decoded");
        assert_eq!(results[0]["codec"], "H.264");
        assert_eq!(results[0]["mime"], "video/avc");
        assert_eq!(results[0]["medianDecodeMs"], 1.18);
        assert_eq!(results[1]["backend"], "D3D11VA");
        let test = &json["latencyTest"];
        assert_eq!(test["panelHz"], 143.998);
        assert_eq!(test["audioSampleRate"], 48000);
        assert_eq!(test["audioFramesPerBuffer"], 480);
        assert_eq!(test["frameIntervalMs"]["samples"], 1439);
        assert_eq!(test["timerWakeupDelayMs"]["p99"], 1.21);

        let fresh: serde_json::Value = serde_json::from_str(&Bench::default().json("4.13.0")).unwrap();
        assert_eq!(fresh["latencyTest"], serde_json::Value::Null, "no test, no values");
        assert_eq!(fresh["decoderBenchmark"]["measured"], false);
        assert!(fresh["decoderBenchmark"].get("clip").is_none());
    }

    #[test]
    fn the_bundled_clips_are_android_s() {
        for codec in Codec::ALL {
            let clip = mp4::read(codec_clip(codec)).unwrap();
            assert_eq!((clip.width, clip.height, clip.fps), (1920, 1080, 60), "{}", codec.name());
            assert_eq!(clip.codec as u8, codec as u8, "{}", codec.name());
        }
    }

    /// Times the bundled clips in software, as a decoder-less device would: every codec decodes,
    /// in a sane time per frame.
    #[test]
    fn software_decodes_every_bundled_clip() {
        let never = AtomicBool::new(false);
        for codec in Codec::ALL {
            let clip = mp4::read(codec_clip(codec)).unwrap();
            let measured = measure(media::Hardware::None, &clip, &never).unwrap_or_else(|why| panic!("{}: {why}", codec.name()));
            assert_eq!(measured.hardware, None);
            assert!(measured.median_us > 0 && measured.median_us < 2_000_000, "{}: {} us", codec.name(), measured.median_us);
        }
        let stopped = AtomicBool::new(true);
        let clip = mp4::read(H264_CLIP).unwrap();
        assert!(time(&mut media::VideoDecoder::open(media::Codec::H264, media::Hardware::None, 1, false).unwrap(), &clip.samples, &stopped).is_err(), "Stop ends it");
    }
}
