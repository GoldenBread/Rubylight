//! Tests for five-second stutter windows, overload reporting, and mode-switch resets.

use super::*;

#[test]
fn the_stutter_report_names_an_encode_stall_and_counts_each_picture_once() {
    use crate::stutter_report::Stage;
    let period = Duration::from_micros(16_667);
    let start = Instant::now();
    let mut p = Performance::default();
    let mut totals = StutterTotals::default();
    for n in 0..60 {
        let mut stages = [1_000, 20, 5, 300, 4_000, 10, 40, 50, 500];
        if n == 30 {
            // The encoder held this picture for 54 ms instead of 4.
            stages[4] = 54_000;
        }
        p.record_stages(&stamped(start + Duration::from_millis(n), stages), 0);
    }
    // The console reads the window that has not been advanced yet.
    let report = stutter(&p, &totals, period);
    assert_eq!((report.pictures(), report.late_pictures()), (60, 1));
    assert_eq!(report.dominant(), Some(Stage::Encode));
    assert_eq!(report.total_lost_us(), 50_000);
    // Advancing moves the window into the stream's totals once.
    advance(&mut p, &mut totals, period);
    assert_eq!(stutter(&p, &totals, period), report);
    advance(&mut p, &mut totals, period);
    assert_eq!(stutter(&p, &totals, period), report);
    assert_eq!(
        report.snapshot()["stages"][0],
        serde_json::json!({ "id": "encode", "lost_ms": 50.0, "percent": 100 })
    );
}

#[test]
fn five_seconds_of_windows_name_the_overload_against_the_earlier_ones() {
    use crate::stutter_report::Stage;
    let period = Duration::from_micros(16_667);
    let start = Instant::now();
    let mut p = Performance::default();
    let mut totals = StutterTotals::default();
    for n in 0..60 {
        p.record_stages(
            &stamped(
                start + Duration::from_millis(n),
                [1_000, 20, 5, 300, 4_000, 10, 40, 50, 500],
            ),
            0,
        );
    }
    advance(&mut p, &mut totals, period);
    assert_eq!(stutter(&p, &totals, period).pictures(), 60);
    for n in 0..60 {
        p.record_stages(
            &stamped(
                start + Duration::from_millis(1_000 + n),
                [1_000, 20, 5, 300, 24_000, 10, 40, 50, 500],
            ),
            0,
        );
    }
    advance(&mut p, &mut totals, period);
    let report = stutter(&p, &totals, period);
    assert_eq!((report.windows(), report.pictures()), (2, 120));
    assert_eq!(report.late_pictures(), 60);
    assert_eq!(report.dominant(), Some(Stage::Encode));
    assert_eq!(report.total_lost_us(), 60 * 20_000);
}

#[test]
fn a_window_with_too_few_pictures_waits_for_the_next_one() {
    let period = Duration::from_micros(16_667);
    let start = Instant::now();
    let mut p = Performance::default();
    let mut totals = StutterTotals::default();
    for n in 0..5 {
        p.record_stages(
            &stamped(
                start + Duration::from_millis(n),
                [1_000, 20, 5, 300, 4_000, 10, 40, 50, 500],
            ),
            0,
        );
    }
    advance(&mut p, &mut totals, period);
    assert_eq!(
        stutter(&p, &totals, period).pictures(),
        5,
        "nothing is dropped"
    );
    for n in 5..10 {
        p.record_stages(
            &stamped(
                start + Duration::from_millis(n),
                [1_000, 20, 5, 300, 4_000, 10, 40, 50, 500],
            ),
            0,
        );
    }
    advance(&mut p, &mut totals, period);
    assert_eq!(stutter(&p, &totals, period).pictures(), 10);
}

#[test]
fn a_late_five_second_mark_still_counts_every_picture_of_its_window() {
    // The housekeeping loop checks the five-second mark only every 250 ms, so
    // the window is up to that much longer than the stage samples it needs.
    // No picture may be evicted before it has been counted.
    let period = Duration::from_micros(16_667);
    let start = Instant::now();
    let mut p = Performance::default();
    let mut totals = StutterTotals::default();
    for n in 0..525 {
        p.record_stages(
            &stamped(
                start + Duration::from_millis(n * 10),
                [1_000, 20, 5, 300, 4_000, 10, 40, 50, 500],
            ),
            0,
        );
    }
    advance(&mut p, &mut totals, period);
    let report = stutter(&p, &totals, period);
    assert_eq!((report.windows(), report.pictures()), (1, 525));
}

