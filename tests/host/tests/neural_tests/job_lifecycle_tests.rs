//! Library sync removal, pending removal queues, job serialization, and stream-lock tests for Neural Rendering.
use super::common::*;
use super::common::{install, remove};
use butterpollo_session::neural::{self, Gpu, MANIFEST, Mode};
use butterpollo_session::state::App;
use butterpollo_stream::neural::folder::Response;
use butterpollo_stream::neural::*;
use serde_json::json;

/// The launcher sync drops an uninstalled game without the console's
/// delete: the mod leaves its folder all the same, and a folder on a drive
/// that is not connected stays a pending removal that is tried again.
#[test]
fn a_game_the_library_sync_drops_loses_the_mod_or_has_its_removal_pending() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let before = names(folder);
    assert!(matches!(
        install(Some(&exe), None, &cache),
        Response::Placed { .. }
    ));
    assert_ne!(names(folder), before);
    let away = unconnected_drive().join(r"Games\Away\Away.exe");
    butterpollo_stream::steam::update_library(h, |apps, _| {
        apps.push(switched_on("Game", "game", &exe));
        apps.push(switched_on("Away", "away", &away));
        apps.push(json!({"name": "Other", "uuid": "other"}));
        true
    })
    .unwrap();

    // The sync drops both games; the other app stays.
    butterpollo_stream::steam::update_library(h, |apps, _| {
        apps.retain(|app| app["uuid"] == "other");
        true
    })
    .unwrap();
    assert_eq!(names(folder), before);
    let away_pending = Pending {
        name: "Away".into(),
        exe: away.clone(),
        uuid: Some("away".into()),
    };
    assert_eq!(pending(h), std::slice::from_ref(&away_pending));
    // Tried again: the drive is still not there.
    assert!(removal_due(h));
    assert!(!removal_due(h), "not again before the retry interval");
    remove_pending(h);
    assert_eq!(pending(h), std::slice::from_ref(&away_pending));

    // A pending folder that can be reached again loses the mod and its record.
    let back = tempdir();
    let back_exe = game(back.path());
    let back_before = names(back_exe.parent().unwrap());
    assert!(matches!(
        install(Some(&back_exe), None, &cache),
        Response::Placed { .. }
    ));
    record_pending(
        h,
        &[Pending {
            name: "Back".into(),
            exe: back_exe.clone(),
            uuid: None,
        }],
    )
    .unwrap();
    remove_pending(h);
    assert_eq!(names(back_exe.parent().unwrap()), back_before);
    assert_eq!(pending(h), [away_pending]);

    // The console's delete of a game on that drive goes through, its removal pending too.
    let mut gone = h.apps.read().unwrap()[0].clone();
    gone.extra
        .insert(KEY.into(), json!({"exe": away.with_file_name("Other.exe")}));
    before_delete(h, &gone).unwrap();
    assert_eq!(pending(h).len(), 2);
}

/// An app deleted or dropped by the library sync while its install runs
/// has no key to remove the mod by: a delete waits for the job, and an
/// install whose app is gone takes the mod out again or leaves its removal
/// pending.
#[test]
fn an_install_whose_app_went_meanwhile_takes_the_mod_out_again() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let before = names(folder);
    let gone = app_named("Gone", "gone");

    start_job(h, "gone", "install", "place").unwrap();
    let Err(error) = before_delete(h, &gone) else {
        panic!("deleted during the install")
    };
    assert!(error.is::<Busy>(), "{error:#}");
    let placed = install(Some(&exe), None, &cache);
    assert!(matches!(placed, Response::Placed { .. }), "{placed:?}");
    let recorded = Pending {
        name: "Gone".into(),
        exe: exe.clone(),
        uuid: Some("gone".into()),
    };
    record_pending(h, &[recorded]).unwrap();
    finish_install(h, "gone", &gone, Some(&exe), Ok(placed));
    assert_eq!(names(folder), before);
    let job = h.neural.lock().jobs["gone"].clone();
    assert_eq!(job.phase, "failed");
    assert!(job.error.unwrap().contains("taken out again"));
    assert!(pending(h).is_empty());
    before_delete(h, &gone).unwrap();

    // A folder that can't be reached now: its removal is pending.
    let away = unconnected_drive().join(r"Games\Away\Away.exe");
    start_job(h, "gone", "install", "place").unwrap();
    finish_install(
        h,
        "gone",
        &gone,
        None,
        Ok(Response::Placed {
            exe: away.clone(),
            release: "Alpha0.3.5.8".into(),
            proxy: "dxgi.dll".into(),
            updated: false,
            mode: Mode::Full,
        }),
    );
    let job = h.neural.lock().jobs["gone"].clone();
    assert!(job.error.unwrap().contains("pending"));
    assert_eq!(
        pending(h),
        [Pending {
            name: "Gone".into(),
            exe: away,
            uuid: Some("gone".into()),
        }]
    );
}

