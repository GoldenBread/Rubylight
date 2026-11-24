//! Migration, refusal preservation, and removal tests for earlier Rubylight 3.0 AMDNR upscaling installs.
use super::common::*;
use super::common::{install, remove};
use butterpollo_session::neural::{Gpu, LEGACY_MANIFEST, MANIFEST, Manifest};
use butterpollo_stream::neural::folder::{Request, Response, Writer, handle};
use butterpollo_stream::neural::*;
use serde_json::json;
use std::path::{Path, PathBuf};

/// Rubylight 3.0's AMDNR in `folder`, its files and manifest as it wrote
/// them, under `proxy`.
fn amdnr(folder: &Path, proxy: &str) {
    let files: [(&str, &[u8]); 5] = [
        (proxy, b"AMDNR's OptiScaler"),
        ("OptiScaler.ini", b"[Upscalers]\n"),
        ("LmxxfNrRuntime.dll", b"runtime"),
        ("LmxxfNrRuntime.pak", b"model"),
        ("OptiScaler/libxess.dll", b"xess"),
    ];
    std::fs::create_dir_all(folder.join("OptiScaler")).unwrap();
    let mut listed = Vec::new();
    for (name, bytes) in files {
        std::fs::write(folder.join(name), bytes).unwrap();
        listed.push(format!(
            r#""{name}": {{"sha256": "{}", "size": {}}}"#,
            sha(bytes),
            bytes.len()
        ));
    }
    let manifest = format!(
        r#"{{
  "by": "Rubylight",
  "release": "Alpha0.3.5.8R2",
  "proxy": "{proxy}",
  "files": {{ {} }},
  "folders": ["OptiScaler"]
}}"#,
        listed.join(", ")
    );
    std::fs::write(folder.join(LEGACY_MANIFEST), manifest).unwrap();
}

/// Rubylight 3.0's AMDNR is still checked before a launch and removed
/// byte for byte, the user's changed settings and logs with it; frame
/// generation is refused while it is there.
#[test]
fn an_earlier_amdnr_install_is_still_removed_exactly() {
    let temp = tempdir();
    let cache = release(temp.path(), "FG-V.B16", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let before = snapshot(folder);
    amdnr(folder, "winmm.dll");
    assert!(matches!(
        prepare(&exe, None),
        Response::Placed { updated: false, .. }
    ));
    let Response::Refused { reason } = install(Some(&exe), None, &cache) else {
        panic!()
    };
    assert!(
        reason.contains("earlier Neural Rendering upscaling"),
        "{reason}"
    );
    std::fs::write(
        folder.join("OptiScaler.ini"),
        "[Upscalers]\nDx12Upscaler=fsr31\n",
    )
    .unwrap();
    std::fs::write(folder.join("OptiScaler.log"), "log").unwrap();
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
    assert_eq!(snapshot(folder), before);
}

/// An upgrade changes no game folder: the host's own AMDNR download goes at
/// start, the game keeps AMDNR and its app the record of it, and the
/// console's page offers its removal.
#[test]
fn an_upgrade_touches_no_game_folder() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    let temp = tempdir();
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    amdnr(folder, "winmm.dll");
    let installed = snapshot(folder);
    butterpollo_stream::steam::update_library(h, |apps, _| {
        apps.push(json!({
            "name": "Game",
            "uuid": "game",
            LEGACY_KEY: {"exe": exe, "release": "Alpha0.3.5.8R2", "proxy": "winmm.dll"},
        }));
        true
    })
    .unwrap();
    let legacy = legacy_cache_root(h);
    std::fs::create_dir_all(legacy.join("Alpha0.3.5.8R2/files")).unwrap();
    std::fs::write(
        legacy.join("Alpha0.3.5.8R2/files/LmxxfNrRuntime.pak"),
        "model",
    )
    .unwrap();
    let ours = release(&cache_root(h), "FG-V.B16", b"OptiScaler proxy");

    clear_legacy_cache(h);
    assert!(!legacy.exists());
    assert!(ours.exists());
    assert_eq!(snapshot(folder), installed);
    let app = app(h, "game").unwrap();
    assert_eq!(legacy_exe(&app), Some(exe.clone()));
    assert_eq!(placed_exes(&app), std::slice::from_ref(&exe));
    let status = status(h, "game");
    assert_eq!(status["enabled"], false);
    assert_eq!(status["earlier_upscaling"]["exe"], json!(exe));
    assert_eq!(status["earlier_upscaling"]["release"], "Alpha0.3.5.8R2");
    // Again: nothing there, nothing to do.
    clear_legacy_cache(h);
    assert_eq!(snapshot(folder), installed);
}

