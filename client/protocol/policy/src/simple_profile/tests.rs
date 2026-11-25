//! Unit tests for the simple settings profile mapping and JSON wire handler.

use super::*;
use serde_json::{json, Value};

fn hdr_device() -> Capabilities {
    Capabilities {
        display: Display { width: 1920, height: 1080, refresh_hz: 120, hdr: true, vrr: true },
        decoder: Decoder { h264: true, hevc: true, av1: true, hdr: true },
        host: Host { hdr: Some(true), hevc: Some(true), av1: Some(true) },
    }
}

fn plain_device() -> Capabilities {
    Capabilities {
        display: Display { width: 1280, height: 720, refresh_hz: 60, hdr: false, vrr: false },
        decoder: Decoder { h264: true, hevc: false, av1: false, hdr: false },
        ..Capabilities::default()
    }
}

#[test]
fn the_recommended_choices_are_the_same_on_every_device() {
    for capabilities in [plain_device(), hdr_device()] {
        assert_eq!(
            recommended(&capabilities),
            Choices { smoothness: Smoothness::Auto, picture: Picture::Auto, hdr: Hdr::Auto }
        );
    }
}

#[test]
fn every_screen_keeps_its_own_size_and_rate() {
    // A decode time is not a sustained rate: nothing in the device's capabilities caps the
    // frame rate, so the layer always leaves the screen's own rate in charge.
    for capabilities in [plain_device(), hdr_device()] {
        let auto = values(Choices::default(), &capabilities);
        assert!(auto.native_resolution);
        assert_eq!(auto.fps, None, "the screen's own rate");
    }
    assert_eq!(hdr_device().display.frame_rate(), 120);
}

#[test]
fn smoother_choices_pace_and_use_vrr_where_the_screen_has_it() {
    let auto = values(Choices::default(), &hdr_device());
    assert_eq!(auto.pacing, Pacing::Latency);
    assert!(auto.vrr, "Auto uses VRR where the screen offers it");
    let fixed = values(Choices::default(), &plain_device());
    assert!(!fixed.vrr, "Auto does not invent VRR on a fixed screen");
    assert_eq!(fixed.pacing, Pacing::Latency);

    let balanced = values(Choices { smoothness: Smoothness::Balanced, ..Choices::default() }, &hdr_device());
    assert_eq!(balanced.pacing, Pacing::Balanced);
    assert!(balanced.vrr);

    let smoothest = values(Choices { smoothness: Smoothness::Smoothest, ..Choices::default() }, &hdr_device());
    assert_eq!(smoothest.pacing, Pacing::Smoothest);
    assert!(!smoothest.vrr, "VRR replaces the pacing, so Smoothest turns it off");

    let latency = values(Choices { smoothness: Smoothness::LowestDelay, ..Choices::default() }, &hdr_device());
    assert_eq!(latency.pacing, Pacing::Latency);
    assert!(!latency.vrr, "Lowest delay does not wait for the screen's refresh");
    for values in [auto, balanced, smoothest, latency] {
        assert!(values.latency_switches, "the low-latency switches stay on");
        assert!(values.drop_late_frames && !values.host_frame_times);
        assert!(!values.reduce_refresh && !values.content_rate);
    }
}

#[test]
fn picture_quality_sets_the_bitrate_and_whether_it_falls() {
    let auto = values(Choices { picture: Picture::Auto, ..Choices::default() }, &hdr_device());
    assert_eq!(auto.bitrate, Bitrate::Default);
    assert!(!auto.adaptive_bitrate, "the player did not ask the bitrate to fall");
    assert_eq!(auto.adaptive_quality, AdaptiveQuality::Off);

    let best = values(Choices { picture: Picture::Best, ..Choices::default() }, &hdr_device());
    assert_eq!(best.bitrate, Bitrate::Factor(1500));
    assert!(!best.adaptive_bitrate);
    assert_eq!(best.adaptive_quality, AdaptiveQuality::Off);
    assert_eq!(best.codec, Codec::Auto, "the stream negotiates the best codec both ends decode");

    let save = values(Choices { picture: Picture::SaveData, ..Choices::default() }, &hdr_device());
    assert_eq!(save.bitrate, Bitrate::Factor(600));
    assert!(save.adaptive_bitrate);
    assert_eq!(save.adaptive_quality, AdaptiveQuality::SavePower);
    assert_eq!(save.codec, Codec::Auto);
}

