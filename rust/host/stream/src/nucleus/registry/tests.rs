//! Unit tests for the Nucleus Co-op session registry.

use super::*;

#[test]
fn registry_leave_releases_held_controller_before_marking_away() {
    let pc = Arc::new(fake::Pc::installed());
    let registry = Registry::new(pc.clone());
    let apps = vec![App {
        name: "Portal 2".into(),
        cmd: format!("\"{}\" -novid", fake::EXE),
        working_dir: String::new(),
        prep: Vec::new(),
        computed_id: None,
        aliases: Vec::new(),
        extra: BTreeMap::new(),
    }];
    let coop = registry.entries(&apps, "").remove(0);
    let client: Client = serde_json::from_value(serde_json::json!({
        "name": "Device A",
        "cert": "",
        "uuid": "A",
        "perm": butterpollo_session::state::PLAY_DEVICE_PERMISSION,
    }))
    .unwrap();
    let seat = registry
        .admit(Admission {
            launch: "a",
            client: &client,
            game: Game::Coop(&coop),
            size: (1920, 1080),
            config: &Config::default(),
            warnings: Default::default(),
            release_parked: &|| Ok(()),
            instances: None,
        })
        .unwrap();
    let instance = registry.instance(seat.session).unwrap();
    instance
        .input(
            "A",
            Input::Controller {
                id: 0,
                active: 1,
                buttons: 0x0010,
                left_trigger: 128,
                right_trigger: 64,
                sticks: [1234, -5678, 0, 0],
            },
        )
        .unwrap();

    registry.leave(seat, "a");

    let inputs = pc.last().inputs.lock().unwrap().clone();
    assert_eq!(inputs.len(), 2);
    assert_eq!(inputs[1].0, seat.slot);
    assert!(matches!(
        inputs[1].1,
        Input::Controller {
            buttons: 0,
            left_trigger: 0,
            right_trigger: 0,
            sticks: [0, 0, 0, 0],
            ..
        }
    ));
}

#[cfg(windows)]
#[test]
fn game_folder_probes_always_run_under_user_impersonation_and_refuse_when_unavailable() {
    use std::sync::atomic::Ordering;
    let dir = std::env::temp_dir().join(format!("nucleus-probe-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("indie.exe");
    std::fs::write(&exe, b"MZ").unwrap();
    let app = App {
        name: "Indie".into(),
        cmd: format!("\"{}\"", exe.display()),
        working_dir: String::new(),
        prep: Vec::new(),
        computed_id: None,
        aliases: Vec::new(),
        extra: BTreeMap::new(),
    };

    let pc = Arc::new(fake::Pc::installed());
    let registry = Registry::new(pc.clone());
    // Clear hub catalog so `own_status` runs the game-folder probes.
    pc.catalog.lock().unwrap().games.clear();

    // When impersonation fails, probes refuse without touching disk and are not cached as lasting.
    *pc.user_probe_error.lock().unwrap() = Some("nobody is signed in to Windows".into());
    let refused = registry.own_status(&app).unwrap_err();
    assert!(
        matches!(refused, Missing::Refused { ref reason } if reason.contains("signed-in user")),
        "{refused:?}"
    );
    assert!(
        pc.user_probes.load(Ordering::Relaxed) >= 2,
        "expected metadata + unreal_shipping probes to attempt user impersonation"
    );

    // Once a user is signed in, the same app probes cleanly (was not cached as a lasting error).
    *pc.user_probe_error.lock().unwrap() = None;
    let before = pc.user_probes.load(Ordering::Relaxed);
    let run = registry.own_status(&app).unwrap();
    assert!(run.is_some());
    assert!(
        pc.user_probes.load(Ordering::Relaxed) >= before + 4,
        "expected metadata, unreal_shipping, is_file, and anti_cheat to all run under as_game_user"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_background_refusal_reads_on_after_its_colon() {
    let mid = super::admit::mid_sentence;
    assert_eq!(
        mid("The co-op helper is not ready: x"),
        "the co-op helper is not ready: x"
    );
    assert_eq!(
        mid("Co-op sessions need the service"),
        "co-op sessions need the service"
    );
    assert_eq!(
        mid("Portal 2 refused its hooks"),
        "Portal 2 refused its hooks"
    );
}