/// Turning frame generation on for a game with Rubylight 3.0's AMDNR takes
/// AMDNR out first, in the same job, then puts frame generation in.
#[tokio::test(flavor = "multi_thread")]
async fn frame_generation_replaces_an_earlier_amdnr_install() {
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
    let temp = tempdir();
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let before = snapshot(folder);
    amdnr(folder, "winmm.dll");
    butterpollo_stream::steam::update_library(h, |apps, _| {
        apps.push(json!({
            "name": "Game",
            "uuid": "game",
            LEGACY_KEY: {"exe": exe, "release": "Alpha0.3.5.8R2", "proxy": "winmm.dll"},
        }));
        true
    })
    .unwrap();
    let cache = release(&cache_root(h), "FG-V.B16", b"OptiScaler proxy");
    pin_to(h, &cache);

    // A game that gained anti-cheat keeps AMDNR and is refused.
    let with_amdnr = snapshot(folder);
    std::fs::write(folder.join("BEClient_x64.dll"), "").unwrap();
    enable(h, "game", None).unwrap();
    let job = ended(h, "game").await;
    assert_eq!(job.phase, "failed", "{job:?}");
    assert!(
        job.error.as_deref().unwrap().contains("BattlEye"),
        "{job:?}"
    );
    std::fs::remove_file(folder.join("BEClient_x64.dll")).unwrap();
    assert_eq!(snapshot(folder), with_amdnr);
    assert!(legacy_exe(&app(h, "game").unwrap()).is_some());

    // The program comes from the earlier install's record.
    enable(h, "game", None).unwrap();
    let job = ended(h, "game").await;
    assert_eq!(job.phase, "done", "{job:?}");
    assert!(!folder.join(LEGACY_MANIFEST).exists());
    assert!(!folder.join("LmxxfNrRuntime.pak").exists());
    assert!(folder.join(MANIFEST).exists());
    assert_eq!(
        std::fs::read(folder.join("winmm.dll")).unwrap(),
        b"OptiScaler proxy"
    );
    let app = app(h, "game").unwrap();
    assert_eq!(placed_exe(&app), Some(exe.clone()));
    assert_eq!(legacy_exe(&app), None);
    assert!(status(h, "game")["earlier_upscaling"].is_null());
    disable(h, "game").unwrap();
    let job = ended(h, "game").await;
    assert_eq!(job.phase, "done", "{job:?}");
    assert_eq!(snapshot(folder), before);
}

/// A host that turns frame generation on for `games`, each with Rubylight
/// 3.0's AMDNR under the proxy given, from a release that also places
/// `amd_fidelityfx_dx12.dll` beside the game.
fn amdnr_host(f: &butterpollo_stream::state::test_support::Fixture, games: &[(&str, &Path, &str)]) {
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
    for (uuid, exe, proxy) in games {
        amdnr(exe.parent().unwrap(), proxy);
        butterpollo_stream::steam::update_library(h, |apps, _| {
            apps.push(json!({
                "name": format!("Game {uuid}"),
                "uuid": uuid,
                LEGACY_KEY: {"exe": exe, "release": "Alpha0.3.5.8R2", "proxy": proxy},
            }));
            true
        })
        .unwrap();
    }
    let (pdll, pini, pak) = (
        format!("full/game/{PDLL}"),
        format!("full/game/{PINI}"),
        format!("full/game/{PAK}"),
    );
    let cache = package(
        &cache_root(h),
        "FG-V.B16",
        &[
            ("full/game/dxgi.dll", b"OptiScaler proxy"),
            ("full/game/OptiScaler.ini", b"[FrameGen]\nFGInput=nvngxfg\n"),
            ("full/game/OptiScaler/libxess.dll", b"xess"),
            ("full/game/amd_fidelityfx_dx12.dll", b"FidelityFX"),
            (&pdll, b"provider"),
            (&pini, b"path = auto\n"),
            (&pak, b"model"),
            ("full/LICENSES/OptiScaler-GPL-3.0.txt", b"GPL"),
        ],
    );
    pin_to(h, &cache);
}

