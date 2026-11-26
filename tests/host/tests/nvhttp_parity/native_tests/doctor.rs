//! `GET /stream/doctor` and `POST /stream/doctor/apply` (Stream Doctor 2.0)
//! through the router: the snapshot, a bitrate step-down and its undo, and
//! every refusal.
use super::*;
use butterpollo_session::multiclient::LAUNCH_OPTION;
use rubylight_native_api::doctor::{DoctorSnapshot, RemedyId};
use std::sync::atomic::Ordering;

async fn apply(s: &Setup, connection: &Connection, body: &str) -> Reply {
    send(
        &s.f,
        true,
        connection,
        "POST",
        &format!("{PREFIX}/stream/doctor/apply"),
        Some("application/json"),
        body.as_bytes().to_vec(),
    )
    .await
}

fn snapshot(reply: &Reply) -> DoctorSnapshot {
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.headers[header::CACHE_CONTROL], "no-store");
    serde_json::from_slice(&reply.body).unwrap()
}

const STEP_DOWN: &str = r#"{"action":"apply","remedy":"bitrate_stepdown"}"#;

#[tokio::test]
async fn the_doctor_is_advertised_and_reads_an_idle_host() {
    let s = Setup::new();
    let caps: Capabilities = serde_json::from_slice(&s.get("/capabilities").await.body).unwrap();
    assert!(caps.features.iter().any(|f| f == "stream.doctor_v2"));
    let idle = snapshot(&s.get("/stream/doctor").await);
    assert!(!idle.session_active && idle.encoder.is_none());
    assert!(idle.eligible_remedies.is_empty());
    let lister = Setup::with(LIST);
    let caps: Capabilities =
        serde_json::from_slice(&lister.get("/capabilities").await.body).unwrap();
    assert!(!caps.features.iter().any(|f| f == "stream.doctor_v2"));
    lister
        .get("/stream/doctor")
        .await
        .assert_error(403, "permission_required");
}

#[tokio::test]
async fn the_owner_steps_the_bitrate_down_and_undoes_it() {
    let s = Setup::new();
    let session = s.streaming();
    session.set_client_encoder_bitrate(45_000);
    let read = snapshot(&s.get("/stream/doctor").await);
    assert!(read.session_active && read.caller_is_owner);
    assert_eq!(read.encoder.as_ref().unwrap().live_bitrate_kbps, 45_000);
    let offer = &read.eligible_remedies[0];
    assert_eq!(
        (offer.id, offer.target_kbps),
        (RemedyId::BitrateStepdown, Some(36_000))
    );
    let applied = snapshot(&apply(&s, &s.connection, STEP_DOWN).await);
    assert_eq!(session.bitrate.load(Ordering::Acquire), 36_000);
    let trial = applied.active_trials[0].clone();
    assert!(trial.can_undo);
    assert_eq!(
        (trial.applied_value, trial.previous_value),
        (36_000, 45_000)
    );
    apply(&s, &s.connection, STEP_DOWN)
        .await
        .assert_error(422, "not_writable");
    let undo = format!(
        r#"{{"action":"undo","remedy":"bitrate_stepdown","trial_id":"{}"}}"#,
        trial.trial_id
    );
    let undone = snapshot(&apply(&s, &s.connection, &undo).await);
    assert_eq!(session.bitrate.load(Ordering::Acquire), 45_000);
    assert!(undone.active_trials.is_empty());
    apply(&s, &s.connection, &undo)
        .await
        .assert_error(422, "invalid_value");
}

#[tokio::test]
async fn a_change_since_the_trial_keeps_its_bitrate() {
    let s = Setup::new();
    let session = s.streaming();
    session.set_client_encoder_bitrate(20_000);
    let applied = snapshot(&apply(&s, &s.connection, STEP_DOWN).await);
    let id = &applied.active_trials[0].trial_id;
    // The player's own bitrate change.
    session.set_client_encoder_bitrate(12_000);
    let read = snapshot(&s.get("/stream/doctor").await);
    assert!(!read.active_trials[0].can_undo);
    let undo = format!(r#"{{"action":"undo","remedy":"bitrate_stepdown","trial_id":"{id}"}}"#);
    let reply = apply(&s, &s.connection, &undo).await;
    reply.assert_error(422, "not_writable");
    assert!(
        reply.text().contains("keep that change"),
        "{}",
        reply.text()
    );
    assert_eq!(session.bitrate.load(Ordering::Acquire), 12_000);
}

#[tokio::test]
async fn guests_bodies_and_permissions_are_refused() {
    let s = Setup::new();
    apply(&s, &s.connection, STEP_DOWN)
        .await
        .assert_error(409, "no_session");
    for body in [
        "",
        "{}",
        r#"{"action":"apply","remedy":"bitrate_stepdown","percent":5}"#,
    ] {
        apply(&s, &s.connection, body)
            .await
            .assert_error(400, "invalid_request");
    }
    // The caller plays along as a guest in another device's game.
    let mut launch = s.f.launch(s.client.clone(), Role::Stream);
    launch.options.insert(LAUNCH_OPTION.into(), "7".into());
    let guest = Session::new(launch, Default::default());
    guest.set_client_encoder_bitrate(20_000);
    s.host()
        .sessions
        .lock()
        .unwrap()
        .active
        .insert(guest.launch.id.clone(), guest.clone());
    let read = snapshot(&s.get("/stream/doctor").await);
    assert!(read.session_active && !read.caller_is_owner);
    assert_eq!(
        read.eligible_remedies[0].reason.as_deref(),
        Some("not_owner")
    );
    apply(&s, &s.connection, STEP_DOWN)
        .await
        .assert_error(409, "not_session_owner");
    apply(
        &s,
        &s.connection,
        r#"{"action":"apply","remedy":"fec_auto"}"#,
    )
    .await
    .assert_error(422, "invalid_value");
    assert_eq!(guest.bitrate.load(Ordering::Acquire), 20_000);
    let viewer = Setup::with(LIST | VIEW_BIT);
    apply(&viewer, &viewer.connection, STEP_DOWN)
        .await
        .assert_error(403, "permission_required");
}
