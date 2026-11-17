use super::*;

/// A 60 fps stream's frame interval, in microseconds.
const PERIOD: Duration = Duration::from_micros(16_667);

fn picture(capture: u64, convert: u64, encode: u64, packetize: u64, send: u64) -> Picture {
    Picture::new(capture, convert, encode, packetize, send)
}

/// A window of `count` pictures at the same stage times.
fn steady(count: usize, times: Picture) -> Vec<Picture> {
    vec![times; count]
}

fn add(totals: &mut Totals, pictures: &[Picture]) {
    totals.add(pictures, PERIOD);
}

#[test]
fn a_steady_stream_has_no_stutter_to_name() {
    let mut totals = Totals::default();
    for _ in 0..3 {
        add(
            &mut totals,
            &steady(60, picture(1_000, 100, 4_000, 100, 500)),
        );
    }
    let report = totals.report();
    assert_eq!(
        (report.windows(), report.pictures(), report.late_pictures()),
        (3, 180, 0)
    );
    assert_eq!(report.total_lost_us(), 0);
    assert_eq!(report.dominant(), None);
    assert_eq!(
        report.snapshot(),
        serde_json::json!({
            "windows": 3,
            "pictures": 180,
            "late_pictures": 0,
            "lost_ms": 0.0,
            "dominant": null,
            "stages": [],
            "not_measured": ["game", "network", "decode", "present"],
        })
    );
}

#[test]
fn an_encode_stall_of_fifty_milliseconds_is_named_encode() {
    let mut pictures = steady(60, picture(1_000, 100, 4_000, 100, 500));
    // One picture the encoder held for 54 ms instead of 4.
    pictures[30] = picture(1_000, 100, 54_000, 100, 500);
    let mut totals = Totals::default();
    add(&mut totals, &pictures);
    let report = totals.report();
    assert_eq!(report.late_pictures(), 1);
    assert_eq!(report.dominant(), Some(Stage::Encode));
    assert_eq!(report.lost_us(Stage::Encode), 50_000);
    assert_eq!(report.total_lost_us(), 50_000);
    assert_eq!(report.share_percent(Stage::Encode), 100);
    let snapshot = report.snapshot();
    assert_eq!(snapshot["dominant"], "encode");
    assert_eq!(snapshot["lost_ms"], 50.0);
    assert_eq!(snapshot["late_pictures"], 1);
    assert_eq!(
        snapshot["stages"],
        serde_json::json!([{ "id": "encode", "lost_ms": 50.0, "percent": 100 }])
    );
}

#[test]
fn a_capture_that_hands_over_a_late_picture_is_named_capture() {
    let mut pictures = steady(60, picture(1_000, 100, 4_000, 100, 500));
    // The capture's present stamp is 40 ms older when the stream takes it.
    pictures[10] = picture(41_000, 100, 4_000, 100, 500);
    let mut totals = Totals::default();
    add(&mut totals, &pictures);
    let report = totals.report();
    assert_eq!(report.dominant(), Some(Stage::Capture));
    assert_eq!(report.lost_us(Stage::Capture), 40_000);
}

#[test]
fn a_slow_stage_is_judged_against_its_own_usual_not_the_biggest_one() {
    // Encoding always takes 8 ms, far more than anything else; a conversion
    // spike is still the stutter.
    let mut pictures = steady(60, picture(1_000, 500, 8_000, 100, 500));
    pictures[20] = picture(1_000, 3_500, 8_000, 100, 500);
    let mut totals = Totals::default();
    // 2 ms frames: half a frame is the one-millisecond floor, which the 3 ms
    // conversion spike clears. A 60 fps stream's 8.3 ms half frame would hide
    // this spike, so the floor is what keeps it reportable.
    totals.add(&pictures, Duration::from_micros(2_000));
    let report = totals.report();
    assert_eq!(report.dominant(), Some(Stage::Convert));
    assert_eq!(report.lost_us(Stage::Convert), 3_000);
    assert_eq!(report.lost_us(Stage::Encode), 0);
}

