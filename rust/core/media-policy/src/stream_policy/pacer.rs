//! Arrival pacing, frame cadence, and still-picture resend timing.

use super::source_phase::SourcePhase;
use butterpollo_config::config::Config;
use std::time::{Duration, Instant};

/// A frame slot is consumed by a submission, not by checking an unchanged image.
pub struct Cadence {
    due: Instant,
    period: Duration,
    smooth: bool,
}
impl Cadence {
    pub fn new(now: Instant, period: Duration, smooth: bool) -> Self {
        Self {
            due: now,
            period,
            smooth,
        }
    }
    pub fn deadline(&self) -> Instant {
        self.due
    }
    pub fn submitted(&mut self, now: Instant) {
        self.submitted_after(now, self.period);
    }
    /// Like [`Self::submitted`], with the next slot `interval` away instead of
    /// one stream period: phase lock follows the client's display this way.
    pub fn submitted_after(&mut self, now: Instant, interval: Duration) {
        let anchored = self.due + interval;
        // After a static interval, start a new cadence without a catch-up burst.
        self.due = if self.smooth && anchored > now {
            anchored
        } else {
            now + interval
        };
    }
}
/// How the encoder chooses when to claim a captured frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pacing {
    /// Claim each new frame as it arrives, at most at the stream rate. A frame
    /// never waits for a slot that is unrelated to when Windows presented it.
    Arrival,
    /// Claim the newest frame on a fixed grid at the stream rate.
    Grid,
}
impl Pacing {
    pub fn from_config(config: &Config) -> Self {
        match config
            .get("frame_pacing", "arrival")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "grid" | "fixed" => Self::Grid,
            "arrival" | "" => Self::Arrival,
            other => {
                butterpollo_config::config::fallback("frame_pacing", other, "arrival");
                Self::Arrival
            }
        }
    }
}
/// What the encoder should do with the newest unclaimed frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pace {
    Claim,
    /// Wait for a fresher frame or this deadline, whichever comes first.
    WaitUntil(Instant),
}

