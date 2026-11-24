//! Fractional V-Sync: how many refreshes each picture of a V-Sync stream stays on a display that
//! refreshes at a whole multiple of the stream's rate (60 fps on 120 Hz: 2). Shown for exactly that
//! many refreshes, a picture that comes a little early or late no longer shows the one before it
//! for one refresh more or less. Android's `FramePacingPolicy.refreshesPerFrame` is its Java port.

/// How far a refresh may be from a whole multiple of the stream's rate and still count as one
/// (59.94 against 60), per multiple.
pub const RATE_TOLERANCE_HZ: f64 = 0.5;

/// The most refreshes a picture is held for: the swap chains' sync interval goes up to 4.
pub const MAX_REFRESHES: u32 = 4;

/// The multiple of `fps` that `refresh_hz` is (1 for the same rate, 2 for twice it, up to 4), or
/// None when it is none of them.
pub fn rate_multiple(refresh_hz: f64, fps: f64) -> Option<u32> {
    if fps.is_nan() || refresh_hz.is_nan() || fps <= 0.0 || refresh_hz <= 0.0 {
        return None;
    }
    (1..=MAX_REFRESHES)
        .find(|&k| (refresh_hz - fps * f64::from(k)).abs() <= RATE_TOLERANCE_HZ * f64::from(k))
}

/// The refreshes each picture is shown for: the multiple the display's refresh is of the stream's
/// rate with fractional V-Sync on (`fractional`), else 1. A VRR display refreshes when a picture
/// comes, so it takes 1 too, as does a refresh that is no whole multiple (75 Hz for 60 fps).
pub fn refreshes_per_frame(refresh_hz: f64, fps: f64, vrr: bool, fractional: bool) -> u32 {
    if !fractional || vrr {
        return 1;
    }
    rate_multiple(refresh_hz, fps).unwrap_or(1)
}

/// The longest period a phase report carries (`rubylight_protocol::phase_lock::Report`: 24 Hz), ns.
pub const MAX_REPORT_PERIOD_NS: u64 = 41_700_000;

/// The refreshes a held stream's phase report counts its slack over: the hold, or one refresh when
/// the hold is longer than a report's period may be (20 fps held 3 refreshes of 60 Hz, 15 fps held
/// 4), which the host would drop. `refresh_period_ns` is the display's. Android's
/// `FramePacingPolicy.heldReportGrid` takes the same rule.
pub fn report_refreshes(held: u32, refresh_period_ns: u64) -> u32 {
    if held > 1 && u64::from(held).saturating_mul(refresh_period_ns) > MAX_REPORT_PERIOD_NS {
        1
    } else {
        held
    }
}

pub const HOST_GRID_LATCH_ROOM_NS: i64 = 1_000_000;
pub const HOST_GRID_LATCH_ROOM_US: i64 = 1_000;
pub const MAX_QUEUED_FRAME_NS: u64 = 250_000_000;
pub const MAX_QUEUED_FRAME_US: u64 = 250_000;
pub const EARLY_FOR_NS: u64 = 2_000_000_000;
pub const EARLY_FOR_US: u64 = 2_000_000;
pub const EARLY_SPARE_NS: i64 = 3_000_000;
pub const EARLY_SPARE_US: u64 = 3_000;

