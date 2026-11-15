//! `GET /neural` and `POST /neural/settings` (`docs/native-api.md`, "Neural
//! Rendering") through the router, against the host's own Neural Rendering
//! with a test GPU. Nothing here downloads or touches a game.
use super::*;
use butterpollo_core::native_api::{
    capabilities::Capabilities as Caps,
    feature,
    neural::{FRAME_GENERATION, Neural},
    radeon::ControlState,
};
use butterpollo_core::neural::{AMD, Adapter, NVIDIA};
use serde_json::json;

/// The host streams with `name`; `game` (and `old`, with Rubylight 3.0's
/// upscaling) are in its library.
fn host(s: &Setup, name: &str, vendor: u32, device: u32) {
    *s.host().neural.streams.lock().unwrap() = Some(Adapter {
        name: name.into(),
        luid: 1,
        vendor,
        device,
    });
    crate::steam::update_library(s.host(), |apps, _| {
        apps.push(json!({"name": "Game", "uuid": "game"}));
        apps.push(json!({"name": "Old", "uuid": "old",
            "neural-rendering": {"exe": r"C:\Games\Old\Old.exe", "release": "Alpha0.3.5.8R2"}}));
        true
    })
    .unwrap();
}

async fn write(s: &Setup, connection: &Connection, body: &str) -> Reply {
    send(
        &s.f,
        true,
        connection,
        "POST",
        &format!("{PREFIX}/neural/settings"),
        Some("application/json"),
        body,
    )
    .await
}

fn neural(reply: &Reply) -> Neural {
    assert!(reply.status.is_success(), "{}", reply.text());
    assert_eq!(reply.headers[header::CACHE_CONTROL], "no-store");
    serde_json::from_slice(&reply.body).unwrap()
}

#[tokio::test]
async fn the_switch_shows_what_this_pc_and_the_game_allow() {
    let s = Setup::new();
    host(&s, "NVIDIA GeForce GTX 1080", NVIDIA, 0x1B80);
    let state = neural(&s.get("/neural?app=game").await);
    assert_eq!(state.app, "game");
    assert!(!state.accepted && !state.replaces_earlier_upscaling);
    assert_eq!(state.gpu, None);
    let switch = &state.switches[0];
    assert_eq!(
        (switch.id.as_str(), switch.state, switch.on),
        (FRAME_GENERATION, ControlState::Unsupported, false)
    );
    assert!(
        switch.reason.as_deref().unwrap().contains("which is older"),
        "{switch:?}"
    );

    let s = Setup::new();
    host(&s, "AMD Radeon 780M Graphics", AMD, 0x1900);
    let state = neural(&s.get("/neural?app=old").await);
    assert_eq!(state.gpu.as_deref(), Some("AMD Radeon 780M Graphics"));
    assert!(state.replaces_earlier_upscaling);
    // A device without Host control sees it read-only, and why, so it never
    // shows the notice for a write it would be refused.
    let switch = &state.switches[0];
    assert_eq!(switch.state, ControlState::ReadOnly);
    assert!(
        switch.reason.as_deref().unwrap().contains("Host control"),
        "{switch:?}"
    );
    s.host_control(true);
    let state = neural(&s.get("/neural?app=old").await);
    assert_eq!(state.switches[0].state, ControlState::Configurable);
    assert_eq!(state.switches[0].reason, None);
    // While a stream runs it can't be turned, on or off.
    s.streaming();
    let state = neural(&s.get("/neural?app=game").await);
    assert_eq!(
        state.switches[0].state,
        ControlState::TemporarilyUnavailable
    );
    write(
        &s,
        &s.connection,
        r#"{"app":"game","switch":"frame_generation","on":true,"accept_notice":true}"#,
    )
    .await
    .assert_error(409, "host_in_use");
    let reply = write(
        &s,
        &s.connection,
        r#"{"app":"game","switch":"frame_generation","on":false}"#,
    )
    .await;
    reply.assert_error(409, "host_in_use");
    assert!(
        reply.json()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("turn it off"),
        "{}",
        reply.text()
    );
    assert!(crate::neural::status(s.host(), "game")["job"].is_null());

    s.get("/neural?app=missing")
        .await
        .assert_error(404, "not_found");
    s.get("/neural").await.assert_error(400, "invalid_request");
}

