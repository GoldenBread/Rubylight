//! Registry fixture and admission helper for Nucleus Co-op tests.
use super::{Admission, App, Client, Config, CoopApp, Game, Refusal, Registry, Seat, app, fake};
use std::sync::Arc;

pub(super) fn setup() -> (Registry, Arc<fake::Pc>, Vec<App>) {
    let pc = Arc::new(fake::Pc::installed());
    let apps = vec![
        app("Desktop", ""),
        app("Portal 2", &format!("\"{}\" -novid", fake::EXE)),
        app("Steam", "steam://open/bigpicture"),
    ];
    (Registry::new(pc.clone()), pc, apps)
}

pub(super) fn admit(
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
