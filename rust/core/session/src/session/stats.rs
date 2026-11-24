//! Live session counters, stutter and pipeline-rate snapshots, and console info JSON.
use super::*;

#[derive(Default)]
pub struct Stats {
    pub frames: AtomicU64,
    pub packets: AtomicU64,
    pub bytes: AtomicU64,
    pub idr_requests: AtomicU64,
    pub reference_invalidations: AtomicU64,
    /// Keyframes the host started itself after it failed to send a frame
    /// that FEC can repair; not counted in `idr_requests`.
    pub send_loss_recoveries: AtomicU64,
    /// Video frames the host could not send whole (a socket refused more
    /// than FEC repairs).
    pub frames_lost_on_send: AtomicU64,
    /// Keyframes too large for full FEC within Moonlight's four blocks.
    pub keyframes_past_fec: AtomicU64,
    /// Streamed video frames whose encoder output passed the byte cap their
    /// block plan relied on (`video_slice_delivery`); each turned it off.
    pub slice_delivery_overflows: AtomicU64,
    pub latency_us: AtomicU64,
    pub frames_replaced: AtomicU64,
    /// Last frame offered to the video socket, plus one; zero means none yet.
    pub video_frame: AtomicU64,
    pub performance: std::sync::Mutex<butterpollo_media_policy::performance::Performance>,
    /// The video thread's stutter totals over the windows that ended, published
    /// after each one. The send path never takes this lock; the console and the
    /// teardown read it, with the pictures recorded since in `performance`.
    pub stutter: std::sync::Mutex<butterpollo_media_policy::stutter_report::Totals>,
    /// The stages `frames` doesn't count, for the pipeline frame rates.
    pub pipeline: butterpollo_media_policy::pipeline_rates::Counters,
    /// The console's pipeline frame rates, over `pipeline_rates::CONSOLE_WINDOW`.
    pub pipeline_console: std::sync::Mutex<butterpollo_media_policy::pipeline_rates::Sampler>,
    /// Socket read to injected for ENet input passes, for the `stream timings` line.
    pub input_latency: std::sync::Mutex<butterpollo_input_policy::input_latency::InputLatency>,
}
impl Stats {
    /// The video thread publishes its stutter totals after each window, so a
    /// reader never does the medians while holding the lock the send path
    /// records under.
    pub fn publish_stutter(&self, totals: &butterpollo_media_policy::stutter_report::Totals) {
        *self.stutter.lock().unwrap() = totals.clone();
    }
    /// The stream's stutter report so far: the totals the video thread last
    /// published, plus the pictures it has not counted yet, copied under the
    /// stage lock. The medians and the JSON run after both locks are released,
    /// as [`Performance::window`](butterpollo_media_policy::performance::Performance::window)
    /// sorts after the same lock.
    pub fn stutter_window(&self) -> butterpollo_media_policy::performance::StutterWindow {
        let pictures = self
            .performance
            .lock()
            .unwrap()
            .uncounted_stutter()
            .collect();
        let totals = self.stutter.lock().unwrap().clone();
        butterpollo_media_policy::performance::StutterWindow::new(totals, pictures)
    }
}
impl<P, A> Session<P, A> {
    /// Notes a new picture whose last packet was just handed to the socket: for a client
    /// that asked for host timing (0x5536) and for the host-side record. `rtp_timestamp` is
    /// the picture's RTP timestamp on the wire and `epoch` the media clock's start.
    pub fn record_sent(
        &self,
        wire_frame: u32,
        rtp_timestamp: u32,
        stamps: &butterpollo_media_policy::performance::Stamps,
        epoch: Instant,
    ) {
        self.host_timings.record(wire_frame, stamps, epoch);
        if let Some(sink) = &*self.timing_record.read().unwrap() {
            sink.record(butterpollo_media_policy::timing_record::Row::new(
                wire_frame,
                rtp_timestamp,
                stamps,
                epoch,
            ));
        }
    }
    /// The running counts behind the pipeline frame rates.
    pub fn pipeline_counts(&self) -> butterpollo_media_policy::pipeline_rates::Counts {
        self.stats
            .pipeline
            .counts(self.stats.frames.load(Ordering::Relaxed))
    }

