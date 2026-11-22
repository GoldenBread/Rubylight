use super::*;
use crate::state::{ALL_INPUT, LAUNCH_APPS};

const NOW: u64 = 1_800_000_000;
const DAY: Duration = DEFAULT_LIFETIME;

fn auth(pin: &str, salt: &str) -> [u8; 32] {
    butterpollo_crypto::crypto::hash(format!("{pin}{salt}{PASSPHRASE}").as_bytes())
}

fn made(invites: &mut Invites, pin: &str) -> u32 {
    invites
        .create([1; 32], pin.into(), Permission::Controller, DAY, None, NOW)
        .unwrap()
        .id
}

#[test]
fn an_invite_matches_its_pin_once_and_only_while_it_works() {
    let mut invites = Invites::default();
    let id = made(&mut invites, "aa".repeat(16).as_str());
    let other = made(&mut invites, "bb".repeat(16).as_str());
    assert_ne!(id, other);
    let salt = "00112233445566778899aabbccddeeff";
    let good = auth(&"aa".repeat(16), salt);
    assert_eq!(invites.matching(&good, salt, NOW).map(|i| i.id), Some(id));
    assert!(invites.matching(&good, "ff", NOW).is_none(), "another salt");
    assert!(invites.matching(&good[..31], salt, NOW).is_none());
    assert!(
        invites.matching(&good, salt, NOW + DAY.as_secs()).is_none(),
        "expired"
    );

    let spent = invites.redeem(id, "device-1", "Phone", NOW + 5).unwrap();
    assert_eq!(spent.redeemed.as_ref().unwrap().device, "device-1");
    assert!(invites.matching(&good, salt, NOW + 6).is_none(), "spent");
    assert!(
        invites.redeem(id, "device-2", "Tablet", NOW + 6).is_none(),
        "single use"
    );
    assert!(
        invites
            .redeem(other, "d", "n", NOW + DAY.as_secs())
            .is_none(),
        "expired"
    );
}

#[test]
fn lifetimes_and_the_number_waiting_are_bounded() {
    let mut invites = Invites::default();
    for lifetime in [
        Duration::from_secs(299),
        MAX_LIFETIME + Duration::from_secs(1),
    ] {
        assert_eq!(
            invites
                .create([0; 32], "p".into(), Permission::View, lifetime, None, NOW)
                .unwrap_err(),
            CreateError::Lifetime
        );
    }
    for _ in 0..MAX_LIVE {
        made(&mut invites, "p");
    }
    assert_eq!(
        invites
            .create([0; 32], "p".into(), Permission::View, DAY, None, NOW)
            .unwrap_err(),
        CreateError::TooMany
    );
    // Expired ones no longer count.
    let later = NOW + DAY.as_secs();
    assert!(
        invites
            .create([0; 32], "p".into(), Permission::View, DAY, None, later)
            .is_ok()
    );
    assert_eq!(invites.list().len(), 1);
}

#[test]
fn a_spent_invite_keeps_its_rendezvous_for_the_grace_only() {
    let mut invites = Invites::default();
    let id = made(&mut invites, "p");
    assert_eq!(invites.rendezvous(NOW), vec![(id, [1; 32])]);
    invites.redeem(id, "device", "Phone", NOW + 10).unwrap();
    assert!(invites.any(NOW + 10 + SPENT_GRACE - 1));
    assert!(!invites.any(NOW + 10 + SPENT_GRACE));
    let version = invites.version();
    assert!(invites.expire(NOW + 10 + SPENT_GRACE));
    assert_ne!(invites.version(), version);
    assert!(invites.list().is_empty());
    assert!(!invites.expire(NOW + 10 + SPENT_GRACE));

    let id = made(&mut invites, "p");
    assert_eq!(invites.revoke(id).map(|i| i.id), Some(id));
    assert!(invites.revoke(id).is_none());
    assert!(!invites.any(NOW));
}

#[test]
fn debug_never_shows_the_pin_or_the_secret() {
    let mut invites = Invites::default();
    made(&mut invites, "secretpin");
    let shown = format!("{invites:?}");
    assert!(!shown.contains("secretpin"), "{shown}");
    assert!(!shown.contains("rendezvous"), "{shown}");
}

#[test]
fn an_invited_device_may_watch_and_play_but_never_launch() {
    assert_eq!(
        device_permission(Permission::View),
        LIST_APPS | VIEW_STREAMS
    );
    for permission in [
        Permission::View,
        Permission::Controller,
        Permission::ControllerKeyboardMouse,
    ] {
        let bits = device_permission(permission);
        assert_eq!(bits & LAUNCH_APPS, 0);
        assert_eq!(
            bits & !(LIST_APPS | VIEW_STREAMS | ALL_INPUT),
            0,
            "{permission:?}"
        );
        assert_eq!(bits & ALL_INPUT, permission.input_mask());
    }
}

#[test]
fn an_invite_never_gives_more_input_than_its_maker_has() {
    let full = crate::state::FIRST_DEVICE_PERMISSION;
    assert_eq!(
        capped(Permission::ControllerKeyboardMouse, full),
        Some(Permission::ControllerKeyboardMouse)
    );
    let controller_only = LIST_APPS | VIEW_STREAMS | Permission::Controller.input_mask();
    assert_eq!(
        capped(Permission::ControllerKeyboardMouse, controller_only),
        Some(Permission::Controller)
    );
    assert_eq!(capped(Permission::View, full), Some(Permission::View));
    assert_eq!(
        capped(Permission::Controller, LIST_APPS | VIEW_STREAMS),
        Some(Permission::View)
    );
    assert_eq!(capped(Permission::Controller, VIEW_STREAMS), None);
}

#[test]
fn the_marker_reads_back_and_an_unreadable_one_is_the_safe_reading() {
    let mut client = Client {
        name: "Phone".into(),
        cert: String::new(),
        uuid: "u".into(),
        perm: 0,
        enabled: true,
        extra: Default::default(),
    };
    assert_eq!(invited_guest(&client), None);
    let guest = InvitedGuest {
        until: NOW,
        permission: Permission::ControllerKeyboardMouse,
    };
    client.extra.insert(MARKER.into(), guest.marker());
    assert_eq!(invited_guest(&client), Some(guest));
    assert!(guest.ended(NOW) && !guest.ended(NOW - 1));
    client.extra.insert(MARKER.into(), json!("garbage"));
    let unreadable = invited_guest(&client).unwrap();
    assert_eq!(unreadable.permission, Permission::View);
    assert!(unreadable.ended(0));
}
