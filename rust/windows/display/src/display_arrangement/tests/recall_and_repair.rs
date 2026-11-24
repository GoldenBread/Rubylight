//! Tests for Windows display database recall detection, entry restoration, and departure repair.

use super::*;
use butterpollo_media_policy::topology::Position;

#[test]
fn a_saved_layout_left_behind_by_a_crash_is_noticed_when_the_virtual_display_arrives() {
    let before = vec![node("phys", true, true), node("tv", false, false)];
    let connected = ["phys", "tv", "vdd"].map(String::from);
    let extended_stream = |current: &[Node], connected: &[String]| {
        recall_to_undo(
            persist(Arrangement::Extended, true),
            Arrangement::Extended,
            &before,
            current,
            connected,
        )
    };
    // Windows recalled a stream's saved layout as the display arrived.
    assert!(extended_stream(&[node("vdd", true, true)], &connected));
    // As Windows lays out a new display: everything stays on.
    let extended = [node("phys", true, true), node("vdd", true, false)];
    assert!(!extended_stream(&extended, &connected));
    // A display unplugged since, or off before the stream, is no sign.
    let unplugged = ["tv", "vdd"].map(String::from);
    assert!(!extended_stream(&[node("vdd", true, true)], &unplugged));
    assert!(persist(Arrangement::Exclusive, true));
    assert!(!persist(Arrangement::Exclusive, false));
    assert!(!persist(Arrangement::Extended, true));
}
#[test]
fn a_saved_layout_is_undone_for_a_layout_that_keeps_the_monitor_and_not_for_one_that_saves_its_own()
{
    let before = [node("phys", true, true)];
    let connected = ["phys", "vdd"].map(String::from);
    // What an exclusive stream's end used to save: the virtual display primary and
    // the monitor at the timing Windows picked. Measured: 2560x1440 at 60, not 120.
    let mut slow = node("phys", true, false);
    slow.mode.refresh_hz = 30.;
    let recalled = [slow, node("vdd", true, true)];
    let undo =
        |saves, arrangement| recall_to_undo(saves, arrangement, &before, &recalled, &connected);
    for arrangement in [
        Arrangement::Extended,
        Arrangement::Primary,
        Arrangement::Isolated,
        Arrangement::PrimaryIsolated,
    ] {
        assert!(
            undo(persist(arrangement, true), arrangement),
            "{arrangement:?}"
        );
    }
    assert!(
        !undo(
            persist(Arrangement::Exclusive, true),
            Arrangement::Exclusive
        ),
        "an exclusive stream switches the monitor off anyway"
    );
    // A second stream on a layout that saves its own: `display_arrived` settled it.
    assert!(!undo(true, Arrangement::Exclusive));
}
#[test]
fn the_entry_a_stream_puts_back_has_the_users_displays_as_they_had_them() {
    let mut monitor = node("monitor", true, true);
    monitor.mode.width = 2560;
    monitor.mode.refresh_hz = 120.;
    let before = snapshot(vec![monitor.clone(), node("vdd", false, false)]);
    // The exclusive layout, with the monitor just switched on by Windows.
    let mut retimed = monitor.clone();
    retimed.mode.refresh_hz = 60.;
    retimed.primary = false;
    retimed.desired_position = Position { x: 1280, y: 0 };
    let mut vdd = node("vdd", true, true);
    vdd.mode.width = 1280;
    let current = [vdd, retimed];
    let connected = ["monitor", "vdd"].map(String::from);
    let layout = entry_layout(&before, &current, &connected, "vdd").unwrap();
    let find = |id: &str| layout.nodes.iter().find(|n| n.device_id == id).unwrap();
    let monitor_now = find("monitor");
    assert_eq!(monitor_now.mode.refresh_hz, 120.);
    assert_eq!(monitor_now.desired_position, Position { x: 0, y: 0 });
    assert!(monitor_now.active && monitor_now.primary);
    let vdd_now = find("vdd");
    assert!(vdd_now.active && !vdd_now.primary);
    assert_eq!(vdd_now.desired_position, Position { x: 2560, y: 0 });
    assert_eq!(vdd_now.mode.width, 1280);
    assert_eq!(layout.nodes.len(), 2);
    // The user's HDR and scale go with them.
    assert_eq!(layout.hdr, before.hdr);
    assert_eq!(layout.scale, before.scale);
    // Nothing to save for a display that is off.
    assert!(entry_layout(&before, &[node("vdd", false, false)], &connected, "vdd").is_err());
}
#[test]
fn a_display_that_is_gone_leaves_no_gap_in_the_entry() {
    let mut monitor = node("monitor", true, true);
    monitor.mode.width = 2560;
    // A remote monitor that left during the stream: `retain_in` keeps its node on.
    let mut gone = node("remote", true, false);
    gone.desired_position = Position { x: 2560, y: 0 };
    let before = snapshot(vec![monitor, gone]);
    let current = [node("vdd", true, true)];
    let connected = ["monitor", "vdd"].map(String::from);
    let layout = entry_layout(&before, &current, &connected, "vdd").unwrap();
    assert!(!layout.nodes.iter().any(|n| n.device_id == "remote"));
    let vdd = layout.nodes.iter().find(|n| n.device_id == "vdd").unwrap();
    assert_eq!(vdd.desired_position, Position { x: 2560, y: 0 });
    // Still connected, it keeps its place and the entry goes beyond it.
    let connected = ["monitor", "remote", "vdd"].map(String::from);
    let layout = entry_layout(&before, &current, &connected, "vdd").unwrap();
    let vdd = layout.nodes.iter().find(|n| n.device_id == "vdd").unwrap();
    assert_eq!(vdd.desired_position, Position { x: 4480, y: 0 });
}
#[test]
fn a_layout_that_keeps_its_displays_apart_saves_no_entry() {
    for arrangement in [Arrangement::Isolated, Arrangement::PrimaryIsolated] {
        let mut monitor = node("monitor", true, true);
        monitor.mode.refresh_hz = 120.;
        let mut apart = node("a", true, false);
        apart.desired_position = Position { x: 64000, y: 64000 };
        let mut state = holding(arrangement, vec![monitor, apart], false);
        state.owners = vec![("a".to_owned(), 1)];
        assert!(!repair_saves(&state), "{arrangement:?}");
        // The repair at a departure still puts the monitor back, unsaved.
        let connected = ["monitor", "a"].map(String::from);
        let repair = departure_repair(
            &state,
            arrangement,
            &[node("monitor", true, true), node("a", true, false)],
            &connected,
            &[],
        )
        .unwrap();
        assert!(!repair.saves, "{arrangement:?}");
    }
}
/// Run [`put_entry_back`] with the stream's display as `current` has it: the steps
/// taken, in order, and its result.
fn entry_steps(
    current: Result<Vec<Node>>,
    restored: Result<()>,
) -> (Vec<&'static str>, Result<()>) {
    let steps = std::cell::RefCell::new(Vec::new());
    let before = snapshot(vec![node("monitor", true, true)]);
    let result = put_entry_back(
        &before,
        "vdd",
        || {
            steps.borrow_mut().push("connected");
            Ok(vec!["monitor".to_owned(), "vdd".to_owned()])
        },
        |ids| {
            assert_eq!(ids, ["monitor", "vdd"]);
            steps.borrow_mut().push("on");
            Ok(())
        },
        || current,
        |layout| {
            assert!(
                layout
                    .nodes
                    .iter()
                    .any(|n| n.device_id == "vdd" && n.active)
            );
            steps.borrow_mut().push("restore");
            restored
        },
        || {
            steps.borrow_mut().push("save");
            Ok(())
        },
    );
    (steps.into_inner(), result)
}
#[test]
fn a_stream_display_that_is_gone_saves_no_entry_for_the_users_displays() {
    // A driver reset: `set_active` dropped it and switched on the monitor at
    // Windows' timing. That is not what to file for the monitor alone.
    let (steps, result) = entry_steps(Ok(vec![node("monitor", true, false)]), Ok(()));
    assert_eq!(steps, ["connected", "on"]);
    assert!(result.is_err());
    let (steps, result) = entry_steps(
        Ok(vec![
            node("monitor", true, false),
            node("vdd", false, false),
        ]),
        Ok(()),
    );
    assert_eq!(steps, ["connected", "on"]);
    assert!(result.is_err());
    // A read that fails says nothing of it either: `settled` tries again.
    let (steps, result) = entry_steps(Err(anyhow::anyhow!("pending")), Ok(()));
    assert_eq!(steps, ["connected", "on"]);
    assert!(result.is_err());
}
#[test]
fn a_monitor_dropping_off_the_list_as_it_comes_on_stays_in_its_entry() {
    let connected = std::cell::RefCell::new(vec!["monitor".to_owned(), "vdd".to_owned()]);
    let laid_out = std::cell::RefCell::new(Vec::new());
    let before = snapshot(vec![node("monitor", true, true)]);
    put_entry_back(
        &before,
        "vdd",
        || Ok(connected.borrow().clone()),
        |_| {
            // A monitor off for a while comes up: for a moment Windows doesn't list it.
            connected.borrow_mut().retain(|id| id != "monitor");
            Ok(())
        },
        || Ok(vec![node("vdd", true, true)]),
        |layout| {
            let on = layout.nodes.iter().filter(|n| n.active);
            *laid_out.borrow_mut() = on.map(|n| n.device_id.clone()).collect();
            Ok(())
        },
        || Ok(()),
    )
    .unwrap();
    assert_eq!(*laid_out.borrow(), ["monitor", "vdd"]);
}
#[test]
fn the_stream_displays_entry_is_saved_after_its_layout_and_even_if_that_only_partly_went_back() {
    let on = || vec![node("monitor", true, false), node("vdd", true, true)];
    let (steps, result) = entry_steps(Ok(on()), Ok(()));
    assert_eq!(steps, ["connected", "on", "restore", "save"]);
    assert!(result.is_ok());
    let (steps, result) = entry_steps(Ok(on()), Err(anyhow::anyhow!("mode refused")));
    assert_eq!(steps, ["connected", "on", "restore", "save"]);
    assert!(result.is_err());
}
#[test]
fn a_repair_that_failed_still_lays_out_and_comes_back_for_the_card() {
    let laid_out = std::cell::Cell::new(false);
    let lay_out = || {
        laid_out.set(true);
        Ok(())
    };
    let failure = repair_then_lay_out(
        || Err(anyhow::anyhow!("refused").context("putting it back")),
        lay_out,
    )
    .unwrap();
    assert_eq!(failure.as_deref(), Some("putting it back: refused"));
    assert!(laid_out.get());
    // A repair that went through has nothing to say.
    assert_eq!(repair_then_lay_out(|| Ok(()), || Ok(())).unwrap(), None);
    // A layout that failed stays an error: the watch tries again.
    assert!(repair_then_lay_out(|| Ok(()), || anyhow::bail!("no")).is_err());
    assert!(repair_then_lay_out(|| anyhow::bail!("refused"), || anyhow::bail!("no")).is_err());
    // The repair comes first: the layout then lays out what it put back.
    let order = std::cell::RefCell::new(Vec::new());
    repair_then_lay_out(
        || {
            order.borrow_mut().push("repair");
            Ok(())
        },
        || {
            order.borrow_mut().push("lay out");
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(*order.borrow(), ["repair", "lay out"]);
}
fn exclusive_lease() -> State {
    holding(
        Arrangement::Exclusive,
        vec![node("monitor", false, false), node("r1", true, true)],
        false,
    )
}
#[test]
fn a_repair_under_a_layout_that_keeps_the_monitor_off_saves_no_entry_with_it_off() {
    // A remote monitor's exclusive lease, or a physical display's: nothing puts an
    // entry back, so saved it would keep the monitor dark on a later layout.
    assert!(!repair_saves(&exclusive_lease()));
    // A layout that leaves the monitor on saves what the repair put back.
    let extended = holding(
        Arrangement::Extended,
        vec![node("monitor", true, true), node("r1", true, false)],
        false,
    );
    assert!(repair_saves(&extended));
    // One that saves its own has its end to put that entry back.
    let saving = holding(
        Arrangement::Exclusive,
        vec![node("monitor", false, false), node("r1", true, true)],
        true,
    );
    assert!(repair_saves(&saving));
    // Nothing held: the entry is for the user's displays as they are.
    assert!(repair_saves(&State::default()));
}
#[test]
fn a_second_remote_monitor_under_an_exclusive_lease_is_judged_by_that_lease() {
    let state = exclusive_lease();
    let connected = ["monitor", "r1", "r2"].map(String::from);
    let before = [node("r1", true, true)];
    // The arrival made the monitor primary: exclusive has no primary to keep, and
    // its own layout puts that right.
    let moved = [
        node("monitor", true, true),
        node("r1", true, false),
        node("r2", true, false),
    ];
    assert_eq!(
        remote_monitor_repair(&state, &before, &moved, &connected),
        None
    );
    // The arrival retimed R1: repaired, and not saved with the monitor off.
    let mut slow = node("r1", true, true);
    slow.mode.refresh_hz = 30.;
    let retimed = [slow, node("r2", true, false)];
    assert_eq!(
        remote_monitor_repair(&state, &before, &retimed, &connected),
        Some(false)
    );
    // No lease: the extended check, and the entry is the user's as they had it.
    let idle = State::default();
    let before = [node("monitor", true, true)];
    let recalled = [node("monitor", true, false), node("r2", true, true)];
    assert_eq!(
        remote_monitor_repair(&idle, &before, &recalled, &connected),
        Some(true)
    );
    // A lease that ended is none.
    let mut ended = exclusive_lease();
    ended.users = 0;
    ended.before = None;
    assert_eq!(
        remote_monitor_repair(&ended, &before, &recalled, &connected),
        Some(true)
    );
}
/// Extended, stream `a` on beside the monitor at 120 Hz, as B's display left them.
fn left_by_a_departure() -> State {
    let mut monitor = node("monitor", true, true);
    monitor.mode.refresh_hz = 120.;
    let mut state = holding(
        Arrangement::Extended,
        vec![monitor, node("a", true, false)],
        false,
    );
    state.owners = vec![("a".to_owned(), 1)];
    state
}
fn at_60(id: &str, primary: bool) -> Node {
    node(id, true, primary)
}
#[test]
fn a_display_on_its_way_to_a_stream_stays_on_through_a_departures_repair() {
    let state = left_by_a_departure();
    let connected = ["monitor", "a", "c"].map(String::from);
    // C launched in the settle window: its display is on, and in no layout yet.
    let current = [
        at_60("monitor", true),
        node("a", true, false),
        node("c", true, false),
    ];
    let repair =
        departure_repair(&state, Arrangement::Extended, &current, &connected, &[]).unwrap();
    assert_eq!(repair.keep_on, ["c"]);
    assert!(!repair.saves, "the entry is the arriving display's to save");
}
#[test]
fn a_physical_display_windows_recalled_is_not_an_arriving_display() {
    // Exclusive, a display just left: Windows recalled an entry with the TV on, which
    // was off before the stream and so in no layout, while C's display is on its way.
    let mut state = holding(
        Arrangement::Exclusive,
        vec![node("monitor", false, false), node("a", true, true)],
        true,
    );
    state.owners = vec![("a".to_owned(), 1)];
    state.persists_for = vec!["a".to_owned()];
    let physical = ["monitor", "tv"].map(String::from);
    let current = [
        node("tv", true, false),
        node("a", true, true),
        node("c", true, false),
    ];
    // C stays on, the TV doesn't: left on it would stay on for the stream's life.
    assert_eq!(arriving_displays(&state, &current, &physical), ["c"]);
    let (desired, arriving) =
        compose_around_arrivals(&state, Arrangement::Exclusive, &current, &physical).unwrap();
    assert_eq!(arriving, ["c"]);
    let on = |id: &str| desired.iter().any(|n| n.device_id == id && n.active);
    assert!(on("a") && on("c"));
    assert!(!on("tv"));
    // With no virtual display arriving, what the pass lays out is saved: the entry no
    // longer has the TV on when Windows next recalls it.
    let current = [node("tv", true, false), node("a", true, true)];
    let (desired, arriving) =
        compose_around_arrivals(&state, Arrangement::Exclusive, &current, &physical).unwrap();
    assert!(arriving.is_empty());
    let saved = std::cell::Cell::new(false);
    settle_layout(&mut state, desired, None, &arriving, || {
        saved.set(true);
        Ok(())
    });
    assert!(saved.get());
    // A display whose path can't be read for the moment isn't known to be physical,
    // and might be a stream's: it stays on.
    let current = [node("noid", true, false), node("a", true, true)];
    state.applied = vec![node("a", true, true)];
    assert_eq!(arriving_displays(&state, &current, &physical), ["noid"]);
}
#[test]
fn a_repair_leaves_a_virtual_display_on_and_not_a_physical_one() {
    let state = left_by_a_departure();
    let connected = ["monitor", "a", "c", "tv"].map(String::from);
    let current = [
        at_60("monitor", true),
        node("a", true, false),
        node("c", true, false),
        node("tv", true, false),
    ];
    let physical = ["monitor", "tv"].map(String::from);
    let repair = departure_repair(
        &state,
        Arrangement::Extended,
        &current,
        &connected,
        &physical,
    )
    .unwrap();
    assert_eq!(repair.keep_on, ["c"]);
}
#[test]
fn only_a_repair_that_finds_the_same_displays_on_is_saved() {
    let state = left_by_a_departure();
    let connected = ["monitor", "a"].map(String::from);
    let run = |current: &[Node]| {
        departure_repair(&state, Arrangement::Extended, current, &connected, &[])
    };
    // Windows retimed the monitor: put back, and the entry healed.
    let repair = run(&[at_60("monitor", true), node("a", true, false)]).unwrap();
    assert!(repair.saves && repair.keep_on.is_empty());
    assert_eq!(
        repair.on.iter().map(String::as_str).collect::<Vec<_>>(),
        ["a", "monitor"]
    );
    // The monitor is off: Windows' entry or the user's Win+P, which can't be told
    // apart. On again, and not filed as if the user's choice was known.
    let repair = run(&[node("a", true, true)]).unwrap();
    assert!(!repair.saves);
    assert!(repair.on.contains("monitor"));
    // As it was left: nothing to do.
    let mut same = node("monitor", true, true);
    same.mode.refresh_hz = 120.;
    assert_eq!(run(&[same, node("a", true, false)]), None);
    // A display gone since is no change in what is on.
    let mut tv = node("tv", true, false);
    tv.desired_position = Position { x: 1920, y: 0 };
    let mut with_tv = left_by_a_departure();
    with_tv.applied.push(tv);
    let repair = departure_repair(
        &with_tv,
        Arrangement::Extended,
        &[at_60("monitor", true), node("a", true, false)],
        &connected,
        &[],
    )
    .unwrap();
    assert!(repair.saves);
    // A layout that saves its own entry has nothing to repair here.
    let mut saving = left_by_a_departure();
    saving.persisted = true;
    assert_eq!(
        departure_repair(
            &saving,
            Arrangement::Exclusive,
            &[at_60("monitor", true), node("a", true, false)],
            &connected,
            &[],
        ),
        None
    );
}
#[test]
fn a_departures_repair_under_a_layout_with_the_monitor_off_saves_nothing() {
    let mut state = left_by_a_departure();
    state.applied = vec![node("monitor", false, false), node("a", true, true)];
    // A second display the layout keeps on, retimed: that is what gets repaired.
    let mut kept = node("kept", true, false);
    kept.mode.refresh_hz = 120.;
    state.applied.push(kept);
    let connected = ["monitor", "a", "kept"].map(String::from);
    let repair = departure_repair(
        &state,
        Arrangement::Exclusive,
        &[node("a", true, true), at_60("kept", false)],
        &connected,
        &[],
    )
    .unwrap();
    assert!(!repair.saves);
}
#[test]
fn a_layout_lays_out_around_what_is_arriving_and_keeps_it_out_of_its_record() {
    // Exclusive, B just left, A stays: the monitor came back on, and C's display is
    // on its way. Switching it off now fails C's launch.
    let mut state = holding(
        Arrangement::Exclusive,
        vec![node("monitor", false, false), node("a", true, true)],
        true,
    );
    state.owners = vec![("a".to_owned(), 1)];
    state.persists_for = vec!["a".to_owned()];
    let current = [
        node("monitor", true, false),
        node("a", true, true),
        node("c", true, false),
    ];
    let (desired, arriving) =
        compose_around_arrivals(&state, Arrangement::Exclusive, &current, &[]).unwrap();
    assert_eq!(arriving, ["c"]);
    let on = |id: &str| desired.iter().any(|n| n.device_id == id && n.active);
    assert!(on("a") && on("c"));
    assert!(!on("monitor"));
    let saved = std::cell::Cell::new(false);
    settle_layout(&mut state, desired.clone(), None, &arriving, || {
        saved.set(true);
        Ok(())
    });
    assert!(!saved.get(), "C's own layout saves the entry with it in");
    assert!(!state.applied.iter().any(|n| n.device_id == "c"));
    // So it still counts as arriving at the next pass, and no less on.
    assert_eq!(arriving_displays(&state, &current, &[]), ["c"]);
    // Once C holds a lease it is no longer arriving, nor is a retained display.
    add_owner(&mut state, "c", true);
    assert!(arriving_displays(&state, &current, &[]).is_empty());
    state.owners.pop();
    state.retained.push("c".to_owned());
    assert!(arriving_displays(&state, &current, &[]).is_empty());
}
#[test]
fn what_windows_recalled_after_a_display_left_goes_back_to_the_layout_it_left() {
    let mut monitor = node("monitor", true, true);
    monitor.mode.refresh_hz = 120.;
    let mut left = node("a", false, false);
    left.desired_position = Position { x: 1920, y: 0 };
    let mut stays = node("b", true, false);
    stays.desired_position = Position { x: 3840, y: 0 };
    let applied = vec![monitor.clone(), left, stays];
    let mut before = snapshot(vec![monitor]);
    before.clone_groups = vec![vec!["monitor".to_owned(), "twin".to_owned()]];
    let layout = after_departure(&applied, Some(&before));
    let on = |nodes: &[Node]| -> Vec<(String, bool, i32, f64)> {
        nodes
            .iter()
            .map(|n| {
                (
                    n.device_id.clone(),
                    n.active,
                    n.desired_position.x,
                    n.mode.refresh_hz,
                )
            })
            .collect()
    };
    assert_eq!(on(&layout.nodes), on(&applied));
    assert_eq!(layout.clone_groups, before.clone_groups);
    // The user's other settings may have changed since: none of them is touched.
    assert!(layout.hdr.is_empty() && layout.scale.is_empty() && layout.rotation.is_empty());
    assert!(after_departure(&applied, None).clone_groups.is_empty());
}
#[test]
fn the_original_layout_leaves_out_the_streams_virtual_display() {
    let layout = original_layout(
        snapshot(vec![node("monitor", true, true), node("vdd", true, false)]),
        "vdd",
    );
    let vdd = layout.nodes.iter().find(|n| n.device_id == "vdd").unwrap();
    assert!(!vdd.active && !vdd.primary);
    assert!(
        layout
            .nodes
            .iter()
            .any(|n| n.device_id == "monitor" && n.active)
    );
    assert!(!layout.hdr.contains_key("vdd") && !layout.scale.contains_key("vdd"));
    assert!(layout.hdr.contains_key("monitor"));
    // With no other active display the layout cannot exclude it.
    let only = original_layout(
        snapshot(vec![node("monitor", false, false), node("vdd", true, true)]),
        "vdd",
    );
    assert!(only.nodes.iter().any(|n| n.device_id == "vdd" && n.active));
}
