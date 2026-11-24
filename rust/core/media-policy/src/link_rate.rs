//! Lowers the encoder bitrate when the network cannot carry the stream.
//!
//! A pure state machine: the session feeds it what the host sent and what the
//! client reports back, a periodic `tick` returns the encoder bitrate to use,
//! and every instant is passed in, so tests are deterministic.
//!
//! It acts only on loss that FEC cannot absorb (see the `*_FRAMES`, `LOSS_*`
//! and `KEYFRAME_REQUESTS` constants). On a healthy link it never returns a
//! target and never touches pacing, so the stream takes exactly the path it
//! takes without it. The one delay signal, a standing queue (see
//! [`QUEUE_MARGIN`]), is a lossless queue held for seconds, which no loss
//! evidence can show. After a cut it ignores reports
//! for frames sent before the cut took effect (the [`Fence`]); the opt-in
//! `adaptive_bitrate` controller lacks this, and on the 2026-10-23 link-speed
//! runs it cut once a second (179 to 24.7 Mbps in 9 s on a link that carries
//! about 115) because late reports kept counting against the lower bitrate.
//!
//! PyroWave streams get it too: nothing else on the host reacts to their loss
//! (the loss guard and Wi-Fi pacing leave PyroWave alone), and on 2026-10-29 a
//! 200 Mbps stream to a phone on 5 GHz Wi-Fi lost about a quarter of its
//! packets for the whole stream. Its rate control holds every picture to one
//! frame's budget, so a cut shrinks every picture; the pace shrinks with it only
//! where it follows the stream's demand, not a client pace hint,
//! a fixed `packet_pacing` rate or a wired client link. With record framing only a
//! lost coarse block loses the frame ([`LinkRate::frame_lost`]). PyroWave stops
//! at its test-scene detail level, and its keyframe requests are no evidence:
//! every picture is intra, so the session passes none of them on, and a
//! PyroWave client is judged by its FEC status reports alone.
//!
//! A path that cannot carry the stream loses at a rate near its edge, and the
//! plain "climb back to the requested rate" probe found that edge again and
//! again: a mesh node's wireless backhaul, or any hop the host cannot see,
//! repeated a cut every ten to fifty seconds (2026-11-17: a 4K120 PyroWave
//! client on a path good for about 600 Mbps asked for 800 and collapsed from
//! 641 to 676 Mbps into 116 to 279 ms of round trip). The controller now
//! keeps a learned ceiling: the rate a cut or a failed step landed on, which
//! probes climb back to and stop at. One step past it (a retry) goes
//! [`NEAR_STEP_DIVISOR`] up, alone, after [`HOLD`] for a new edge and after
//! [`RETRY_FIRST`], doubling to [`RETRY_MAX`], for a failure at the same edge
//! (within [`EDGE_PERCENT`], or a retry that failed, or a cut soon after a
//! retry confirmed). On a path that has shown a queue, a step confirms only
//! after the time its overshoot would take to fill that queue
//! ([`LinkRate::confirm_window`]) and while the RTT has not risen. A retry
//! that confirms well under the failed rate lifts the ceiling, so a dip that
//! passes is climbed out of at the usual pace; a link that never congests
//! never learns one and takes exactly the path it took before.
//!
//! The host's own socket refusing frames, with no loss reported and the RTT
//! flat, is not the path's limit: it stops probes for [`HOST_LOSS_HOLD`] and
//! moves no learned ceiling. Refusals that come with client loss or a queue
//! (a switch pausing the host's adapter behind a full uplink) are the path's.
//!
//! A still or calm picture needs a fraction of the stream's bitrate, and loss
//! during it is not the rate's doing: a cut that would land at or above what
//! the stream sent, in the largest frame around the loss too, is held back and
//! only remembered ([`HELD_FOR`]), so the bitrate is whole when the picture moves
//! again, and loss that goes on still reaches the cut that lowers what is
//! sent.
//!
//! Inactive (`State::Off`) when disabled in the configuration and once the
//! client sets the bitrate itself.
//!
//! # Calling it (brief 3)
//!
//! - One `Mutex<LinkRate>` per session, shared by the video send path, the
//!   control thread that reads client reports and the stream tick. Every
//!   method takes `&mut self` and does bounded work with no allocation after
//!   `new`.
//! - [`LinkRate::on_sent`] comes once the host has sent the frame, with the
//!   wire frame number the client will report and the packets (data and
//!   parity) and wire bytes actually sent. The host cannot say it earlier: a
//!   frame sent in slices has no size until its last slice, and a frame the
//!   host abandons part way counts only what left. It is stamped with the
//!   time the host recorded it. A report that arrives before `on_sent` for
//!   its frame (the first blocks of a large frame, on a fast link) has
//!   nothing to count against and is dropped: loss is under-counted, never
//!   over-counted.
//! - Feed [`LinkRate::on_status`] only deduplicated FEC blocks (the same block
//!   repeated in several reports once); a frame with several blocks is
//!   counted once for E1, and its packets once for E2.
//! - From one client report call `on_status` first, then `on_unrecoverable`
//!   for the same frame, then `on_latch` (a fast-rate latch is a reaction to
//!   the same loss and starts the fence).
//! - Call `tick` from the stream tick with the current RTT, the FEC percentage
//!   in use, the frame period, the encoder bitrate now in effect and whether
//!   the picture is moving (`busy`: a static desktop sends almost nothing, so a
//!   probe sent then proves nothing). A returned kbps is applied through the
//!   encoder bitrate path; `None` means the requested rate. Log the result of
//!   `take_change` after each call.
//! - The wire frame counter must not restart under one controller. Use
//!   [`LinkRate::rebuild`] when the stream is reconfigured (new bitrate, FEC,
//!   mode or a restarted frame counter): it keeps a client takeover and
//!   forgets the rest.
//!
//! Units: the controller works in total wire bits per second (video with FEC
//! and framing plus the audio reserve), the unit of
//! [`WireBudget::total_bps`], and converts to encoder kbps at the current FEC
//! percentage with [`WireBudget::encoder_kbps`].
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use crate::{adaptive_bitrate::WireBudget, rtsp::Negotiated};
use rubylight_wire::fec_status::Status;

