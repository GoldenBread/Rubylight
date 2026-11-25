use super::*;
use std::collections::HashMap;

/// The global profile as a map: a missing key reads as the driver default.
#[derive(Default)]
struct Fake {
    stored: HashMap<u32, u32>,
    driver: Option<u32>,
    saves: usize,
}

impl Store for Fake {
    fn get(&mut self, id: u32) -> Result<Option<u32>> {
        Ok(self.stored.get(&id).copied())
    }
    fn set(&mut self, id: u32, value: u32) -> Result<()> {
        self.stored.insert(id, value);
        Ok(())
    }
    fn delete(&mut self, id: u32) -> Result<()> {
        self.stored.remove(&id);
        Ok(())
    }
    fn save(&mut self) -> Result<()> {
        self.saves += 1;
        Ok(())
    }
    fn driver(&mut self) -> Option<u32> {
        self.driver
    }
}

fn find<'a>(settings: &'a [Setting], id: &str) -> &'a Setting {
    settings
        .iter()
        .find(|s| s.id == id)
        .unwrap_or_else(|| panic!("no {id}"))
}

fn value_of(store: &mut Fake, id: &str) -> Value {
    find(&read(store).unwrap(), id).value.clone().unwrap()
}

#[test]
fn an_untouched_driver_reads_as_the_control_panels_defaults() {
    let mut store = Fake {
        driver: Some(58102),
        ..Fake::default()
    };
    let settings = read(&mut store).unwrap();
    let expected = [
        ("nvidia_low_latency.mode", Value::Choice("off".into())),
        ("nvidia_frame_rate_limit.fps", Value::Int(0)),
        ("nvidia_vsync.mode", Value::Choice("application".into())),
        ("nvidia_power.mode", Value::Choice("optimal_power".into())),
        (
            "nvidia_texture_filtering.quality",
            Value::Choice("quality".into()),
        ),
        (
            "nvidia_anisotropic_filtering.mode",
            Value::Choice("application".into()),
        ),
        ("nvidia_smooth_motion.enabled", Value::Bool(false)),
    ];
    assert_eq!(settings.len(), expected.len());
    for (id, value) in expected {
        let setting = find(&settings, id);
        assert_eq!(setting.value.as_ref(), Some(&value), "{id}");
        assert_eq!(setting.state, State::Configurable, "{id}");
        if setting.kind == Kind::Choice {
            let Value::Choice(name) = &value else {
                unreachable!()
            };
            assert!(setting.options.iter().any(|o| &o.value == name), "{id}");
        }
    }
    // The groups are NVIDIA's own, so no Radeon id or saved Radeon value ever reaches them.
    assert!(
        settings
            .iter()
            .all(|s| s.group.starts_with("nvidia_") && s.id.starts_with(s.group))
    );
}

#[test]
fn every_offered_value_writes_and_reads_back() {
    let mut store = Fake::default();
    for setting in read(&mut store).unwrap() {
        let values: Vec<Value> = match setting.kind {
            Kind::Choice => setting
                .options
                .iter()
                .map(|o| Value::Choice(o.value.clone()))
                .collect(),
            Kind::Toggle => vec![Value::Bool(true), Value::Bool(false)],
            Kind::Integer => vec![Value::Int(141), Value::Int(1023), Value::Int(0)],
            Kind::Action => vec![],
        };
        for value in values {
            write(&mut store, setting.id, &value).unwrap();
            assert_eq!(value_of(&mut store, setting.id), value, "{}", setting.id);
        }
    }
}

/// The three keys of Low Latency Mode, as the NVIDIA Control Panel writes them.
#[test]
fn low_latency_ultra_sets_the_control_panels_three_keys() {
    let mut store = Fake::default();
    write(
        &mut store,
        "nvidia_low_latency.mode",
        &Value::Choice("ultra".into()),
    )
    .unwrap();
    assert_eq!(store.stored[&PRERENDERLIMIT], 1);
    assert_eq!(store.stored[&ULL_CPL_STATE], 2);
    assert_eq!(store.stored[&ULL_ENABLED], 1);
    assert_eq!(store.saves, 1);
    write(
        &mut store,
        "nvidia_low_latency.mode",
        &Value::Choice("off".into()),
    )
    .unwrap();
    assert_eq!(
        [PRERENDERLIMIT, ULL_CPL_STATE, ULL_ENABLED].map(|k| store.stored[&k]),
        [0, 0, 0]
    );
}

