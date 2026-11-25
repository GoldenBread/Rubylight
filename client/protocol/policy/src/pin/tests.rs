//! Pure unit tests for PIN throttle backoff, gate decision tables and grace windows.

use super::*;

#[test]
fn four_wrong_tries_are_free_then_the_wait_doubles_to_half_an_hour() {
    let delays: Vec<i64> = (1..=12).map(throttle::delay_after).collect();
    assert_eq!(
        delays,
        [
            0, 0, 0, 0, 30_000, 60_000, 120_000, 240_000, 480_000, 960_000, 1_800_000, 1_800_000
        ]
    );
    assert_eq!(throttle::delay_after(u32::MAX), throttle::MAX_MS);
    let locked = throttle::State {
        fails: 5,
        lock_until: 100_000,
        lock_boot: 1,
    };
    assert_eq!(throttle::remaining_ms(locked, 90_000), 10_000);
    assert_eq!(throttle::remaining_ms(locked, 200_000), 0);
    let hostile = throttle::State {
        lock_until: i64::MAX,
        ..locked
    };
    assert_eq!(
        throttle::remaining_ms(hostile, 0),
        throttle::MAX_MS,
        "never longer than the longest step"
    );
    // A restart renews the lock at its step instead of dropping it.
    assert_eq!(
        throttle::on_boot(locked, 5, 2, false),
        throttle::State {
            fails: 5,
            lock_until: 30_005,
            lock_boot: 2
        }
    );
    assert_eq!(
        throttle::on_boot(locked, 5, 1, true),
        locked,
        "same boot: as stored"
    );
}

#[test]
fn the_gate_decides_as_androids_table_does() {
    let pcs = vec!["AbC".to_owned()];
    // No PIN: nothing is locked.
    assert!(!app_needed(false, Mode::App, false));
    assert!(!stream_needed(
        false,
        Mode::Streams,
        &pcs,
        "abc",
        false,
        false
    ));
    // App lock: the app and its streams, until unlocked.
    assert!(app_needed(true, Mode::App, false) && !app_needed(true, Mode::App, true));
    assert!(stream_needed(true, Mode::App, &pcs, "x", false, false));
    assert!(!stream_needed(true, Mode::App, &pcs, "x", true, false));
    // Every stream, unless just granted.
    assert!(stream_needed(true, Mode::Streams, &pcs, "x", true, false));
    assert!(!stream_needed(true, Mode::Streams, &pcs, "x", true, true));
    // Selected PCs, matched without case.
    assert!(stream_needed(
        true,
        Mode::Selected,
        &pcs,
        "abc",
        true,
        false
    ));
    assert!(!stream_needed(
        true,
        Mode::Selected,
        &pcs,
        "other",
        true,
        false
    ));
    assert!(!stream_needed(true, Mode::Selected, &pcs, "", true, false));
    assert!(!stream_needed(true, Mode::Off, &pcs, "abc", true, false));
    // Grace: same boot, inside the minutes, never with 0.
    assert!(within_grace(true, 1000, 1000 + 9 * 60_000, 10));
    assert!(!within_grace(true, 1000, 1000 + 10 * 60_000, 10));
    assert!(!within_grace(false, 1000, 1001, 10));
    assert!(!within_grace(true, 1000, 1001, 0));
    assert!(!within_grace(true, 1000, 999, 10), "a clock that went back");
}

#[test]
fn throttle_and_grace_saturate_on_extreme_clock_values() {
    let extreme_lock = throttle::State {
        fails: 5,
        lock_until: i64::MAX,
        lock_boot: 1,
    };
    assert_eq!(throttle::remaining_ms(extreme_lock, i64::MIN), throttle::MAX_MS);
    assert_eq!(
        throttle::remaining_ms(
            throttle::State {
                lock_until: i64::MIN,
                ..extreme_lock
            },
            i64::MAX
        ),
        0
    );
    let renewed = throttle::on_boot(extreme_lock, i64::MAX - 10, 2, false);
    assert_eq!(renewed.lock_until, i64::MAX);
    let failed = throttle::failed(
        throttle::State {
            fails: u32::MAX,
            lock_until: 0,
            lock_boot: 1,
        },
        i64::MAX - 10,
        1,
    );
    assert_eq!(failed.fails, u32::MAX);
    assert_eq!(failed.lock_until, i64::MAX);

    assert!(!within_grace(true, i64::MIN, i64::MAX, 10));
    assert!(!within_grace(true, i64::MAX, i64::MIN, 10));
    let mut gate = PinGate::empty();
    gate.grant_at("pc", i64::MIN);
    assert!(!gate.granted(i64::MAX, "pc"));
    gate.grant_at("pc", i64::MAX);
    assert!(!gate.granted(i64::MIN, "pc"));
    assert!(gate.granted(i64::MAX, "pc"));
}
