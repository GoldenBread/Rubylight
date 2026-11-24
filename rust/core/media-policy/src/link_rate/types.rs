//! Constants, public state/change/evidence types, and internal event/fence records for link-rate adaptation.

use std::time::{Duration, Instant};

#[allow(unused_imports)]
use super::LinkRate;

/// Every evidence test runs over this window.
pub const EVIDENCE_WINDOW: Duration = Duration::from_secs(2);
/// Evidence is evaluated at most this often.
pub const EVALUATE_EVERY: Duration = Duration::from_millis(100);
/// Distinct unrecoverable frames in the window that cut the bitrate (E1).
/// Matches the loss guard's "3 unrecoverable frames in 2 s" (Oct 17).
pub const UNRECOVERABLE_FRAMES: usize = 3;
/// Share of the packets the host sent that the FEC reports show missing in
/// blocks the client could not rebuild, and that cuts (E2). Blocks FEC rebuilt
/// count their missing data packets only while the RTT is elevated: the report
/// is made when enough packets are in, so what is still in flight reads as
/// missing (14 % on the 2026-10-24 Wi-Fi arm, 0.5 % real loss, RTT flat), but
/// with a queue filling the same share is real loss FEC is absorbing. The
/// share is over all packets sent, not only those of reported frames.
pub const LOSS_SHARE_PERCENT: u64 = 5;
/// E2 needs the share in this many consecutive windows of [`LOSS_WINDOW`].
pub const LOSS_WINDOWS: u8 = 2;
pub const LOSS_WINDOW: Duration = Duration::from_millis(500);
/// A window in which the host sent fewer packets than this says nothing: a
/// quiet stream loses 5 % to a single packet.
pub const LOSS_MIN_PACKETS: u64 = 100;
/// After a fast-to-slow pacing latch has had its fence, this long a single
/// event is enough (E1 is otherwise [`UNRECOVERABLE_FRAMES`]) if it proves the
/// latch failed: a lost keyframe, or a lost frame or keyframe request while no
/// keyframe has come through whole since the latch. Once one has, a lone event
/// is not proof: the latch can be provisional (start-up) and almost every IDR
/// request is counted as a network request. A client waiting for a keyframe reports little (one
/// lost keyframe per request), so three frames took 4 s on the 2026-10-24
/// `r50-hevc-hi` arm (75 Mbps into 50 Mbit/s) and the stream lost its first
/// 5.8 s. A cut with no measured share goes to [`MIN_CUT_PERCENT`] then.
///
/// The events of the watch count whether or not they fall inside the fence
/// (the fence only keeps them out of the cut's evidence): two of them at least
/// [`WATCH_EVENT_GAP`] apart show the latch failed, and the next admitted event
/// cuts. A keyframe sent inside the fence that nothing was reported about
/// may be one that never got through (the same 2026-10-24 arm, where the
/// client asked again 100 ms and 1.3 s after the latch and lost 321 frames).
pub const LATCH_WATCH: Duration = Duration::from_secs(4);
/// Events of a latch watch closer together than this are one burst.
pub const WATCH_EVENT_GAP: Duration = Duration::from_millis(100);
/// Two events fail a latch only when they come within this span: a latch that
/// fixed the loss and a stray frame lost every couple of seconds is not a
/// failed latch (the r50 arm's requests came 1.2 s apart).
pub const WATCH_EVENT_SPAN: Duration = Duration::from_millis(1_500);
/// Frames lost on send (WSAENOBUFS and the like) that cut (E3).
pub const SEND_LOSS_FRAMES: usize = 3;
/// Network-classified keyframe requests that cut (E4): from a client that
/// sends no FEC status, or from any client while a probe step is unconfirmed.
/// A storm of recovery keyframes after a probe (38 in 3.5 s on r50-hevc-hi)
/// is not reported as unrecoverable frames, so the requests are the evidence.
pub const KEYFRAME_REQUESTS: usize = 3;
/// Reference invalidations that continue the frame range of the incident
/// counted last, within this time of its first request, are the same
/// incident (see [`LinkRate::on_invalidation`]). The r-def client asked
/// eleven times in about 90 ms; one that is still asking after this long has
/// not got its repair, and that counts again, so three counts in
/// [`EVIDENCE_WINDOW`] take a client stuck for one second.
pub const INCIDENT_GAP: Duration = Duration::from_millis(500);
/// The loss share a cut will believe. Losses come in bursts, and the share is
/// measured up to the newest lost frame, so a short window can read 100% on a
/// link that carries half the rate; a cut never goes below a quarter of what
/// was sent.
pub(super) const MAX_LOSS_SHARE: f64 = 0.75;
/// Rebuilt blocks the running share of reordered data packets averages over.
pub(super) const REORDER_BLOCKS: f64 = 64.;