/// Claims frames when they arrive, without exceeding the stream rate on average.
///
/// A source faster than the stream (the 2x virtual display, a 240 Hz monitor)
/// delivers frames the stream cannot use. Claiming the first one that may be
/// claimed would let it age until the claim; instead the pacer waits for the
/// next source frame when it is expected shortly after the claim becomes
/// allowed. A source at or below the stream rate is claimed on arrival.
pub struct Pacer {
    period: Duration,
    pub(super) credit: f64,
    pub(super) credit_at: Instant,
    pub(super) last_claim: Option<Instant>,
    prediction: bool,
    /// The shortest source interval a prediction waits for.
    prediction_floor: Duration,
    pub(super) source_phase: Option<SourcePhase>,
    /// The least time between two claims, in periods.
    spacing: f64,
    /// The source presents faster than the stream rate.
    faster: bool,
    /// The source's mean frame interval, when the capture knows it.
    source_rate: Option<Duration>,
    /// When the source's latest picture was first captured.
    picture: Option<Instant>,
    /// The picture interval nearly all recent pictures arrived within.
    late_picture: Option<Duration>,
    /// The newest capture only extends the focus window's latest picture.
    extends: bool,
    /// The first capture of the picture the latest claim took.
    claimed_picture: Option<Instant>,
    /// Time after which an unchanged picture is encoded again.
    static_period: Duration,
    /// Credit a faster source may be lent, in frames, to repay a deficit.
    grace: f64,
    grace_at: Instant,
    /// The source phase's 1.25-period bound when a wait since the latest
    /// claim ran past it into `PREDICTED_NOTICE` (`claimed`).
    notice_bound: Option<Instant>,
    /// The newest capture `decide` has seen, and how long after their
    /// presents the latest ones first reached it (`notice`).
    noticed: Option<Instant>,
    pub(super) notices: [Duration; 16],
    notice_next: usize,
}
impl Pacer {
    /// Credit above one frame absorbs arrival jitter of a source at the stream rate.
    const CREDIT_CAP: f64 = 1.5;
    pub fn new(now: Instant, period: Duration) -> Self {
        Self {
            period,
            credit: Self::CREDIT_CAP,
            credit_at: now,
            last_claim: None,
            prediction: true,
            prediction_floor: Duration::ZERO,
            source_phase: None,
            spacing: 0.75,
            notice_bound: None,
            noticed: None,
            notices: [Duration::ZERO; 16],
            notice_next: 0,
            faster: false,
            source_rate: None,
            picture: None,
            late_picture: None,
            extends: false,
            claimed_picture: None,
            static_period: Duration::from_millis(50),
            grace: Self::GRACE_CAP,
            grace_at: now,
        }
    }
    /// When an unchanged picture is encoded again (`Rates::static_period`):
    /// another window's update next to a focus picture already claimed waits
    /// as long, unless the focus window changes first.
    pub fn with_static_period(mut self, period: Duration) -> Self {
        self.static_period = period;
        self
    }
    /// Whether the newest capture only extends the focus window's latest
    /// picture (`Freshness::extends_picture`).
    pub fn observe_extension(&mut self, extends: bool) {
        self.extends = extends;
    }
    /// VRR: the display follows each frame, so a game's uneven frame times
    /// should reach it as they are. The average stays capped at the stream
    /// rate; 3/4 of a period between claims distorted that cadence by up to
    /// 3 ms, half a period by about 1 ms.
    /// Phase lock: claims follow the client's refresh period instead of the
    /// stream's own. The source phase keeps its period; the two differ by a
    /// few parts in ten thousand.
    pub fn set_period(&mut self, now: Instant, period: Duration) {
        self.credit = self.credit(now);
        self.credit_at = now;
        self.period = period;
    }
    pub fn with_spacing(mut self, periods: f64) -> Self {
        self.spacing = periods;
        self
    }
    /// Diagnostic comparison: keep the same rate and burst limits, but claim
    /// at the earliest allowed slot instead of waiting for a predicted frame.
    /// Prediction remains enabled unless explicitly disabled.
    pub fn with_prediction(mut self, prediction: bool) -> Self {
        self.prediction = prediction;
        self
    }
    /// Predictive waits only for a source whose frames are at least this far
    /// apart. VRR runs the virtual display at 1000 Hz, whose frame interval is
    /// the display's own refresh, not content: a wait for the next one would
    /// hold every claim up to a quarter period for nothing. A game or desktop
    /// on that display (120 or 240 Hz) shows its own, longer interval.
    pub fn with_prediction_floor(mut self, floor: Duration) -> Self {
        self.prediction_floor = floor;
        self
    }
    /// Learn the dominant phase from observed capture
    /// timestamps. Apply it only when at least 32 recent observations show
    /// capture updates arriving at least 10% faster than the requested rate.
    /// The host enables this for WGC; ordinary arrival pacing is retained
    /// whenever the observed source does not meet those conditions.
    pub fn with_source_phase(mut self, enabled: bool) -> Self {
        self.source_phase = enabled.then(|| SourcePhase::new(self.period));
        self
    }
    /// Observe each newest image seen by the stream, including images replaced
    /// before a claim. Re-reading the same captured timestamp is deduplicated.
    pub fn observe_source(&mut self, presented: Instant) {
        if let Some(phase) = &mut self.source_phase {
            phase.observe(presented);
        }
    }
    /// Turn source-phase learning on or off for the capture that is open now;
    /// a change starts it from scratch, the same state keeps what was learned.
    pub fn set_source_phase(&mut self, enabled: bool) {
        if enabled != self.source_phase.is_some() {
            self.source_phase = enabled.then(|| SourcePhase::new(self.period));
        }
    }
    /// The source's mean frame interval over its recent frames. Whether it is
    /// faster than the stream follows its rate, not its median interval: a
    /// 240 Hz virtual display alternating between one and two refreshes per
    /// picture has an 8 ms median at about 160 frames a second. Judged by the
    /// median, it refilled credit 1% fast, so claims ran at 121 a second off
    /// the source's arrivals and each took a picture up to 4 ms old.
    pub fn observe_source_rate(&mut self, mean_interval: Option<Duration>) {
        self.source_rate = mean_interval;
    }
    /// When the source's latest picture was first captured, and the picture
    /// interval nearly all recent pictures arrived within
    /// (`Freshness::picture_start`, `Freshness::late_picture_interval`). A
    /// prediction counts from the picture's first capture: from a second
    /// composition of the same picture, 1 ms later, it expected the next frame
    /// 1 ms late. A claim waiting for the next picture keeps waiting until the
    /// late interval, at most a quarter period past the median: a 120 Hz
    /// game's frames under VRR came 7.3 to 9.3 ms apart, so a wait that gave up
    /// 1 ms after the median prediction still took 0.5-2 % of frames 8 ms old.
    pub fn observe_picture(&mut self, start: Option<Instant>, late: Option<Duration>) {
        self.picture = start;
        self.late_picture = late;
    }
    /// Capture recovery or replacement invalidates the learned source phase.
    pub fn reset_source_phase(&mut self) {
        if self.source_phase.is_some() {
            self.source_phase = Some(SourcePhase::new(self.period));
        }
    }
    /// Credit refills slightly faster than the stream rate. A source at exactly
    /// the stream rate refills what each claim spends, so a deficit left by a
    /// startup burst would otherwise delay every following claim for the rest
    /// of the session; with the margin it is repaid within about a second.
    /// A faster source always has a frame for the next slot, so it refills at
    /// exactly the stream rate: with the margin, a 240 Hz display streamed at
    /// 120 fps was claimed 121.2 times a second.
    const REFILL: f64 = 1.01;
    fn credit(&self, now: Instant) -> f64 {
        let refill = if self.faster { 1. } else { Self::REFILL };
        let earned = now.saturating_duration_since(self.credit_at).as_secs_f64() * refill
            / self.period.as_secs_f64();
        (self.credit + earned).min(Self::CREDIT_CAP)
    }
    /// A faster source refills at exactly the stream rate, so a deficit
    /// would last the whole session: every claim waited the same fraction of
    /// a frame for credit while a fresh frame was in hand (2-3 ms at 60 fps on
    /// the 2x virtual display). Such a frame may borrow the shortfall from a
    /// grace that refills at a tenth of a frame a second, a quarter frame at
    /// most, so the claim rate stays within 0.1 frames a second of the stream.
    const GRACE_CAP: f64 = 0.25;
    const GRACE_PER_SECOND: f64 = 0.1;
    fn lend_grace(&mut self, now: Instant) {
        let grace = (self.grace
            + now.saturating_duration_since(self.grace_at).as_secs_f64() * Self::GRACE_PER_SECOND)
            .min(Self::GRACE_CAP);
        self.grace_at = now;
        let spaced = self
            .last_claim
            .is_none_or(|claim| now >= claim + self.period.mul_f64(self.spacing));
        let credit = self.credit(now);
        let lent = if spaced {
            (Self::CLAIM_CREDIT - credit).clamp(0., grace)
        } else {
            0.
        };
        self.grace = grace - lent;
        if lent > 0. {
            self.credit = credit + lent;
            self.credit_at = now;
        }
    }
    /// A claim needs this much credit. A frame at the stream rate that arrives
    /// a little before the previous claim's slot (because that claim was itself
    /// late) is claimed at once; waiting would carry the lateness into every
    /// following frame. Each claim still costs a whole frame of credit.
    const CLAIM_CREDIT: f64 = 7. / 8.;
    /// How long after its latest predicted present a frame the claim waits
    /// for gets to be noticed, even when that runs past the prediction budget.
    /// Cut off at the budget, a frame due just before it was noticed just
    /// after: the claim waited the whole budget, then took the frame one
    /// source interval old, and each such claim set the next budget one period
    /// later, so the miss repeated. A missed prediction holds a frame at most
    /// this long past the budget, plus the late picture interval's lead over
    /// the median (`observe_picture`), at most a quarter period.
    pub(super) const PREDICTED_NOTICE: Duration = Duration::from_millis(1);
    /// The earliest instant the next frame may be claimed: no two claims closer
    /// than three quarters of a period, and no more than the stream rate overall.
    pub fn allowed_at(&self, now: Instant) -> Instant {
        let spaced = self
            .last_claim
            .map_or(now, |claim| claim + self.period.mul_f64(self.spacing));
        let credit = self.credit(now);
        let funded = if credit >= Self::CLAIM_CREDIT {
            now
        } else {
            now + self.period.mul_f64(Self::CLAIM_CREDIT - credit)
        };
        spaced.max(funded)
    }
    /// When a still-picture resend due at `due` may go out, asked at `now`:
    /// no closer than the spacing to the last claim, and only with enough
    /// credit left that a new picture a spacing after it is claimed at once.
    /// A resend spends a claim's credit but is never paced by
    /// [`Self::decide`]; on its slot grid, a late resend and an on-time one
    /// went out less than the spacing apart, and the next new picture waited
    /// up to half a period for the credit they spent.
    pub fn resend_at(&self, due: Instant, now: Instant) -> Instant {
        let spaced = self
            .last_claim
            .map_or(now, |claim| claim + self.period.mul_f64(self.spacing));
        // Spent, the resend leaves `reserve - 1`; a spacing later (at the
        // slowest refill, one claim a period) that is a claim's credit.
        let reserve = (1. + Self::CLAIM_CREDIT - self.spacing).min(Self::CREDIT_CAP);
        let credit = self.credit(now);
        let funded = if credit >= reserve {
            now
        } else {
            now + self.period.mul_f64(reserve - credit)
        };
        due.max(spaced).max(funded)
    }
    /// Give an imminent fresh capture a bounded chance to replace an unchanged
    /// image before a minimum-rate repeat consumes its pacing credit. `due`
    /// must be the original repeat deadline, not the current polling time or a
    /// previously deferred deadline; the result is fixed for the same inputs.
    /// A stale prediction adds no wait. This only delays repeats, so it also
    /// protects fresh frames with Desktop Duplication and VRR arrival pacing.
    pub fn repeat_deadline(
        &self,
        due: Instant,
        presented: Instant,
        source_interval: Option<Duration>,
    ) -> Instant {
        let Some(interval) =
            source_interval.filter(|interval| *interval >= self.period.mul_f64(0.875))
        else {
            return due;
        };
        let next = presented + interval;
        let detection = self.period / 4;
        let latest = due + detection + Duration::from_micros(500);
        let detected = next + detection;
        if detected > due && next <= latest {
            detected.min(latest)
        } else {
            due
        }
    }
    /// The first capture of the picture `presented` belongs to, and when the
    /// next picture is due at the latest, by the late interval at most a
    /// quarter period past the median (`observe_picture`).
    fn picture_due(&self, presented: Instant, interval: Duration) -> (Instant, Instant) {
        // Count from the first capture of the picture `presented` belongs
        // to. A start after it is a newer capture this claim has not seen.
        let since = self
            .picture
            .filter(|start| {
                *start <= presented
                    && (self.extends || presented.duration_since(*start) < interval / 2)
            })
            .unwrap_or(presented);
        let late = since
            + self.late_picture.map_or(interval, |late| {
                late.clamp(interval, interval + self.period / 4)
            });
        (since, late)
    }
    /// How long after its present a capture reaches the claim: the longest
    /// of the latest 16 captures' first decisions, at least
    /// `PREDICTED_NOTICE` and at most a quarter period. The quarter period wins where
    /// it is the shorter, above 250 fps: `clamp` would panic there.
    pub(super) fn notice(&self) -> Duration {
        let longest = self.notices.iter().copied().max().unwrap_or_default();
        longest.max(Self::PREDICTED_NOTICE).min(self.period / 4)
    }
    /// Decide for the newest unclaimed frame, presented at `presented`, given
    /// the source's recent frame interval as seen by the capture worker.
    pub fn decide(
        &mut self,
        now: Instant,
        presented: Instant,
        source_interval: Option<Duration>,
    ) -> Pace {
        self.faster = self
            .source_rate
            .or(source_interval)
            .is_some_and(|i| i < self.period.mul_f64(0.95));
        if self.faster {
            self.lend_grace(now);
        }
        if self.noticed.is_none_or(|seen| presented > seen) {
            self.noticed = Some(presented);
            self.notices[self.notice_next] = now.saturating_duration_since(presented);
            self.notice_next = (self.notice_next + 1) % self.notices.len();
        }
        let allowed = self.allowed_at(now);
        // Another window's update next to a focus picture already claimed
        // carries no new focus frame: claimed once credit allowed, it showed
        // that frame twice and the next claim skipped one. It waits like an
        // unchanged picture unless the focus window changes first.
        if self.extends
            && self.claimed_picture.is_some()
            && self.picture.filter(|start| *start <= presented) == self.claimed_picture
        {
            let repeat = self
                .last_claim
                .map_or(allowed, |last| last + self.static_period)
                .max(allowed);
            return if now >= repeat {
                Pace::Claim
            } else {
                Pace::WaitUntil(repeat)
            };
        }
        if let Some(phase) = &mut self.source_phase {
            let limit = source_interval.map_or(1, |interval| {
                let interval = interval.as_nanos().max(1);
                ((u128::from(phase.period_ns) + interval / 2) / interval).clamp(1, 4) as u64
            });
            phase.limit_to(limit, now);
        }
        if let Some(phase) = &self.source_phase
            && let Some(center) = phase.center
            && phase.has_surplus()
        {
            let cycle = u128::from(phase.cycle_ns());
            let offset = phase.offset(presented);
            let mut advance = (u128::from(center) + cycle - u128::from(offset)) % cycle;
            // A capture near the learned phase belongs to that update, even
            // when jitter puts it just before the center. Its next update is
            // one cycle later; an old extra composition must not skip this wait.
            if advance <= u128::from(phase.radius_ns()) {
                advance += cycle;
            }
            let mut next = presented + Duration::from_nanos(advance as u64);
            // A compositor update can arrive half a stream period late.
            // Keep the fallback anchored to this capture, not the polling time.
            // With several updates a period, half a cycle late (a missed
            // refresh of a display at twice the source's rate) is the most:
            // later than that, it is the next update.
            let detection = if phase.multiple > 1 {
                Duration::from_nanos(phase.cycle_ns() / 2) + Self::PREDICTED_NOTICE
            } else {
                self.period / 2 + Duration::from_micros(500)
            };
            // The update this capture waits for is the first one due once the
            // claim is allowed; an earlier one, if it comes, is decided on its
            // own. Waiting for one due before then (a 120 Hz game's frame
            // 1.3 ms early, just outside the radius, or a game falling to
            // 60 Hz) took the capture 10-14 ms old at that update's fallback,
            // just before the game's next frame was noticed. One whose late
            // window has passed was missed: the capture is claimed.
            if phase.multiple > 1 {
                while next < allowed && now < next + detection {
                    next += Duration::from_nanos(phase.cycle_ns());
                }
            }
            // A source updating several times a period: an in-phase capture's
            // next update is at least a cycle less a radius away, and the
            // slack stays a radius short of that. Past it (a 240 Hz source at
            // 60 fps), a fresh in-phase capture waited for the next one, and
            // every claim ran a cycle late. Up to it, an update half a 120 Hz
            // game's cycle off its phase (another window's on the 240 Hz
            // display's other refresh) waits for the game's next frame, which
            // came 4.1 to 5.3 ms later: a quarter period was too short.
            let slack = if phase.multiple > 1 {
                Duration::from_nanos(phase.cycle_ns().saturating_sub(2 * phase.radius_ns()))
            } else {
                self.period / 4 + Duration::from_micros(500)
            };
            // An in-phase capture of a source updating several times a period
            // is that update, claimed once allowed: noticed later than about
            // two radii after it, it waited for the next update, noticed as
            // late, and so on to the bound (a 240 Hz game at 120 fps noticed
            // 0.9-1.5 ms late: 88-105 claims a second, p95 age 4.3-5.8 ms).
            let in_phase =
                phase.multiple > 1 && phase.distance(offset, center) <= phase.radius_ns();
            if next + detection > allowed && next <= allowed + slack {
                // A new picture can precede the learned phase. Waiting for
                // another composition must not stretch delivery to 1.5 periods.
                // With several updates a period, an update due right at that
                // bound still gets `PREDICTED_NOTICE` to be noticed in: a
                // 120 Hz game frame that missed one 240 Hz refresh came at
                // exactly 1.25 periods, and a cut there took the frame before
                // it, 12.5 ms old. At one update a period the notice only
                // delayed stale claims: a 60 Hz game frame that misses a
                // 120 Hz refresh comes half a period late.
                let notice = if phase.multiple > 1 {
                    Self::PREDICTED_NOTICE
                } else {
                    Duration::ZERO
                };
                let bound = self
                    .last_claim
                    .map(|last| last + self.period + self.period / 4);
                let mut deadline = bound
                    .filter(|bound| *bound + notice < next + detection)
                    .map_or(next + detection, |bound| bound + notice);
                // With several updates a period, the wait ends where the
                // picture interval's would (below), given as long as recent
                // captures took to be noticed: to the bound, a game's frame
                // before a 30 ms hitch or a pause was held for an update that
                // did not come and claimed 14 ms old, against 9.6 ms. A game
                // whose frame missed a refresh within `MISS_HOLD` waits to the
                // bound, where its late frames come: cut where the picture
                // interval's would, one claim in 13 took the frame before,
                // 9.5 ms old, when the game missed every 13th refresh.
                if phase.multiple > 1
                    && let Some(interval) = source_interval
                    && phase.missed.is_none_or(|missed| {
                        now.saturating_duration_since(missed) > SourcePhase::MISS_HOLD
                    })
                {
                    let (_, late) = self.picture_due(presented, interval);
                    deadline = deadline.min(
                        (allowed + self.period / 4 + Duration::from_micros(500))
                            .max(late + self.notice()),
                    );
                }
                // A capture just early of the radius (up to half a radius
                // more) is most likely that update come early: it waits only
                // to the phase center and a notice. Second compositions pull
                // the learned center late, and a jittery 120 Hz game's frame
                // 1.2-1.8 ms early waited for itself to the bound, claimed
                // 6.6 ms old with 21.8 ms gaps. Widening `in_phase` instead
                // took other windows' updates just before game frames.
                if phase.multiple > 1 && now >= allowed {
                    let early = (u128::from(center) + cycle - u128::from(offset)) % cycle;
                    let radius = u128::from(phase.radius_ns());
                    if early > radius && early <= radius + radius / 2 {
                        deadline = deadline
                            .min(presented + Duration::from_nanos(early as u64) + self.notice());
                    }
                }
                // Kept until the claim (`claimed`): the decision that claims
                // can come from a later capture, or skip this wait.
                if let Some(bound) =
                    bound.filter(|bound| notice > Duration::ZERO && deadline > *bound)
                {
                    self.notice_bound = Some(bound);
                }
                // Only an update predicted before the deadline is worth the
                // wait: a fresh frame a missed refresh made late (2 to 5 ms on
                // a 240 or 360 Hz display) was held to the deadline, then
                // claimed anyway.
                if !(in_phase && now >= allowed) && deadline > allowed && next < deadline {
                    return Pace::WaitUntil(deadline);
                }
            }
        }
        if now >= allowed {
            return Pace::Claim;
        }
        // A long pause is a static desktop, not the source cadence.
        if self.prediction
            && let Some(interval) = source_interval.filter(|interval| {
                *interval >= self.prediction_floor && *interval <= self.period * 4
            })
        {
            let (since, late) = self.picture_due(presented, interval);
            let next = since + interval;
            let slack = self.period / 4 + Duration::from_micros(500);
            // `next` is when Windows will present the fresher frame; the
            // capture worker notices it up to a detection delay later, which
            // can reach a millisecond with a polled capture. A frame presented
            // just before the claim is allowed may still arrive after it.
            let detection = self.period / 4;
            if late + detection > allowed && next <= allowed + slack {
                // Detection shares the prediction budget: adding it after
                // the latest expected arrival held an available frame for
                // another quarter-period when that prediction was missed.
                // A frame due at the end of the budget still gets
                // `PREDICTED_NOTICE` to be noticed in.
                let noticed = late + Self::PREDICTED_NOTICE;
                return Pace::WaitUntil((late + detection).min((allowed + slack).max(noticed)));
            }
        }
        Pace::WaitUntil(allowed)
    }
    pub fn claimed(&mut self, now: Instant) {
        // The picture of the newest capture, which every claim encodes.
        self.claimed_picture = self.picture;
        self.credit = (self.credit(now) - 1.).max(-1.);
        self.credit_at = now;
        // A claim that waited past the phase bound into the notice spaces the
        // next one from the bound: spaced from the claim, a missed update (a
        // 120 Hz game frame that missed a 120 Hz refresh comes a whole update
        // late) also held the game's next frame on its cadence, and each miss
        // cost two claims.
        self.last_claim = Some(
            self.notice_bound
                .take()
                .filter(|bound| now > *bound)
                .map_or(now, |bound| {
                    now.checked_sub(Self::PREDICTED_NOTICE)
                        .map_or(bound, |early| bound.max(early))
                }),
        );
    }
}
/// When arrival pacing encodes an unchanged picture again. A static repeat
/// is due at `repeat_due`. For a keyframe or reference invalidation the
/// client asked for (`recovery`), a moving source's next new picture carries
/// it: the picture is held for two stream periods or one and a half of the
/// source's own frame intervals after it was presented, whichever is longer,
/// which leaves room for uneven frame times and capture detection. A picture
/// already unchanged that long (a still or slow screen) is encoded again at
/// once. Encoding a moving picture again at once sent an extra keyframe and
/// delayed the game's next frame behind it.
pub fn reencode_at(
    recovery: bool,
    presented: Instant,
    period: Duration,
    source_interval: Option<Duration>,
    repeat_due: Instant,
) -> Instant {
    if recovery {
        let hold = (period * 2).max(source_interval.unwrap_or(period).mul_f64(1.5));
        (presented + hold).min(repeat_due)
    } else {
        repeat_due
    }
}
/// Where the next still-picture resend counts from after an encode that
/// began at `begin`; the resend is due one `static_period` later. A static
/// resend (not `fresh`, not a `recovery` re-encode) stays on the grid of the
/// slot it filled while it was less than a period late, so the loop's wake-up
/// and encode time do not add up to a slower stream (a still desktop at about
/// 170 on a 180 fps stream); a resend a period or more late starts the grid
/// over at `begin`, one picture and never a catch-up of missed slots. A new
/// picture or a recovery re-encode anchors at `begin`, so a real frame is never
/// followed early. Fixed grid and arrival pacing share this; under arrival
/// pacing [`Pacer::resend_at`] also keeps a resend that falls due soon after a
/// late one to the claim spacing and credit.
pub fn resend_anchor(
    previous: Instant,
    begin: Instant,
    static_period: Duration,
    fresh: bool,
    recovery: bool,
) -> Instant {
    if fresh || recovery {
        begin
    } else {
        crate::capture_policy::grid_next(previous + static_period, begin, static_period)
    }
}
/// Whether an encode spends arrival pacing credit. New pictures and static
/// repeats do, so repeats stay within the stream rate. A picture encoded
/// again only for the client's recovery request does not: it would hold the
/// game's next frame back by up to a period, and leave every frame for about
/// a second after it late while the credit is repaid.
pub fn counts_toward_rate(fresh: bool, recovery: bool) -> bool {
    fresh || !recovery
}

/// Whether claims wait for the capture's learned phase, for the backend that
/// opened. Only WGC's frames arrive on a composition beat the wait can align
/// with: Rubylight's display driver pushes every composed frame, and a wait for
/// the next one under GPU load held claims up to 1.25 periods. Capture `auto`
/// is planned as WGC before it opens, so the plan cannot decide this.
/// `frame_pacing_source_phase` set either way wins.
pub fn source_phase(config: &Config, backend: &str) -> bool {
    config.boolean("frame_pacing_source_phase", backend == "wgc")
}
