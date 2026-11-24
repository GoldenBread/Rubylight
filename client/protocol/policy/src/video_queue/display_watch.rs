//! Display queue watch (Nightjar issue 7) and deadline present scheduler lateness window /
//! counters.

/// Lowest latency and Balanced: detects a buffer too many in the display's queue (Nightjar issue
/// 7) from render callback waits and drops one frame so the display gives the buffer back.
#[derive(Debug, Clone)]
pub struct DisplayQueueWatch {
    enabled: bool,
    refresh_ns: i64,

    waits: [i64; 512],
    count: usize,
    second_start_ns: i64,
    medians: [i64; Self::LEVEL_SECONDS],
    median_count: usize,

    floor_ns: i64,
    floor_known: bool,
    over_seconds: i32,
    steady_seconds: i32,
    steady_level_ns: i64,

    drop_due: bool,
    probe: bool,
    drop_level_ns: i64,
    checking: bool,
    checking_probe: bool,
    check_from_ns: i64,
    last_drop_ns: i64,
    drops: i32,
    misses: i32,
    off: bool,

    last_render_ns: i64,
    replaced: i32,
    replaced_logged_ns: i64,
    event: Option<String>,
}

impl DisplayQueueWatch {
    pub const SECOND_NS: i64 = 1_000_000_000;
    pub const MIN_FRAMES: usize = 10;
    pub const LEVEL_SECONDS: usize = 3;
    pub const SUSTAIN_SECONDS: i32 = 3;
    pub const MARGIN_NS: i64 = 1_000_000;
    pub const STEADY_SECONDS: i32 = 5;
    pub const PROBE_REFRESHES: f64 = 2.5;
    pub const MIN_GAP_NS: i64 = 10 * Self::SECOND_NS;
    pub const MAX_DROPS: i32 = 12;
    pub const MAX_MISSES: i32 = 2;
    pub const MAX_WAIT_NS: i64 = 500_000_000;
    pub const REPLACED_LOG_NS: i64 = 10 * Self::SECOND_NS;

    pub fn new() -> Self {
        Self {
            enabled: false,
            refresh_ns: 0,
            waits: [0; 512],
            count: 0,
            second_start_ns: 0,
            medians: [0; Self::LEVEL_SECONDS],
            median_count: 0,
            floor_ns: i64::MAX,
            floor_known: false,
            over_seconds: 0,
            steady_seconds: 0,
            steady_level_ns: 0,
            drop_due: false,
            probe: false,
            drop_level_ns: 0,
            checking: false,
            checking_probe: false,
            check_from_ns: 0,
            last_drop_ns: 0,
            drops: 0,
            misses: 0,
            off: false,
            last_render_ns: 0,
            replaced: 0,
            replaced_logged_ns: 0,
            event: None,
        }
    }

    pub fn applies(
        setting_on: bool,
        latency_or_balanced: bool,
        scheduled: bool,
        render_callbacks: bool,
        vrr: bool,
        stream_frame_rate_milli_hz: i32,
        refresh_ns: i64,
    ) -> bool {
        if !setting_on
            || !latency_or_balanced
            || scheduled
            || !render_callbacks
            || vrr
            || refresh_ns <= 0
            || stream_frame_rate_milli_hz <= 0
        {
            return false;
        }
        let frame_interval_ns = 1_000_000_000_000_i64 / i64::from(stream_frame_rate_milli_hz);
        i128::from(frame_interval_ns) <= i128::from(refresh_ns) + i128::from(refresh_ns / 50)
    }

    pub fn configure(&mut self, enabled: bool, refresh_ns: i64) {
        if enabled == self.enabled && refresh_ns == self.refresh_ns {
            return;
        }
        let was_enabled = self.enabled;
        self.enabled = enabled;
        self.refresh_ns = refresh_ns;
        self.reset();
        if enabled && was_enabled {
            self.event = Some(format!(
                "Display queue: refresh now {:.2} ms, starting over",
                refresh_ns as f64 / 1e6
            ));
        }
    }

    pub fn on_rendered(&mut self, wait_ns: i64, render_ns: i64) {
        if !self.enabled {
            return;
        }
        if self.last_render_ns != 0 && render_ns <= self.last_render_ns {
            self.replaced += 1;
            if self.replaced_logged_ns == 0
                || i128::from(render_ns) - i128::from(self.replaced_logged_ns)
                    >= i128::from(Self::REPLACED_LOG_NS)
            {
                self.replaced_logged_ns = render_ns;
                self.event = Some(format!(
                    "Display queue: a frame reported {:.1} ms before the previous one (wait {:.1} ms) was replaced in the display's queue ({} this stream)",
                    (i128::from(self.last_render_ns) - i128::from(render_ns)) as f64 / 1e6,
                    wait_ns as f64 / 1e6,
                    self.replaced
                ));
            }
            return;
        }
        self.last_render_ns = render_ns;
        if !(0..=Self::MAX_WAIT_NS).contains(&wait_ns) {
            return;
        }
        if self.count == 0 {
            self.second_start_ns = render_ns;
        }
        if self.count < self.waits.len() {
            self.waits[self.count] = wait_ns;
            self.count += 1;
        }
        if i128::from(render_ns) - i128::from(self.second_start_ns) >= i128::from(Self::SECOND_NS) {
            if self.count >= Self::MIN_FRAMES {
                let mut sorted = [0i64; 512];
                sorted[..self.count].copy_from_slice(&self.waits[..self.count]);
                sorted[..self.count].sort_unstable();
                let second_median = sorted[self.count / 2];
                self.on_second(second_median, render_ns);
            }
            self.count = 0;
        }
    }