mod cut;
mod ingest;
mod probe;
mod signal;
mod types;

#[cfg(test)]
mod sim;
#[cfg(test)]
mod tests;

#[cfg(test)]
use types::LARGE_FRAME_LEAD;
pub use types::{
    CONFIRM_AFTER, CONFIRM_MAX, Change, ChangeKind, DELIVERED_PERCENT, EDGE_PERCENT,
    ENCODER_OVERSHOOT_PERCENT, EVALUATE_EVERY, EVIDENCE_WINDOW, Evidence, FENCE_MARGIN,
    FORGET_BAD_AFTER, FULL_AFTER, HELD_FOR, HOLD, HOST_LOSS_HOLD, INCIDENT_GAP, KEYFRAME_REQUESTS,
    KNEE_BACKOFF_PERCENT, KNEE_CAP_PERCENT, KNEE_CUT_PERCENT, KNEE_MAX_HOLD, KNEE_PERCENT,
    KNEE_QUIET, LATCH_WATCH, LOSS_MIN_PACKETS, LOSS_SHARE_PERCENT, LOSS_WINDOW, LOSS_WINDOWS,
    MAX_CUT_PERCENT, MAX_HOLD, MIN_CUT_PERCENT, MIN_FENCE, NEAR_BAD_PERCENT, NEAR_STEP_DIVISOR,
    PIPELINE_FRAMES, PROBE_EVERY, PROBE_STEP_DIVISOR, QUEUE_CUT_PERCENT, QUEUE_FLOOR_WINDOW,
    QUEUE_HOLD, QUEUE_MARGIN, QUEUE_RAISED, QUEUE_SETTLE, REPEAT_CUT_FLOOR_BPS, RETRY_FIRST,
    RETRY_MAX, SEND_LOSS_FRAMES, State, UNKNOWN_SHARE_PERCENT, UNRECOVERABLE_FRAMES,
    WATCH_EVENT_GAP, WATCH_EVENT_SPAN,
};
use types::{
    Confirmed, EVENT_CAP, Events, FLOOR_BUCKETS, Fence, PENDING_CAP, RING, Sent, Step,
    WatchedKeyframe,
};