/// What the panel can't write back exactly stays untouched: a pre-rendered limit of 3, a
/// half-rate V-Sync, an anisotropic override with an odd level.
#[test]
fn values_outside_the_options_are_read_only_custom() {
    let mut store = Fake::default();
    store.stored.insert(PRERENDERLIMIT, 3);
    store.stored.insert(VSYNCMODE, 0x3261_0244);
    store.stored.insert(ANISO_MODE_SELECTOR, 1);
    store.stored.insert(ANISO_MODE_LEVEL, 3);
    store.stored.insert(PREFERRED_PSTATE, 4);
    let settings = read(&mut store).unwrap();
    for id in [
        "nvidia_low_latency.mode",
        "nvidia_vsync.mode",
        "nvidia_anisotropic_filtering.mode",
        "nvidia_power.mode",
    ] {
        let setting = find(&settings, id);
        assert_eq!(setting.value, Some(Value::Choice("custom".into())), "{id}");
        assert_eq!(setting.state, State::ReadOnly, "{id}");
        assert!(!setting.options.iter().any(|o| o.value == "custom"), "{id}");
        assert!(
            setting.notes.iter().any(|n| n.contains("leaves it")),
            "{id}"
        );
    }
}

#[test]
fn smooth_motion_needs_driver_571_86_and_keeps_a_chosen_api_mask() {
    let mut store = Fake {
        driver: Some(56694),
        ..Fake::default()
    };
    let smooth = find(&read(&mut store).unwrap(), "nvidia_smooth_motion.enabled").clone();
    assert_eq!(smooth.state, State::NeedsExternalSetup);
    assert!(
        smooth.notes.iter().any(|n| n.contains("566.94")),
        "{:?}",
        smooth.notes
    );

    let mut store = Fake {
        driver: Some(57186),
        ..Fake::default()
    };
    assert_eq!(
        find(&read(&mut store).unwrap(), "nvidia_smooth_motion.enabled").state,
        State::Configurable
    );
    write(
        &mut store,
        "nvidia_smooth_motion.enabled",
        &Value::Bool(true),
    )
    .unwrap();
    assert_eq!(store.stored[&SMOOTH_MOTION_APIS], SMOOTH_MOTION_ALL_APIS);
    store.stored.insert(SMOOTH_MOTION_APIS, 2);
    write(
        &mut store,
        "nvidia_smooth_motion.enabled",
        &Value::Bool(false),
    )
    .unwrap();
    write(
        &mut store,
        "nvidia_smooth_motion.enabled",
        &Value::Bool(true),
    )
    .unwrap();
    assert_eq!(
        store.stored[&SMOOTH_MOTION_APIS], 2,
        "a mask set elsewhere stays"
    );
}

#[test]
fn values_the_setting_does_not_take_are_refused_before_anything_is_written() {
    let mut store = Fake::default();
    for (id, value) in [
        ("nvidia_vsync.mode", Value::Choice("custom".into())),
        ("nvidia_frame_rate_limit.fps", Value::Int(1024)),
        ("nvidia_frame_rate_limit.fps", Value::Bool(true)),
        (
            "nvidia_anisotropic_filtering.mode",
            Value::Choice("3x".into()),
        ),
        ("anti_lag.level", Value::Choice("on".into())),
    ] {
        assert!(write(&mut store, id, &value).is_err(), "{id} {value:?}");
    }
    assert!(store.stored.is_empty());
    assert_eq!(store.saves, 0);
}

