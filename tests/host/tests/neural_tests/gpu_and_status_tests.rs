//! GPU capability probing, hybrid graphics, runtime checks, and non-blocking status read tests for Neural Rendering.
use super::common::*;
use butterpollo_session::neural::{self, Gpu};
use butterpollo_stream::neural::*;
use butterpollo_stream::state::Shared;
use serde_json::json;
use std::{
    path::Path,
    sync::mpsc,
    time::{Duration, Instant},
};

/// [`capability`]'s GPU, by name.
fn gpu_of(h: &Shared) -> Result<String, String> {
    capability(h, None).map(|capable| capable.gpu)
}

/// A laptop that streams with its integrated graphics, whose game renders on
/// its NVIDIA card: frame generation is offered, and the switch names the
/// card and says it runs when the game renders there. Intel graphics alone
/// are not offered; beside an NVIDIA card they are.
#[test]
fn a_hybrid_pc_is_offered_frame_generation_on_its_dedicated_gpu() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    let adapter = |name: &str, luid: u64, vendor: u32, device: u32| neural::Adapter {
        name: name.into(),
        luid,
        vendor,
        device,
    };
    let vega = adapter("AMD Radeon(TM) Graphics", 2, neural::AMD, 0x1638);
    let rtx = adapter(
        "NVIDIA GeForce RTX 3060 Laptop GPU",
        4,
        neural::NVIDIA,
        0x2560,
    );
    let intel = adapter("Intel(R) UHD Graphics", 6, 0x8086, 0x9a49);
    let set = |streams: &neural::Adapter, adapters: &[&neural::Adapter]| {
        *h.neural.streams.lock().unwrap() = Some(streams.clone());
        *h.neural.adapters.lock().unwrap() =
            Some(adapters.iter().map(|adapter| (*adapter).clone()).collect());
        let mut state = h.neural.lock();
        (state.probe, state.offered) = (None, None);
    };
    *h.neural.probe.lock().unwrap() = Some(Probe {
        gpus: vec![Gpu {
            name: vega.name.clone(),
            arch: "gfx90c".into(),
            compute_units: 8,
            luid: Some(2),
        }],
        error: None,
    });
    set(&vega, &[&vega, &rtx]);
    assert!(offered(h));
    let shown = status(h, "missing");
    assert_eq!(shown["available"], true, "{shown}");
    assert_eq!(shown["gpu"], rtx.name.as_str());
    let note = shown["gpu_note"].as_str().unwrap();
    assert!(
        note.contains("runs when the game renders on NVIDIA GeForce RTX 3060 Laptop GPU"),
        "{note}"
    );
    // Without the card, the Vega graphics alone are refused.
    set(&vega, &[&vega]);
    let shown = status(h, "missing");
    assert_eq!(shown["available"], false);
    assert!(shown["gpu_note"].is_null());
    set(&intel, &[&intel]);
    assert!(!offered(h));
    set(&intel, &[&intel, &rtx]);
    assert!(offered(h));
    assert_eq!(gpu_of(h), Ok(rtx.name.clone()));
}

/// A streaming GPU that can't be found keeps the switch offered, and is
/// looked for again next time instead of being held for the whole run.
#[test]
fn offered_asks_again_after_the_gpu_lookup_fails() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    *h.neural.streams.lock().unwrap() = None;
    *h.neural.probe.lock().unwrap() = None;
    assert!(offered(h));
    assert!(h.neural.lock().offered.is_none());
    *h.neural.streams.lock().unwrap() = Some(neural::Adapter {
        name: "Intel Arc A770".into(),
        luid: 7,
        vendor: 0x8086,
        device: 0x56A0,
    });
    assert!(!offered(h));
    // A known answer is held.
    *h.neural.streams.lock().unwrap() = None;
    assert!(!offered(h));
}

