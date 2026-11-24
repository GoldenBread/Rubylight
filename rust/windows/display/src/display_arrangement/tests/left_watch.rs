//! Tests for watches left behind by launch holds, settling state, and baseline selection.

use super::*;
use std::sync::Arc;

/// [`left_watch`] with its writes noted and displays as `read` says.
fn noted_left_watch(
    stop: &WatchStop,
    writes: &Writes,
    read: impl FnMut() -> Result<Vec<String>>,
    restore: impl FnMut() -> Result<()>,
) -> bool {
    let note = |what: &'static str| {
        let writes = writes.clone();
        move || {
            writes.lock().unwrap().push(what);
            Ok(())
        }
    };
    left_watch(
        watched_left(),
        stop,
        read,
        note("strays"),
        restore,
        note("journal"),
    )
}
#[test]
fn a_launch_with_no_lease_has_the_layout_go_back_when_the_monitor_does_and_only_then_the_entry() {
    let _shared = SHARED_STATE.lock().unwrap();
    let stop = WatchStop::default();
    let writes = Writes::default();
    let looks = std::cell::Cell::new(0);
    let left = noted_left_watch(
        &stop,
        &writes,
        || {
            looks.set(looks.get() + 1);
            // Nothing is on but what Windows lit, away and away, and then it is back.
            assert_eq!(
                writes
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|w| **w != "strays")
                    .count(),
                0,
                "nothing but strays while it is away"
            );
            Ok(listed(looks.get() >= 3))
        },
        || {
            writes.lock().unwrap().push("layout");
            Ok(())
        },
    );
    assert!(!left);
    assert_eq!(
        *writes.lock().unwrap(),
        ["strays", "strays", "layout", "journal"]
    );
    // One that never dropped has its look all the same: what is on may still be wrong by
    // then, gone and returned while the launch took its time ([`layout_is_back`]).
    let writes = Writes::default();
    let left = noted_left_watch(
        &stop,
        &writes,
        || Ok(listed(true)),
        || {
            writes.lock().unwrap().push("layout");
            Ok(())
        },
    );
    assert!(!left);
    assert_eq!(*writes.lock().unwrap(), ["layout", "journal"]);
}
#[test]
fn a_layout_that_did_not_go_back_keeps_its_journal_entry_however_the_hold_ended() {
    let _shared = SHARED_STATE.lock().unwrap();
    let stop = WatchStop::default();
    let writes = Writes::default();
    let looks = std::cell::Cell::new(0);
    let left = noted_left_watch(
        &stop,
        &writes,
        || {
            looks.set(looks.get() + 1);
            Ok(listed(looks.get() != 1))
        },
        || anyhow::bail!("the monitor left again"),
    );
    assert_eq!(*writes.lock().unwrap(), ["strays"]);
    writes.lock().unwrap().clear();
    // Finished: the next stream or start has it, as after any restore that failed.
    assert!(!left);
    assert!(writes.lock().unwrap().is_empty());
}
#[test]
fn a_left_watch_stopped_by_the_next_launch_writes_nothing_and_is_left_again() {
    let _shared = SHARED_STATE.lock().unwrap();
    let stop = WatchStop::default();
    let writes = Writes::default();
    let left = noted_left_watch(
        &stop,
        &writes,
        || {
            stop.stop();
            Ok(listed(true))
        },
        || {
            writes.lock().unwrap().push("layout");
            Ok(())
        },
    );
    assert!(left);
    assert!(writes.lock().unwrap().is_empty());
}
#[test]
fn a_left_watch_changes_nothing_for_a_lease_that_began_and_runs_again_after_a_hold() {
    let _shared = SHARED_STATE.lock().unwrap();
    state().lock().unwrap().users = 1;
    let stop = WatchStop::default();
    let writes = Writes::default();
    let looks = std::cell::Cell::new(0);
    let left = noted_left_watch(
        &stop,
        &writes,
        || {
            looks.set(looks.get() + 1);
            Ok(listed(looks.get() != 1))
        },
        || {
            writes.lock().unwrap().push("layout");
            Ok(())
        },
    );
    state().lock().unwrap().users = 0;
    assert!(left);
    assert!(writes.lock().unwrap().is_empty());
    // A watcher runs one on what a stopped one left, and a new hold stops that in turn.
    let watcher = WakeWatcher::new();
    let (display, _) = FakeDisplay::new();
    assert!(start_slow_watch(&watcher, &writes, display, Ok(())));
    watcher.hold_off();
    let left = watcher.release().unwrap();
    let again = {
        let writes = writes.clone();
        watcher.start(left, move |stop| {
            noted_left_watch(
                stop,
                &writes,
                || Ok(listed(true)),
                || {
                    writes.lock().unwrap().push("layout");
                    Ok(())
                },
            )
        })
    };
    assert_eq!(again, WatchStart::Started);
    assert!(watcher.settling());
    watcher.hold_off();
    assert_eq!(watcher.release(), Some(watched_left()));
    assert!(writes.lock().unwrap().is_empty());
}
#[test]
fn what_a_watch_left_is_known_before_its_display_goes() {
    /// The display of a watch a hold refused: while it goes, which takes a while, that
    /// hold ends. The end of the hold has to know then.
    struct EndsTheHold(Arc<WakeWatcher>, Arc<Mutex<Option<LeftWatch>>>);
    impl Drop for EndsTheHold {
        fn drop(&mut self) {
            *self.1.lock().unwrap() = self.0.release();
        }
    }
    let watcher = Arc::new(WakeWatcher::new());
    watcher.hold_off();
    let found = Arc::new(Mutex::new(None));
    let display = EndsTheHold(watcher.clone(), found.clone());
    let start = watcher.start(watched_left(), move |_| {
        drop(display);
        false
    });
    assert_eq!(start, WatchStart::Refused);
    assert_eq!(*found.lock().unwrap(), Some(watched_left()));
    // The next hold finds nothing of it.
    watcher.hold_off();
    assert!(watcher.release().is_none());
}
#[test]
fn a_watch_with_no_thread_leaves_its_entry_only_if_the_layout_is_not_back() {
    let watcher = WakeWatcher::new();
    let (display, display_gone) = FakeDisplay::new();
    let start = watcher.start_on(
        watched_left(),
        move |_| {
            drop(display);
            false
        },
        |_| Err(std::io::Error::other("no thread")),
    );
    assert_eq!(start, WatchStart::NoThread);
    assert!(gone(&display_gone));
    assert!(watcher.0.lock().unwrap().running.is_none());
    assert!(entry_goes_unwatched(&start, true));
    assert!(!entry_goes_unwatched(&start, false));
    // Refused, the end of the hold or the next start has it.
    assert!(!entry_goes_unwatched(&WatchStart::Refused, true));
    assert!(!entry_goes_unwatched(&WatchStart::Started, true));
}
#[test]
fn the_displays_count_as_settling_while_a_watch_works_or_left_work() {
    let watcher = WakeWatcher::new();
    assert!(!watcher.settling());
    let writes = Writes::default();
    let (display, _) = FakeDisplay::new();
    assert!(start_slow_watch(&watcher, &writes, display, Ok(())));
    assert!(watcher.settling());
    watcher.hold_off();
    assert!(watcher.settling(), "what it left is not looked after yet");
    assert!(watcher.release().is_some());
    assert!(!watcher.settling());
    // One that finished isn't.
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
    assert!(!watcher.settling());
}
#[test]
fn a_watch_that_takes_over_leaves_nothing_of_the_one_it_stopped() {
    let watcher = WakeWatcher::new();
    let writes = Writes::default();
    let (display, _) = FakeDisplay::new();
    assert!(start_slow_watch(&watcher, &writes, display, Ok(())));
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
    assert!(!watcher.settling());
    watcher.hold_off();
    assert!(watcher.release().is_none());
}
#[test]
fn several_woken_displays_are_listed_only_when_each_one_is() {
    let woken = ["a", "b"].map(String::from);
    assert!(all_listed(&woken, &["a", "b", "tv"].map(String::from)));
    assert!(!all_listed(&woken, &["a".to_owned()]));
    assert!(!all_listed(&woken, &[]));
}
#[test]
fn a_layout_is_back_only_with_the_monitor_on_at_its_mode_and_nothing_else_lit() {
    let before = [at_hz("monitor", true, true, 120.)];
    let both = ["monitor", "tv"].map(String::from);
    let tv_only = ["tv".to_owned()];
    let monitor = at_hz("monitor", true, true, 120.);
    let back = |current: &[Node], connected: &[String], keep: &[String]| {
        layout_is_back(&before, current, connected, &both, keep)
    };
    assert!(back(std::slice::from_ref(&monitor), &both, &[]));
    // What Windows lit with the monitor away and then left on.
    assert!(!back(
        &[monitor.clone(), at_hz("tv", true, false, 60.)],
        &both,
        &[]
    ));
    // The monitor connected and left off, or at Windows' timing, or no longer primary.
    assert!(!back(&[at_hz("tv", true, true, 60.)], &both, &[]));
    assert!(!back(&[at_hz("monitor", true, true, 60.)], &both, &[]));
    assert!(!back(
        &[
            at_hz("monitor", true, false, 120.),
            at_hz("a", true, true, 60.)
        ],
        &both,
        &[]
    ));
    // A display someone holds is no stray, and one away can't be asked for.
    assert!(back(
        &[monitor.clone(), at_hz("tv", true, false, 60.)],
        &both,
        &["tv".to_owned()]
    ));
    assert!(back(&[], &tv_only, &[]));
    // A display not known physical (a stream's) is no stray either.
    assert!(back(&[monitor, at_hz("rm", true, false, 60.)], &both, &[]));
}
#[test]
fn a_pending_baseline_keeps_a_remote_monitor_that_arrived_since_on() {
    let pending = snapshot(vec![node("monitor", true, true), node("old", false, false)]);
    let current = snapshot(vec![
        node("rm", true, false),
        node("old", true, false),
        node("tv", true, true),
        node("v", true, false),
    ]);
    let retained = ["rm", "old", "gone"].map(String::from);
    let before = first_baseline(
        Some(pending.clone()),
        None,
        &current,
        "rm",
        &retained,
        false,
    );
    let on: Vec<_> = before
        .nodes
        .iter()
        .filter(|n| n.active)
        .map(|n| n.device_id.as_str())
        .collect();
    // In a layout that goes back, the leased one stays on. The stray and the stream's
    // aren't taken in, and a retained display that isn't on has no node to take.
    assert_eq!(on, ["monitor", "rm", "old"]);
    assert_eq!(before.nodes.len(), 3);
    // With nothing pending the layout is the one found or the one from before the display.
    let found = first_baseline(None, None, &current, "v", &retained, false);
    assert_eq!(found.nodes.len(), current.nodes.len());
    let original = snapshot(vec![node("monitor", true, true), node("v", true, false)]);
    let made = first_baseline(None, Some(original), &current, "v", &retained, true);
    assert!(!made.nodes.iter().any(|n| n.device_id == "v" && n.active));
    let no_original = first_baseline(None, None, &current, "v", &retained, true);
    assert_eq!(no_original.nodes.len(), current.nodes.len());
    assert!(
        !no_original
            .nodes
            .iter()
            .any(|n| n.device_id == "v" && n.active)
    );
}
