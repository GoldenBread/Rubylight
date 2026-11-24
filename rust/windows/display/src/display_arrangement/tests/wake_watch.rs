//! Tests for post-stream wake watches, cancellation, locking, and retries.

use super::*;
use std::sync::Arc;

#[test]
fn a_stream_end_wakes_only_the_users_displays_that_are_off_and_connected() {
    let before = [
        node("monitor", true, true),
        node("unplugged", true, false),
        node("dormant", false, false),
        node("a", true, false),
    ];
    let connected = ["monitor", "dormant", "a"].map(String::from);
    let stream_only = [node("a", true, true)];
    assert_eq!(
        displays_to_wake(&before, &stream_only, &connected, "a"),
        ["monitor"]
    );
    // On already, as when the layout left it on: nothing wakes, and the end doesn't wait.
    let both = [node("a", true, true), node("monitor", true, false)];
    assert!(displays_to_wake(&before, &both, &connected, "a").is_empty());
    // With the stream's display gone there is nothing to hold the layout with.
    let no_stream = ["monitor", "dormant"].map(String::from);
    assert!(displays_to_wake(&before, &[], &no_stream, "a").is_empty());
}
/// [`watch_wake`] on a clock that a sleep moves on: what it decided, how often it looked
/// after the display that is away, and when.
fn watched(
    connected_at: impl Fn(std::time::Duration) -> Result<Vec<String>>,
) -> (Wake, usize, std::time::Duration) {
    let now = std::cell::Cell::new(std::time::Duration::ZERO);
    let mut away = 0;
    let wake = watch_wake(
        &["monitor".to_owned()],
        || connected_at(now.get()),
        || now.get(),
        || {
            now.set(now.get() + WAKE_POLL);
            false
        },
        || away += 1,
    );
    (wake, away, now.get())
}
#[test]
fn a_woken_display_that_stays_ends_the_watch_at_its_end() {
    let (wake, away, at) = watched(|_| Ok(listed(true)));
    assert_eq!(wake, Wake::Stayed);
    assert_eq!(away, 0);
    assert_eq!(at, WAKE_WATCH);
}
#[test]
fn a_woken_display_that_drops_is_waited_for_until_it_returns() {
    use std::time::Duration;
    // The monitor measured: gone a second after it came on, back about 3.2 s later.
    let (wake, away, at) = watched(|now| {
        Ok(listed(
            !(Duration::from_secs(1)..Duration::from_millis(4200)).contains(&now),
        ))
    });
    assert_eq!(wake, Wake::CameBack);
    assert!(away > 0, "what Windows recalls meanwhile is looked after");
    assert_eq!(at, Duration::from_millis(4200));
    // Past the end of the watch too: the wait is for the return, not the watch.
    assert!(at > WAKE_WATCH);
}
#[test]
fn a_woken_display_that_never_returns_ends_the_wait_loudly() {
    use std::time::Duration;
    let (wake, _, at) = watched(|now| Ok(listed(now < Duration::from_secs(1))));
    assert_eq!(wake, Wake::StayedAway);
    assert_eq!(at, Duration::from_secs(1) + WAKE_RETURN);
}
#[test]
fn a_read_that_fails_says_nothing_of_a_woken_display() {
    use std::time::Duration;
    // Windows is still applying the change: neither a drop nor a return.
    let (wake, away, _) = watched(|now| {
        if now < Duration::from_secs(1) {
            anyhow::bail!("pending")
        }
        Ok(listed(true))
    });
    assert_eq!((wake, away), (Wake::Stayed, 0));
    let (wake, _, at) = watched(|now| {
        if now < Duration::from_secs(1) {
            Ok(listed(true))
        } else if now < Duration::from_secs(2) {
            Ok(listed(false))
        } else if now < Duration::from_secs(3) {
            anyhow::bail!("pending")
        } else {
            Ok(listed(true))
        }
    });
    assert_eq!(wake, Wake::CameBack);
    assert_eq!(at, Duration::from_secs(3));
}
#[test]
fn a_display_that_came_back_gets_its_layout_again() {
    // (entry written again, layout written again, what came of the layout)
    let run = |first_fails: bool, restored_fails: bool, wake: Wake| {
        let (puts, restores) = (std::cell::Cell::new(0), std::cell::Cell::new(0));
        let failed = |fails: bool| {
            if fails {
                anyhow::bail!("the monitor left halfway");
            }
            Ok(())
        };
        let result = after_wake(
            &["monitor".to_owned()],
            wake,
            failed(first_fails),
            failed(restored_fails),
            || {
                puts.set(puts.get() + 1);
                Some(Ok(()))
            },
            || {
                restores.set(restores.get() + 1);
                Some(Ok(()))
            },
        );
        (puts.get(), restores.get(), result.map(|r| r.is_ok()))
    };
    // The first try went well but the display dropped: its entry may be for the rest.
    assert_eq!(run(false, false, Wake::CameBack), (1, 1, Some(true)));
    assert_eq!(run(true, true, Wake::CameBack), (1, 1, Some(true)));
    // It stayed and all went well: nothing to write.
    assert_eq!(run(false, false, Wake::Stayed), (0, 0, Some(true)));
    // Still applying some other change: tried again, the entry only if that failed,
    // and the layout after it either way, as the entry's try switches displays on.
    assert_eq!(run(true, false, Wake::Stayed), (1, 1, Some(true)));
    assert_eq!(run(false, true, Wake::Stayed), (0, 1, Some(true)));
    // Gone for good: the entry is what the first try came to, and the layout has a last go.
    assert_eq!(run(false, false, Wake::StayedAway), (0, 1, Some(true)));
    assert_eq!(run(true, false, Wake::StayedAway), (0, 1, Some(true)));
    // Stopped: nothing at all, whatever went before.
    assert_eq!(run(true, true, Wake::Stopped), (0, 0, None));
}
#[test]
fn a_write_stopped_halfway_through_its_steps_leaves_the_rest_unwritten() {
    let restores = std::cell::Cell::new(0);
    let result = after_wake(
        &["monitor".to_owned()],
        Wake::CameBack,
        Ok(()),
        Ok(()),
        || None,
        || {
            restores.set(restores.get() + 1);
            Some(Ok(()))
        },
    );
    assert!(result.is_none());
    assert_eq!(restores.get(), 0);
    // A failed entry doesn't stop the layout: that is what the journal waits for.
    let result = after_wake(
        &["monitor".to_owned()],
        Wake::CameBack,
        Ok(()),
        Ok(()),
        || Some(Err(anyhow::anyhow!("still applying"))),
        || Some(Err(anyhow::anyhow!("not back"))),
    );
    assert!(result.is_some_and(|r| r.is_err()));
}
#[test]
fn a_launch_gets_past_a_slow_watch_at_once_and_it_writes_nothing() {
    assert!(STOP_BOUND < WAKE_WATCH);
    let watcher = WakeWatcher::new();
    let writes = Writes::default();
    let (display, display_gone) = FakeDisplay::new();
    assert!(start_slow_watch(&watcher, &writes, display, Ok(())));
    let launch = std::time::Instant::now();
    watcher.hold_off();
    assert!(launch.elapsed() < STOP_BOUND, "{:?}", launch.elapsed());
    // Past the stop the stream's display is gone: a new one on its slot can't meet it.
    assert!(gone(&display_gone));
    assert!(writes.lock().unwrap().is_empty());
    // It isn't done: what it was to look after goes to the end of the hold.
    assert_eq!(watcher.release(), Some(watched_left()));
}
#[test]
fn a_watch_stopped_as_the_monitor_returns_writes_nothing_and_keeps_its_journal_entry() {
    let _shared = SHARED_STATE.lock().unwrap();
    let stop = WatchStop::default();
    let writes = Writes::default();
    let looks = std::cell::Cell::new(0);
    let left = noted_watch(wake_end(Ok(()), Ok(())), &stop, &writes, || {
        looks.set(looks.get() + 1);
        if looks.get() == 1 {
            return Ok(listed(false));
        }
        // The return and the launch at the same moment: it writes for no one.
        stop.stop();
        Ok(listed(true))
    });
    assert!(left);
    assert_eq!(looks.get(), 2);
    // Only what it looked after while the monitor was away, before the stop.
    assert_eq!(*writes.lock().unwrap(), ["strays"]);
    // Stopped with the monitor away: that isn't looked after for anyone either.
    let stop = WatchStop::default();
    let writes = Writes::default();
    let left = noted_watch(wake_end(Ok(()), Ok(())), &stop, &writes, || {
        stop.stop();
        Ok(listed(false))
    });
    assert!(left);
    assert!(writes.lock().unwrap().is_empty());
}
#[test]
fn a_stop_during_a_write_ends_the_writes_after_it() {
    let _shared = SHARED_STATE.lock().unwrap();
    let writes = Writes::default();
    let looks = std::cell::Cell::new(0);
    let read = || {
        looks.set(looks.get() + 1);
        Ok(listed(looks.get() % 2 == 0))
    };
    let note = |what: &'static str| writes.lock().unwrap().push(what);
    let stop = WatchStop::default();
    let left = run_wake_watch(
        wake_end(Ok(()), Ok(())),
        &stop,
        read,
        || Ok(()),
        || {
            note("entry");
            stop.stop();
            Ok(())
        },
        || {
            note("layout");
            Ok(())
        },
        || {
            note("journal");
            Ok(())
        },
    );
    assert!(left);
    assert_eq!(*writes.lock().unwrap(), ["entry"]);
    writes.lock().unwrap().clear();
    let stop = WatchStop::default();
    let left = run_wake_watch(
        wake_end(Ok(()), Ok(())),
        &stop,
        read,
        || Ok(()),
        || {
            note("entry");
            Ok(())
        },
        || {
            note("layout");
            stop.stop();
            Ok(())
        },
        || {
            note("journal");
            Ok(())
        },
    );
    assert!(left);
    assert_eq!(*writes.lock().unwrap(), ["entry", "layout"]);
}
#[test]
fn a_stop_ends_a_watchs_wait_at_once() {
    let stop = Arc::new(WatchStop::default());
    let waiting = {
        let stop = stop.clone();
        std::thread::spawn(move || stop.wait(std::time::Duration::from_secs(30)))
    };
    std::thread::sleep(std::time::Duration::from_millis(50));
    let stopping = std::time::Instant::now();
    stop.stop();
    assert!(waiting.join().unwrap());
    assert!(stopping.elapsed() < STOP_BOUND, "{:?}", stopping.elapsed());
}
#[test]
fn a_watch_that_starts_ends_the_one_before_it() {
    let watcher = WakeWatcher::new();
    let writes = Writes::default();
    let (first, first_gone) = FakeDisplay::new();
    assert!(start_slow_watch(&watcher, &writes, first, Ok(())));
    let (second, second_gone) = FakeDisplay::new();
    let started = std::time::Instant::now();
    assert!(start_slow_watch(&watcher, &writes, second, Ok(())));
    assert!(started.elapsed() < STOP_BOUND, "{:?}", started.elapsed());
    assert!(gone(&first_gone));
    assert!(!gone(&second_gone));
    watcher.close();
    assert!(gone(&second_gone));
    assert!(writes.lock().unwrap().is_empty());
}
#[test]
fn a_watch_stopped_with_the_layout_not_back_leaves_that_to_the_end_of_the_hold_too() {
    let watcher = WakeWatcher::new();
    let writes = Writes::default();
    let (display, _) = FakeDisplay::new();
    assert!(start_slow_watch(
        &watcher,
        &writes,
        display,
        Err(anyhow::anyhow!("the monitor left halfway"))
    ));
    watcher.hold_off();
    assert_eq!(watcher.release(), Some(watched_left()));
}
#[test]
fn a_watch_left_alone_writes_after_the_return_and_only_then_clears_its_journal_entry() {
    let _shared = SHARED_STATE.lock().unwrap();
    let stop = WatchStop::default();
    let writes = Writes::default();
    let looks = std::cell::Cell::new(0);
    let left = noted_watch(wake_end(Ok(()), Ok(())), &stop, &writes, || {
        looks.set(looks.get() + 1);
        Ok(listed(looks.get() != 1))
    });
    assert!(!left);
    assert_eq!(
        *writes.lock().unwrap(),
        ["strays", "entry", "layout", "journal"]
    );
    // A layout that didn't go back keeps the journal entry, for the next start.
    let writes = Writes::default();
    looks.set(0);
    let left = run_wake_watch(
        wake_end(Ok(()), Ok(())),
        &stop,
        || {
            looks.set(looks.get() + 1);
            Ok(listed(looks.get() != 1))
        },
        || Ok(()),
        || Ok(()),
        || anyhow::bail!("not back"),
        || {
            writes.lock().unwrap().push("journal");
            Ok(())
        },
    );
    assert!(!left);
    assert!(writes.lock().unwrap().is_empty());
    // A layout the stream's end didn't owe stays out of it: the user changed it.
    let writes = Writes::default();
    looks.set(0);
    let mut end = wake_end(Ok(()), Ok(()));
    end.restore = false;
    let left = noted_watch(end, &stop, &writes, || {
        looks.set(looks.get() + 1);
        Ok(listed(looks.get() != 1))
    });
    assert!(!left);
    assert_eq!(*writes.lock().unwrap(), ["strays", "entry", "journal"]);
}
#[test]
fn the_hosts_stop_ends_a_watch_for_good_and_keeps_its_journal_entry() {
    let watcher = WakeWatcher::new();
    let writes = Writes::default();
    let (display, display_gone) = FakeDisplay::new();
    assert!(start_slow_watch(&watcher, &writes, display, Ok(())));
    let stopping = std::time::Instant::now();
    watcher.close();
    assert!(stopping.elapsed() < STOP_BOUND, "{:?}", stopping.elapsed());
    assert!(gone(&display_gone));
    assert!(writes.lock().unwrap().is_empty());
    // Nothing starts from here on, a stream ending on the way out included, and a
    // stream's end loses no time on it.
    let (display, display_gone) = FakeDisplay::new();
    assert!(!start_slow_watch(&watcher, &writes, display, Ok(())));
    assert!(gone(&display_gone));
    // The journal entry stays for the next start: nothing clears it, and a hold's end
    // doesn't open the watcher again.
    watcher.close();
    watcher.hold_off();
    assert!(watcher.release().is_none());
    let (display, _) = FakeDisplay::new();
    assert!(!start_slow_watch(&watcher, &writes, display, Ok(())));
}
#[test]
fn stopping_a_watch_again_or_with_none_running_is_safe() {
    let watcher = WakeWatcher::new();
    // None ever ran.
    watcher.hold_off();
    assert!(watcher.release().is_none());
    let writes = Writes::default();
    let (display, _) = FakeDisplay::new();
    assert!(start_slow_watch(&watcher, &writes, display, Ok(())));
    // Two launches, the second while the first prepares: each gets past, and the entry
    // is judged once, when the last ends.
    watcher.hold_off();
    watcher.hold_off();
    assert!(watcher.release().is_none());
    assert!(watcher.release().is_some());
    // Started again once they have ended, and a watch that finished is nothing to stop.
    assert_eq!(
        watcher.start(watched_left(), |_| false),
        WatchStart::Started
    );
    while watcher
        .0
        .lock()
        .unwrap()
        .running
        .as_ref()
        .is_some_and(|w| !w.thread.is_finished())
    {
        std::thread::yield_now();
    }
    watcher.hold_off();
    assert!(watcher.release().is_none(), "it finished and left nothing");
}
#[test]
fn a_stream_ending_during_a_launch_starts_no_watch() {
    let watcher = WakeWatcher::new();
    let writes = Writes::default();
    watcher.hold_off();
    let (display, display_gone) = FakeDisplay::new();
    assert!(!start_slow_watch(&watcher, &writes, display, Ok(())));
    // Its display goes at once, and the watch it did not get ran nothing.
    assert!(gone(&display_gone));
    assert!(watcher.0.lock().unwrap().running.is_none());
    assert!(writes.lock().unwrap().is_empty());
    assert_eq!(watcher.release(), Some(watched_left()));
    // Only once: a later launch finds nothing to judge.
    watcher.hold_off();
    assert!(watcher.release().is_none());
}
#[test]
fn a_watch_writes_only_under_the_arrangements_lock_and_not_for_a_stream_that_began() {
    let _shared = SHARED_STATE.lock().unwrap();
    let stop = WatchStop::default();
    let wrote = std::cell::Cell::new(0);
    let write = || {
        wrote.set(wrote.get() + 1);
        // Held by the step, so no lease can begin in the middle of its write.
        assert!(state().try_lock().is_err());
        Ok(())
    };
    assert!(watch_step(&stop, write).is_some());
    assert_eq!(wrote.get(), 1);
    state().lock().unwrap().users = 1;
    let began = watch_step(&stop, write);
    state().lock().unwrap().users = 0;
    assert!(began.is_none());
    stop.stop();
    assert!(watch_step(&stop, write).is_none());
    assert_eq!(wrote.get(), 1);
}
#[test]
fn a_watchs_retries_end_at_a_stop_and_try_again_otherwise() {
    let _shared = SHARED_STATE.lock().unwrap();
    let stop = WatchStop::default();
    let tries = std::cell::Cell::new(0);
    let result = settled_step(&stop, || {
        tries.set(tries.get() + 1);
        if tries.get() < 3 {
            anyhow::bail!("still applying");
        }
        Ok(())
    });
    assert!(result.is_some_and(|r| r.is_ok()));
    assert_eq!(tries.get(), 3);
    // A stop during the wait between tries: no further try, and no wait for its end.
    let tries = std::cell::Cell::new(0);
    let started = std::time::Instant::now();
    let result = settled_step(&stop, || -> Result<()> {
        tries.set(tries.get() + 1);
        stop.stop();
        anyhow::bail!("still applying")
    });
    assert!(result.is_none());
    assert_eq!(tries.get(), 1);
    assert!(started.elapsed() < SETTLE_RETRY);
}
#[test]
fn only_a_display_outside_the_users_layout_makes_a_stream_end_watch() {
    let before = [node("monitor", true, true), node("dormant", false, false)];
    // A PC with its monitor and nothing else: nothing Windows could switch on instead.
    let others = ["monitor".to_owned()];
    assert!(displays_it_could_switch_on(&before, &others).is_empty());
    // A TV that is off, listed or not in the layout from before the stream.
    let others = ["monitor", "dormant", "tv"].map(String::from);
    assert_eq!(
        displays_it_could_switch_on(&before, &others),
        ["dormant", "tv"]
    );
}
#[test]
fn a_pc_with_nothing_else_connected_watches_nothing_and_reads_no_further() {
    let before = [node("monitor", true, true)];
    let connected = ["monitor", "a"].map(String::from);
    let others = ["monitor".to_owned()];
    let unread = || -> Result<Vec<Node>> { panic!("nothing to protect, nothing to read") };
    let watched = woken_to_watch(&before, unread, &connected, &others, "a").unwrap();
    assert!(watched.is_empty());
    // With a TV that is off the same end has the monitor to watch.
    let connected = ["monitor", "tv", "a"].map(String::from);
    let others = ["monitor", "tv"].map(String::from);
    let active = || Ok(vec![node("a", true, true)]);
    let watched = woken_to_watch(&before, active, &connected, &others, "a").unwrap();
    assert_eq!(watched, ["monitor"]);
}
