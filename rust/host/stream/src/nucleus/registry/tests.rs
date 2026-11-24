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
