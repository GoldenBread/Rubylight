//! Tests for WebRTC gamepad slots, mapping, sequence tracking, rumble, and wire parsing.
mod mouse_mode;

use super::*;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

fn connected(id: u32) -> Connected {
    Connected {
        controller_id: id,
        id: "Xbox Wireless Controller (STANDARD GAMEPAD Vendor: 045e)".into(),
        mapping: "standard".into(),
        buttons: 17,
        axes: 4,
        dual_rumble: true,
        trigger_rumble: false,
    }
}

fn state(id: u32, seq: u64, buttons: u32) -> State {
    State {
        controller_id: id,
        seq,
        buttons: u64::from(buttons),
        left_trigger: 0.,
        right_trigger: 0.,
        left_stick_x: 0.,
        left_stick_y: 0.,
        right_stick_x: 0.,
        right_stick_y: 0.,
    }
}

fn inputs(outcome: &Outcome) -> Vec<&Input> {
    outcome
        .actions
        .iter()
        .filter_map(|a| match a {
            Action::Input(input) => Some(input),
            Action::Control(_) => None,
        })
        .collect()
}

fn controls(outcome: &Outcome) -> Vec<&Control> {
    outcome
        .actions
        .iter()
        .filter_map(|a| match a {
            Action::Control(control) => Some(control),
            Action::Input(_) => None,
        })
        .collect()
}

fn mouse_mode(id: u32, slot: u8, active: bool) -> Action {
    Action::Control(Control::MouseMode {
        controller_id: id,
        slot,
        active,
    })
}

fn button(button: u8, down: bool) -> Action {
    Action::Input(Input::MouseButton { button, down })
}

/// Hold Start for `held`, release it, and return the release outcome.
fn toggle(
    pads: &mut Gamepads,
    id: u32,
    seq: &mut u64,
    t0: Instant,
    held: Duration,
    other: u32,
) -> Outcome {
    *seq += 1;
    pads.state(&state(id, *seq, button::START | other), t0);
    *seq += 1;
    pads.state(&state(id, *seq, other), t0 + held)
}

#[test]
fn connect_announces_an_xbox_pad_in_the_lowest_free_slot() {
    let mut pads = Gamepads::new();
    let out = pads.connect(&connected(7));
    assert!(out.accepted);
    assert_eq!(
        out.actions,
        vec![Action::Input(Input::Arrival {
            id: 0,
            kind: 1,
            capabilities: 0x01 | 0x02,
            buttons: 0x1000
                | 0x2000
                | 0x4000
                | 0x8000
                | 0x0100
                | 0x0200
                | 0x0020
                | 0x0010
                | 0x0040
                | 0x0080
                | 0x0001
                | 0x0002
                | 0x0004
                | 0x0008
                | 0x0400,
        })]
    );
    assert_eq!(pads.active_mask(), 1);
    pads.connect(&connected(8));
    pads.connect(&connected(9));
    assert_eq!(pads.slot(9), Some(2));
    assert_eq!(pads.active_mask(), 0b111);
    // A freed slot is reused.
    pads.disconnect(8);
    assert_eq!(pads.active_mask(), 0b101);
    pads.connect(&connected(10));
    assert_eq!(pads.slot(10), Some(1));
}

#[test]
fn capabilities_follow_the_announcement() {
    let mut pads = Gamepads::new();
    let mut c = connected(1);
    c.buttons = 7;
    c.dual_rumble = false;
    c.trigger_rumble = true;
    let out = pads.connect(&c);
    assert_eq!(
        inputs(&out),
        vec![&Input::Arrival {
            id: 0,
            kind: 1,
            capabilities: 0x04,
            buttons: 0x1000 | 0x2000 | 0x4000 | 0x8000 | 0x0100 | 0x0200,
        }]
    );
}