/// The running reordering share fades toward zero with this time constant
/// while no flat-RTT block renews it, so a spell of heavy loss in the past does
/// not hide a queue now.
pub(super) const REORDER_FADE: Duration = Duration::from_secs(20);

/// A silence this long between rebuilt blocks fades the baseline even while
/// the RTT is up.
pub(super) const REORDER_GAP: Duration = Duration::from_secs(1);

/// What reordering produces at most, over the block's parity share: a block is
/// reported when `data` packets are in, so what is still on the way is at most
/// the parity (0.18 at 20 % FEC, 0.24 at 30 %). The running baseline is held to
/// the block's parity share plus this, so a flat-RTT share of loss does not
/// raise it past what a path can reorder. Each block is averaged unclipped.
pub(super) const REORDER_MARGIN: f64 = 0.02;

/// How far below the baseline the excess of rebuilt data loss may sink, in
/// packets, so that a run of clean blocks pays for the next noisy ones.
pub(super) const EXCESS_CREDIT_CAP: i64 = 64;

/// A cut aims at this share of the delivered rate, FEC included.
pub const DELIVERED_PERCENT: u64 = 85;
/// One cut lowers the target by at least 15 % ...
pub const MAX_CUT_PERCENT: u64 = 85;
/// ... and a first cut at most by half, so one late or inflated estimate
/// cannot collapse the stream. A further cut inside [`EVIDENCE_WINDOW`] is
/// not held to half (see [`REPEAT_CUT_FLOOR_BPS`]).
pub const MIN_CUT_PERCENT: u64 = 50;
/// A further cut within [`EVIDENCE_WINDOW`] of the last one is not held to
/// that limit, only to this many wire bits per second.
pub const REPEAT_CUT_FLOOR_BPS: u64 = 1_000_000;
/// A still or calm picture needs a fraction of its bitrate, so the rate is not
/// what the path failed to carry, and a cut would only meet the picture when
/// it moves again (after three frames lost to one Wi-Fi gap in a menu, half
/// the bitrate and a climb of 12 % a step). A cut that would land at or above
/// what the stream sent, on average and in the largest frame the loss can have
/// come from ([`LARGE_FRAME_LEAD`]), is held: not applied, only remembered for
/// this long as the rate the next cut starts from. Loss that goes on halves
/// that rate with each round of evidence until a cut lowers what is sent, and
/// that one is made; from there cuts go as deep as they did. What a held cut
/// leaves as it was: the pace and the size limit of the keyframe that repairs
/// the loss, which a made cut would have halved. That keyframe does not count
/// as the large frame, rebuilt by error correction or not yet reported,
/// unless it is lost too: with that report in before the cut is weighed the
/// cut is made; later it is behind the fence the hold sets, and keyframes
/// that fail after the fence are lost frames like any other, large ones, so
/// the cut is made then. Not held: with a queue standing in the path, the
/// PC's own refused sends, a failed pacing latch, a failing probe step, and
/// PyroWave, whose pictures shrink with every cut.
pub const HELD_FOR: Duration = Duration::from_secs(10);
/// How many frames before the first one with loss evidence may be the large
/// frame the loss came from: a frame of several times the usual size takes
/// that many frame times to leave a link at the stream's rate, and the frames
/// behind it are dropped from the queue it filled.
pub(super) const LARGE_FRAME_LEAD: u32 = 6;
/// Target after a cut when the loss share is unknown (E4 alone).
pub const UNKNOWN_SHARE_PERCENT: u64 = 70;
/// After a change, evidence from frames sent within this long is ignored so
/// the bottleneck queue can drain; the longer of this and
/// `2 x RTT + FENCE_MARGIN` applies.
pub const MIN_FENCE: Duration = Duration::from_millis(500);
pub const FENCE_MARGIN: Duration = Duration::from_millis(250);
/// Frames in the encoder when the bitrate changes (Oct 7: "two frames in the
/// encoder"); the fence frame is this far past the next frame sent.
pub const PIPELINE_FRAMES: u32 = 2;
/// After a cut, frames sent past the fence that run more than this share above
/// the target mean the encoder cannot go that low (H.264 on AMD stops at about
/// 5 Mbps at 1080p60 however low it is asked, 14 Mbps on noise): cutting again
/// only collapses the target while the stream stays the same.
pub const ENCODER_OVERSHOOT_PERCENT: u64 = 120;
/// Wait after a step down before the first probe.
pub const HOLD: Duration = Duration::from_secs(2);
/// A second failure at the same edge (see [`LinkRate::learn`]) waits at least
/// this long before the next retry past the learned ceiling, and each further
/// one doubles the wait up to [`RETRY_MAX`]. A fixed edge then pays rarer and
/// rarer small retries instead of a climb and a collapse every few seconds,
/// while a path that improves is found again by the same retries. A first
/// failure at a new edge retries after [`HOLD`], as before: one bad moment is
/// not a fixed limit.
pub const RETRY_FIRST: Duration = Duration::from_secs(10);
/// Each failure at the same edge doubles the wait before the next retry, up
/// to this: a link whose capacity stays put is still checked every five
/// minutes, and one whose capacity rises is found within that time.
pub const RETRY_MAX: Duration = Duration::from_secs(300);
/// Two failures within this share of each other are the same edge.
pub const EDGE_PERCENT: u64 = 10;
/// The longest a step waits to confirm on a path with a deep queue (see
/// [`LinkRate::confirm_window`]).
pub const CONFIRM_MAX: Duration = Duration::from_secs(10);
/// A queue deeper than this counts as this deep for the confirm window.
pub(super) const QUEUE_PEAK_MAX: Duration = Duration::from_secs(1);
/// After [`SEND_LOSS_FRAMES`] frames the host's socket refused within
/// [`EVIDENCE_WINDOW`], no probe goes up for this long: a busy PC keeps the
/// socket full for tens of milliseconds at a time, and raising the bitrate
/// into it only makes the next refusals worse. The rate still steps down on
/// further refusals.
pub const HOST_LOSS_HOLD: Duration = Duration::from_secs(5);
/// A failed probe doubles the wait up to this. The link may have recovered
/// since, and only a probe can tell, so the wait stays short; a repeat probe is
/// cheap because it goes only [`NEAR_STEP_DIVISOR`] past the bad rate.
pub const MAX_HOLD: Duration = Duration::from_secs(8);
/// A probe that fails within [`KNEE_PERCENT`] of the previous bad rate means
/// the link has a fixed knee there. Each such failure doubles a wait, up to
/// [`KNEE_MAX_HOLD`], during which probes stay under [`KNEE_CAP_PERCENT`] of
/// the bad rate. Without it a fixed link pays a failed probe every 9 s forever:
/// on 45 fixed-link runs of 5 minutes (225 on 150, 120 on 80, 40 on 20, 20 on
/// 20 and 179 on 138 Mbit/s, three feedback delays, three burst/queue models)
/// 4.7 failed probes a minute and 1.00 % of frames lost fell to 1.4 a minute
/// and 0.77 % (0.76 % is the periodic 3x frame). 16 s left 2.5 a minute, 60 s
/// 0.9, but a 60 s wait keeps a recovered link at 90 % of its old rate that
/// long; 30 s is the middle.
pub const KNEE_MAX_HOLD: Duration = Duration::from_secs(30);
/// A failed probe keeps probes under the knee this long, at least; so does a
/// cut, but only one made with the RTT elevated (a queue standing in the
/// path). Such a cut from a rate the stream climbed to goes only
/// [`KNEE_BACKOFF_PERCENT`] of it when it has no estimate. r50-hevc-hi (50 Mbit/s, 50 ms queue, 59 Mbps asked) cut to
/// 70 % from every standing queue and climbed back into the next one, a
/// sawtooth with a freeze at each tooth.
pub const KNEE_QUIET: Duration = Duration::from_secs(20);
pub const KNEE_BACKOFF_PERCENT: u64 = 85;
pub const KNEE_PERCENT: u64 = 5;
/// A standing queue: the RTT stays this far above the lowest of the last
/// [`QUEUE_FLOOR_WINDOW`] for [`QUEUE_HOLD`], in every sample, while video is
/// busy and no loss signal fired. The 2026-10-24 r50-hevc-hi run held 36.8 Mbps
/// into 50 Mbit/s with a 25 ms queue and no loss at all (p50 48 ms), where
/// nothing else could cut. The wifi arm's jitter reaches 8 ms, and its RTT
/// excursions (20 to 60 ms for 1.5 s) are bursty, so a dip below the margin
/// starts the wait over.
pub const QUEUE_MARGIN: Duration = Duration::from_millis(15);
pub const QUEUE_HOLD: Duration = Duration::from_secs(2);
/// The floor is the lowest RTT of this long, in one-second buckets, so a
/// route that got slower for good is the floor after it.
pub const QUEUE_FLOOR_WINDOW: Duration = Duration::from_secs(30);
/// The queue has to start within this long of a probe step.
pub const QUEUE_RAISED: Duration = Duration::from_secs(8);
/// A queue cut waits this long for the queue to drain before it looks again.
pub const QUEUE_SETTLE: Duration = Duration::from_secs(1);
/// A queue cut goes down by 12 %, not deep: the stream is near the link's
/// edge, and the next step is a few seconds away if the RTT stays up.
pub const QUEUE_CUT_PERCENT: u64 = 88;
/// A queue that a cut did not change (the excess stays within a quarter or
/// 5 ms) is no queue: a slower route or another flow. The floor is re-learned
/// there and the stream is not cut again for it.
pub(super) const QUEUE_FLAT_MS: u64 = 5;
/// A queue that three cuts (about a third of the rate) did not drain is
/// another flow's: further cuts would only take the stream's own share.
pub(super) const QUEUE_STEPS_MAX: u8 = 3;
pub(super) const FLOOR_BUCKETS: usize = 32;
/// A step down from within this share of the bad rate is a knee failure too.
pub const KNEE_CUT_PERCENT: u64 = 90;
/// 95 % left the stream at the link's edge, where each periodic big frame
/// cost a cut (a window at 62 % of the link); 90 % never fell below 76 %.
pub const KNEE_CAP_PERCENT: u64 = 90;
/// Minimum time between probes (also at least the fence).
pub const PROBE_EVERY: Duration = Duration::from_millis(500);
/// Probe step as a fraction of the current rate: 1/8. A 25 % step loses more
/// than 20 % FEC can repair (2.2.19).
pub const PROBE_STEP_DIVISOR: u64 = 8;
/// Step within [`NEAR_BAD_PERCENT`] of the last bad rate, one step at a time
/// and confirmed before the next.
pub const NEAR_STEP_DIVISOR: u64 = 20;
pub const NEAR_BAD_PERCENT: u64 = 90;
/// A probe step counts as good after this long without evidence.
pub const CONFIRM_AFTER: Duration = EVIDENCE_WINDOW;
/// `bad` is forgotten after this long clean at 95 % of it or more.
pub const FORGET_BAD_AFTER: Duration = Duration::from_secs(60);
/// `Full` again after this long clean at the ceiling.
pub const FULL_AFTER: Duration = Duration::from_secs(10);
/// A control ping this recent counts as fresh feedback.
pub(super) const PING_FRESH: Duration = Duration::from_secs(2);
pub(super) const RING: usize = 1024;
pub(super) const EVENT_CAP: usize = 32;
pub(super) const PENDING_CAP: usize = 4;

