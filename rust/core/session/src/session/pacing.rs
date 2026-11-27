//! Keyframe recovery, reference invalidation, automatic pacing, and keyframe-loss guard.
use super::*;

#[derive(Default)]
pub(super) struct RfiConfirm {
    armed_us: u64,
    repair_sent: Option<(u32, u64)>,
    failed: bool,
    repair_clean_blocks: u8,
    repair_block_count: u8,
    clean_fec_after: bool,
    transport_loss: bool,
    clean_transport_after: bool,
    early_fec: [Option<(u32, u8, u8, bool, bool)>; 8],
    next_early_fec: usize,
}

impl RfiConfirm {
    fn on_fec(
        &mut self,
        frame: u32,
        block_idx: u8,
        block_count: u8,
        clean: bool,
        unrecoverable: bool,
    ) {
        let Some((repair_frame, _)) = self.repair_sent else {
            let slot = self.next_early_fec % self.early_fec.len();
            self.early_fec[slot] = Some((frame, block_idx, block_count, clean, unrecoverable));
            self.next_early_fec = self.next_early_fec.wrapping_add(1);
            return;
        };
        self.apply_fec(
            repair_frame,
            frame,
            block_idx,
            block_count,
            clean,
            unrecoverable,
        );
    }

    fn apply_fec(
        &mut self,
        repair_frame: u32,
        frame: u32,
        block_idx: u8,
        block_count: u8,
        clean: bool,
        unrecoverable: bool,
    ) {
        let ahead = frame.wrapping_sub(repair_frame);
        if ahead >= (1 << 31) {
            return;
        }
        if unrecoverable {
            self.failed = true;
            return;
        }
        if ahead == 0 {
            if clean && block_idx < 8 && (1..=4).contains(&block_count) {
                self.repair_block_count = block_count;
                self.repair_clean_blocks |= 1 << block_idx;
            }
        } else if clean {
            self.clean_fec_after = true;
        }
    }

    fn repair_fec_clean(&self) -> bool {
        self.repair_block_count > 0
            && self.repair_clean_blocks == ((1u16 << self.repair_block_count) - 1) as u8
    }
}

impl<P, A> Session<P, A> {
    /// Arms post-RFI recovery tracking for the LTR repair frame about to be encoded.
    pub fn begin_rfi_confirmation(&self) {
        let armed_us = self.transport_sent.micros(Instant::now());
        *self.rfi_confirm.lock().unwrap() = Some(RfiConfirm {
            armed_us,
            ..RfiConfirm::default()
        });
        self.rfi_confirm_active.store(true, Ordering::Release);
    }

    /// Clears post-RFI confirmation tracking once an IDR runs or recovery is confirmed.
    pub fn clear_rfi_confirmation(&self) {
        if self.rfi_confirm_active.swap(false, Ordering::AcqRel) {
            *self.rfi_confirm.lock().unwrap() = None;
        }
    }

    /// Records the wire frame number of the LTR repair picture once handed to the socket.
    pub fn note_rfi_repair_sent(&self, wire_frame: u32, lost_on_send: bool) {
        if !self.rfi_confirm_active.load(Ordering::Acquire) {
            return;
        }
        let now_us = self.transport_sent.micros(Instant::now());
        let mut guard = self.rfi_confirm.lock().unwrap();
        let Some(confirm) = guard.as_mut() else {
            return;
        };
        if lost_on_send {
            confirm.failed = true;
            return;
        }
        if confirm.repair_sent.is_none() {
            confirm.repair_sent = Some((wire_frame, now_us));
            let early = std::mem::take(&mut confirm.early_fec);
            for (frame, block_idx, block_count, clean, unrecoverable) in early.into_iter().flatten()
            {
                confirm.apply_fec(
                    wire_frame,
                    frame,
                    block_idx,
                    block_count,
                    clean,
                    unrecoverable,
                );
            }
        }
    }

