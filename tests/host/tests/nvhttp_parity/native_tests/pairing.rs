//! `/pairing/*`: the real handshake behind an approval from another device.
use super::super::parity_tests::{device_request, finish_pairing, one_time_pin};
use super::*;
use butterpollo_crypto::crypto;
use butterpollo_session::state::{
    FIRST_DEVICE_PERMISSION, LATER_DEVICE_PERMISSION, PLAY_DEVICE_PERMISSION,
};
use butterpollo_stream::{
    nvhttp::pairing::{do_pair, pairing_id},
    pairing_approvals as pairing,
};
use rubylight_native_api::{capabilities::Capabilities, pairing::PairingRequests};
use std::time::{Duration, Instant};

const PEER: &str = "127.0.0.1";

fn clients(h: &Shared) -> Arc<butterpollo_webrtc::clients::Registry> {
    butterpollo_webrtc::WebRtcRuntime::for_host(h)
        .clients
        .clone()
}

async fn approve(f: &Fixture, connection: &Connection, id: &str, pin: &str) -> Reply {
    send(
        f,
        true,
        connection,
        "POST",
        &format!("{PREFIX}/pairing/approve"),
        Some("application/json"),
        serde_json::json!({"id": id, "pin": pin}).to_string(),
    )
    .await
}
async fn listed(f: &Fixture, connection: &Connection) -> Vec<String> {
    let reply = api(f, connection, "GET", "/pairing/requests").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let requests: PairingRequests = serde_json::from_slice(&reply.body).unwrap();
    requests.requests.into_iter().map(|r| r.name).collect()
}
/// Waits for the host's state, and fails rather than hangs if it never comes.
async fn until(what: &str, ready: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::task::yield_now().await;
    }
}
/// A device asking to pair: its certificate request waits for the PIN.
async fn asking(f: &Fixture, name: &str) -> (crypto::Identity, String) {
    let client = Identity::generate().unwrap();
    let request = device_request(&client, name, None);
    let host = f.host.clone();
    tokio::spawn(async move { do_pair(host, &request, PEER.parse().unwrap()).await });
    let id = pairing_id(name, PEER.parse().unwrap());
    until("the device's request", || {
        f.host.pins.lock().unwrap().contains_key(&id)
    })
    .await;
    (client, id)
}
fn saved(f: &Fixture, name: &str) -> Option<Client> {
    f.host
        .paired
        .read()
        .unwrap()
        .clients
        .iter()
        .find(|c| c.name == name)
        .cloned()
}
/// The approval and the new device's handshake with `pin`, as they interleave.
async fn run(f: &Fixture, approver: &Connection, name: &str, typed: &str, shown: &str) -> Reply {
    let (client, id) = asking(f, name).await;
    let (reply, _) = tokio::join!(approve(f, approver, &id, typed), async {
        until("the PIN to reach the handshake", || {
            !f.host.pins.lock().unwrap().contains_key(&id)
                && f.host.pairings.lock().unwrap().sessions.contains_key(&id)
        })
        .await;
        let _ = finish_pairing(f, &client, name, shown).await;
    });
    reply
}

#[path = "pairing/approval_tests.rs"]
mod approval_tests;
#[path = "pairing/limits_and_grants_tests.rs"]
mod limits_and_grants_tests;