#[test]
fn sixteen_slots_and_no_more() {
    let mut pads = Gamepads::new();
    for id in 0..16 {
        assert!(pads.connect(&connected(id)).accepted);
    }
    assert_eq!(pads.active_mask(), 0xffff);
    assert!(!pads.connect(&connected(16)).accepted);
    assert_eq!(pads.slot(16), None);
}

#[test]
fn the_same_announcement_again_leaves_the_pad_plugged() {
    let mut pads = Gamepads::new();
    pads.connect(&connected(3));
    let before = pads.state(&state(3, 1, button::A), Instant::now());
    assert!(!inputs(&before).is_empty());
    // A client that announces on both channels sends it twice.
    let out = pads.connect(&connected(3));
    assert!(out.accepted);
    assert!(out.actions.is_empty(), "{:?}", out.actions);
    assert_eq!(pads.active_mask(), 1);
    assert_eq!(pads.slot(3), Some(0));
    // The pad kept its sequence: the next snapshot continues, not restarts.
    assert!(inputs(&pads.state(&state(3, 1, button::A), Instant::now())).is_empty());
    assert!(!inputs(&pads.state(&state(3, 2, button::B), Instant::now())).is_empty());
    // After a disconnect it is a new pad again.
    pads.disconnect(3);
    assert!(matches!(
        inputs(&pads.connect(&connected(3)))[..],
        [Input::Arrival { id: 0, .. }]
    ));
}

#[test]
fn a_reconnect_with_another_description_replaces_the_controller() {
    let mut pads = Gamepads::new();
    pads.connect(&connected(3));
    let mut changed = connected(3);
    changed.dual_rumble = !changed.dual_rumble;
    let out = pads.connect(&changed);
    assert!(out.accepted);
    // The old pad is neutralised first, then the new one announced.
    let inputs = inputs(&out);
    assert!(matches!(inputs[0], Input::Controller { active: 1, .. }));
    assert!(matches!(inputs[1], Input::Controller { active: 0, .. }));
    assert!(matches!(inputs[2], Input::Arrival { id: 0, .. }));
    assert_eq!(pads.active_mask(), 1);
}

#[test]
fn every_button_bit_maps_to_its_moonlight_flag() {
    for (bit, flag) in [
        (button::A, 0x1000),
        (button::B, 0x2000),
        (button::X, 0x4000),
        (button::Y, 0x8000),
        (button::LEFT_BUMPER, 0x0100),
        (button::RIGHT_BUMPER, 0x0200),
        (button::BACK, 0x0020),
        (button::START, 0x0010),
        (button::LEFT_STICK, 0x0040),
        (button::RIGHT_STICK, 0x0080),
        (button::DPAD_UP, 0x0001),
        (button::DPAD_DOWN, 0x0002),
        (button::DPAD_LEFT, 0x0004),
        (button::DPAD_RIGHT, 0x0008),
        (button::GUIDE, 0x0400),
    ] {
        assert_eq!(map_buttons(bit), flag, "bit {bit:#x}");
        // And through a state message.
        let mut pads = Gamepads::new();
        pads.connect(&connected(0));
        let out = pads.state(&state(0, 1, bit), Instant::now());
        assert!(matches!(
            inputs(&out)[..],
            [Input::Controller { id: 0, active: 1, buttons, .. }] if *buttons == flag
        ));
    }
    assert_eq!(map_buttons(0), 0);
    assert_eq!(map_buttons(button::ALL), 0xf7ff);
}

#[test]
fn the_button_flags_are_distinct() {
    let all = BUTTON_MAP.iter().fold(0, |acc, (_, flag)| {
        assert_eq!(acc & flag, 0, "{flag:#x} is mapped twice");
        acc | flag
    });
    assert_eq!(map_buttons(button::ALL), all);
    assert_eq!(all.count_ones(), 15);
    let bits = BUTTON_MAP.iter().fold(0, |acc, (bit, _)| acc | bit);
    assert_eq!(bits, button::ALL);
}