#[test]
fn a_stream_that_is_always_slow_has_no_stutter_against_itself() {
    let mut totals = Totals::default();
    for _ in 0..3 {
        // 24 ms of encoding on a 16.7 ms stream: slower than the rate, but the
        // stream's own usual path.
        add(
            &mut totals,
            &steady(60, picture(1_000, 500, 24_000, 100, 500)),
        );
    }
    let report = totals.report();
    assert_eq!(report.late_pictures(), 0);
    assert_eq!(report.dominant(), None);
}

#[test]
fn an_overload_is_measured_against_the_windows_before_it() {
    let mut totals = Totals::default();
    for _ in 0..3 {
        add(
            &mut totals,
            &steady(60, picture(1_000, 500, 4_000, 100, 500)),
        );
    }
    // Every picture of the next window is 20 ms slower in the encoder: its own
    // median would hide the whole window, the earlier windows do not.
    add(
        &mut totals,
        &steady(60, picture(1_000, 500, 24_000, 100, 500)),
    );
    let report = totals.report();
    assert_eq!((report.windows(), report.pictures()), (4, 240));
    assert_eq!(report.late_pictures(), 60);
    assert_eq!(report.dominant(), Some(Stage::Encode));
    assert_eq!(report.lost_us(Stage::Encode), 60 * 20_000);
}

#[test]
fn a_new_mode_forgets_the_usual_path_of_the_old_one() {
    let mut totals = Totals::default();
    add(
        &mut totals,
        &steady(60, picture(1_000, 100, 4_000, 100, 500)),
    );
    totals.forget_usual();
    // A heavier mode after a resolution or rate change: this window is its own
    // usual, or every picture would be late against the old mode.
    add(
        &mut totals,
        &steady(60, picture(2_000, 200, 24_000, 200, 1_000)),
    );
    assert_eq!(totals.report().late_pictures(), 0);
    let mut spiked = steady(60, picture(2_000, 200, 24_000, 200, 1_000));
    spiked[9] = picture(2_000, 200, 54_000, 200, 1_000);
    add(&mut totals, &spiked);
    let report = totals.report();
    assert_eq!(report.late_pictures(), 1);
    assert_eq!(report.dominant(), Some(Stage::Encode));
    assert_eq!(report.lost_us(Stage::Encode), 30_000);
}

#[test]
fn a_picture_less_than_half_a_frame_later_than_usual_is_not_late() {
    let mut pictures = steady(60, picture(1_000, 500, 4_000, 100, 500));
    // 5 ms over the usual path, under the 8.3 ms half frame of 60 fps.
    pictures[5] = picture(1_000, 500, 9_000, 100, 500);
    let mut totals = Totals::default();
    add(&mut totals, &pictures);
    assert_eq!(totals.report().late_pictures(), 0);
}

#[test]
fn a_very_fast_stream_uses_the_one_millisecond_floor() {
    // 1000 fps: half a frame is 0.5 ms, less than a millisecond, which would
    // report noise as a stutter.
    let fast = Duration::from_micros(1_000);
    let mut totals = Totals::default();
    let mut pictures = steady(60, picture(200, 50, 300, 50, 100));
    // 900 us over, under the floor.
    pictures[5] = picture(200, 50, 1_200, 50, 100);
    totals.add(&pictures, fast);
    assert_eq!(totals.report().late_pictures(), 0);
    pictures[5] = picture(200, 50, 2_400, 50, 100);
    totals.add(&pictures, fast);
    let report = totals.report();
    assert_eq!(report.dominant(), Some(Stage::Encode));
    assert_eq!(report.lost_us(Stage::Encode), 2_100);
}