/// A first install records its removal before anything is written, so one
/// cut short is taken out by the pending removals; one that ends well
/// leaves no record.
#[tokio::test(flavor = "multi_thread")]
async fn a_first_install_cut_short_is_taken_out_by_the_pending_removals() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    *h.neural.probe.lock().unwrap() = Some(Probe {
        gpus: vec![Gpu {
            name: "AMD Radeon RX 7900 XT".into(),
            arch: "gfx1100".into(),
            compute_units: 84,
            luid: None,
        }],
        error: None,
    });
    accept(h).unwrap();
    butterpollo_stream::steam::update_library(h, |apps, _| {
        apps.push(json!({"name": "Game", "uuid": "game"}));
        true
    })
    .unwrap();
    let temp = tempdir();
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let before = names(folder);
    let cache = release(&cache_root(h), "Alpha0.3.5.8", b"OptiScaler proxy");
    pin_to(h, &cache);

    enable(h, "game", Some(exe.clone())).unwrap();
    let job = ended(h, "game").await;
    assert_eq!(job.phase, "done", "{job:?}");
    assert!(pending(h).is_empty());
    // A second install, as a console that still shows the switch off asks for, would
    // leave the first one's files without a record: it is refused.
    let other = temp.path().join("other").join("Other.exe");
    assert!(matches!(
        enable(h, "game", Some(other)),
        Err(Refusal::Unavailable(reason)) if reason == ALREADY_ON
    ));
    assert_eq!(h.neural.lock().jobs["game"].phase, "done");
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
    set_key(h, "game", KEY, None).unwrap();

    // The cache, checked once at this host start, lost a file since: the
    // install fails partway.
    std::fs::remove_file(cache.join(format!("files/full/game/{PAK}"))).unwrap();
    enable(h, "game", Some(exe.clone())).unwrap();
    let job = ended(h, "game").await;
    assert_eq!(job.phase, "failed", "{job:?}");
    assert_eq!(
        pending(h),
        [Pending {
            name: "Game".into(),
            exe: exe.clone(),
            uuid: Some("game".into()),
        }]
    );
    remove_pending(h);
    assert_eq!(names(folder), before);
    assert!(pending(h).is_empty());
    // The install's failure still shows, not the removal behind it.
    let now = h.neural.lock().jobs["game"].clone();
    assert_eq!((now.phase, now.error), (job.phase, job.error));
}

/// A delete holds the switch's slot until the app is gone, so no launch's
/// update writes the folder meanwhile; a pending removal waits while the
/// switch is busy with its game.
#[test]
fn a_removal_holds_the_switch_until_the_app_is_gone() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let before = names(folder);
    let placed = || {
        let placed = install(Some(&exe), None, &cache);
        assert!(matches!(placed, Response::Placed { .. }), "{placed:?}");
    };
    placed();
    let app: App = serde_json::from_value(switched_on("Game", "game", &exe)).unwrap();
    let removing = before_delete(h, &app).unwrap();
    assert_eq!(names(folder), before);
    let error = format!("{:#}", before_launch(h, &app).unwrap_err());
    assert!(error.contains("busy"), "{error}");
    drop(removing);
    assert_eq!(h.neural.lock().jobs["game"].phase, "done");

    placed();
    record_pending(h, &Pending::of(&app)).unwrap();
    start_job(h, "game", "update", "place").unwrap();
    remove_pending(h);
    assert_ne!(names(folder), before);
    assert_eq!(pending(h).len(), 1);
    update_job(h, "game", |job| job.phase = "done");
    remove_pending(h);
    assert_eq!(names(folder), before);
    assert!(pending(h).is_empty());
}