pub struct LinkRate {
    budget: Option<WireBudget>,
    reason: &'static str,
    client_owned: bool,
    state: State,
    ceiling_kbps: u32,
    floor_kbps: u32,
    /// Total wire bits per second the encoder is held to.
    current: u64,
    /// The last rate that stayed clean.
    good: u64,
    /// The last rate that did not.
    bad: Option<u64>,
    /// The learned ceiling, total wire bits per second: where the last cut
    /// or failed step landed, raised only by a retry that confirmed. Probes
    /// climb to it and stop (see [`Self::learn`]).
    learned: Option<u64>,
    /// When the next retry past `learned` may go.
    next_retry: Option<Instant>,
    /// The wait before that retry; doubles after each failure at the same
    /// edge, and stays across a return to `Full`.
    retry_wait: Duration,
    /// The rate the last failure was at, to tell the same edge.
    failed_at: Option<u64>,
    /// When the host's socket last refused [`SEND_LOSS_FRAMES`] frames
    /// within the evidence window (see [`Self::host_limited`]).
    host_loss_at: Option<Instant>,
    /// The deepest queue (RTT over its floor) seen when a network cut or a
    /// failed step was made: how long an overshoot can hide on this path.
    queue_peak: Duration,
    /// Total wire bits per second the encoder was seen to send after a cut
    /// asked for less (see [`ENCODER_OVERSHOOT_PERCENT`]); cuts stop here.
    encoder_floor: Option<u64>,
    /// Evidence rounds in a row whose frames ran above the target.
    overshoots: u8,
    /// Probe steps above `good` not yet confirmed clean, oldest first.
    pending: [Option<Step>; PENDING_CAP],
    confirmed: Option<Confirmed>,
    hold: Duration,
    /// Knee wait, doubled by failures at the bad rate, reset past it.
    knee: Duration,
    /// Probes stay under [`KNEE_CAP_PERCENT`] of `bad` until then.
    knee_until: Option<Instant>,
    next_probe: Option<Instant>,
    at_ceiling: Option<Instant>,
    near_bad: Option<Instant>,
    fence: Option<Fence>,
    /// Until when one unrecoverable frame cuts (see [`LATCH_WATCH`]).
    latch_watch: Option<Instant>,
    /// When the latch being watched happened: a keyframe sent from then on
    /// answers it, even one sent inside the fence.
    latch_at: Option<Instant>,
    /// Keyframes sent since the latch of the latch being watched, with whether
    /// the client reported each unrecoverable.
    watch_keyframes: Vec<WatchedKeyframe>,
    /// Unrecoverable frames (with their number) and network keyframe requests
    /// since the latch being watched, inside the fence or not.
    watch_events: Vec<(Instant, Option<u32>)>,
    /// Rebuilt blocks seen, their packets and those of them reported missing
    /// before the block was rebuilt (data only), for the snapshot: on a path
    /// that reorders, nearly every block is "missing" its trailing packets.
    rebuilt_blocks: u64,
    rebuilt_packets: u64,
    rebuilt_data_missing: u64,
    /// The last tick saw a static picture: next to nothing was sent.
    static_picture: bool,
    /// Running share of a rebuilt block's data packets reported missing while
    /// the RTT is flat (what reordering looks like on this path), and the
    /// excess over it seen while the RTT is up, in packets (see
    /// [`Self::on_status`]).
    reorder_share: f64,
    reorder_at: Option<Instant>,
    excess_credit: i64,
    excess_high: i64,
    last_sent: Option<u32>,
    ring: Box<[Sent]>,
    events: Events,
    fec_status_seen: bool,
    /// PyroWave record framing: only the coarse block (block 0) carries the
    /// picture, so only its loss loses the frame (see [`Self::frame_lost`]).
    records: bool,
    /// PyroWave whose test-scene detail level is at or above the
    /// stream's own bitrate: cuts stop at half the bitrate instead, and the
    /// floor warning says this link loses PyroWave (see
    /// [`Self::pyrowave_no_room`]).
    pyrowave_no_room: bool,
    /// The newest frame of the reference invalidation incident counted last,
    /// and when it began: the client's repeats for the same loss are one
    /// request (see [`Self::on_invalidation`]).
    incident: Option<(u64, Instant)>,
    last_ping: Option<Instant>,
    minimum_rtt: Option<Duration>,
    /// Lowest RTT per second over [`QUEUE_FLOOR_WINDOW`], oldest first.
    rtt_floor: VecDeque<(Instant, Duration)>,
    /// Since when every RTT sample was over the floor by [`QUEUE_MARGIN`].
    queue_since: Option<Instant>,
    /// A probe raised the rate at this time.
    raised_at: Option<Instant>,
    /// A loss cut was made at this time: a queue that stands on after it
    /// (a deep buffer the cut did not drain; 2026-11-17 sim: 400 ms at 50
    /// Mbit/s) is the stream's own, as one after a probe step is.
    loss_cut_at: Option<Instant>,
    queue_quiet_until: Option<Instant>,
    /// The excess over the floor when the last queue cut was made, and the
    /// cuts of this queue so far.
    queue_excess: Option<Duration>,
    queue_steps: u8,
    rtt: Duration,
    next_eval: Option<Instant>,
    cache: Option<((u64, usize, Duration), u32)>,
    delivered_bps: u64,
    last_change: Option<Instant>,
    change: Option<Change>,
    steps_down: u32,
    last_cut: Option<Instant>,
    /// Where the cuts held back since the stream last sent more would have
    /// left the rate, and when the last of them was held (see [`HELD_FOR`]).
    held: Option<(u64, Instant)>,
    held_cuts: u32,
    probes: u32,
    failed_probes: u32,
}

