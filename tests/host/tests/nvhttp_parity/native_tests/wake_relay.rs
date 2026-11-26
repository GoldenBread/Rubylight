//! `GET /wake/relay` and `POST /wake/relay/enroll` against a fake PC (`wake::fake`, Ethernet
//! 192.168.4.20/24): off by default, who may read and enroll, and only from the PC's network.
use super::*;
use butterpollo_stream::wake::fake::Fake;
use rubylight_native_api::wake_relay::{RelayEnrolled, WakeRelayInfo};

fn turn_on(s: &Setup) {
    let mut config = s.host().config.write().unwrap();
    for key in ["internet_play", "away_wake_relay"] {
        config.values.insert(key.into(), "true".into());
    }
}

/// The request came from `peer` on the PC's Ethernet.
fn from(s: &Setup, peer: &str) -> Connection {
    Connection {
        peer: format!("{peer}:50000").parse().unwrap(),
        local: "192.168.4.20:47984".parse().unwrap(),
        ..s.connection.clone()
    }
}

async fn enroll(s: &Setup, connection: &Connection, body: &str) -> Reply {
    send(
        &s.f,
        true,
        connection,
        "POST",
        &format!("{PREFIX}/wake/relay/enroll"),
        Some("application/json"),
        body.to_owned(),
    )
    .await
}

async fn features(s: &Setup) -> Vec<String> {
    let reply = s.get("/capabilities").await;
    serde_json::from_value(reply.json()["features"].clone()).unwrap()
}

#[tokio::test]
async fn off_by_default_and_nothing_is_handed_out() {
    let s = Setup::new();
    Fake::new().on(s.host());
    s.host_control(true);
    assert!(!features(&s).await.iter().any(|f| f == "wake.relay"));
    let info: WakeRelayInfo = serde_json::from_slice(&s.get("/wake/relay").await.body).unwrap();
    assert!(!info.enabled && info.bundle.is_none() && info.relays.is_empty());
    enroll(&s, &from(&s, "192.168.4.50"), r#"{"action":"enroll"}"#)
        .await
        .assert_error(403, "forbidden");
}

#[tokio::test]
async fn a_relay_enrolls_only_with_host_control_from_the_pcs_own_network() {
    let s = Setup::new();
    Fake::new().on(s.host());
    turn_on(&s);
    assert!(features(&s).await.iter().any(|f| f == "wake.relay"));
    let lan = from(&s, "192.168.4.50");
    enroll(&s, &lan, r#"{"action":"enroll"}"#)
        .await
        .assert_error(403, "host_control_required");
    s.host_control(true);
    // Through a tunnel (loopback) or from another network it is refused.
    enroll(&s, &s.connection, r#"{"action":"enroll"}"#)
        .await
        .assert_error(403, "forbidden");
    enroll(&s, &from(&s, "10.1.2.3"), r#"{"action":"enroll"}"#)
        .await
        .assert_error(403, "forbidden");
    // A request that names what to wake is refused before anything happens.
    enroll(
        &s,
        &lan,
        r#"{"action":"enroll","macs":["AA:BB:CC:DD:EE:FF"]}"#,
    )
    .await
    .assert_error(400, "invalid_request");
    let reply = enroll(&s, &lan, r#"{"action":"enroll","name":"NUC"}"#).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let enrolled: RelayEnrolled = serde_json::from_slice(&reply.body).unwrap();
    let assignment = enrolled.assignment.unwrap();
    assert_eq!(assignment.macs[0], "02:7C:16:B4:13:4F");
    assert_eq!(assignment.broadcasts, ["192.168.4.255"]);
    assert_eq!(assignment.address.as_deref(), Some("192.168.4.20"));
    assert_eq!(enrolled.relays[0].name, "NUC");

    // The device reads its keys for that relay.
    let info: WakeRelayInfo = serde_json::from_slice(&s.get("/wake/relay").await.body).unwrap();
    assert_eq!(info.bundle.unwrap().relays.len(), 1);

    // Revoked, it does not come back by refreshing.
    let id = enrolled.relays[0].id.clone();
    let revoke = format!(r#"{{"action":"revoke","relay":"{id}"}}"#);
    assert_eq!(enroll(&s, &lan, &revoke).await.status, StatusCode::OK);
    enroll(&s, &lan, r#"{"action":"refresh"}"#)
        .await
        .assert_error(404, "not_found");
}

#[tokio::test]
async fn reading_needs_launch_or_host_control_and_only_launch_gets_keys() {
    let s = Setup::with(VIEW_BIT);
    Fake::new().on(s.host());
    turn_on(&s);
    s.get("/wake/relay")
        .await
        .assert_error(403, "permission_required");
    s.host_control(true);
    let info: WakeRelayInfo = serde_json::from_slice(&s.get("/wake/relay").await.body).unwrap();
    assert!(info.enabled && info.bundle.is_none());

    let s = Setup::with(LAUNCH);
    Fake::new().on(s.host());
    turn_on(&s);
    let info: WakeRelayInfo = serde_json::from_slice(&s.get("/wake/relay").await.body).unwrap();
    assert!(info.bundle.is_some());
    // Only GET reads, only POST enrolls.
    s.post("/wake/relay").await.assert_error(404, "not_found");
}
