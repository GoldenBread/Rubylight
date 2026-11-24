//! Learned capture source phase tracking for arrival pacing.

use std::time::{Duration, Instant};

/// Rolling phase of a stable source close to the requested stream rate, or
/// close to a whole multiple of it.
/// Capture can publish extra compositions between real source updates. Those
/// extra timestamps must not become the anchor for predicting the next update.
pub(super) struct SourcePhase {
    /// The stream period the phase was learned for.
    pub(super) period_ns: u64,
    /// The source's updates per stream period: phases fold over
    /// `period_ns / multiple` (`multiple_from`).
    pub(super) multiple: u64,
    /// The most updates per stream period the picture interval allows
    /// (`limit_to`), and when it last allowed as many.
    pub(super) limit: u64,
    pub(super) limit_at: Option<Instant>,
    pub(super) origin: Option<Instant>,
    pub(super) last: Option<Instant>,
    pub(super) samples: [u64; 64],
    pub(super) elapsed_ns: [u64; 64],
    pub(super) count: usize,
    pub(super) next: usize,
    pub(super) center: Option<u64>,
    pub(super) last_phase: Option<Instant>,
    /// When an update last came a display refresh late (`observe`).
    pub(super) missed: Option<Instant>,
}
impl SourcePhase {
    pub(super) fn new(period: Duration) -> Self {
        Self {
            period_ns: period.as_nanos().clamp(1, u128::from(u64::MAX)) as u64,
            multiple: 1,
            limit: 1,
            limit_at: None,
            origin: None,
            last: None,
            samples: [0; 64],
            elapsed_ns: [0; 64],
            count: 0,
            next: 0,
            center: None,
            last_phase: None,
            missed: None,
        }
    }
    /// The interval phases fold over: one source update.
    pub(super) fn cycle_ns(&self) -> u64 {
        (self.period_ns / self.multiple).max(1)
    }
    /// Fold the samples over one update of a source updating `multiple`
    /// times a stream period.
    pub(super) fn fold(&mut self, multiple: u64) {
        self.multiple = multiple;
        let cycle = self.cycle_ns();
        for index in 0..self.count {
            self.samples[index] = self.elapsed_ns[index] % cycle;
        }
    }
    /// A source updating 2 to 4 times a stream period (a 120 Hz game at
    /// 60 fps) shows one phase per update; folded over the stream period
    /// they are as many peaks, none with enough support, and the phase was
    /// never learned. Another window's updates then started pictures of
    /// their own: claimed once allowed, each took the game's previous frame
    /// a few milliseconds before its next, and the claims after it waited
    /// for the credit it spent. The largest multiple whose peak has an update
    /// in 80 % of its cycles is the source's, counted for every update of the
    /// period on its own (`cycle_share`): another window's updates on a
    /// 240 Hz display's other refreshes, 50 to 70 a second, filled 80 % of a
    /// 120 Hz game's quarter-period cycles together, but only the game's own
    /// two updates of the period fill them all. The multiple comes from the
    /// captures, with the picture interval only as the limit: other windows'
    /// updates and late second compositions split pictures, and the
    /// interval's median flickered between 2, 3 and 1 for a 120 Hz game.
    /// Tries the multiples from the limit down to `least`; leaves the samples
    /// folded over the last one tried.
    pub(super) fn multiple_from(&mut self, least: u64) -> Option<u64> {
        for multiple in (least.max(2)..=self.limit.min(4)).rev() {
            self.fold(multiple);
            let (center, _) = self.peak();
            let (matched, complete) = self.cycle_share(center);
            if complete > 0 && matched * 100 >= complete * 80 {
                return Some(center);
            }
        }
        None
    }
    /// A source that falls to fewer updates a period keeps the ones that
    /// still come: the phase at the largest whole divisor of the multiple the
    /// limit allows whose cycles one of its updates still covers. A 120 Hz
    /// game falling to 60 Hz left neither a 2x phase nor a 1x peak of 65 %
    /// among the samples of both rates; the claims fell back to the picture
    /// interval, still the 120 Hz one, and its wait ended just before the
    /// next 60 Hz frame was noticed: a claim 16.8 ms old. Leaves the samples
    /// folded over the phase found, or over the multiple when none is.
    pub(super) fn lower_multiple(&mut self, center: u64) -> Option<u64> {
        let (multiple, cycle) = (self.multiple, self.cycle_ns());
        for lower in (1..multiple).rev() {
            if multiple % lower != 0 || lower > self.limit {
                continue;
            }
            self.fold(lower);
            for update in 0..multiple / lower {
                let candidate = (center + update * cycle) % self.cycle_ns();
                if self.covers_cycles(candidate) {
                    return Some(candidate);
                }
            }
        }
        self.fold(multiple);
        None
    }
    /// A lower limit takes over once the higher one has not shown for this
    /// long: the picture interval's median flickers.
    pub(super) const LIMIT_HOLD: Duration = Duration::from_millis(250);
    /// How long a missed refresh (`missed`) keeps a wait to the phase bound.
    pub(super) const MISS_HOLD: Duration = Duration::from_secs(1);
    /// The whole source updates per period the picture interval allows at
    /// `now`. With focus labels the picture interval is the focus window's:
    /// another window at 240 Hz beside a 60 Hz game must not become the
    /// phase, and a multiple above the limit is learned again.
    pub(super) fn limit_to(&mut self, limit: u64, now: Instant) {
        if limit >= self.limit
            || self
                .limit_at
                .is_none_or(|at| now.saturating_duration_since(at) >= Self::LIMIT_HOLD)
        {
            self.limit = limit;
            self.limit_at = Some(now);
        }
    }
    /// The center of the densest radius-wide window of samples, and how many
    /// samples it holds.
    pub(super) fn peak(&self) -> (u64, usize) {
        let cycle = self.cycle_ns();
        let mut bins = [0usize; 64];
        for &sample in &self.samples[..self.count] {
            bins[(u128::from(sample) * 64 / u128::from(cycle)) as usize] += 1;
        }
        // The bins within the radius on either side: 4 of 64 at a multiple
        // of 1.
        let half = (u128::from(self.radius_ns()) * 64 / u128::from(cycle)) as usize;
        let population = |bin: usize| {
            (0..=2 * half)
                .map(|offset| bins[(bin + offset + 64 - half) % 64])
                .sum::<usize>()
        };
        let peak = (0..64).max_by_key(|&bin| population(bin)).unwrap();
        (
            ((peak as u128 * 2 + 1) * u128::from(cycle) / 128) as u64,
            population(peak),
        )
    }
    pub(super) fn offset(&self, at: Instant) -> u64 {
        (at.saturating_duration_since(self.origin.unwrap_or(at))
            .as_nanos()
            % u128::from(self.cycle_ns())) as u64
    }
    pub(super) fn distance(&self, a: u64, b: u64) -> u64 {
        let delta = a.abs_diff(b);
        delta.min(self.cycle_ns() - delta)
    }
    /// How far from the center an update still counts as in phase: 9/128 of
    /// the stream period, whatever the multiple (a game's frame times and a
    /// composition's second capture spread its updates as much), at most a
    /// quarter of an update.
    pub(super) fn radius_ns(&self) -> u64 {
        ((self.period_ns / 128) * 9).min(self.cycle_ns() / 4)
    }
    pub(super) fn has_surplus(&self) -> bool {
        if self.count < 32 {
            return false;
        }
        let oldest = if self.count == self.samples.len() {
            self.next
        } else {
            0
        };
        let newest = (self.next + self.samples.len() - 1) % self.samples.len();
        let span = self.elapsed_ns[newest].saturating_sub(self.elapsed_ns[oldest]);
        // Phase alignment only addresses surplus compositions. Near 1:1 and
        // slower sources keep normal pacing, even if their phase is stable.
        // Use observed timestamp intervals (not sample count / elapsed time)
        // so a finite window does not overestimate a healthy source's rate.
        span > 0
            && (self.count as u128 - 1) * u128::from(self.period_ns) * 10 >= u128::from(span) * 11
    }
    pub(super) fn covers_cycles(&self, center: u64) -> bool {
        let (matched, complete) = self.cycle_share(center);
        complete > 0 && matched * 100 >= complete * 65
    }
    /// The complete cycles within the samples, and how many of them have a
    /// sample near `center`: of the update of the stream period (the cycle's
    /// index modulo the multiple) with the fewest.
    pub(super) fn cycle_share(&self, center: u64) -> (u128, u128) {
        let oldest = if self.count == self.samples.len() {
            self.next
        } else {
            0
        };
        let newest = (self.next + self.samples.len() - 1) % self.samples.len();
        let period = u128::from(self.cycle_ns());
        let cycle = |elapsed: u64| {
            (u128::from(elapsed) + period - u128::from(center) + period / 2) / period
        };
        let first = cycle(self.elapsed_ns[oldest]);
        let last = cycle(self.elapsed_ns[newest]);
        let complete = last.saturating_sub(first + 1);
        let updates = u128::from(self.multiple.clamp(1, 4));
        let mut matched = [0u128; 4];
        let mut previous = None;
        for offset in 0..self.count {
            let index = (oldest + offset) % self.samples.len();
            let cycle = cycle(self.elapsed_ns[index]);
            if cycle > first
                && cycle < last
                && previous != Some(cycle)
                && self.distance(self.samples[index], center) <= self.radius_ns()
            {
                matched[(cycle % updates) as usize] += 1;
                previous = Some(cycle);
            }
        }
        // The complete cycles first + 1 ..= last - 1 of each update.
        let start = (first + 1) % updates;
        (0..updates)
            .map(|update| {
                let extra = u128::from((update + updates - start) % updates < complete % updates);
                (matched[update as usize], complete / updates + extra)
            })
            .min_by(|a, b| (a.0 * b.1).cmp(&(b.0 * a.1)).then(a.1.cmp(&b.1)))
            .unwrap_or((0, 0))
    }
    pub(super) fn observe(&mut self, at: Instant) {
        if self.last == Some(at) {
            return;
        }
        if self.last.is_some_and(|last| {
            at < last || at.duration_since(last) > Duration::from_nanos(self.period_ns) * 4
        }) {
            self.restart();
        }
        self.origin.get_or_insert(at);
        let mut phase = self.offset(at);
        if self
            .center
            .is_some_and(|center| self.distance(phase, center) <= self.radius_ns())
        {
            self.last_phase = Some(at);
        }
        // A frame that missed a refresh of a display at two to four times the
        // source's rate comes that refresh late, two radii to half a cycle off
        // the phase, in place of its update: the update before that one came.
        // Another window's update off the phase comes after the source's own
        // update, not in its place.
        if let (Some(center), Some(last)) = (self.center, self.last_phase) {
            let (cycle, radius) = (self.cycle_ns(), self.radius_ns());
            let late = (phase + cycle - center) % cycle;
            let since = at.duration_since(last).as_nanos();
            if (2 * radius..=cycle / 2 + radius).contains(&late)
                && (u128::from(late + cycle - radius)..=u128::from(late + cycle + radius))
                    .contains(&since)
            {
                self.missed = Some(at);
            }
        }
        // A phase jump or irregular source must not keep a stale prediction.
        // Extra compositions can outnumber source updates without a phase jump.
        // Counted in updates: kept three stream periods, the old phase of a
        // 120 Hz game whose frames moved one 240 Hz refresh held a claim to
        // the phase bound, showing a frame 5.4 ms old.
        if self.last_phase.is_some_and(|last| {
            at.duration_since(last) >= Duration::from_nanos(self.cycle_ns()) * 3
        }) {
            self.restart();
            self.origin = Some(at);
            phase = 0;
        }
        self.last = Some(at);
        self.samples[self.next] = phase;
        self.elapsed_ns[self.next] = at
            .duration_since(self.origin.unwrap())
            .as_nanos()
            .min(u128::from(u64::MAX)) as u64;
        self.next = (self.next + 1) % self.samples.len();
        self.count = (self.count + 1).min(self.samples.len());
        let previous = self.center.take();
        self.last_phase = None;
        self.learn(previous);
    }
    /// Start learning from scratch, for the same stream period, multiple and
    /// limit.
    pub(super) fn restart(&mut self) {
        let (multiple, limit, limit_at) = (self.multiple, self.limit, self.limit_at);
        *self = Self::new(Duration::from_nanos(self.period_ns));
        (self.multiple, self.limit, self.limit_at) = (multiple, limit, limit_at);
    }
    /// Find the center from the samples, keeping `previous` while the source
    /// still updates near it.
    pub(super) fn learn(&mut self, previous: Option<u64>) {
        if self.count < 32 {
            return;
        }
        // Retain a learned source while it still updates in most stream cycles;
        // counting all captures would let extra compositions dilute its support.
        // A larger multiple the limit allows replaces it.
        let mut retained =
            previous.filter(|&center| self.multiple <= self.limit && self.covers_cycles(center));
        if retained.is_none()
            && let Some(center) = previous
        {
            retained = self.lower_multiple(center);
        }
        let multiple = self.multiple;
        let least = if retained.is_some() { multiple + 1 } else { 2 };
        let center = match (self.multiple_from(least), retained) {
            (Some(center), _) => center,
            (None, Some(center)) => {
                self.fold(multiple);
                center
            }
            (None, None) => {
                // The phase over the whole period, as before.
                self.fold(1);
                let (center, population) = self.peak();
                if population * 100 < self.count * 65 {
                    return;
                }
                center
            }
        };
        let cycle = self.cycle_ns();
        let half = i128::from(cycle / 2);
        let period = i128::from(cycle);
        let radius = i128::from(self.radius_ns());
        let mut sum = 0i128;
        let mut count = 0i128;
        for &sample in &self.samples[..self.count] {
            let delta = (i128::from(sample) - i128::from(center) + half).rem_euclid(period) - half;
            if delta.abs() <= radius {
                sum += delta;
                count += 1;
            }
        }
        if count > 0 {
            let center = (i128::from(center) + sum / count).rem_euclid(period) as u64;
            self.center = Some(center);
            self.last_phase = self.samples[..self.count]
                .iter()
                .zip(&self.elapsed_ns[..self.count])
                .filter(|&(sample, _)| self.distance(*sample, center) <= self.radius_ns())
                .map(|(_, &elapsed)| self.origin.unwrap() + Duration::from_nanos(elapsed))
                .max();
        }
    }
}
