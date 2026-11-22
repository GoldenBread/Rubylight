//! Invites to play over the internet (`butterpollo_stream::invites`): a link pairs one device as an invited
//! guest, which only ever joins the game being played when the player lets it in, and goes when
//! the invite's time ends.
use super::super::parity_tests::{device_request, finish_pairing};
use super::*;
use butterpollo_crypto::crypto;
use butterpollo_session::{
    invites::{MARKER, invited_guest},
    multiclient::{Permission, Refusal},
    state::{ALL_INPUT, LAUNCH_APPS, LIST_APPS, PAIRING_APPROVE_KEY, VIEW_STREAMS},
};
use butterpollo_stream::multiclient::{Gate, Join};
use butterpollo_stream::nvhttp::pairing::do_pair;
use rubylight_native_api::guests::{GuestInvites, InviteCreated};
use rubylight_protocol::invite_uri::InviteUri;
use std::time::{Duration, Instant};

/// The link's PIN and secret, as Nightjar derives them.
fn pin_of(link: &str) -> (String, InviteUri) {
    let parsed = InviteUri::parse(link).expect("the host's link parses");
    (
        rubylight_traverse::invite::pairing_pin(&parsed.secret),
        parsed,
    )
}

/// Pairs a new device from `link` as Nightjar would; the device and its pairing's result.
async fn redeem(f: &Fixture, link: &str, name: &str) -> (crypto::Identity, Result<()>) {
    let (pin, _) = pin_of(link);
    let identity = crypto::Identity::generate().unwrap();
    let peer = "127.0.0.1".parse().unwrap();
    let request = device_request(&identity, name, Some((&pin, "invite")));
    if let Err(error) = do_pair(f.host.clone(), &request, peer).await {
        return (identity, Err(error));
    }
    let result = finish_pairing(f, &identity, name, &pin).await;
    (identity, result)
}

fn paired(f: &Fixture, name: &str) -> Option<Client> {
    f.host
        .paired
        .read()
        .unwrap()
        .clients
        .iter()
        .find(|c| c.name == name)
        .cloned()
}

fn console_invite(
    f: &Fixture,
    permission: Option<Permission>,
) -> butterpollo_stream::invites::Created {
    butterpollo_stream::invites::create(&f.host, permission, None, None).unwrap()
}

fn join(
    h: &Shared,
    client: &Client,
    joins_game: bool,
    plain: bool,
) -> tokio::task::JoinHandle<Result<Gate, Refusal>> {
    let host = h.clone();
    let client = client.clone();
    tokio::task::spawn_blocking(move || {
        host.device_guests.gate(
            &host,
            &Join {
                client: &client,
                joins_game,
                plain,
            },
        )
    })
}