/// Every refusal the install gives comes before Rubylight 3.0's AMDNR is
/// taken out: a game refused keeps it, byte for byte, and its app the
/// record of it, so it is still offered for removal and still launches.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_install_leaves_the_earlier_upscaling_as_it_was() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    let temp = tempdir();
    type Prepare = fn(&Path, &Path);
    let cases: [(&str, &str, Prepare, &str); 6] = [
        (
            "special-k",
            "winmm.dll",
            |folder, _| std::fs::write(folder.join("SpecialK64.dll"), "Special K").unwrap(),
            "Special K is installed",
        ),
        (
            "left-in-its-folder",
            "winmm.dll",
            |folder, _| std::fs::write(folder.join("OptiScaler/notes.txt"), "mine").unwrap(),
            "OptiScaler/notes.txt changed since or is not Rubylight's and would stay",
        ),
        (
            "changed-in-its-folder",
            "winmm.dll",
            |folder, _| std::fs::write(folder.join("OptiScaler/libxess.dll"), "XeSS 2").unwrap(),
            "OptiScaler/libxess.dll",
        ),
        (
            "users-optiscaler",
            "winmm.dll",
            |folder, _| std::fs::write(folder.join("version.dll"), "OptiScaler 0.7").unwrap(),
            "can't load another frame generation",
        ),
        (
            "every-name-taken",
            "dxgi.dll",
            |folder, exe| {
                std::fs::write(exe, b"MZ ... vulkan-1.dll ... kernel32.dll").unwrap();
                std::fs::write(folder.join("winmm.dll"), "a sound mod").unwrap();
                std::fs::write(folder.join("version.dll"), "a loader").unwrap();
            },
            "every name OptiScaler can load under",
        ),
        (
            "target-taken",
            "winmm.dll",
            |folder, _| {
                std::fs::write(folder.join("amd_fidelityfx_dx12.dll"), "the game's").unwrap()
            },
            "amd_fidelityfx_dx12.dll is already in the game's folder",
        ),
    ];
    let exes: Vec<PathBuf> = cases
        .iter()
        .map(|(uuid, ..)| game(&temp.path().join(uuid)))
        .collect();
    let games: Vec<(&str, &Path, &str)> = cases
        .iter()
        .zip(&exes)
        .map(|((uuid, proxy, ..), exe)| (*uuid, exe.as_path(), *proxy))
        .collect();
    amdnr_host(&f, &games);
    for ((uuid, _, prepare, refusal), exe) in cases.iter().zip(&exes) {
        let folder = exe.parent().unwrap();
        prepare(folder, exe);
        let before = snapshot(folder);
        enable(h, uuid, None).unwrap();
        let job = ended(h, uuid).await;
        assert_eq!(job.phase, "failed", "{uuid}: {job:?}");
        assert!(
            job.error.as_deref().unwrap().contains(refusal),
            "{uuid}: {job:?}"
        );
        assert_eq!(snapshot(folder), before, "{uuid}");
        let app = app(h, uuid).unwrap();
        assert_eq!(legacy_exe(&app).as_ref(), Some(exe), "{uuid}");
        assert_eq!(placed_exe(&app), None, "{uuid}");
    }
}

