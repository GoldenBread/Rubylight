//! Release download pipeline, real-release check, user OptiScaler add-on mode, DLSS, and Vulkan tests for Neural Rendering.
use super::common::*;
use super::common::{install, remove};
use butterpollo_session::neural::{self, Gpu, MANIFEST, Manifest, Mode};
use butterpollo_stream::neural::folder::{Request, Response, Writer, handle};
use butterpollo_stream::neural::*;
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

/// Only a test's own server gets a plain client: GitHub gets the updater's,
/// HTTPS only and with the User-Agent its API requires (without one it
/// answers 403).
#[tokio::test]
async fn github_gets_the_updaters_client() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    let error = butterpollo_stream::neural::http(h, Duration::from_secs(5))
        .unwrap()
        .get("http://127.0.0.1:9/")
        .send()
        .await
        .unwrap_err();
    // Refused before it connects: the updater's client is HTTPS only.
    assert!(error.is_builder(), "{error:#}");
}

/// The whole download against a server of its own: the pinned release, its
/// checksum list (itself pinned) checked against GitHub's digests before
/// any archive comes, both archives, then the cache. An older release in
/// the cache goes only once no copy reads it; GitHub listing other digests
/// than the pin's, a checksum list that doesn't name an archive's digest
/// or a file other than GitHub's digest says caches nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_download_goes_list_checksums_archives_cache() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    let files = anr_files();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = serve(
        listener,
        release_list(&base, TAG, &files),
        Arc::new(files.clone()),
    )
    .await;
    *h.neural.origin.lock().unwrap() = Some(server.base.clone());
    pin_files(h, &files);
    let stale = release(&cache_root(h), "Alpha0.3.5.6", b"OptiScaler proxy");
    let older = release(&cache_root(h), "Alpha0.3.5.7", b"OptiScaler proxy");
    // An install about to copy the older release: it outlasts the newer
    // one's download.
    let using = pin(h, &older);

    // A copy from the older release holds the cache: it stays until then.
    let (held, done) = (mpsc::channel(), mpsc::channel::<()>());
    let copy = {
        let h = h.clone();
        std::thread::spawn(move || {
            let _reading = h.neural.reading();
            held.0.send(()).unwrap();
            let _ = done.1.recv();
        })
    };
    held.1.recv().unwrap();
    let fetching = {
        let h = h.clone();
        tokio::spawn(async move { fetch(&h, &|_, _| {}).await })
    };
    let deadline = Instant::now() + Duration::from_secs(20);
    while server.log.lock().unwrap().len() < 4 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!fetching.is_finished());
    assert!(older.exists());
    done.0.send(()).unwrap();
    copy.join().unwrap();
    let (folder, package) = fetching.await.unwrap().unwrap();
    assert_eq!(
        *server.log.lock().unwrap(),
        ["list", "SHA256SUMS.txt", FULL, ADDON]
    );
    assert_eq!(folder, cache_root(h).join(TAG));
    assert!(!stale.exists());
    assert!(older.exists(), "an install was about to copy it");
    // It goes with the last install that held it; the newest stays.
    let again = pin(h, &older);
    drop(using);
    assert!(older.exists(), "another install still copies it");
    drop(again);
    assert!(!older.exists());
    drop(pin(h, &folder));
    assert!(folder.exists());
    assert!(h.neural.lock().using.is_empty());
    assert_eq!(
        package.archives,
        BTreeMap::from([
            (ADDON.to_owned(), sha(&files[ADDON])),
            (FULL.to_owned(), sha(&files[FULL])),
        ])
    );
    assert_eq!(
        std::fs::read(folder.join(format!("files/full/game/{PAK}"))).unwrap(),
        b"model"
    );
    assert_eq!(
        std::fs::read(folder.join(format!(
            "files/addon/{}/dlssg/nvngx_dlssg.dll",
            neural::PROVIDER
        )))
        .unwrap(),
        b"addon provider"
    );
    // The same release again: from the cache, without asking GitHub.
    server.log.lock().unwrap().clear();
    assert_eq!(fetch(h, &|_, _| {}).await.unwrap().1, package);
    assert!(server.log.lock().unwrap().is_empty());
    // The same release damaged in the cache, held for an install and one of
    // its files open: it comes again, and the one there is never left half
    // deleted, whether or not it could be replaced meanwhile.
    let whole = names(&folder);
    let ini = folder.join("files/full/game/OptiScaler.ini");
    std::fs::write(&ini, "changed").unwrap();
    h.neural.lock().verified = None;
    let using = pin(h, &folder);
    let held = hold(&folder.join(format!("files/full/game/{PAK}")));
    let again = fetch(h, &|_, _| {}).await.map(|(folder, _)| folder);
    drop(held);
    assert_eq!(names(&folder), whole, "{again:?}");
    if again.is_err() {
        // Once nothing holds it, it is replaced.
        fetch(h, &|_, _| {}).await.unwrap();
    }
    drop(using);
    assert_eq!(names(&folder), whole);
    assert_eq!(std::fs::read(&ini).unwrap(), b"[FrameGen]\n");
    assert_eq!(cached(&cache_root(h)).unwrap().0, folder);
    std::fs::remove_dir_all(cache_root(h)).unwrap();

    // A stream starts while the switch's own download runs: it stops.
    f.session(
        f.client(u32::MAX),
        butterpollo_session::session::Role::Stream,
    );
    let error = fetch(h, &|_, _| {}).await.unwrap_err();
    assert!(error.is::<StreamStarted>(), "{error:#}");
    assert!(cached(&cache_root(h)).is_none());
    h.sessions.lock().unwrap().active.clear();

    // A checksum list (as pinned) that doesn't name the full package's
    // digest: no archive comes.
    let mut lying = files.clone();
    let mut named = files.clone();
    named.insert(FULL.into(), b"another".to_vec());
    named.remove("SHA256SUMS.txt");
    lying.insert("SHA256SUMS.txt".into(), sums_of(&named).into_bytes());
    // A file other than GitHub's digest says, its file list the same.
    let mut tampered = files.clone();
    let add_on = tampered.get_mut(ADDON).unwrap();
    let at = add_on.windows(5).position(|w| w == b"addon").unwrap();
    add_on[at + 1] = b'x';
    // A package other than the pinned one, as GitHub lists it.
    let mut other = files.clone();
    other.insert(FULL.into(), files[FULL].iter().rev().copied().collect());
    for (served, listed, problem) in [
        (other.clone(), other, "another SHA-256 than Rubylight pins"),
        (
            lying.clone(),
            lying,
            "does not match the release's SHA256SUMS.txt",
        ),
        (tampered, files.clone(), "SHA-256"),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let list = release_list(&base, TAG, &listed);
        // The lying list as the pin has it: the pin names the list, the
        // list names the archives.
        pin_files(
            h,
            if problem.contains("SHA256SUMS") {
                &listed
            } else {
                &files
            },
        );
        let server = serve(listener, list, Arc::new(served)).await;
        *h.neural.origin.lock().unwrap() = Some(server.base.clone());
        let error = format!("{:#}", fetch(h, &|_, _| {}).await.unwrap_err());
        assert!(error.contains(problem), "{error}");
        assert!(cached(&cache_root(h)).is_none());
        let log = server.log.lock().unwrap().clone();
        if problem.contains("pins") {
            assert_eq!(log, ["list"]);
        }
        if problem.contains("SHA256SUMS") {
            assert_eq!(log, ["list", "SHA256SUMS.txt"]);
        }
    }
}