#[test]
fn the_stages_shares_add_up_to_the_time_the_picture_lost() {
    // 60 ms later than usual, split 3:1 between the encoder and the sender.
    let mut pictures = steady(60, picture(1_000, 100, 4_000, 100, 500));
    pictures[7] = picture(1_000, 100, 49_000, 100, 15_500);
    let mut totals = Totals::default();
    add(&mut totals, &pictures);
    let report = totals.report();
    assert_eq!(report.total_lost_us(), 60_000);
    assert_eq!(report.lost_us(Stage::Encode), 45_000);
    assert_eq!(report.lost_us(Stage::Send), 15_000);
    assert_eq!(report.share_percent(Stage::Encode), 75);
    let snapshot = report.snapshot();
    assert_eq!(snapshot["dominant"], "encode");
    assert_eq!(
        snapshot["stages"],
        serde_json::json!([
            { "id": "encode", "lost_ms": 45.0, "percent": 75 },
            { "id": "send", "lost_ms": 15.0, "percent": 25 },
        ])
    );
}

#[test]
fn a_tie_names_the_earlier_stage_of_the_path() {
    let mut pictures = steady(60, picture(1_000, 100, 4_000, 100, 500));
    // Two pictures 10 ms late, one in the capture and one in the conversion.
    pictures[0] = picture(11_000, 100, 4_000, 100, 500);
    pictures[1] = picture(1_000, 10_100, 4_000, 100, 500);
    let mut totals = Totals::default();
    add(&mut totals, &pictures);
    let report = totals.report();
    assert_eq!(report.late_pictures(), 2);
    assert_eq!(report.share_percent(Stage::Capture), 50);
    assert_eq!(report.share_percent(Stage::Convert), 50);
    assert_eq!(report.dominant(), Some(Stage::Capture));
    assert_eq!(report.snapshot()["stages"][0]["id"], "capture");
}

#[test]
fn a_remainder_goes_to_the_stage_with_the_most_excess() {
    // Just over a millisecond late, split three ways: the two smaller stages
    // round down and the largest keeps the remainder, so the shares still add
    // up to the time the picture lost.
    let normal = picture(1_000, 1_000, 1_000, 1_000, 1_000);
    let mut pictures = steady(60, normal);
    pictures[3] = picture(1_001, 1_001, 2_002, 1_000, 1_000);
    let mut totals = Totals::default();
    // 2 ms frames: half a frame is under a millisecond, so the floor decides.
    totals.add(&pictures, Duration::from_micros(2_000));
    let report = totals.report();
    assert_eq!(report.total_lost_us(), 1_004);
    assert_eq!(report.lost_us(Stage::Capture), 1);
    assert_eq!(report.lost_us(Stage::Convert), 1);
    assert_eq!(report.lost_us(Stage::Encode), 1_002);
    assert_eq!(report.dominant(), Some(Stage::Encode));
}

#[test]
fn an_empty_window_adds_nothing() {
    let mut totals = Totals::default();
    totals.add(&[], PERIOD);
    assert_eq!(totals.report().windows(), 0);
    assert_eq!(totals.report().pictures(), 0);
}

#[test]
fn huge_times_saturate_instead_of_overflowing() {
    let normal = picture(0, 0, 0, 0, 0);
    let mut pictures = steady(60, normal);
    pictures[0] = picture(u32::MAX.into(), u32::MAX.into(), u32::MAX.into(), 0, 0);
    let mut totals = Totals::default();
    add(&mut totals, &pictures);
    let report = totals.report();
    assert_eq!(report.late_pictures(), 1);
    assert!(report.total_lost_us() >= u64::from(u32::MAX) * 2);
}

#[test]
fn a_picture_groups_the_nine_stage_samples_the_video_thread_takes() {
    // capture, hold, claim, convert, encode, queue, packetize, send_queue, send,
    // then the total and the two capture ages, which are not stages.
    let samples = [100, 20, 5, 300, 4_000, 10, 40, 50, 500, 0, 0, 0, 0];
    let grouped = Picture::of(&samples);
    assert_eq!(grouped, picture(125, 300, 4_000, 50, 550));
    assert_eq!(grouped.total(), 5_025);
    assert_eq!(
        Stage::ALL.map(|stage| grouped.get(stage)),
        [125, 300, 4_000, 50, 550]
    );
    assert_eq!(Picture::of(&[]), Picture::default());
}