impl Default for LinkRate {
    fn default() -> Self {
        Self::off("disabled")
    }
}

/// PyroWave's detail floor: the bitrate below which the project's test scenes lost
/// detail ([`crate::pyrowave::minimum_kbps`], about 186 Mbps at 1080p60), in the
/// session's bitrate, which the sender scales by configured over negotiated. The
/// floor uses the whole frame rate, as the client's own minimum does, so 59.94 fps
/// is compared as 60.
pub fn pyrowave_floor_kbps(stream: &Negotiated) -> u32 {
    let minimum =
        crate::pyrowave::minimum_kbps(stream.width, stream.height, stream.fps.saturating_mul(1000));
    in_session_bitrate(minimum, stream)
}

/// PyroWave's recommended rate ([`crate::pyrowave::recommended_kbps`]) in the session's bitrate,
/// scaled exactly as the floor is, so the stream card's amber level, the log and the launch
/// warning agree. The whole frame rate is used, as for the floor.
pub fn pyrowave_recommended_kbps(stream: &Negotiated) -> u32 {
    let recommended = crate::pyrowave::recommended_kbps(
        stream.width,
        stream.height,
        stream.fps.saturating_mul(1000),
    );
    in_session_bitrate(recommended, stream)
}

/// `rate_kbps` reduced from the bitrate the client configured to the one the sender codes
/// (`bitrate_kbps` over `configured_bitrate_kbps`), unchanged when the client's number is unknown.
fn in_session_bitrate(rate_kbps: u32, stream: &Negotiated) -> u32 {
    if stream.configured_bitrate_kbps == 0 {
        return rate_kbps;
    }
    (u64::from(rate_kbps) * u64::from(stream.bitrate_kbps)
        / u64::from(stream.configured_bitrate_kbps))
    .min(u64::from(u32::MAX)) as u32
}