#[test]
fn triggers_scale_to_0_255() {
    assert_eq!(map_trigger(0.), 0);
    assert_eq!(map_trigger(1.), 255);
    assert_eq!(map_trigger(0.5), 128);
    assert_eq!(map_trigger(-0.5), 0);
    assert_eq!(map_trigger(7.), 255);
    let mut pads = Gamepads::new();
    pads.connect(&connected(0));
    let mut s = state(0, 1, 0);
    s.left_trigger = 1.;
    s.right_trigger = 0.5;
    assert!(matches!(
        inputs(&pads.state(&s, Instant::now()))[..],
        [Input::Controller {
            left_trigger: 255,
            right_trigger: 128,
            ..
        }]
    ));
}

#[test]
fn sticks_scale_to_i16_with_y_inverted() {
    assert_eq!(map_stick(1., false), 32767);
    assert_eq!(map_stick(-1., false), -32767);
    assert_eq!(map_stick(0., false), 0);
    assert_eq!(map_stick(0.5, false), 16384);
    assert_eq!(map_stick(5., false), 32767);
    assert_eq!(map_stick(1., true), -32767);
    assert_eq!(map_stick(-1., true), 32767);
    assert_eq!(map_stick(0., true), 0);
    let mut pads = Gamepads::new();
    pads.connect(&connected(0));
    let mut s = state(0, 1, 0);
    s.left_stick_x = 1.;
    s.left_stick_y = 1.;
    s.right_stick_x = -1.;
    s.right_stick_y = -1.;
    // Up on the Gamepad API (Y -1) is up positive for Moonlight.
    assert!(matches!(
        inputs(&pads.state(&s, Instant::now()))[..],
        [Input::Controller {
            sticks: [32767, -32767, -32767, 32767],
            ..
        }]
    ));
}

#[test]
fn a_stale_sequence_is_dropped() {
    let mut pads = Gamepads::new();
    pads.connect(&connected(0));
    let t = Instant::now();
    assert!(pads.state(&state(0, 5, button::A), t).accepted);
    // Out of order and duplicate snapshots change nothing.
    for seq in [5, 4, 1, 0] {
        let out = pads.state(&state(0, seq, 0), t);
        assert!(!out.accepted);
        assert!(out.actions.is_empty());
    }
    assert_eq!(pads.stale_states(0), 4);
    // A gap is counted and the newer snapshot applies.
    let out = pads.state(&state(0, 9, 0), t);
    assert!(out.accepted);
    assert_eq!(pads.sequence_gaps(0), 3);
    assert!(matches!(
        inputs(&out)[..],
        [Input::Controller { buttons: 0, .. }]
    ));
}

#[test]
fn a_keepalive_refreshes_state_without_a_change() {
    let mut pads = Gamepads::new();
    pads.connect(&connected(0));
    let t = Instant::now();
    assert_eq!(inputs(&pads.state(&state(0, 1, button::A), t)).len(), 1);
    let again = pads.state(&state(0, 2, button::A), t + KEEPALIVE);
    assert!(again.accepted);
    assert!(again.actions.is_empty());
    // The sequence moved on, so the old number is now stale.
    assert!(!pads.state(&state(0, 2, button::A), t).accepted);
    // A real change still goes through.
    assert_eq!(inputs(&pads.state(&state(0, 3, 0), t)).len(), 1);
    // And another controller joining changes the mask the pad carries.
    pads.connect(&connected(1));
    let out = pads.state(&state(0, 4, 0), t);
    assert!(matches!(
        inputs(&out)[..],
        [Input::Controller { active: 0b11, .. }]
    ));
}

#[test]
fn states_for_unknown_controllers_are_ignored() {
    let mut pads = Gamepads::new();
    let out = pads.state(&state(4, 1, button::A), Instant::now());
    assert!(!out.accepted);
    assert!(out.actions.is_empty());
    assert!(!pads.disconnect(4).accepted);
}

