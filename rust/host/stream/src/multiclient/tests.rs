//! Unit tests for multi-client guest joins and keyboard/mouse takeover.

use super::*;
use crate::state::test_support::Fixture;
use butterpollo_session::{multiclient::LAUNCH_OPTION, session::Role};

#[test]
fn joined_rejects_unknown_or_ended_guests_without_recording_pads() {
    let f = Fixture::new();
    let mut guest_client = f.client(0);
    guest_client.uuid = "guest-device".into();
    let mut launch = f.launch(guest_client, Role::Stream);
    launch.options.insert(LAUNCH_OPTION.into(), "42".into());

    let guests = &f.host.device_guests;
    assert!(!guests.joined(42, &launch));
    assert!(locked(&guests.pads).is_empty());
    assert!(!guests.streaming.load(Ordering::Acquire));

    // Admit a valid guest, then confirm joined returns true and records its pad.
    let Admission::Granted { id, .. } = ({
        let mut state = locked(&guests.state);
        let _ = state.follow_game(Some(("game-1", "parity-device")));
        state.admit(
            "guest-device",
            "Guest",
            JoinPolicy::Allow,
            Permission::ControllerKeyboardMouse,
            unix_now(),
            Instant::now(),
        )
    }) else {
        panic!("expected guest to be admitted");
    };
    launch.options.insert(LAUNCH_OPTION.into(), id.to_string());
    assert!(guests.joined(id, &launch));
    assert!(locked(&guests.pads).contains_key(&id));
    assert!(guests.streaming.load(Ordering::Acquire));
}

#[test]
fn kbm_transfer_away_from_owner_records_owner_release() {
    let f = Fixture::new();
    let owner_client = f.client(0);
    let owner = f.session(owner_client, Role::Stream);
    let mut guest_client = f.client(0);
    guest_client.uuid = "guest-device".into();
    let mut guest_launch = f.launch(guest_client, Role::Stream);

    let guests = &f.host.device_guests;
    let Admission::Granted { id, .. } = ({
        let mut state = locked(&guests.state);
        let _ = state.follow_game(Some(("game-1", "parity-device")));
        state.admit(
            "guest-device",
            "Guest",
            JoinPolicy::Allow,
            Permission::ControllerKeyboardMouse,
            unix_now(),
            Instant::now(),
        )
    }) else {
        panic!("expected guest to be admitted");
    };
    guest_launch
        .options
        .insert(LAUNCH_OPTION.into(), id.to_string());
    assert!(guests.joined(id, &guest_launch));
    let guest = Session::new(guest_launch, Default::default());

    // Owner holds KBM, then gives it to the guest: owner must release held keys.
    assert_eq!(locked(&guests.kbm).holder(), Source::Owner);
    guests.give_keyboard_mouse(Some(id)).unwrap();
    assert!(guests.take_release(&owner));
    assert!(!guests.take_release(&owner));
    assert!(!guests.take_release(&guest));

    // Owner takes KBM back by sending input: guest must release held keys.
    let kbm_perm = butterpollo_input_policy::input::required_permission(
        &butterpollo_input_policy::input::Input::Relative { x: 1, y: 1 },
    );
    assert!(guests.admits(&owner, kbm_perm));
    assert!(guests.take_release(&guest));
    assert!(!guests.take_release(&owner));

    // Guest takes KBM via admits once handoff idle elapses: owner must release held keys.
    {
        let mut kbm = locked(&guests.kbm);
        let past = Instant::now() - policy::HANDOFF_IDLE - Duration::from_secs(1);
        assert_eq!(kbm.input(Source::Owner, past), Verdict::Accept);
    }
    assert!(guests.admits(&guest, kbm_perm));
    assert!(guests.take_release(&owner));
    assert!(!guests.take_release(&owner));
}

#[test]
fn kbm_transfer_away_from_owner_notifies_every_owner_session() {
    let f = Fixture::new();
    let owner_client = f.client(0);
    let owner_stream = f.session(owner_client.clone(), Role::Stream);
    let owner_input = f.session(owner_client, Role::InputOnly);
    let mut guest_client = f.client(0);
    guest_client.uuid = "guest-device".into();
    let mut guest_launch = f.launch(guest_client, Role::Stream);

    let guests = &f.host.device_guests;
    let Admission::Granted { id, .. } = ({
        let mut state = locked(&guests.state);
        let _ = state.follow_game(Some(("game-1", "parity-device")));
        state.admit(
            "guest-device",
            "Guest",
            JoinPolicy::Allow,
            Permission::ControllerKeyboardMouse,
            unix_now(),
            Instant::now(),
        )
    }) else {
        panic!("expected guest to be admitted");
    };
    guest_launch
        .options
        .insert(LAUNCH_OPTION.into(), id.to_string());
    assert!(guests.joined(id, &guest_launch));

    guests.give_keyboard_mouse(Some(id)).unwrap();
    assert!(guests.take_release(&owner_stream));
    assert!(guests.take_release(&owner_input));
    assert!(!guests.take_release(&owner_stream));
    assert!(!guests.take_release(&owner_input));
}