#[test]
fn picture_quality_never_forces_a_codec_the_pc_may_not_send() {
    let hevc_only = Capabilities {
        display: hdr_device().display,
        decoder: Decoder { av1: false, ..hdr_device().decoder },
        host: Host { av1: Some(false), ..hdr_device().host },
    };
    for capabilities in [hdr_device(), hevc_only, plain_device()] {
        for picture in Picture::ALL {
            assert_eq!(values(Choices { picture, ..Choices::default() }, &capabilities).codec, Codec::Auto);
        }
    }
}

#[test]
fn hdr_auto_needs_the_whole_chain_and_off_always_wins() {
    assert!(values(Choices::default(), &hdr_device()).hdr);
    let cases = [
        ("no HDR screen", Capabilities { display: Display { hdr: false, ..hdr_device().display }, ..hdr_device() }),
        ("no HDR decoder", Capabilities { decoder: Decoder { hdr: false, ..hdr_device().decoder }, ..hdr_device() }),
        ("no HDR PC", Capabilities { host: Host { hdr: Some(false), ..hdr_device().host }, ..hdr_device() }),
        ("no 10-bit codec", Capabilities { decoder: Decoder { hevc: false, av1: false, ..hdr_device().decoder }, ..hdr_device() }),
        ("PC's HDR codec unknown", Capabilities { host: Host { hdr: Some(true), hevc: Some(false), av1: Some(false) }, ..hdr_device() }),
    ];
    for (name, capabilities) in cases {
        assert!(!values(Choices::default(), &capabilities).hdr, "{name}");
    }
    assert!(!values(Choices { hdr: Hdr::Off, ..Choices::default() }, &hdr_device()).hdr);
}

#[test]
fn stored_choices_survive_a_later_version_and_unknown_values_read_as_recommended() {
    let choices = Choices { smoothness: Smoothness::LowestDelay, picture: Picture::SaveData, hdr: Hdr::Off };
    assert_eq!(Choices::from_keys("lowest-delay", "save-data", "off"), choices);
    assert_eq!(Choices::from_keys("teleport", "wat", "maybe"), Choices::default());
    assert_eq!(Choices::default().keys(), ("auto", "auto", "auto"));
}

#[test]
fn the_screen_rate_is_a_sane_stream_rate() {
    assert_eq!(Display { refresh_hz: 119, ..Display::default() }.frame_rate(), 119);
    assert_eq!(Display { refresh_hz: 0, ..Display::default() }.frame_rate(), 60);
    assert_eq!(Display { refresh_hz: 5000, ..Display::default() }.frame_rate(), 1000);
}

fn current_of(choices: Choices, capabilities: &Capabilities) -> Current {
    let values = values(choices, capabilities);
    Current {
        native_resolution: values.native_resolution,
        fps: values.fps,
        pacing: Some(values.pacing),
        vrr: values.vrr,
        latency_switches: values.latency_switches,
        drop_late_frames: values.drop_late_frames,
        host_frame_times: values.host_frame_times,
        real_vrr: values.real_vrr.unwrap_or(true),
        reduce_refresh: values.reduce_refresh,
        content_rate: values.content_rate,
        bitrate_milli: match values.bitrate {
            Bitrate::Default => None,
            Bitrate::Factor(milli) => Some(milli),
        },
        codec: values.codec,
        hdr: values.hdr,
        adaptive_bitrate: values.adaptive_bitrate,
        adaptive_quality: Some(values.adaptive_quality),
        stored_smoothness: Some(choices.smoothness),
        stored_picture: Some(choices.picture),
        stored_hdr: Some(choices.hdr),
    }
}