#[test]
fn disconnect_neutralises_the_pad_then_clears_its_mask_bit() {
    let mut pads = Gamepads::new();
    pads.connect(&connected(0));
    pads.connect(&connected(1));
    pads.state(&state(1, 1, button::A), Instant::now());
    let out = pads.disconnect(1);
    assert!(out.accepted);
    assert_eq!(
        out.actions,
        vec![
            Action::Input(Input::Controller {
                id: 1,
                active: 0b11,
                buttons: 0,
                left_trigger: 0,
                right_trigger: 0,
                sticks: [0; 4]
            }),
            Action::Input(Input::Controller {
                id: 1,
                active: 0b01,
                buttons: 0,
                left_trigger: 0,
                right_trigger: 0,
                sticks: [0; 4]
            }),
        ]
    );
    assert_eq!(pads.active_mask(), 1);
    assert_eq!(pads.slot(1), None);
}

#[test]
fn rumble_scales_motors_to_0_1() {
    assert_eq!(rumble_magnitude(0), 0.);
    assert_eq!(rumble_magnitude(65535), 1.);
    assert!((rumble_magnitude(32768) - 0.5).abs() < 1e-4);
    let mut pads = Gamepads::new();
    pads.connect(&connected(42));
    pads.connect(&connected(43));
    let Some(Control::Rumble {
        controller_id,
        slot,
        strong,
        weak,
        left_trigger,
        right_trigger,
    }) = pads.rumble(1, 65535, 0)
    else {
        panic!("no rumble");
    };
    assert_eq!((controller_id, slot), (43, 1));
    assert_eq!(
        (strong, weak, left_trigger, right_trigger),
        (1., 0., 0., 0.)
    );
    // Empty slots and slots past the table are ignored.
    assert_eq!(pads.rumble(5, 1, 1), None);
    assert_eq!(pads.rumble(99, 1, 1), None);
    // A pad that announced no trigger rumble ignores it.
    assert_eq!(pads.trigger_rumble(0, 1, 1), None);
}

#[test]
fn the_rumble_message_has_the_clients_field_names() {
    let mut pads = Gamepads::new();
    let mut c = connected(42);
    c.trigger_rumble = true;
    pads.connect(&c);
    pads.rumble(0, 65535, 0);
    let control = pads.trigger_rumble(0, 0, 65535).unwrap();
    let json: Value = serde_json::from_str(&control.to_json()).unwrap();
    assert_eq!(
        json,
        json!({
            "v": 2,
            "type": "rumble",
            "controllerId": 42,
            "controllerSlot": 0,
            "strongMagnitude": 1.0,
            "weakMagnitude": 0.0,
            "leftTrigger": 0.0,
            "rightTrigger": 1.0,
        })
    );
}

#[test]
fn host_sent_control_messages_use_version_2() {
    for control in [
        Control::MouseMode {
            controller_id: 1,
            slot: 0,
            active: false,
        },
        Control::Rumble {
            controller_id: 1,
            slot: 0,
            strong: 0.,
            weak: 0.,
            left_trigger: 0.,
            right_trigger: 0.,
        },
    ] {
        let json: Value = serde_json::from_str(&control.to_json()).unwrap();
        assert_eq!(json["v"], 2, "{control:?}");
    }
    assert_eq!(CONTROL_VERSION, 2);
}

#[test]
fn a_disconnect_stops_rumble() {
    let mut pads = Gamepads::new();
    pads.connect(&connected(42));
    pads.rumble(0, 1000, 1000);
    let out = pads.disconnect(42);
    assert_eq!(
        controls(&out),
        vec![&Control::Rumble {
            controller_id: 42,
            slot: 0,
            strong: 0.,
            weak: 0.,
            left_trigger: 0.,
            right_trigger: 0.
        }]
    );
}