/// A file of Rubylight 3.0's AMDNR that changed since would stay when
/// AMDNR is taken out, and the game would have neither: the install is
/// refused before anything changes, naming the file, and the earlier
/// upscaling stays byte for byte with its record.
#[tokio::test(flavor = "multi_thread")]
async fn a_legacy_file_changed_in_size_leaves_the_earlier_upscaling_as_it_was() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    let temp = tempdir();
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    amdnr_host(&f, &[("game", &exe, "winmm.dll")]);
    std::fs::write(folder.join("LmxxfNrRuntime.pak"), "the user's model").unwrap();
    let before = snapshot(folder);
    enable(h, "game", None).unwrap();
    let job = ended(h, "game").await;
    assert_eq!(job.phase, "failed", "{job:?}");
    let error = job.error.as_deref().unwrap();
    assert!(
        error.contains("can't be removed whole: LmxxfNrRuntime.pak changed since")
            && error.contains("nothing was changed"),
        "{error}"
    );
    assert_eq!(snapshot(folder), before);
    let app = app(h, "game").unwrap();
    assert_eq!(legacy_exe(&app).as_ref(), Some(&exe));
    assert_eq!(placed_exe(&app), None);
    assert_eq!(
        folder::kept_text(&["LmxxfNrRuntime.pak".into()], true),
        "the earlier upscaling was removed, but LmxxfNrRuntime.pak is still in the game's folder: it changed since or is not Rubylight's, so frame generation was not installed; move it out and turn it on again"
    );
}

/// Rubylight 3.0's AMDNR beside the launcher while the game's own program
/// is chosen in `Binaries/Win64`: the check before its removal judges the
/// launcher's folder, frame generation goes in beside the chosen program,
/// and what stays of AMDNR keeps its record, so the console still offers
/// its removal. That removal takes only AMDNR out, finishing once someone
/// else's file is gone from its folder, and names what stays.
#[tokio::test(flavor = "multi_thread")]
async fn amdnr_beside_the_launcher_is_judged_in_its_own_folder() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    let temp = tempdir();
    let launcher = game(temp.path());
    let folder = launcher.parent().unwrap();
    let win64 = folder.join("Binaries/Win64");
    std::fs::create_dir_all(&win64).unwrap();
    for name in ["Game.exe", "nvngx_dlssg.dll"] {
        std::fs::copy(folder.join(name), win64.join(name)).unwrap();
    }
    let chosen = win64.join("Game.exe");
    amdnr_host(&f, &[("game", &launcher, "winmm.dll")]);
    std::fs::write(folder.join("LmxxfNrRuntime.pak"), "the user's model").unwrap();
    std::fs::write(folder.join("OptiScaler/notes.txt"), "the user's").unwrap();
    enable(h, "game", Some(chosen.clone())).unwrap();
    let job = ended(h, "game").await;
    assert_eq!(job.phase, "done", "{job:?}");
    let left: Manifest =
        serde_json::from_slice(&std::fs::read(folder.join(LEGACY_MANIFEST)).unwrap()).unwrap();
    assert_eq!(Vec::from_iter(left.files.keys()), ["LmxxfNrRuntime.pak"]);
    assert_eq!(left.folders, ["OptiScaler"]);
    assert!(win64.join(MANIFEST).exists());
    let record = app(h, "game").unwrap();
    assert_eq!(placed_exe(&record), Some(chosen.clone()));
    assert_eq!(legacy_exe(&record), Some(launcher.clone()));
    let shown = status(h, "game");
    assert_eq!(shown["enabled"], true, "{shown}");
    assert!(!shown["earlier_upscaling"].is_null(), "{shown}");

    std::fs::remove_file(folder.join("OptiScaler/notes.txt")).unwrap();
    let placed = snapshot(&win64);
    remove_earlier(h, "game").unwrap();
    let job = ended(h, "game").await;
    assert_eq!(job.phase, "done", "{job:?}");
    assert_eq!(job.kept, ["LmxxfNrRuntime.pak"]);
    assert!(!folder.join(LEGACY_MANIFEST).exists() && !folder.join("OptiScaler").exists());
    assert_eq!(snapshot(&win64), placed);
    let record = app(h, "game").unwrap();
    assert_eq!(
        (placed_exe(&record), legacy_exe(&record)),
        (Some(chosen), None)
    );
}

