//! Tests for WebRTC gamepad mouse emulation mode, pointer curves, and release cleanup.
use super::super::*;
use super::{button, connected, controls, inputs, mouse_mode, state, toggle};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

#[test]
fn mouse_mode_needs_a_750_ms_hold() {
    let t0 = Instant::now();
    let ms = Duration::from_millis;
    let mut pads = Gamepads::new();
    pads.connect(&connected(0));
    let mut seq = 0;
    let out = toggle(&mut pads, 0, &mut seq, t0, ms(749), 0);
    assert!(!pads.mouse_mode(0));
    assert!(controls(&out).is_empty());
    // Start released: the game saw it go down and up like any press.
    assert!(matches!(
        inputs(&out)[..],
        [Input::Controller { buttons: 0, .. }]
    ));

    let out = toggle(&mut pads, 0, &mut seq, t0, ms(750), 0);
    assert!(pads.mouse_mode(0));
    assert!(out.actions.contains(&mouse_mode(0, 0, true)));
    assert_eq!(pads.next_tick(), Some(t0 + ms(750) + MOUSE_TICK));

    // The same long hold again turns it off.
    let out = toggle(&mut pads, 0, &mut seq, t0 + ms(2000), ms(900), 0);
    assert!(!pads.mouse_mode(0));
    assert!(out.actions.contains(&mouse_mode(0, 0, false)));
    assert_eq!(pads.next_tick(), None);
}

#[test]
fn a_controller_without_mouse_mode_stays_a_pad_after_a_long_start_hold() {
    let t0 = Instant::now();
    let mut pads = Gamepads::new();
    pads.connect_with(&connected(3), false);
    pads.connect(&connected(4));
    assert_eq!(pads.controller_ids(), [3, 4]);
    let mut seq = 0;
    let out = toggle(&mut pads, 3, &mut seq, t0, Duration::from_secs(2), 0);
    assert!(!pads.mouse_mode(3));
    assert!(controls(&out).is_empty(), "no mouse-mode message");
    assert!(matches!(
        inputs(&out)[..],
        [Input::Controller { buttons: 0, .. }]
    ));
    assert_eq!(pads.next_tick(), None);
    let mut seq = 0;
    toggle(&mut pads, 4, &mut seq, t0, Duration::from_secs(2), 0);
    assert!(pads.mouse_mode(4), "the other controller keeps it");
}

#[test]
fn a_held_start_does_not_toggle_until_it_is_released() {
    let t0 = Instant::now();
    let mut pads = Gamepads::new();
    pads.connect(&connected(0));
    pads.state(&state(0, 1, button::START), t0);
    let out = pads.state(&state(0, 2, button::START), t0 + Duration::from_secs(5));
    assert!(!pads.mouse_mode(0));
    assert!(out.accepted);
    let out = pads.state(&state(0, 3, 0), t0 + Duration::from_secs(6));
    assert!(pads.mouse_mode(0));
    assert!(!controls(&out).is_empty());
}

#[test]
fn mouse_mode_message_has_the_clients_field_names() {
    let json: Value = serde_json::from_str(
        &Control::MouseMode {
            controller_id: 5,
            slot: 2,
            active: true,
        }
        .to_json(),
    )
    .unwrap();
    assert_eq!(
        json,
        json!({"v":2,"type":"mouse-mode","controllerId":5,"controllerSlot":2,"active":true})
    );
}

#[test]
fn entering_mouse_mode_neutralises_the_game_pad() {
    let t0 = Instant::now();
    let mut pads = Gamepads::new();
    pads.connect(&connected(0));
    let mut seq = 0;
    // B and the d-pad are held through the toggle.
    let other = button::B | button::DPAD_LEFT;
    let out = toggle(&mut pads, 0, &mut seq, t0, Duration::from_secs(1), other);
    assert!(pads.mouse_mode(0));
    assert_eq!(
        inputs(&out),
        vec![&Input::Controller {
            id: 0,
            active: 1,
            buttons: 0,
            left_trigger: 0,
            right_trigger: 0,
            sticks: [0; 4]
        }]
    );
}

#[test]
fn buttons_in_mouse_mode_are_mouse_buttons_and_do_not_reach_the_pad() {
    let t0 = Instant::now();
    let mut pads = Gamepads::new();
    pads.connect(&connected(0));
    let mut seq = 0;
    toggle(&mut pads, 0, &mut seq, t0, Duration::from_secs(1), 0);
    for (bit, mouse) in [
        (button::A, 1),
        (button::B, 3),
        (button::X, 2),
        (button::LEFT_BUMPER, 4),
        (button::RIGHT_BUMPER, 5),
    ] {
        seq += 1;
        let down = pads.state(&state(0, seq, bit), t0);
        assert_eq!(down.actions, vec![button(mouse, true)], "{bit:#x}");
        seq += 1;
        let up = pads.state(&state(0, seq, 0), t0);
        assert_eq!(up.actions, vec![button(mouse, false)], "{bit:#x}");
    }
    // Buttons without a mouse meaning do nothing at all: not a pad state either.
    for bit in [
        button::Y,
        button::BACK,
        button::START,
        button::LEFT_STICK,
        button::RIGHT_STICK,
        button::GUIDE,
    ] {
        seq += 1;
        assert!(pads.state(&state(0, seq, bit), t0).actions.is_empty());
        seq += 1;
        assert!(pads.state(&state(0, seq, 0), t0).actions.is_empty());
    }
}

