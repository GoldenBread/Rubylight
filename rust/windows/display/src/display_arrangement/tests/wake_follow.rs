//! Tests for mid-stream wake follow-up, repair retries, and cut-watch handoff.

use super::*;
use butterpollo_media_policy::topology::Position;

#[test]
fn only_a_layout_that_keeps_the_others_on_follows_what_a_stopped_watch_left() {
    let before = [node("monitor", true, true), node("rm", true, false)];
    let connected = ["monitor", "tv", "b", "rm"].map(String::from);
    let retained = ["rm".to_owned()];
    let follow = |arrangement| wake_follow(arrangement, &before, &connected, "b", &retained);
    assert!(follow(Arrangement::Exclusive).is_none());
    for arrangement in [
        Arrangement::Extended,
        Arrangement::Primary,
        Arrangement::Isolated,
        Arrangement::PrimaryIsolated,
    ] {
        let follow = follow(arrangement).unwrap();
        // The user's own: not what others hold, which comes and goes on its own.
        assert_eq!(follow.woken, ["monitor"]);
        assert_eq!(follow.looks.gone_since, None);
    }
    // Away as the lease starts: returning is all it can do.
    let away = ["tv", "b", "rm"].map(String::from);
    let follow = wake_follow(Arrangement::Extended, &before, &away, "b", &retained).unwrap();
    assert_eq!(follow.looks.gone_since, Some(std::time::Duration::ZERO));
    // Nothing of the user's to wait for.
    let nothing = [node("rm", true, false), node("b", true, true)];
    assert!(wake_follow(Arrangement::Extended, &nothing, &connected, "b", &retained).is_none());
}
#[test]
fn a_followed_display_gets_strays_switched_off_while_away_and_one_layout_when_back() {
    let before = [node("monitor", true, true)];
    let at = std::time::Duration::from_millis;
    let with = ["monitor", "b"].map(String::from);
    let without = ["b".to_owned()];
    // Away as the stream started.
    let mut follow = wake_follow(Arrangement::Extended, &before, &without, "b", &[]).unwrap();
    assert_eq!(
        follow.step(at(250), Some(&without)),
        FollowStep::SwitchOffStrays
    );
    // A read that fails says nothing of it.
    assert_eq!(follow.step(at(500), None), FollowStep::SwitchOffStrays);
    assert_eq!(follow.step(at(750), Some(&with)), FollowStep::LayOut);
    // Windows still applying a change: another try on the next look, a bounded number.
    assert!(follow.tried(false));
    assert_eq!(follow.step(at(1000), Some(&with)), FollowStep::LayOut);
    assert!(!follow.tried(true));
    assert_eq!(follow.step(at(1250), Some(&with)), FollowStep::Nothing);
    let mut follow = wake_follow(Arrangement::Extended, &before, &without, "b", &[]).unwrap();
    for _ in 1..FOLLOW_TRIES {
        assert!(follow.tried(false));
    }
    assert!(!follow.tried(false));
    assert_eq!(follow.step(at(250), Some(&with)), FollowStep::Nothing);
    // There as the stream started: it can still drop, which is watched as ever.
    let mut follow = wake_follow(Arrangement::Extended, &before, &with, "b", &[]).unwrap();
    assert_eq!(follow.step(at(250), Some(&with)), FollowStep::Nothing);
    assert_eq!(
        follow.step(at(500), Some(&without)),
        FollowStep::SwitchOffStrays
    );
    assert_eq!(follow.step(at(5000), Some(&with)), FollowStep::LayOut);
    // Or it stays: that is looked at too, as it can have gone and returned between looks.
    let mut follow = wake_follow(Arrangement::Extended, &before, &with, "b", &[]).unwrap();
    assert_eq!(follow.step(WAKE_WATCH, Some(&with)), FollowStep::LayOut);
    // Gone for good, the stream goes on as it is and asks no more.
    let mut follow = wake_follow(Arrangement::Extended, &before, &without, "b", &[]).unwrap();
    assert_eq!(
        follow.step(WAKE_RETURN, Some(&without)),
        FollowStep::Nothing
    );
    assert_eq!(
        follow.step(WAKE_RETURN + at(250), Some(&with)),
        FollowStep::Nothing
    );
}
#[test]
fn a_returned_monitor_needs_its_displays_put_back_only_if_they_are_not_as_they_were() {
    let before = [at_hz("monitor", true, true, 120.)];
    let connected = ["monitor", "tv", "b"].map(String::from);
    let physical = ["monitor", "tv"].map(String::from);
    let streams = ["b".to_owned()];
    let wrong = |current: &[Node], keep: &[String], keeps_primary: bool| {
        wake_layout_wrong(
            &before,
            current,
            &connected,
            &physical,
            &streams,
            keep,
            keeps_primary,
        )
    };
    let b = at_hz("b", true, false, 60.);
    let keep = streams.clone();
    // As it was: no write but its place in the layout, so the stream isn't disturbed.
    assert!(!wrong(
        &[at_hz("monitor", true, true, 120.), b.clone()],
        &keep,
        true
    ));
    // Windows' timing.
    assert!(wrong(
        &[at_hz("monitor", true, true, 60.), b.clone()],
        &keep,
        true
    ));
    // Back and dark.
    assert!(wrong(std::slice::from_ref(&b), &keep, true));
    // Windows lit the TV when it returned.
    assert!(wrong(
        &[
            at_hz("monitor", true, true, 120.),
            at_hz("tv", true, false, 60.),
            b.clone()
        ],
        &keep,
        true
    ));
    // The primary counts only on a layout that keeps it.
    let primary_b = at_hz("b", true, true, 60.);
    let moved = [at_hz("monitor", true, false, 120.), primary_b];
    assert!(wrong(&moved, &keep, true));
    assert!(!wrong(&moved, &keep, false));
    // A physical display the stream holds is no stray.
    let keep_tv = ["b".to_owned(), "tv".to_owned()];
    assert!(!wrong(
        &[
            at_hz("monitor", true, true, 120.),
            at_hz("tv", true, false, 60.),
            b
        ],
        &keep_tv,
        true
    ));
}
#[test]
fn switching_off_a_stray_saves_only_an_entry_with_the_users_displays_on() {
    let before = [node("monitor", true, true), node("second", true, false)];
    let on = ["b", "second"].map(String::from);
    // The monitor away: the entry is for what is connected, and has nothing wrong.
    assert!(strays_off_saves(
        &before,
        &on,
        &["second", "tv", "b"].map(String::from)
    ));
    // Connected and off: saved, Windows would recall it off from now on.
    assert!(!strays_off_saves(
        &before,
        &on,
        &["monitor", "second", "tv", "b"].map(String::from)
    ));
}
#[test]
fn laying_out_around_a_returned_monitor_keeps_what_others_hold_on() {
    let current = [
        node("a", true, true),
        node("b", true, false),
        node("rm", true, false),
        node("tv", true, false),
    ];
    let keep = ["a", "b", "rm"].map(String::from);
    let nodes = with_kept(vec![node("monitor", true, true)], &current, &keep);
    let on: Vec<_> = nodes.iter().map(|n| n.device_id.as_str()).collect();
    assert_eq!(on, ["monitor", "a", "b", "rm"]);
}
#[test]
fn a_display_kept_in_a_layout_going_back_goes_to_its_right_and_not_where_it_is_now() {
    // Measured: a remote monitor beside what Windows lit, a TV 3840 wide: in a layout with
    // the user's monitor, 2560 wide, that is a gap.
    let mut monitor = node("monitor", true, true);
    monitor.mode.width = 2560;
    let mut rm = node("rm", true, false);
    rm.mode.width = 1280;
    rm.desired_position = Position { x: 3840, y: 0 };
    let mut held = node("held", true, false);
    held.mode.width = 1280;
    held.desired_position = Position { x: 7000, y: 0 };
    let mut there = held.clone();
    there.desired_position = Position { x: 2560, y: 0 };
    let keep = ["rm", "held"].map(String::from);
    let nodes = with_kept(vec![monitor.clone(), there], &[rm.clone(), held], &keep);
    let place = |id: &str| {
        let node = nodes.iter().find(|n| n.device_id == id).unwrap();
        (node.desired_position.x, node.primary)
    };
    assert_eq!(place("monitor"), (0, true));
    // Already on in the layout: it stays where the layout has it.
    assert_eq!(place("held"), (2560, false));
    assert_eq!(place("rm"), (3840, false));
    // Off now: it isn't on to keep.
    let mut gone = rm.clone();
    gone.active = false;
    assert_eq!(with_kept(vec![monitor.clone()], &[gone], &keep).len(), 1);
    // Off in the layout: where it was doesn't count either.
    let mut off = rm.clone();
    off.active = false;
    off.desired_position = Position { x: 9000, y: 0 };
    let nodes = with_kept(vec![monitor, off], &[rm], &keep);
    let rm = nodes.iter().find(|n| n.device_id == "rm").unwrap();
    assert!(rm.active);
    assert_eq!(rm.desired_position.x, 2560);
    assert_eq!(nodes.len(), 2);
}
#[test]
fn a_remote_monitor_in_the_gap_goes_by_the_users_layout_and_not_what_windows_lit() {
    let lit = snapshot(vec![node("tv", true, true)]);
    let users = snapshot(vec![node("monitor", true, true)]);
    let by = |count, settling, pending| {
        remote_monitor_baseline(count, settling, pending, &lit).nodes[0]
            .device_id
            .clone()
    };
    assert_eq!(by(0, true, Some(&users)), "monitor");
    // Nothing left: the layout from just before it. A lease held: its own entry is no news.
    assert_eq!(by(0, true, None), "tv");
    assert_eq!(by(1, true, Some(&users)), "tv");
    // An entry stuck long since, with nothing at work: what is on is the user's by now.
    assert_eq!(by(0, false, Some(&users)), "tv");
}
#[test]
fn a_pass_for_a_returned_display_leaves_what_arrives_or_is_leased_and_saves_nothing_then() {
    let state = State {
        owners: vec![("v".to_owned(), 1)],
        retained: vec!["rm".to_owned()],
        applied: vec![node("v", true, true), node("monitor", true, false)],
        ..State::default()
    };
    let physical = ["monitor", "c", "tv"].map(String::from);
    let current = [
        node("v", true, true),
        node("v2", true, false),
        node("c", true, false),
    ];
    let (keep, arrives) = beside_the_streams(&state, &current, &physical, &["c".to_owned()]);
    assert!(arrives);
    for id in ["v", "rm", "v2", "c"] {
        assert!(keep.iter().any(|k| k == id), "{id}");
    }
    // The layout the pass switches on has them, and a TV Windows lit is still no one's.
    let layout =
        layout_around_streams(snapshot(vec![node("monitor", true, true)]), &current, &keep);
    let on: Vec<_> = layout.nodes.iter().map(|n| n.device_id.as_str()).collect();
    assert_eq!(on, ["monitor", "v", "v2", "c"]);
    let current = [node("v", true, true), node("tv", true, false)];
    let (keep, arrives) = beside_the_streams(&state, &current, &physical, &[]);
    assert!(!arrives);
    assert!(!keep.iter().any(|k| k == "tv"));
}
#[test]
fn a_pass_for_a_returned_display_only_places_what_a_stream_set() {
    let mut before = snapshot(vec![
        at_hz("stream", true, true, 60.),
        at_hz("monitor", true, false, 120.),
    ]);
    before.hdr.insert("stream".to_owned(), false);
    before.hdr.insert("monitor".to_owned(), true);
    before.scale.insert("stream".to_owned(), 100);
    before.scale.insert("monitor".to_owned(), 125);
    before.rotation.insert("stream".to_owned(), 0);
    before.rotation.insert("monitor".to_owned(), 90);
    let current = [at_hz("stream", true, false, 144.)];
    let layout = layout_around_streams(before, &current, &["stream".to_owned()]);
    assert!(!layout.hdr.contains_key("stream"));
    assert_eq!(layout.hdr.get("monitor"), Some(&true));
    assert!(!layout.scale.contains_key("stream"));
    assert_eq!(layout.scale.get("monitor"), Some(&125));
    assert!(!layout.rotation.contains_key("stream"));
    assert!(layout.rotation.contains_key("monitor"));
    let hz = |id: &str| {
        let node = layout.nodes.iter().find(|n| n.device_id == id).unwrap();
        node.mode.refresh_hz
    };
    assert_eq!(hz("stream"), 144.);
    assert_eq!(hz("monitor"), 120.);
}
#[test]
fn a_repair_goes_once_and_the_stream_gets_its_layout_anyway() {
    let (repairs, layouts) = (std::cell::Cell::new(0), std::cell::Cell::new(0));
    // `repair`: what the repair answers this go, `None` when it gets through.
    let told = AtomicU64::new(0);
    let go = |state: &mut WakeRepair, wrong, repair: Option<&str>, laid_out: bool| {
        pass_after_wake(
            state,
            wrong,
            &told,
            || {
                repairs.set(repairs.get() + 1);
                match repair {
                    Some(answer) => Err(anyhow::anyhow!("{answer}")),
                    None => Ok(()),
                }
            },
            || {
                layouts.set(layouts.get() + 1);
                if laid_out {
                    Ok(())
                } else {
                    anyhow::bail!("pending")
                }
            },
        )
    };
    const REFUSED: &str = "display layout restored except a monitor's mode";
    let mut state = WakeRepair::default();
    let failure = go(&mut state, true, Some(REFUSED), true).unwrap();
    assert!(failure.unwrap().contains("a monitor's mode"));
    assert_eq!((repairs.get(), layouts.get()), (1, 1));
    // Still wrong at the layout's next try: no second pass on the running stream.
    assert_eq!(go(&mut state, true, None, true).unwrap(), None);
    assert_eq!((repairs.get(), layouts.get()), (1, 2));
    // A layout that failed is tried again, the repair not.
    let mut state = WakeRepair::default();
    assert!(go(&mut state, true, None, false).is_err());
    assert_eq!(go(&mut state, true, None, true).unwrap(), None);
    assert_eq!((repairs.get(), layouts.get()), (2, 4));
    // Nothing wrong: the layout alone, and a later wrong still gets its one repair.
    let mut state = WakeRepair::default();
    assert_eq!(go(&mut state, false, None, true).unwrap(), None);
    assert!(!state.done);
    assert_eq!(repairs.get(), 2);
    assert_eq!(
        (go(&mut state, true, None, true).unwrap(), state.done),
        (None, true)
    );
    assert_eq!(repairs.get(), 3);
}
#[test]
fn a_repair_that_failed_for_now_is_tried_again_before_any_layout() {
    let (repairs, layouts) = (std::cell::Cell::new(0), std::cell::Cell::new(0));
    let told = AtomicU64::new(0);
    let mut state = WakeRepair::default();
    let pass = pass_after_wake(
        &mut state,
        true,
        &told,
        || {
            repairs.set(repairs.get() + 1);
            anyhow::bail!("a layout change is pending")
        },
        || {
            layouts.set(layouts.get() + 1);
            Ok(())
        },
    );
    // An error, so the follow goes again. The layout would compose from what is on, with
    // the monitor off and what Windows lit on, and end the follow there.
    assert!(pass.unwrap_err().to_string().contains("pending"));
    assert_eq!((repairs.get(), layouts.get()), (1, 0));
    assert!(!state.done);
    assert_eq!(told.load(Ordering::Relaxed), 0);
    let pass = pass_after_wake(
        &mut state,
        true,
        &told,
        || {
            repairs.set(repairs.get() + 1);
            Ok(())
        },
        || {
            layouts.set(layouts.get() + 1);
            Ok(())
        },
    );
    assert_eq!(pass.unwrap(), None);
    assert_eq!((repairs.get(), layouts.get()), (2, 1));
    assert!(state.done);
}
#[test]
fn what_a_display_refused_reaches_the_card_though_the_layout_took_another_go() {
    let told = AtomicU64::new(0);
    let refused = || -> Result<()> { anyhow::bail!("display layout restored except a mode") };
    let mut state = WakeRepair::default();
    let pending = || anyhow::bail!("pending");
    assert!(pass_after_wake(&mut state, true, &told, refused, pending).is_err());
    assert!(state.done);
    let failure = pass_after_wake(
        &mut state,
        true,
        &told,
        || panic!("no second pass"),
        || Ok(()),
    )
    .unwrap();
    assert!(failure.unwrap().contains("a mode"));
}
#[test]
fn the_streams_hear_of_a_repair_when_its_layout_is_through() {
    let told = AtomicU64::new(0);
    let heard = || told.load(Ordering::Relaxed);
    let mut state = WakeRepair::default();
    // Each stream reapplies its mode when it hears; a layout still to come can recall it.
    let pending = || anyhow::bail!("pending");
    assert!(pass_after_wake(&mut state, true, &told, || Ok(()), pending).is_err());
    assert_eq!(heard(), 0);
    pass_after_wake(&mut state, true, &told, || Ok(()), || Ok(())).unwrap();
    assert_eq!(heard(), 1);
    // A repair a display refused can have moved things just the same.
    let mut state = WakeRepair::default();
    let refused = || -> Result<()> { anyhow::bail!("display layout restored except a mode") };
    pass_after_wake(&mut state, true, &told, refused, || Ok(())).unwrap();
    assert_eq!(heard(), 2);
    // No repair, nothing to hear.
    let mut state = WakeRepair::default();
    pass_after_wake(&mut state, false, &told, || Ok(()), || Ok(())).unwrap();
    assert_eq!(heard(), 2);
}
#[test]
fn only_a_stream_joining_while_a_cut_watch_is_at_work_counts_as_after_it() {
    assert!(!joins_a_cut_watch(None));
    let before = [node("monitor", true, true)];
    let mut follow = wake_follow(Arrangement::Extended, &before, &[], "a", &[]).unwrap();
    assert!(joins_a_cut_watch(Some(&follow)));
    follow.done = true;
    assert!(!joins_a_cut_watch(Some(&follow)));
}
#[test]
fn an_end_that_failed_goes_to_a_watch_over_the_users_displays() {
    let before = snapshot(vec![
        node("monitor", true, true),
        node("rm", true, false),
        node("held", true, false),
        node("off", false, false),
    ]);
    let held = ["held".to_owned()];
    let refused = || Err(anyhow::anyhow!("invalid parameter"));
    let left = what_an_end_leaves(refused(), Some(before.clone()), "rm", &held);
    let EndLeaves::Left(left) = left else {
        panic!("an end that failed goes to a watch");
    };
    assert_eq!(left.woken, ["monitor"]);
    assert!(left.restore);
    assert!(matches!(
        what_an_end_leaves(refused(), None, "rm", &held),
        EndLeaves::Nothing
    ));
    assert!(matches!(
        what_an_end_leaves(Ok(None), Some(before.clone()), "rm", &held),
        EndLeaves::Nothing
    ));
    let end = WakeEnd {
        woken: Vec::new(),
        restore: false,
        first: Ok(()),
        restored: Ok(()),
    };
    assert!(matches!(
        what_an_end_leaves(Ok(Some(end)), Some(before), "rm", &held),
        EndLeaves::Watch(..)
    ));
}
#[test]
fn a_layout_without_the_display_at_its_origin_begins_there_with_what_is_left() {
    let mut tv = node("tv", true, false);
    tv.desired_position = Position { x: 2560, y: 0 };
    let before = snapshot(vec![node("monitor", true, true), tv]);
    let places = |connected: &[String]| -> Vec<(String, i32)> {
        present_layout(&before, connected)
            .nodes
            .iter()
            .map(|n| (n.device_id.clone(), n.desired_position.x))
            .collect()
    };
    let both = ["monitor", "tv"].map(String::from);
    assert_eq!(
        places(&both),
        [("monitor".to_owned(), 0), ("tv".to_owned(), 2560)]
    );
    assert_eq!(places(&["tv".to_owned()]), [("tv".to_owned(), 0)]);
    assert_eq!(places(&["monitor".to_owned()]), [("monitor".to_owned(), 0)]);
    assert!(places(&[]).is_empty());
    // A display left of the primary is no reason to move anything.
    let mut left = node("left", true, false);
    left.desired_position = Position { x: -1920, y: 0 };
    let before = snapshot(vec![node("monitor", true, true), left]);
    let layout = present_layout(&before, &["monitor", "left"].map(String::from));
    assert_eq!(layout.nodes[1].desired_position.x, -1920);
    assert_eq!(layout.nodes[0].desired_position.x, 0);
}
#[test]
fn a_lease_ending_clears_what_it_followed() {
    let _shared = SHARED_STATE.lock().unwrap();
    let before = [node("monitor", true, true)];
    {
        let mut state = state().lock().unwrap();
        state.users = 1;
        state.owners = vec![("a".to_owned(), 1)];
        state.following = wake_follow(Arrangement::Extended, &before, &[], "a", &[]);
    }
    let lease = Lease {
        arrangement: Arrangement::Extended,
        target: Mutex::new("a".to_owned()),
        repair_failure: None,
        remains: Mutex::default(),
        after_cut: true,
    };
    assert!(lease.after_cut_watch());
    drop(lease);
    assert!(state().lock().unwrap().following.is_none());
}