impl LinkRate {
    fn off(reason: &'static str) -> Self {
        Self {
            budget: None,
            reason,
            client_owned: false,
            state: State::Off,
            ceiling_kbps: 0,
            floor_kbps: 0,
            current: 0,
            good: 0,
            bad: None,
            learned: None,
            next_retry: None,
            retry_wait: HOLD,
            failed_at: None,
            host_loss_at: None,
            queue_peak: Duration::ZERO,
            encoder_floor: None,
            overshoots: 0,
            pending: [None; PENDING_CAP],
            confirmed: None,
            hold: HOLD,
            knee: HOLD,
            knee_until: None,
            next_probe: None,
            at_ceiling: None,
            near_bad: None,
            fence: None,
            latch_watch: None,
            latch_at: None,
            watch_keyframes: Vec::new(),
            watch_events: Vec::new(),
            rebuilt_blocks: 0,
            rebuilt_packets: 0,
            rebuilt_data_missing: 0,
            static_picture: false,
            reorder_share: 0.,
            reorder_at: None,
            excess_credit: 0,
            excess_high: 0,
            last_sent: None,
            ring: Box::default(),
            events: Events::default(),
            fec_status_seen: false,
            records: false,
            pyrowave_no_room: false,
            incident: None,
            last_ping: None,
            minimum_rtt: None,
            rtt_floor: VecDeque::with_capacity(FLOOR_BUCKETS),
            queue_since: None,
            raised_at: None,
            loss_cut_at: None,
            queue_quiet_until: None,
            queue_excess: None,
            queue_steps: 0,
            rtt: Duration::ZERO,
            next_eval: None,
            cache: None,
            delivered_bps: 0,
            last_change: None,
            change: None,
            steps_down: 0,
            last_cut: None,
            held: None,
            held_cuts: 0,
            probes: 0,
            failed_probes: 0,
        }
    }

    /// Whether a block the client could not rebuild lost its frame (E1). In
    /// PyroWave's record framing the coarse block (block 0) carries the
    /// picture and the detail blocks have no parity while it moves: a lost
    /// detail packet blurs part of one picture, so it counts toward the loss
    /// share (E2) only. 0.1 % random loss hits about one picture in six at
    /// 200 Mbps, and counted as lost frames it cut a healthy link to the floor.
    /// Block 0 is the coarse block only while it carries parity: with
    /// `pyrowave_critical_fec_percentage = 0`, or coarse records and parity
    /// over 255 packets, the sender splits the picture into quarters without
    /// parity (`packet.rs`), and block 0 is a quarter of the detail.
    pub fn frame_lost(&self, status: &Status) -> bool {
        let received = u32::from(status.received_data) + u32::from(status.received_parity);
        received < u32::from(status.data)
            && (!self.records || (status.block == 0 && status.parity > 0))
    }

    /// PyroWave's floor is at or above the stream's own bitrate (1080p120 at
    /// 200 Mbps needs 373, 4K120 at 800 about 1490): there is no quality floor
    /// to stop at, so cuts stop at half the bitrate.
    pub fn pyrowave_no_room(&self) -> bool {
        self.pyrowave_no_room
    }

