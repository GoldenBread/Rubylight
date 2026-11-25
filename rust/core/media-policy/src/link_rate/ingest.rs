//! Sent-frame tracking, client FEC/keyframe/latch feedback ingestion, and fence checks.

use std::time::{Duration, Instant};

use rubylight_wire::fec_status::Status;

use super::{LinkRate, State, types::*};

impl LinkRate {
    /// Record the video frame the host just sent: its wire frame number,
    /// packets (data and parity) and wire bytes. One store, no allocation.
    pub fn on_sent(&mut self, frame: u32, packets: u32, wire_bytes: u32, now: Instant) {
        if self.state == State::Off {
            return;
        }
        self.last_sent = Some(match self.last_sent {
            Some(last) if frame.wrapping_sub(last) >= 1 << 31 => last,
            _ => frame,
        });
        self.ring[frame as usize % RING] = Sent {
            frame,
            packets,
            bytes: wire_bytes,
            missing: 0,
            at: Some(now),
        };
        self.events.sent += u64::from(packets);
    }

    pub(super) fn sent(&self, frame: u32) -> Option<Sent> {
        if self.ring.is_empty() {
            return None;
        }
        Some(self.ring[frame as usize % RING]).filter(|s| s.at.is_some() && s.frame == frame)
    }

    pub(super) fn admits(&self, frame: Option<u32>, now: Instant) -> bool {
        let Some(fence) = self.fence else {
            return true;
        };
        let sent = frame.and_then(|f| self.sent(f)).and_then(|s| s.at);
        fence.admits(frame, sent.unwrap_or(now))
    }

    /// One validated, deduplicated block of a client FEC status (0x5502).
    /// Feeds the packet-loss share (E2) and records that this client reports
    /// FEC, which turns the keyframe-request trigger (E4) off.
    pub fn on_status(&mut self, status: &Status, now: Instant) {
        self.fec_status_seen = true;
        if !self.is_active() || !self.admits(Some(status.frame), now) {
            return;
        }
        if self.sent(status.frame).is_none() {
            return;
        }
        let total = u32::from(status.data) + u32::from(status.parity);
        let received = u32::from(status.received_data) + u32::from(status.received_parity);
        // A block the client rebuilt, or had enough packets to rebuild, is
        // FEC's job and says nothing about the link on its own. Its report is
        // made the moment `data` packets are in, so the parity (and, on a path
        // that reorders, some data) not yet arrived is in flight, not lost: on
        // the 2026-10-24 Wi-Fi arm (3 +/- 2 ms delay, 0.5 % loss) that read as
        // 14 % of the data missing and cut the stream to its floor. While the
        // RTT is up, a queue is filling: the same share is then loss FEC is
        // absorbing for now, and the next step up in load breaks it.
        if received >= u32::from(status.data) {
            let data_missing = u32::from(status.data.saturating_sub(status.received_data));
            self.rebuilt_blocks += 1;
            self.rebuilt_packets += u64::from(total);
            self.rebuilt_data_missing += u64::from(data_missing);
            let data = u64::from(status.data.max(1));
            let elevated = self.rtt_elevated();
            // The baseline fades with the time since the last block that
            // renewed it; while the RTT is up only a silence counts, or a
            // long queue would fade its own baseline and cut itself.
            if let Some(at) = self.reorder_at.replace(now) {
                let dt = now.saturating_duration_since(at);
                if !elevated || dt >= REORDER_GAP {
                    self.reorder_share *= (-dt.as_secs_f64() / REORDER_FADE.as_secs_f64()).exp();
                }
            }
            let cap = f64::from(status.parity) / total.max(1) as f64 + REORDER_MARGIN;
            if !elevated {
                // What this path reorders, learned while nothing queues.
                let share = f64::from(data_missing) / data as f64;
                self.reorder_share += (share - self.reorder_share) / REORDER_BLOCKS;
                self.reorder_share = self.reorder_share.min(cap);
                self.excess_credit = 0;
                self.excess_high = 0;
            }
            // Reordering does not stop when the RTT rises for a moment (a roam,
            // a competing flow): only the packets over what the flat RTT
            // already showed are a queue. The excess is a running sum; only
            // what it rises over its own high-water mark counts, so noise
            // around the baseline counts nothing however long it goes on, and
            // a standing queue's extra loss adds up.
            let excess = if elevated {
                let expected = (self.reorder_share.min(cap) * data as f64).round() as i64;
                let credit = (self.excess_credit + i64::from(data_missing) - expected)
                    .max(-EXCESS_CREDIT_CAP);
                let counted = (credit - self.excess_high).max(0);
                self.excess_high = self.excess_high.max(credit);
                self.excess_credit = credit;
                u64::try_from(counted).unwrap_or(0)
            } else {
                0
            };
            if excess > 0 {
                // The parity still in flight cannot be told from lost, so the
                // share is taken from the data packets and scaled up to all of
                // them: 5 % of the data lost is 6 % of the block.
                let missing = (excess * u64::from(total)).div_ceil(data);
                self.events.missing += missing;
                self.events.lost(status.frame);
                self.ring[status.frame as usize % RING].missing +=
                    u32::try_from(missing).unwrap_or(u32::MAX);
            }
            return;
        }
        let missing = total - received;
        if missing > 0 {
            self.events.missing += u64::from(missing);
            self.events.lost(status.frame);
            self.ring[status.frame as usize % RING].missing += missing;
        }
    }