#[test]
fn dpad_presses_scroll_one_notch() {
    let t0 = Instant::now();
    let mut pads = Gamepads::new();
    pads.connect(&connected(0));
    let mut seq = 0;
    toggle(&mut pads, 0, &mut seq, t0, Duration::from_secs(1), 0);
    for (bit, amount, horizontal) in [
        (button::DPAD_UP, 120, false),
        (button::DPAD_DOWN, -120, false),
        (button::DPAD_RIGHT, 120, true),
        (button::DPAD_LEFT, -120, true),
    ] {
        seq += 1;
        let out = pads.state(&state(0, seq, bit), t0);
        assert_eq!(
            out.actions,
            vec![Action::Input(Input::Scroll { amount, horizontal })]
        );
        // Held, a keepalive scrolls nothing more; the release is silent.
        seq += 1;
        assert!(pads.state(&state(0, seq, bit), t0).actions.is_empty());
        seq += 1;
        assert!(pads.state(&state(0, seq, 0), t0).actions.is_empty());
    }
}

#[test]
fn buttons_held_when_the_mode_turns_on_stay_suppressed_until_released() {
    let t0 = Instant::now();
    let mut pads = Gamepads::new();
    pads.connect(&connected(0));
    let mut seq = 0;
    // A is held through the toggle.
    toggle(
        &mut pads,
        0,
        &mut seq,
        t0,
        Duration::from_secs(1),
        button::A,
    );
    assert!(pads.mouse_mode(0));
    seq += 1;
    assert!(pads.state(&state(0, seq, button::A), t0).actions.is_empty());
    // B joins while A is still held; only B clicks.
    seq += 1;
    let out = pads.state(&state(0, seq, button::A | button::B), t0);
    assert_eq!(out.actions, vec![button(3, true)]);
    // A is released and pressed again: now it is a real click.
    seq += 1;
    let out = pads.state(&state(0, seq, button::B), t0);
    assert_eq!(out.actions, vec![]);
    seq += 1;
    let out = pads.state(&state(0, seq, button::B | button::A), t0);
    assert_eq!(out.actions, vec![button(1, true)]);
}

#[test]
fn leaving_mouse_mode_releases_every_simulated_button() {
    let t0 = Instant::now();
    let mut pads = Gamepads::new();
    pads.connect(&connected(0));
    let mut seq = 0;
    toggle(&mut pads, 0, &mut seq, t0, Duration::from_secs(1), 0);
    seq += 1;
    pads.state(&state(0, seq, button::A | button::RIGHT_BUMPER), t0);
    // Hold both, then press Start for a toggle.
    seq += 1;
    pads.state(
        &state(0, seq, button::A | button::RIGHT_BUMPER | button::START),
        t0 + Duration::from_secs(2),
    );
    seq += 1;
    let out = pads.state(
        &state(0, seq, button::A | button::RIGHT_BUMPER),
        t0 + Duration::from_secs(3),
    );
    assert!(!pads.mouse_mode(0));
    assert_eq!(
        out.actions[..3],
        [button(1, false), button(5, false), mouse_mode(0, 0, false)]
    );
    // The pad gets its real state back, held buttons included.
    assert!(matches!(
        inputs(&out)[2..],
        [Input::Controller { buttons, .. }] if *buttons == 0x0200 | 0x1000
    ));
    // Nothing stays simulated: a later release does not repeat.
    seq += 1;
    let out = pads.state(&state(0, seq, 0), t0 + Duration::from_secs(4));
    assert!(controls(&out).is_empty());
    assert!(
        inputs(&out)
            .iter()
            .all(|i| !matches!(i, Input::MouseButton { .. }))
    );
}

#[test]
fn disconnecting_in_mouse_mode_releases_the_buttons() {
    let t0 = Instant::now();
    let mut pads = Gamepads::new();
    pads.connect(&connected(0));
    let mut seq = 0;
    toggle(&mut pads, 0, &mut seq, t0, Duration::from_secs(1), 0);
    seq += 1;
    pads.state(&state(0, seq, button::X | button::LEFT_BUMPER), t0);
    let out = pads.disconnect(0);
    assert_eq!(
        out.actions[..3],
        [button(2, false), button(4, false), mouse_mode(0, 0, false)]
    );
    assert_eq!(pads.next_tick(), None);
    assert!(pads.tick(t0 + Duration::from_secs(60)).is_empty());
}