#[tokio::test]
async fn the_switch_is_offered_only_on_a_capable_gpu_after_the_notice() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    let streams = |name: &str, vendor: u32, device: u32| {
        *h.neural.streams.lock().unwrap() = Some(neural::Adapter {
            name: name.into(),
            luid: 7,
            vendor,
            device,
        });
        h.neural.lock().probe = None;
    };
    streams("NVIDIA GeForce GTX 1080", neural::NVIDIA, 0x1B80);
    let status = status(h, "missing");
    assert_eq!(status["available"], false);
    assert!(
        status["reason"]
            .as_str()
            .unwrap()
            .contains("which is older"),
        "{status}"
    );
    assert_eq!(status["accepted"], false);
    assert!(matches!(
        enable(h, "missing", None),
        Err(Refusal::NotAccepted)
    ));
    accept(h).unwrap();
    assert!(accepted(h));
    assert!(matches!(
        enable(h, "missing", None),
        Err(Refusal::Unavailable(_))
    ));
    // The answer is kept: a new one only after a while.
    *h.neural.streams.lock().unwrap() = Some(neural::Adapter {
        name: "NVIDIA GeForce RTX 2060".into(),
        luid: 7,
        vendor: neural::NVIDIA,
        device: 0x1F08,
    });
    assert!(gpu_of(h).is_err());
    h.neural.lock().probe = None;
    assert_eq!(gpu_of(h), Ok("NVIDIA GeForce RTX 2060".into()));
    // Without the Visual C++ runtime OptiScaler needs.
    h.neural
        .vc_missing
        .lock()
        .unwrap()
        .push("VCRUNTIME140_1.dll");
    h.neural.lock().probe = None;
    let error = gpu_of(h).unwrap_err();
    assert!(error.contains("Visual C++"), "{error}");
    // A copy beside the game's program counts: its DLL finds it there first.
    let game = tempdir();
    std::fs::write(game.path().join("VCRUNTIME140_1.dll"), "runtime").unwrap();
    assert_eq!(
        capability(h, Some(game.path())).map(|capable| capable.gpu),
        Ok("NVIDIA GeForce RTX 2060".into())
    );
    // Installed: the switch says so at once, with the GPU's answer still held.
    h.neural.vc_missing.lock().unwrap().clear();
    assert!(h.neural.lock().probe.is_some());
    assert_eq!(gpu_of(h), Ok("NVIDIA GeForce RTX 2060".into()));

    // AMD: the compute runtime's architecture of the GPU that streams.
    let probe = |arch: Option<&str>| {
        *h.neural.probe.lock().unwrap() = Some(match arch {
            Some(arch) => Probe {
                gpus: vec![Gpu {
                    name: "AMD Radeon".into(),
                    arch: arch.into(),
                    compute_units: 60,
                    luid: Some(7),
                }],
                error: None,
            },
            None => Probe {
                gpus: Vec::new(),
                error: Some("no compute runtime".into()),
            },
        });
    };
    probe(Some("gfx1030"));
    streams("AMD Radeon RX 6800 XT", neural::AMD, 0);
    assert_eq!(gpu_of(h), Ok("AMD Radeon RX 6800 XT".into()));
    probe(Some("gfx906"));
    streams("AMD Radeon VII", neural::AMD, 0x66AF);
    let error = gpu_of(h).unwrap_err();
    assert!(error.contains("which is older"), "{error}");
    // The runtime can't say: the PCI device ID decides, else it is refused.
    probe(None);
    streams("AMD Radeon 780M Graphics", neural::AMD, 0x1900);
    assert_eq!(gpu_of(h), Ok("AMD Radeon 780M Graphics".into()));
    streams("AMD Radeon VII", neural::AMD, 0x66AF);
    let error = gpu_of(h).unwrap_err();
    assert!(error.contains("can't identify the GPU"), "{error}");
    // Neither AMD nor NVIDIA.
    streams("Intel Arc A770", 0x8086, 0x56A0);
    let error = gpu_of(h).unwrap_err();
    assert!(error.contains("AMD Radeon RX 5000"), "{error}");
    streams("AMD Radeon 780M Graphics", neural::AMD, 0x1900);
    assert!(matches!(enable(h, "missing", None), Err(Refusal::NotFound)));
}

/// A GPU found capable stays so until the host restarts, but not past a
/// change of the display or Graphics card a stream uses: the GPU it named
/// may no longer be the one that streams.
#[test]
fn the_gpu_check_is_asked_again_once_the_streaming_gpu_is_set_otherwise() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    let answer = |name: &str| {
        *h.neural.probe.lock().unwrap() = Some(Probe {
            gpus: vec![Gpu {
                name: name.into(),
                arch: "gfx1100".into(),
                compute_units: 84,
                luid: None,
            }],
            error: None,
        });
    };
    answer("AMD Radeon RX 7900 XT");
    assert_eq!(gpu_of(h), Ok("AMD Radeon RX 7900 XT".into()));
    answer("AMD Radeon RX 7900 XTX");
    assert_eq!(gpu_of(h), Ok("AMD Radeon RX 7900 XT".into()));
    // Each of the three settings, changed alone, asks again.
    for (n, config) in [
        "adapter_name=AMD Radeon RX 7900 XTX",
        "adapter_name=AMD Radeon RX 7900 XTX\nadapter_pnp_id=PCI\\VEN_1002&DEV_744C",
        "adapter_name=AMD Radeon RX 7900 XTX\nadapter_pnp_id=PCI\\VEN_1002&DEV_744C\noutput_name=\\\\.\\DISPLAY2",
    ]
    .into_iter()
    .enumerate()
    {
        *h.config.write().unwrap() = butterpollo_config::config::Config::parse(config).unwrap();
        let name = format!("AMD Radeon PRO W7900 {n}");
        answer(&name);
        assert_eq!(gpu_of(h), Ok(name.clone()), "{config}");
        answer("AMD Radeon RX 9070 XT");
        assert_eq!(gpu_of(h), Ok(name), "{config}");
    }
}