    pub fn take_drop(&mut self, now_ns: i64) -> bool {
        if !self.enabled || !self.drop_due {
            return false;
        }
        self.drop_due = false;
        self.drops += 1;
        self.last_drop_ns = now_ns;
        self.checking = true;
        self.checking_probe = self.probe;
        self.check_from_ns = self.drop_level_ns;
        self.count = 0;
        self.median_count = 0;
        true
    }

    pub fn take_event(&mut self) -> Option<String> {
        self.event.take()
    }

    fn reset(&mut self) {
        self.count = 0;
        self.median_count = 0;
        self.floor_ns = i64::MAX;
        self.floor_known = false;
        self.over_seconds = 0;
        self.steady_seconds = 0;
        self.steady_level_ns = 0;
        self.drop_due = false;
        self.checking = false;
        self.last_drop_ns = 0;
        self.drops = 0;
        self.misses = 0;
        self.off = false;
        self.last_render_ns = 0;
        self.replaced = 0;
        self.replaced_logged_ns = 0;
    }

    fn on_second(&mut self, second_median_ns: i64, now_ns: i64) {
        self.medians[self.median_count % Self::LEVEL_SECONDS] = second_median_ns;
        self.median_count += 1;
        if self.median_count < Self::LEVEL_SECONDS || self.off {
            return;
        }
        let mut sorted = self.medians;
        sorted.sort_unstable();
        let level = sorted[Self::LEVEL_SECONDS / 2];
        if self.checking {
            self.checking = false;
            if i128::from(level) <= i128::from(self.check_from_ns) - i128::from(self.refresh_ns / 2)
            {
                self.event = Some(format!(
                    "Display queue: wait {:.1} -> {:.1} ms after one dropped frame: the display held a buffer too many",
                    self.check_from_ns as f64 / 1e6,
                    level as f64 / 1e6
                ));
                self.floor_ns = level;
                self.floor_known = false;
            } else {
                self.floor_ns = level;
                self.floor_known = true;
                if !self.checking_probe {
                    self.misses += 1;
                }
                if self.misses >= Self::MAX_MISSES {
                    self.off = true;
                }
                let suffix = if self.off {
                    "; this display's wait does not follow its queue, no more drops this stream"
                } else {
                    ""
                };
                self.event = Some(format!(
                    "Display queue: wait {:.1} -> {:.1} ms after one dropped frame: no buffer too many{}",
                    self.check_from_ns as f64 / 1e6,
                    level as f64 / 1e6,
                    suffix
                ));
            }
            self.over_seconds = 0;
            self.steady_seconds = 0;
            self.steady_level_ns = level;
            return;
        }
        self.floor_ns = self.floor_ns.min(level);
        self.over_seconds = if i128::from(level)
            >= i128::from(self.floor_ns) + i128::from(self.refresh_ns) + i128::from(Self::MARGIN_NS)
        {
            self.over_seconds + 1
        } else {
            0
        };
        if (i128::from(level) - i128::from(self.steady_level_ns)).abs()
            <= i128::from(self.refresh_ns / 4)
        {
            self.steady_seconds += 1;
        } else {
            self.steady_seconds = 0;
            self.steady_level_ns = level;
        }

        let (why, as_probe) = if self.over_seconds >= Self::SUSTAIN_SECONDS {
            (
                format!(
                    "{:.1} ms over its lowest level, {:.1} ms",
                    (i128::from(level) - i128::from(self.floor_ns)) as f64 / 1e6,
                    self.floor_ns as f64 / 1e6
                ),
                false,
            )
        } else if !self.floor_known
            && self.steady_seconds >= Self::STEADY_SECONDS
            && (level as f64) >= Self::PROBE_REFRESHES * (self.refresh_ns as f64)
        {
            (
                format!(
                    "steady at {:.1} refreshes, its own level not known yet",
                    level as f64 / self.refresh_ns as f64
                ),
                true,
            )
        } else {
            return;
        };

        if self.drops >= Self::MAX_DROPS
            || self.drop_due
            || (self.last_drop_ns != 0
                && i128::from(now_ns) - i128::from(self.last_drop_ns)
                    < i128::from(Self::MIN_GAP_NS))
        {
            return;
        }
        self.drop_due = true;
        self.probe = as_probe;
        self.drop_level_ns = level;
        self.event = Some(format!(
            "Display queue: wait {:.1} ms ({}; refresh {:.2} ms): dropping one frame so the display gives back a buffer",
            level as f64 / 1e6,
            why,
            self.refresh_ns as f64 / 1e6
        ));
    }
}

