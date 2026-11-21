//! The control stream's ENet clock must be monotonic. rusty_enet upstream defaults to the wall
//! clock, which an NTP adjustment or a clock step moves backward; ENet then reads every reliable
//! command as overdue and can time the peer out on a healthy stream. The patched copy in
//! `protocol/vendor/rusty_enet` defaults to an `Instant`-based clock that starts near zero.

use rusty_enet::{monotonic_time, HostSettings};

#[test]
fn the_default_enet_clock_is_monotonic_and_starts_near_zero() {
    let clock = HostSettings::default().time;
    let first = clock();
    // The wall clock reads about 1.7e12 ms since the epoch; a monotonic clock from process start
    // reads a few milliseconds.
    assert!(
        first < std::time::Duration::from_secs(3600),
        "the default ENet clock reads {first:?}, which looks like wall-clock time"
    );
    let mut last = first;
    for _ in 0..10_000 {
        let now = clock();
        assert!(
            now >= last,
            "the ENet clock went backward: {last:?} then {now:?}"
        );
        last = now;
    }
    assert!(monotonic_time() >= last);
}

#[test]
fn the_default_enet_clock_advances_with_real_time() {
    let clock = HostSettings::default().time;
    let before = clock();
    std::thread::sleep(std::time::Duration::from_millis(30));
    assert!(clock() - before >= std::time::Duration::from_millis(25));
}