/// A confirmed probe step, kept for one evidence window: loss that FEC
/// absorbs for two seconds shows up just after the step was confirmed.
#[derive(Clone, Copy)]
pub(super) struct Confirmed {
    /// The good rate before the step.
    pub(super) before: u64,
    /// The lowest rate confirmed.
    pub(super) rate: u64,
    pub(super) at: Instant,
    /// How long after `at` a failure is still this step's: the evidence
    /// window, or the step's own confirm window if that is longer.
    pub(super) window: Duration,
    /// The step went past the learned ceiling.
    pub(super) retry: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Disabled, or the client owns the bitrate.
    Off,
    /// At the requested bitrate. `tick` returns `None`.
    Full,
    /// Below the requested bitrate because of the network, probing back up.
    Limited,
    /// At the minimum bitrate and still losing.
    Floor,
}
impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Off => "off",
            State::Full => "full",
            State::Limited => "limited",
            State::Floor => "floor",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    StepDown,
    Probe,
    ProbeFailed,
    Floor,
    /// The encoder keeps sending far more than the target; the rate it sends
    /// is now the lowest this controller asks for.
    EncoderFloor,
    Full,
}

/// The evidence behind a step down, for the log and the session warning.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Evidence {
    pub unrecoverable_frames: usize,
    pub send_losses: usize,
    pub keyframe_requests: usize,
    /// Consecutive 500 ms windows at or over [`LOSS_SHARE_PERCENT`].
    pub loss_windows: u8,
    /// Missing packets over packets sent, 0..1, since the last change.
    pub loss_share: f64,
}