impl Default for DisplayQueueWatch {
    fn default() -> Self {
        Self::new()
    }
}

/// Lateness window (128-sample p97 + margin) and diagnostic counters for [`PresentScheduler`].
#[derive(Debug, Clone)]
pub struct PresentSchedulerState {
    lateness: [i64; Self::WINDOW],
    count: usize,
    next: usize,
    first_host_us: i64,
    last_host_us: i64,
    late_dropped: i64,
    governor_dropped: i64,
    cap_dropped: i64,
    cap_held: i64,
    planned: i64,
    shared: i64,
}

impl PresentSchedulerState {
    pub const WINDOW: usize = 128;
    pub const PERCENTILE: usize = 97;
    pub const MARGIN_US: i64 = 500;
    pub const MIN_SAMPLES: usize = 32;
    pub const WARMUP_US: i64 = 250_000;
    const DISCONTINUITY_US: i64 = 10_000_000;

    pub fn new() -> Self {
        Self {
            lateness: [0; Self::WINDOW],
            count: 0,
            next: 0,
            first_host_us: i64::MIN,
            last_host_us: i64::MIN,
            late_dropped: 0,
            governor_dropped: 0,
            cap_dropped: 0,
            cap_held: 0,
            planned: 0,
            shared: 0,
        }
    }

    /// Forget the lateness window, for example after a decoder flush; counters stay.
    pub fn reset(&mut self) {
        self.count = 0;
        self.next = 0;
        self.first_host_us = i64::MIN;
        self.last_host_us = i64::MIN;
    }

    /// Notes a frame whose host time is `host_us` (non-negative) and ready time is `ready_ns`, and
    /// returns whether it is behind the game's cadence by more than half of `content_interval_ns`.
    pub fn is_late(&mut self, host_us: i64, ready_ns: i64, content_interval_ns: i64) -> bool {
        if host_us < 0 {
            return false;
        }
        let behind_us = (ready_ns / 1000).wrapping_sub(host_us);
        if self.last_host_us != i64::MIN
            && (i128::from(host_us) - i128::from(self.last_host_us)).abs()
                > i128::from(Self::DISCONTINUITY_US)
        {
            self.reset();
        }
        self.last_host_us = host_us;
        if self.first_host_us == i64::MIN {
            self.first_host_us = host_us;
        }
        let mut late = false;
        if self.count >= Self::MIN_SAMPLES
            && i128::from(host_us) - i128::from(self.first_host_us) >= i128::from(Self::WARMUP_US)
        {
            let delay_us = i128::from(self.percentile()) + i128::from(Self::MARGIN_US);
            late = i128::from(behind_us) - delay_us > i128::from(content_interval_ns / 2000);
        }
        self.lateness[self.next] = behind_us;
        self.next = (self.next + 1) % Self::WINDOW;
        self.count = (self.count + 1).min(Self::WINDOW);
        late
    }

    /// Whether a late frame is replaced by the newer output already decoded. Never twice in a row.
    pub fn drop_late(late: bool, newer_available: bool, previous_dropped: bool) -> bool {
        late && newer_available && !previous_dropped
    }

    pub fn on_planned(&mut self, shares_vsync: bool) {
        self.planned += 1;
        if shares_vsync {
            self.shared += 1;
        }
    }

    pub fn on_late_dropped(&mut self) {
        self.late_dropped += 1;
    }

    pub fn on_governor_dropped(&mut self) {
        self.governor_dropped += 1;
    }

    pub fn on_cap_dropped(&mut self) {
        self.cap_dropped += 1;
    }

    pub fn on_cap_held(&mut self) {
        self.cap_held += 1;
    }

    pub fn late_dropped(&self) -> i64 {
        self.late_dropped
    }

    pub fn governor_dropped(&self) -> i64 {
        self.governor_dropped
    }

    pub fn cap_dropped(&self) -> i64 {
        self.cap_dropped
    }

    pub fn cap_held(&self) -> i64 {
        self.cap_held
    }

    pub fn overlay_line(&self) -> Option<String> {
        if self.planned == 0 {
            return None;
        }
        Some(format!(
            "Scheduler: shared vsync {:.1}% | late dropped {} | queue dropped {} | compositor held {}",
            100.0 * self.shared as f64 / self.planned as f64,
            self.late_dropped,
            self.governor_dropped + self.cap_dropped,
            self.cap_held
        ))
    }

    fn percentile(&self) -> i64 {
        let mut sorted = [0i64; Self::WINDOW];
        sorted[..self.count].copy_from_slice(&self.lateness[..self.count]);
        sorted[..self.count].sort_unstable();
        sorted[(self.count - 1) * Self::PERCENTILE / 100]
    }
}

impl Default for PresentSchedulerState {
    fn default() -> Self {
        Self::new()
    }
}