#[test]
fn every_choice_matches_itself_and_reads_back_as_itself() {
    for capabilities in [plain_device(), hdr_device()] {
        for smoothness in Smoothness::ALL {
            for picture in Picture::ALL {
                for hdr in Hdr::ALL {
                    let choices = Choices { smoothness, picture, hdr };
                    let current = current_of(choices, &capabilities);
                    assert!(matches(choices, &capabilities, &current), "{choices:?}");
                    assert_eq!(chosen(&current, &capabilities), Some(choices), "{choices:?}");
                }
            }
        }
    }
}

#[test]
fn auto_and_lowest_delay_are_told_apart_by_the_stored_choice() {
    // On a fixed screen Auto and Lowest delay write the same values; the stored choice decides
    // which one the row shows, and a fresh install with no stored choice shows Auto.
    let capabilities = plain_device();
    let lowest = Choices { smoothness: Smoothness::LowestDelay, ..Choices::default() };
    let mut current = current_of(lowest, &capabilities);
    assert_eq!(chosen(&current, &capabilities), Some(lowest));
    (current.stored_smoothness, current.stored_picture, current.stored_hdr) = (None, None, None);
    assert_eq!(chosen(&current, &capabilities), Some(Choices::default()));
}

#[test]
fn one_changed_advanced_setting_reads_as_custom() {
    let choices = Choices::default();
    let capabilities = hdr_device();
    let base = current_of(choices, &capabilities);
    let changes: [fn(&mut Current); 10] = [
        |c| c.native_resolution = false,
        |c| c.fps = Some(60),
        |c| c.pacing = None,
        |c| c.latency_switches = false,
        |c| c.drop_late_frames = false,
        |c| c.host_frame_times = true,
        |c| c.reduce_refresh = true,
        |c| c.content_rate = true,
        |c| c.bitrate_milli = Some(1234),
        |c| c.codec = Codec::H264,
    ];
    for change in changes {
        let mut current = base;
        change(&mut current);
        assert_eq!(chosen(&current, &capabilities), None, "{current:?}");
    }
    let mut real_vrr = base;
    real_vrr.real_vrr = false;
    assert_eq!(chosen(&real_vrr, &capabilities), None, "Real VRR off under VRR");
    let mut quality = base;
    quality.adaptive_quality = Some(AdaptiveQuality::SavePower);
    assert_eq!(chosen(&quality, &capabilities), None);
    let mut adaptive = base;
    adaptive.adaptive_bitrate = true;
    assert_eq!(chosen(&adaptive, &capabilities), None);
    // A hand change that lands on another profile's values is still Custom: the stored choice
    // alone decides. Changing Auto's pacing to Balanced equals the Balanced profile exactly.
    let mut other_profile = base;
    other_profile.pacing = Some(Pacing::Balanced);
    assert_eq!(chosen(&other_profile, &capabilities), None, "another profile's values are Custom");
    // And the reverse: Balanced with Auto's pacing would once have read as Auto.
    let balanced = Choices { smoothness: Smoothness::Balanced, ..Choices::default() };
    let mut from_balanced = current_of(balanced, &capabilities);
    from_balanced.pacing = Some(Pacing::Latency);
    assert_eq!(chosen(&from_balanced, &capabilities), None, "Balanced with Auto's pacing is Custom");
}

#[test]
fn hdr_turned_off_by_the_device_is_also_a_choice() {
    // A player on a plain screen sees "Auto" and the profile writes HDR off, so the two match.
    let capabilities = plain_device();
    let current = current_of(Choices::default(), &capabilities);
    assert!(!current.hdr);
    assert_eq!(chosen(&current, &capabilities), Some(Choices::default()));
    // Best on the same screen still says HDR off.
    let best = Choices { picture: Picture::Best, ..Choices::default() };
    assert!(!values(best, &capabilities).hdr);
}

