//! Display arrangement composition, hotplug primary preservation, and recall tests.

use super::*;

#[test]
fn a_layout_chosen_during_the_stream_is_kept_for_the_device() {
    use super::chosen_layout;
    let on = |list: &[(&str, bool)]| -> Vec<(String, bool)> {
        list.iter().map(|(n, p)| (n.to_string(), *p)).collect()
    };
    let vd = r"\\.\DISPLAY9";
    // Issue #33: extended, then Windows' "Show only on 2".
    assert_eq!(
        chosen_layout("extended", vd, &on(&[(vd, true)])),
        Some("exclusive")
    );
    // Made the main display while extended.
    assert_eq!(
        chosen_layout("extended", vd, &on(&[(vd, true), (r"\\.\DISPLAY1", false)])),
        Some("extended_primary")
    );
    // A primary plan keeps its primary: Windows or a game moving it back to a
    // monitor is not the user's choice, so it is never learned as "extended".
    assert_eq!(
        chosen_layout(
            "extended_primary",
            vd,
            &on(&[(vd, false), (r"\\.\DISPLAY1", true)])
        ),
        None
    );
    // The user's "Show only on 2" from there is learned.
    assert_eq!(
        chosen_layout("extended_primary", vd, &on(&[(vd, true)])),
        Some("exclusive")
    );
    // Unchanged, the stream's display off ("Show only on 1"), an exclusive
    // stream (kept by the host itself) and isolated layouts learn nothing.
    assert_eq!(
        chosen_layout("extended", vd, &on(&[(vd, false), (r"\\.\DISPLAY1", true)])),
        None
    );
    assert_eq!(
        chosen_layout("extended", vd, &on(&[(r"\\.\DISPLAY1", true)])),
        None
    );
    assert_eq!(
        chosen_layout(
            "exclusive",
            vd,
            &on(&[(vd, false), (r"\\.\DISPLAY1", true)])
        ),
        None
    );
    assert_eq!(
        chosen_layout("extended_isolated", vd, &on(&[(vd, true)])),
        None
    );
    // Windows names displays in either case.
    assert_eq!(
        chosen_layout("extended", r"\\.\display9", &on(&[(vd, true)])),
        Some("exclusive")
    );
}