    pub(super) fn note_rfi_fec_status(
        &self,
        payload: &[u8],
        block: butterpollo_media_policy::adaptive_fec::Block,
        route_abandoned: bool,
    ) {
        let Ok(status) = rubylight_wire::fec_status::Status::parse(payload) else {
            return;
        };
        let mut guard = self.rfi_confirm.lock().unwrap();
        let Some(confirm) = guard.as_mut() else {
            return;
        };
        let unrecoverable = route_abandoned
            || matches!(
                block,
                butterpollo_media_policy::adaptive_fec::Block::Unrecoverable
            );
        let clean = !route_abandoned
            && matches!(block, butterpollo_media_policy::adaptive_fec::Block::Clean);
        confirm.on_fec(
            status.frame,
            status.block,
            status.blocks,
            clean,
            unrecoverable,
        );
    }

    pub(super) fn note_rfi_transport_feedback(&self, payload: &[u8], now_us: u64) {
        let Some(report) = butterpollo_media_policy::congestion::TransportFeedback::decode(payload)
        else {
            return;
        };
        let mut guard = self.rfi_confirm.lock().unwrap();
        let Some(confirm) = guard.as_mut() else {
            return;
        };
        let skipped = report.flags & rubylight_protocol::transport_feedback::FLAG_SKIPPED != 0;
        if skipped {
            confirm.transport_loss = true;
        }
        let mut arrived_after = false;
        for (id, offset) in report.packets() {
            let Some((sent_us, _)) = self.transport_sent.lookup(id, now_us) else {
                continue;
            };
            if sent_us >= confirm.armed_us && offset.is_none() {
                confirm.transport_loss = true;
            }
            if let Some((_, repair_done_us)) = confirm.repair_sent
                && sent_us >= repair_done_us
                && offset.is_some()
            {
                arrived_after = true;
            }
        }
        if arrived_after && !skipped {
            confirm.clean_transport_after = true;
        }
    }

    /// True when the client actively proved clean frame delivery at/after the LTR repair frame
    /// via `0x5502` or `0x5547` and sent no follow-up `0x0301`/`0x0302` recovery requests.
    pub fn rfi_recovery_confirmed(&self) -> bool {
        if !self.rfi_confirm_active.load(Ordering::Acquire) {
            return false;
        }
        let guard = self.rfi_confirm.lock().unwrap();
        let Some(confirm) = guard.as_ref() else {
            return false;
        };
        let Some((repair_frame, _)) = confirm.repair_sent else {
            return false;
        };
        if confirm.failed || self.idr.load(Ordering::Acquire) {
            return false;
        }
        if self
            .invalidation
            .lock()
            .unwrap()
            .is_some_and(|(_, last)| (last as u32).wrapping_sub(repair_frame) < (1 << 31))
        {
            return false;
        }
        confirm.repair_fec_clean()
            || confirm.clean_fec_after
            || (!confirm.transport_loss && confirm.clean_transport_after)
    }