/// One change of state or bitrate, taken with [`LinkRate::take_change`] and
/// logged by the host.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Change {
    pub kind: ChangeKind,
    /// `unrecoverable_frames`, `fec_loss_share`, `send_losses` (the PC's own
    /// socket refused frames, the RTT flat), `send_losses_queued` (refusals
    /// with a queue in the path: the network's), `keyframe_requests`,
    /// `probe`, `recovered`.
    pub reason: &'static str,
    pub from_kbps: u32,
    pub to_kbps: u32,
    /// Video wire rate the link delivered, kbps (0 when unknown).
    pub delivered_kbps: u32,
    pub evidence: Evidence,
}

#[derive(Clone, Copy, Default)]
pub(super) struct Sent {
    pub(super) frame: u32,
    pub(super) packets: u32,
    pub(super) bytes: u32,
    /// Packets the client reported missing (FEC status).
    pub(super) missing: u32,
    pub(super) at: Option<Instant>,
}

/// Evidence counts only for frames sent at or after `frame` and at or after
/// `at`.
#[derive(Clone, Copy)]
pub(super) struct Fence {
    pub(super) frame: Option<u32>,
    pub(super) at: Instant,
}
impl Fence {
    pub(super) fn admits(&self, frame: Option<u32>, sent: Instant) -> bool {
        sent >= self.at
            && match (self.frame, frame) {
                (Some(fence), Some(frame)) => frame.wrapping_sub(fence) < 1 << 31,
                _ => true,
            }
    }
}

