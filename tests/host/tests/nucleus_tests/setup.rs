//! Registry fixture and admission helpers for Nucleus Co-op tests.
use butterpollo_config::config::Config;
use butterpollo_session::nucleus::install::{Problem, Unusable};
use butterpollo_session::session::Seat;
use butterpollo_session::state::{App, Client};
use butterpollo_stream::nucleus::{Admission, CoopApp, Game, Refusal, Registry, fake};
use std::{collections::BTreeMap, sync::Arc};

pub(crate) fn app(name: &str, cmd: &str) -> App {
    App {
        name: name.into(),
        cmd: cmd.into(),
        working_dir: String::new(),
        prep: Vec::new(),
        computed_id: None,
        aliases: Vec::new(),
        extra: BTreeMap::new(),
    }
}
pub(crate) fn client(uuid: &str, coop_join: bool) -> Client {
    let mut client: Client = serde_json::from_value(serde_json::json!({
        "name": format!("Device {uuid}"), "cert": "", "uuid": uuid,
        "perm": butterpollo_session::state::PLAY_DEVICE_PERMISSION,
    }))
    .unwrap();
    // Set either way: never set, a device that launches apps may join.
    client.extra.insert("coop_join".into(), coop_join.into());
    client
}

/// A chosen folder the fake PC refuses, as the PC would.
pub(crate) fn refuse(pc: &fake::Pc, path: &str, problem: Problem, message: &str) {
    pc.unusable.lock().unwrap().insert(
        path.into(),
        Unusable {
            problem,
            message: message.into(),
        },
    );
}

pub(crate) fn background(
    registry: &Registry,
    app: &App,
    launch: &str,
    client: &Client,
    config: &Config,
) -> Result<Seat, Refusal> {
    registry.admit(Admission {
        launch,
        client,
        game: Game::Background(app),
        size: (1280, 720),
        config,
        warnings: Default::default(),
        release_parked: &|| Ok(()),
        instances: None,
    })
}

pub(crate) fn setup() -> (Registry, Arc<fake::Pc>, Vec<App>) {
    let pc = Arc::new(fake::Pc::installed());
    let apps = vec![
        app("Desktop", ""),
        app("Portal 2", &format!("\"{}\" -novid", fake::EXE)),
        app("Steam", "steam://open/bigpicture"),
    ];
    (Registry::new(pc.clone()), pc, apps)
}

pub(crate) fn admit(
    registry: &Registry,
    coop: &CoopApp,
    launch: &str,
    client: &Client,
) -> Result<Seat, Refusal> {
    registry.admit(Admission {
        launch,
        client,
        game: Game::Coop(coop),
        size: (1920, 1080),
        config: &Config::default(),
        warnings: Default::default(),
        release_parked: &|| Ok(()),
        instances: None,
    })
}