/// The switch's download waits until no stream runs, and one that fails
/// tells the console so without the release's file names; the log has them.
#[tokio::test(flavor = "multi_thread")]
async fn a_download_waits_for_no_stream_and_its_failure_names_no_files() {
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
    f.session(
        f.client(u32::MAX),
        butterpollo_session::session::Role::Stream,
    );
    assert!(matches!(enable(h, "game", None), Err(Refusal::Streaming)));
    assert!(!h.neural.lock().jobs.contains_key("game"));
    // A release downloaded already changes nothing: its copy into the
    // game's folder would take the stream's disk.
    let cache = release(&cache_root(h), "Alpha0.3.5.8", b"OptiScaler proxy");
    assert!(matches!(enable(h, "game", None), Err(Refusal::Streaming)));
    assert!(!h.neural.lock().jobs.contains_key("game"));
    h.sessions.lock().unwrap().active.clear();
    // Once it ends, the switch goes on from the cache, without GitHub.
    pin_to(h, &cache);
    enable(h, "game", None).unwrap();
    let job = ended(h, "game").await;
    // The app names no program or install: the user chooses the game's.
    assert!(job.choose_exe, "{job:?}");
    std::fs::remove_dir_all(cache.parent().unwrap()).unwrap();

    let mut files = anr_files();
    let mut named = files.clone();
    named.insert(FULL.into(), b"another".to_vec());
    named.remove("SHA256SUMS.txt");
    files.insert("SHA256SUMS.txt".into(), sums_of(&named).into_bytes());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let list = release_list(&base, TAG, &files);
    pin_files(h, &files);
    let server = serve(listener, list, Arc::new(files)).await;
    *h.neural.origin.lock().unwrap() = Some(server.base);
    // The game's program is found before the download.
    let temp = tempdir();
    enable(h, "game", Some(game(temp.path()))).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let job = loop {
        let job = h.neural.lock().jobs["game"].clone();
        if job.phase == "failed" || Instant::now() > deadline {
            break job;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(job.phase, "failed");
    assert_eq!(job.error.as_deref(), Some(DOWNLOAD_FAILED));
}

/// The real pinned release from GitHub into a fake game in the temp folder
/// and out again, byte for byte; then the add-on into a fake game with the
/// release's own OptiScaler as the user's, its OptiScaler.ini restored
/// exactly: `cargo test -p butterpollo neural::tests::real_release -- --ignored --nocapture`.
#[tokio::test]
#[ignore = "downloads about 14 MB from GitHub"]
async fn real_release_into_a_fake_game() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    let base = std::env::temp_dir().join("rubylight-neural-real");
    let _ = std::fs::remove_dir_all(&base);
    let game = base.join("fakegame");
    std::fs::create_dir_all(&game).unwrap();
    let exe = game.join("FakeGame.exe");
    std::fs::write(&exe, "MZ dummy").unwrap();
    std::fs::write(
        game.join("nvngx_dlssg.dll"),
        "the game's DLSS frame generation",
    )
    .unwrap();
    std::fs::write(game.join("dxgi.dll"), "a dummy loader of another program").unwrap();
    let show = |when: &str, folder: &Path| {
        println!("== {when}");
        for (name, hash) in snapshot(folder) {
            println!("   {name} {}", hash.as_deref().unwrap_or("(folder)"));
        }
    };
    let before = snapshot(&game);
    show("before install", &game);
    let started = Instant::now();
    let (folder, package) = fetch(h, &|_, _| {}).await.unwrap();
    println!(
        "cached {} from {:?} ({} files) in {:?} at {}",
        package.tag,
        package.archives,
        package.files.len(),
        started.elapsed(),
        folder.display()
    );
    let response = install(Some(&exe), None, &folder);
    println!("{response:?}");
    assert!(matches!(
        response,
        Response::Placed {
            mode: Mode::Full,
            ..
        }
    ));
    show("after install", &game);
    assert!(matches!(
        prepare(&exe, Some(&folder)),
        Response::Placed { updated: false, .. }
    ));
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
    show("after removal", &game);
    assert_eq!(snapshot(&game), before);

    // The add-on into the release's own OptiScaler, as a user installed it.
    let addon = base.join("addongame");
    std::fs::create_dir_all(&addon).unwrap();
    let exe = addon.join("AddonGame.exe");
    std::fs::write(&exe, "MZ dummy").unwrap();
    std::fs::write(
        addon.join("nvngx_dlssg.dll"),
        "the game's DLSS frame generation",
    )
    .unwrap();
    let files = folder.join("files/full/game");
    std::fs::copy(files.join("dxgi.dll"), addon.join("dxgi.dll")).unwrap();
    let mut ini = std::fs::read(files.join("OptiScaler.ini")).unwrap();
    // The user's own value for one of the three.
    let at = ini
        .windows(13)
        .position(|w| w == b"FGInput=auto\r")
        .or_else(|| ini.windows(12).position(|w| w == b"FGInput=auto"));
    println!("FGInput=auto in the release's OptiScaler.ini: {at:?}");
    if let Some(at) = at {
        ini.splice(at..at + 12, b"FGInput=xefg".iter().copied());
    }
    std::fs::write(addon.join("OptiScaler.ini"), &ini).unwrap();
    let before = snapshot(&addon);
    let response = install(Some(&exe), None, &folder);
    println!("{response:?}");
    assert!(matches!(
        response,
        Response::Placed {
            mode: Mode::Addon,
            ..
        }
    ));
    show("after the add-on", &addon);
    let changed = String::from_utf8(std::fs::read(addon.join("OptiScaler.ini")).unwrap()).unwrap();
    for value in [
        "FGInput=nvngxfg",
        "FGNvngxReplacement=Apex Neural Rendering FG",
        "DisableHudless=false",
    ] {
        assert!(changed.contains(value), "{value}");
    }
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
    show("after the add-on's removal", &addon);
    assert_eq!(snapshot(&addon), before);
    assert_eq!(std::fs::read(addon.join("OptiScaler.ini")).unwrap(), ini);
    let _ = std::fs::remove_dir_all(&base);
}

/// The user's own OptiScaler that can load another frame generation: only
/// the provider goes into its `custom` folder and three values into its
/// settings; a removal takes the provider out and puts the three back byte
/// for byte, leaving what the user changed since.
#[test]
fn the_users_own_optiscaler_gets_the_provider_and_its_settings_back_exactly() {
    let temp = tempdir();
    let cache = release(temp.path(), "FG-V.B16", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    std::fs::write(
        folder.join("dxgi.dll"),
        b"MZ OptiScaler ... Custom DLSSG ...",
    )
    .unwrap();
    let ini: &[u8] = b"\xef\xbb\xbf[FrameGen]\r\nEnabled=auto\r\nFGInput=auto\r\nDisableHudless=auto\r\n\r\n[NvngxFG]\r\nDisableHudless = auto\r\n";
    std::fs::write(folder.join("OptiScaler.ini"), ini).unwrap();
    std::fs::create_dir(folder.join("OptiScaler")).unwrap();
    std::fs::write(folder.join("OptiScaler/libxess.dll"), b"the user's").unwrap();
    // fakenvapi beside it is OptiScaler's own add-on; another mod is refused.
    std::fs::write(folder.join("fakenvapi.ini"), b"").unwrap();
    for (file, other) in [
        ("SpecialK64.dll", "Special K"),
        ("dlssg_to_fsr3_amd_is_better.dll", "dlssg-to-fsr3"),
        ("dlss-enabler.dll", "DLSS Enabler"),
    ] {
        std::fs::write(folder.join(file), b"another mod").unwrap();
        let mine = snapshot(folder);
        let Response::Refused { reason } = install(Some(&exe), None, &cache) else {
            panic!("{file} beside the user's OptiScaler was not refused")
        };
        assert!(reason.contains(other), "{reason}");
        assert_eq!(snapshot(folder), mine);
        std::fs::remove_file(folder.join(file)).unwrap();
    }
    let before = snapshot(folder);

    assert_eq!(
        install(Some(&exe), None, &cache),
        Response::Placed {
            exe: exe.clone(),
            release: "FG-V.B16".into(),
            proxy: "dxgi.dll".into(),
            updated: false,
            mode: Mode::Addon,
        }
    );
    let provider = "OptiScaler/custom/Apex Neural Rendering FG/dlssg";
    assert_eq!(
        std::fs::read(folder.join(format!("{provider}/nvngx_dlssg.dll"))).unwrap(),
        b"addon provider"
    );
    assert_eq!(
        std::fs::read(folder.join(format!("{provider}/kernels/pack.nrx"))).unwrap(),
        b"addon model"
    );
    // The user's OptiScaler and its files stay theirs.
    assert_eq!(
        std::fs::read(folder.join("dxgi.dll")).unwrap(),
        b"MZ OptiScaler ... Custom DLSSG ..."
    );
    assert_eq!(
        std::fs::read(folder.join("OptiScaler/libxess.dll")).unwrap(),
        b"the user's"
    );
    let changed = String::from_utf8(std::fs::read(folder.join("OptiScaler.ini")).unwrap()).unwrap();
    let frame_gen = changed.split("[NvngxFG]").next().unwrap();
    for value in [
        "FGInput=nvngxfg\r\n",
        "FGNvngxReplacement=Apex Neural Rendering FG\r\n",
        "DisableHudless=false\r\n",
        "Enabled=auto\r\n",
    ] {
        assert!(frame_gen.contains(value), "{value:?} in {changed:?}");
    }
    assert!(!frame_gen.contains("FGInput=auto"), "{changed:?}");
    assert!(!frame_gen.contains("DisableHudless=auto"), "{changed:?}");
    assert!(
        changed.ends_with("[NvngxFG]\r\nDisableHudless = auto\r\n"),
        "{changed:?}"
    );
    let manifest =
        Manifest::parse(&std::fs::read_to_string(folder.join(MANIFEST)).unwrap()).unwrap();
    assert_eq!(manifest.mode, Mode::Addon);
    assert!(!manifest.files.contains_key("dxgi.dll"));
    assert!(manifest.ini.is_some());
    // Turned on again: nothing changes.
    assert!(matches!(
        install(Some(&exe), None, &cache),
        Response::Placed { updated: false, .. }
    ));
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
    assert_eq!(snapshot(folder), before);

    // The menu changed another value meanwhile: it stays as the user set it.
    install(Some(&exe), None, &cache);
    let menu = std::fs::read_to_string(folder.join("OptiScaler.ini"))
        .unwrap()
        .replace("Enabled=auto", "Enabled=true");
    std::fs::write(folder.join("OptiScaler.ini"), menu).unwrap();
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
    let expected = String::from_utf8(ini.to_vec())
        .unwrap()
        .replace("Enabled=auto", "Enabled=true");
    assert_eq!(
        std::fs::read_to_string(folder.join("OptiScaler.ini")).unwrap(),
        expected
    );

    // A provider folder already there that holds someone's provider is
    // never written over (an empty one is taken over: nothing in it to
    // write over).
    std::fs::write(folder.join("OptiScaler.ini"), ini).unwrap();
    std::fs::create_dir_all(folder.join(provider)).unwrap();
    std::fs::write(
        folder.join(format!("{provider}/nvngx_dlssg.dll")),
        b"theirs",
    )
    .unwrap();
    let Response::Refused { reason } = install(Some(&exe), None, &cache) else {
        panic!()
    };
    assert!(reason.contains("from an earlier install"), "{reason}");
    assert_eq!(std::fs::read(folder.join("OptiScaler.ini")).unwrap(), ini);
    assert_eq!(
        std::fs::read(folder.join(format!("{provider}/nvngx_dlssg.dll"))).unwrap(),
        b"theirs"
    );
}

/// A game without DLSS frame generation is refused; the game's own (a
/// Streamline plugin deeper in its install counts) is what the provider
/// takes the place of. A copy in OptiScaler's folder doesn't count.
#[test]
fn a_game_without_dlss_frame_generation_is_refused() {
    let temp = tempdir();
    let cache = release(temp.path(), "FG-V.B16", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    std::fs::remove_file(folder.join("nvngx_dlssg.dll")).unwrap();
    let before = snapshot(folder);
    let Response::Refused { reason } = install(Some(&exe), None, &cache) else {
        panic!()
    };
    assert!(reason.contains("no DLSS frame generation"), "{reason}");
    assert_eq!(snapshot(folder), before);
    let found = handle(
        Request::Find {
            exe: Some(exe.clone()),
            install: None,
        },
        Writer::User,
        &|_, _| {},
    );
    assert!(matches!(found, Response::Refused { .. }), "{found:?}");
    std::fs::create_dir_all(folder.join("OptiScaler/custom/x/dlssg")).unwrap();
    std::fs::write(
        folder.join("OptiScaler/custom/x/dlssg/nvngx_dlssg.dll"),
        "a provider",
    )
    .unwrap();
    assert!(matches!(
        install(Some(&exe), None, &cache),
        Response::Refused { .. }
    ));
    std::fs::remove_dir_all(folder.join("OptiScaler")).unwrap();
    std::fs::create_dir_all(folder.join("bin/plugins")).unwrap();
    std::fs::write(folder.join("bin/plugins/SL.DLSS_G.DLL"), "Streamline").unwrap();
    assert!(matches!(
        install(Some(&exe), None, &cache),
        Response::Placed { .. }
    ));
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
}

/// A Vulkan game never gets dxgi.dll, which it doesn't load: OptiScaler
/// goes under winmm.dll.
#[test]
fn a_vulkan_game_gets_optiscaler_under_a_name_it_loads() {
    let temp = tempdir();
    let cache = release(temp.path(), "FG-V.B16", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    std::fs::remove_file(folder.join("dxgi.dll")).unwrap();
    std::fs::write(&exe, b"MZ ... vulkan-1.dll ... kernel32.dll").unwrap();
    let Response::Placed { proxy, .. } = install(Some(&exe), None, &cache) else {
        panic!()
    };
    assert_eq!(proxy, "winmm.dll");
    assert!(!folder.join("dxgi.dll").exists());
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
    // A D3D12 game takes dxgi.dll.
    std::fs::write(&exe, b"MZ ... d3d12.dll ... dxgi.dll").unwrap();
    let Response::Placed { proxy, .. } = install(Some(&exe), None, &cache) else {
        panic!()
    };
    assert_eq!(proxy, "dxgi.dll");
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
}
