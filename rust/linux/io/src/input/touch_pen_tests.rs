//! Tests of native touch and pen: the axis conversions, the pointer emulation, and the events
//! the virtual touchscreen and pen stage (where `/dev/uinput` can be written).
use super::*;

fn touch(event: u8, id: u32, x: f32, y: f32) -> Event {
    Event::Touch {
        event,
        id,
        x,
        y,
        pressure: 0.,
        major: 0.,
        minor: 0.,
        rotation: u16::MAX,
    }
}
fn pen(event: u8, tool: u8, buttons: u8, x: f32, y: f32, pressure: f32) -> Event {
    Event::Pen {
        event,
        tool,
        buttons,
        x,
        y,
        pressure,
        rotation: u16::MAX,
        tilt: u8::MAX,
        roll: u16::MAX,
    }
}

#[test]
fn fractions_pressures_and_tilts_convert() {
    assert_eq!(axis(0.), 0);
    assert_eq!(axis(1.), ABS_MAX);
    assert_eq!(axis(0.5), 32768);
    assert_eq!(axis(-3.), 0);
    assert_eq!(axis(7.), ABS_MAX);
    assert_eq!(axis(f32::NAN), 0);
    // An unknown pressure is half; a known one keeps the tip down.
    assert_eq!(pressure(0.), PRESSURE_MAX / 2);
    assert_eq!(pressure(f32::NAN), PRESSURE_MAX / 2);
    assert_eq!(pressure(1.), PRESSURE_MAX);
    assert_eq!(pressure(1e-9), 1);
    assert_eq!(pressure(9.), PRESSURE_MAX);
    assert_eq!(tilt(u8::MAX, 0), None);
    assert_eq!(tilt(30, u16::MAX), None);
    assert_eq!(tilt(0, 90), Some((0, 0)));
    // Leaning towards the top of the screen tilts along y only.
    assert_eq!(tilt(30, 0), Some((0, 30)));
    let (x, y) = tilt(45, 90).unwrap();
    assert_eq!((x, y), (-45, 0));
    assert_eq!(tilt(200, 180).unwrap().1, -90);
}

#[test]
fn the_first_finger_drives_the_pointer_where_touch_cannot_be_injected() {
    let mut emulated = Emulated::default();
    let at = |x: i16, y: i16| Event::Absolute {
        x,
        y,
        width: 32767,
        height: 32767,
    };
    let button = |down| Event::MouseButton { button: 1, down };
    assert_eq!(
        emulated.events(&touch(1, 7, 0.5, 1.)),
        vec![at(16384, 32767), button(true)]
    );
    // A second finger, and a move of it, change nothing.
    assert!(emulated.events(&touch(1, 8, 0.1, 0.1)).is_empty());
    assert!(emulated.events(&touch(3, 8, 0.2, 0.2)).is_empty());
    assert_eq!(emulated.events(&touch(3, 7, 0., 0.)), vec![at(0, 0)]);
    assert_eq!(
        emulated.events(&touch(2, 7, 1., 0.)),
        vec![at(32767, 0), button(false)]
    );
    // The other finger is not taken up when the first lifts, and a stray up does nothing.
    assert!(emulated.events(&touch(3, 8, 0.2, 0.2)).is_empty());
    assert!(emulated.events(&touch(2, 9, 0.2, 0.2)).is_empty());
    // A cancel-all releases a held button once.
    emulated.events(&touch(1, 3, 0.5, 0.5));
    assert_eq!(emulated.events(&touch(7, 0, 0., 0.)), vec![button(false)]);
    assert!(emulated.events(&touch(7, 0, 0., 0.)).is_empty());
    // The pen hovers as motion and holds the button while down.
    assert_eq!(emulated.events(&pen(0, 1, 0, 0., 0., 0.)), vec![at(0, 0)]);
    assert_eq!(
        emulated.events(&pen(1, 1, 0, 0., 0., 0.5)),
        vec![at(0, 0), button(true)]
    );
    assert!(emulated.events(&pen(1, 1, 0, 0., 0., 0.5)).is_empty());
    assert_eq!(
        emulated.events(&pen(6, 1, 0, 0., 0., 0.)),
        vec![button(false)]
    );
    assert!(emulated.events(&pen(2, 1, 0, 0., 0., 0.)).is_empty());
}

#[test]
fn a_named_output_is_used_as_it_is() {
    assert_eq!(
        output_name(&DesktopMonitor::Named("Virtual-1".into())).as_deref(),
        Some("Virtual-1")
    );
    assert_eq!(output_name(&DesktopMonitor::Named(String::new())), None);
    assert_eq!(output_name(&DesktopMonitor::Named("node 42".into())), None);
}

/// The devices, or None (and a note) where `/dev/uinput` cannot be written, as on CI.
fn devices() -> Option<TouchPen> {
    // An output no KWin has, so the background mapping changes nothing on this desktop.
    match TouchPen::create(&DesktopMonitor::Named("Rubylight-test-none".into())) {
        Ok(devices) => Some(devices),
        Err(error) => {
            eprintln!("skipped: {error:#}");
            None
        }
    }
}

