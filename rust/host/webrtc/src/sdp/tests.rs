//! WebRTC SDP offer tests and shared test helpers.

use super::testing::*;
use super::*;

mod answer;

const HOST: &str = "192.168.1.10:8001";
const CLIENT: &str = "192.168.1.20:50000";

fn offer_for(spec: &OfferSpec) -> String {
    host_offer(spec, HOST.parse().unwrap()).1.sdp
}

fn video_section_of(sdp: &str) -> Vec<String> {
    let l = lines(sdp);
    l[section(&l, "video").unwrap()].to_vec()
}

#[test]
fn offer_has_one_codec_at_the_fixed_payload_type() {
    for (codec, name) in [
        (VideoCodec::H264, "H264/90000"),
        (VideoCodec::Hevc, "H265/90000"),
        (VideoCodec::Av1, "AV1/90000"),
    ] {
        let sdp = offer_for(&spec(codec, false));
        let video = video_section_of(&sdp);
        assert!(
            video[0].starts_with("m=video ") && video[0].ends_with(" 96"),
            "{}",
            video[0]
        );
        let rtpmaps: Vec<_> = video
            .iter()
            .filter(|l| l.starts_with("a=rtpmap:"))
            .collect();
        assert_eq!(rtpmaps, [&format!("a=rtpmap:96 {name}")]);
        assert!(sdp.contains("a=ice-lite"));
    }
}

#[test]
fn offer_has_exactly_one_imageattr_and_the_playout_delay_extension() {
    let sdp = offer_for(&spec(VideoCodec::Hevc, false));
    assert_eq!(sdp.matches("a=imageattr:").count(), 1);
    let video = video_section_of(&sdp);
    assert!(
        video
            .contains(&"a=imageattr:96 send [x=[1920:1920],y=[1080:1080],fps=[60:60]]".to_string())
    );
    assert!(
        video
            .iter()
            .any(|l| l.starts_with("a=extmap:6 ") && l.contains(PLAYOUT_DELAY_URI))
    );
    assert!(!sdp.contains("rtx") && !sdp.contains("ssrc-group"), "{sdp}");
    assert_eq!(
        video
            .iter()
            .filter(|l| l.starts_with("a=ssrc:") && l.contains(" cname:"))
            .count(),
        1
    );
    assert!(
        !sdp.contains("transport-cc") && !sdp.contains("goog-remb"),
        "{sdp}"
    );
    assert!(!sdp.contains("a=rtcp-fb:96 nack\r\n"), "{sdp}");
    assert!(
        video.iter().any(|l| l == "a=rtcp-fb:96 nack pli"),
        "{video:?}"
    );
    assert!(
        video.iter().any(|l| l == "a=rtcp-fb:96 ccm fir"),
        "{video:?}"
    );
    assert!(sdp.contains("m=application"));
    assert!(sdp.contains("a=group:BUNDLE"));
}

#[test]
fn offer_for_a_different_mode_has_its_own_imageattr() {
    let mut s = spec(VideoCodec::Av1, false);
    s.video.width = 3840;
    s.video.height = 2160;
    s.video.fps = 120;
    assert!(
        offer_for(&s).contains("a=imageattr:96 send [x=[3840:3840],y=[2160:2160],fps=[120:120]]")
    );
}

#[test]
fn rtx_and_nack_come_back_only_when_asked() {
    let mut s = spec(VideoCodec::H264, false);
    s.rtx = true;
    let sdp = offer_for(&s);
    assert!(sdp.contains("a=rtpmap:97 rtx/90000"), "{sdp}");
    assert!(sdp.contains("a=rtcp-fb:96 nack\r\n"), "{sdp}");
    assert!(sdp.contains("a=ssrc-group:FID"), "{sdp}");
    assert!(!sdp.contains("transport-cc") && !sdp.contains("goog-remb"));
}

fn fmtp(sdp: &str) -> String {
    video_section_of(sdp)
        .iter()
        .find(|l| l.starts_with("a=fmtp:96 "))
        .cloned()
        .unwrap_or_default()
}

fn param_of(line: &str, name: &str) -> Option<String> {
    fmtp_params(line)
        .into_iter()
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
}

