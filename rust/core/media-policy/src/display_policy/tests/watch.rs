//! LayoutWatch, PrimaryWatch, and HeartbeatThrottle tests.

use super::*;

#[test]
fn an_extended_primary_start_without_primary_is_reported_and_watched() {
    use super::{PrimaryAction, PrimaryWatch, chosen_layout, layout_mismatch};
    let vd = r"\\.\DISPLAY9";
    let active = vec![(vd.to_string(), false), (r"\\.\DISPLAY1".to_string(), true)];
    // The start-of-stream check sees a layout Windows did not take, even though learning
    // must not keep that demotion for the device.
    assert_eq!(
        layout_mismatch("extended_primary", vd, &active),
        Some("extended")
    );
    assert_eq!(chosen_layout("extended_primary", vd, &active), None);
    // The start is a change Windows made as it took the layout: the watch puts the primary
    // back after the settle (issue #33's stream opened with the display on but not primary).
    let start = std::time::Instant::now();
    let mut watch = PrimaryWatch::starting(start);
    assert_eq!(watch.observe(true, start), PrimaryAction::Wait);
    assert_eq!(
        watch.observe(true, start + super::LayoutWatch::SETTLE),
        PrimaryAction::Restore
    );
    // The first heartbeat can come after the event window: the start still decides.
    let mut watch = PrimaryWatch::starting(start);
    let late = start + PrimaryWatch::WINDOW * 2;
    assert_eq!(watch.observe(true, late), PrimaryAction::Wait);
    assert_eq!(
        watch.observe(true, late + super::LayoutWatch::SETTLE),
        PrimaryAction::Restore
    );
    // A watch that did not start is not armed by a move Windows made before it existed.
    let mut watch = PrimaryWatch::default();
    assert_eq!(
        watch.observe(true, start + super::LayoutWatch::SETTLE),
        PrimaryAction::Wait
    );
}

#[test]
fn a_delayed_primary_move_after_a_display_change_is_restored() {
    use super::{PrimaryAction, PrimaryChange, PrimaryWatch};
    let start = std::time::Instant::now();
    let settle = super::LayoutWatch::SETTLE;
    // A display changed; the first look still finds the stream's display primary, which is
    // not the end of the change's window: Windows can move the primary a moment later.
    let mut watch = PrimaryWatch::default();
    watch.changed(PrimaryChange::Display, start);
    watch.settled(start);
    let moved = start + std::time::Duration::from_secs(2);
    assert_eq!(watch.observe(true, moved), PrimaryAction::Wait);
    assert_eq!(watch.observe(true, moved + settle), PrimaryAction::Restore);
}

#[test]
fn a_move_first_seen_late_in_the_window_still_settles() {
    use super::{PrimaryAction, PrimaryChange, PrimaryWatch};
    let start = std::time::Instant::now();
    let settle = super::LayoutWatch::SETTLE;
    let mut watch = PrimaryWatch::default();
    watch.changed(PrimaryChange::Display, start);
    // The first look already sees the move, just before the change's window ends: it must
    // hold its settle and be put back, rather than being left to the user.
    let seen = start + PrimaryWatch::WINDOW - settle / 2;
    assert_eq!(watch.observe(true, seen), PrimaryAction::Wait);
    assert_eq!(watch.observe(true, seen + settle), PrimaryAction::Restore);
}

#[test]
fn a_refused_restore_stays_armed_through_its_retry() {
    use super::{PrimaryAction, PrimaryChange, PrimaryWatch};
    let start = std::time::Instant::now();
    let settle = super::LayoutWatch::SETTLE;
    let retry = super::LayoutWatch::RETRY;
    let mut watch = PrimaryWatch::default();
    watch.changed(PrimaryChange::Display, start);
    // The move is first seen a moment before the window ends, so the retry after a refusal
    // falls outside it; the watch must still put the primary back then.
    let seen = start + PrimaryWatch::WINDOW - settle / 2;
    assert_eq!(watch.observe(true, seen), PrimaryAction::Wait);
    assert_eq!(watch.observe(true, seen + settle), PrimaryAction::Restore);
    assert!(!watch.failed(seen + settle));
    let after = seen + settle + retry;
    assert!(after > start + PrimaryWatch::WINDOW);
    assert!(watch.due(after));
    assert_eq!(watch.observe(true, after), PrimaryAction::Wait);
    assert_eq!(watch.observe(true, after + settle), PrimaryAction::Restore);
}