    /// `enabled` is the configuration (`link_rate_adaptation`). `max_kbps` is
    /// the user's `max_bitrate` (0 for none), `fec` the FEC percentage in use.
    pub fn new(enabled: bool, stream: &Negotiated, max_kbps: u32, fec: usize) -> Self {
        let mut this = Self::off("disabled");
        if !enabled {
            return this;
        }
        let pyrowave = stream.codec == 3;
        let ceiling = stream
            .bitrate_kbps
            .min(if pyrowave { 2_000_000 } else { 500_000 })
            .min(if max_kbps > 0 { max_kbps } else { u32::MAX });
        let budget = WireBudget::of(stream);
        let period = crate::reconfigure::Mode::of(stream).period();
        let Some(minimum) = budget.minimum_encoder_kbps(ceiling, fec, period) else {
            this.reason = "wire_budget_too_small";
            return this;
        };
        // Below the level where the project's test scenes kept detail, PyroWave loses text and texture detail:
        // it stops there and the warning says to use HEVC or AV1.
        let lowest = if pyrowave {
            pyrowave_floor_kbps(stream)
        } else {
            2_000
        };
        this.pyrowave_no_room = pyrowave && lowest >= ceiling;
        // A stream already under PyroWave's floor has no quality floor to keep.
        // Until 2026-11-17 client loss could not lower it at all, so a path
        // that carried less than the request (a mesh node's wireless backhaul
        // at 4K120) held it there into hundreds of lost pictures and 279 ms of
        // queue; only the host's own refused sends could cut, and to half.
        // Half the bitrate is now the floor for every cut.
        let lowest = if this.pyrowave_no_room {
            ceiling / 2
        } else {
            lowest
        };
        this.floor_kbps = ceiling.min(lowest).max(minimum).min(ceiling);
        this.ceiling_kbps = ceiling;
        this.records = pyrowave && stream.pyrowave_records;
        // A client that announced FEC status reports every frame it lost, so
        // its keyframe requests are no loss evidence from the first frame on,
        // as `KeyframeLoss` already treats them (it calls such a client's
        // repeated request its decoder asking again). The trade-off: a client
        // that sets the flag but never sends a valid report (0x5502) gets no
        // request-based evidence either, except while a probe step is
        // unconfirmed or under a latch watch.
        this.fec_status_seen = stream.moonlight_features & crate::rtsp::FEC_STATUS_FEATURE != 0;
        this.budget = Some(budget);
        this.ring = vec![Sent::default(); RING].into_boxed_slice();
        for events in [
            &mut this.events.unrecoverable,
            &mut this.events.send_losses,
            &mut this.events.refused,
        ] {
            events.reserve_exact(EVENT_CAP);
        }
        this.events.requests.reserve_exact(EVENT_CAP);
        this.state = State::Full;
        this.reason = "healthy";
        this
    }