    pub fn request_idr(&self) {
        self.stats.idr_requests.fetch_add(1, Ordering::Relaxed);
        // Intra-only frames already recover; feedback must not bypass cadence.
        if self.config.codec != 3 {
            self.idr.store(true, Ordering::Release);
            self.wake_recovery();
        }
    }
    /// A keyframe the host makes on its own initiative (a recreated encoder,
    /// a frame beyond the packet limit). Whatever the client requests around
    /// it is a reaction to the host, not evidence about the network.
    pub fn request_host_keyframe(&self) {
        self.keyframe_loss.lock().unwrap().host_keyframe();
        self.request_idr();
    }
    /// A keyframe a second after a reference invalidation the decoder may not
    /// have followed. It answers loss, so it is counted like loss.
    pub fn request_confirming_keyframe(&self) {
        self.clear_rfi_confirmation();
        self.keyframe_loss.lock().unwrap().loss_keyframe();
        self.idr.store(true, Ordering::Release);
    }
    /// A keyframe the client needs after losing frames: its request (0x0302),
    /// or an invalidation the encoder cannot repair with a reference frame.
    pub fn request_recovery_keyframe(&self) {
        if self.rfi_confirm_active.load(Ordering::Acquire)
            && let Some(confirm) = self.rfi_confirm.lock().unwrap().as_mut()
        {
            confirm.failed = true;
        }
        let (latched, ignored) = {
            let mut loss = self.keyframe_loss.lock().unwrap();
            let latched = loss.requested(Instant::now());
            if self.config.codec == 3 {
                // Intra-only: no keyframe will answer it.
                loss.unanswerable();
            }
            (latched, loss.take_ignored())
        };
        self.report_keyframe_loss(latched, ignored);
        self.request_idr();
    }
    /// The client asked for a keyframe (0x0302): the same, and evidence for
    /// the bitrate controller when it is the network and not the host or the
    /// client's decode queue that caused the request.
    pub fn request_client_keyframe(&self) {
        self.link_request();
        self.request_recovery_keyframe();
    }
    /// The host could not send enough of a frame for the client to rebuild
    /// it, so the next frame is a keyframe. Moonlight, waiting for one,
    /// then recovers on that frame and sends no request of its own.
    pub fn request_send_loss_recovery(&self) {
        self.stats
            .send_loss_recoveries
            .fetch_add(1, Ordering::Relaxed);
        if self.config.codec != 3 {
            self.keyframe_loss.lock().unwrap().loss_keyframe();
            self.idr.store(true, Ordering::Release);
        }
    }
    pub fn request_invalidation(&self, first: u64, last: u64) {
        self.stats
            .reference_invalidations
            .fetch_add(1, Ordering::Relaxed);
        if self.config.codec == 3 {
            return;
        }
        self.link_invalidation(first, last);
        if first == 0 || first > last {
            self.request_recovery_keyframe();
            return;
        }
        if self.rfi_confirm_active.load(Ordering::Acquire)
            && let Some(confirm) = self.rfi_confirm.lock().unwrap().as_mut()
            && let Some((repair_frame, _)) = confirm.repair_sent
            && (last as u32).wrapping_sub(repair_frame) < (1 << 31)
        {
            confirm.failed = true;
        }
        let mut pending = self.invalidation.lock().unwrap();
        *pending = Some(pending.map_or((first, last), |(a, b)| (a.min(first), b.max(last))));
        drop(pending);
        self.wake_recovery();
    }
    fn wake_recovery(&self) {
        if let Some(wake) = self.recovery_wake.lock().unwrap().as_ref() {
            wake();
        }
    }
    fn wireless_unknown_link(&self) -> bool {
        self.config.client_wireless && self.client_link_mbps() == 0
    }

