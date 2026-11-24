//! Hardware profile, anti-lag, snapshot restore and configurable setting round-trip tests.

use super::*;

/// Read-only, and deliberately in this process: `AdlxBackend` opens an ADLX session per call, so
/// many in a row must stay stable.
#[test]
#[ignore = "talks to the driver"]
fn session_churn_is_stable() {
    let _exclusive = exclusive();
    let gpu = first_gpu();
    let first = AdlxBackend.read(gpu).expect("read");
    for round in 0..300 {
        let again = AdlxBackend
            .read(gpu)
            .unwrap_or_else(|e| panic!("round {round}: {e}"));
        assert!(diff(&first, &again).is_empty(), "round {round}");
    }
}

/// The path the native API takes (plan A3 brief 4): the worker thread owns the controller. One
/// real read through `Worker::panel`, one write of `anti_lag.enabled` (proven to round-trip) and
/// the restore of that session, with the driver checked after each step.
#[test]
#[ignore = "changes real driver settings"]
fn worker_writes_and_restores_anti_lag() {
    let _exclusive = exclusive();
    assert_host_idle();
    let gpu = first_gpu();
    let start = helper().read(gpu).expect("read");
    let _restore = Restore::new(gpu, &start);
    let dir = scratch_dir("worker");
    let worker = Worker::start(helper(), dir.join(JOURNAL_FILE), Timing::default());
    let until = std::time::Instant::now() + Duration::from_secs(60);
    while worker.status().phase == Phase::Recovering {
        assert!(
            std::time::Instant::now() < until,
            "the recovery never ended"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let panel = worker
        .panel(None, Duration::from_secs(60))
        .expect("panel")
        .expect("an AMD GPU");
    assert_eq!(panel.gpu.id, gpu);
    assert!(panel.pending.is_empty());
    println!(
        "gpu {} {} driver {}",
        panel.gpu.id, panel.gpu.name, panel.gpu.driver
    );
    let anti_lag = panel
        .settings
        .iter()
        .find(|s| s.id == "anti_lag.enabled")
        .expect("anti_lag.enabled");
    assert_eq!(anti_lag.state, State::Configurable);
    let original = anti_lag.value.clone().expect("a value");
    let Value::Bool(on) = original else {
        panic!("anti_lag.enabled is not a toggle");
    };
    let written = Value::Bool(!on);
    println!(
        "anti_lag.enabled: {} -> {}",
        original.display(),
        written.display()
    );
    let after = worker
        .apply(
            "hw-session",
            gpu,
            vec![Change {
                id: "anti_lag.enabled".into(),
                value: written.clone(),
            }],
        )
        .expect("apply");
    assert_eq!(
        after
            .iter()
            .find(|s| s.id == "anti_lag.enabled")
            .and_then(|s| s.value.clone()),
        Some(written.clone())
    );
    assert_eq!(
        helper().read_value(gpu, "anti_lag.enabled").expect("read"),
        written
    );
    let panel = worker
        .panel(None, Duration::from_secs(60))
        .expect("panel")
        .expect("an AMD GPU");
    let entry = panel
        .pending
        .iter()
        .find(|e| e.id == "anti_lag.enabled")
        .expect("the change is journaled");
    assert_eq!((&entry.original, &entry.written), (&original, &written));
    let report = worker.restore_session("hw-session").expect("restore");
    assert!(report.is_clean(), "{:?}", report.outcomes);
    assert_eq!(
        helper().read_value(gpu, "anti_lag.enabled").expect("read"),
        original
    );
    let moved = diff(&start, &helper().read(gpu).expect("read"));
    assert!(
        moved.is_empty(),
        "after the restore the settings differ: {moved:?}"
    );
    assert!(
        worker
            .panel(None, Duration::from_secs(60))
            .expect("panel")
            .expect("an AMD GPU")
            .pending
            .is_empty()
    );
    println!(
        "anti_lag.enabled written, read back and restored to {}",
        original.display()
    );
}

/// The per-game profiles of plan B5 (`butterpollo_media_policy::radeon_profile`) against the real
/// driver: each named profile is written as the host writes it at launch (the stream's 60 fps
/// in place of `$stream_fps`), every value is read back, and the session's restore must leave
/// the driver as it was found.
#[test]
#[ignore = "changes real driver settings"]
fn game_profiles_apply_and_restore() {
    use butterpollo_media_policy::radeon_profile::selected;
    let _exclusive = exclusive();
    assert_host_idle();
    let gpu = first_gpu();
    let start = helper().read(gpu).expect("read");
    let _restore = Restore::new(gpu, &start);
    let dir = scratch_dir("profiles");
    let worker = Worker::start(helper(), dir.join(JOURNAL_FILE), Timing::default());
    let until = std::time::Instant::now() + Duration::from_secs(60);
    while worker.status().phase == Phase::Recovering {
        assert!(
            std::time::Instant::now() < until,
            "the recovery never ended"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    for name in ["competitive", "smoothness", "quality"] {
        let extra = std::collections::BTreeMap::from([(
            "radeon-profile".to_owned(),
            serde_json::json!(name),
        )]);
        let profile = selected(&extra, Some(60))
            .expect("valid")
            .expect("a profile");
        let changes: Vec<Change> = profile
            .changes
            .iter()
            .map(|item| Change {
                id: item.id.clone(),
                value: match &item.value {
                    serde_json::Value::Bool(on) => Value::Bool(*on),
                    serde_json::Value::Number(n) => Value::Int(n.as_i64().expect("whole")),
                    serde_json::Value::String(text) => Value::Choice(text.clone()),
                    other => panic!("{other}"),
                },
            })
            .collect();
        let session = format!("hw-profile-{name}");
        let after = worker
            .apply(&session, gpu, changes.clone())
            .unwrap_or_else(|e| panic!("{name}: the driver refused the profile: {e}"));
        for change in &changes {
            let now = after
                .iter()
                .find(|s| s.id == change.id)
                .and_then(|s| s.value.clone());
            println!(
                "{name}: {} = {:?}",
                change.id,
                now.as_ref().map(Value::display)
            );
            assert_eq!(now.as_ref(), Some(&change.value), "{name}: {}", change.id);
            assert_eq!(
                helper().read_value(gpu, &change.id).expect("read"),
                change.value,
                "{name}: {} read in a new process",
                change.id
            );
        }
        let report = worker.restore_session(&session).expect("restore");
        assert!(report.is_clean(), "{name}: {:?}", report.outcomes);
        let moved = diff(&start, &helper().read(gpu).expect("read"));
        assert!(moved.is_empty(), "{name}: after the restore: {moved:?}");
        println!("{name}: applied, read back and restored");
    }
}

/// Safety net: write back every setting to the values in `RADEON_SNAPSHOT` (default the file
/// `snapshot` leaves behind).
#[test]
#[ignore = "changes real driver settings"]
fn restore_snapshot() {
    let _exclusive = exclusive();
    assert_host_idle();
    let path = std::env::var("RADEON_SNAPSHOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| out_path("rl-a5-snapshot.json"));
    let wanted: std::collections::BTreeMap<String, Value> =
        serde_json::from_slice(&std::fs::read(&path).expect("snapshot")).expect("snapshot json");
    let gpu = first_gpu();
    let restore = Restore {
        gpu,
        originals: wanted.into_iter().collect(),
    };
    println!("differing before: {:?}", restore.differing());
}

/// Every `configurable` setting: write a different valid value through the journal, read it back,
/// restore, and check nothing else moved. Ends with an empty probe diff.
#[test]
#[ignore = "changes real driver settings"]
fn prove_configurable_settings_round_trip() {
    let _exclusive = exclusive();
    assert_host_idle();
    let gpu = first_gpu();
    let start = helper().read(gpu).expect("read");
    let _restore = Restore::new(gpu, &start);
    snapshot(&start);
    let dir = scratch_dir("prove");
    let journal = dir.join(JOURNAL_FILE);
    let mut controller = Controller::new(helper(), journal.clone());
    let mut proofs = vec![];
    for setting in start.iter().filter(|s| s.state == State::Configurable) {
        if setting.kind == Kind::Action {
            proofs.push(Proof {
                id: setting.id,
                kind: setting.kind,
                proven_writable: "action",
                original: None,
                tried: None,
                detail: "a destructive action, not a setting; deliberately never written".into(),
            });
            continue;
        }
        assert_host_idle();
        let Some(tried) = alternative(setting) else {
            proofs.push(Proof {
                id: setting.id,
                kind: setting.kind,
                proven_writable: "no",
                original: setting.value.clone(),
                tried: None,
                detail: "no second valid value to write".into(),
            });
            continue;
        };
        let mut refusals: Vec<String> = vec![];
        let mut outcome = None;
        for changes in &attempts(setting, &start, &tried) {
            match controller.apply("prove", gpu, changes) {
                Ok(after) => {
                    let read_back = after
                        .iter()
                        .find(|s| s.id == setting.id)
                        .and_then(|s| s.value.clone());
                    let report = controller
                        .restore(Scope::Session("prove"))
                        .expect("restore");
                    assert!(
                        report.is_clean(),
                        "restoring {} failed: {:?}",
                        setting.id,
                        report.outcomes
                    );
                    let moved = diff(&start, &helper().read(gpu).expect("read"));
                    assert!(
                        moved.is_empty(),
                        "after restoring {} the settings differ: {moved:?}",
                        setting.id
                    );
                    let mut detail = format!(
                        "wrote {}, read back {}, restored {}",
                        tried.display(),
                        read_back.as_ref().map_or("nothing".into(), Value::display),
                        setting.value.as_ref().map_or("?".into(), Value::display)
                    );
                    if changes.len() > 1 {
                        detail.push_str(&format!(
                            "; only with {} on in the same request (alone: {})",
                            changes[0].id,
                            refusals.join("; ")
                        ));
                    }
                    outcome = Some(("yes", detail));
                    break;
                }
                Err(e) => {
                    let moved = diff(&start, &helper().read(gpu).expect("read"));
                    assert!(
                        moved.is_empty(),
                        "{} was refused ({e}) but the settings differ: {moved:?}",
                        setting.id
                    );
                    refusals.push(e.to_string());
                }
            }
        }
        let (proven_writable, detail) = outcome.unwrap_or(("no", refusals.join("; ")));
        println!("{:<40} {proven_writable:<6} {detail}", setting.id);
        proofs.push(Proof {
            id: setting.id,
            kind: setting.kind,
            proven_writable,
            original: setting.value.clone(),
            tried: Some(tried),
            detail,
        });
    }
    let journal = controller.journal().expect("journal");
    assert!(
        journal.entries.is_empty(),
        "journal left: {:?}",
        journal.entries
    );
    let end = helper().read(gpu).expect("read");
    let moved = diff(&start, &end);
    save("rl-a5-prove.json", &proofs);
    assert!(moved.is_empty(), "final probe diff is not empty: {moved:?}");
    println!(
        "final probe diff: empty ({} settings compared)",
        start.len()
    );
}

#[derive(Serialize)]
struct Exploration {
    id: &'static str,
    original: Option<Value>,
    /// Values the driver accepted and read back unchanged.
    round_trips: Vec<String>,
    /// Values the driver refused or kept another value for, with the answer.
    refused: Vec<String>,
    /// The current value could not be written back, so the host could never restore it.
    restorable: bool,
    promotable: bool,
}

/// The enum-backed `read_only` settings: ADLX cannot list the values a driver accepts, so each
/// SDK value is tried once and the setting goes back to its original after every attempt.
#[test]
#[ignore = "changes real driver settings"]
fn explore_enum_settings() {
    const CANDIDATES: &[&str] = &[
        "vsync.mode",
        "afmf.search_mode",
        "afmf.performance_mode",
        "afmf.fast_motion_response",
        "anti_aliasing.mode",
        "anti_aliasing.level",
        "anti_aliasing.method",
        "anisotropic_filtering.level",
        "tessellation.mode",
        "tessellation.level",
    ];
    let _exclusive = exclusive();
    assert_host_idle();
    let gpu = first_gpu();
    let start = helper().read(gpu).expect("read");
    let _restore = Restore::new(gpu, &start);
    snapshot(&start);
    // The AFMF modes answer ADLX_NOT_ACTIVE while AFMF is off; this run turns it on for a while.
    let afmf_on = std::env::var_os("RADEON_EXPLORE_AFMF_ON").is_some();
    let mut current = start.clone();
    if afmf_on {
        helper()
            .write(gpu, "afmf.enabled", &Value::Bool(true))
            .expect("enable AFMF");
        current = helper().read(gpu).expect("read");
    }
    let mut found = vec![];
    for id in CANDIDATES {
        let Some(setting) = current.iter().find(|s| s.id == *id) else {
            continue;
        };
        let (Some(original), Some(_)) = (setting.value.clone(), enum_items(id)) else {
            found.push(Exploration {
                id: setting.id,
                original: setting.value.clone(),
                round_trips: vec![],
                refused: vec!["not readable".into()],
                restorable: false,
                promotable: false,
            });
            continue;
        };
        let mut backend = helper();
        let mut result = Exploration {
            id: setting.id,
            original: Some(original.clone()),
            round_trips: vec![],
            refused: vec![],
            restorable: true,
            promotable: false,
        };
        // The host can only restore what it can write back.
        eprintln!("writing {id} = {} (its own value)", original.display());
        if let Err(e) = backend.write(gpu, id, &original) {
            result.restorable = false;
            result
                .refused
                .push(format!("original {}: {e}", original.display()));
            println!(
                "{id}: current value {} cannot be written back: {e}",
                original.display()
            );
            found.push(result);
            continue;
        }
        for option in setting
            .options
            .iter()
            .filter(|o| Value::Choice(o.value.clone()) != original)
        {
            assert_host_idle();
            let value = Value::Choice(option.value.clone());
            eprintln!("writing {id} = {}", value.display());
            match backend.write(gpu, id, &value) {
                Err(e) => result.refused.push(format!("{}: {e}", option.value)),
                Ok(()) => {
                    let now = backend
                        .read(gpu)
                        .ok()
                        .and_then(|s| s.into_iter().find(|s| s.id == *id)?.value);
                    if now.as_ref() == Some(&value) {
                        result.round_trips.push(option.value.clone());
                    } else {
                        result.refused.push(format!(
                            "{}: driver kept {}",
                            option.value,
                            now.as_ref().map_or("nothing".into(), Value::display)
                        ));
                    }
                }
            }
            backend.write(gpu, id, &original).unwrap_or_else(|e| {
                panic!("could not put {id} back to {}: {e}", original.display())
            });
            let back = backend
                .read(gpu)
                .ok()
                .and_then(|s| s.into_iter().find(|s| s.id == *id)?.value);
            assert_eq!(
                back.as_ref(),
                Some(&original),
                "{id} did not return to {}",
                original.display()
            );
        }
        result.promotable = result.restorable && !result.round_trips.is_empty();
        println!(
            "{id}: original {} round-trips {:?} refused {:?} promotable {}",
            original.display(),
            result.round_trips,
            result.refused,
            result.promotable
        );
        found.push(result);
    }
    if afmf_on {
        let original = start
            .iter()
            .find(|s| s.id == "afmf.enabled")
            .and_then(|s| s.value.clone())
            .expect("afmf.enabled");
        helper()
            .write(gpu, "afmf.enabled", &original)
            .expect("put AFMF back");
    }
    let moved = diff(&start, &helper().read(gpu).expect("read"));
    save(
        if afmf_on {
            "rl-a5-explore-afmf.json"
        } else {
            "rl-a5-explore.json"
        },
        &found,
    );
    assert!(moved.is_empty(), "probe diff after exploring: {moved:?}");
}