/// A game whose own check finds no DLSS frame generation, on a PC that can
/// run it: an install folder holding only its program.
fn checked_game(f: &butterpollo_stream::state::test_support::Fixture, root: &Path) {
    let h = &f.host;
    *h.neural.streams.lock().unwrap() = Some(neural::Adapter {
        name: "NVIDIA GeForce RTX 2060".into(),
        luid: 7,
        vendor: neural::NVIDIA,
        device: 0x1F08,
    });
    let folder = root.join("without");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("Game.exe"), "MZ game").unwrap();
    butterpollo_stream::steam::update_library(h, |apps, _| {
        apps.push(json!({"name": "Without", "uuid": "without", "launcher-install-dir": folder}));
        true
    })
    .unwrap();
}

fn game_checks(h: &Shared) -> usize {
    h.neural
        .game_checks
        .load(std::sync::atomic::Ordering::SeqCst)
}

/// A game's own check that stalls holds a status read only a few seconds:
/// the read answers without it, a second read starts no second check, and
/// the check's answer serves the next read once it ends.
#[test]
fn a_slow_game_check_answers_a_later_read_and_runs_once() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    let temp = tempdir();
    checked_game(&f, temp.path());
    let wait = Duration::from_secs(3);
    *h.neural.game_wait.lock().unwrap() = Some(wait);
    let (release, gate) = mpsc::channel::<()>();
    *h.neural.game_gate.lock().unwrap() = Some(gate);
    let started = Instant::now();
    let shown = status(h, "without");
    let waited = started.elapsed();
    assert_eq!(shown["available"], true, "{shown}");
    assert!(shown["game"].is_null(), "{shown}");
    assert!(waited >= wait && waited < GAME_WAIT, "{waited:?}");
    let started = Instant::now();
    assert!(status_waiting(h, "without", Duration::ZERO)["game"].is_null());
    assert!(started.elapsed() < wait / 2);
    *h.neural.game_wait.lock().unwrap() = None;
    assert_eq!(game_checks(h), 1);
    release.send(()).unwrap();
    let shown = status(h, "without");
    assert_eq!(shown["game"]["why"], "no_frame_generation", "{shown}");
    assert_eq!(game_checks(h), 1);
}

/// A check that began before a job and ends after it is not kept: the
/// job changed the game's folder.
#[test]
fn a_game_check_that_began_before_a_job_is_not_kept() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    let temp = tempdir();
    checked_game(&f, temp.path());
    let (release, gate) = mpsc::channel::<()>();
    *h.neural.game_gate.lock().unwrap() = Some(gate);
    assert!(status_waiting(h, "without", Duration::ZERO)["game"].is_null());
    start_job(h, "without", "remove", "remove").unwrap();
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while h.neural.lock().checking.contains("without") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!h.neural.lock().checking.contains("without"));
    assert!(!h.neural.lock().games.contains_key("without"));
    // Without a job meanwhile the answer is kept.
    update_job(h, "without", |job| job.phase = "done");
    let shown = status(h, "without");
    assert_eq!(shown["game"]["why"], "no_frame_generation", "{shown}");
    assert!(h.neural.lock().games.contains_key("without"));
}

