//! Co-op console API tests for sessions, player kicks, settings saves, and Nucleus Co-op folder management.
mod folder_and_games;

use crate::test_support::{request, value};
use axum::http::StatusCode;
use butterpollo_config::config::Config;
use butterpollo_media_policy::rtsp::Negotiated;
use butterpollo_session::{
    nucleus::{
        elevation::{self, Account},
        install::{Problem, Unusable},
    },
    session::Role,
    state::{App, Client},
};
use butterpollo_stream::nucleus::{Admission, Game, Held, fake};
use butterpollo_stream::state::{Session, test_support::Fixture};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

/// A host with Nucleus Co-op (the fake) running a two-player Portal 2
/// session: the fixture's device streams slot 0, a second device has a
/// pending launch for slot 1. Returns the session's id.
struct Coop {
    f: Fixture,
    pc: Arc<fake::Pc>,
    session: u32,
    stream: Arc<Session>,
    pending: String,
}
fn coop() -> Coop {
    let f = Fixture::new();
    let h = &f.host;
    let pc = Arc::new(fake::Pc::installed());
    h.nucleus.use_pc(pc.clone());
    let apps = vec![App {
        name: "Portal 2".into(),
        cmd: format!("\"{}\"", fake::EXE),
        working_dir: String::new(),
        prep: Vec::new(),
        computed_id: None,
        aliases: Vec::new(),
        extra: Default::default(),
    }];
    let entry = h.nucleus.entries(&apps, "").remove(0);
    let first = f.client(0);
    let mut second = first.clone();
    second.uuid = "second-device".into();
    second.name = "Second device".into();
    // Joining launches the co-op entry, so it needs Launch apps too.
    second.perm |= butterpollo_session::state::LAUNCH_APPS;
    second.extra.insert("coop_join".into(), true.into());
    let seated = |client: Client| {
        let mut launch = f.launch(client, Role::Stream);
        let seat = h
            .nucleus
            .admit(Admission {
                launch: &launch.id,
                client: &launch.client,
                game: Game::Coop(&entry),
                size: (1920, 1080),
                config: &Config::default(),
                warnings: Default::default(),
                release_parked: &|| Ok(()),
                instances: None,
            })
            .unwrap();
        launch.nucleus = Some(seat);
        launch.held = Some(Arc::new(Held::new(h, seat, &launch.id)));
        launch
    };
    let streaming = seated(first);
    let session = streaming.nucleus.unwrap().session;
    let stream = Session::new(streaming, Negotiated::default());
    let waiting = seated(second);
    let pending = waiting.id.clone();
    {
        let mut sessions = h.sessions.lock().unwrap();
        sessions
            .active
            .insert(stream.launch.id.clone(), stream.clone());
        sessions.pending.insert(pending.clone(), waiting);
    }
    assert_eq!(h.nucleus.sessions()[0].held, [true, true]);
    Coop {
        f,
        pc,
        session,
        stream,
        pending,
    }
}