#[test]
fn a_refused_start_stops_after_the_attempt_limit() {
    use super::{PrimaryAction, PrimaryChange, PrimaryWatch};
    let start = std::time::Instant::now();
    let settle = super::LayoutWatch::SETTLE;
    let retry = super::LayoutWatch::RETRY;
    let mut watch = PrimaryWatch::starting(start);
    let mut at = start;
    for attempt in 1..=PrimaryWatch::ATTEMPTS {
        assert_eq!(watch.observe(true, at), PrimaryAction::Wait);
        assert_eq!(watch.observe(true, at + settle), PrimaryAction::Restore);
        assert_eq!(
            watch.failed(at + settle),
            attempt == PrimaryWatch::ATTEMPTS,
            "attempt {attempt}"
        );
        at += retry;
    }
    assert!(watch.stopped());
    // The watch is done for the stream, whatever Windows changes later.
    let later = start + retry * 8;
    watch.changed(PrimaryChange::Display, later);
    assert_eq!(watch.observe(true, later), PrimaryAction::Wait);
    assert_eq!(watch.observe(true, later + settle), PrimaryAction::Wait);
}

#[test]
fn an_interrupted_look_starts_its_settle_again() {
    use super::{PrimaryAction, PrimaryChange, PrimaryWatch};
    let start = std::time::Instant::now();
    let settle = super::LayoutWatch::SETTLE;
    let mut watch = PrimaryWatch::default();
    watch.changed(PrimaryChange::Display, start);
    assert_eq!(watch.observe(true, start), PrimaryAction::Wait);
    // The stream paused before the move settled, or the lock screen hid the displays: the
    // move must settle again after the resume or unlock, however long that wait.
    watch.interrupted();
    let resumed = start + settle * 4;
    assert_eq!(watch.observe(true, resumed), PrimaryAction::Wait);
    assert_eq!(
        watch.observe(true, resumed + settle),
        PrimaryAction::Restore
    );
}

#[test]
fn refused_restores_stop_the_watch_like_restores_do() {
    use super::{PrimaryAction, PrimaryChange, PrimaryWatch};
    let start = std::time::Instant::now();
    let settle = super::LayoutWatch::SETTLE;
    let mut watch = PrimaryWatch::default();
    let mut stopped = false;
    // The first display change's refusal is a one-off; the quick ones after it are the
    // display fighting back.
    for round in 0..=PrimaryWatch::RETURNS as u64 {
        let at = start + std::time::Duration::from_secs(round * 10);
        watch.changed(PrimaryChange::Display, at);
        assert_eq!(watch.observe(true, at), PrimaryAction::Wait);
        assert_eq!(watch.observe(true, at + settle), PrimaryAction::Restore);
        stopped = watch.failed(at + settle);
        assert_eq!(watch.stopped(), stopped);
        assert_eq!(stopped, round >= PrimaryWatch::RETURNS as u64);
    }
    assert!(stopped);
    // A change Windows makes later leaves the primary where it is too.
    let later = start + std::time::Duration::from_secs(120);
    watch.changed(PrimaryChange::Display, later);
    assert_eq!(watch.observe(true, later), PrimaryAction::Wait);
    // A refusal outside the window does not count towards the limit.
    let mut watch = PrimaryWatch::default();
    let mut stopped = false;
    for round in 0..PrimaryWatch::RETURNS as u32 {
        let at = start + PrimaryWatch::RETURN_WINDOW * (round + 1) * 2;
        watch.changed(PrimaryChange::Display, at);
        assert_eq!(watch.observe(true, at), PrimaryAction::Wait);
        assert_eq!(watch.observe(true, at + settle), PrimaryAction::Restore);
        stopped = watch.failed(at + settle);
    }
    assert!(!stopped);
    assert!(!watch.stopped());
}