    pub fn configure_automatic_pacing(&self, enabled: bool) {
        let mut pacing = butterpollo_media_policy::adaptive_pacing::AdaptivePacing::new(enabled);
        pacing.set_wireless_unknown_link(self.wireless_unknown_link());
        *self.automatic_pacing.lock().unwrap() = pacing;
        self.automatic_pacing_on.store(enabled, Ordering::Release);
    }
    /// Whether the client reports a wireless route (`x-rl-video[0].clientWireless`)
    /// and no adaptive controller owns this stream. Then the route base is
    /// paced as Wi-Fi from the first frame. When the controller owns the stream
    /// the base stays the host route's, so the controller starts conservatively
    /// at its own seed and can probe up toward the real ceiling.
    pub fn client_wireless_sets_route_pacing(&self) -> bool {
        self.config.client_wireless && !self.automatic_pacing_enabled()
    }
    /// Read per frame, so it is the flag set at stream start, not the mutex.
    pub fn automatic_pacing_enabled(&self) -> bool {
        self.automatic_pacing_on.load(Ordering::Acquire)
    }
    pub fn video_pacing_bps(&self, base: u64, needed: u64) -> u64 {
        let mut pacing = self.automatic_pacing.lock().unwrap();
        pacing.set_wireless_unknown_link(self.wireless_unknown_link());
        let paced = pacing.rate(base, needed, self.bitrate.load(Ordering::Acquire));
        drop(pacing);
        self.adaptive_pacing_bps(paced)
    }
    /// Encoder-side budget queries must not change the sender's feedback
    /// state when recovery parity differs from the current ordinary-frame FEC.
    pub fn preview_video_pacing_bps(&self, base: u64, needed: u64) -> u64 {
        let paced = self.automatic_pacing.lock().unwrap().preview_rate_for(
            base,
            needed,
            self.bitrate.load(Ordering::Acquire),
            self.wireless_unknown_link(),
        );
        self.adaptive_pacing_bps(paced)
    }
    /// Pacing slows for the session once the client loses a keyframe or
    /// repeatedly reports unrecoverable ordinary frames
    /// (`keyframe_loss_pacing`), including PyroWave's independent intra frames.
    pub fn configure_keyframe_loss(&self, enabled: bool) {
        self.configure_remembered_keyframe_loss(enabled, false);
    }
    /// Restore only an eligible route's recent loss history. Explicit pacing,
    /// disabled adaptation and PyroWave retain their own policies.
    pub fn configure_remembered_keyframe_loss(&self, enabled: bool, remembered: bool) {
        let remembered = remembered && enabled && self.config.codec != 3;
        let mut loss = if remembered {
            butterpollo_media_policy::keyframe_loss::KeyframeLoss::remembered(
                "remembered from the previous session",
            )
        } else {
            butterpollo_media_policy::keyframe_loss::KeyframeLoss::new(enabled)
        };
        loss.set_reports_losses(
            self.config.moonlight_features & butterpollo_media_policy::rtsp::FEC_STATUS_FEATURE
                != 0,
        );
        *self.keyframe_loss.lock().unwrap() = loss;
        if remembered {
            tracing::info!(stream_id = %self.launch.id, "pacing video like Wi-Fi from the first frame: remembered from the previous session");
            self.launch.warnings.set("network_keyframe_loss", "This device lost video frames in a previous stream at this address. Video starts at the Wi-Fi pacing rate to limit packet bursts without reducing the encoder bitrate. This memory expires after 24 hours and the host periodically tests the fast rate. Reset it in Devices after changing the network.");
        } else {
            self.launch.warnings.clear("network_keyframe_loss");
        }
    }
    pub(super) fn report_keyframe_loss(
        &self,
        latched: Option<butterpollo_media_policy::keyframe_loss::Latched>,
        ignored: Option<String>,
    ) {
        if let Some(why) = ignored {
            tracing::info!(stream_id = %self.launch.id, why, "video loss evidence not counted");
        }
        if let Some(butterpollo_media_policy::keyframe_loss::Latched {
            reason,
            detail,
            kind,
            confirmed,
            from,
            host_only,
        }) = latched
        {
            use butterpollo_media_policy::keyframe_loss::LatchKind;
            if self.link_sent.live() {
                let mut link = self.link_rate.lock().unwrap();
                link.drain(&self.link_sent);
                // A provisional latch confirmed by more evidence is the same
                // latch, already counted when it began.
                link.rate.on_latch(
                    Instant::now(),
                    self.pace_was_fast.load(Ordering::Relaxed),
                    !confirmed,
                );
            }
            if confirmed && matches!(from, Some(LatchKind::Send | LatchKind::Sustained)) {
                tracing::warn!(stream_id = %self.launch.id, reason, detail, "the client reported video loss after the host's send loss; pacing video like a Wi-Fi route for the rest of the stream");
            } else if confirmed {
                tracing::warn!(stream_id = %self.launch.id, reason, detail, "more video loss after the start-up keyframe; pacing video like a Wi-Fi route for the rest of the stream");
            } else if kind == LatchKind::Send {
                tracing::warn!(stream_id = %self.launch.id, reason, detail, "the host's socket refused video packets; pacing video like a Wi-Fi route until it has been clean for 10 s");
            } else if kind == LatchKind::Startup {
                tracing::warn!(stream_id = %self.launch.id, reason, detail, "the stream's first keyframe was lost; pacing video like a Wi-Fi route until it has been clean for 10 s");
            } else if host_only {
                // After a lift, or on a remembered route: held, never saved.
                tracing::warn!(stream_id = %self.launch.id, reason, detail, "the host's socket refused video packets in a stream that already slowed down; pacing video like a Wi-Fi route for the rest of the stream");
            } else {
                tracing::warn!(stream_id = %self.launch.id, reason, detail, "video frames are not arriving; pacing video like a Wi-Fi route for the rest of the stream");
            }
            if kind == LatchKind::Startup {
                self.launch.warnings.set("network_keyframe_loss", format!("The first video frame was lost on the way to this device ({reason}), so video is paced like Wi-Fi for now. This limits packet bursts without reducing the encoder bitrate. The host returns to the normal rate after 10 seconds without further loss."));
            } else if kind == LatchKind::Send {
                self.launch.warnings.set("network_keyframe_loss", format!("The PC could not send video packets fast enough ({reason}; its CPU or network adapter fell behind), so video is paced like Wi-Fi for now. This limits packet bursts without reducing the encoder bitrate. The host returns to the normal rate after 10 seconds without further loss."));
            } else if host_only {
                self.launch.warnings.set("network_keyframe_loss", format!("The PC could not send video packets fast enough ({reason}; its CPU or network adapter fell behind), so video is paced like Wi-Fi for the rest of the stream. This limits packet bursts without reducing the encoder bitrate. The device reported no loss; close other work on the PC, or check the network adapter and its driver, if this continues."));
            } else {
                self.launch.warnings.set("network_keyframe_loss", format!("Video frames were lost on the way to this device ({reason}), so video is now paced like Wi-Fi for the rest of the stream. This limits packet bursts without reducing the encoder bitrate. Check the network path if loss continues."));
            }
        }
    }
    /// Read the loss guard without recording a frame that has not been sent.
    /// Sender selection uses this before deciding whether to send inline.
    pub fn keyframe_loss_rate_bps(&self, base: u64, safe: u64) -> u64 {
        self.keyframe_loss.lock().unwrap().bps(base, safe)
    }
    /// The host dropped this frame because the client's network route was
    /// unavailable: mark it abandoned for `KeyframeLoss` so client repeats or FEC
    /// reports do not treat it as in-transit loss, without penalizing send loss
    /// counters, pacing, or link rate.
    pub fn keyframe_abandoned_route(&self, wire_frame: u32, reached_socket: bool) {
        self.keyframe_loss
            .lock()
            .unwrap()
            .route_abandoned(wire_frame, reached_socket);
    }
    /// The host stopped sending this frame part way because its socket
    /// refused packets: loss on send. See [`Self::keyframe_abandoned_as`].
    pub fn keyframe_abandoned(&self, wire_frame: u32) {
        self.keyframe_abandoned_as(wire_frame, true);
    }
    /// The host stopped sending this frame part way: loss on send. It counts
    /// toward slower pacing under its own reason, and the client's report of
    /// the same frame is not counted again. `refused` is whether the socket
    /// refused packets of it (a full buffer); a frame lost only to datagrams
    /// too large for the path does not make the PC's send path the limit.
    pub fn keyframe_abandoned_as(&self, wire_frame: u32, refused: bool) {
        self.stats
            .frames_lost_on_send
            .fetch_add(1, Ordering::Relaxed);
        let period = self.stream_mode().period();
        let (latched, ignored) = {
            let mut loss = self.keyframe_loss.lock().unwrap();
            (
                loss.abandoned(wire_frame, period, Instant::now()),
                loss.take_ignored(),
            )
        };
        if self.link_sent.live() {
            let mut link = self.link_rate.lock().unwrap();
            link.drain(&self.link_sent);
            if refused {
                link.rate.on_send_loss(wire_frame, Instant::now());
            } else {
                link.rate.on_send_too_large(wire_frame, Instant::now());
            }
        }
        self.report_keyframe_loss(latched, ignored);
    }
    pub fn keyframe_loss_active(&self) -> bool {
        self.keyframe_loss.lock().unwrap().latched()
    }
    pub fn keyframe_loss_reason(&self) -> Option<&'static str> {
        self.keyframe_loss.lock().unwrap().reason()
    }
    pub fn keyframe_loss_observed_at(&self) -> Option<Instant> {
        self.keyframe_loss.lock().unwrap().observed_at()
    }
    /// When loss last proved sustained: the only loss the next session
    /// inherits. A provisional (start-up) latch has none.
    pub fn keyframe_loss_sustained_at(&self) -> Option<Instant> {
        self.keyframe_loss.lock().unwrap().sustained_at()
    }
    /// Any latch happened in this stream, even a provisional one that lifted.
    pub fn keyframe_loss_ever_latched(&self) -> bool {
        self.keyframe_loss.lock().unwrap().ever_latched()
    }
    /// The pacing rate for the frame about to be sent as `wire_frame`: the
    /// route policy's `base`, or after confirmed losses at most `safe`, the
    /// Wi-Fi route's rate for this stream.
    pub fn keyframe_loss_bps(&self, base: u64, safe: u64, wire_frame: u32, keyframe: bool) -> u64 {
        self.keyframe_loss_bps_at(base, safe, wire_frame, keyframe, Instant::now())
    }
    pub(super) fn keyframe_loss_bps_at(
        &self,
        base: u64,
        safe: u64,
        wire_frame: u32,
        keyframe: bool,
        now: Instant,
    ) -> u64 {
        let (bps, lifted) = {
            let mut loss = self.keyframe_loss.lock().unwrap();
            loss.sending_at(wire_frame, keyframe, now);
            (loss.bps(base, safe), loss.take_lifted())
        };
        // Fast is what was applied to this frame, not what the route allows.
        let fast = bps > safe;
        self.pace_was_fast.store(fast, Ordering::Relaxed);
        // Every PyroWave picture is intra and its loss guard is off, so no
        // latch watch needs them, and the sender must not wait for this lock.
        if keyframe && self.config.codec != 3 && self.link_sent.live() {
            self.link_rate
                .lock()
                .unwrap()
                .rate
                .on_keyframe_sent(wire_frame, now);
        }
        if let Some(why) = lifted {
            tracing::info!(stream_id = %self.launch.id, why, "returning to the route's pacing rate: the loss did not recur");
            self.launch.warnings.clear("network_keyframe_loss");
            if self.link_sent.live() {
                let mut link = self.link_rate.lock().unwrap();
                link.drain(&self.link_sent);
                link.rate.on_unlatch(now, fast);
            }
        }
        bps
    }
    /// Whether legacy `AdaptiveBitrate` is configured; skips its mutex on control pings when off.
    pub fn adaptive_bitrate_enabled(&self) -> bool {
        self.adaptive_bitrate_on.load(Ordering::Acquire)
    }

    pub fn record_control_ping(&self, now: Instant, payload: &[u8]) {
        if self.adaptive_bitrate_enabled() {
            self.adaptive_bitrate.lock().unwrap().on_ping(now, payload);
        }
        if self.link_sent.live() {
            self.link_rate.lock().unwrap().rate.on_ping(now, payload);
        }
        self.automatic_pacing.lock().unwrap().on_ping(now, payload);
    }
    pub fn tick_automatic_pacing(&self, now: Instant, rtt: Duration) -> Option<(u64, u64)> {
        self.tick_automatic_pacing_step(now, rtt)
            .map(|step| (step.old_bps, step.new_bps))
    }

    pub fn tick_automatic_pacing_step(
        &self,
        now: Instant,
        rtt: Duration,
    ) -> Option<butterpollo_media_policy::adaptive_pacing::PacingStep> {
        let period = self.stream_mode().period();
        let mut pacing = self.automatic_pacing.lock().unwrap();
        pacing.set_wireless_unknown_link(self.wireless_unknown_link());
        pacing.tick_step(
            now,
            butterpollo_media_policy::adaptive_pacing::Traffic {
                frames: self.stats.frames.load(Ordering::Relaxed),
                bytes: self.stats.bytes.load(Ordering::Relaxed),
                packets: self.stats.packets.load(Ordering::Relaxed),
                send_losses: self.stats.send_loss_recoveries.load(Ordering::Relaxed),
            },
            self.stats
                .video_frame
                .load(Ordering::Acquire)
                .checked_sub(1)
                .map(|v| v as u32),
            period,
            self.bitrate.load(Ordering::Acquire),
            rtt,
        )
    }
}
