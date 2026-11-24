//! Tests for guest input permissions, controller slot limits and backpressure queues.

use super::*;
use butterpollo_session::webrtc::guests::Permission;

/// A guest's pad announcement and snapshots, as its page sends them.
fn guest_pad(r: &mut Rig, guest: u64, id: u32, ms: u64) {
    let text = format!(
        r#"{{"v":2,"type":"gamepad-connected","controllerId":{id},"id":"Pad","mapping":"standard","buttons":17,"axes":4,"dualRumble":true,"triggerRumble":true}}"#
    );
    r.core.guest_gamepad(guest, &text, r.at(ms));
}

fn guest_state(r: &mut Rig, guest: u64, id: u32, seq: u64, buttons: u32, ms: u64) {
    let text = format!(
        r#"{{"v":2,"type":"gamepad-state","controllerId":{id},"seq":{seq},"buttons":{buttons},"leftTrigger":0,"rightTrigger":0,"leftStickX":0,"leftStickY":0,"rightStickX":0,"rightStickY":0}}"#
    );
    r.core.guest_gamepad(guest, &text, r.at(ms));
}

/// What a guest's control channel got.
fn guest_control(r: &mut Rig, guest: u64) -> Arc<Mutex<Vec<Value>>> {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let log = sent.clone();
    r.core.guests.get_mut(&guest).unwrap().control = Some(Box::new(move |data| {
        log.lock()
            .unwrap()
            .push(serde_json::from_slice(&data).unwrap());
        true
    }));
    sent
}