#[test]
fn a_primary_move_windows_made_is_put_back_but_a_plain_one_is_left_alone() {
    use super::{Arrangement, PrimaryAction, PrimaryChange, PrimaryWatch, primary_moved};
    // Only the layouts that keep the user's displays on and make the stream's display
    // primary have a primary to put back.
    for planned in [Arrangement::Primary, Arrangement::PrimaryIsolated] {
        assert!(primary_moved(planned, Some(false)), "{planned:?}");
        assert!(!primary_moved(planned, Some(true)), "{planned:?}");
        // Off, or Windows still laying it out: the recovery and the layout watch own that.
        assert!(!primary_moved(planned, None), "{planned:?}");
    }
    for planned in [
        Arrangement::Extended,
        Arrangement::Exclusive,
        Arrangement::Isolated,
    ] {
        assert!(!primary_moved(planned, Some(false)), "{planned:?}");
    }
    // A primary picked while nothing changed the displays is the user's or the game's, and
    // stays, however long it stays: only a display arriving or leaving starts the watch.
    let start = std::time::Instant::now();
    let mut watch = PrimaryWatch::default();
    for seconds in [0, 1, 10, 300, 3600] {
        let now = start + std::time::Duration::from_secs(seconds);
        assert_eq!(
            watch.observe(true, now),
            PrimaryAction::Wait,
            "no change at {seconds}s"
        );
    }
    // Windows changed the layout (a display arrived or left, a session switch): the move
    // that follows must hold for the settle time, then it is put back.
    watch.changed(PrimaryChange::Display, start);
    assert_eq!(watch.observe(true, start), PrimaryAction::Wait);
    assert_eq!(
        watch.observe(true, start + super::LayoutWatch::SETTLE / 2),
        PrimaryAction::Wait
    );
    assert_eq!(
        watch.observe(true, start + super::LayoutWatch::SETTLE),
        PrimaryAction::Restore
    );
    assert!(!watch.restored(start + super::LayoutWatch::SETTLE));
    // A restore Windows refused waits before the next try.
    watch.changed(PrimaryChange::Display, start);
    assert_eq!(watch.observe(true, start), PrimaryAction::Wait);
    assert_eq!(
        watch.observe(true, start + super::LayoutWatch::SETTLE),
        PrimaryAction::Restore
    );
    watch.failed(start + super::LayoutWatch::SETTLE);
    assert!(!watch.due(start + super::LayoutWatch::SETTLE));
    let later = start + super::LayoutWatch::SETTLE + super::LayoutWatch::RETRY;
    assert!(watch.due(later));
    assert_eq!(
        watch.observe(true, later),
        PrimaryAction::Wait,
        "the move is timed from the first look after the wait"
    );
}

#[test]
fn a_display_that_keeps_taking_the_primary_back_is_left_alone_after_three_fights() {
    use super::{PrimaryAction, PrimaryChange, PrimaryWatch};
    let start = std::time::Instant::now();
    let mut watch = PrimaryWatch::default();
    let mut restores = 0;
    // Issue #56: a monitor that drops its link and reappears every 30-60 s makes Windows move
    // the primary each time. The first restore answers the change; the quick ones after it
    // are the display fighting back, and the third fight stops the watch for the stream.
    for round in 0..=PrimaryWatch::RETURNS as u64 {
        let at = start + std::time::Duration::from_secs(round * 60);
        watch.changed(PrimaryChange::Display, at);
        assert_eq!(watch.observe(true, at), PrimaryAction::Wait);
        if watch.observe(true, at + super::LayoutWatch::SETTLE) == PrimaryAction::Restore {
            restores += 1;
            watch.restored(at + super::LayoutWatch::SETTLE);
        }
        assert_eq!(watch.stopped(), restores > PrimaryWatch::RETURNS);
        if watch.stopped() {
            break;
        }
    }
    assert_eq!(restores, PrimaryWatch::RETURNS + 1);
    assert!(watch.stopped());
    // Even a change Windows makes later leaves the primary where it is.
    let later = start + PrimaryWatch::RETURN_WINDOW * 2;
    watch.changed(PrimaryChange::Display, later);
    assert_eq!(watch.observe(true, later), PrimaryAction::Wait);
    assert_eq!(
        watch.observe(true, later + super::LayoutWatch::SETTLE),
        PrimaryAction::Wait
    );
}

