//! Host adaptive bitrate, client bitrate takeover, and packet-level congestion control.
use super::*;

impl<P, A> Session<P, A> {
    /// Configures host bitrate adaptation; again after a roam, to forget the old path
    /// and the client's hints about it.
    pub fn configure_adaptive_bitrate(&self, enabled: bool, maximum_kbps: u32) {
        let mut stream = self.config.clone();
        stream.pyrowave_client_link_mbps = self.client_link_mbps();
        stream.pyrowave_client_pace_mbps = self.client_pace_mbps();
        let mut controller = self.adaptive_bitrate.lock().unwrap();
        controller.configure(enabled, &stream, maximum_kbps, self.fec_percent());
        self.adaptive_bitrate_on
            .store(controller.configured(), Ordering::Release);
    }
    /// A bitrate the client set in the terms of its launch (0x553C,
    /// `stream_policy::runtime_encoder_kbps`): the host's controllers stand
    /// back for the stream.
    pub fn set_client_bitrate(&self, kbps: u32) {
        self.take_client_bitrate(kbps, launch_fec_share(&self.config));
    }
    /// [`Self::set_client_bitrate`] for a bitrate that is the encoder's own
    /// (the HTTP bitrate route): no share of the launch's is inside it.
    pub fn set_client_encoder_bitrate(&self, kbps: u32) {
        self.take_client_bitrate(kbps, 0);
    }
    /// The client's bitrate and the launch's FEC share inside it, read as
    /// one pair, as [`Self::encoder_bitrate`] reads them.
    pub fn client_bitrate(&self) -> (u32, u32) {
        loop {
            let launch = self.fec_split_launch.load(Ordering::Acquire);
            let bitrate = self.bitrate.load(Ordering::Acquire);
            if self.fec_split_launch.load(Ordering::Acquire) == launch {
                return (bitrate, launch);
            }
        }
    }
    /// Stream Doctor's bitrate step-down and its undo: `kbps` with
    /// `launch_share` of the launch's FEC inside it, so the encoder's rate
    /// moves by the same fraction and an undo gives back exactly what it had.
    /// Only while the bitrate is still `expected`, checked under the locks
    /// every client and controller write takes: false, with nothing changed,
    /// when something moved it meanwhile.
    pub fn set_client_bitrate_from(&self, expected: u32, kbps: u32, launch_share: u32) -> bool {
        self.take_client_bitrate_if(Some(expected), kbps, launch_share)
    }
    /// `launch_share` is the FEC share of the launch's that is inside `kbps`.
    fn take_client_bitrate(&self, kbps: u32, launch_share: u32) {
        self.take_client_bitrate_if(None, kbps, launch_share);
    }
    fn take_client_bitrate_if(&self, expected: Option<u32>, kbps: u32, launch_share: u32) -> bool {
        let mut controller = self.adaptive_bitrate.lock().unwrap();
        {
            // The link controller's own writes take this lock too, so none
            // lands after the client's value.
            let mut link = self.link_rate.lock().unwrap();
            let mut congestion = self.congestion.lock().unwrap();
            if expected.is_some_and(|expected| self.bitrate.load(Ordering::Acquire) != expected) {
                return false;
            }
            controller.client_takeover();
            self.adaptive_bitrate_on.store(false, Ordering::Release);
            self.adaptive_video_pacing_bps.store(0, Ordering::Release);
            link.takeover();
            self.link_sent.set_live(false);
            congestion.takeover();
            drop(congestion);
            self.transport_sent.disable();
            self.congestion_pacing_bps.store(0, Ordering::Release);
            // The encoder reads these without the lock. In this order a frame in between
            // gets too little: never a cut's bitrate unsplit, nor (with its second look at
            // the share) a bitrate with a share put back that is not inside it.
            self.fec_split_full.store(0, Ordering::Release);
            if launch_share == 0 {
                self.fec_split_launch.store(0, Ordering::Release);
            }
            self.bitrate.store(kbps, Ordering::Release);
            self.fec_split_launch.store(launch_share, Ordering::Release);
        }
        self.launch.warnings.clear("host_bitrate_budget");
        self.launch.warnings.clear(LINK_WARNING);
        self.link_notices.note_full(kbps);
        true
    }
    pub fn adaptive_pacing_bps(&self, configured_bps: u64) -> u64 {
        let limit = self.adaptive_video_pacing_bps.load(Ordering::Acquire);
        let paced = if limit == 0 {
            configured_bps
        } else {
            configured_bps.min(limit)
        };
        // The bottleneck's rate, when congestion control found one.
        match self.congestion_pacing_bps.load(Ordering::Acquire) {
            0 => paced,
            congestion => paced.min(congestion),
        }
    }
    /// Starts packet-level congestion control for the stream: `enabled` is
    /// `Config::congestion_control`, `maximum_kbps` the user's `max_bitrate`, `shadow` the A/B's
    /// measure-only mode.
    pub fn configure_congestion(&self, enabled: bool, maximum_kbps: u32, shadow: bool) {
        self.rebuild_congestion(&self.config, enabled, maximum_kbps, shadow);
    }
    /// A controller for a stream whose mode or bitrate changed: a limit it set goes back to
    /// the bitrate from before it, what it learned is forgotten, a client takeover stays.
    pub fn rebuild_congestion(
        &self,
        stream: &Negotiated,
        enabled: bool,
        maximum_kbps: u32,
        shadow: bool,
    ) {
        let link = self.link_rate.lock().unwrap();
        let mut congestion = self.congestion.lock().unwrap();
        if let Some(kbps) = congestion.take_restore() {
            self.store_limited(self.bitrate.load(Ordering::Acquire), kbps, link.restore);
        }
        *congestion = congestion.rebuild(stream, enabled, maximum_kbps, shadow);
        if !congestion.active() {
            self.transport_sent.disable();
        }
        self.congestion_pacing_bps.store(0, Ordering::Release);
    }
    /// Whether congestion control runs for this stream (0x5547 is offered to it).
    pub fn congestion_active(&self) -> bool {
        self.congestion.lock().unwrap().active()
    }
    /// A 0x5547 report from the client. The first one turns the sender's ledger on, so a
    /// client that never reports costs the sender nothing; later ones are joined with it.
    /// Returns whether the report was valid.
    pub fn record_transport_feedback(&self, payload: &[u8]) -> bool {
        self.record_transport_feedback_at(payload, Instant::now())
    }
    fn record_transport_feedback_at(&self, payload: &[u8], now: Instant) -> bool {
        let mut congestion = self.congestion.lock().unwrap();
        if !congestion.active() {
            return false;
        }
        if !self.transport_sent.live() {
            self.transport_sent.enable(self.config.encryption & 2 != 0);
        }
        congestion.on_report(
            &self.transport_sent,
            self.transport_sent.micros(now),
            payload,
        )
    }
    /// Video packets just handed to the socket: one store each into the ledger, nothing
    /// while it is off. `overhead` is the IP and UDP framing per packet.
    pub fn record_transport_sent<T: AsRef<[u8]>>(&self, packets: &[T], overhead: usize) {
        if self.transport_sent.live() {
            self.transport_sent
                .record(packets, overhead, Instant::now());
        }
    }
    pub fn congestion_snapshot(&self) -> serde_json::Value {
        let congestion = self.congestion.lock().unwrap();
        congestion.snapshot(self.transport_sent.micros(Instant::now()))
    }
    /// Queueing delay p50/p95 (ms) and samples since the last call, for `stream timings`.
    pub fn take_congestion_window(&self) -> (Option<f64>, Option<f64>, u64) {
        self.congestion.lock().unwrap().take_window_delay()
    }
    /// Call from the feedback tick, after `tick_link_rate`: runs congestion control and
    /// applies its bitrate and pace. Returns the change to log and `(old, new)` kbps when the
    /// encoder bitrate changed.
    pub fn tick_congestion(
        &self,
        now: Instant,
        rtt: Duration,
    ) -> (
        Option<butterpollo_media_policy::congestion::Change>,
        Option<(u32, u32)>,
    ) {
        let link = self.link_rate.lock().unwrap();
        let mut congestion = self.congestion.lock().unwrap();
        if !congestion.active() {
            return (None, None);
        }
        let now_us = self.transport_sent.micros(now);
        let pace = congestion.tick(now_us, rtt, self.fec_percent(), self.stream_mode().period());
        self.congestion_pacing_bps
            .store(pace.unwrap_or(0), Ordering::Release);
        let current = self.bitrate.load(Ordering::Acquire);
        let applied = congestion.combine(current);
        self.store_limited(current, applied, link.restore.or(congestion.limited_from()));
        let change = congestion.take_change();
        let shadow = congestion.shadow();
        drop(congestion);
        drop(link);
        if let Some(change) = change.filter(|_| !shadow) {
            use butterpollo_media_policy::congestion::ChangeKind;
            match change.kind {
                ChangeKind::Restored | ChangeKind::Stale => self.link_notices.note_full(applied),
                ChangeKind::Overuse | ChangeKind::Loss => {
                    self.link_notices
                        .note(butterpollo_media_policy::link_notice::LinkNotice {
                            state: butterpollo_media_policy::link_notice::LinkState::Limited,
                            reason: butterpollo_media_policy::link_notice::LinkReason::Network,
                            ceiling_kbps: self.config.bitrate_kbps.max(applied),
                            current_kbps: applied.max(1),
                        })
                }
            }
        }
        (change, (applied != current).then_some((current, applied)))
    }
    pub fn reserve_controller_pcm(&self, streams: usize) {
        let mut controller = self.adaptive_bitrate.lock().unwrap();
        controller.reserve_controller_pcm(streams, self.fec_percent(), self.stream_mode().period());
        self.adaptive_video_pacing_bps
            .store(controller.video_pacing_bps(), Ordering::Release);
    }
    pub fn tick_adaptive_bitrate(&self, now: Instant, rtt: Duration) -> Option<(u32, u32)> {
        let mut controller = self.adaptive_bitrate.lock().unwrap();
        let previous = self.bitrate.load(Ordering::Acquire);
        let target = controller.tick(
            now,
            butterpollo_media_policy::adaptive_bitrate::Traffic {
                frames: self.stats.frames.load(Ordering::Relaxed),
                bytes: self.stats.bytes.load(Ordering::Relaxed),
                send_losses: self.stats.send_loss_recoveries.load(Ordering::Relaxed),
            },
            rtt,
            self.fec_percent(),
            self.stream_mode().period(),
            previous,
        );
        self.adaptive_video_pacing_bps
            .store(controller.video_pacing_bps(), Ordering::Release);
        if controller.budget_unusable() {
            self.launch.warnings.set("host_bitrate_budget", "Host bitrate adaptation cannot fit the negotiated audio and minimum video packets inside this network budget, so it is inactive. Use stereo audio, reduce FPS or raise the configured bitrate.");
        } else {
            self.launch.warnings.clear("host_bitrate_budget");
        }
        let target = target?;
        if target == previous {
            return None;
        }
        self.bitrate.store(target, Ordering::Release);
        Some((previous, target))
    }
}