#[test]
fn a_picture_sent_before_an_advance_that_ran_after_it_is_counted_once() {
    // A sender records a picture when its last packet is out, and the window
    // advances on the video thread's own five-second mark. The window's old
    // time cut-off dropped a picture whose send time was before the mark but
    // which the sender recorded after it, from every later window too; the
    // sequence number counts it once.
    let period = Duration::from_micros(16_667);
    let start = Instant::now();
    let mut p = Performance::default();
    let mut totals = StutterTotals::default();
    for n in 0..60 {
        p.record_stages(
            &stamped(
                start + Duration::from_millis(n),
                [1_000, 20, 5, 300, 4_000, 10, 40, 50, 500],
            ),
            0,
        );
    }
    advance(&mut p, &mut totals, period);
    assert_eq!(stutter(&p, &totals, period).pictures(), 60);
    for n in 0..12 {
        p.record_stages(
            &stamped(
                start + Duration::from_millis(500 + n),
                [1_000, 20, 5, 300, 4_000, 10, 40, 50, 500],
            ),
            0,
        );
    }
    assert_eq!(stutter(&p, &totals, period).pictures(), 72);
    advance(&mut p, &mut totals, period);
    assert_eq!(stutter(&p, &totals, period).pictures(), 72);
    advance(&mut p, &mut totals, period);
    assert_eq!(
        stutter(&p, &totals, period).pictures(),
        72,
        "a picture counted twice"
    );
}

#[test]
fn a_stream_without_a_picture_has_no_stutter_report() {
    let report = stutter(
        &Performance::default(),
        &StutterTotals::default(),
        Duration::from_micros(16_667),
    );
    assert_eq!((report.windows(), report.pictures()), (0, 0));
    assert_eq!(report.dominant(), None);
    assert_eq!(report.snapshot()["stages"], serde_json::json!([]));
}
/// The video thread's side of the stutter report: the window is copied out
/// under the stage lock and [`StutterTotals::add`] runs after it is released.
fn advance(p: &mut Performance, totals: &mut StutterTotals, period: Duration) {
    if let Some(pictures) = p.advance_stutter() {
        totals.add(&pictures, period);
    }
}

/// A mode switch: the old mode's last window, then a new usual path.
fn forget_usual(p: &mut Performance, totals: &mut StutterTotals, old: Duration) {
    if let Some(pictures) = p.forget_stutter_usual() {
        totals.add(&pictures, old);
    }
    totals.forget_usual();
}

/// What the console reads: the totals the video thread published, plus the
/// pictures recorded since the last advance.
fn stutter(p: &Performance, totals: &StutterTotals, period: Duration) -> StutterReport {
    StutterWindow::new(totals.clone(), p.uncounted_stutter().collect()).report(period)
}

#[test]
fn a_mode_switch_counts_the_old_window_then_judges_the_new_mode_by_itself() {
    use crate::stutter_report::Stage;
    let old = Duration::from_micros(16_667);
    let start = Instant::now();
    let mut p = Performance::default();
    let mut totals = StutterTotals::default();
    // A stall in the old mode's last, unadvanced window is still named.
    for n in 0..60 {
        let mut stages = [1_000, 20, 5, 300, 4_000, 10, 40, 50, 500];
        if n == 30 {
            stages[4] = 54_000;
        }
        p.record_stages(&stamped(start + Duration::from_millis(n), stages), 0);
    }
    forget_usual(&mut p, &mut totals, old);
    let before = stutter(&p, &totals, old);
    assert_eq!((before.pictures(), before.late_pictures()), (60, 1));
    assert_eq!(before.dominant(), Some(Stage::Encode));
    // A heavier mode right after a resolution or rate change: its first window
    // is its own usual, not a stutter against the old mode.
    for n in 0..60 {
        p.record_stages(
            &stamped(
                start + Duration::from_millis(1_000 + n),
                [2_000, 40, 10, 600, 24_000, 20, 80, 100, 1_000],
            ),
            0,
        );
    }
    let after = stutter(&p, &totals, Duration::from_micros(8_333));
    assert_eq!((after.pictures(), after.late_pictures()), (120, 1));
    assert_eq!(after.dominant(), Some(Stage::Encode));
}