#[test]
fn a_start_or_session_restore_is_not_counted_as_fighting_back() {
    use super::{PrimaryAction, PrimaryChange, PrimaryWatch};
    let start = std::time::Instant::now();
    let settle = super::LayoutWatch::SETTLE;
    let restore = |watch: &mut PrimaryWatch, at| {
        assert_eq!(watch.observe(true, at), PrimaryAction::Wait);
        assert_eq!(watch.observe(true, at + settle), PrimaryAction::Restore);
        watch.restored(at + settle)
    };
    // The start's restore and the lock screen's or a session switch's are one-offs.
    let mut watch = PrimaryWatch::starting(start);
    assert!(!restore(&mut watch, start));
    let mut at = start + std::time::Duration::from_secs(10);
    for kind in [PrimaryChange::Session, PrimaryChange::Secure] {
        watch.changed(kind, at);
        assert!(!restore(&mut watch, at));
        at += std::time::Duration::from_secs(10);
    }
    // The first display change's restore answers it; the quick ones after it are the fights.
    watch.changed(PrimaryChange::Display, at);
    assert!(!restore(&mut watch, at));
    for fight in 1..=PrimaryWatch::RETURNS as u64 {
        at += std::time::Duration::from_secs(60);
        watch.changed(PrimaryChange::Display, at);
        assert_eq!(
            restore(&mut watch, at),
            fight >= PrimaryWatch::RETURNS as u64
        );
    }
}

#[test]
fn a_move_that_stopped_being_seen_starts_its_settle_again() {
    use super::{PrimaryAction, PrimaryChange, PrimaryWatch};
    let start = std::time::Instant::now();
    let mut watch = PrimaryWatch::default();
    watch.changed(PrimaryChange::Display, start);
    assert_eq!(watch.observe(true, start), PrimaryAction::Wait);
    // The display is off for a look (Windows laying it out, or the stream's display gone):
    // the watch has nothing pending, and the move is timed from where it is seen again.
    watch.settled(start);
    assert_eq!(
        watch.observe(true, start + super::LayoutWatch::SETTLE),
        PrimaryAction::Wait
    );
    assert_eq!(
        watch.observe(
            true,
            start + super::LayoutWatch::SETTLE + super::LayoutWatch::SETTLE
        ),
        PrimaryAction::Restore
    );
}

#[test]
fn an_excuse_lasts_the_window_and_no_longer() {
    use super::LayoutWatch;
    let start = std::time::Instant::now();
    let mut watch = LayoutWatch::default();
    assert!(!watch.excused(start));
    watch.excuse(start);
    assert!(watch.excused(start));
    assert!(watch.excused(start + LayoutWatch::EXCUSE / 2));
    assert!(!watch.excused(start + LayoutWatch::EXCUSE));
}

