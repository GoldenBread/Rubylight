//! Luminance histogram statistics, scene-cut smoothing, and dynamic HDR writer tests.

use super::*;

#[test]
fn histogram_statistics_follow_the_bins() {
    // 990 pixels at PQ 0.25, ten at PQ 0.75: P99 still in the dark bin.
    let mut bins = [0u32; 1024];
    bins[255] = 990;
    bins[767] = 10;
    let stats = FrameStats::from_histogram(&bins, 0.2495, 0.7495).unwrap();
    assert!(stats.is_valid());
    assert_eq!(stats.percentiles_pq[..8], [0.25; 8]);
    assert_eq!(stats.percentiles_pq[8], 0.25, "P99 is pixel 990 of 1000");
    let mean_pq = (990. * 255.5 + 10. * 767.5) / 1000. / 1024.;
    assert!((stats.mean_pq - mean_pq).abs() < 1e-5);
    let mean_nits = (990. * pq_to_nits(255.5 / 1024.) + 10. * pq_to_nits(767.5 / 1024.)) / 1000.;
    assert!(
        (stats.mean_nits - mean_nits).abs() < 0.01,
        "{}",
        stats.mean_nits
    );
    assert!(
        stats.mean_nits > pq_to_nits(stats.mean_pq),
        "linear mean above PQ mean"
    );

    bins[767] = 11;
    let stats = FrameStats::from_histogram(&bins, 0.2495, 0.7495).unwrap();
    assert_eq!(stats.percentiles_pq[8], 0.7495, "kept at the frame maximum");

    let mut single = [0u32; 16];
    single[0] = 4;
    let black = FrameStats::from_histogram(&single, 0., 0.).unwrap();
    assert_eq!(black, FrameStats::default(), "kept inside min and max");

    assert_eq!(FrameStats::from_histogram(&[0; 1024], 0., 1.), None);
    assert_eq!(FrameStats::from_histogram(&bins, 0.5, 0.4), None);
    assert_eq!(FrameStats::from_histogram(&bins, f32::NAN, 1.), None);
}

#[test]
fn the_stand_in_follows_the_static_levels() {
    use crate::hdr::Metadata;
    let near = |nits: f32, expected: f32| (nits - expected).abs() <= expected * 2e-3;
    // MaxCLL first, MaxFALL as the mean.
    let mut metadata = Metadata::display(1000., 0.01, 400.);
    (metadata.max_cll, metadata.max_fall) = (800, 200);
    let stats = FrameStats::from_static(&metadata);
    assert!(stats.is_valid());
    assert_eq!((stats.min_pq, stats.max_pq), (0., nits_to_pq(800.)));
    assert_eq!(stats.percentiles_pq[8], stats.max_pq, "P99 at the peak");
    assert!(stats.percentiles_pq.is_sorted());
    assert_eq!(stats.mean_nits, 200.);
    let hdr10plus = Hdr10Plus::from_stats(&stats).unwrap();
    assert!(
        (hdr10plus.maxscl as i64 - 8000).abs() <= 2,
        "{}",
        hdr10plus.maxscl
    );
    assert_eq!(hdr10plus.average_maxrgb, 2000);

    // Without light levels: the mastering peak, and the spread's own linear mean.
    let stats = FrameStats::from_static(&Metadata::display(600., 0.01, 300.));
    assert!(near(pq_to_nits(stats.max_pq), 600.));
    assert!(stats.mean_nits > 0. && stats.mean_nits < 600.);
    // A MaxFALL above the peak is not taken.
    (metadata.max_cll, metadata.max_fall) = (500, 900);
    assert!(FrameStats::from_static(&metadata).mean_nits < 500.);
    // No peak at all: 1000 nits; one past PQ's range: 10000.
    metadata = Metadata {
        maximum_nits: 0,
        ..Metadata::default()
    };
    assert!(near(
        pq_to_nits(FrameStats::from_static(&metadata).max_pq),
        1000.
    ));
    metadata.max_cll = u16::MAX;
    let stats = FrameStats::from_static(&metadata);
    assert!(stats.is_valid());
    assert_eq!(stats.max_pq, 1.);
}

#[test]
fn target_display_prefers_client_then_host_then_1000() {
    assert_eq!(target_display_nits(Some(800.), Some(1500.)), 800);
    assert_eq!(target_display_nits(None, Some(1500.)), 1500);
    assert_eq!(target_display_nits(Some(0.), Some(1500.)), 1500);
    assert_eq!(target_display_nits(Some(f32::NAN), None), 1000);
    assert_eq!(target_display_nits(None, None), 1000);
    assert_eq!(target_display_nits(Some(40_000.), None), 10_000);
}

