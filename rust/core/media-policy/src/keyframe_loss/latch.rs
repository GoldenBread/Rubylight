//! Loss counting and rate-latch state transitions for [`KeyframeLoss`].

use super::*;

impl KeyframeLoss {
    /// Counts a frame that did not get through; the third distinct one within
    /// two seconds slows the stream. `reported` is the client's report, not
    /// only the host's socket.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn count_loss(
        &mut self,
        frame: u32,
        last_sent: u32,
        period: Duration,
        now: Instant,
        reason: &'static str,
        what: &str,
        reported: bool,
    ) -> Option<Latched> {
        // A sustained latch the client's reports backed has nothing left to
        // learn; a provisional one, or one only the host saw, still does.
        if (self.observed_at.is_some() && !self.provisional() && self.sustained_at.is_some())
            || period.is_zero()
            || period.saturating_mul(last_sent.wrapping_sub(frame)) >= LOSS_WINDOW
        {
            return None;
        }
        // A provisional latch learns only from frames sent after it.
        if self.provisional() && !self.after_latch(frame) {
            return None;
        }
        // The normal feedback parser accepts up to 1024 previous frames. A
        // delayed train of old reports must not look like current congestion.
        // Keep this additional fence tied to the negotiated frame period.
        for loss in &mut self.recent_losses {
            if loss.is_some_and(|(_, at, _)| now.saturating_duration_since(at) >= LOSS_WINDOW) {
                *loss = None;
            }
        }
        if self
            .recent_losses
            .iter()
            .flatten()
            .any(|(seen, _, _)| *seen == frame)
        {
            return None;
        }
        // Full only while a latch host-only evidence made still learns: the
        // newest loss replaces the oldest, so a client report can confirm it.
        let slot = match self.recent_losses.iter().position(Option::is_none) {
            Some(vacant) => vacant,
            None => self
                .recent_losses
                .iter()
                .enumerate()
                .min_by_key(|(_, loss)| loss.map(|(_, at, _)| at))
                .map(|(i, _)| i)?,
        };
        self.recent_losses[slot] = Some((frame, now, reported));
        if self.provisional() {
            self.last_loss_at = Some(now);
        }
        if self.recent_losses.iter().all(Option::is_some) {
            let frames: Vec<u32> = self
                .recent_losses
                .iter()
                .flatten()
                .map(|(frame, _, _)| *frame)
                .collect();
            self.latch(
                reason,
                format!("{what} {frames:?}; last frame sent {last_sent}"),
                now,
                None,
                reported,
            )
        } else {
            None
        }
    }

    /// `keyframe` is the one this evidence is about, if any. A latch about the
    /// stream's start, with nothing else seen, is provisional. `reported`:
    /// the client saw the loss, not only the host's socket; any loss the
    /// client reported within [`LOSS_WINDOW`] counts as well.
    pub(super) fn latch(
        &mut self,
        reason: &'static str,
        detail: String,
        now: Instant,
        keyframe: Option<u32>,
        reported: bool,
    ) -> Option<Latched> {
        if !self.enabled {
            return None;
        }
        let reported =
            reported
                || self.recent_losses.iter().flatten().any(|(_, at, client)| {
                    *client && now.saturating_duration_since(*at) < LOSS_WINDOW
                });
        if self.observed_at.is_some() {
            let kind = self.kind;
            if !self.provisional() {
                // A latch only the host's socket made, after a lift or on
                // top of an inherited one, holds for the stream; the client's
                // report is what lets the next session remember it.
                if self.sustained_at.is_none() && reported {
                    self.sustained_at = Some(now);
                    return Some(Latched {
                        reason,
                        detail,
                        kind: LatchKind::Sustained,
                        confirmed: true,
                        from: kind,
                        host_only: false,
                    });
                }
                return None;
            }
            // A provisional latch learns, but not from another report about
            // the keyframe it was made for.
            if (keyframe.is_some() && keyframe == self.latch_keyframe)
                || keyframe.is_some_and(|frame| !self.after_latch(frame))
            {
                return None;
            }
            if !reported {
                // Host-only evidence never confirms; it restarts the clean 10 s.
                self.last_loss_at = Some(now);
                return None;
            }
            self.kind = Some(LatchKind::Sustained);
            self.sustained_at = Some(now);
            self.latch_keyframe = None;
            return Some(Latched {
                reason,
                detail,
                kind: LatchKind::Sustained,
                confirmed: true,
                from: kind,
                host_only: false,
            });
        }
        if !reported {
            // Only the host's socket refused video: a transmit stall on the
            // host is not the route. Provisional, unless the stream already
            // lifted one or inherited a latch; never remembered either way.
            let kind = if self.lifted || self.latched.is_some() {
                LatchKind::Sustained
            } else {
                LatchKind::Send
            };
            self.observed_at = Some(now);
            self.latched = Some(reason);
            self.kind = Some(kind);
            self.ever_latched = true;
            self.frames_since_latch = 0;
            self.latch_keyframe = keyframe.filter(|_| kind.provisional());
            self.latch_frame = self.last_sent;
            self.last_loss_at = None;
            return Some(Latched {
                reason,
                detail,
                kind,
                confirmed: false,
                from: None,
                host_only: true,
            });
        }
        let evidence = self
            .recent_losses
            .iter()
            .flatten()
            .any(|(_, at, _)| now.saturating_duration_since(*at) < LOSS_WINDOW);
        // An inherited latch, a second latch and evidence beyond the first
        // keyframe all say the route is slow, not that the start was rough.
        let provisional = self.latched.is_none()
            && !self.lifted
            && !evidence
            && keyframe.is_some_and(|frame| self.startup_keyframe(frame));
        let kind = if provisional {
            LatchKind::Startup
        } else {
            LatchKind::Sustained
        };
        self.observed_at = Some(now);
        self.latched = Some(reason);
        self.kind = Some(kind);
        self.ever_latched = true;
        self.frames_since_latch = 0;
        self.latch_keyframe = keyframe.filter(|_| provisional);
        self.latch_frame = self.last_sent;
        self.last_loss_at = None;
        if !provisional {
            self.sustained_at = Some(now);
        }
        Some(Latched {
            reason,
            detail,
            kind,
            confirmed: false,
            from: None,
            host_only: false,
        })
    }
}