#[tokio::test]
async fn reading_needs_view_and_a_write_host_control_too() {
    let s = Setup::with(LAUNCH);
    host(&s, "AMD Radeon 780M Graphics", AMD, 0x1900);
    s.get("/neural?app=game")
        .await
        .assert_error(403, "permission_required");
    let s = Setup::with(VIEW_BIT);
    host(&s, "AMD Radeon 780M Graphics", AMD, 0x1900);
    assert_eq!(s.get("/neural?app=game").await.status, StatusCode::OK);
    let body = r#"{"app":"game","switch":"frame_generation","on":false}"#;
    write(&s, &s.connection, body)
        .await
        .assert_error(403, "host_control_required");
    s.host_control(true);
    assert_eq!(
        write(&s, &s.connection, body).await.status,
        StatusCode::ACCEPTED
    );
}

#[tokio::test]
async fn a_write_names_no_file_and_needs_the_notice() {
    let s = Setup::new();
    s.host_control(true);
    host(&s, "AMD Radeon 780M Graphics", AMD, 0x1900);
    // A device never names a program on the PC.
    write(
        &s,
        &s.connection,
        r#"{"app":"game","switch":"frame_generation","on":true,"exe":"C:\x.exe"}"#,
    )
    .await
    .assert_error(400, "invalid_request");
    let reply = write(
        &s,
        &s.connection,
        r#"{"app":"game","switch":"upscaling","on":true}"#,
    )
    .await;
    reply.assert_error(422, "invalid_value");
    assert_eq!(reply.json()["error"]["setting"], "upscaling");
    write(
        &s,
        &s.connection,
        r#"{"app":"missing","switch":"frame_generation","on":true}"#,
    )
    .await
    .assert_error(404, "not_found");
    // The notice first.
    let reply = write(
        &s,
        &s.connection,
        r#"{"app":"game","switch":"frame_generation","on":true}"#,
    )
    .await;
    reply.assert_error(422, "not_writable");
    assert!(
        reply.json()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("accept_notice")
    );
    assert!(!crate::neural::accepted(s.host()));
    // Accepted with the write: the host can't find the game's program, so the
    // switch says to choose it in the console.
    let reply = write(
        &s,
        &s.connection,
        r#"{"app":"game","switch":"frame_generation","on":true,"accept_notice":true}"#,
    )
    .await;
    assert_eq!(reply.status, StatusCode::ACCEPTED, "{}", reply.text());
    assert!(crate::neural::accepted(s.host()));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let state = loop {
        let state = neural(&s.get("/neural?app=game").await);
        if state.switches[0].state != ControlState::TemporarilyUnavailable
            || std::time::Instant::now() > deadline
        {
            break state;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    let switch = &state.switches[0];
    assert_eq!(switch.state, ControlState::NeedsExternalSetup, "{state:?}");
    assert!(
        switch.reason.as_deref().unwrap().contains("console"),
        "{switch:?}"
    );
    assert!(!switch.on);
}

/// Listed where the GPU a stream uses is AMD's or NVIDIA's (an older one
/// too: the switch says why), left out for any other.
#[tokio::test]
async fn the_capability_is_listed_for_amd_and_nvidia_only() {
    for (vendor, device, listed) in [
        (AMD, 0x1900, true),
        (NVIDIA, 0x1B80, true),
        (0x8086, 0x56a0, false),
    ] {
        let s = Setup::with(VIEW_BIT);
        host(&s, "GPU", vendor, device);
        let caps: Caps = serde_json::from_slice(&s.get("/capabilities").await.body).unwrap();
        assert_eq!(
            caps.features
                .iter()
                .any(|f| f == feature::NEURAL_FRAME_GENERATION),
            listed,
            "{vendor:#x}"
        );
    }
}

/// The id a client sends is the `<UUID>` `/applist` gives the game (Android's
/// `NvApp.getAppUuid()`, desktop's `install_id`): the app's `extra["uuid"]`.
#[tokio::test]
async fn the_applist_uuid_opens_the_games_switch() {
    let s = Setup::new();
    host(&s, "AMD Radeon 780M Graphics", AMD, 0x1900);
    let applist = send(&s.f, true, &s.connection, "GET", "/applist", None, vec![])
        .await
        .text();
    let game = applist
        .split("<App>")
        .find(|app| app.contains("<AppTitle>Game</AppTitle>"))
        .unwrap_or_else(|| panic!("{applist}"));
    let uuid = game
        .split("<UUID>")
        .nth(1)
        .and_then(|rest| rest.split("</UUID>").next())
        .unwrap();
    assert_eq!(uuid, "game");
    let state = neural(&s.get(&format!("/neural?app={uuid}")).await);
    assert_eq!(state.app, uuid);
}

/// A game that can't have frame generation says so before any write, as a
/// code the device words itself (`why`) and in the host's words: no DLSS
/// frame generation, an anti-cheat, or a program the host can't tell. One
/// that fits stays configurable. A switch that is on is never held back by
/// the game's check, and names how and which release is in the folder.
#[tokio::test]
async fn the_game_is_checked_before_any_write_and_each_state_has_a_code() {
    use butterpollo_core::native_api::neural::{Mode, Why};
    let s = Setup::new();
    host(&s, "AMD Radeon 780M Graphics", AMD, 0x1900);
    s.host_control(true);
    let root =
        std::env::temp_dir().join(format!("rubylight-native-neural-{}", uuid::Uuid::new_v4()));
    let install = |name: &str, files: &[&str]| {
        let folder = root.join(name);
        std::fs::create_dir_all(&folder).unwrap();
        for file in files {
            std::fs::write(folder.join(file), "x").unwrap();
        }
        folder
    };
    let fits = install("fits", &["Game.exe", "nvngx_dlssg.dll"]);
    let without = install("without", &["Game.exe"]);
    let guarded = install(
        "guarded",
        &["Game.exe", "nvngx_dlssg.dll", "EasyAntiCheat_x64.dll"],
    );
    crate::steam::update_library(s.host(), |apps, _| {
        for (uuid, folder) in [("fits", &fits), ("without", &without), ("guarded", &guarded)] {
            apps.push(json!({"name": uuid, "uuid": uuid, "launcher-install-dir": folder}));
        }
        apps.push(json!({"name": "On", "uuid": "on", "launcher-install-dir": without,
            "neural-anr": {"exe": without.join("Game.exe"), "release": "FG-V.B16", "proxy": "dxgi.dll", "mode": "addon"}}));
        true
    })
    .unwrap();
    let switch = |state: &Neural| state.switches[0].clone();
    let fits_state = neural(&s.get("/neural?app=fits").await);
    assert_eq!(switch(&fits_state).state, ControlState::Configurable);
    assert_eq!(
        (
            switch(&fits_state).why,
            fits_state.mode,
            fits_state.release.clone()
        ),
        (None, None, None)
    );
    let without_state = switch(&neural(&s.get("/neural?app=without").await));
    assert_eq!(
        (without_state.state, without_state.why),
        (ControlState::Unsupported, Some(Why::NoFrameGeneration))
    );
    assert!(
        without_state
            .reason
            .as_deref()
            .unwrap()
            .contains("DLSS frame generation")
    );
    let guarded_state = switch(&neural(&s.get("/neural?app=guarded").await));
    assert_eq!(
        (guarded_state.state, guarded_state.why),
        (ControlState::Unsupported, Some(Why::AntiCheat))
    );
    assert!(
        guarded_state
            .reason
            .as_deref()
            .unwrap()
            .contains("Easy Anti-Cheat")
    );
    // A hand-added app whose program the host doesn't know.
    let unknown = switch(&neural(&s.get("/neural?app=game").await));
    assert_eq!(
        (unknown.state, unknown.why),
        (ControlState::NeedsExternalSetup, Some(Why::ChooseProgram))
    );
    // On: no check holds it back, and the answer says how it is in.
    let on = neural(&s.get("/neural?app=on").await);
    assert_eq!(
        (switch(&on).state, switch(&on).on),
        (ControlState::Configurable, true)
    );
    assert_eq!(
        (on.mode, on.release.as_deref()),
        (Some(Mode::Addon), Some("FG-V.B16"))
    );
    // The other states' codes.
    s.host_control(false);
    assert_eq!(
        switch(&neural(&s.get("/neural?app=fits").await)).why,
        Some(Why::NoHostControl)
    );
    s.host_control(true);
    let _stream = s.streaming();
    assert_eq!(
        switch(&neural(&s.get("/neural?app=fits").await)).why,
        Some(Why::Streaming)
    );
    let s = Setup::new();
    host(&s, "NVIDIA GeForce GTX 1080", NVIDIA, 0x1B80);
    assert_eq!(
        switch(&neural(&s.get("/neural?app=game").await)).why,
        Some(Why::Gpu)
    );
    let _ = std::fs::remove_dir_all(&root);
}