/// The Radeon controller journals and restores NVIDIA settings unchanged: a stream's write is
/// put back at its end.
#[test]
fn the_controller_journals_and_restores_nvidia_settings() {
    use crate::radeon::{Change, Controller, Scope};
    struct Backend(std::sync::Arc<std::sync::Mutex<Fake>>);
    impl RadeonBackend for Backend {
        fn read(&mut self, _: i32) -> Result<Vec<Setting>, BackendError> {
            read(&mut *self.0.lock().unwrap()).map_err(|e| BackendError(e.to_string()))
        }
        fn write(&mut self, _: i32, id: &str, value: &Value) -> Result<(), BackendError> {
            write(&mut *self.0.lock().unwrap(), id, value)
        }
        fn snapshot_keys(
            &mut self,
            _: i32,
            id: &str,
        ) -> Result<Option<Vec<(u32, Option<u32>)>>, BackendError> {
            snapshot_keys(&mut *self.0.lock().unwrap(), id)
        }
        fn restore_raw(&mut self, entry: &Entry) -> Result<bool, BackendError> {
            restore_raw_keys(&mut *self.0.lock().unwrap(), entry)
        }
        fn gpus(&mut self) -> Result<Vec<GpuInfo>, BackendError> {
            Ok(vec![GpuInfo {
                id: 0,
                name: "NVIDIA GeForce RTX 5070".into(),
                driver: "581.02".into(),
            }])
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let mut store = Fake::default();
    store.stored.insert(FRL_FPS, 90);
    let store = std::sync::Arc::new(std::sync::Mutex::new(store));
    let mut controller = Controller::new(Backend(store.clone()), dir.path().join(JOURNAL_FILE));
    let changes = vec![
        Change {
            id: "nvidia_low_latency.mode".into(),
            value: Value::Choice("ultra".into()),
        },
        Change {
            id: "nvidia_frame_rate_limit.fps".into(),
            value: Value::Int(141),
        },
    ];
    let after = controller.apply("session", 0, &changes).unwrap();
    assert_eq!(
        find(&after, "nvidia_frame_rate_limit.fps").value,
        Some(Value::Int(141))
    );
    controller.restore(Scope::Session("session")).unwrap();
    let settings = controller.backend().read(0).unwrap();
    assert_eq!(
        find(&settings, "nvidia_frame_rate_limit.fps").value,
        Some(Value::Int(90))
    );
    assert_eq!(
        find(&settings, "nvidia_low_latency.mode").value,
        Some(Value::Choice("off".into()))
    );
    // Absent DRS keys restore via deletion rather than writing explicit defaults, and
    // companion keys (e.g. SMOOTH_MOTION_APIS, ANISO_MODE_LEVEL) restore cleanly too.
    assert!(
        !store.lock().unwrap().stored.contains_key(&PRERENDERLIMIT),
        "absent low-latency keys must be deleted rather than written as defaults"
    );
}

#[test]
fn restore_deletes_absent_keys_and_preserves_companions_raw_frl_and_machine_scope() {
    use crate::radeon::{Change, Controller, Scope};
    let dir = tempfile::tempdir().unwrap();
    let journal = dir.path().join(JOURNAL_FILE);
    let store = std::sync::Arc::new(std::sync::Mutex::new(Fake::default()));
    {
        let mut s = store.lock().unwrap();
        s.driver = Some(58102);
        // Original frame rate cap above UI 0x3FF ceiling (HG-17), plus absent Smooth Motion &
        // Anisotropic Filtering keys (HG-16).
        s.stored.insert(FRL_FPS, 1200);
    }
    struct Backend(std::sync::Arc<std::sync::Mutex<Fake>>);
    impl RadeonBackend for Backend {
        fn read(&mut self, _: i32) -> Result<Vec<Setting>, BackendError> {
            read(&mut *self.0.lock().unwrap()).map_err(|e| BackendError(e.to_string()))
        }
        fn write(&mut self, _: i32, id: &str, value: &Value) -> Result<(), BackendError> {
            write(&mut *self.0.lock().unwrap(), id, value)
        }
        fn snapshot_keys(
            &mut self,
            _: i32,
            id: &str,
        ) -> Result<Option<Vec<(u32, Option<u32>)>>, BackendError> {
            snapshot_keys(&mut *self.0.lock().unwrap(), id)
        }
        fn restore_raw(&mut self, entry: &Entry) -> Result<bool, BackendError> {
            restore_raw_keys(&mut *self.0.lock().unwrap(), entry)
        }
        fn gpus(&mut self) -> Result<Vec<GpuInfo>, BackendError> {
            Ok(vec![GpuInfo {
                id: 0,
                name: "NVIDIA GeForce RTX 5070".into(),
                driver: "581.02".into(),
            }])
        }
    }

    // Machine-scoped controller applies & restores even when nobody is signed in (HG-18).
    let mut controller = Controller::new(Backend(store.clone()), journal.clone())
        .with_machine_scope(true)
        .with_user(|| None);
    let changes = vec![
        Change {
            id: "nvidia_smooth_motion.enabled".into(),
            value: Value::Bool(true),
        },
        Change {
            id: "nvidia_anisotropic_filtering.mode".into(),
            value: Value::Choice("8x".into()),
        },
        Change {
            id: "nvidia_frame_rate_limit.fps".into(),
            value: Value::Int(120),
        },
    ];
    controller.apply("s1", 0, &changes).unwrap();
    {
        let s = store.lock().unwrap();
        assert_eq!(s.stored[&SMOOTH_MOTION], 1);
        assert_eq!(s.stored[&SMOOTH_MOTION_APIS], SMOOTH_MOTION_ALL_APIS);
        assert_eq!(s.stored[&ANISO_MODE_SELECTOR], 1);
        assert_eq!(s.stored[&ANISO_MODE_LEVEL], 8);
        assert_eq!(s.stored[&FRL_FPS], 120);
    }

    // New controller instance with a different (or absent) user recovers everything cleanly.
    let mut recovery = Controller::new(Backend(store.clone()), journal)
        .with_machine_scope(true)
        .with_user(|| Some("S-1-5-21-999-888".into()));
    let report = recovery.recover_on_startup().unwrap();
    assert!(report.is_clean(), "{report:?}");
    let s = store.lock().unwrap();
    assert!(!s.stored.contains_key(&SMOOTH_MOTION));
    assert!(!s.stored.contains_key(&SMOOTH_MOTION_APIS));
    assert!(!s.stored.contains_key(&ANISO_MODE_SELECTOR));
    assert!(!s.stored.contains_key(&ANISO_MODE_LEVEL));
    assert_eq!(s.stored.get(&FRL_FPS).copied(), Some(1200));

    // Pre-existing/legacy journal entries with ANISO changed -> "application" and out-of-range
    // FRL_FPS without `raw` snapshot also restore raw values cleanly.
    drop(s);
    store.lock().unwrap().stored.insert(ANISO_MODE_LEVEL, 16);
    store.lock().unwrap().stored.insert(ANISO_MODE_SELECTOR, 1);
    let legacy_entry = Entry {
        gpu: 0,
        id: "nvidia_frame_rate_limit.fps".into(),
        session: "legacy".into(),
        original: Value::Int(1440),
        written: Value::Int(120),
        side_effect: false,
        user: "S-1-5-21-old-user".into(),
        raw: None,
    };
    store.lock().unwrap().stored.insert(FRL_FPS, 120);
    let mut legacy_journal = crate::radeon::Journal::default();
    legacy_journal.entries.push(legacy_entry);
    let legacy_path = dir.path().join("legacy-nvidia.json");
    legacy_journal.save(&legacy_path).unwrap();
    let mut legacy_ctrl = Controller::new(Backend(store.clone()), legacy_path)
        .with_machine_scope(true)
        .with_user(|| None);
    let report = legacy_ctrl.restore(Scope::All).unwrap();
    assert!(report.is_clean(), "{report:?}");
    assert_eq!(
        store.lock().unwrap().stored.get(&FRL_FPS).copied(),
        Some(1440)
    );
}