const SYN: (u16, u16, i32) = (EV_SYN, 0, 0);

#[test]
fn fingers_take_slots_and_tracking_ids() {
    let Some(mut devices) = devices() else {
        return;
    };
    devices.apply(&touch(1, 70, 0.5, 0.25));
    devices.apply(&touch(1, 71, 1., 1.));
    devices.apply(&touch(3, 71, 0., 1.));
    devices.apply(&touch(3, 70, 0.25, 0.25));
    let half = PRESSURE_MAX / 2;
    assert_eq!(
        devices.screen.staged(),
        vec![
            (EV_ABS, ABS_MT_SLOT, 0),
            (EV_ABS, ABS_MT_TRACKING_ID, 0),
            (EV_ABS, ABS_MT_POSITION_X, 32768),
            (EV_ABS, ABS_MT_POSITION_Y, 16384),
            (EV_ABS, ABS_MT_PRESSURE, half),
            (EV_ABS, ABS_X, 32768),
            (EV_ABS, ABS_Y, 16384),
            (EV_KEY, BTN_TOUCH, 1),
            SYN,
            (EV_ABS, ABS_MT_SLOT, 1),
            (EV_ABS, ABS_MT_TRACKING_ID, 1),
            (EV_ABS, ABS_MT_POSITION_X, ABS_MAX),
            (EV_ABS, ABS_MT_POSITION_Y, ABS_MAX),
            (EV_ABS, ABS_MT_PRESSURE, half),
            SYN,
            // Moves share one report; only the oldest finger moves the single-touch axes.
            (EV_ABS, ABS_MT_SLOT, 1),
            (EV_ABS, ABS_MT_POSITION_X, 0),
            (EV_ABS, ABS_MT_POSITION_Y, ABS_MAX),
            (EV_ABS, ABS_MT_PRESSURE, half),
            (EV_ABS, ABS_MT_SLOT, 0),
            (EV_ABS, ABS_MT_POSITION_X, 16384),
            (EV_ABS, ABS_MT_POSITION_Y, 16384),
            (EV_ABS, ABS_MT_PRESSURE, half),
            (EV_ABS, ABS_X, 16384),
            (EV_ABS, ABS_Y, 16384),
        ]
    );
    devices.screen.flush().unwrap();
    // A finger that was never down does nothing; the first up keeps the touch, the last ends it.
    devices.apply(&touch(3, 99, 0.5, 0.5));
    devices.apply(&touch(2, 70, 0.5, 0.5));
    devices.apply(&touch(4, 71, 0.5, 0.5));
    devices.apply(&touch(2, 71, 0.5, 0.5));
    assert_eq!(
        devices.screen.staged(),
        vec![
            (EV_ABS, ABS_MT_SLOT, 0),
            (EV_ABS, ABS_MT_TRACKING_ID, -1),
            SYN,
            (EV_ABS, ABS_MT_SLOT, 1),
            (EV_ABS, ABS_MT_TRACKING_ID, -1),
            (EV_KEY, BTN_TOUCH, 0),
            SYN,
        ]
    );
    devices.screen.flush().unwrap();
    // Contact size and orientation, then a cancel-all.
    devices.apply(&Event::Touch {
        event: 1,
        id: 5,
        x: 0.,
        y: 0.,
        pressure: 1.,
        major: 0.1,
        minor: 0.2,
        rotation: 400,
    });
    devices.apply(&touch(7, 0, 0., 0.));
    let staged = devices.screen.staged();
    assert!(staged.contains(&(EV_ABS, ABS_MT_TRACKING_ID, 2)));
    assert!(staged.contains(&(EV_ABS, ABS_MT_PRESSURE, PRESSURE_MAX)));
    assert!(staged.contains(&(EV_ABS, ABS_MT_TOUCH_MAJOR, axis(0.2))));
    assert!(staged.contains(&(EV_ABS, ABS_MT_TOUCH_MINOR, axis(0.1))));
    assert!(staged.contains(&(EV_ABS, ABS_MT_ORIENTATION, 40)));
    assert!(staged.ends_with(&[
        (EV_ABS, ABS_MT_SLOT, 0),
        (EV_ABS, ABS_MT_TRACKING_ID, -1),
        (EV_KEY, BTN_TOUCH, 0),
        SYN,
    ]));
    assert!(devices.slots.iter().all(Option::is_none));
}

#[test]
fn seventeen_fingers_leave_the_last_out() {
    let Some(mut devices) = devices() else {
        return;
    };
    for id in 0..17 {
        devices.apply(&touch(1, id, 0.5, 0.5));
    }
    assert!(devices.slots.iter().all(Option::is_some));
    assert!(!devices.slots.contains(&Some(16)));
    devices.apply(&touch(2, 3, 0.5, 0.5));
    devices.apply(&touch(1, 16, 0.5, 0.5));
    assert_eq!(devices.slots[3], Some(16));
}

