//! Unit tests for [`StreamHealth`] and recorded arrival replay.

use super::*;

fn sample(now_ms: u64, decode_ms: f32) -> Stats {
    Stats {
        now_ms,
        recent_video: true,
        fps: 60,
        loss_percent: Some(0.0),
        jitter_ms: Some(1.0),
        decode_ms: Some(decode_ms),
        host_p95_ms: Some(3.0),
        display_p95_ms: Some(2.0),
        received_fps: Some(60.0),
        released_fps: Some(60.0),
        shown_fps: Some(60.0),
        ..Stats::default()
    }
}

// JSON has no NaN, so the traces cannot carry StreamHealthTest's NaN counters.
#[test]
fn nan_counters_are_not_evidence() {
    let mut health = StreamHealth::default();
    for n in 1..=4 {
        health.add(&sample(n * 1000, f32::NAN));
    }
    let report = health.report(4000);
    assert!(report.findings.is_empty());
    assert_eq!(report.worst_decode_ms, -1.0);
    let mut slow = StreamHealth::default();
    for n in 1..=4 {
        slow.add(&sample(n * 1000, 20.0));
    }
    assert_eq!(slow.report(4000).findings, [Finding::Decoder]);
}

/// Recorded arrivals (desktop Nightjar on a laptop over Wi-Fi 6E against a Rubylight host): the
/// frames' RTP and receive time deltas, replayed as one-second ticks. The reports of each tick.
fn replay(recording: &str) -> Vec<Report> {
    replay_from(recording, 0)
}

/// [`replay`] of the recording without its first `skip_us` of arrivals, as if the stream had
/// started there.
fn replay_from(recording: &str, skip_us: u64) -> Vec<Report> {
    let mut lines = recording.lines().filter(|line| !line.starts_with('#'));
    let fps: u32 = lines.next().and_then(|l| l.strip_prefix("fps ")).unwrap().parse().unwrap();
    let (mut late, mut health) = (crate::stats::LateFrames::default(), StreamHealth::default());
    let (mut rtp, mut at_us, mut tick_us) = (0u32, 1_000_000u64, 2_000_000u64);
    let mut reports = Vec::new();
    let mut skipped = 0u64;
    let mut lines = lines.skip_while(|line| {
        skipped += line.split_once(' ').unwrap().1.parse::<u64>().unwrap();
        skipped <= skip_us
    });
    // The first frame kept starts the stream: its deltas lead from a frame that is gone.
    if skip_us > 0 {
        lines.next();
    }
    for line in lines {
        let (ticks, us) = line.split_once(' ').unwrap();
        rtp = rtp.wrapping_add(ticks.parse::<i64>().unwrap() as u32);
        at_us += us.parse::<u64>().unwrap();
        while at_us >= tick_us {
            let now_ms = tick_us / 1000;
            health.add(&Stats { late_frames: Some(late.tick(None)), fps, ..sample(now_ms, 0.5) });
            reports.push(health.report(now_ms));
            tick_us += 1_000_000;
        }
        late.push(0, rtp, 0, at_us, fps);
    }
    reports
}

#[test]
fn frames_that_wifi_bunches_up_are_reported_as_bursts() {
    // HEVC at 60 fps: 31-50 ms receive gaps about once a second, while the host sent every frame
    // on time. The stream's health line had no findings.
    let reports = replay(include_str!("../../tests/arrivals/wifi-bursts-hevc-60.txt"));
    assert!(reports.len() >= 50, "{}", reports.len());
    for (second, report) in reports.iter().enumerate().skip(14) {
        assert_eq!(report.findings, [Finding::Bursts], "second {second}: {report:?}");
        assert!(report.late_samples >= BURST_MIN_VOTES as usize);
    }
    assert!(reports.last().unwrap().late_samples >= 20, "{:?}", reports.last());
}

#[test]
fn a_launch_early_in_the_stream_is_not_bursts() {
    // The launch recording with its start cut off, so its launch and fullscreen switch fall in
    // the first seconds, where a third of the samples is fewer than BURST_MIN_VOTES.
    let recording = include_str!("../../tests/arrivals/launch-and-fullscreen-hevc-120.txt");
    for skip_ms in (0..=30_000).step_by(500) {
        let reports = replay_from(recording, skip_ms * 1000);
        for (second, report) in reports.iter().enumerate() {
            assert!(!report.findings.contains(&Finding::Bursts), "skip {skip_ms} ms, second {second}: {report:?}");
        }
    }
}

#[test]
fn one_run_of_late_seconds_is_not_bursts_unless_it_goes_on() {
    let late = |health: &mut StreamHealth, from: u64, to: u64, frames: u32| {
        for n in from..=to {
            health.add(&Stats { late_frames: Some(frames), ..sample(n * 1000, 0.5) });
        }
    };
    // Five late seconds in a row right after the start: one transition, two votes.
    let mut start = StreamHealth::default();
    late(&mut start, 1, 2, 0);
    late(&mut start, 3, 7, 14);
    late(&mut start, 8, 15, 0);
    for now in [7_000, 10_000, 15_000] {
        let report = start.report(now);
        assert!(report.findings.is_empty(), "{now}: {report:?}");
    }
    assert_eq!(start.report(15_000).late_samples, 5, "the report still counts every late second");
    // Late seconds without a break are no transition: they vote in full once the run is long.
    let mut steady = StreamHealth::default();
    late(&mut steady, 1, BURST_LONG_RUN as u64, 2);
    assert!(steady.report(BURST_LONG_RUN as u64 * 1000).findings.is_empty());
    late(&mut steady, BURST_LONG_RUN as u64 + 1, BURST_LONG_RUN as u64 + 1, 2);
    assert_eq!(steady.report((BURST_LONG_RUN as u64 + 1) * 1000).findings, [Finding::Bursts]);
    // Three short runs apart are a pattern.
    let mut repeating = StreamHealth::default();
    for start in [1, 5, 9] {
        late(&mut repeating, start, start + 1, 1);
        late(&mut repeating, start + 2, start + 3, 0);
    }
    assert_eq!(repeating.report(12_000).findings, [Finding::Bursts]);
}

#[test]
fn a_launch_and_a_switch_to_fullscreen_are_not_bursts() {
    // 120 fps. The first: the launch at 20 fps of still pictures, the game's switch to
    // fullscreen, and three seconds of frames that came in fives (seconds 29-31). The second:
    // the same arm on a clean run.
    for recording in [
        include_str!("../../tests/arrivals/launch-and-fullscreen-hevc-120.txt"),
        include_str!("../../tests/arrivals/clean-hevc-120.txt"),
    ] {
        let reports = replay(recording);
        assert!(reports.len() >= 50, "{}", reports.len());
        for (second, report) in reports.iter().enumerate() {
            assert!(!report.findings.contains(&Finding::Bursts), "second {second}: {report:?}");
        }
        assert!(reports.iter().any(|r| r.late_samples > 0), "late frames were seen, just not often");
    }
}

#[test]
fn no_frame_rate_means_no_budget() {
    let mut health = StreamHealth::default();
    for n in 1..=4 {
        health.add(&Stats {
            fps: 0,
            loss_percent: Some(50.0),
            ..sample(n * 1000, 20.0)
        });
    }
    let report = health.report(4000);
    assert_eq!((report.samples, report.stalls), (0, 0));
    assert!(report.recent);
}
