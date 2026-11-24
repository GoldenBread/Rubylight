//! Tests for WebRTC H.264, HEVC, and AV1 codec level and tier selection.
use super::super::*;

#[test]
fn the_signalled_levels_cover_every_offered_mode() {
    // Every mode at every rate the default offers has an HEVC and an AV1 level that
    // holds the picture, across the bitrate range. 4K120 is HEVC 5.2 and AV1
    // seq_level_idx 14 (5.2, whose limits 5.3 shares). H.264 is not offered at 4K:
    // 4K120 would need level 6.0, which browsers refuse, so H.264 stops at 1440p.
    let offer = FrameRateOffer::default();
    for mode in &MODES {
        for &fps in offer.rates(mode.width, mode.height) {
            let (w, h, f) = (mode.width.into(), mode.height.into(), fps.into());
            for kbps in [10_000, 50_000, 100_000] {
                let (level, _) = hevc_level(mode.width, mode.height, fps, kbps).unwrap();
                let (_, limits) = HEVC_LEVELS
                    .iter()
                    .find(|(l, _)| l.level_id == level.level_id)
                    .unwrap();
                assert!(limits.holds_picture(w, h, f));
                let (level, _) = av1_level(mode.width, mode.height, fps, kbps).unwrap();
                let (_, limits) = AV1_LEVELS
                    .iter()
                    .find(|(l, _)| l.level_idx == level.level_idx)
                    .unwrap();
                assert!(limits.holds_picture(w, h, f));
            }
        }
    }
    assert!(hevc(3840, 2160, 120, 50_000).1 >= 156);
    assert!(av1(3840, 2160, 120, 50_000).1 >= 14);
    assert!(!ModeSpec::find(3840, 2160).unwrap().h264);
}

/// HEVC level name, id and tier flag at 10 Mbps unless a rate is given.
fn hevc(w: u32, h: u32, fps: u32, kbps: u32) -> (&'static str, u32, u32) {
    let (level, tier) = hevc_level(w, h, fps, kbps).unwrap();
    (level.name, level.level_id, tier.flag())
}

fn av1(w: u32, h: u32, fps: u32, kbps: u32) -> (&'static str, u32, u32) {
    let (level, tier) = av1_level(w, h, fps, kbps).unwrap();
    (level.name, level.level_idx, tier.flag())
}

#[test]
fn hevc_levels_match_the_protocol_examples() {
    assert_eq!(hevc(1920, 1080, 60, 10_000), ("4.1", 123, 0));
    assert_eq!(hevc(2560, 1440, 60, 10_000), ("5", 150, 0));
    assert_eq!(hevc(1920, 1080, 120, 10_000), ("5", 150, 0));
    assert_eq!(hevc(3840, 2160, 60, 10_000), ("5.1", 153, 0));
    assert_eq!(hevc(2560, 1440, 120, 10_000), ("5.1", 153, 0));
    assert_eq!(hevc(3840, 2160, 120, 10_000), ("5.2", 156, 0));
}

#[test]
fn hevc_levels_never_drop_below_4_1() {
    // The reference floors at 4.1: a Samsung decoder may refuse a lower offer.
    assert_eq!(hevc(1920, 1080, 30, 10_000), ("4.1", 123, 0));
    assert_eq!(hevc(1280, 720, 30, 10_000), ("4.1", 123, 0));
    assert_eq!(hevc(1280, 720, 60, 10_000), ("4.1", 123, 0));
    assert_eq!(hevc(1280, 720, 120, 10_000), ("4.1", 123, 0));
    assert_eq!(hevc(1920, 1080, 30, 100_000).2, 1);
}

#[test]
fn hevc_levels_take_the_bitrate_into_account() {
    // 1080p60 fits level 4.1, but 50 Mbps is over its 20 Mbps Main tier.
    assert_eq!(hevc(1920, 1080, 60, 20_000), ("4.1", 123, 0));
    assert_eq!(hevc(1920, 1080, 60, 20_001), ("5", 150, 0));
    assert_eq!(hevc(1920, 1080, 60, 50_000), ("5.2", 156, 0));
    // 4K120 at 100 Mbps is over every Main tier: 5.2 High.
    assert_eq!(hevc(3840, 2160, 120, 60_000), ("5.2", 156, 0));
    assert_eq!(hevc(3840, 2160, 120, 100_000), ("5.2", 156, 1));
    // 1080p30 HDR at its usual bitrate stays at 4.1.
    assert_eq!(hevc(1920, 1080, 30, 20_000), ("4.1", 123, 0));
    // Picture size or rate beyond 5.2 has no level at any bitrate.
    assert_eq!(hevc_level(16384, 16384, 60, 10_000), None);
    assert_eq!(hevc_level(3840, 2160, 240, 10_000), None);
}