#[test]
fn the_end_of_the_stream_releases_everything() {
    let t0 = Instant::now();
    let mut pads = Gamepads::new();
    pads.connect(&connected(0));
    pads.connect(&connected(1));
    let mut seq = 0;
    toggle(&mut pads, 1, &mut seq, t0, Duration::from_secs(1), 0);
    seq += 1;
    pads.state(&state(1, seq, button::A), t0);
    pads.state(&state(0, 1, button::B), t0);
    let actions = pads.close();
    assert!(actions.contains(&button(1, false)));
    assert!(actions.contains(&mouse_mode(1, 1, false)));
    assert_eq!(pads.active_mask(), 0);
    assert_eq!(pads.slot(0), None);
    assert_eq!(pads.next_tick(), None);
    // Both pads end neutral, with their bits gone from the mask.
    let last: Vec<_> = actions
        .iter()
        .filter_map(|a| match a {
            Action::Input(Input::Controller { id, active, .. }) => Some((*id, *active)),
            _ => None,
        })
        .collect();
    assert_eq!(last, vec![(0, 0b11), (0, 0b10), (1, 0b10), (1, 0)]);
    assert!(pads.close().is_empty());
}

#[test]
fn the_stick_moves_the_pointer_every_50_ms() {
    let t0 = Instant::now();
    let ms = Duration::from_millis;
    let mut pads = Gamepads::new();
    pads.connect(&connected(0));
    let mut seq = 0;
    toggle(&mut pads, 0, &mut seq, t0, ms(1000), 0);
    let due = pads.next_tick().unwrap();
    let mut full_right_down = state(0, 100, 0);
    full_right_down.left_stick_x = 1.;
    full_right_down.left_stick_y = 1.;
    // Sticks do not reach the game pad.
    assert!(
        pads.state(&full_right_down, t0 + ms(1010))
            .actions
            .is_empty()
    );

    // Too early: nothing, and the schedule is untouched.
    assert!(pads.tick(due - ms(1)).is_empty());
    assert_eq!(pads.next_tick(), Some(due));
    assert_eq!(
        pads.tick(due),
        vec![Action::Input(Input::Relative { x: 62, y: 62 })]
    );
    assert_eq!(pads.next_tick(), Some(due + MOUSE_TICK));
    assert!(pads.tick(due + ms(49)).is_empty());
    assert_eq!(pads.tick(due + ms(50)).len(), 1);
    // A centred stick makes no motion.
    pads.state(&state(0, 101, 0), due + ms(60));
    assert!(pads.tick(due + ms(100)).is_empty());
}

#[test]
fn the_pointer_curve_is_cubic_with_a_deadzone() {
    // Full deflection: (32767/32766*4)^3 = 64.0, less the deadzone of 2.
    assert_eq!(mouse_delta([32767, 0, 0, 0]), (62, 0));
    assert_eq!(mouse_delta([-32767, 0, 0, 0]), (-62, 0));
    // Half: (0.5*4)^3 = 8, less 2.
    assert_eq!(mouse_delta([16383, 0, 0, 0]), (6, 0));
    // A third: (1/3*4)^3 = 2.37, less 2, rounds to 0.
    assert_eq!(mouse_delta([10922, 0, 0, 0]), (0, 0));
    // Just inside the deadzone: 4*raw/32766 cubed <= 2 up to raw 10320.
    assert_eq!(mouse_delta([10320, 0, 0, 0]), (0, 0));
    assert_eq!(mouse_delta([0, 0, 0, 0]), (0, 0));
    // Small pushes are lost entirely.
    assert_eq!(mouse_delta([3000, -3000, 0, 0]), (0, 0));
    // Y is Moonlight up positive; the pointer's is down positive.
    assert_eq!(mouse_delta([0, 32767, 0, 0]), (0, -62));
    assert_eq!(mouse_delta([0, -16383, 0, 0]), (0, 6));
    // Whichever stick is pushed further drives the pointer.
    assert_eq!(mouse_delta([16383, 0, -32767, 0]), (-62, 0));
    assert_eq!(mouse_delta([32767, 0, 16383, 16383]), (62, 0));
    // The curve is monotonic.
    let mut last = 0;
    for raw in (0..=32767).step_by(256) {
        let (x, _) = mouse_delta([raw, 0, 0, 0]);
        assert!(x >= last);
        last = x;
    }
}

#[test]
fn every_tick_moves_every_controller_in_mouse_mode() {
    let t0 = Instant::now();
    let mut pads = Gamepads::new();
    pads.connect(&connected(0));
    pads.connect(&connected(1));
    let mut seq = 0;
    toggle(&mut pads, 0, &mut seq, t0, Duration::from_secs(1), 0);
    let mut s = state(0, 50, 0);
    s.right_stick_x = -1.;
    pads.state(&s, t0);
    // Controller 1 is not in mouse mode, so it moves nothing.
    let mut s = state(1, 1, 0);
    s.right_stick_x = 1.;
    pads.state(&s, t0);
    let due = pads.next_tick().unwrap();
    assert_eq!(
        pads.tick(due),
        vec![Action::Input(Input::Relative { x: -62, y: 0 })]
    );
}