    /// A distinct frame the client could not rebuild, already classified by
    /// `KeyframeLoss` (not inside a host-change window, not counted as lost on
    /// send). Feeds E1.
    /// `keyframe` is whether the frame is one the host sent as a keyframe: one
    /// of those lost while a latch is watched shows the latch did not help.
    pub fn on_unrecoverable(&mut self, frame: u32, keyframe: bool, now: Instant) {
        if !self.is_active() {
            return;
        }
        // The keyframe answering a latch is often sent, and reported lost,
        // inside the fence: it still did not arrive.
        if keyframe {
            for k in self.watch_keyframes.iter_mut().filter(|k| k.frame == frame) {
                k.failed = true;
            }
        }
        self.watch_event(Some(frame), now);
        if self.admits(Some(frame), now) {
            Events::distinct(&mut self.events.unrecoverable, now, frame);
            self.events.lost(frame);
            // A frame lost while no keyframe has come through since the latch
            // is the same proof a request is: the latch has not helped yet.
            if self.watching(now) && (keyframe || !self.keyframe_arrived(now)) {
                self.events.latch_failed = true;
            }
        }
    }

    /// An unrecoverable frame or network request under a latch watch. Two,
    /// [`WATCH_EVENT_GAP`] or more and [`WATCH_EVENT_SPAN`] or less apart, fail
    /// the latch. A frame sent before
    /// the latch is not evidence about it.
    fn watch_event(&mut self, frame: Option<u32>, now: Instant) {
        let Some(latched) = self.latch_at.filter(|_| self.watching(now)) else {
            return;
        };
        if let Some(frame) = frame
            && (self
                .sent(frame)
                .and_then(|s| s.at)
                .is_some_and(|at| at < latched)
                || self
                    .watch_events
                    .iter()
                    .any(|(_, seen)| *seen == Some(frame)))
        {
            return;
        }
        if self.watch_events.iter().any(|(at, _)| {
            let apart = now.saturating_duration_since(*at);
            apart >= WATCH_EVENT_GAP && apart <= WATCH_EVENT_SPAN
        }) {
            self.events.latch_failed = true;
        }
        if self.watch_events.len() >= EVENT_CAP {
            self.watch_events.remove(0);
        }
        self.watch_events.push((now, frame));
    }

    /// The host is sending `frame` as a keyframe.
    pub fn on_keyframe_sent(&mut self, frame: u32, now: Instant) {
        if !self.is_active() || !self.watching(now) || self.latch_at.is_none_or(|at| now < at) {
            return;
        }
        if self.watch_keyframes.len() >= EVENT_CAP {
            self.watch_keyframes.remove(0);
        }
        self.watch_keyframes.push(WatchedKeyframe {
            frame,
            at: now,
            failed: false,
        });
    }