#[derive(Clone, Copy)]
pub(super) struct WatchedKeyframe {
    pub(super) frame: u32,
    pub(super) at: Instant,
    pub(super) failed: bool,
}

#[derive(Default)]
pub(super) struct Events {
    pub(super) unrecoverable: Vec<(Instant, u32)>,
    pub(super) send_losses: Vec<(Instant, u32)>,
    /// Frames the host's socket refused (not those too large for the path),
    /// fence or not: they decide [`LinkRate::host_limited`].
    pub(super) refused: Vec<(Instant, u32)>,
    pub(super) requests: Vec<Instant>,
    pub(super) window: Option<Instant>,
    /// Packets reported missing and packets sent in the current E2 window.
    pub(super) missing: u64,
    pub(super) sent: u64,
    pub(super) hot: u8,
    /// The oldest frame with any loss evidence; the delivered rate is
    /// measured from there, not diluted by the clean frames before the drop.
    pub(super) first_loss: Option<u32>,
    /// The newest frame with loss evidence. Frames after it may simply not
    /// have been reported yet (feedback lag), so the loss share is measured
    /// only up to here.
    pub(super) last_loss: Option<u32>,
    /// During a latch watch: a keyframe the client could not rebuild, or a
    /// keyframe request made while no keyframe had arrived whole since the
    /// latch. Either shows the latch did not fix the loss.
    pub(super) latch_failed: bool,
}
impl Events {
    pub(super) fn clear(&mut self) {
        self.unrecoverable.clear();
        self.send_losses.clear();
        self.requests.clear();
        self.window = None;
        self.missing = 0;
        self.sent = 0;
        self.hot = 0;
        self.first_loss = None;
        self.last_loss = None;
        self.latch_failed = false;
    }
    pub(super) fn lost(&mut self, frame: u32) {
        if self
            .last_loss
            .is_none_or(|last| frame.wrapping_sub(last) < 1 << 31)
        {
            self.last_loss = Some(frame);
        }
        if self
            .first_loss
            .is_none_or(|first| first.wrapping_sub(frame) < 1 << 31)
        {
            self.first_loss = Some(frame);
        }
    }
    pub(super) fn distinct(events: &mut Vec<(Instant, u32)>, now: Instant, frame: u32) {
        if events.iter().any(|(_, seen)| *seen == frame) {
            return;
        }
        if events.len() >= EVENT_CAP {
            events.remove(0);
        }
        events.push((now, frame));
    }
    pub(super) fn prune(&mut self, now: Instant) {
        let recent = |at: &Instant| now.saturating_duration_since(*at) < EVIDENCE_WINDOW;
        self.unrecoverable.retain(|(at, _)| recent(at));
        self.send_losses.retain(|(at, _)| recent(at));
        self.refused.retain(|(at, _)| recent(at));
        self.requests.retain(recent);
    }
    /// Closes the finished 500 ms windows of reported loss.
    pub(super) fn roll(&mut self, now: Instant) {
        let Some(start) = self.window else {
            self.window = Some(now);
            self.missing = 0;
            self.sent = 0;
            return;
        };
        if now.saturating_duration_since(start) < LOSS_WINDOW {
            return;
        }
        let hot =
            self.sent >= LOSS_MIN_PACKETS && self.missing * 100 >= self.sent * LOSS_SHARE_PERCENT;
        self.hot = if hot { self.hot.saturating_add(1) } else { 0 };
        self.missing = 0;
        self.sent = 0;
        self.window = Some(now);
    }
}

#[derive(Clone, Copy)]
pub(super) struct Step {
    pub(super) rate: u64,
    pub(super) at: Instant,
    /// How long it must stay clean ([`LinkRate::confirm_window`]).
    pub(super) confirm: Duration,
    /// The RTT when it was sent: on a path that has shown a queue, a step
    /// confirms only while the RTT stays within [`QUEUE_MARGIN`] of this.
    pub(super) rtt: Duration,
    /// It went past the learned ceiling.
    pub(super) retry: bool,
}

pub(super) struct Trigger {
    pub(super) reason: &'static str,
    pub(super) share_known: bool,
    /// A watched latch is known not to have fixed the loss.
    pub(super) latch_failed: bool,
    pub(super) evidence: Evidence,
}