/// The Visual C++ runtime beside a program the user chose in the console
/// counts, as beside the one the app names.
#[tokio::test(flavor = "multi_thread")]
async fn the_runtime_beside_a_chosen_program_counts() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    *h.neural.streams.lock().unwrap() = Some(neural::Adapter {
        name: "NVIDIA GeForce RTX 2060".into(),
        luid: 7,
        vendor: neural::NVIDIA,
        device: 0x1F08,
    });
    accept(h).unwrap();
    let cache = release(&cache_root(h), "FG-V.B16", b"OptiScaler proxy");
    pin_to(h, &cache);
    butterpollo_stream::steam::update_library(h, |apps, _| {
        apps.push(json!({"name": "Game", "uuid": "game"}));
        true
    })
    .unwrap();
    h.neural
        .vc_missing
        .lock()
        .unwrap()
        .push("VCRUNTIME140_1.dll");
    let temp = tempdir();
    let exe = game(temp.path());
    std::fs::write(exe.with_file_name("VCRUNTIME140_1.dll"), "runtime").unwrap();
    assert!(matches!(
        enable(h, "game", None),
        Err(Refusal::Unavailable(reason)) if reason.contains("Visual C++")
    ));
    assert_eq!(status(h, "game")["unavailable"], "runtime");
    enable(h, "game", Some(exe.clone())).unwrap();
    let job = ended(h, "game").await;
    assert_eq!(job.phase, "done", "{job:?}");
}

/// A streaming GPU that can't be found still leaves the PC's other GPUs
/// judged, with no note on which one streams.
#[test]
fn an_unknown_streaming_gpu_leaves_the_others_judged() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    let adapter = |name: &str, luid: u64, vendor: u32, device: u32| neural::Adapter {
        name: name.into(),
        luid,
        vendor,
        device,
    };
    let intel = adapter("Intel(R) UHD Graphics", 6, 0x8086, 0x9a49);
    let rtx = adapter(
        "NVIDIA GeForce RTX 3060 Laptop GPU",
        4,
        neural::NVIDIA,
        0x2560,
    );
    *h.neural.streams_unknown.lock().unwrap() = true;
    *h.neural.adapters.lock().unwrap() = Some(vec![intel.clone(), rtx.clone()]);
    let shown = status(h, "missing");
    assert_eq!(shown["available"], true, "{shown}");
    assert_eq!(shown["gpu"], rtx.name.as_str());
    assert!(shown["gpu_note"].is_null(), "{shown}");
    // Held only for a while, as a failure is: the GPU that streams may be
    // found later, and the note with it.
    assert!(matches!(h.neural.lock().probe, Some((.., Ok(_), false))));
    *h.neural.adapters.lock().unwrap() = Some(vec![intel]);
    h.neural.lock().probe = None;
    let shown = status(h, "missing");
    assert_eq!(shown["available"], false, "{shown}");
    assert_eq!(shown["unavailable"], "gpu");
    assert!(
        shown["reason"].as_str().unwrap().contains("can't be found"),
        "{shown}"
    );
}

/// Runs `release` once `done` is sent, or after a while all the same, so
/// a test of something that should not wait fails rather than hangs.
fn release_later(release: impl FnOnce() + Send + 'static) -> mpsc::Sender<()> {
    let (done, wait) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        let _ = wait.recv_timeout(Duration::from_secs(8));
        release();
    });
    done
}

/// Waits for `uuid`'s game check to end.
fn check_ended(h: &Shared, uuid: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while h.neural.lock().checking.contains(uuid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!h.neural.lock().checking.contains(uuid));
}

/// Holds `uuid`'s game folder lookup and check as made longer ago than
/// [`GAME_RECHECK`].
fn expire(h: &Shared, uuid: &str) {
    let old = Instant::now()
        .checked_sub(GAME_RECHECK + Duration::from_secs(1))
        .unwrap();
    let mut state = h.neural.lock();
    state.lookups.get_mut(uuid).unwrap().at = old;
    state.games.get_mut(uuid).unwrap().0 = old;
}

/// A game folder on a share that stalls holds no status read: the game's
/// program and the Visual C++ runtime beside it are looked for in the
/// check's thread, the read answers within its wait without calling the
/// runtime missing before the folder is looked into, and the thread's
/// answer serves the next read.
#[test]
fn a_stalled_game_folder_never_holds_a_status_read() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    *h.neural.streams.lock().unwrap() = Some(neural::Adapter {
        name: "NVIDIA GeForce RTX 2060".into(),
        luid: 7,
        vendor: neural::NVIDIA,
        device: 0x1F08,
    });
    h.neural
        .vc_missing
        .lock()
        .unwrap()
        .push("VCRUNTIME140_1.dll");
    let temp = tempdir();
    let folder = temp.path().join("stalled");
    std::fs::create_dir_all(&folder).unwrap();
    let exe = folder.join("Game.exe");
    std::fs::write(&exe, "MZ game").unwrap();
    std::fs::write(folder.join("VCRUNTIME140_1.dll"), "runtime").unwrap();
    butterpollo_stream::steam::update_library(h, |apps, _| {
        apps.push(json!({"name": "Stalled", "uuid": "stalled", "cmd": exe, "launcher-install-dir": folder}));
        true
    })
    .unwrap();
    let stall = butterpollo_stream::neural::Stall::new(&folder);
    let done = release_later(move || drop(stall));
    let started = Instant::now();
    let shown = status_waiting(h, "stalled", Duration::from_millis(300));
    let waited = started.elapsed();
    let _ = done.send(());
    assert!(waited < Duration::from_secs(4), "{waited:?}");
    // Not known while the folder is looked into: no runtime refusal then.
    assert_eq!(shown["available"], true, "{shown}");
    assert!(shown["game"].is_null(), "{shown}");
    let shown = status(h, "stalled");
    assert_eq!(shown["available"], true, "{shown}");
    assert_eq!(shown["game"]["why"], "no_frame_generation", "{shown}");
}