#[test]
fn a_recalled_layout_is_seen_in_a_display_off_at_another_mode_or_no_longer_primary() {
    let names = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let connected = names(&["monitor", "vdd"]);
    let before = [node("monitor", 0)];
    let beside = node("vdd", 1920);
    let recalled = |now: Node, keeps_primary| {
        recalled_over(
            &before,
            &[now, beside.clone()],
            &connected,
            &[],
            keeps_primary,
        )
    };
    assert!(!recalled(node("monitor", 0), true));
    // A rate within rounding of the same timing is the same mode.
    let mut close = node("monitor", 0);
    close.mode.refresh_hz = 59.94;
    assert!(!recalled(close, true));
    // The refresh Windows picked, or another size.
    let mut slower = node("monitor", 0);
    slower.mode.refresh_hz = 30.;
    assert!(recalled(slower, true));
    let mut smaller = node("monitor", 0);
    smaller.mode.width = 1280;
    assert!(recalled(smaller, true));
    let mut shorter = node("monitor", 0);
    shorter.mode.height = 720;
    assert!(recalled(shorter, true));
    // The primary moved: that counts only for a layout that keeps it.
    let moved = [node("monitor", 1280), node("vdd", 0)];
    assert!(recalled_over(&before, &moved, &connected, &[], true));
    assert!(!recalled_over(&before, &moved, &connected, &[], false));
    // No display reads as primary when the read failed (DXGI during a hotplug): that
    // says nothing of a recall, and a live stream isn't flashed for it.
    assert!(!recalled(node("monitor", 1280), true));
    // The mode is read apart from that, and still counts.
    let mut unread = node("monitor", 1280);
    unread.mode.refresh_hz = 30.;
    assert!(recalled(unread, true));
    // Off, or listed as off.
    assert!(recalled_over(
        &before,
        std::slice::from_ref(&beside),
        &connected,
        &[],
        false
    ));
    let mut off = node("monitor", 0);
    off.active = false;
    assert!(recalled(off, false));
    // Off and no longer connected is the wall switch, not Windows.
    assert!(!recalled_over(
        &before,
        std::slice::from_ref(&beside),
        &names(&["vdd"]),
        &[],
        true
    ));
    // Off before the stream stays the user's.
    let mut was_off = node("tv", 1920);
    was_off.active = false;
    assert!(!recalled_over(
        &[node("monitor", 0), was_off],
        &[node("monitor", 0)],
        &names(&["monitor", "tv"]),
        &[],
        true
    ));
    // A stream's own display is the stream's to lay out and to give a mode.
    let mut stream = node("a", 1920);
    stream.mode.refresh_hz = 1000.;
    assert!(!recalled_over(
        &[node("monitor", 0), stream],
        &[node("monitor", 0), node("a", 1920)],
        &names(&["monitor", "a"]),
        &names(&["a"]),
        true
    ));
}
#[test]
fn a_virtual_displays_entry_keeps_the_users_displays_and_puts_it_to_their_right() {
    let mut monitor = node("monitor", 0);
    monitor.mode.refresh_hz = 120.;
    let mut other = node("other", 1920);
    other.desired_position.y = 200;
    other.mode.width = 2560;
    let mut off = node("left off", 9000);
    off.active = false;
    off.mode.width = 7000;
    // The stream's own display, not on in the user's layout: its old node goes.
    let mut old = node("vdd", 5000);
    old.active = false;
    let mut vdd = node("vdd", 0);
    vdd.mode.width = 1280;
    let nodes = beside_the_users(&[monitor, other, off, old], &vdd);
    assert_eq!(nodes.len(), 4);
    let (users, entry) = (&nodes[..3], &nodes[3]);
    // The user's displays come through as they are, the one off stays off.
    assert_eq!(users[0].device_id, "monitor");
    assert_eq!(users[0].mode.refresh_hz, 120.);
    assert!(users[0].primary);
    assert_eq!(users[1].device_id, "other");
    assert_eq!(users[1].desired_position, Position { x: 1920, y: 200 });
    assert!(!users[2].active);
    assert_eq!(entry.device_id, "vdd");
    assert_eq!(entry.mode.width, 1280);
    assert!(entry.active && !entry.primary);
    // Against the right edge of the rightmost display on, level with it.
    assert_eq!(entry.desired_position, Position { x: 4480, y: 200 });
    // Headless: this display is all there is.
    let alone = beside_the_users(&[], &vdd);
    assert!(alone[0].active && alone[0].primary);
    assert_eq!(alone[0].desired_position, Position { x: 0, y: 0 });
}
#[test]
fn extended_hotplug_keeps_the_original_primary_when_windows_remembers_the_virtual_one() {
    let before = [node("physical", 0), node("other", 1920)];
    // Windows recalls the virtual screen at the origin and moves the
    // physical screens left. All displays remain active, so off-screen
    // recovery does not detect this change.
    let current = [
        node("physical", -1920),
        node("other", 1920),
        node("virtual", 0),
    ];
    for arrangement in [Arrangement::Extended, Arrangement::Isolated] {
        let result = arrangement
            .compose_after_hotplug(&current, "virtual", &[], &before)
            .unwrap();
        assert!(result[0].active && result[0].primary);
        assert_eq!(result[0].desired_position, Position { x: 0, y: 0 });
        assert!(result[1].active && !result[1].primary);
        assert_eq!(result[1].desired_position.x, 3840);
        assert!(result[2].active && !result[2].primary);
        assert_eq!(
            result[2].desired_position,
            if arrangement == Arrangement::Isolated {
                Position { x: 64000, y: 64000 }
            } else {
                Position { x: 1920, y: 0 }
            }
        );
        assert_eq!(
            serde_json::to_value(&result[0].mode).unwrap(),
            serde_json::to_value(&current[0].mode).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&result[1].mode).unwrap(),
            serde_json::to_value(&current[1].mode).unwrap()
        );
    }
}
#[test]
fn deliberate_virtual_primary_layouts_still_promote_the_stream_display() {
    let before = [node("physical", 0)];
    let current = [node("physical", 0), node("virtual", 1920)];
    for arrangement in [
        Arrangement::Primary,
        Arrangement::Exclusive,
        Arrangement::PrimaryIsolated,
    ] {
        let result = arrangement
            .compose_after_hotplug(&current, "virtual", &[], &before)
            .unwrap();
        assert_eq!(
            serde_json::to_value(&result).unwrap(),
            serde_json::to_value(arrangement.compose(&current, "virtual", &[]).unwrap()).unwrap()
        );
        assert!(result[1].active && result[1].primary);
        assert_eq!(result[1].desired_position, Position { x: 0, y: 0 });
    }
}
#[test]
fn extended_hotplug_does_not_choose_a_missing_or_inactive_original_primary() {
    let before = [node("physical", 0)];
    let current = [node("virtual", 0)];
    assert_eq!(
        serde_json::to_value(
            Arrangement::Extended
                .compose_after_hotplug(&current, "virtual", &[], &before)
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(&current).unwrap()
    );
    assert_eq!(
        serde_json::to_value(
            Arrangement::Extended
                .compose_after_hotplug(&current, "virtual", &[], &[])
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(&current).unwrap()
    );
    let mut physical = node("physical", -1920);
    physical.active = false;
    let current = [physical, node("virtual", 0)];
    assert_eq!(
        serde_json::to_value(
            Arrangement::Extended
                .compose_after_hotplug(&current, "virtual", &[], &before)
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(&current).unwrap()
    );
}
#[test]
fn shared_and_recreated_displays_keep_the_first_owners_original_primary() {
    let before = [node("physical", 0)];
    for arrangement in [Arrangement::Extended, Arrangement::Isolated] {
        // A second display arrives as primary while the first still owns
        // the layout. Both streamed displays must remain active.
        let targets = ["first".to_owned(), "second".to_owned()];
        let current = [
            node("physical", -3840),
            node("first", -1920),
            node("second", 0),
        ];
        let result = arrangement
            .compose_all_after_hotplug(&current, &targets, &[], &before)
            .unwrap();
        assert!(result[0].active && result[0].primary);
        assert_eq!(result[0].desired_position, Position { x: 0, y: 0 });
        assert!(result[1].active && !result[1].primary);
        assert!(result[2].active && !result[2].primary);
        let first = if arrangement == Arrangement::Isolated {
            Position { x: 64000, y: 64000 }
        } else {
            Position { x: 1920, y: 0 }
        };
        assert_eq!(result[1].desired_position, first);
        assert_eq!(
            result[2].desired_position,
            Position {
                x: first.x + 1920,
                y: first.y
            }
        );
        // Recovery can replace the first display's identity. The saved
        // physical primary, not the old target identity, is the anchor.
        let current = [node("physical", -1920), node("recreated", 0)];
        let result = arrangement
            .compose_all_after_hotplug(&current, &["recreated".into()], &[], &before)
            .unwrap();
        assert!(result[0].active && result[0].primary);
        assert_eq!(result[0].desired_position, Position { x: 0, y: 0 });
        assert!(result[1].active && !result[1].primary);
        assert_eq!(result[1].desired_position, first);
    }
}
#[test]
fn shared_hotplug_keeps_deliberate_primary_modes_and_missing_baseline_fallbacks() {
    let before = [node("physical", 0)];
    let targets = ["first".to_owned(), "second".to_owned()];
    let current = [
        node("physical", -3840),
        node("first", -1920),
        node("second", 0),
    ];
    for arrangement in [
        Arrangement::Primary,
        Arrangement::Exclusive,
        Arrangement::PrimaryIsolated,
    ] {
        let result = arrangement
            .compose_all_after_hotplug(&current, &targets, &[], &before)
            .unwrap();
        assert!(result[1].active && result[1].primary);
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            serde_json::to_value(arrangement.compose_all(&current, &targets, &[]).unwrap())
                .unwrap()
        );
    }
    let mut off = current.clone();
    off[0].active = false;
    for arrangement in [Arrangement::Extended, Arrangement::Isolated] {
        for (current, before) in [
            (&current[..], &[][..]),
            (&current[1..], &before[..]),
            (&off[..], &before[..]),
        ] {
            assert_eq!(
                serde_json::to_value(
                    arrangement
                        .compose_all_after_hotplug(current, &targets, &[], before)
                        .unwrap()
                )
                .unwrap(),
                serde_json::to_value(arrangement.compose_all(current, &targets, &[]).unwrap())
                    .unwrap()
            );
        }
    }
}
#[test]
fn several_streams_share_one_layout_without_overlapping() {
    let mut second = node("second", 1920);
    second.mode.width = 1968;
    let nodes = vec![node("physical", 0), node("first", 1920), second];
    let targets = ["first".to_owned(), "second".to_owned()];
    let at = |nodes: &[Node], id: &str| {
        let n = nodes.iter().find(|n| n.device_id == id).unwrap();
        (
            n.active,
            n.primary,
            n.desired_position.x,
            n.desired_position.y,
        )
    };
    // Exclusive: only the streamed displays stay on; the first is primary.
    let exclusive = Arrangement::Exclusive
        .compose_all(&nodes, &targets, &[])
        .unwrap();
    assert!(!at(&exclusive, "physical").0);
    assert_eq!(at(&exclusive, "first"), (true, true, 0, 0));
    assert_eq!(at(&exclusive, "second"), (true, false, 1920, 0));
    // Extended: the second display goes right of everything active.
    let extended = Arrangement::Extended
        .compose_all(&nodes, &targets, &[])
        .unwrap();
    assert_eq!(at(&extended, "physical"), (true, true, 0, 0));
    assert_eq!(at(&extended, "first"), (true, false, 1920, 0));
    assert_eq!(at(&extended, "second"), (true, false, 3840, 0));
    // Isolated: streamed displays sit next to each other, away from the rest.
    let isolated = Arrangement::Isolated
        .compose_all(&nodes, &targets, &[])
        .unwrap();
    assert_eq!(at(&isolated, "first"), (true, false, 64000, 64000));
    assert_eq!(at(&isolated, "second"), (true, false, 65920, 64000));
    assert_eq!(at(&isolated, "physical"), (true, true, 0, 0));
    // One target is the single-stream layout.
    let single = Arrangement::Exclusive
        .compose_all(&nodes, &targets[..1], &[])
        .unwrap();
    assert!(!at(&single, "second").0);
    assert!(
        Arrangement::Exclusive
            .compose_all(&nodes, &[], &[])
            .is_err()
    );
}
#[test]
fn exclusive_retains_remote_monitors_and_isolation_keeps_physical_geometry() {
    let nodes = vec![
        node("physical", 0),
        node("game", 1920),
        node("remote", 3840),
    ];
    let exclusive = Arrangement::Exclusive
        .compose(&nodes, "game", &["remote".into()])
        .unwrap();
    assert!(!exclusive[0].active);
    assert!(exclusive[1].primary);
    assert!(exclusive[2].active);
    assert_eq!(exclusive[1].desired_position, Position { x: 0, y: 0 });
    let isolated = Arrangement::PrimaryIsolated
        .compose(&nodes, "game", &[])
        .unwrap();
    assert_eq!(
        isolated[0].desired_position,
        Position { x: 64000, y: 64000 }
    );
    assert_eq!(
        isolated[2].desired_position.x - isolated[0].desired_position.x,
        3840
    );
}
#[test]
fn remote_monitors_follow_which_displays_stay_on_but_keep_their_place() {
    let layout = |value| Arrangement::parse(value).unwrap().for_remote_monitor();
    assert_eq!(layout("exclusive"), Some(Arrangement::Exclusive));
    assert_eq!(layout("extended_primary"), Some(Arrangement::Primary));
    assert_eq!(
        layout("extended_primary_isolated"),
        Some(Arrangement::Primary)
    );
    assert_eq!(layout("extended"), None);
    assert_eq!(layout("extended_isolated"), None);
    // Exclusive on the remote monitor: the physical display goes off and
    // the remote monitor becomes primary, with another device's remote
    // monitor kept on beside it.
    let nodes = vec![
        node("physical", 0),
        node("remote", 1920),
        node("other", 3840),
    ];
    let exclusive = Arrangement::Exclusive
        .compose(&nodes, "remote", &["other".into()])
        .unwrap();
    assert!(!exclusive[0].active && !exclusive[0].primary);
    assert!(exclusive[1].active && exclusive[1].primary);
    assert_eq!(exclusive[1].desired_position, Position { x: 0, y: 0 });
    assert!(exclusive[2].active);
}