#[test]
fn hevc_fmtp_levels_come_from_core_at_main_tier() {
    // (width, height, fps, kbps, level-id, tier-flag); core's levels, never below 4.1, and
    // Main tier even where core's table would need High (the last case).
    let cases = [
        (1280, 720, 60, 10_000, 123, 0),
        (1920, 1080, 60, 10_000, 123, 0),
        (1920, 1080, 60, 20_001, 150, 0),
        (1920, 1080, 120, 10_000, 150, 0),
        (2560, 1440, 60, 10_000, 150, 0),
        (2560, 1440, 120, 10_000, 153, 0),
        (3840, 2160, 60, 10_000, 153, 0),
        (3840, 2160, 120, 10_000, 156, 0),
        (3840, 2160, 120, 100_000, 156, 0),
    ];
    for (w, h, f, kbps, level, tier) in cases {
        for (hdr, profile) in [(false, 1), (true, 2)] {
            let mut s = spec(VideoCodec::Hevc, hdr);
            s.video.width = w;
            s.video.height = h;
            s.video.fps = f;
            s.video.bitrate_kbps = kbps;
            let line = fmtp(&offer_for(&s));
            let p = |n: &str| param_of(&line, n);
            assert_eq!(p("profile-id"), Some(profile.to_string()), "{line}");
            assert_eq!(p("level-asymmetry-allowed"), Some("1".into()), "{line}");
            assert_eq!(
                p("tier-flag"),
                Some(tier.to_string()),
                "{w}x{h}@{f} {kbps}: {line}"
            );
            assert_eq!(
                p("level-id"),
                Some(level.to_string()),
                "{w}x{h}@{f} {kbps}: {line}"
            );
        }
    }
}

#[test]
fn a_mode_no_level_holds_cannot_be_offered() {
    let mut s = spec(VideoCodec::Hevc, false);
    s.video.fps = 240;
    s.video.width = 3840;
    s.video.height = 2160;
    let Err(err) = rtc_config(&s) else {
        panic!("a mode no HEVC level holds was offered");
    };
    assert!(err.to_string().contains("no HEVC level"), "{err}");
    s.video.codec = VideoCodec::Av1;
    let Err(err) = rtc_config(&s) else {
        panic!("a mode no AV1 level holds was offered");
    };
    assert!(err.to_string().contains("no AV1 level"), "{err}");
}

#[test]
fn av1_fmtp_matches_the_mode() {
    for (w, h, f, kbps) in [
        (1920, 1080, 60, 20_000),
        (2560, 1440, 60, 20_000),
        (3840, 2160, 60, 20_000),
        (3840, 2160, 120, 20_000),
        (3840, 2160, 120, 200_000),
    ] {
        let (level, tier) = av1_level(w, h, f, kbps).unwrap();
        let mut s = spec(VideoCodec::Av1, true);
        s.video.width = w;
        s.video.height = h;
        s.video.fps = f;
        s.video.bitrate_kbps = kbps;
        let line = fmtp(&offer_for(&s));
        assert_eq!(param_of(&line, "profile").as_deref(), Some("0"), "{line}");
        assert_eq!(
            param_of(&line, "tier"),
            Some(tier.flag().to_string()),
            "{line}"
        );
        assert_eq!(
            param_of(&line, "level-idx"),
            Some(level.level_idx.to_string()),
            "{line}"
        );
    }
}

#[test]
fn h264_fmtp_is_packetization_mode_one_with_asymmetric_levels() {
    let line = fmtp(&offer_for(&spec(VideoCodec::H264, false)));
    let mut params: Vec<(String, String)> = fmtp_params(&line)
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    params.sort();
    assert_eq!(
        params,
        [
            ("level-asymmetry-allowed".to_string(), "1".to_string()),
            ("packetization-mode".to_string(), "1".to_string()),
            ("profile-level-id".to_string(), "42e02a".to_string()),
        ],
        "{line}"
    );
}