#[test]
fn a_picture_that_keeps_jumping_does_not_snap_with_every_jump() {
    let with_mean = |mean_pq: f32| {
        let mut stats = sample();
        stats.mean_pq = mean_pq;
        stats
    };
    let (dark, bright) = (with_mean(0.3), with_mean(0.5));
    let start = Instant::now();
    let at = |millis: u64| start + Duration::from_millis(millis);
    let mut smoother = StatsSmoother::default();
    smoother.update_at(&dark, at(0)).unwrap();
    // The first cut of a stream snaps, right after its first sample too.
    let first = smoother.update_at(&bright, at(8)).unwrap();
    assert!(first.scene_cut);
    assert_eq!(first.stats, bright);

    // Back at once: smoothed, a step of ALPHA instead of the whole way.
    let back = smoother.update_at(&dark, at(16)).unwrap();
    assert!(!back.scene_cut);
    assert!((back.stats.mean_pq - (0.5 - StatsSmoother::ALPHA * 0.2)).abs() < 1e-5);
    assert_eq!(smoother.held_cuts(), 1);

    // The last moment within the gap still holds a jump; samples in between don't move it.
    assert!(!smoother.update_at(&dark, at(500)).unwrap().scene_cut);
    assert!(!smoother.update_at(&bright, at(1007)).unwrap().scene_cut);
    assert_eq!(smoother.held_cuts(), 2);

    // From a second after the cut a jump is a cut again, and it starts the next gap.
    let cut = smoother.update_at(&dark, at(1008)).unwrap();
    assert!(cut.scene_cut);
    assert_eq!(cut.stats, dark);
    assert!(!smoother.update_at(&bright, at(1016)).unwrap().scene_cut);
    assert_eq!(smoother.held_cuts(), 3);

    // Time, not samples: at a slow source's 30 samples a second, cuts a second apart snap.
    let mut slow = StatsSmoother::default();
    slow.update_at(&dark, at(0)).unwrap();
    assert!(slow.update_at(&bright, at(33)).unwrap().scene_cut);
    assert!(slow.update_at(&dark, at(1033)).unwrap().scene_cut);
    assert_eq!(slow.held_cuts(), 0);

    smoother.reset();
    assert_eq!(smoother.held_cuts(), 0);
    smoother.update(&dark).unwrap();
    assert!(
        smoother.update(&bright).unwrap().scene_cut,
        "a reset starts over"
    );
}

#[test]
fn smoothing_follows_slowly_and_snaps_at_a_scene_cut() {
    let mut smoother = StatsSmoother::default();
    let first = smoother.update(&sample()).unwrap();
    assert!(first.scene_cut, "the first sample is taken as it is");
    assert_eq!(first.stats, sample());

    // A small change between samples: smoothed, not a cut.
    let mut brighter = sample();
    brighter.mean_pq += 0.03;
    brighter.percentiles_pq[8] += 0.05;
    let step = smoother.update(&brighter).unwrap();
    assert!(!step.scene_cut);
    let expected = sample().mean_pq + StatsSmoother::ALPHA * 0.03;
    assert!((step.stats.mean_pq - expected).abs() < 1e-6);
    assert!(step.stats.percentiles_pq[8] < brighter.percentiles_pq[8]);

    // A lone peak (cursor, subtitle) is not a cut either.
    let mut peak = brighter;
    peak.max_pq = 1.;
    assert!(!smoother.update(&peak).unwrap().scene_cut);

    // Cut to a dark scene: snaps to the new statistics.
    let dark = FrameStats {
        min_pq: 0.,
        mean_pq: 0.1,
        max_pq: 0.3,
        mean_nits: 0.5,
        percentiles_pq: [0.02, 0.03, 0.04, 0.06, 0.1, 0.14, 0.2, 0.25, 0.3],
    };
    let cut = smoother.update(&dark).unwrap();
    assert!(cut.scene_cut);
    assert_eq!(cut.stats, dark);

    // Invalid samples change nothing: neither the smoothed state nor the cut reference.
    let mut nan = dark;
    nan.mean_pq = f32::NAN;
    assert_eq!(smoother.update(&nan), None);
    let mut bright_but_broken = sample();
    bright_but_broken.min_pq = -0.1;
    assert_eq!(smoother.update(&bright_but_broken), None);
    let after = smoother.update(&dark).unwrap();
    assert!(!after.scene_cut, "compared with the last valid sample");
    assert_eq!(after.stats, dark);

    smoother.reset();
    let fresh = smoother.update(&sample()).unwrap();
    assert!(fresh.scene_cut, "a reset starts over");
    assert_eq!(fresh.stats, sample());
}