#[tokio::test]
async fn the_panel_shows_the_mode_the_install_and_the_sessions() {
    let c = coop();
    let body = value(
        request(&c.f, "GET", "/api/coop", Value::Null).await,
        StatusCode::OK,
    )
    .await;
    for key in ["setting", "mode", "label", "reason"] {
        assert!(body["mode"].get(key).is_some(), "{key}: {body}");
    }
    assert_eq!(
        body["install"],
        json!({"state": "installed", "path": r"C:\Nucleus", "chosen": "", "version": "2.4.2",
               "problem": null, "error": null, "notice": null,
               "release": {"version": "2.4.2", "page": "https://github.com/SplitScreen-Me/splitscreenme-nucleus/releases/tag/v2.4.2"}})
    );
    assert_eq!(
        (&body["available"], &body["reason"]),
        (&json!(true), &Value::Null)
    );
    let session = &body["sessions"][0];
    assert_eq!(session["id"], c.session);
    assert_eq!(session["kind"], "coop");
    assert_eq!(session["title"], "Portal 2");
    assert_eq!(session["game"], fake::GUID);
    assert_eq!(session["phase"], "starting");
    assert_eq!(session["end"], Value::Null);
    assert_eq!(session["kicked"], json!([]));
    assert_eq!(
        session["players"],
        json!([
            {"slot": 0, "device": "parity-device", "name": "Parity device", "away": false, "rejoin_in": null},
            {"slot": 1, "device": "second-device", "name": "Second device", "away": false, "rejoin_in": null},
        ])
    );
    // A device whose stream ended is away and shows how long it may come back.
    let h = &c.f.host;
    let seat = h.sessions.lock().unwrap().pending[&c.pending]
        .nucleus
        .unwrap();
    h.nucleus.leave(seat, &c.pending);
    let body = value(
        request(&c.f, "GET", "/api/coop", Value::Null).await,
        StatusCode::OK,
    )
    .await;
    let away = &body["sessions"][0]["players"][1];
    assert_eq!(away["away"], true);
    let left = away["rejoin_in"].as_u64().unwrap();
    assert!(
        (110..=120).contains(&left),
        "{left} of {}",
        butterpollo_session::nucleus::coop::GRACE.as_secs()
    );
    assert_eq!(body["sessions"][0]["players"][0]["rejoin_in"], Value::Null);
    assert_eq!(body["elevates"], true);
    assert_eq!(body["admin_protection"], false);
    assert_eq!(body["open_refusal"], Value::Null);
    // Unavailable on this host: the reason shows.
    let elevations = || c.pc.elevations.load(std::sync::atomic::Ordering::Relaxed);
    let asked = elevations();
    *c.pc.unavailable.lock().unwrap() = Some("not on this test PC".into());
    let body = value(
        request(&c.f, "GET", "/api/coop", Value::Null).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(body["available"], false);
    assert_eq!(body["reason"], "not on this test PC");
    // A host that runs no sessions doesn't ask whether the account elevates.
    assert_eq!(body["elevates"], Value::Null);
    assert_eq!(body["open_refusal"], Value::Null);
    assert_eq!(elevations(), asked);
}

/// Home polls the sessions alone: the panels' list, without the apps' co-op
/// status or the install.
#[tokio::test]
async fn home_polls_the_sessions_alone() {
    let c = coop();
    let panel = value(
        request(&c.f, "GET", "/api/coop", Value::Null).await,
        StatusCode::OK,
    )
    .await;
    let home = value(
        request(&c.f, "GET", "/api/coop/sessions", Value::Null).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(home, json!({"sessions": panel["sessions"]}));
    assert_eq!(home["sessions"][0]["id"], c.session);
}

#[tokio::test]
async fn kick_ends_that_players_stream_only() {
    let c = coop();
    let h = &c.f.host;
    let body = value(
        request(
            &c.f,
            "POST",
            "/api/coop/kick",
            json!({"session": c.session, "slot": 1}),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(body, json!({"device": "second-device"}));
    assert!(!h.sessions.lock().unwrap().pending.contains_key(&c.pending));
    assert!(!c.stream.stopping());
    let listed = h.nucleus.sessions();
    assert_eq!(listed[0].held, [true, false]);
    assert_eq!(listed[0].session.players()[1].device, None);
    // The panel names the removed device, so the console never invites it back.
    let status = value(
        request(&c.f, "GET", "/api/coop", Value::Null).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(status["sessions"][0]["kicked"], json!(["second-device"]));
    // The slot is empty now; the player streaming slot 0 is kicked next.
    for (session, slot) in [(c.session, 1), (c.session + 1, 0)] {
        value(
            request(
                &c.f,
                "POST",
                "/api/coop/kick",
                json!({"session": session, "slot": slot}),
            )
            .await,
            StatusCode::NOT_FOUND,
        )
        .await;
    }
    value(
        request(
            &c.f,
            "POST",
            "/api/coop/kick",
            json!({"session": c.session, "slot": 0}),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert!(c.stream.stopping());
    assert!(!c.pc.last().stopped());
}

#[tokio::test]
async fn end_stops_nucleus_and_every_players_stream() {
    let c = coop();
    let h = &c.f.host;
    value(
        request(
            &c.f,
            "POST",
            "/api/coop/end",
            json!({"session": c.session + 1}),
        )
        .await,
        StatusCode::NOT_FOUND,
    )
    .await;
    assert!(!c.stream.stopping());
    let body = value(
        request(&c.f, "POST", "/api/coop/end", json!({"session": c.session})).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(body, json!({}));
    assert!(c.pc.last().stopped());
    assert!(c.stream.stopping());
    assert!(h.sessions.lock().unwrap().pending.is_empty());
    let body = value(
        request(&c.f, "GET", "/api/coop", Value::Null).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(body["sessions"][0]["phase"], "ended");
    assert_eq!(
        body["sessions"][0]["end"],
        json!({"reason": "ended", "detail": null})
    );
}

#[tokio::test]
async fn kick_and_end_refuse_a_body_without_the_numbers() {
    let f = Fixture::new();
    for (path, body) in [
        ("/api/coop/kick", json!({"session": 1})),
        ("/api/coop/kick", json!({"session": "1", "slot": 0})),
        ("/api/coop/end", json!({"session": -1})),
        ("/api/coop/nothing", json!({})),
    ] {
        value(
            request(&f, "POST", path, body).await,
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
}

async fn panel(f: &Fixture) -> Value {
    value(
        request(f, "GET", "/api/coop", Value::Null).await,
        StatusCode::OK,
    )
    .await
}

/// `nucleus_path` as an earlier Rubylight kept it, or as written into the
/// settings file by hand: never checked.
fn kept_by_hand(f: &Fixture, folder: &str) {
    super::super::config::save_setting(&f.host, "nucleus_path", |_| Ok(folder.into())).unwrap();
}

#[tokio::test]
async fn a_settings_save_keeps_the_folder_as_the_panels_do() {
    let f = Fixture::new();
    let pc = Arc::new(fake::Pc::installed());
    let why = r"D:\Downloads holds no NucleusCoop.exe: choose the folder you unpacked Nucleus Co-op 2.4.2 into";
    pc.unusable.lock().unwrap().insert(
        r"D:\Downloads".into(),
        Unusable {
            problem: Problem::Missing,
            message: why.into(),
        },
    );
    f.host.nucleus.use_pc(pc.clone());
    let save = |body: Value| request(&f, "PATCH", "/api/config", body);
    // Refused with the reason, and nothing else in the save kept.
    let body = value(
        save(json!({"nucleus_path": r"D:\Downloads", "locale": "de"})).await,
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert_eq!(body["error"], why);
    assert_eq!(kept(&f), "");
    assert_eq!(f.host.config.read().unwrap().get("locale", "en"), "en");
    let body = value(
        save(json!({"nucleus_path": r"D:\Games #2\Nucleus"})).await,
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert!(
        body["error"].as_str().unwrap().contains("# in its path"),
        "{body}"
    );
    assert_eq!(kept(&f), "");
    // A setting the save gets wrong keeps no folder either.
    value(
        save(json!({"nucleus_path": r"D:\Nucleus", "stream_preset": "fastest"})).await,
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert_eq!(kept(&f), "");
    // Checked and kept, with the rest of the save.
    value(
        save(json!({"nucleus_path": r" D:\Nucleus ", "locale": "de"})).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(kept(&f), r"D:\Nucleus");
    assert_eq!(f.host.config.read().unwrap().get("locale", "en"), "de");
    // The same folder again is no change, and a save without it leaves it.
    value(
        save(json!({"nucleus_path": r"D:\Nucleus"})).await,
        StatusCode::OK,
    )
    .await;
    value(save(json!({"locale": "en"})).await, StatusCode::OK).await;
    assert_eq!(kept(&f), r"D:\Nucleus");
    // Emptied, it is forgotten.
    value(save(json!({"nucleus_path": ""})).await, StatusCode::OK).await;
    assert_eq!(kept(&f), "");
}

fn kept(f: &Fixture) -> String {
    f.host
        .config
        .read()
        .unwrap()
        .get("nucleus_path", "")
        .to_string()
}

#[tokio::test]
async fn a_chosen_folder_is_checked_kept_and_remembered_and_nothing_downloads() {
    let f = Fixture::new();
    let pc = Arc::new(fake::Pc::installed());
    *pc.installed.lock().unwrap() = None;
    f.host.nucleus.use_pc(pc.clone());
    let before = panel(&f).await;
    assert_eq!(before["install"]["state"], "missing");
    // The console links to the release's page: Rubylight downloads nothing.
    assert_eq!(
        before["install"]["release"],
        json!({"version": "2.4.2", "page": "https://github.com/SplitScreen-Me/splitscreenme-nucleus/releases/tag/v2.4.2"})
    );
    let chosen = value(
        request(
            &f,
            "POST",
            "/api/coop/folder",
            json!({"path": r" D:\Nucleus "}),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(chosen["state"], "configured");
    assert_eq!(chosen["path"], r"D:\Nucleus");
    assert_eq!(kept(&f), r"D:\Nucleus");
    // Saved with the host's settings, so it outlasts a restart.
    let saved = Config::load(&f.host.config_path).unwrap();
    assert_eq!(saved.get("nucleus_path", ""), r"D:\Nucleus");
    let after = panel(&f).await;
    assert_eq!(after["install"]["state"], "configured");
    assert_eq!(after["install"]["chosen"], r"D:\Nucleus");
    assert_eq!(after["install"]["version"], "2.4.2");
    // An empty folder goes back to an earlier install, here none.
    let cleared = value(
        request(&f, "POST", "/api/coop/folder", json!({"path": ""})).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(cleared["state"], "missing");
    assert_eq!(kept(&f), "");
    // Gone from the settings file too, not just from memory.
    let saved = Config::load(&f.host.config_path).unwrap();
    assert!(!saved.values.contains_key("nucleus_path"), "{saved:?}");
    assert_eq!(panel(&f).await["install"]["state"], "missing");
}

#[tokio::test]
async fn a_folder_is_forgotten_unchecked_even_where_sessions_cant_run() {
    let f = Fixture::new();
    let pc = Arc::new(fake::Pc::installed());
    f.host.nucleus.use_pc(pc.clone());
    let chosen = || {
        request(
            &f,
            "POST",
            "/api/coop/folder",
            json!({"path": r"D:\Nucleus"}),
        )
    };
    value(chosen().await, StatusCode::OK).await;
    assert_eq!(kept(&f), r"D:\Nucleus");
    let put_back = pc.put_back.lock().unwrap().clone();
    // A folder refused now, or a host without the service: forgetting it
    // still works, never a 503.
    pc.unusable.lock().unwrap().insert(
        r"D:\Nucleus".into(),
        Unusable {
            problem: Problem::Untrusted,
            message: r"D:\Nucleus can be changed by S-1-5-11".into(),
        },
    );
    *pc.unavailable.lock().unwrap() = Some("not on this test PC".into());
    let cleared = value(
        request(&f, "POST", "/api/coop/folder", json!({"path": "  "})).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(cleared["state"], "installed");
    assert_eq!(cleared["path"], r"C:\Nucleus");
    assert_eq!(kept(&f), "");
    assert!(
        !Config::load(&f.host.config_path)
            .unwrap()
            .values
            .contains_key("nucleus_path")
    );
    // Nothing was put back into a folder that is refused, or on a host
    // that runs no sessions.
    assert_eq!(*pc.put_back.lock().unwrap(), put_back);
}

#[tokio::test]
async fn a_hash_in_the_folder_is_refused_before_anything_is_checked() {
    let f = Fixture::new();
    let pc = Arc::new(fake::Pc::installed());
    *pc.unavailable.lock().unwrap() = Some("not on this test PC".into());
    f.host.nucleus.use_pc(pc.clone());
    let body = value(
        request(
            &f,
            "POST",
            "/api/coop/folder",
            json!({"path": r"D:\Games #2\Nucleus"}),
        )
        .await,
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert_eq!(
        body["error"],
        "Rubylight can't keep a folder with # in its path: rename the folder, or unpack the release into another one"
    );
    assert_eq!(kept(&f), "");
}

#[tokio::test]
async fn changing_the_folder_puts_back_what_the_one_left_kept() {
    let f = Fixture::new();
    let pc = Arc::new(fake::Pc::installed());
    f.host.nucleus.use_pc(pc.clone());
    let choose =
        |path: &'static str| request(&f, "POST", "/api/coop/folder", json!({ "path": path }));
    // From the earlier install to a folder chosen: the earlier one's
    // journals are put back first.
    value(choose(r"D:\Nucleus").await, StatusCode::OK).await;
    assert_eq!(*pc.put_back.lock().unwrap(), [PathBuf::from(r"C:\Nucleus")]);
    // The same folder again: nothing to put back.
    value(choose(r"D:\Nucleus").await, StatusCode::OK).await;
    assert_eq!(pc.put_back.lock().unwrap().len(), 1);
    // One that can't be put back keeps the folder, and says why.
    let why = r"D:\Nucleus\Rubylight-layers-S-1-5-21-9.json keeps the compatibility settings a Nucleus Co-op session changed for another Windows account: sign in to this PC as that account, so Rubylight puts them back, then choose the folder again";
    *pc.put_back_error.lock().unwrap() = Some(why.into());
    let body = value(choose(r"E:\Nucleus").await, StatusCode::CONFLICT).await;
    assert_eq!(body["error"], why);
    assert_eq!(kept(&f), r"D:\Nucleus");
    *pc.put_back_error.lock().unwrap() = None;
    value(choose("").await, StatusCode::OK).await;
    assert_eq!(
        *pc.put_back.lock().unwrap(),
        [PathBuf::from(r"C:\Nucleus"), PathBuf::from(r"D:\Nucleus")]
    );
    assert_eq!(kept(&f), "");
}