#[test]
fn a_fresh_hdr_screen_reads_hdr_off_until_the_switch_is_on() {
    // A fresh install stores no choice (Android) or Auto (desktop), and the HDR switch it never
    // set leaves the stream SDR: the row reads Off, not the recommended Auto.
    let capabilities = hdr_device();
    let mut current = current_of(Choices::default(), &capabilities);
    current.hdr = false;
    let fresh = Choices { hdr: Hdr::Off, ..Choices::default() };
    assert_eq!(chosen(&current, &capabilities), Some(fresh));
    (current.stored_smoothness, current.stored_picture, current.stored_hdr) = (None, None, None);
    assert_eq!(chosen(&current, &capabilities), Some(fresh), "a fresh Android install stores nothing");
    // A pick in another row carries the row's Off, so it never turns HDR on behind the player's back.
    let picked = values(Choices { smoothness: Smoothness::Balanced, ..fresh }, &capabilities);
    assert!(!picked.hdr);
    // Turning the switch on in the Video settings streams HDR, and the row says Auto.
    current.hdr = true;
    assert_eq!(chosen(&current, &capabilities), Some(Choices::default()));
}

#[test]
fn json_answers_the_apps_requests() {
    let ask = |request: &str| -> Value { serde_json::from_str(&json::call(request)).unwrap() };
    let caps = json!({
        "display": {"width": 1920, "height": 1080, "refresh_hz": 120, "hdr": true, "vrr": true},
        "decoder": {"h264": true, "hevc": true, "av1": true, "hdr": true},
        "host": {"hdr": true, "hevc": true, "av1": true}
    });
    let request = json!({
        "op": "values", "smoothness": "smoothest", "picture": "best", "hdr": "auto",
        "display": caps["display"], "decoder": caps["decoder"], "host": caps["host"]
    });
    let answer = ask(&request.to_string());
    assert_eq!(answer["ok"]["pacing"], "smoothest");
    assert_eq!(answer["ok"]["vrr"], false);
    assert_eq!(answer["ok"]["bitrate"], json!({"mode": "factor", "milli": 1500}));
    assert_eq!(answer["ok"]["codec"], "auto");
    assert_eq!(answer["ok"]["hdr"], true);
    assert_eq!(answer["ok"]["native_resolution"], true);

    let recommended = ask(&json!({"op": "recommended", "display": caps["display"], "decoder": caps["decoder"], "host": caps["host"]}).to_string());
    assert_eq!(recommended["ok"], json!({"smoothness": "auto", "picture": "auto", "hdr": "auto"}));
    // A values request without a smoothness key reads as the recommended Auto, not the old Balanced.
    let omitted = ask(&json!({"op": "values", "picture": "auto", "hdr": "auto",
        "display": caps["display"], "decoder": caps["decoder"], "host": caps["host"]}).to_string());
    assert_eq!(omitted["ok"]["pacing"], "latency");

    let current = json!({
        "native_resolution": true, "fps": null, "pacing": "balanced", "vrr": true,
        "latency_switches": true, "drop_late_frames": true, "host_frame_times": false,
        "real_vrr": true, "reduce_refresh": false, "content_rate": false,
        "bitrate_milli": null, "codec": "auto", "hdr": true,
        "adaptive_bitrate": false, "adaptive_quality": "off",
        "stored_smoothness": "balanced", "stored_picture": "auto", "stored_hdr": "auto"
    });
    let chosen = ask(&json!({"op": "chosen", "current": current, "display": caps["display"], "decoder": caps["decoder"], "host": caps["host"]}).to_string());
    assert_eq!(chosen["ok"]["choices"]["smoothness"], "balanced");
    assert_eq!(ask(&json!({"op": "chosen", "current": current}).to_string())["ok"]["choices"], Value::Null);
    assert_eq!(ask(r#"{"op":"teleport"}"#)["error"], "invalid");
    assert_eq!(ask(r#"{"op":"chosen"}"#)["error"], "invalid");
    assert_eq!(ask("not json")["error"], "invalid");
}