#[test]
fn a_view_only_guests_input_is_dropped_by_the_host() {
    let mut r = rig(u32::MAX);
    r.core.guest_joined(1, Permission::View);
    guest_pad(&mut r, 1, 0, 0);
    guest_state(&mut r, 1, 0, 1, A, 10);
    r.core.guest_input(
        1,
        r#"{"v":1,"type":"key","code":"KeyA","down":true}"#,
        r.at(20),
    );
    r.core
        .guest_input(1, r#"{"v":1,"type":"mouse-move","dx":3,"dy":0}"#, r.at(30));
    assert!(r.events().is_empty(), "{:?}", r.events());
    assert_eq!(r.core.pads.active_mask(), 0, "no slot either");
    assert_eq!(r.core.summary().dropped, 4);
    // Nor does a guest the worker never heard of get anything through.
    guest_pad(&mut r, 9, 0, 40);
    assert!(r.events().is_empty());
}

#[test]
fn a_controller_guest_gets_its_own_pad_but_no_keyboard_mouse_or_mouse_mode() {
    let mut r = rig(u32::MAX);
    r.pad(0, 0);
    r.core.guest_joined(1, Permission::Controller);
    let sent = guest_control(&mut r, 1);
    // The guest's browser numbers its pad 0 too: it is a second pad, in the next slot.
    guest_pad(&mut r, 1, 0, 0);
    guest_state(&mut r, 1, 0, 1, A, 10);
    assert_eq!(r.core.pads.active_mask(), 0b11);
    let events = r.events();
    assert!(
        matches!(events[1], Input::Arrival { id: 1, .. }),
        "{events:?}"
    );
    assert!(matches!(
        events[2],
        Input::Controller {
            id: 1,
            active: 0b11,
            buttons: 0x1000,
            ..
        }
    ));
    r.rec.clear();
    r.core.guest_input(
        1,
        r#"{"v":1,"type":"key","code":"KeyA","down":true}"#,
        r.at(20),
    );
    // Holding Start would make the pad a mouse; for this guest it stays a pad.
    guest_state(&mut r, 1, 0, 2, START, 30);
    guest_state(&mut r, 1, 0, 3, 0, 900);
    guest_state(&mut r, 1, 0, 4, A, 950);
    assert!(!r.core.pads.mouse_mode(GUEST_IDS | 1 << GUEST_ID_BITS));
    assert!(
        r.events()
            .iter()
            .all(|e| matches!(e, Input::Controller { id: 1, .. })),
        "{:?}",
        r.events()
    );
    assert!(sent.lock().unwrap().is_empty());
    assert!(r.sent("mouse-mode").is_empty());
}

#[test]
fn a_guests_rumble_and_mouse_mode_go_to_the_guest_under_its_own_id() {
    let mut r = rig(u32::MAX);
    r.core.guest_joined(4, Permission::ControllerKeyboardMouse);
    let sent = guest_control(&mut r, 4);
    guest_pad(&mut r, 4, 2, 0);
    r.rec
        .post_feedback(0, RUMBLE_DATA, &[0xff, 0xff, 0x00, 0x00]);
    assert!(r.core.poll(r.at(8)));
    guest_state(&mut r, 4, 2, 1, START, 10);
    guest_state(&mut r, 4, 2, 2, 0, 800);
    let sent = sent.lock().unwrap().clone();
    let kinds: Vec<_> = sent.iter().map(|m| m["type"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["rumble", "mouse-mode"]);
    assert!(sent.iter().all(|m| m["controllerId"] == 2));
    assert!(r.sent("rumble").is_empty(), "nothing reaches the owner");
    assert!(r.sent("mouse-mode").is_empty());
}

#[test]
fn a_keyboard_guest_types_and_what_it_holds_is_let_go_when_it_leaves() {
    let mut r = rig(u32::MAX);
    r.core.guest_joined(2, Permission::ControllerKeyboardMouse);
    r.input(r#"{"v":1,"type":"key","code":"KeyW","down":true}"#);
    for text in [
        r#"{"v":1,"type":"key","code":"KeyA","down":true}"#,
        r#"{"v":1,"type":"mouse-button","button":0,"down":true}"#,
        r#"{"v":1,"type":"text","text":"hi"}"#,
    ] {
        r.core.guest_input(2, text, r.at(1));
    }
    guest_pad(&mut r, 2, 0, 2);
    assert_eq!(
        r.events()[..4],
        [
            key_down(0x57),
            key_down(0x41),
            button(1, true),
            Input::Text("hi".into())
        ]
    );
    r.rec.clear();
    r.core.guest_left(2, r.at(3));
    let events = r.events();
    assert!(events.contains(&key_up(0x41)) && events.contains(&button(1, false)));
    assert!(
        !events.contains(&key_up(0x57)),
        "the owner's key stays down"
    );
    assert_eq!(r.core.pads.active_mask(), 0, "its pad is unplugged");
    assert!(r.core.guests.is_empty());
}

#[test]
fn the_owners_gamepad_channel_closing_leaves_the_guests_pads_plugged() {
    let mut r = rig(u32::MAX);
    r.pad(0, 0);
    r.core.guest_joined(1, Permission::Controller);
    r.core.guest_joined(2, Permission::Controller);
    guest_pad(&mut r, 1, 0, 0);
    guest_pad(&mut r, 2, 0, 0);
    assert_eq!(r.core.pads.active_mask(), 0b111);
    r.core.unplug(r.at(1));
    assert_eq!(r.core.pads.active_mask(), 0b110);
    r.core.guest_unplug(1, r.at(2));
    assert_eq!(r.core.pads.active_mask(), 0b100, "one guest's pads only");
    // The end of the stream unplugs everyone's.
    r.core.finish(r.at(3));
    assert_eq!(r.core.pads.active_mask(), 0);
}

#[test]
fn guests_share_the_owners_sixteen_slots() {
    let mut r = rig(u32::MAX);
    r.core.guest_joined(1, Permission::Controller);
    r.core.guest_joined(2, Permission::Controller);
    for id in 0..10 {
        guest_pad(&mut r, 1, id, 0);
    }
    for id in 0..10 {
        guest_pad(&mut r, 2, id, 0);
    }
    assert_eq!(r.core.pads.active_mask(), 0xff);
    assert_eq!(
        r.core.summary().gamepad_ignored,
        12,
        "each guest's pads past its fourth get no slot"
    );
    // An id beyond the guest's range is refused, not folded onto another pad.
    guest_pad(&mut r, 1, 1 << 16, 0);
    assert_eq!(r.core.summary().invalid, 1);
}

#[test]
fn guest_controller_ids_round_trip_and_never_look_like_the_owners() {
    for (tag, id) in [(1, 0), (2, 0), (MAX_GUEST_TAG, 0xffff)] {
        let shared = guest_controller(tag, id).unwrap();
        assert_eq!(guest_of(shared), Some((tag, id)));
    }
    assert_eq!(guest_of(3), None);
    assert_eq!(guest_controller(1, 1 << 16), None);
}

#[test]
fn a_guest_plugs_in_four_pads_at_most_and_the_owner_keeps_four_slots() {
    let mut r = rig(u32::MAX);
    for guest in 1..=4 {
        r.core.guest_joined(guest, Permission::Controller);
    }
    // One guest's browser numbering pads up to 65535 gets four of them.
    for id in [0, 1, 2, 3, 4, 900, 65_535] {
        guest_pad(&mut r, 1, id, 0);
    }
    assert_eq!(r.core.pads.controller_ids().len(), 4);
    // Announcing a pad it has already plugged in again changes nothing.
    guest_pad(&mut r, 1, 0, 5);
    assert_eq!(r.core.pads.controller_ids().len(), 4);
    for guest in 2..=4 {
        for id in 0..4 {
            guest_pad(&mut r, guest, id, 10);
        }
    }
    // The guests together stop at twelve: the fourth guest got none.
    assert_eq!(r.core.pads.controller_ids().len(), 12);
    assert!(
        r.core
            .pads
            .controller_ids()
            .into_iter()
            .all(|c| guest_of(c).is_none_or(|(tag, _)| tag != r.core.guests[&4].tag))
    );
    // The owner's four still plug in, into every slot left.
    for id in 0..4 {
        r.pad(id, 20);
    }
    assert_eq!(r.core.pads.active_mask(), u16::MAX);
    // A guest that left frees its share for the next.
    r.core.guest_left(1, r.at(30));
    guest_pad(&mut r, 4, 0, 40);
    assert_eq!(r.core.pads.controller_ids().len(), 13);
}

#[test]
fn a_guest_handle_queues_only_what_its_permission_allows_and_a_bounded_amount() {
    let (handle, queues) = channel();
    assert!(
        handle.guest(1, Permission::View).is_none(),
        "view only sends nothing"
    );
    let guest = handle.guest(2, Permission::Controller).unwrap();
    let now = Instant::now();
    for _ in 0..GUEST_QUEUE * 4 {
        guest.gamepad(b"{}".to_vec(), false, now);
    }
    // A controller guest's keys and mice never reach the queue.
    guest.input(
        br#"{"v":1,"type":"key","code":"KeyA","down":true}"#.to_vec(),
        false,
    );
    {
        let inbox = queues.guests.state.lock().unwrap();
        assert_eq!(
            inbox.queue.len(),
            1 + GUEST_QUEUE,
            "its join, then the bound"
        );
        assert_eq!(inbox.waiting[&2].data, GUEST_QUEUE);
        assert!(
            inbox.queue.iter().all(|(m, _)| matches!(
                m,
                GuestMessage::Joined(2, _) | GuestMessage::Gamepad(2, ..)
            ))
        );
    }
    // However much it sent, the owner's queue got one wake.
    handle.gamepad(b"{}".to_vec(), false, now);
    let owner: Vec<Message> = queues.rx.try_iter().collect();
    assert_eq!(owner.len(), 2);
    assert!(matches!(owner[0], Message::Guests));
    assert!(matches!(owner[1], Message::Gamepad(..)));
    // Leaving always queues, even at the bound, and taking the queue frees the guest's share.
    drop(guest);
    let mut taken = 0;
    while let Some(message) = queues.guests.pop() {
        taken += 1;
        if taken == 2 + GUEST_QUEUE {
            assert!(matches!(message, GuestMessage::Left(2)));
        }
    }
    assert_eq!(taken, 2 + GUEST_QUEUE);
    assert!(queues.guests.state.lock().unwrap().waiting.is_empty());
    // A wake already queued covers what comes after it; once the worker took it, the next
    // message wakes it again.
    assert!(queues.rx.try_recv().is_err());
    queues.guests.woken.store(false, Ordering::Release);
    let again = handle
        .guest(3, Permission::ControllerKeyboardMouse)
        .unwrap();
    again.input(
        br#"{"v":1,"type":"key","code":"KeyA","down":true}"#.to_vec(),
        false,
    );
    let owner: Vec<Message> = queues.rx.try_iter().collect();
    assert_eq!(owner.len(), 1);
    assert!(matches!(owner[0], Message::Guests));
}

#[test]
fn the_worker_takes_the_owners_input_before_a_guests() {
    let f = Fixture::new();
    let rec = Recorder::default();
    let make = rec.clone();
    let (handle, queues) = channel();
    let s = session(&f, u32::MAX);
    let worker = std::thread::spawn(move || run(queues, Box::new(move |_| make.make())));
    // The guest's pad is announced first; the owner's after it, before the worker runs.
    let guest = handle.guest(1, Permission::Controller).unwrap();
    // The owner's pad has no motors, so its arrival tells the two apart.
    let announce = |id: u32, rumble: bool| {
        format!(
            r#"{{"v":2,"type":"gamepad-connected","controllerId":{id},"id":"Pad","mapping":"standard","buttons":17,"axes":4,"dualRumble":{rumble},"triggerRumble":false}}"#
        )
        .into_bytes()
    };
    guest.gamepad(announce(0, true), false, Instant::now());
    handle.gamepad(announce(5, false), false, Instant::now());
    handle.bind(s.clone());
    wait_for("both pads arrived", || {
        rec.events()
            .iter()
            .filter(|e| matches!(e, Input::Arrival { .. }))
            .count()
            == 2
    });
    // The owner's pad arrived first, in the first slot.
    let arrivals: Vec<(u8, u16)> = rec
        .events()
        .iter()
        .filter_map(|e| match e {
            Input::Arrival {
                id, capabilities, ..
            } => Some((*id, *capabilities)),
            _ => None,
        })
        .collect();
    assert_eq!((arrivals[0].0, arrivals[1].0), (0, 1));
    assert!(arrivals[0].1 < arrivals[1].1, "{arrivals:?}");
    drop(guest);
    s.stop();
    worker.join().unwrap();
    drop(handle);
}

/// Feed what the guests' queue holds to the rig's worker, in order.
fn drain(r: &mut Rig, queues: &Queues) {
    while let Some(message) = queues.guests.pop() {
        let now = r.at(1);
        r.core.guest(message, now);
    }
}

fn keyboard(events: &[Input]) -> Vec<bool> {
    events
        .iter()
        .filter_map(|e| match e {
            Input::Keyboard { down, .. } => Some(*down),
            _ => None,
        })
        .collect()
}

const SHIFT_DOWN: &[u8] = br#"{"v":1,"type":"key","code":"ShiftLeft","down":true}"#;
const SHIFT_UP: &[u8] = br#"{"v":1,"type":"key","code":"ShiftLeft","down":false}"#;

fn pad_state(id: u32, seq: u64, buttons: u32) -> Vec<u8> {
    format!(
        r#"{{"v":2,"type":"gamepad-state","controllerId":{id},"seq":{seq},"buttons":{buttons},"leftTrigger":0,"rightTrigger":0,"leftStickX":0,"leftStickY":0,"rightStickX":0,"rightStickY":0}}"#
    )
    .into_bytes()
}

fn pad_connected(id: u32) -> Vec<u8> {
    format!(
        r#"{{"v":2,"type":"gamepad-connected","controllerId":{id},"id":"Pad","mapping":"standard","buttons":17,"axes":4,"dualRumble":true,"triggerRumble":true}}"#
    )
    .into_bytes()
}

#[test]
fn a_guests_key_release_queues_past_the_bound_while_motion_gives_way() {
    let mut r = rig(u32::MAX);
    let (handle, queues) = channel();
    let guest = handle
        .guest(1, Permission::ControllerKeyboardMouse)
        .unwrap();
    let now = Instant::now();
    guest.input(SHIFT_DOWN.to_vec(), false);
    // The worker stalls while the guest moves the mouse: past the bound the motion is dropped.
    for _ in 0..GUEST_QUEUE * 2 {
        guest.input(
            br#"{"v":1,"type":"mouse-move","dx":1,"dy":0}"#.to_vec(),
            false,
        );
    }
    // A pad plugs in and changes state three times: only its latest state waits.
    guest.gamepad(pad_connected(0), false, now);
    guest.gamepad(pad_state(0, 1, A), false, now);
    guest.gamepad(pad_state(0, 2, 0), false, now);
    guest.gamepad(pad_state(0, 3, A), false, now);
    // The guest lets go of Shift while the queue is full.
    guest.input(SHIFT_UP.to_vec(), false);
    {
        let inbox = queues.guests.state.lock().unwrap();
        // Its join, the bound (Shift and 63 moves), the pad, one state and Shift's release.
        assert_eq!(inbox.queue.len(), 1 + GUEST_QUEUE + 3);
        let states: Vec<&GuestMessage> = inbox
            .queue
            .iter()
            .map(|(m, _)| m)
            .filter(|m| matches!(m, GuestMessage::Gamepad(_, text, _) if text.contains("gamepad-state")))
            .collect();
        assert_eq!(states.len(), 1);
        assert!(
            matches!(states[0], GuestMessage::Gamepad(_, text, _) if text.contains(r#""seq":3"#))
        );
    }
    drain(&mut r, &queues);
    let events = r.events();
    assert_eq!(keyboard(&events), [true, false], "Shift ends up released");
    assert!(
        events.iter().any(|e| matches!(
            e,
            Input::Controller {
                buttons: 0x1000,
                ..
            }
        )),
        "the pad's latest state arrived: {events:?}"
    );
    assert!(queues.guests.state.lock().unwrap().waiting.is_empty());
}

#[test]
fn a_guest_past_its_backlog_has_everything_it_held_released() {
    let mut r = rig(u32::MAX);
    let (handle, queues) = channel();
    let guest = handle
        .guest(1, Permission::ControllerKeyboardMouse)
        .unwrap();
    let now = Instant::now();
    guest.gamepad(pad_connected(0), false, now);
    guest.input(SHIFT_DOWN.to_vec(), false);
    // A page that clicks faster than a stalled desktop takes it fills its whole backlog.
    for i in 0..GUEST_BACKLOG - 2 {
        let down = i % 2 == 0;
        guest.input(
            format!(r#"{{"v":1,"type":"mouse-button","button":0,"down":{down}}}"#).into_bytes(),
            false,
        );
    }
    // Shift's release and the pad's unplugging no longer fit.
    guest.input(SHIFT_UP.to_vec(), false);
    guest.gamepad(
        br#"{"v":2,"type":"gamepad-disconnected","controllerId":0}"#.to_vec(),
        false,
        now,
    );
    {
        let inbox = queues.guests.state.lock().unwrap();
        assert_eq!(inbox.queue.len(), 1 + GUEST_BACKLOG + 1);
        assert!(matches!(
            inbox.queue.back(),
            Some((GuestMessage::Lost(1), _))
        ));
        assert_eq!(
            inbox
                .queue
                .iter()
                .filter(|(m, _)| matches!(m, GuestMessage::Lost(_)))
                .count(),
            1,
            "one release covers every message dropped in a row"
        );
    }
    drain(&mut r, &queues);
    let events = r.events();
    assert_eq!(keyboard(&events), [true, false], "Shift ends up released");
    assert_eq!(r.core.pads.active_mask(), 0, "no pad is left plugged in");
    // The guest still plays: what it sends next goes through.
    guest.input(SHIFT_DOWN.to_vec(), false);
    drain(&mut r, &queues);
    assert_eq!(keyboard(&r.events()), [true, false, true]);
    assert!(queues.guests.state.lock().unwrap().waiting.is_empty());
}