#[test]
fn messages_parse_from_the_tv_apps_format() {
    let mut pads = Gamepads::new();
    let t = Instant::now();
    let out = pads
        .handle_message(
            r#"{"v":2,"type":"gamepad-connected","controllerId":3,"index":0,"browserIndex":0,
                    "id":"Pad","mapping":"standard","buttons":17,"axes":4,"dualRumble":true,
                    "triggerRumble":false}"#,
            t,
        )
        .unwrap();
    assert!(out.accepted);
    let out = pads
        .handle_message(
            r#"{"v":2,"type":"gamepad-state","controllerId":3,"seq":1,"timestampMs":12.5,
                    "buttons":129,"leftTrigger":0.5,"rightTrigger":0,"leftStickX":1,
                    "leftStickY":-1,"rightStickX":0,"rightStickY":0}"#,
            t,
        )
        .unwrap();
    assert_eq!(
        out.actions,
        vec![Action::Input(Input::Controller {
            id: 0,
            active: 1,
            buttons: 0x1000 | 0x0010,
            left_trigger: 128,
            right_trigger: 0,
            sticks: [32767, 32767, 0, 0],
        })]
    );
    let out = pads
        .handle_message(
            r#"{"v":2,"type":"gamepad-disconnected","controllerId":3}"#,
            t,
        )
        .unwrap();
    assert!(out.accepted);
    assert_eq!(pads.active_mask(), 0);
    // v 1 is accepted too.
    assert!(
            pads.handle_message(
                r#"{"v":1,"type":"gamepad-connected","controllerId":3,"id":"Pad","mapping":"","buttons":4}"#,
                t
            )
            .unwrap()
            .accepted
        );
}

#[test]
fn bad_messages_are_errors_not_panics() {
    let mut pads = Gamepads::new();
    let t = Instant::now();
    let state = |extra: &str| {
        format!(
            r#"{{"v":2,"type":"gamepad-state","controllerId":0,"seq":1,"leftTrigger":0,
                "rightTrigger":0,"leftStickX":0,"leftStickY":0,"rightStickX":0,"rightStickY":0,
                {extra}}}"#
        )
    };
    assert!(matches!(
        pads.handle_message(&state(r#""buttons":32768"#), t),
        Err(MessageError::Invalid(_))
    ));
    assert!(matches!(
        pads.handle_message(&state(r#""buttons":-1"#), t),
        Err(MessageError::Malformed(_))
    ));
    assert!(matches!(
        pads.handle_message(&state(r#""buttons":"1""#), t),
        Err(MessageError::Malformed(_))
    ));
    assert_eq!(
        pads.handle_message(r#"{"v":3,"type":"gamepad-state"}"#, t),
        Err(MessageError::Version(Some(3)))
    );
    assert_eq!(
        pads.handle_message(r#"{"v":2,"type":"rumble"}"#, t),
        Err(MessageError::UnknownType("rumble".into()))
    );
    assert!(matches!(
        pads.handle_message(r#"{"v":2,"type":"gamepad-connected","controllerId":1}"#, t),
        Err(MessageError::Malformed(_))
    ));
    for text in [
        r#"{"v":2,"type":"gamepad-connected","controllerId":1,"id":"p","mapping":"","buttons":65}"#,
        r#"{"v":2,"type":"gamepad-connected","controllerId":1,"id":"p","mapping":"","buttons":4,"axes":17}"#,
    ] {
        assert!(matches!(
            pads.handle_message(text, t),
            Err(MessageError::Invalid(_))
        ));
    }
    let long = "x".repeat(513);
    assert!(
            pads.handle_message(
                &format!(
                    r#"{{"v":2,"type":"gamepad-connected","controllerId":1,"id":"{long}","mapping":"","buttons":4}}"#
                ),
                t
            )
            .is_err()
        );
    assert!(pads.handle_message("", t).is_err());
    assert!(pads.handle_message("[1]", t).is_err());
    assert_eq!(pads.active_mask(), 0);
}