    /// A keyframe sent since the latch's fence that the client had time to
    /// report lost and did not.
    fn keyframe_arrived(&self, now: Instant) -> bool {
        self.watch_keyframes
            .iter()
            .any(|k| !k.failed && now.saturating_duration_since(k.at) >= self.fence_wait())
    }

    /// A frame the host could not send whole because its socket refused
    /// packets (a full buffer, WSAEWOULDBLOCK or WSAENOBUFS). Feeds E3 and
    /// [`Self::host_limited`].
    pub fn on_send_loss(&mut self, frame: u32, now: Instant) {
        if !self.is_active() {
            return;
        }
        Events::distinct(&mut self.events.refused, now, frame);
        self.events
            .refused
            .retain(|(at, _)| now.saturating_duration_since(*at) < EVIDENCE_WINDOW);
        if self.events.refused.len() >= SEND_LOSS_FRAMES {
            self.host_loss_at = Some(now);
        }
        self.on_send_too_large(frame, now);
    }

    /// A frame the host could not send whole because the path refused its
    /// datagrams' size. Feeds E3 only: the PC is not short of anything.
    pub fn on_send_too_large(&mut self, frame: u32, now: Instant) {
        if self.is_active() && self.admits(Some(frame), now) {
            Events::distinct(&mut self.events.send_losses, now, frame);
            self.events.lost(frame);
        }
    }

    /// The host's socket refused [`SEND_LOSS_FRAMES`] frames within
    /// [`EVIDENCE_WINDOW`], less than [`HOST_LOSS_HOLD`] ago: the PC's own
    /// send path is the limit for now, and no probe goes up until it has been
    /// quiet that long.
    pub fn host_limited(&self, now: Instant) -> bool {
        self.host_loss_at
            .is_some_and(|at| now.saturating_duration_since(at) < HOST_LOSS_HOLD)
    }

    /// A keyframe or RFI request. `network` is `KeyframeLoss`'s verdict that
    /// the network, not the host or the decoder queue, caused it. Feeds E4,
    /// and only from a client that has sent no FEC status.
    pub fn on_keyframe_request(&mut self, network: bool, now: Instant) {
        if !network || !self.is_active() {
            return;
        }
        self.watch_event(None, now);
        if !self.admits(None, now) {
            return;
        }
        // Under a latch watch a request from a client that reports FEC status
        // counts when no keyframe has got through since the latch.
        if self.watching(now) && (self.events.latch_failed || !self.keyframe_arrived(now)) {
            self.events.latch_failed = true;
        } else if self.fec_status_seen && !self.probe_unconfirmed(now) {
            return;
        }
        if self.events.requests.len() >= EVENT_CAP {
            self.events.requests.remove(0);
        }
        self.events.requests.push(now);
    }

    /// A reference invalidation request for frames `first..=last` (0x0301).
    /// The client asks again for every frame it cannot decode until the
    /// repair arrives, so a request whose range starts at or before the
    /// newest frame of the incident counted last, within [`INCIDENT_GAP`] of
    /// its first request, is the same loss and not counted again (2026-10-29:
    /// 11 requests for frames 2685-2695 after one lost frame counted as 10
    /// and cut 21 to 14.5 Mbps on a link that carried the stream). A range
    /// the host cannot read (`first` 0 or past `last`) is a request of its own.
    /// Under a latch watch or an unconfirmed probe step every request counts,
    /// as keyframe requests do: there repeats are the evidence.
    pub fn on_invalidation(&mut self, first: u64, last: u64, network: bool, now: Instant) {
        if !network || !self.is_active() {
            return;
        }
        if first == 0 || first > last || !self.admits(None, now) {
            self.on_keyframe_request(network, now);
            return;
        }
        let merge = !(self.watching(now) || self.probe_unconfirmed(now));
        match &mut self.incident {
            Some((newest, began))
                if merge
                    && first <= newest.saturating_add(1)
                    && now.saturating_duration_since(*began) < INCIDENT_GAP =>
            {
                *newest = (*newest).max(last);
            }
            incident => {
                *incident = Some((last, now));
                self.on_keyframe_request(network, now);
            }
        }
    }