    /// A new controller for a reconfigured stream (new bitrate, FEC or
    /// codec). It keeps what the stream already decided: once the client set
    /// the bitrate, it stays in charge.
    pub fn rebuild(&self, enabled: bool, stream: &Negotiated, max_kbps: u32, fec: usize) -> Self {
        let mut this = Self::new(enabled, stream, max_kbps, fec);
        if self.client_owned {
            this.client_takeover();
        }
        this
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn ceiling_kbps(&self) -> u32 {
        self.ceiling_kbps
    }

    /// The client set the bitrate itself (`/bitrate`); this controller stays
    /// off until the stream is reconfigured.
    pub fn client_takeover(&mut self) {
        self.client_owned = true;
        self.state = State::Off;
        self.reason = "client_controlled";
        self.events.clear();
        self.pending = [None; PENDING_CAP];
        self.confirmed = None;
        self.learned = None;
        self.next_retry = None;
    }

    fn is_active(&self) -> bool {
        self.budget.is_some() && self.state != State::Off
    }

    fn encoder_kbps(&mut self, fec: usize, period: Duration) -> u32 {
        let key = (self.current, fec, period);
        if let Some((cached, kbps)) = self.cache
            && cached == key
        {
            return kbps;
        }
        let mut kbps = self
            .budget()
            .encoder_kbps(self.current, self.ceiling_kbps, fec, period)
            .clamp(self.floor_kbps, self.ceiling_kbps);
        // Less FEC frees wire bits for video, but only a probe may spend them.
        if let Some(((current, was_fec, was_period), cached)) = self.cache
            && current == self.current
            && period == was_period
            && fec < was_fec
        {
            kbps = kbps.min(cached);
        }
        self.cache = Some((key, kbps));
        kbps
    }

    fn kbps_for(&self, total: u64, fec: usize, period: Duration) -> u32 {
        self.budget()
            .encoder_kbps(total, self.ceiling_kbps, fec, period)
            .clamp(self.floor_kbps, self.ceiling_kbps)
    }

    /// The learned ceiling in total wire bits per second, 0 while the path
    /// has shown no edge; for the simulated-link tests.
    #[cfg(test)]
    pub fn learned_bps(&self) -> u64 {
        self.learned.unwrap_or(0)
    }

    fn budget(&self) -> WireBudget {
        self.budget.expect("active controller has a budget")
    }

    fn floor_total(&self, fec: usize, period: Duration) -> u64 {
        self.budget().total_bps(self.floor_kbps, fec, period)
    }

    /// The lowest total a cut may ask for: the configured minimum, or the rate
    /// the encoder was seen to send if that is higher.
    fn cut_floor(&self, fec: usize, period: Duration) -> u64 {
        self.floor_total(fec, period)
            .max(self.encoder_floor.unwrap_or(0))
    }

    fn ceiling_total(&self, fec: usize, period: Duration) -> u64 {
        self.budget().total_bps(self.ceiling_kbps, fec, period)
    }

    fn record(&mut self, now: Instant, change: Change) {
        self.last_change = Some(now);
        self.change = Some(change);
    }

    /// Take the change made by the last `tick`, once, to log it.
    pub fn take_change(&mut self) -> Option<Change> {
        self.change.take()
    }

    /// State for the session API and the `stream timings` log line.
    pub fn snapshot(&self, now: Instant) -> serde_json::Value {
        let current_kbps = match self.cache {
            Some((_, kbps)) if matches!(self.state, State::Limited | State::Floor) => kbps,
            _ => self.ceiling_kbps,
        };
        serde_json::json!({
            "state": self.state.name(),
            "reason": self.reason,
            "ceiling_kbps": self.ceiling_kbps,
            "current_kbps": current_kbps,
            "delivered_kbps": self.delivered_bps / 1000,
            "bad_kbps": self.bad.map_or(0, |bad| self.kbps_of(bad)),
            "learned_kbps": self.learned.map_or(0, |learned| self.kbps_of(learned)),
            "host_limited": self.host_limited(now),
            "encoder_floor_kbps": self.encoder_floor.map_or(0, |floor| self.kbps_of(floor)),
            "steps_down": self.steps_down,
            "held_cuts": self.held_cuts,
            "probes": self.probes,
            "failed_probes": self.failed_probes,
            "rebuilt_blocks": self.rebuilt_blocks,
            "rebuilt_data_missing_percent": if self.rebuilt_packets == 0 {
                0.
            } else {
                self.rebuilt_data_missing as f64 * 100. / self.rebuilt_packets as f64
            },
            "rtt_ms": self.rtt.as_millis() as u64,
            "rtt_floor_ms": self.rtt_floor().map_or(0, |floor| floor.as_millis() as u64),
            "last_change_ms_ago": self
                .last_change
                .map(|at| now.saturating_duration_since(at).as_millis() as u64),
        })
    }

    /// Rough encoder kbps for a wire rate at the last FEC seen; only for the
    /// snapshot.
    fn kbps_of(&self, wire: u64) -> u32 {
        match (self.cache, self.budget) {
            (Some(((_, fec, period), _)), Some(budget)) => {
                budget.encoder_kbps(wire, self.ceiling_kbps, fec, period)
            }
            _ => 0,
        }
    }
}