#[test]
fn h264_level_comes_from_the_mode_and_rate() {
    use butterpollo_session::webrtc::capabilities::{FRAME_RATES, MODES};
    // (width, height, fps, profile-level-id) at each mode's default bitrate; the profile bytes
    // never change. 1080p60 is 4.2, 1080p120 5.1 and 1440p90 and 120 are 5.2 (table A-1).
    let expected = [
        (1280, 720, 30, "42e01f"),
        (1280, 720, 60, "42e020"),
        (1280, 720, 90, "42e02a"),
        (1280, 720, 120, "42e02a"),
        (1920, 1080, 30, "42e028"),
        (1920, 1080, 60, "42e02a"),
        (1920, 1080, 90, "42e033"),
        (1920, 1080, 120, "42e033"),
        (2560, 1440, 30, "42e032"),
        (2560, 1440, 60, "42e033"),
        (2560, 1440, 90, "42e034"),
        (2560, 1440, 120, "42e034"),
    ];
    let mut offered = 0;
    for mode in MODES.iter().filter(|m| m.h264) {
        for fps in FRAME_RATES {
            let mut s = spec(VideoCodec::H264, false);
            s.video.width = mode.width;
            s.video.height = mode.height;
            s.video.fps = fps;
            s.video.bitrate_kbps = mode.default_bitrate_kbps;
            let line = fmtp(&offer_for(&s));
            let want = expected
                .iter()
                .find(|(w, h, f, _)| (*w, *h, *f) == (mode.width, mode.height, fps))
                .map(|(.., id)| id.to_string());
            assert_eq!(
                param_of(&line, "profile-level-id"),
                want,
                "{}x{}@{fps}: {line}",
                mode.width,
                mode.height
            );
            // The answer keeps the profile and may name any level.
            let answer = answer_to(&s, &offer_for(&s));
            assert!(check_answer(&answer, &s).is_ok(), "{answer}");
            offered += 1;
        }
    }
    assert_eq!(offered, expected.len(), "every offered H.264 mode and rate");
}

#[test]
fn colour_space_extension_is_offered_for_hdr_only() {
    let hdr = offer_for(&spec(VideoCodec::Hevc, true));
    assert!(
        video_section_of(&hdr)
            .iter()
            .any(|l| l.starts_with("a=extmap:9 ") && l.contains(COLOR_SPACE_URI)),
        "{hdr}"
    );
    assert!(!offer_for(&spec(VideoCodec::Hevc, false)).contains(COLOR_SPACE_URI));
}

#[test]
fn stereo_audio_uses_the_protocol_fmtp() {
    let sdp = offer_for(&spec(VideoCodec::H264, false));
    assert!(sdp.contains("a=rtpmap:111 opus/48000/2"));
    assert!(sdp.contains(&format!("a=fmtp:111 {STEREO_FMTP}")), "{sdp}");
    assert!(!sdp.contains("multiopus"));
}

#[test]
fn the_offer_is_the_same_standard_opus_for_stereo_5_1_and_7_1() {
    // Only the identifiers str0m makes up for each offer differ; take them out.
    let normalised = |spec: &OfferSpec| {
        offer_for(spec)
            .lines()
            .filter(|l| {
                ![
                    "o=",
                    "a=ice-ufrag",
                    "a=ice-pwd",
                    "a=fingerprint:",
                    "a=ssrc",
                    "a=msid:",
                    "a=candidate:",
                    "a=mid:",
                    "a=group:BUNDLE",
                ]
                .iter()
                .any(|p| l.starts_with(p))
            })
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    let stereo = normalised(&spec(VideoCodec::H264, false));
    for channels in [6, 8] {
        let surround = normalised(&surround_spec(channels));
        assert_eq!(stereo, surround, "{channels} channels");
    }
    let sdp = offer_for(&surround_spec(8));
    assert!(sdp.contains("a=rtpmap:111 opus/48000/2\r\n"), "{sdp}");
    assert!(
        sdp.contains(&format!("a=fmtp:111 {STEREO_FMTP}\r\n")),
        "{sdp}"
    );
    assert!(
        !sdp.contains("multiopus") && !sdp.contains("channel_mapping"),
        "{sdp}"
    );
}

/// The client side of an answer: what a Chromium-like receiver would send back.
fn answer_to(spec: &OfferSpec, offer: &str) -> String {
    let mut client = client(spec, CLIENT.parse().unwrap());
    answer(&mut client, offer)
}