#[test]
fn every_offered_mode_has_a_level_across_the_bitrate_range() {
    for mode in &MODES {
        for fps in FRAME_RATES {
            for kbps in [10_000, 50_000, 100_000] {
                assert!(hevc_level(mode.width, mode.height, fps, kbps).is_some());
                assert!(av1_level(mode.width, mode.height, fps, kbps).is_some());
            }
        }
    }
}

#[test]
fn av1_levels_follow_the_same_rules() {
    assert_eq!(av1(1920, 1080, 60, 10_000), ("4.1", 9, 0));
    assert_eq!(av1(1920, 1080, 30, 10_000), ("4.1", 9, 0));
    assert_eq!(av1(1280, 720, 30, 10_000), ("4.1", 9, 0));
    assert_eq!(av1(2560, 1440, 60, 10_000), ("5.0", 12, 0));
    assert_eq!(av1(1920, 1080, 120, 10_000), ("5.0", 12, 0));
    assert_eq!(av1(3840, 2160, 60, 10_000), ("5.1", 13, 0));
    assert_eq!(av1(2560, 1440, 120, 10_000), ("5.1", 13, 0));
    assert_eq!(av1(3840, 2160, 120, 10_000), ("5.2", 14, 0));
    assert_eq!(av1(1920, 1080, 60, 50_000), ("5.2", 14, 0));
    assert_eq!(av1(3840, 2160, 120, 100_000), ("5.2", 14, 1));
    assert_eq!(av1_level(100_000, 100_000, 30, 10_000), None);
}

fn h264(w: u32, h: u32, fps: u32, kbps: u32) -> (&'static str, u32) {
    let level = h264_level(w, h, fps, kbps).unwrap();
    (level.name, level.level_idc)
}

#[test]
fn h264_levels_follow_table_a1() {
    // 1080p is 120x68 macroblocks (8160), 1440p 160x90 (14400), 720p 80x45 (3600).
    assert_eq!(h264(1280, 720, 30, 12_000), ("3.1", 31));
    assert_eq!(h264(1280, 720, 60, 12_000), ("3.2", 32));
    assert_eq!(h264(1280, 720, 120, 12_000), ("4.2", 42));
    assert_eq!(h264(1920, 1080, 30, 20_000), ("4", 40));
    assert_eq!(h264(1920, 1080, 60, 20_000), ("4.2", 42));
    assert_eq!(h264(1920, 1080, 90, 20_000), ("5.1", 51));
    assert_eq!(h264(1920, 1080, 120, 20_000), ("5.1", 51));
    assert_eq!(h264(2560, 1440, 30, 30_000), ("5", 50));
    assert_eq!(h264(2560, 1440, 60, 30_000), ("5.1", 51));
    assert_eq!(h264(2560, 1440, 90, 30_000), ("5.2", 52));
    assert_eq!(h264(2560, 1440, 120, 30_000), ("5.2", 52));
    // The bitrate counts too: 1080p60 at 100 Mbps is over 4.2's 50 Mbps.
    assert_eq!(h264(1920, 1080, 60, 50_000), ("4.2", 42));
    assert_eq!(h264(1920, 1080, 60, 100_000), ("5", 50));
    // 4K120 would be level 6, which is not offered.
    assert_eq!(h264_level(3840, 2160, 120, 50_000), None);
    assert_eq!(
        h264_level(3840, 2160, 60, 50_000).map(|l| l.name),
        Some("5.2")
    );
}

#[test]
fn every_offered_h264_mode_has_a_level_across_the_bitrate_range() {
    for mode in MODES.iter().filter(|m| m.h264) {
        for fps in FRAME_RATES {
            for kbps in [BITRATE_RANGE_KBPS.start(), BITRATE_RANGE_KBPS.end()] {
                assert!(
                    h264_level(mode.width, mode.height, fps, *kbps).is_some(),
                    "{}x{}@{fps} {kbps}",
                    mode.width,
                    mode.height
                );
            }
        }
    }
}
