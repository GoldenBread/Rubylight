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
    /// The published reports of the last [`STUTTER_RECENT`] and the newest one
    /// before it, newest last (at most [`STUTTER_HISTORY`]), so the stream doctor
    /// can name the stage of its last 30 seconds: one small copy per five-second
    /// window or mode change, none per picture.
    pub stutter_history: std::sync::Mutex<
        std::collections::VecDeque<(Instant, butterpollo_media_policy::stutter_report::Report)>,
    >,
    /// The stages `frames` doesn't count, for the pipeline frame rates.
    pub pipeline: butterpollo_media_policy::pipeline_rates::Counters,
    /// The console's pipeline frame rates, over `pipeline_rates::CONSOLE_WINDOW`.
    pub pipeline_console: std::sync::Mutex<butterpollo_media_policy::pipeline_rates::Sampler>,
    /// Socket read to injected for ENet input passes, for the `stream timings` line.
    pub input_latency: std::sync::Mutex<butterpollo_input_policy::input_latency::InputLatency>,
}
/// The most published stutter reports kept for [`Stats::stutter_recent`]. They are
/// kept by age; this bounds a burst of mode changes, which publish between the
/// five-second windows too.
pub const STUTTER_HISTORY: usize = 32;
/// The span of `stutter_recent` in the console's session info, the stream doctor's window.
pub const STUTTER_RECENT: Duration = Duration::from_secs(30);
/// Adds the report published `at` and forgets those the window no longer
/// needs: every report since [`STUTTER_RECENT`] ago stays, with the newest
/// one before it, up to [`STUTTER_HISTORY`].
fn keep_stutter(
    history: &mut std::collections::VecDeque<(
        Instant,
        butterpollo_media_policy::stutter_report::Report,
    )>,
    at: Instant,
    report: butterpollo_media_policy::stutter_report::Report,
) {
    history.push_back((at, report));
    while history.len() > STUTTER_HISTORY
        || history
            .get(1)
            .is_some_and(|(then, _)| at.saturating_duration_since(*then) >= STUTTER_RECENT)
    {
        history.pop_front();
    }
}
impl Stats {
    /// The video thread publishes its stutter totals after each window, so a
    /// reader never does the medians while holding the lock the send path
    /// records under.
    pub fn publish_stutter(&self, totals: &butterpollo_media_policy::stutter_report::Totals) {
        *self.stutter.lock().unwrap() = totals.clone();
        let report = totals.report();
        keep_stutter(
            &mut self.stutter_history.lock().unwrap(),
            Instant::now(),
            report,
        );
    }
    /// The stutter report of about the last `window`: the stream so far less
    /// the newest published report at least `window` old. A stream younger
    /// than that has its whole report; after a burst of mode changes filled the
    /// history, the oldest report kept stands in.
    pub fn stutter_recent(
        &self,
        period: Duration,
        window: Duration,
        now: Instant,
    ) -> butterpollo_media_policy::stutter_report::Report {
        self.stutter_since(&self.stutter_window().report(period), window, now)
    }
    /// `report`, the stream's report so far, less what it held `window` ago.
    pub fn stutter_since(
        &self,
        report: &butterpollo_media_policy::stutter_report::Report,
        window: Duration,
        now: Instant,
    ) -> butterpollo_media_policy::stutter_report::Report {
        let history = self.stutter_history.lock().unwrap();
        match history
            .iter()
            .rev()
            .find(|(at, _)| now.saturating_duration_since(*at) >= window)
        {
            Some((_, earlier)) => report.since(earlier),
            None if history.len() == STUTTER_HISTORY => report.since(&history[0].1),
            None => report.clone(),
        }
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
    pub fn console_pipeline_rates(
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
        let stutter = self.stats.stutter_window().report(mode.period());
        // The last 30 seconds, as Nightjar's stream doctor reads them (`native::doctor`).
        let stutter_recent = self
            .stats
            .stutter_since(&stutter, STUTTER_RECENT, Instant::now())
            .snapshot();
        let stutter = stutter.snapshot();
        serde_json::json!({"warnings":warnings,"encoder":*self.encoder.read().unwrap(),"uuid":self.launch.client.uuid,"device_name":self.launch.client.name,"width":width,"height":height,"display_width":requested.width,"display_height":requested.height,"fps":fps,"requested_fps":requested_fps,"video_format":self.config.codec,"hdr":self.hdr_active(),"vrr":self.config.vrr_low_latency,"encoder_bitrate_kbps":self.bitrate.load(Ordering::Relaxed),"adaptive_bitrate":self.adaptive_bitrate.lock().unwrap().snapshot(),"link_rate":self.link_rate_snapshot(),"congestion":self.congestion_snapshot(),"packet_pacing":self.automatic_pacing.lock().unwrap().snapshot(),"fec_percent":self.fec_percent(),"pyrowave_minimum_kbps":(self.config.codec == 3).then(|| butterpollo_media_policy::link_rate::pyrowave_floor_kbps(&self.config)),"pyrowave_recommended_kbps":(self.config.codec == 3).then(|| butterpollo_media_policy::link_rate::pyrowave_recommended_kbps(&self.config)),"audio_channels":self.config.audio_channels,"state":if self.stopping(){"STOPPING"}else{"RUNNING"},"frames_sent":self.stats.frames.load(Ordering::Relaxed),"frames_replaced":self.stats.frames_replaced.load(Ordering::Relaxed),"packets_sent":self.stats.packets.load(Ordering::Relaxed),"bytes_sent":self.stats.bytes.load(Ordering::Relaxed),"idr_requests":self.stats.idr_requests.load(Ordering::Relaxed),"reference_invalidations":self.stats.reference_invalidations.load(Ordering::Relaxed),"send_loss_recoveries":self.stats.send_loss_recoveries.load(Ordering::Relaxed),"frames_lost_on_send":self.stats.frames_lost_on_send.load(Ordering::Relaxed),"keyframes_past_fec":self.stats.keyframes_past_fec.load(Ordering::Relaxed),"slice_delivery_overflows":self.stats.slice_delivery_overflows.load(Ordering::Relaxed),"encode_latency_ms":self.stats.latency_us.load(Ordering::Relaxed) as f64/1000.,"performance":performance,"stutter":stutter,"stutter_recent":stutter_recent,"pipeline_rates":self.console_pipeline_rates(),"uptime_seconds":self.started.elapsed().as_secs_f64(),"role":self.launch.role,"guest":self.launch.guest()})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use butterpollo_media_policy::stutter_report::{Picture, Stage, Totals};

    #[test]
    fn the_recent_report_leaves_out_what_came_before_the_window() {
        let stats = Stats::default();
        let period = Duration::from_millis(8);
        let usual = Picture::new(2_000, 1_000, 4_000, 500, 500);
        let mut totals = Totals::default();
        totals.add(&vec![usual; 30], period);
        let mut stalled = vec![usual; 30];
        stalled[3] = Picture::new(2_000, 1_000, 30_000, 500, 500);
        totals.add(&stalled, period);
        stats.publish_stutter(&totals);
        let window = Duration::from_secs(30);
        let young = stats.stutter_recent(period, window, Instant::now());
        assert_eq!(
            young.dominant(),
            Some(Stage::Encode),
            "a young stream has its whole report"
        );
        // The stall's window was published 35 seconds ago; a quiet one since.
        let stalled_at = stats.stutter_history.lock().unwrap()[0].0;
        totals.add(&vec![usual; 30], period);
        *stats.stutter.lock().unwrap() = totals.clone();
        let later = stats.stutter_recent(period, window, stalled_at + Duration::from_secs(35));
        assert_eq!((later.pictures(), later.late_pictures()), (30, 0));
        assert_eq!(later.dominant(), None, "the stall is older than the window");
        for _ in 0..STUTTER_HISTORY + 2 {
            stats.publish_stutter(&totals);
        }
        assert_eq!(stats.stutter_history.lock().unwrap().len(), STUTTER_HISTORY);
    }

    #[test]
    fn mode_changes_inside_the_window_keep_the_report_from_before_it() {
        let stats = Stats::default();
        let period = Duration::from_millis(8);
        let usual = Picture::new(2_000, 1_000, 4_000, 500, 500);
        let mut stalled = vec![usual; 30];
        stalled[3] = Picture::new(2_000, 1_000, 30_000, 500, 500);
        let mut totals = Totals::default();
        let start = Instant::now();
        let publish = |totals: &Totals, seconds: u64| {
            let at = start + Duration::from_secs(seconds);
            keep_stutter(
                &mut stats.stutter_history.lock().unwrap(),
                at,
                totals.report(),
            );
        };
        // A stall 60 seconds into a long stream, then quiet five-second windows
        // and two mode changes inside the last 30 seconds.
        totals.add(&stalled, period);
        publish(&totals, 60);
        for seconds in [65, 70, 75, 80, 85, 90, 92, 94, 95, 100, 105] {
            totals.add(&vec![usual; 30], period);
            publish(&totals, seconds);
        }
        let now = start + Duration::from_secs(106);
        let recent = stats.stutter_since(&totals.report(), STUTTER_RECENT, now);
        assert_eq!(
            recent.late_pictures(),
            0,
            "the stall is older than the window"
        );
        assert!(recent.pictures() > 0 && recent.pictures() < totals.report().pictures());
        assert!(stats.stutter_history.lock().unwrap().len() < STUTTER_HISTORY);
    }
}
