//! Tests for arrangement ownership, persistence, arrivals, departures, and remote monitor retention.

use super::*;
use butterpollo_media_policy::topology::Position;
use std::sync::Arc;

#[test]
fn a_display_the_layout_switched_off_is_still_watched_after_another_display_left() {
    // Exclusive, two streams: the physical monitor is off, so the snapshot a
    // later layout is composed from does not list it.
    let before = vec![node("physical", true, true)];
    let applied = vec![
        node("physical", false, false),
        node("a", true, true),
        node("b", true, false),
    ];
    let mut desired = vec![node("a", false, false), node("b", true, true)];
    keep_switched_off(&mut desired, &applied);
    assert_eq!(desired.len(), 3);
    // Windows switches it on when the other virtual display is removed.
    assert_eq!(
        butterpollo_media_policy::display_policy::switched_back_on(
            &before,
            &desired,
            &["physical".to_owned(), "b".to_owned()],
        ),
        ["physical"]
    );
    // One the new layout has an answer for keeps that answer.
    let mut desired = vec![node("physical", true, true)];
    keep_switched_off(&mut desired, &[node("physical", false, false)]);
    assert!(desired[0].active);
    assert_eq!(desired.len(), 1);
}
#[test]
fn a_lease_that_ends_beside_others_drops_what_remains_of_it_without_its_lock() {
    /// Removing a display takes a while: not with the arrangement's lock held, and only
    /// once the lease has gone.
    struct Remains(Arc<std::sync::atomic::AtomicBool>);
    impl Drop for Remains {
        fn drop(&mut self) {
            assert_eq!(state().try_lock().map(|s| s.users).ok(), Some(1));
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let _shared = SHARED_STATE.lock().unwrap();
    {
        let mut state = state().lock().unwrap();
        state.users = 2;
        state.owners = vec![("a".to_owned(), 2)];
    }
    let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let lease = Lease {
        arrangement: Arrangement::Exclusive,
        target: Mutex::new("a".to_owned()),
        repair_failure: None,
        remains: Mutex::default(),
        after_cut: false,
    };
    lease.end(Remains(flag.clone()));
    assert!(gone(&flag));
    let mut state = state().lock().unwrap();
    assert_eq!(state.users, 1);
    state.users = 0;
    state.owners.clear();
}
#[test]
fn only_a_physical_display_off_before_the_stream_is_switched_off_while_the_monitor_is_away() {
    let before = [node("monitor", true, true), node("second", true, false)];
    let active = ["a", "c", "second", "tv"].map(String::from);
    // `noid` isn't known to be physical: it can be a stream's, and stays.
    let physical = ["monitor", "second", "tv"].map(String::from);
    let (off, on) = strays(&before, &active, &physical, &[]);
    assert_eq!(off, ["tv"]);
    assert_eq!(on, ["a", "c", "second"]);
    // A physical display a stream holds stays.
    let (off, on) = strays(&before, &active, &physical, &["tv".to_owned()]);
    assert!(off.is_empty());
    assert_eq!(on.len(), 4);
}
#[test]
fn a_first_exclusive_layout_knows_the_monitor_windows_already_switched_off() {
    // Windows recalled the entry an earlier stream left: the virtual display alone
    // when the layout is composed, the monitor already off.
    let before = [node("monitor", true, true)];
    let current = [node("a", true, true)];
    let desired = first_layout(Arrangement::Exclusive, &current, "a", &[], &before, true).unwrap();
    let monitor = desired.iter().find(|n| n.device_id == "monitor").unwrap();
    assert!(!monitor.active && !monitor.primary);
    // So its coming back is seen, and it is no arrival to leave on.
    assert_eq!(
        butterpollo_media_policy::display_policy::switched_back_on(
            &before,
            &desired,
            &["monitor".to_owned(), "a".to_owned()],
        ),
        ["monitor"]
    );
    let mut state = holding(Arrangement::Exclusive, desired, true);
    state.owners = vec![("a".to_owned(), 1)];
    let on = [node("monitor", true, false), node("a", true, true)];
    assert!(arriving_displays(&state, &on, &[]).is_empty());
    // A display the stream keeps on is not one to switch off.
    let retained = ["monitor".to_owned()];
    let kept = first_layout(
        Arrangement::Exclusive,
        &current,
        "a",
        &retained,
        &before,
        true,
    )
    .unwrap();
    assert!(!kept.iter().any(|n| n.device_id == "monitor"));
    // Nor does a layout that leaves the others on: one that is off stays no concern.
    let extended = first_layout(Arrangement::Extended, &current, "a", &[], &before, true).unwrap();
    assert!(!extended.iter().any(|n| n.device_id == "monitor"));
}
/// A layout that saves its entry, as the first stream's virtual display leaves it.
fn saving_state(first: &str) -> State {
    let mut state = State {
        persisted: persist(Arrangement::Exclusive, true),
        ..Default::default()
    };
    add_owner(&mut state, first, true);
    state
}
/// Lay out as `apply` ends: true when the layout was saved.
fn saves(state: &mut State, leaving: Option<&str>) -> bool {
    let saved = std::cell::Cell::new(false);
    settle_layout(state, Vec::new(), leaving, &[], || {
        saved.set(true);
        Ok(())
    });
    saved.get()
}
#[test]
fn a_layout_without_a_display_on_its_way_out_is_not_saved_under_its_entry() {
    let mut state = saving_state("a");
    add_owner(&mut state, "b", true);
    assert!(!saves(&mut state, Some("a")));
    assert!(saves(&mut state, None));
}
#[test]
fn a_remote_monitor_left_alone_saves_no_entry_that_keeps_the_monitors_off() {
    // A exclusive beside remote monitor M.
    let mut state = saving_state("a");
    add_owner(&mut state, "m", false);
    assert!(saves(&mut state, None));
    // A ends. Saved now, {P, m} would have P off, and Windows recalls it when
    // C's display goes while M's retained display stays.
    assert_eq!(
        release_owner(&mut state, "a"),
        Released {
            left: true,
            saved: true
        }
    );
    assert!(!saves(&mut state, None));
    // C arrives: its entry, {P, c, m}, is saved, and put back when it ends.
    add_owner(&mut state, "c", true);
    assert!(saves(&mut state, None));
    assert!(!release_owner(&mut state, "m").saved);
    assert!(saves(&mut state, None));
    assert!(release_owner(&mut state, "c").saved);
    assert!(!saves(&mut state, None));
}
#[test]
fn a_physical_display_left_alone_never_writes_the_users_own_entry() {
    let mut state = saving_state("a");
    add_owner(&mut state, "p2", false);
    release_owner(&mut state, "a");
    assert!(!saves(&mut state, None));
    // Its end puts nothing back either: the entry for the user's displays is theirs.
    assert_eq!(
        release_owner(&mut state, "p2"),
        Released {
            left: true,
            saved: false
        }
    );
}
#[test]
fn a_stream_sharing_a_display_keeps_its_entry_until_the_last_lease() {
    let mut state = saving_state("a");
    state.owners[0].1 += 1;
    assert!(!release_owner(&mut state, "a").left);
    assert!(saves(&mut state, None));
    assert!(release_owner(&mut state, "a").left);
    assert!(state.owners.is_empty());
    assert!(!saves(&mut state, None));
}
#[test]
fn a_layout_that_saves_nothing_does_not_start_to_for_a_later_virtual_display() {
    let mut state = State::default();
    add_owner(&mut state, "m", false);
    add_owner(&mut state, "c", true);
    assert!(!saves(&mut state, None));
    assert!(!release_owner(&mut state, "c").saved);
}
#[test]
fn a_device_joining_a_display_leaves_it_the_device_whose_screen_it_is() {
    let mut state = State::default();
    // A host that streams nothing notes whoever starts, over what an ended stream left.
    state.owners.push(("a".to_owned(), 1));
    state.devices.insert("a".to_owned(), "old".to_owned());
    note_device(&mut state, "a", "fold");
    state.owners.clear();
    add_owner(&mut state, "a", true);
    state.users = 1;
    note_device(&mut state, "b", "fold");
    add_owner(&mut state, "b", true);
    state.users = 2;
    let targets = ["a", "b"].map(String::from);
    assert!(remembered::kept_for(&targets, &state.devices, &[]));
    // Another device watches the first screen: both are still the first device's.
    note_device(&mut state, "a", "tv");
    assert_eq!(state.devices["a"], "fold");
    assert!(remembered::kept_for(&targets, &state.devices, &[]));
}
#[test]
fn a_display_that_could_not_be_laid_out_is_no_owner_and_saves_nothing() {
    let mut state = saving_state("a");
    add_owner(&mut state, "b", true);
    remove_added_owner(&mut state, "b");
    assert_eq!(state.owners, [("a".to_owned(), 1)]);
    assert_eq!(state.persists_for, ["a"]);
}
#[test]
fn only_the_stream_the_layout_makes_primary_has_a_primary_to_watch() {
    let applied = [node("a", true, true), node("b", true, false)];
    assert!(applied_primary_for(&applied, "a"));
    assert!(!applied_primary_for(&applied, "b"));
    assert!(!applied_primary_for(&applied, "gone"));
    // A display the layout switched off is nobody's primary.
    let off = [node("a", false, false), node("b", true, true)];
    assert!(!applied_primary_for(&off, "a"));
    assert!(applied_primary_for(&off, "b"));
    // Nothing applied, or a layout recorded for another owner: no primary of this stream's.
    assert!(!applied_primary_for(&[], "a"));
}
#[test]
fn a_display_leaving_a_layout_the_user_changed_lays_nothing_out_again() {
    let mut state = State {
        applied: vec![node("a", true, true), node("phys", true, false)],
        ..Default::default()
    };
    let before = departures();
    let mut applied = false;
    display_left(&mut state, "a", false, |_| {
        applied = true;
        Ok(())
    })
    .unwrap();
    // Win+P or a primary picked in Windows stays, now and after Windows settles.
    assert!(!applied);
    assert_eq!(departures(), before);
    assert!(!state.applied[0].active);
    assert!(state.applied[1].active);
    // Still as applied: laid out now, and again once Windows has settled.
    display_left(&mut state, "a", true, |_| {
        applied = true;
        Ok(())
    })
    .unwrap();
    assert!(applied);
    assert_eq!(departures(), before + 1);
    assert!(minimized_before_departure().is_some());
}
#[test]
fn only_a_recent_departure_explains_a_minimised_window() {
    let start = std::time::Instant::now();
    let windows = |ids: &[isize]| {
        ids.iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
    };
    let mut last = None;
    note_departure(&mut last, start, || windows(&[1]));
    assert_eq!(recent_departure(&last, start), Some(&windows(&[1])));
    // A second display soon after: what the first minimised was not the user's doing.
    let second = start + std::time::Duration::from_secs(9);
    note_departure(&mut last, second, || windows(&[1, 2]));
    assert_eq!(recent_departure(&last, second), Some(&windows(&[1])));
    // The streams handle it once Windows settled, past the first display's window.
    let handled = second + std::time::Duration::from_millis(1500);
    assert!(handled > start + DEPARTURE_RECENT);
    assert_eq!(recent_departure(&last, handled), Some(&windows(&[1])));
    // A stream that was paused meanwhile comes back too late to tell.
    let late = second + DEPARTURE_RECENT;
    assert_eq!(recent_departure(&last, late), None);
    note_departure(&mut last, late, || windows(&[3]));
    assert_eq!(recent_departure(&last, late), Some(&windows(&[3])));
}
#[test]
fn a_read_of_the_layout_settles_and_a_failed_one_is_no_user_change() {
    let reads = |last| {
        let mut reads = [Err(anyhow::anyhow!("Windows is applying")), Ok(last)].into_iter();
        left_as_applied(|| reads.next().unwrap())
    };
    // Windows applying a change: the read after it is the answer, either way.
    assert!(reads(true));
    assert!(!reads(false));
    // It never answered: nothing says the user changed the layout.
    assert!(left_as_applied(|| anyhow::bail!("Windows is applying")));
}
#[test]
fn a_display_arriving_beside_a_remote_monitor_left_alone_saves_no_entry() {
    let arrive = |state: &State| {
        let on = std::cell::RefCell::new(Vec::new());
        let saved = std::cell::Cell::new(0);
        settle_arrival(
            state,
            "m2",
            |ids| {
                *on.borrow_mut() = ids.to_vec();
                Ok(())
            },
            || {
                saved.set(saved.get() + 1);
                Ok(())
            },
        )
        .unwrap();
        (on.into_inner(), saved.get())
    };
    // A exclusive beside remote monitor M: a second one arriving settles the entry.
    let mut state = saving_state("a");
    add_owner(&mut state, "m", false);
    state.users = 2;
    assert_eq!(
        arrive(&state),
        (vec!["a".into(), "m".into(), "m2".into()], 1)
    );
    // A ended. The displays still go on as they should, but {P, m, m2} with P off is
    // no entry to save: nothing would forget it, and Windows would recall P dark.
    release_owner(&mut state, "a");
    state.users = 1;
    assert_eq!(arrive(&state), (vec!["m".into(), "m2".into()], 0));
    // No stream left to arrive beside.
    state.users = 0;
    assert_eq!(arrive(&state), (Vec::<String>::new(), 0));
}
#[test]
fn a_display_arriving_beside_a_stream_is_on_with_it_and_what_it_keeps() {
    let owners = [("a".to_owned(), 1), ("kept".to_owned(), 2)];
    assert_eq!(
        arrived_layout(&owners, &["monitor".to_owned(), "a".to_owned()], "b"),
        ["a", "b", "kept", "monitor"]
    );
}
#[test]
fn a_streams_display_windows_dropped_is_switched_back_on_before_the_layout() {
    let targets = ["a".to_owned(), "b".to_owned()];
    assert_eq!(
        targets_off(
            &[node("physical", true, true), node("a", true, false)],
            &targets
        ),
        ["b"]
    );
    assert!(targets_off(&[node("a", true, true), node("b", true, false)], &targets).is_empty());
    assert_eq!(
        targets_off(&[node("a", true, true), node("b", false, false)], &targets),
        ["b"]
    );
}
#[test]
fn reapplying_follows_a_display_windows_listed_under_another_name() {
    let monitor = |device_id: &str, display_name: &str| crate::display::Monitor {
        display_name: display_name.into(),
        friendly_name: String::new(),
        hdr_supported: false,
        ..crate::display::monitor(device_id, 0, false)
    };
    let monitors = [
        monitor("SAM7454", r"\\.\DISPLAY1"),
        monitor("RBY0001", r"\\.\DISPLAY25"),
    ];
    // Issue 56: the stream still knew DISPLAY23.
    let target = reapplied_target(&monitors, r"\\.\DISPLAY23", "RBY0001").unwrap();
    assert_eq!(target.display_name, r"\\.\DISPLAY25");
    assert_eq!(
        reapplied_target(&monitors, r"\\.\DISPLAY1", "RBY0001")
            .unwrap()
            .device_id,
        "SAM7454",
        "a known output name wins"
    );
    assert!(reapplied_target(&monitors, r"\\.\DISPLAY23", "").is_none());
    assert!(reapplied_target(&monitors, r"\\.\DISPLAY23", "RBY0002").is_none());
}
#[test]
fn disabling_an_original_display_requires_restoration_after_a_competing_layout_change() {
    let before = vec![node("physical", true, true), node("tv", true, false)];
    let exclusive = vec![
        node("physical", false, false),
        node("tv", false, false),
        node("vdd", true, true),
    ];
    let user_recovery = vec![node("physical", true, true), node("vdd", true, false)];
    assert!(!matches(&user_recovery, &exclusive));
    assert!(disabled_original_displays(&before, &exclusive));
    let extended = vec![
        node("physical", true, true),
        node("tv", true, false),
        node("vdd", true, false),
    ];
    assert!(!disabled_original_displays(&before, &extended));
    // A display that was already off must not cause a forced restore.
    assert!(!disabled_original_displays(
        &[node("physical", false, false)],
        &exclusive
    ));
    // Adopting an earlier failed restoration can mean the physical
    // target is already absent from the new stream's active snapshot.
    assert!(disabled_original_displays(
        &before,
        &[node("vdd", true, true)]
    ));
}
#[test]
fn an_unplugged_display_does_not_keep_the_layout_from_being_restored() {
    let mut tv = node("tv", true, false);
    tv.desired_position = Position { x: 1920, y: 0 };
    let applied = vec![node("monitor", true, true), tv];
    let current = vec![node("monitor", true, true)];
    let without_tv: std::collections::BTreeSet<String> = ["monitor".to_string()].into();
    assert!(matches_connected(&current, &applied, Some(&without_tv)));
    // Still connected but switched off: the user changed the layout.
    let with_tv: std::collections::BTreeSet<String> =
        ["monitor".to_string(), "tv".to_string()].into();
    assert!(!matches_connected(&current, &applied, Some(&with_tv)));
    assert!(!matches(&current, &applied));
    assert!(matches(&applied, &applied));
}
#[test]
fn a_remote_monitor_switched_on_mid_stream_keeps_the_streams_layout() {
    let mut state = State {
        users: 1,
        before: Some(snapshot(vec![node("phys", true, true)])),
        applied: vec![node("vdd", true, true), node("phys", false, false)],
        ..Default::default()
    };
    let mut remote = node("remote", true, false);
    remote.desired_position = Position { x: 1920, y: 0 };
    let current = vec![node("vdd", true, true), remote];
    let connected: std::collections::BTreeSet<String> =
        ["vdd", "phys", "remote"].map(String::from).into();
    assert!(!matches_connected(
        &current,
        &state.applied,
        Some(&connected)
    ));
    assert!(retain_in(&mut state, &current, "remote", true));
    assert!(matches_connected(
        &current,
        &state.applied,
        Some(&connected)
    ));
    // The restore keeps it on beside the user's own displays.
    let before = state.before.as_ref().unwrap();
    assert!(
        before
            .nodes
            .iter()
            .any(|n| n.device_id == "remote" && n.active)
    );
    assert!(
        before
            .nodes
            .iter()
            .any(|n| n.device_id == "phys" && n.active)
    );
    assert_eq!(state.retained, ["remote"]);
    // Disconnected: no longer kept on, and its removal is no change.
    assert!(!retain_in(&mut state, &[], "remote", false));
    assert!(state.retained.is_empty());
    let current = vec![node("vdd", true, true)];
    let connected: std::collections::BTreeSet<String> = ["vdd", "phys"].map(String::from).into();
    assert!(matches_connected(
        &current,
        &state.applied,
        Some(&connected)
    ));
    // Without a stream holding a layout there is nothing to record.
    let mut idle = State::default();
    assert!(!retain_in(
        &mut idle,
        &[node("remote", true, false)],
        "remote",
        true
    ));
    assert!(idle.applied.is_empty() && idle.retained.is_empty());
}