#[test]
fn a_display_the_exclusive_layout_switched_off_is_put_back_off_after_it_settles() {
    use std::time::{Duration, Instant};
    // Before the stream: the physical monitor on its own, a TV switched
    // off. The exclusive layout keeps only the virtual display on.
    let mut phys = node("phys", 0);
    phys.primary = true;
    let mut tv = node("tv", 1920);
    tv.active = false;
    let mut vdd = node("vdd", 0);
    vdd.active = false;
    let before = vec![phys.clone(), tv.clone(), vdd.clone()];
    let applied = Arrangement::Exclusive
        .compose(&[phys.clone(), tv.clone(), node("vdd", 3840)], "vdd", &[])
        .unwrap();
    let on = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert!(switched_back_on(&before, &applied, &on(&["vdd"])).is_empty());
    // The Win key over an exclusive-fullscreen game: Windows recalls its
    // saved layout and the physical monitor comes back beside the stream.
    assert_eq!(
        switched_back_on(&before, &applied, &on(&["vdd", "phys"])),
        ["phys"]
    );
    // The TV was off before the stream: switching it on is the user's call.
    assert!(switched_back_on(&before, &applied, &on(&["vdd", "tv"])).is_empty());
    // Another client's display arriving was never laid out by this stream.
    assert!(switched_back_on(&before, &applied, &on(&["vdd", "vdd2"])).is_empty());
    // The extended layout switches nothing off, so there is nothing to keep off.
    let extended = Arrangement::Extended
        .compose(&[phys.clone(), tv, node("vdd", 3840)], "vdd", &[])
        .unwrap();
    assert!(switched_back_on(&before, &extended, &on(&["vdd", "phys"])).is_empty());

    let start = Instant::now();
    let at = |ms: u64| start + Duration::from_millis(ms);
    let mut watch = LayoutWatch::default();
    assert!(watch.due(at(0)));
    assert!(!watch.due(at(100)));
    assert!(watch.due(at(250)));
    // A game's own mode switch that settles within the window is left alone.
    let phys = vec!["phys".to_string()];
    assert_eq!(watch.observe(&phys, at(0)), LayoutAction::Wait);
    assert_eq!(watch.observe(&phys, at(250)), LayoutAction::Wait);
    assert_eq!(watch.observe(&[], at(500)), LayoutAction::Wait);
    assert_eq!(watch.observe(&phys, at(750)), LayoutAction::Wait);
    assert_eq!(watch.observe(&phys, at(1000)), LayoutAction::Wait);
    // Still on after the settle time: put the layout back once.
    assert_eq!(
        watch.observe(&phys, at(1250)),
        LayoutAction::Reapply {
            displays: phys.clone(),
            kept_now: vec![]
        }
    );
    assert_eq!(watch.observe(&phys, at(1500)), LayoutAction::Wait);
    // A refused attempt waits before the next check.
    watch.failed(at(1500));
    assert!(!watch.due(at(3000)));
    assert!(watch.due(at(3500)));
}

#[test]
fn a_monitor_that_keeps_coming_back_is_left_on() {
    use std::time::{Duration, Instant};
    let start = Instant::now();
    let at = |s: u64| start + Duration::from_secs(s);
    let settled = |watch: &mut LayoutWatch, displays: &[String], s: u64| {
        assert_eq!(watch.observe(displays, at(s)), LayoutAction::Wait);
        watch.observe(displays, at(s) + LayoutWatch::SETTLE)
    };
    let reapply = |displays: &[String]| LayoutAction::Reapply {
        displays: displays.to_vec(),
        kept_now: vec![],
    };
    let phys = vec!["phys".to_string()];
    let tv = vec!["tv".to_string()];
    let mut watch = LayoutWatch::default();
    // Refused attempts (the secure desktop) are not returns.
    for s in [0, 2, 4, 6] {
        assert_eq!(settled(&mut watch, &phys, s), reapply(&phys));
        watch.failed(at(s));
    }
    // Returns spread wider than the window keep the fight going.
    for s in [100, 320, 640, 660] {
        assert_eq!(settled(&mut watch, &phys, s), reapply(&phys));
        watch.reapplied(&phys, at(s));
    }
    // Another display's returns count on their own.
    for s in [670, 680] {
        assert_eq!(settled(&mut watch, &tv, s), reapply(&tv));
        watch.reapplied(&tv, at(s));
    }
    // Issue #56: the third return of the same monitor within the window leaves it on.
    assert_eq!(
        settled(&mut watch, &phys, 700),
        LayoutAction::Reapply {
            displays: vec![],
            kept_now: phys.clone()
        }
    );
    assert_eq!(watch.kept(), phys);
    // The watch goes on for the other displays; the kept one is no return.
    assert!(watch.due(at(800)));
    assert_eq!(watch.observe(&phys, at(800)), LayoutAction::Wait);
}

#[test]
fn a_monitor_kept_on_does_not_stop_switching_another_off() {
    use std::time::{Duration, Instant};
    let start = Instant::now();
    let at = |s: u64| start + Duration::from_secs(s);
    let phys = "phys".to_string();
    let tv = "tv".to_string();
    let mut watch = LayoutWatch::default();
    for s in [0, 30] {
        watch.observe(std::slice::from_ref(&phys), at(s));
        watch.observe(std::slice::from_ref(&phys), at(s) + LayoutWatch::SETTLE);
        watch.reapplied(std::slice::from_ref(&phys), at(s));
    }
    // Phys's third return comes with the TV's first: the TV is still switched off.
    let both = [phys.clone(), tv.clone()];
    assert_eq!(watch.observe(&both, at(60)), LayoutAction::Wait);
    assert_eq!(
        watch.observe(&both, at(60) + LayoutWatch::SETTLE),
        LayoutAction::Reapply {
            displays: vec![tv.clone()],
            kept_now: vec![phys.clone()]
        }
    );
    watch.reapplied(std::slice::from_ref(&tv), at(60));
    // Later TV returns are still handled while phys stays on.
    assert_eq!(watch.observe(&both, at(90)), LayoutAction::Wait);
    assert_eq!(
        watch.observe(&both, at(90) + LayoutWatch::SETTLE),
        LayoutAction::Reapply {
            displays: vec![tv],
            kept_now: vec![]
        }
    );
}