/// A delete while a stream runs is refused before anything is touched: the
/// streamed game may hold the provider's DLL loaded, so the removal would
/// put the ini back and stop at the DLL. Once the stream ends it goes.
#[test]
fn a_delete_while_a_stream_runs_leaves_the_folder_untouched() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let before = names(folder);
    let placed = install(Some(&exe), None, &cache);
    assert!(matches!(placed, Response::Placed { .. }), "{placed:?}");
    let with_mod = snapshot(folder);
    let app: App = serde_json::from_value(switched_on("Game", "game", &exe)).unwrap();
    f.session(
        f.client(u32::MAX),
        butterpollo_session::session::Role::Stream,
    );
    let error = format!("{:#}", reserve(h, &app).err().unwrap());
    assert!(
        error.contains("remove it when no stream is running"),
        "{error}"
    );
    assert_eq!(snapshot(folder), with_mod);
    assert!(!h.neural.lock().jobs.contains_key("game"));
    // An app without files is no risk: its delete goes on.
    let plain: App = serde_json::from_value(json!({"name": "Plain", "uuid": "plain"})).unwrap();
    drop(reserve(h, &plain).unwrap());
    h.sessions.lock().unwrap().active.clear();
    drop(before_delete(h, &app).unwrap());
    assert_eq!(names(folder), before);
}

/// A launch's check and update hold the switch's slot, so a disable or a
/// delete can't race them, and give it back however they end.
#[test]
fn a_launch_holds_the_switch_while_it_checks() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let exe = game(temp.path());
    assert!(matches!(
        install(Some(&exe), None, &cache),
        Response::Placed { .. }
    ));
    let mut app = app_named("Game", "game");
    app.extra.insert(KEY.into(), json!({"exe": exe}));
    before_launch(h, &app).unwrap();
    let job = h.neural.lock().jobs["game"].clone();
    assert_eq!((job.action, job.phase), ("update", "done"));
    std::fs::remove_file(exe.with_file_name(MANIFEST)).unwrap();
    assert!(before_launch(h, &app).is_err());
    assert_eq!(h.neural.lock().jobs["game"].phase, "failed");
    start_job(h, "game", "remove", "remove").unwrap();
    let error = before_launch(h, &app).unwrap_err().to_string();
    assert!(error.contains("busy"), "{error}");
    assert_eq!(h.neural.lock().jobs["game"].action, "remove");
}

/// A launch's failed check and an app delete's failed removal reach the
/// client and the console without the provider's folder, whose name is the
/// mod's.
#[test]
fn launch_and_delete_errors_name_no_provider_folder() {
    use std::os::windows::fs::OpenOptionsExt as _;
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    let temp = tempdir();
    let cache = release(temp.path(), "FG-V.B16", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap().to_path_buf();
    assert!(matches!(
        install(Some(&exe), None, &cache),
        Response::Placed { .. }
    ));
    let mut app = app_named("Game", "game");
    app.extra.insert(KEY.into(), json!({"exe": exe}));

    std::fs::remove_file(folder.join(PDLL)).unwrap();
    let error = format!("{:#}", before_launch(h, &app).unwrap_err());
    assert!(error.contains("nvngx_dlssg.dll"), "{error}");
    assert!(!error.contains(neural::PROVIDER), "{error}");
    let job = h.neural.lock().jobs["game"].clone();
    assert!(!job.error.unwrap().contains(neural::PROVIDER));
    update_job(h, "game", |job| job.phase = "done");

    // A placed file another program holds, sharing it for reading only, is
    // still the one placed but can't be deleted.
    std::fs::write(folder.join(PDLL), b"provider").unwrap();
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1) // FILE_SHARE_READ
        .open(folder.join(PAK))
        .unwrap();
    let error = format!("{:#}", before_delete(h, &app).err().unwrap());
    drop(held);
    assert!(error.contains("pack.nrx"), "{error}");
    assert!(!error.contains(neural::PROVIDER), "{error}");
    let job = h.neural.lock().jobs["game"].clone();
    assert!(!job.error.unwrap_or_default().contains(neural::PROVIDER));
}

#[test]
fn the_console_cannot_set_the_hosts_key() {
    let app = |exe: &str| {
        let mut app = app_named("Game", "game");
        app.extra = [(KEY.to_string(), json!({"exe": exe}))]
            .into_iter()
            .collect();
        app
    };
    let stored = app(r"C:\Games\Game.exe");
    let mut saved = app(r"C:\Windows\System32\x.exe");
    keep_key(Some(&stored), &mut saved);
    assert_eq!(saved.extra[KEY]["exe"], r"C:\Games\Game.exe");
    let mut new = app(r"C:\x.exe");
    keep_key(None, &mut new);
    assert!(!new.extra.contains_key(KEY));
}