#[test]
fn frame_units_reject_invalid_statistics() {
    let mut broken = sample();
    broken.min_pq = -0.1;
    assert_eq!(frame_units(Codec::Hevc, &broken), None);
    assert_eq!(frame_units(Codec::Av1, &broken), None);
    assert_eq!(Codec::from_video_format(1), Some(Codec::Hevc));
    assert_eq!(Codec::from_video_format(2), Some(Codec::Av1));
    assert_eq!(Codec::from_video_format(0), None);
}

#[test]
fn spliced_copies_as_splice_inserts() {
    let headers = [nal(32, &[0x0C]), nal(33, &[0x01]), nal(34, &[0xC1])].concat();
    let keyframe = [headers.clone(), nal(19, &[0xAF, 0x12])].concat();
    let units = frame_units(Codec::Hevc, &sample()).unwrap();
    let mut expected = keyframe.clone();
    assert!(splice(Codec::Hevc, &mut expected, &units));
    let copy = spliced(Codec::Hevc, &keyframe, &units).unwrap();
    assert_eq!(copy, expected);
    assert_eq!(
        copy.capacity(),
        keyframe.len() + units.len(),
        "one allocation"
    );
    assert_eq!(spliced(Codec::Hevc, &keyframe, &[]), None);
    assert_eq!(spliced(Codec::Hevc, &headers, &units), None);

    let av1 = [obu(2, &[]), obu(1, &[0x0A, 0x0B]), obu(6, &[0x10, 0x20])].concat();
    let units = frame_units(Codec::Av1, &sample()).unwrap();
    let mut expected = av1.clone();
    assert!(splice(Codec::Av1, &mut expected, &units));
    assert_eq!(spliced(Codec::Av1, &av1, &units), Some(expected));
}

#[test]
fn writer_keeps_the_last_sample_until_the_next() {
    let mut writer = Writer::new(Codec::Hevc);
    let default_stand_in = FrameStats::from_static(&crate::hdr::Metadata::default());
    assert_eq!(
        writer.units(),
        frame_units(Codec::Hevc, &default_stand_in).unwrap(),
        "the first frames carry the stand-in"
    );
    assert!(!writer.measured());
    // An invalid first sample keeps the stand-in.
    let mut broken = sample();
    broken.max_pq = f32::NAN;
    assert_eq!(writer.sample(&broken), None);
    assert!(!writer.measured());
    // Static metadata rebuilds the stand-in until the first sample.
    let mut metadata = crate::hdr::Metadata::display(600., 0.01, 300.);
    metadata.max_cll = 450;
    writer.set_metadata(&metadata);
    assert_eq!(
        writer.units(),
        frame_units(Codec::Hevc, &FrameStats::from_static(&metadata)).unwrap()
    );
    assert!(writer.sample(&sample()).unwrap().scene_cut);
    assert!(writer.measured());
    assert_eq!(writer.units(), frame_units(Codec::Hevc, &sample()).unwrap());
    // After it, measurements stand.
    writer.set_metadata(&crate::hdr::Metadata::default());
    assert_eq!(writer.units(), frame_units(Codec::Hevc, &sample()).unwrap());

    // Invalid statistics keep the units.
    let before = writer.units().to_vec();
    assert_eq!(writer.sample(&broken), None);
    assert_eq!(writer.units(), before);

    // A small change is smoothed into the units, not taken as it is.
    let mut brighter = sample();
    brighter.mean_pq += 0.03;
    let step = writer.sample(&brighter).unwrap();
    assert!(!step.scene_cut);
    assert_eq!(
        writer.units(),
        frame_units(Codec::Hevc, &step.stats).unwrap()
    );
    assert_ne!(writer.units(), frame_units(Codec::Hevc, &brighter).unwrap());

    let mut av1 = Writer::new(Codec::Av1);
    assert_eq!(
        av1.units(),
        frame_units(Codec::Av1, &default_stand_in).unwrap()
    );
    av1.sample(&sample()).unwrap();
    assert_eq!(av1.codec(), Codec::Av1);
    assert_eq!(av1.units(), frame_units(Codec::Av1, &sample()).unwrap());
}