#[test]
fn a_return_after_the_lock_screen_or_a_session_switch_is_not_counted() {
    use std::time::{Duration, Instant};
    let start = Instant::now();
    let at = |s: u64| start + Duration::from_secs(s);
    let phys = vec!["phys".to_string()];
    let mut watch = LayoutWatch::default();
    let back = |watch: &mut LayoutWatch, s: u64| {
        watch.observe(&phys, at(s));
        let action = watch.observe(&phys, at(s) + LayoutWatch::SETTLE);
        watch.reapplied(&phys, at(s));
        action
    };
    // Unlocking five times: Windows recalls its layout each time.
    for s in [0, 40, 80, 120, 160] {
        watch.excuse(at(s));
        back(&mut watch, s + 5);
    }
    assert!(watch.kept().is_empty());
    // A successful reapply right after a refused one (the secure desktop) is excused too.
    watch.failed(at(200));
    back(&mut watch, 203);
    back(&mut watch, 240);
    back(&mut watch, 250);
    assert!(watch.kept().is_empty());
    // Two real returns counted: the third leaves it on.
    assert_eq!(
        back(&mut watch, 260),
        LayoutAction::Reapply {
            displays: vec![],
            kept_now: phys.clone()
        }
    );
}
#[test]
fn a_display_that_left_lays_the_stream_out_again_once_windows_has_settled() {
    let mut watch = LayoutWatch::default();
    let start = std::time::Instant::now();
    assert!(!watch.departure_due(start));
    watch.display_left(start);
    assert!(!watch.departure_due(start + LayoutWatch::DEPARTURE_SETTLE / 2));
    assert!(watch.departure_due(start + LayoutWatch::DEPARTURE_SETTLE));
    // A failed attempt stays due; a done one is over.
    assert!(watch.departure_due(start + LayoutWatch::DEPARTURE_SETTLE * 2));
    watch.departure_handled();
    assert!(!watch.departure_due(start + LayoutWatch::DEPARTURE_SETTLE * 3));
}

#[test]
fn heartbeat_throttle_warns_once_per_error_and_backs_off() {
    use std::time::Duration;
    let base = Duration::from_millis(100);
    let max = Duration::from_secs(1);
    let mut throttle = HeartbeatThrottle::default();

    assert_eq!(throttle.next_tick(base, max), Duration::from_millis(100));
    assert!(!throttle.note_ok());

    assert!(throttle.note_error("selected display does not support HDR"));
    assert_eq!(throttle.next_tick(base, max), Duration::from_millis(100));

    assert!(!throttle.note_error("selected display does not support HDR"));
    assert_eq!(throttle.next_tick(base, max), Duration::from_millis(200));

    assert!(!throttle.note_error("selected display does not support HDR"));
    assert_eq!(throttle.next_tick(base, max), Duration::from_millis(400));

    assert!(!throttle.note_error("selected display does not support HDR"));
    assert_eq!(throttle.next_tick(base, max), Duration::from_millis(800));

    assert!(!throttle.note_error("selected display does not support HDR"));
    assert_eq!(throttle.next_tick(base, max), Duration::from_secs(1));

    // A distinct error warns again and resets backoff to base for the first retry.
    assert!(throttle.note_error("Access is denied."));
    assert_eq!(throttle.next_tick(base, max), Duration::from_millis(100));

    // Recovering reports once and restores normal cadence.
    assert!(throttle.note_ok());
    assert!(!throttle.note_ok());
    assert_eq!(throttle.next_tick(base, max), Duration::from_millis(100));
    assert!(throttle.note_error("selected display does not support HDR"));
}