async fn until(what: &str, ready: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn a_link_pairs_one_device_as_a_guest_never_with_a_first_devices_rights() {
    // No device paired yet: an ordinary pairing would get everything and Approve new devices.
    let f = Fixture::new();
    let created = console_invite(&f, Some(Permission::Controller));
    let (_, link) = pin_of(&created.link);
    assert_eq!(link.permission, "controller");
    assert_eq!(link.unique_id, f.host.paired.read().unwrap().unique_id);
    assert_eq!(
        link.cert_sha256,
        <[u8; 32]>::from(<sha2::Sha256 as sha2::Digest>::digest(&f.host.identity.der))
    );
    assert_eq!(link.expires, created.expires);

    let (_, result) = redeem(&f, &created.link, "Friend").await;
    result.unwrap();
    let friend = paired(&f, "Friend").unwrap();
    assert_eq!(
        friend.perm,
        LIST_APPS | VIEW_STREAMS | Permission::Controller.input_mask()
    );
    assert_eq!(friend.perm & LAUNCH_APPS, 0);
    assert!(!friend.extra.contains_key(PAIRING_APPROVE_KEY));
    let guest = invited_guest(&friend).unwrap();
    assert_eq!(guest.permission, Permission::Controller);
    assert_eq!(guest.until, created.expires);
    assert!(butterpollo_stream::invites::invited_live(&friend));

    // The link is spent: a second device pairs nothing.
    let (_, again) = redeem(&f, &created.link, "Stranger").await;
    assert!(again.is_err());
    assert!(paired(&f, "Stranger").is_none());
    let view: GuestInvites =
        serde_json::from_value(butterpollo_stream::invites::view(&f.host)).unwrap();
    assert!(view.links.is_empty());
    assert_eq!(view.friends.len(), 1);
    assert_eq!(view.friends[0].name, "Friend");
}

#[tokio::test]
async fn a_wrong_or_taken_back_link_pairs_nothing() {
    let f = Fixture::new();
    f.client(ALL);
    let created = console_invite(&f, None);
    assert!(butterpollo_stream::invites::revoke(&f.host, created.id));
    assert!(!butterpollo_stream::invites::revoke(&f.host, created.id));
    let (_, result) = redeem(&f, &created.link, "Late").await;
    assert!(result.is_err());
    assert!(paired(&f, "Late").is_none());

    // Another secret: the PIN answers no invite.
    let live = console_invite(&f, None);
    let (_, mut parsed) = pin_of(&live.link);
    parsed.secret[0] ^= 1;
    let (_, result) = redeem(&f, &parsed.to_uri(), "Forger").await;
    assert!(result.is_err());
    assert!(paired(&f, "Forger").is_none());
}

#[tokio::test]
async fn only_a_device_that_may_approve_new_devices_invites_and_never_beyond_its_own_input() {
    let s = Setup::new();
    let reply = send(
        &s.f,
        true,
        &s.connection,
        "POST",
        &format!("{PREFIX}/session/guests/invites"),
        Some("application/json"),
        "{}",
    )
    .await;
    reply.assert_error(403, "permission_required");

    // Approve new devices, with controllers but no keyboard or mouse of its own.
    {
        let mut paired = s.host().paired.write().unwrap();
        let me = &mut paired.clients[0];
        me.extra.insert(PAIRING_APPROVE_KEY.into(), true.into());
        me.perm = LIST_APPS | VIEW_STREAMS | LAUNCH_APPS | Permission::Controller.input_mask();
    }
    let reply = send(
        &s.f,
        true,
        &s.connection,
        "POST",
        &format!("{PREFIX}/session/guests/invites"),
        Some("application/json"),
        serde_json::json!({"permission": "controller-keyboard-mouse", "seconds": 3600}).to_string(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let created: InviteCreated = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(created.permission, "controller");
    assert!(created.link.starts_with("rubylight://invite#"));

    let caps: Capabilities = serde_json::from_slice(&s.get("/capabilities").await.body).unwrap();
    assert!(caps.features.iter().any(|f| f == "session.guest-invites"));
    let reply = s.get("/session/guests/invites").await;
    let view: GuestInvites = serde_json::from_slice(&reply.body).unwrap();
    assert_eq!(view.links.len(), 1);
    assert_eq!(view.links[0].id, created.id);

    let reply = send(
        &s.f,
        true,
        &s.connection,
        "POST",
        &format!("{PREFIX}/session/guests/invites/revoke"),
        Some("application/json"),
        serde_json::json!({"id": created.id}).to_string(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let view: GuestInvites = serde_json::from_slice(&reply.body).unwrap();
    assert!(view.links.is_empty());
    let reply = send(
        &s.f,
        true,
        &s.connection,
        "POST",
        &format!("{PREFIX}/session/guests/invites"),
        Some("application/json"),
        serde_json::json!({"seconds": 10}).to_string(),
    )
    .await;
    reply.assert_error(422, "invalid_value");
}

#[tokio::test]
async fn an_invited_device_only_joins_the_game_being_played_and_the_player_lets_it_in() {
    let s = Setup::new();
    // `guest_join` stays off: an invited device asks anyway.
    let created = console_invite(&s.f, Some(Permission::ControllerKeyboardMouse));
    let (_, result) = redeem(&s.f, &created.link, "Friend").await;
    result.unwrap();
    let friend = paired(&s.f, "Friend").unwrap();

    // Nothing is played yet.
    assert_eq!(
        join(s.host(), &friend, true, true).await.unwrap(),
        Err(Refusal::InvitedJoinsOnly)
    );
    let _owner = s.streaming();
    // Another app, or a stream of another kind.
    assert_eq!(
        join(s.host(), &friend, false, true).await.unwrap(),
        Err(Refusal::InvitedJoinsOnly)
    );
    assert_eq!(
        join(s.host(), &friend, true, false).await.unwrap(),
        Err(Refusal::InvitedJoinsOnly)
    );
    let waiting = join(s.host(), &friend, true, true);
    until("the request", || {
        !s.host().device_guests.view(s.host()).waiting.is_empty()
    })
    .await;
    let request = s.host().device_guests.view(s.host()).waiting[0].id;
    s.host()
        .device_guests
        .approve(request, Permission::ControllerKeyboardMouse, None)
        .unwrap();
    assert_eq!(
        waiting.await.unwrap().unwrap(),
        Gate::Guest {
            id: request,
            permission: Permission::ControllerKeyboardMouse
        }
    );

    // Its time ended and the sweep has not run yet: it joins nothing.
    let mut ended = friend.clone();
    ended.extra.insert(
        MARKER.into(),
        serde_json::json!({"until": 1, "permission": "controller"}),
    );
    assert_eq!(
        join(s.host(), &ended, true, true).await.unwrap(),
        Err(Refusal::Revoked)
    );
    assert!(!butterpollo_stream::invites::invited_live(&ended));
}

#[tokio::test]
async fn an_invited_device_gets_its_internet_details_until_its_time_ends_and_then_goes() {
    let f = Fixture::new();
    f.client(ALL);
    let created = console_invite(&f, None);
    let (identity, result) = redeem(&f, &created.link, "Friend").await;
    result.unwrap();
    // Play over the internet is off, yet the invited device gets its secret.
    let described =
        butterpollo_stream::internet_play::describe(&f.host, &identity.certificate).unwrap();
    assert_eq!(described["enabled"], true);
    assert_eq!(described["secret"].as_str().map(str::len), Some(64));
    let owner_cert = f.host.identity.certificate.clone();
    let owners = butterpollo_stream::internet_play::describe(&f.host, &owner_cert).unwrap();
    assert_eq!(owners["enabled"], false);
    assert!(owners.get("secret").is_none());
    assert!(butterpollo_stream::invites::need_rendezvous(&f.host));

    // Its time ends: the sweep removes it.
    {
        let mut paired = f.host.paired.write().unwrap();
        let friend = paired
            .clients
            .iter_mut()
            .find(|c| c.name == "Friend")
            .unwrap();
        friend.extra.insert(
            MARKER.into(),
            serde_json::json!({"until": 1, "permission": "view"}),
        );
    }
    butterpollo_stream::invites::sweep(&f.host);
    assert!(paired(&f, "Friend").is_none());
    let saved = butterpollo_session::state::PairedState::load(&f.host.paired_path).unwrap();
    assert!(saved.clients.iter().all(|c| c.name != "Friend"));
}

#[tokio::test]
async fn a_device_paired_on_its_own_keeps_its_rights_when_it_opens_an_invite() {
    let f = Fixture::new();
    let created = console_invite(&f, Some(Permission::View));
    let (pin, _) = pin_of(&created.link);
    // The device pairs from the link while it already is a device of this PC with everything.
    let identity = crypto::Identity::generate().unwrap();
    f.host
        .paired
        .write()
        .unwrap()
        .add(
            &f.host.paired_path,
            Client {
                name: "Own".into(),
                cert: identity.certificate.clone(),
                uuid: "own".into(),
                perm: ALL | ALL_INPUT,
                enabled: true,
                extra: Default::default(),
            },
        )
        .unwrap();
    let peer = "127.0.0.1".parse().unwrap();
    do_pair(
        f.host.clone(),
        &device_request(&identity, "Own", Some((&pin, "invite"))),
        peer,
    )
    .await
    .unwrap();
    finish_pairing(&f, &identity, "Own", &pin).await.unwrap();
    let own = paired(&f, "Own").unwrap();
    assert_eq!(own.perm, ALL | ALL_INPUT);
    assert!(invited_guest(&own).is_none());
}