    /// The console's pipeline frame rates; `None` until a window is complete.
    fn console_pipeline_rates(
        &self,
    ) -> Option<butterpollo_media_policy::pipeline_rates::PipelineRates> {
        let counts = self.pipeline_counts();
        self.stats
            .pipeline_console
            .lock()
            .unwrap()
            .sample(
                Instant::now(),
                counts,
                butterpollo_media_policy::pipeline_rates::CONSOLE_WINDOW,
            )
            .as_ref()
            .map(butterpollo_media_policy::pipeline_rates::PipelineRates::from)
    }
    pub fn info(&self) -> serde_json::Value {
        let mut warnings = self.launch.warnings.snapshot();
        warnings.extend(self.capture_warnings.read().unwrap().snapshot());
        // The size and rate the stream runs at: the negotiated ones until the
        // client switches modes (0x5532). The rate is the one encoded, which
        // `limit_framerate` takes from the launch request; the stream
        // request's own rate is `requested_fps`.
        let mode = self.stream_mode();
        let fps = mode.fps_millihz.saturating_add(500) / 1000;
        let (width, height) = (mode.width, mode.height);
        // The size the display keeps: the client's request, which a host
        // `max_stream_resolution` cap leaves alone while it shrinks `width` and
        // `height`.
        let requested = self.requested_mode();
        let requested_fps =
            if mode == butterpollo_media_policy::reconfigure::Mode::applied(&self.config) {
                self.config.fps
            } else {
                fps
            };
        // Taken under the send path's lock and sorted after it is released. The
        // stutter report adds the window the video thread has not advanced yet,
        // from the totals it published and the pictures recorded since, so the
        // console reads this stream to its latest picture.
        let performance = {
            let performance = self.stats.performance.lock().unwrap();
            performance.window(Instant::now())
        };
        let performance = performance.snapshot();
        let stutter = self.stats.stutter_window().report(mode.period()).snapshot();
        serde_json::json!({"warnings":warnings,"encoder":*self.encoder.read().unwrap(),"uuid":self.launch.client.uuid,"device_name":self.launch.client.name,"width":width,"height":height,"display_width":requested.width,"display_height":requested.height,"fps":fps,"requested_fps":requested_fps,"video_format":self.config.codec,"hdr":self.hdr_active(),"vrr":self.config.vrr_low_latency,"encoder_bitrate_kbps":self.bitrate.load(Ordering::Relaxed),"adaptive_bitrate":self.adaptive_bitrate.lock().unwrap().snapshot(),"link_rate":self.link_rate_snapshot(),"congestion":self.congestion_snapshot(),"packet_pacing":self.automatic_pacing.lock().unwrap().snapshot(),"fec_percent":self.fec_percent(),"pyrowave_minimum_kbps":(self.config.codec == 3).then(|| butterpollo_media_policy::link_rate::pyrowave_floor_kbps(&self.config)),"pyrowave_recommended_kbps":(self.config.codec == 3).then(|| butterpollo_media_policy::link_rate::pyrowave_recommended_kbps(&self.config)),"audio_channels":self.config.audio_channels,"state":if self.stopping(){"STOPPING"}else{"RUNNING"},"frames_sent":self.stats.frames.load(Ordering::Relaxed),"frames_replaced":self.stats.frames_replaced.load(Ordering::Relaxed),"packets_sent":self.stats.packets.load(Ordering::Relaxed),"bytes_sent":self.stats.bytes.load(Ordering::Relaxed),"idr_requests":self.stats.idr_requests.load(Ordering::Relaxed),"reference_invalidations":self.stats.reference_invalidations.load(Ordering::Relaxed),"send_loss_recoveries":self.stats.send_loss_recoveries.load(Ordering::Relaxed),"frames_lost_on_send":self.stats.frames_lost_on_send.load(Ordering::Relaxed),"keyframes_past_fec":self.stats.keyframes_past_fec.load(Ordering::Relaxed),"slice_delivery_overflows":self.stats.slice_delivery_overflows.load(Ordering::Relaxed),"encode_latency_ms":self.stats.latency_us.load(Ordering::Relaxed) as f64/1000.,"performance":performance,"stutter":stutter,"pipeline_rates":self.console_pipeline_rates(),"uptime_seconds":self.started.elapsed().as_secs_f64(),"role":self.launch.role,"guest":self.launch.guest()})
    }
}
