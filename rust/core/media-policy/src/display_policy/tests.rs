#[test]
fn a_client_display_scale_is_one_windows_offers() {
    use super::{DISPLAY_SCALE_PARAMETER, client_display_scale};
    let options = |value: &str| {
        std::collections::BTreeMap::from([(DISPLAY_SCALE_PARAMETER.to_owned(), value.to_owned())])
    };
    assert_eq!(client_display_scale(&Default::default()).unwrap(), None);
    assert_eq!(client_display_scale(&options("150")).unwrap(), Some(150));
    assert_eq!(client_display_scale(&options(" 500 ")).unwrap(), Some(500));
    for refused in ["0", "275", "-1", "600", "big", ""] {
        assert!(
            client_display_scale(&options(refused)).is_err(),
            "{refused}"
        );
    }
    // The render scale is a separate option and never reads as a display scale.
    let render = std::collections::BTreeMap::from([("scaleFactor".to_owned(), "200".to_owned())]);
    assert_eq!(client_display_scale(&render).unwrap(), None);
}

#[test]
fn a_kept_resolution_is_width_by_height() {
    use super::parse_resolution;
    assert_eq!(parse_resolution("2560x1440"), Some((2560, 1440)));
    assert_eq!(parse_resolution(" 1920X1080 "), Some((1920, 1080)));
    assert_eq!(parse_resolution("2560x1440x120"), None);
    assert_eq!(parse_resolution("0x1440"), None);
    assert_eq!(parse_resolution(""), None);
}

#[test]
fn the_stream_resolution_cap_is_native_or_a_size_the_display_could_have() {
    use super::parse_max_stream_resolution;
    // Today's behaviour, and what an untouched or reset value reads as.
    for native in ["native", "NATIVE", " Native ", "", "\"native\""] {
        assert_eq!(
            parse_max_stream_resolution(native).unwrap(),
            None,
            "{native}"
        );
    }
    assert_eq!(
        parse_max_stream_resolution("2560x1440").unwrap(),
        Some((2560, 1440))
    );
    assert_eq!(
        parse_max_stream_resolution(" 3840X2160 ").unwrap(),
        Some((3840, 2160))
    );
    for refused in [
        "1440p",
        "nativex",
        "2560",
        "2560x",
        "x1440",
        "319x1440",
        "2560x199",
        "7681x2160",
        "2560x4321",
        "0x0",
    ] {
        assert!(parse_max_stream_resolution(refused).is_err(), "{refused}");
    }
}

#[test]
fn a_capped_stream_keeps_its_shape_inside_the_cap_and_never_grows() {
    use super::capped_encode_size;
    // No cap, or one the request fits in, codes exactly the request.
    assert_eq!(capped_encode_size((3840, 2160), None), (3840, 2160));
    assert_eq!(
        capped_encode_size((1920, 1080), Some((2560, 1440))),
        (1920, 1080)
    );
    // The same shape fills the cap exactly.
    assert_eq!(
        capped_encode_size((3840, 2160), Some((2560, 1440))),
        (2560, 1440)
    );
    // Another shape is fitted, with black bars added by the converter.
    assert_eq!(
        capped_encode_size((3440, 1440), Some((2560, 1440))),
        (2560, 1072)
    );
    assert_eq!(
        capped_encode_size((1920, 1200), Some((2560, 1080))),
        (1728, 1080)
    );
    // An extreme shape keeps a picture the encoder can code.
    assert_eq!(capped_encode_size((16384, 64), Some((320, 200))), (320, 64));
}

#[test]
fn rotations_are_displayconfig_codes_one_to_four() {
    use super::{rotation_code, rotation_degrees};
    for (code, degrees) in [(1, 0), (2, 90), (3, 180), (4, 270)] {
        assert_eq!(rotation_degrees(code), Some(degrees));
        assert_eq!(rotation_code(degrees), Some(code));
    }
    assert_eq!(rotation_degrees(0), None);
    assert_eq!(rotation_degrees(5), None);
    assert_eq!(rotation_code(45), None);
    assert_eq!(rotation_code(360), None);
}