/// Only the helper judges whether AMDNR is beside another program, on the
/// paths followed: a program reached through a junction to AMDNR's own
/// folder is that folder, so the removal ends with no record left.
#[test]
fn only_the_helper_judges_whether_amdnr_is_elsewhere() {
    let temp = tempdir();
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let linked = temp.path().join("Linked");
    junction(&linked, folder);
    let other = temp.path().join("Other/Game.exe");
    let removal = |beside: &Path| {
        amdnr(folder, "winmm.dll");
        std::fs::write(folder.join("LmxxfNrRuntime.pak"), "the user's model").unwrap();
        let request = Request::Remove {
            exe: exe.clone(),
            beside: Some(beside.to_path_buf()),
            earlier: false,
        };
        handle(request, Writer::User, &|_, _| {})
    };
    let kept = vec!["LmxxfNrRuntime.pak".to_string()];
    let same = removal(&linked.join("Game.exe"));
    assert_eq!(same, Response::Removed { kept: kept.clone() });
    assert!(!folder.join(LEGACY_MANIFEST).exists());
    assert_eq!(removal(&other), Response::Left { kept });
    assert!(folder.join(LEGACY_MANIFEST).exists());
}

/// The console's Remove for a game that has only Rubylight 3.0's
/// upscaling: the job ends done, the folder is as it was and the record goes.
#[tokio::test(flavor = "multi_thread")]
async fn the_earlier_upscaling_alone_is_removed_from_the_game_page() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    let temp = tempdir();
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let before = snapshot(folder);
    amdnr(folder, "winmm.dll");
    butterpollo_stream::steam::update_library(h, |apps, _| {
        apps.push(json!({
            "name": "Game",
            "uuid": "game",
            LEGACY_KEY: {"exe": exe, "release": "Alpha0.3.5.8R2", "proxy": "winmm.dll"},
        }));
        true
    })
    .unwrap();
    disable(h, "game").unwrap();
    let job = ended(h, "game").await;
    assert_eq!(job.phase, "done", "{job:?}");
    assert_eq!(snapshot(folder), before);
    let app = app(h, "game").unwrap();
    assert!(!app.extra.contains_key(LEGACY_KEY) && !app.extra.contains_key(KEY));
    assert!(status(h, "game")["earlier_upscaling"].is_null());
}

/// A game with only Rubylight 3.0's upscaling that the library sync drops
/// loses it all the same; one on a drive that is not connected stays a
/// pending removal.
#[test]
fn a_dropped_game_loses_the_earlier_upscaling_or_has_its_removal_pending() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    let temp = tempdir();
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let before = snapshot(folder);
    amdnr(folder, "winmm.dll");
    let away = unconnected_drive().join(r"Games\Away\Away.exe");
    let legacy = |name: &str, uuid: &str, exe: &Path| {
        json!({
            "name": name,
            "uuid": uuid,
            LEGACY_KEY: {"exe": exe, "release": "Alpha0.3.5.8R2", "proxy": "winmm.dll"},
        })
    };
    butterpollo_stream::steam::update_library(h, |apps, _| {
        apps.push(legacy("Game", "game", &exe));
        apps.push(legacy("Away", "away", &away));
        true
    })
    .unwrap();
    butterpollo_stream::steam::update_library(h, |apps, _| {
        apps.clear();
        true
    })
    .unwrap();
    assert_eq!(snapshot(folder), before);
    assert_eq!(
        pending(h),
        [Pending {
            name: "Away".into(),
            exe: away,
            uuid: Some("away".into()),
        }]
    );
}