    /// `KeyframeLoss` latched (paced like Wi-Fi). If the pacing before the
    /// latch was faster than the Wi-Fi rate the latch may be the whole fix
    /// (the Onn burst-loss port), so give it one fence before cutting. `new`
    /// is whether the latch is new; a latch reported again starts nothing, so
    /// the caller may report every latch event without a second fence.
    pub fn on_latch(&mut self, now: Instant, was_fast: bool, new: bool) {
        if new && was_fast && self.is_active() {
            self.set_fence(now);
            self.latch_watch = self.fence.map(|f| f.at + LATCH_WATCH);
            self.latch_at = Some(now);
        }
    }

    /// A provisional (start-up) latch lifted: pacing is back at the route's
    /// rate. `now_fast` is whether that rate is faster than the Wi-Fi rate. The
    /// pace changed under frames already in flight, so one fence keeps their
    /// reports (made under the slow pace) from deciding anything at the new one.
    pub fn on_unlatch(&mut self, now: Instant, now_fast: bool) {
        if now_fast && self.is_active() {
            self.set_fence(now);
        }
    }

    /// When the current fence was set to expire, for tests.
    #[cfg(any(test, feature = "test-internals"))]
    pub fn fence_at(&self) -> Option<Instant> {
        self.fence.map(|f| f.at)
    }

    /// A client control ping (`type 4`, 8 bytes), the proof that feedback is
    /// fresh.
    pub fn on_ping(&mut self, now: Instant, payload: &[u8]) {
        if payload.len() == 8 && payload[..4] == 4u32.to_le_bytes() {
            self.last_ping = Some(now);
        }
    }

    /// A probe step is unconfirmed, or was confirmed within the evidence
    /// window: whatever breaks now is likely the step. Not while the picture
    /// is static: a pending step waits for motion (its clock is held), and
    /// requests made when next to nothing is sent are not the step's doing.
    pub(super) fn probe_unconfirmed(&self, now: Instant) -> bool {
        !self.static_picture
            && (self.pending.iter().any(Option::is_some) || self.recent(now).is_some())
    }

    /// The step confirmed last, while a failure is still its own: within the
    /// evidence window, or its confirm window if that is longer (loss behind
    /// a deep buffer shows up late).
    pub(super) fn recent(&self, now: Instant) -> Option<Confirmed> {
        self.confirmed
            .filter(|c| now.saturating_duration_since(c.at) < c.window)
    }

    /// The step that a failure now belongs to: its rate, the rate to go back
    /// to, and whether it went past the learned ceiling.
    pub(super) fn failing(&self, now: Instant) -> Option<(u64, u64, bool)> {
        match self.pending.iter().flatten().next() {
            Some(oldest) => Some((oldest.rate, self.good, oldest.retry)),
            None => self.recent(now).map(|c| (c.rate, c.before, c.retry)),
        }
    }

    /// Whether a latch is still being judged (see [`LATCH_WATCH`]).
    pub(super) fn watching(&self, now: Instant) -> bool {
        self.latch_watch.is_some_and(|until| now < until)
    }

    pub(super) fn set_fence(&mut self, now: Instant) {
        self.latch_watch = None;
        self.latch_at = None;
        self.watch_keyframes.clear();
        self.watch_events.clear();
        let wait = MIN_FENCE.max(self.rtt * 2 + FENCE_MARGIN);
        self.fence = Some(Fence {
            frame: self.last_sent.map(|f| f.wrapping_add(1 + PIPELINE_FRAMES)),
            at: now + wait,
        });
        self.incident = None;
        self.events.clear();
    }

    pub(super) fn fence_wait(&self) -> Duration {
        MIN_FENCE.max(self.rtt * 2 + FENCE_MARGIN)
    }
}
