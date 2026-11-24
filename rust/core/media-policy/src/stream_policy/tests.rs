//! Shared pacing simulation helpers and test module hub for stream policy.

use super::*;
use std::time::Instant;

/// Claims (claim time, presented time) in milliseconds for source frames
/// presented at `source` and observed `detect` milliseconds later.
fn simulate(period: f64, source: &[f64], detect: impl Fn(usize) -> f64) -> Vec<(f64, f64)> {
    simulate_with(period, source, detect, |pacer| pacer)
}
fn simulate_with(
    period: f64,
    source: &[f64],
    detect: impl Fn(usize) -> f64,
    configure: impl Fn(Pacer) -> Pacer,
) -> Vec<(f64, f64)> {
    let start = Instant::now();
    let at = |ms: f64| start + Duration::from_secs_f64(ms / 1000.);
    let ms = |instant: Instant| instant.duration_since(start).as_secs_f64() * 1000.;
    let mut pacer = configure(Pacer::new(start, Duration::from_secs_f64(period / 1000.)));
    let mut claims = vec![];
    let mut newest: Option<f64> = None;
    let mut deadline: Option<f64> = None;
    let mut interval: Option<Duration> = None;
    let mut evaluate = |now: f64,
                        newest: &mut Option<f64>,
                        deadline: &mut Option<f64>,
                        pacer: &mut Pacer,
                        interval: Option<Duration>| {
        if let Some(presented) = *newest {
            match pacer.decide(at(now), at(presented), interval) {
                Pace::Claim => {
                    pacer.claimed(at(now));
                    claims.push((now, presented));
                    *newest = None;
                    *deadline = None;
                }
                Pace::WaitUntil(t) => *deadline = Some(ms(t).max(now + 0.001)),
            }
        }
    };
    for (index, &presented) in source.iter().enumerate() {
        let detected = presented + detect(index);
        while let Some(t) = deadline.filter(|t| *t < detected) {
            evaluate(t, &mut newest, &mut deadline, &mut pacer, interval);
            if deadline == Some(t) {
                break;
            }
        }
        // The capture worker sees every frame: median of recent intervals.
        let recent = &source[index.saturating_sub(16)..=index];
        let mut intervals: Vec<f64> = recent.windows(2).map(|w| w[1] - w[0]).collect();
        intervals.sort_by(f64::total_cmp);
        interval = (intervals.len() >= 4)
            .then(|| Duration::from_secs_f64(intervals[intervals.len() / 2] / 1000.));
        newest = Some(presented);
        pacer.observe_source(at(presented));
        evaluate(detected, &mut newest, &mut deadline, &mut pacer, interval);
    }
    claims
}
/// Like `simulate_with`, but the pacer's inputs come from the capture
/// worker's `Freshness`, as in the host: the picture interval, start and late
/// interval, and the mean interval as the source rate. Captures are
/// published in order.
fn simulate_captures(
    period: f64,
    captures: &[f64],
    detect: impl Fn(usize) -> f64,
    configure: impl Fn(Pacer) -> Pacer,
) -> Vec<(f64, f64)> {
    let unlabelled: Vec<(f64, Option<bool>)> = captures.iter().map(|c| (*c, None)).collect();
    simulate_focus(period, &unlabelled, detect, configure)
}
/// Like `simulate_captures`, with whether each capture changed the focus
/// window (`Freshness::observe_focus`).
fn simulate_focus(
    period: f64,
    labelled: &[(f64, Option<bool>)],
    detect: impl Fn(usize) -> f64,
    configure: impl Fn(Pacer) -> Pacer,
) -> Vec<(f64, f64)> {
    let captures: Vec<f64> = labelled.iter().map(|c| c.0).collect();
    let start = Instant::now();
    let ns = |ms: f64| (ms * 1e6).round() as u64;
    let at = |ms: f64| start + Duration::from_nanos(ns(ms));
    let ms = |instant: Instant| instant.duration_since(start).as_secs_f64() * 1000.;
    let mut pacer = configure(Pacer::new(start, Duration::from_secs_f64(period / 1000.)));
    let mut cadence = crate::capture_policy::Freshness::default();
    let mut claims = vec![];
    let mut newest: Option<f64> = None;
    let mut deadline: Option<f64> = None;
    let mut interval: Option<Duration> = None;
    let mut published = 0f64;
    let mut evaluate = |now: f64,
                        newest: &mut Option<f64>,
                        deadline: &mut Option<f64>,
                        pacer: &mut Pacer,
                        interval: Option<Duration>| {
        if let Some(presented) = *newest {
            match pacer.decide(at(now), at(presented), interval) {
                Pace::Claim => {
                    pacer.claimed(at(now));
                    claims.push((now, presented));
                    *newest = None;
                    *deadline = None;
                }
                Pace::WaitUntil(t) => *deadline = Some(ms(t).max(now + 0.001)),
            }
        }
    };
    for (index, &presented) in captures.iter().enumerate() {
        published = published.max(presented + detect(index));
        while let Some(t) = deadline.filter(|t| *t < published) {
            evaluate(t, &mut newest, &mut deadline, &mut pacer, interval);
            if deadline == Some(t) {
                break;
            }
        }
        cadence.observe_focus(ns(presented), ns(published - presented), labelled[index].1);
        pacer.observe_extension(cadence.extends_picture());
        interval = cadence.picture_interval().map(Duration::from_nanos);
        pacer.observe_source_rate(cadence.mean_interval().map(Duration::from_nanos));
        pacer.observe_picture(
            cadence
                .picture_start()
                .map(|nanos| start + Duration::from_nanos(nanos)),
            cadence.late_picture_interval().map(Duration::from_nanos),
        );
        newest = Some(presented);
        // As the host does: another window's update next to a focus picture
        // is no update of the source.
        if !cadence.extends_picture() {
            pacer.observe_source(at(presented));
        }
        evaluate(published, &mut newest, &mut deadline, &mut pacer, interval);
    }
    claims
}
/// Deterministic noise in [0, 1).
fn noise(i: usize, seed: u64) -> f64 {
    let mut x = (i as u64)
        .wrapping_mul(6364136223846793005)
        .wrapping_add(seed.wrapping_mul(1442695040888963407));
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    (x % 10_000) as f64 / 10_000.
}
fn source(interval: f64, seconds: f64, jitter: impl Fn(usize) -> f64) -> Vec<f64> {
    (0..(seconds * 1000. / interval) as usize)
        .map(|i| i as f64 * interval + jitter(i))
        .collect()
}
const PERIOD: f64 = 1000. / 120.;
/// A 60 Hz game window on the 240 Hz VRR desktop while another window on the
/// same display animates at the display rate. Returns every composition as
/// (time, whether it carries a new game frame or its duplicate) and the times
/// of the compositions that carry each game frame, in milliseconds.
fn mixed_desktop(seconds: f64, other: f64, seed: u64) -> (Vec<(f64, bool)>, Vec<f64>) {
    mixed_desktop_at(1000. / 60., seconds, seconds, other, seed)
}
/// `mixed_desktop` with the game presenting every `game` ms until `still`.
fn mixed_desktop_at(
    game: f64,
    still: f64,
    seconds: f64,
    other: f64,
    seed: u64,
) -> (Vec<(f64, bool)>, Vec<f64>) {
    let mut compositions = vec![];
    let mut frames = vec![];
    for frame in 0..(still * 1000. / game) as usize {
        let composed = frame as f64 * game + 2.5 + noise(frame, seed);
        frames.push(composed);
        compositions.push((composed, true));
        compositions.push((composed + 0.1 + 0.9 * noise(frame, seed + 1), true));
    }
    let mut at = 0.3;
    let mut index = 0;
    while at < seconds * 1000. {
        // An update within half a millisecond of a game frame is composed with it.
        let next = frames.partition_point(|f| *f < at);
        let near = |i: usize| frames.get(i).is_some_and(|f| (f - at).abs() < 0.5);
        if !near(next) && !(next > 0 && near(next - 1)) {
            compositions.push((at, false));
        }
        at += other + 0.3 * (noise(index, seed + 2) - 0.5);
        index += 1;
    }
    compositions.sort_by(|a, b| a.0.total_cmp(&b.0));
    (compositions, frames)
}
/// Game frames shown by claims of `claims` (claim, presented): repeats,
/// skipped frames, and claims one or more frames behind the newest composed.
fn game_frames_shown(claims: &[(f64, f64)], frames: &[f64]) -> (usize, usize, usize) {
    let shown = |at: f64| frames.partition_point(|f| *f <= at + 1e-9);
    let (mut repeats, mut skipped, mut behind) = (0, 0, 0);
    for pair in claims.windows(2) {
        let (previous, current) = (shown(pair[0].1), shown(pair[1].1));
        if current == previous {
            repeats += 1;
        } else if current > previous + 1 {
            skipped += current - previous - 1;
        }
        if shown(pair[1].0) > current {
            behind += 1;
        }
    }
    (repeats, skipped, behind)
}
/// Labels `compositions` from `mixed_desktop` for `Freshness::observe_focus`:
/// `None` without dirty regions, or whether each composition changed the
/// game window. With `duplicate_other`, the duplicate composition of a game
/// frame reports no change in the game window.
fn focus_labels(
    compositions: &[(f64, bool)],
    labelled: bool,
    duplicate_other: bool,
) -> Vec<(f64, Option<bool>)> {
    compositions
        .iter()
        .enumerate()
        .map(|(i, &(t, game))| {
            let duplicate = i > 0 && compositions[i - 1].1 && t - compositions[i - 1].0 <= 1.;
            let focus = game && !(duplicate_other && duplicate);
            (t, labelled.then_some(focus))
        })
        .collect()
}
fn vrr_desktop(pacer: Pacer) -> Pacer {
    pacer
        .with_prediction_floor(Duration::from_millis(2))
        .with_source_phase(true)
        .with_spacing(0.5)
}
fn steady_rate(claims: &[(f64, f64)]) -> f64 {
    let steady = &claims[claims.len() / 10..];
    (steady.len() - 1) as f64 * 1000. / (steady[steady.len() - 1].0 - steady[0].0)
}
fn steady_age_p99(claims: &[(f64, f64)]) -> f64 {
    let mut ages: Vec<f64> = claims[claims.len() / 10..]
        .iter()
        .map(|(claim, presented)| claim - presented)
        .collect();
    ages.sort_by(f64::total_cmp);
    ages[ages.len() * 99 / 100]
}
/// The ANNOUNCE Artemis sends with its 40 Mbps setting: Warp Drive and Warp 2
/// frame pacing multiply the stream rate (maxFPS) by two and four but launch
/// at 60 fps, and the TV's 60 Hz refresh does not match the request.
fn artemis_announce(max_fps: u32) -> Negotiated {
    Negotiated::from_sdp(
        format!(
            "a=x-nv-video[0].clientViewportWd:2560\na=x-nv-video[0].clientViewportHt:1440\n\
             a=x-nv-video[0].maxFPS:{max_fps}\na=x-nv-video[0].clientRefreshRateX100:6000\n\
             a=x-nv-vqos[0].bw.maximumBitrateKbps:31500\na=x-ml-video.configuredBitrateKbps:40000\n\
             a=x-nv-vqos[0].bitStreamFormat:2\na=x-nv-video[0].packetSize:1392\n"
        )
        .as_bytes(),
    )
    .unwrap()
}

mod cadence_and_bitrate;
mod color_and_resends;
mod focus_and_repeat;
mod pacer_arrival;
mod polled_phase;
mod send_faults;
mod source_phase;