pub fn first_vsync_ns(ready_ns: i64, vsync_ns: i64, interval_ns: i64, lead_ns: i64) -> i64 {
    if vsync_ns <= 0 || interval_ns <= 0 {
        return 0;
    }
    let diff = i128::from(ready_ns) + i128::from(lead_ns.max(0)) - i128::from(vsync_ns);
    let intervals = (diff.div_euclid(i128::from(interval_ns)) + 1).max(1);
    (i128::from(vsync_ns) + intervals * i128::from(interval_ns))
        .clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

pub fn next_vsync_time_ns(now_ns: i64, vsync_ns: i64, interval_ns: i64, deadline_ns: i64) -> i64 {
    if vsync_ns == 0 || interval_ns <= 0 {
        now_ns
    } else {
        first_vsync_ns(now_ns, vsync_ns, interval_ns, deadline_ns)
    }
}

pub fn latch_slack_ns(ready_ns: i64, vsync_ns: i64, interval_ns: i64, deadline_ns: i64) -> i64 {
    if vsync_ns == 0 || interval_ns <= 0 {
        return -1;
    }
    next_vsync_time_ns(ready_ns, vsync_ns, interval_ns, deadline_ns)
        .saturating_sub(deadline_ns)
        .saturating_sub(ready_ns)
}

pub fn held_target_ns(
    earliest_ns: i64,
    last_target_ns: i64,
    vsync_ns: i64,
    interval_ns: i64,
    refreshes: u32,
    every_frame: bool,
) -> i64 {
    if refreshes <= 1 || interval_ns <= 0 || last_target_ns == 0 {
        return earliest_ns;
    }
    let interval = i128::from(interval_ns);
    let offset =
        i128::from(last_target_ns) + i128::from(refreshes) * interval - i128::from(vsync_ns);
    let snapped = i128::from(vsync_ns) + (offset + interval / 2).div_euclid(interval) * interval;
    let span = if every_frame {
        refreshes
    } else {
        refreshes - 1
    };
    let latest = i128::from(earliest_ns) + i128::from(span) * interval;
    snapped
        .min(latest)
        .max(i128::from(earliest_ns))
        .clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

pub fn early_for_turn(release_ns: i64, turn_ns: i64, interval_ns: i64, deadline_ns: i64) -> bool {
    interval_ns > 0
        && i128::from(turn_ns)
            - i128::from(interval_ns)
            - i128::from(deadline_ns)
            - i128::from(release_ns)
            >= i128::from(EARLY_SPARE_NS)
}

pub fn held_report_grid(
    vsync_ns: i64,
    interval_ns: i64,
    anchor_ns: i64,
    refreshes: u32,
) -> (i64, i64) {
    if refreshes <= 1
        || anchor_ns == 0
        || interval_ns <= 0
        || report_refreshes(refreshes, interval_ns as u64) <= 1
    {
        return (vsync_ns, interval_ns);
    }
    let period_ns = interval_ns.saturating_mul(i64::from(refreshes));
    (
        anchor_ns.saturating_sub(4_i64.saturating_mul(period_ns)),
        period_ns,
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HoldStampState {
    pub held_target_ns: i64,
    pub held_early_since_ns: i64,
    pub held_anchor_ns: i64,
    pub held_anchor_refreshes: u32,
}

impl Default for HoldStampState {
    fn default() -> Self {
        Self {
            held_target_ns: 0,
            held_early_since_ns: 0,
            held_anchor_ns: 0,
            held_anchor_refreshes: 1,
        }
    }
}

impl HoldStampState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.held_target_ns = 0;
        self.held_early_since_ns = 0;
        self.held_anchor_refreshes = 1;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn step(
        &mut self,
        release_ns: i64,
        vsync_ns: i64,
        interval_ns: i64,
        deadline_ns: i64,
        refreshes: u32,
        every_frame: bool,
        output_vsync_available: bool,
    ) -> i64 {
        if refreshes <= 1 || vsync_ns == 0 || interval_ns <= 0 || !output_vsync_available {
            self.reset();
            return 0;
        }
        let earliest_ns = next_vsync_time_ns(release_ns, vsync_ns, interval_ns, deadline_ns);
        let mut target_ns = held_target_ns(
            earliest_ns,
            self.held_target_ns,
            vsync_ns,
            interval_ns,
            refreshes,
            every_frame,
        );
        if early_for_turn(release_ns, target_ns, interval_ns, deadline_ns) {
            if self.held_early_since_ns == 0 {
                self.held_early_since_ns = release_ns;
            } else if i128::from(release_ns) - i128::from(self.held_early_since_ns)
                >= i128::from(EARLY_FOR_NS)
            {
                target_ns = target_ns.saturating_sub(interval_ns);
                self.held_early_since_ns = 0;
            }
        } else {
            self.held_early_since_ns = 0;
        }
        self.held_target_ns = target_ns;
        self.held_anchor_ns = target_ns;
        self.held_anchor_refreshes = refreshes;
        target_ns - interval_ns / 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hold_longer_than_a_reports_period_reports_over_a_refresh() {
        assert_eq!(report_refreshes(2, 8_333_333), 2, "60 fps on 120 Hz");
        assert_eq!(report_refreshes(3, 8_333_333), 3, "40 fps on 120 Hz");
        assert_eq!(report_refreshes(2, 16_666_667), 2, "30 fps on 60 Hz: 33 ms");
        assert_eq!(report_refreshes(3, 16_666_667), 1, "20 fps on 60 Hz: 50 ms");
        assert_eq!(report_refreshes(4, 16_666_667), 1, "15 fps on 60 Hz: 67 ms");
        assert_eq!(report_refreshes(1, 50_000_000), 1);
    }

    #[test]
    fn whole_multiples_hold_a_picture_that_many_refreshes() {
        assert_eq!(refreshes_per_frame(120.0, 60.0, false, true), 2);
        assert_eq!(refreshes_per_frame(120.0, 40.0, false, true), 3);
        assert_eq!(refreshes_per_frame(120.0, 30.0, false, true), 4);
        assert_eq!(refreshes_per_frame(240.0, 60.0, false, true), 4);
        assert_eq!(refreshes_per_frame(60.0, 30.0, false, true), 2);
        assert_eq!(
            refreshes_per_frame(120.0, 120.0, false, true),
            1,
            "the display's own rate"
        );
    }

    #[test]
    fn near_rates_count_within_the_tolerance() {
        assert_eq!(refreshes_per_frame(119.88, 60.0, false, true), 2);
        assert_eq!(refreshes_per_frame(119.88, 59.94, false, true), 2);
        assert_eq!(refreshes_per_frame(143.9, 72.0, false, true), 2);
        assert_eq!(
            refreshes_per_frame(121.5, 60.0, false, true),
            1,
            "1.5 Hz off twice the rate is no multiple"
        );
        assert_eq!(refreshes_per_frame(118.9, 60.0, false, true), 1);
    }

    #[test]
    fn no_multiple_vrr_or_off_shows_each_picture_at_the_next_refresh() {
        assert_eq!(refreshes_per_frame(75.0, 60.0, false, true), 1);
        assert_eq!(refreshes_per_frame(144.0, 60.0, false, true), 1);
        assert_eq!(
            refreshes_per_frame(240.0, 50.0, false, true),
            1,
            "more than 4 refreshes a picture"
        );
        assert_eq!(refreshes_per_frame(120.0, 60.0, true, true), 1, "VRR");
        assert_eq!(refreshes_per_frame(120.0, 60.0, false, false), 1, "off");
        assert_eq!(
            refreshes_per_frame(0.0, 60.0, false, true),
            1,
            "unknown refresh"
        );
        assert_eq!(
            refreshes_per_frame(120.0, 0.0, false, true),
            1,
            "unknown rate"
        );
        assert_eq!(refreshes_per_frame(f64::NAN, 60.0, false, true), 1);
    }

    #[test]
    fn rate_multiple_finds_the_smallest_whole_multiple() {
        assert_eq!(rate_multiple(60.0, 60.0), Some(1));
        assert_eq!(rate_multiple(59.94, 60.0), Some(1));
        assert_eq!(rate_multiple(120.0, 60.0), Some(2));
        assert_eq!(rate_multiple(100.0, 60.0), None);
    }

    #[test]
    fn grid_held_target_early_for_turn_and_hold_stamp_match_java() {
        let (interval, vsync, deadline) = (8_333_333_i64, 1_000_000_000_i64, 2_000_000_i64);
        assert_eq!(
            first_vsync_ns(vsync - 500_000, vsync, interval, 0),
            vsync + interval
        );
        assert_eq!(first_vsync_ns(vsync, 0, interval, deadline), 0);
        assert_eq!(
            next_vsync_time_ns(vsync + 1_000, 0, interval, deadline),
            vsync + 1_000
        );
        assert_eq!(latch_slack_ns(vsync + 1_000, 0, interval, deadline), -1);
        assert_eq!(
            latch_slack_ns(vsync + 5_000_000, vsync, 10_000_000, deadline),
            3_000_000
        );

        let last = vsync + 10 * interval;
        let earliest = last + 6 * interval;
        assert_eq!(
            held_target_ns(last + interval, last + 3_000, vsync, interval, 2, false),
            last + 2 * interval
        );
        let a = held_target_ns(earliest, last, vsync, interval, 2, false);
        let b = held_target_ns(earliest, a, vsync, interval, 2, false);
        assert_eq!(
            (a, b, held_target_ns(earliest, b, vsync, interval, 2, false)),
            (earliest, earliest + interval, earliest + interval)
        );
        let ea = held_target_ns(earliest, last, vsync, interval, 2, true);
        let eb = held_target_ns(earliest, ea, vsync, interval, 2, true);
        assert_eq!(
            (
                ea,
                eb,
                held_target_ns(earliest, eb, vsync, interval, 2, true)
            ),
            (earliest, earliest + 2 * interval, earliest + 2 * interval)
        );

        assert!(early_for_turn(83_000_000, 100_000_000, interval, 5_000_000));
        assert!(!early_for_turn(
            84_000_000,
            100_000_000,
            interval,
            5_000_000
        ));
        assert_eq!(
            held_report_grid(500, interval, 9_000_000_000, 1),
            (500, interval)
        );
        assert_eq!(
            held_report_grid(500, interval, 9_000_000_000, 2),
            (9_000_000_000 - 8 * interval, 2 * interval)
        );
        assert_eq!(
            held_report_grid(500, 16_666_667, 9_000_000_000, 3),
            (500, 16_666_667)
        );

        let mut state = HoldStampState::default();
        assert_eq!(state.held_anchor_refreshes, 1);
        let s0 = state.step(
            vsync + 3 * interval + 1_000_000,
            vsync,
            interval,
            deadline,
            2,
            true,
            true,
        );
        assert_eq!(
            (s0, state.held_anchor_refreshes),
            (
                next_vsync_time_ns(vsync + 3 * interval + 1_000_000, vsync, interval, deadline)
                    - interval / 2,
                2
            )
        );
        let r_early1 = state.held_target_ns - interval - deadline - EARLY_SPARE_NS;
        let _ = state.step(r_early1, vsync, interval, deadline, 2, true, true);
        assert_eq!(state.held_early_since_ns, r_early1);
        let r_early2 = r_early1 + EARLY_FOR_NS as i64;
        let s2 = state.step(
            r_early2,
            r_early2 - interval,
            interval,
            deadline,
            2,
            true,
            true,
        );
        assert_eq!(
            (state.held_early_since_ns, s2),
            (0, state.held_anchor_ns - interval / 2)
        );
        assert_eq!(
            state.step(
                r_early2,
                r_early2 - interval,
                interval,
                deadline,
                1,
                true,
                true
            ),
            0
        );
        assert_eq!((state.held_anchor_refreshes, state.held_target_ns), (1, 0));
    }
}