#[test]
fn a_physical_display_turns_to_the_chosen_angle_or_the_streams_shape() {
    use super::physical_rotation as turn;
    use butterpollo_config::config::DisplayAdjust::{Auto, Disabled, Manual};
    let landscape = (2560, 1440);
    let portrait = (1440, 2560);
    let phone = (1080, 2400);
    assert_eq!(turn(Disabled, 90, 0, landscape, phone), None);
    // A chosen angle, unless the display has it already.
    assert_eq!(turn(Manual, 90, 0, landscape, (1920, 1080)), Some(90));
    assert_eq!(turn(Manual, 0, 270, portrait, phone), Some(0));
    assert_eq!(turn(Manual, 180, 180, landscape, phone), None);
    // A portrait phone turns a landscape display, a landscape stream turns
    // it back; one already in the stream's shape stays as it is.
    assert_eq!(turn(Auto, 0, 0, landscape, phone), Some(90));
    assert_eq!(turn(Auto, 0, 180, landscape, phone), Some(90));
    assert_eq!(turn(Auto, 0, 270, portrait, (2400, 1080)), Some(0));
    assert_eq!(turn(Auto, 0, 270, portrait, phone), None);
    assert_eq!(turn(Auto, 0, 0, landscape, (1920, 1080)), None);
    // A monitor that is portrait upright turns to landscape for a landscape stream.
    assert_eq!(turn(Auto, 0, 0, portrait, (1920, 1080)), Some(90));
    // A square has no shape to follow.
    assert_eq!(turn(Auto, 0, 0, landscape, (1024, 1024)), None);
    assert_eq!(turn(Auto, 0, 0, (1024, 1024), phone), None);
}

#[test]
fn a_turned_physical_display_gets_its_mode_in_its_shape_before_the_turn() {
    use super::mode_before_turn;
    // A portrait phone's mode on a landscape display, which then turns to portrait.
    assert_eq!(mode_before_turn((1080, 2400), (2560, 1440)), (2400, 1080));
    assert_eq!(mode_before_turn((1920, 1080), (2560, 1440)), (1920, 1080));
    assert_eq!(mode_before_turn((1920, 1080), (1440, 2560)), (1080, 1920));
    assert_eq!(mode_before_turn((1024, 1024), (1440, 2560)), (1024, 1024));
}