/// A game's own check held longer than [`GAME_RECHECK`] still answers
/// while it is asked again: a refused game never shows as one that can
/// have it for a check that takes longer than the read waits.
#[test]
fn an_expired_game_check_answers_while_it_is_asked_again() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    let temp = tempdir();
    checked_game(&f, temp.path());
    assert_eq!(status(h, "without")["game"]["why"], "no_frame_generation");
    assert_eq!(game_checks(h), 1);
    expire(h, "without");
    let (release, gate) = mpsc::channel::<()>();
    *h.neural.game_gate.lock().unwrap() = Some(gate);
    let done = release_later(move || drop(release));
    let started = Instant::now();
    let shown = status_waiting(h, "without", Duration::from_millis(200));
    let waited = started.elapsed();
    let _ = done.send(());
    assert_eq!(shown["game"]["why"], "no_frame_generation", "{shown}");
    assert!(waited < Duration::from_secs(4), "{waited:?}");
    check_ended(h, "without");
    assert_eq!(game_checks(h), 2);
}

/// A GPU answer due to be asked again is given to a read while the check
/// runs in the background, and the new one serves the next read.
#[test]
fn a_gpu_answer_due_again_is_given_while_it_is_asked_again() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    let shown = status(h, "missing");
    assert_eq!(shown["available"], false, "{shown}");
    *h.neural.probe.lock().unwrap() = Some(Probe {
        gpus: vec![Gpu {
            name: "AMD Radeon RX 7900 XT".into(),
            arch: "gfx1100".into(),
            compute_units: 84,
            luid: None,
        }],
        error: None,
    });
    if let Some((at, ..)) = &mut h.neural.lock().probe {
        *at = Instant::now()
            .checked_sub(PROBE_RETRY + Duration::from_secs(1))
            .unwrap();
    }
    let (release, gate) = mpsc::channel::<()>();
    *h.neural.probe_gate.lock().unwrap() = Some(gate);
    let done = release_later(move || drop(release));
    let started = Instant::now();
    let shown = status_waiting(h, "missing", Duration::ZERO);
    let waited = started.elapsed();
    let _ = done.send(());
    assert!(waited < Duration::from_secs(4), "{waited:?}");
    assert_eq!(shown["available"], false, "{shown}");
    let deadline = Instant::now() + Duration::from_secs(20);
    while h.neural.lock().probing && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let shown = status(h, "missing");
    assert_eq!(shown["available"], true, "{shown}");
}

/// A stream holds a game's own check at its last answer, however old: no
/// helper starts beside the streamed game, and a game never checked gets
/// none until the stream ends.
#[test]
fn a_stream_holds_the_game_check_at_its_last_answer() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    let temp = tempdir();
    checked_game(&f, temp.path());
    assert_eq!(status(h, "without")["game"]["why"], "no_frame_generation");
    assert_eq!(game_checks(h), 1);
    f.session(
        f.client(u32::MAX),
        butterpollo_session::session::Role::Stream,
    );
    expire(h, "without");
    let shown = status(h, "without");
    assert_eq!(shown["game"]["why"], "no_frame_generation", "{shown}");
    let other = temp.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("Game.exe"), "MZ game").unwrap();
    butterpollo_stream::steam::update_library(h, |apps, _| {
        apps.push(json!({"name": "Other", "uuid": "other", "launcher-install-dir": other}));
        true
    })
    .unwrap();
    assert!(status(h, "other")["game"].is_null());
    check_ended(h, "other");
    assert_eq!(game_checks(h), 1);
    h.sessions.lock().unwrap().active.clear();
    assert_eq!(status(h, "other")["game"]["why"], "no_frame_generation");
    assert_eq!(game_checks(h), 2);
}