#[test]
fn the_pen_hovers_touches_presses_and_leaves() {
    let Some(mut devices) = devices() else {
        return;
    };
    devices.apply(&pen(0, 1, 0, 0.5, 0.5, 0.));
    devices.apply(&pen(1, 1, 1, 0.5, 0.5, 0.25));
    devices.apply(&pen(3, 1, 1, 1., 1., 0.5));
    devices.apply(&pen(5, 1, 2, 0., 0., 0.));
    devices.apply(&pen(2, 1, 2, 1., 1., 0.));
    assert_eq!(
        devices.pen.staged(),
        vec![
            (EV_ABS, ABS_X, 32768),
            (EV_ABS, ABS_Y, 32768),
            (EV_KEY, BTN_TOOL_PEN, 1),
            (EV_ABS, ABS_PRESSURE, 0),
            SYN,
            (EV_ABS, ABS_X, 32768),
            (EV_ABS, ABS_Y, 32768),
            (EV_ABS, ABS_PRESSURE, 1024),
            (EV_KEY, BTN_TOUCH, 1),
            (EV_KEY, BTN_STYLUS, 1),
            SYN,
            (EV_ABS, ABS_X, ABS_MAX),
            (EV_ABS, ABS_Y, ABS_MAX),
            (EV_ABS, ABS_PRESSURE, 2048),
            SYN,
            // Buttons only: the barrel button changes to the second one.
            (EV_KEY, BTN_STYLUS, 0),
            (EV_KEY, BTN_STYLUS2, 1),
            SYN,
            // The lift, then it leaves range with its button let go.
            (EV_ABS, ABS_X, ABS_MAX),
            (EV_ABS, ABS_Y, ABS_MAX),
            (EV_ABS, ABS_PRESSURE, 0),
            (EV_KEY, BTN_TOUCH, 0),
            SYN,
            (EV_KEY, BTN_STYLUS2, 0),
            (EV_KEY, BTN_TOOL_PEN, 0),
            SYN,
        ]
    );
    devices.pen.flush().unwrap();
    assert_eq!(devices.pen_state, PenState::default());
    // Buttons alone with no pen in range, and a leave of a pen not there, do nothing.
    devices.apply(&pen(5, 1, 1, 0., 0., 0.));
    devices.apply(&pen(6, 1, 0, 0., 0., 0.));
    assert!(devices.pen.staged().is_empty());
    // The eraser takes over from the pen with a tilt; a cancel-all lifts it.
    devices.apply(&pen(1, 1, 0, 0., 0., 0.));
    devices.apply(&Event::Pen {
        event: 3,
        tool: 2,
        buttons: 0,
        x: 0.,
        y: 0.,
        pressure: 1.,
        rotation: 0,
        tilt: 30,
        roll: u16::MAX,
    });
    let staged = devices.pen.staged();
    let pen_left = staged
        .iter()
        .position(|e| *e == (EV_KEY, BTN_TOOL_PEN, 0))
        .unwrap();
    let eraser = staged
        .iter()
        .position(|e| *e == (EV_KEY, BTN_TOOL_RUBBER, 1))
        .unwrap();
    assert!(pen_left < eraser);
    assert!(staged.contains(&(EV_ABS, ABS_TILT_Y, 30)));
    // The eraser came in hovering: a move does not put it down.
    assert!(!staged[eraser..].contains(&(EV_KEY, BTN_TOUCH, 1)));
    devices.pen.flush().unwrap();
    devices.apply(&touch(7, 0, 0., 0.));
    assert_eq!(
        devices.pen.staged(),
        vec![(EV_KEY, BTN_TOOL_RUBBER, 0), SYN]
    );
}

/// KWin takes the devices as a touchscreen and a tablet and ties them to the output named by
/// `RUBYLIGHT_LIVE_TOUCH_OUTPUT` (`DP-1`). Needs a KDE Plasma session and `/dev/uinput`.
#[test]
#[ignore = "needs a KDE Plasma session; RUBYLIGHT_LIVE_TOUCH_OUTPUT names an output"]
fn live_kwin_ties_touch_and_pen_to_the_output() {
    let Ok(output) = std::env::var("RUBYLIGHT_LIVE_TOUCH_OUTPUT") else {
        eprintln!("skipped: RUBYLIGHT_LIVE_TOUCH_OUTPUT names the output");
        return;
    };
    let devices = TouchPen::create(&DesktopMonitor::Named(output.clone())).unwrap();
    let bus = crate::power::logind::session_bus().unwrap();
    for (sysname, kind) in [
        (&devices.screen.sysname, "touch"),
        (&devices.pen.sysname, "tabletTool"),
    ] {
        let node = event_node(sysname).unwrap();
        let path = format!("/org/kde/KWin/InputDevice/{node}");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(6);
        loop {
            let proxy = zbus::blocking::Proxy::new(
                &bus,
                "org.kde.KWin",
                path.as_str(),
                "org.kde.KWin.InputDevice",
            )
            .unwrap();
            let mapped = proxy.get_property::<String>("outputName").ok();
            let is_kind = proxy.get_property::<bool>(kind).ok();
            eprintln!("{node}: {kind}={is_kind:?} outputName={mapped:?}");
            if mapped.as_deref() == Some(output.as_str()) && is_kind == Some(true) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "{node} was not tied to {output}"
            );
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
    }
}