#[test]
fn a_virtual_display_is_shaped_rather_than_turned() {
    use super::virtual_shape as shape;
    use butterpollo_config::config::DisplayAdjust::{Auto, Disabled, Manual};
    let phone = (1080, 2400);
    assert_eq!(shape(Disabled, 90, (2560, 1440), phone), (2560, 1440));
    assert_eq!(shape(Manual, 90, (1920, 1080), (1920, 1080)), (1080, 1920));
    assert_eq!(shape(Manual, 270, (1080, 2400), phone), (2400, 1080));
    assert_eq!(shape(Manual, 180, (1920, 1080), (1920, 1080)), (1920, 1080));
    assert_eq!(shape(Manual, 0, (1920, 1080), phone), (1920, 1080));
    // A device's landscape display mode takes a portrait phone's shape.
    assert_eq!(shape(Auto, 0, (2560, 1440), phone), (1440, 2560));
    assert_eq!(shape(Auto, 0, phone, phone), phone);
    assert_eq!(shape(Auto, 0, (1440, 2560), (1920, 1080)), (2560, 1440));
    assert_eq!(shape(Auto, 0, (2560, 1440), (1024, 1024)), (2560, 1440));
}

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
fn virtual_opt_in_preserves_host_policy_and_device_force_overrides_physical_output() {
    use super::VirtualDisplayRequest;
    let host = VirtualDisplayRequest {
        configured: true,
        ..Default::default()
    };
    assert!(host.uses_virtual(false, || true));
    let physical_app = VirtualDisplayRequest {
        output_override: Some("physical-monitor"),
        ..host
    };
    assert!(!physical_app.requested());
    let forced = VirtualDisplayRequest {
        client_forced: true,
        ..physical_app
    };
    assert!(forced.uses_virtual(false, || true));
    let opt_in = VirtualDisplayRequest {
        client_requested: true,
        client_forced: false,
        ..forced
    };
    assert!(opt_in.uses_virtual(false, || true));
    assert!(!opt_in.uses_virtual(true, || panic!(
        "physical fallback must not query the driver"
    )));
}
#[test]
fn a_requested_virtual_display_without_its_driver_streams_the_physical_display() {
    use super::VirtualDisplayRequest;
    let client = VirtualDisplayRequest {
        client_requested: true,
        ..Default::default()
    };
    assert!(client.uses_virtual(false, || true));
    assert!(!client.uses_virtual(false, || false));
    assert!(!client.uses_virtual(true, || true));
    for request in [
        VirtualDisplayRequest {
            client_forced: true,
            ..Default::default()
        },
        VirtualDisplayRequest {
            app_requested: true,
            ..Default::default()
        },
        VirtualDisplayRequest {
            configured: true,
            ..Default::default()
        },
    ] {
        assert!(request.uses_virtual(false, || true));
        assert!(!request.uses_virtual(false, || false));
    }
    let named = VirtualDisplayRequest {
        configured_output: "virtual_display",
        ..Default::default()
    };
    assert!(named.uses_virtual(false, || false));
    let none = VirtualDisplayRequest::default();
    assert!(!none.uses_virtual(false, || panic!("no request needs no driver check")));
    // With no active display there is nothing physical to stream, even
    // when the settings and the app name a physical display.
    let headless = VirtualDisplayRequest {
        headless: true,
        output_override: Some("physical-monitor"),
        ..Default::default()
    };
    assert!(headless.uses_virtual(false, || true));
    assert!(!headless.uses_virtual(false, || false));
    assert!(!headless.uses_virtual(true, || true));
}
#[test]
fn a_client_asking_for_the_physical_display_overrides_the_virtual_display_settings() {
    use super::VirtualDisplayRequest;
    for request in [
        VirtualDisplayRequest {
            configured: true,
            ..Default::default()
        },
        VirtualDisplayRequest {
            app_requested: true,
            ..Default::default()
        },
        VirtualDisplayRequest {
            client_forced: true,
            ..Default::default()
        },
        VirtualDisplayRequest {
            client_requested: true,
            ..Default::default()
        },
    ] {
        assert!(request.requested());
        let physical = VirtualDisplayRequest {
            client_physical: true,
            ..request
        };
        assert!(!physical.requested());
        assert!(!physical.uses_virtual(false, || panic!("no virtual display, no driver check")));
    }
    // Nothing physical to show: a headless host, or an output that names
    // the virtual display itself.
    for request in [
        VirtualDisplayRequest {
            client_physical: true,
            headless: true,
            ..Default::default()
        },
        VirtualDisplayRequest {
            client_physical: true,
            output_override: Some("virtual_display"),
            ..Default::default()
        },
        VirtualDisplayRequest {
            client_physical: true,
            configured_output: "sunshine:virtual_display",
            ..Default::default()
        },
    ] {
        assert!(request.uses_virtual(false, || true));
    }
}
#[test]
fn host_display_choices_parse_and_unknown_values_leave_the_host_in_control() {
    use super::{Arrangement, HostDisplay};
    assert_eq!(HostDisplay::PARAMETER, "hostDisplay");
    assert_eq!(HostDisplay::parse("physical"), Some(HostDisplay::Physical));
    assert_eq!(
        HostDisplay::parse(" Physical "),
        Some(HostDisplay::Physical)
    );
    assert_eq!(
        HostDisplay::parse("exclusive"),
        Some(HostDisplay::Virtual(Arrangement::Exclusive))
    );
    assert_eq!(
        HostDisplay::parse("extended"),
        Some(HostDisplay::Virtual(Arrangement::Extended))
    );
    assert_eq!(
        HostDisplay::parse("extended-primary"),
        Some(HostDisplay::Virtual(Arrangement::Primary))
    );
    for unknown in [
        "",
        " ",
        "default",
        "host",
        "mirror",
        "virtual",
        "0",
        "1",
        "ensure_active",
        "ensure_primary",
        "ensure_only_display",
        "ensure-only-display",
    ] {
        assert_eq!(HostDisplay::parse(unknown), None, "{unknown}");
    }
    for arrangement in [
        Arrangement::Extended,
        Arrangement::Primary,
        Arrangement::Exclusive,
        Arrangement::Isolated,
        Arrangement::PrimaryIsolated,
    ] {
        let choice = HostDisplay::Virtual(arrangement);
        assert_eq!(Arrangement::parse(arrangement.name()).unwrap(), arrangement);
        assert_eq!(HostDisplay::parse(choice.name()), Some(choice));
    }
    assert_eq!(HostDisplay::Physical.name(), "physical");
}
#[test]
fn a_kept_display_serves_a_launch_only_when_it_was_set_up_for_the_same_choice() {
    use super::{Arrangement, HostDisplay};
    let exclusive = Some(HostDisplay::Virtual(Arrangement::Exclusive));
    let extended = Some(HostDisplay::Virtual(Arrangement::Extended));
    // A launch without a choice takes the display as it is.
    for prepared in [None, exclusive, extended, Some(HostDisplay::Physical)] {
        assert!(HostDisplay::reusable(prepared, None));
    }
    assert!(HostDisplay::reusable(exclusive, exclusive));
    assert!(!HostDisplay::reusable(exclusive, extended));
    assert!(!HostDisplay::reusable(None, exclusive));
    assert!(!HostDisplay::reusable(
        exclusive,
        Some(HostDisplay::Physical)
    ));
}
#[test]
fn unapplied_display_modes_are_visible_and_recovery_clears_them() {
    let warnings = crate::TestWarnings::default();
    let requested = (Some((3840, 2160)), Some(116_000), Some(true));
    super::report_mode(&warnings, requested, (1920, 1080, 60_000, false), 116_000);
    let entries = warnings.snapshot();
    assert_eq!(
        entries.iter().map(|w| w.code.as_str()).collect::<Vec<_>>(),
        ["display_hdr", "display_refresh", "display_resolution"]
    );
    assert!(entries[1].message.contains("60.000 Hz"));
    super::report_mode(&warnings, requested, (3840, 2160, 116_000, true), 116_000);
    assert!(warnings.snapshot().is_empty());
    super::report_mode(
        &warnings,
        (None, None, None),
        (3840, 2160, 59_940, false),
        60_000,
    );
    assert!(warnings.snapshot().is_empty());
    super::report_mode(
        &warnings,
        (None, None, None),
        (3840, 2160, 60_000, false),
        116_000,
    );
    assert_eq!(warnings.snapshot()[0].code, "display_refresh");
}
use super::*;
#[test]
fn other_shapes_are_centred_with_bars() {
    // The 32:9 desktop on a 16:10 tablet.
    assert_eq!(letterbox((5120, 1440), (2560, 1600)), (0, 440, 2560, 720));
    // A 16:9 source on a tall phone stream.
    assert_eq!(letterbox((1920, 1080), (1968, 2184)), (0, 538, 1968, 1106));
    assert_eq!(letterbox((3840, 2160), (1920, 1080)), (0, 0, 1920, 1080));
    // Within a pixel of the stream's shape fills it.
    assert_eq!(letterbox((1921, 1080), (1920, 1080)), (0, 0, 1920, 1080));
    assert_eq!(letterbox((0, 0), (1920, 1080)), (0, 0, 1920, 1080));
}
#[test]
fn touch_on_a_letterboxed_stream_lands_on_the_picture_not_the_bars() {
    // A 16:9 display on a 4:3 iPad stream: bars above and below.
    let ipad = picture((2560, 1440), (2732, 2048)).unwrap();
    assert_eq!(ipad, (0, 256, 2732, 1536));
    let at = |x: f64, y: f64| stream_to_picture((x, y), (2732, 2048), ipad);
    assert_eq!(at(0., 256. / 2048.), (0., 0.));
    assert_eq!(at(1., 1792. / 2048.), (1., 1.));
    assert_eq!(at(0.5, 0.5), (0.5, 0.5));
    // On a bar: the picture's edge.
    assert_eq!(at(0.25, 0.), (0.25, 0.));
    assert_eq!(at(0.25, 1.), (0.25, 1.));
    // A display of the stream's shape fills it: nothing to map.
    assert_eq!(picture((3840, 2160), (1920, 1080)), None);
    assert_eq!(picture((1920, 1080), (0, 0)), None);
}
#[test]
fn device_display_modes_parse_like_vibepollo() {
    use crate::framegen::Rate;
    assert_eq!(
        parse_display_mode("1920x1080x59.94"),
        Some((1920, 1080, Rate(59940)))
    );
    assert_eq!(
        parse_display_mode(" 2560X1600x120 "),
        Some((2560, 1600, Rate(120000)))
    );
    for bad in [
        "",
        "1920x1080",
        "1920x1080x0",
        "0x1080x60",
        "1920x1080x60x1",
        "axbxc",
    ] {
        assert_eq!(parse_display_mode(bad), None, "{bad}");
    }
}
#[test]
fn unsupported_physical_resolutions_keep_the_current_mode_and_rate() {
    let odyssey = [
        (5120, 1440, 240),
        (5120, 1440, 120),
        (5120, 1440, 60),
        (3840, 1080, 60),
        (2560, 1440, 120),
        (2560, 1440, 60),
    ];
    let current = (5120, 1440, 240_000);
    // The tablet case: no 2560x1600 mode; keep 5120x1440, use 120 Hz.
    assert_eq!(
        physical_mode(&odyssey, (Some((2560, 1600)), Some(120_000)), current),
        (5120, 1440, 120_000)
    );
    // A supported request is applied as asked.
    assert_eq!(
        physical_mode(&odyssey, (Some((2560, 1440)), Some(120_000)), current),
        (2560, 1440, 120_000)
    );
    // A refresh the chosen resolution lacks keeps the current one.
    assert_eq!(
        physical_mode(&odyssey, (None, Some(144_000)), current),
        (5120, 1440, 240_000)
    );
    // A fractional request is kept when the nearest whole-hertz mode exists.
    assert_eq!(
        physical_mode(&odyssey, (None, Some(119_880)), current),
        (5120, 1440, 119_880)
    );
    // A resolution that only lacks the current refresh takes its highest.
    assert_eq!(
        physical_mode(&odyssey, (Some((3840, 1080)), Some(120_000)), current),
        (3840, 1080, 60_000)
    );
    assert_eq!(
        physical_mode(&[], (Some((1, 1)), Some(60_000)), current),
        current
    );
}
use crate::topology::{Kind, Mode};
#[test]
fn previous_virtual_driver_identity_vectors_use_windows_guid_bytes() {
    assert_eq!(
        virtual_display_id("f773d31b-43da-470c-80d5-02e777a6d993"),
        0xed9b7be9fc3f0b82
    );
    assert_eq!(virtual_display_id("example-client"), 0xbce6a166cf4c68bc);
    assert_eq!(
        virtual_identity("example-client"),
        *uuid::Uuid::parse_str("f12b4b23-a6df-5a22-8eed-f5aaef994ef8")
            .unwrap()
            .as_bytes()
    );
    let combined = app_client_identity(
        "f773d31b-43da-470c-80d5-02e777a6d993",
        "d9428888-122b-11e1-b85c-61cd3cbb3210",
    );
    assert_eq!(
        app_client_identity(&combined, "d9428888-122b-11e1-b85c-61cd3cbb3210"),
        "f773d31b-43da-470c-80d5-02e777a6d993"
    );
}
#[test]
fn libdisplaydevice_uuid_vectors_survive_the_unstable_driver_instance_counter() {
    let path = r"\\?\DISPLAY#ACI27EC#5&4FD2DE4&5&UID4352";
    let edid: Vec<u8> = (0..128).collect();
    let expected = "{ccbebcd3-0583-5b71-92e4-6e28da2e07ff}";
    assert_eq!(
        legacy_device_id(path, Some(r"DISPLAY\ACI27EC\5&4FD2DE4&5&UID4352"), &edid),
        expected
    );
    assert_eq!(
        legacy_device_id(path, Some(r"DISPLAY\ACI27EC\5&4FD2DE4&99&UID4352"), &edid),
        expected
    );
    assert_eq!(
        legacy_device_id(path, None, &[]),
        "{3355dc9e-a978-5cee-9a57-36824dcba1e6}"
    );
    assert_ne!(
        legacy_device_id(path, Some(r"DISPLAY\ACI27EC\5&4FD2DE4&5&UID4353"), &edid),
        expected
    );
}
#[test]
fn retained_render_scale_precedence_and_odd_resolution_vectors() {
    assert_eq!(render_dimensions(1920, 1080, 150, 100), (2880, 1620));
    assert_eq!(render_dimensions(1920, 1080, 150, 200), (3840, 2160));
    assert_eq!(render_dimensions(1365, 767, 75, 100), (1022, 574));
    assert_eq!(render_dimensions(1920, 1080, -1, 100), (1920, 1080));
    assert_eq!(render_dimensions(1920, 1080, i64::MAX, 100), (1920, 1080));
}
fn node(id: &str, x: i32) -> Node {
    Node {
        id: id.into(),
        label: id.into(),
        device_id: id.into(),
        kind: Kind::Physical,
        active: true,
        primary: x == 0,
        desired_position: Position { x, y: 0 },
        mode: Mode {
            width: 1920,
            height: 1080,
            refresh_hz: 60.,
        },
    }
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
