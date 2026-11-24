//! Local launcher discovery, on Linux: Steam's libraries are read from its files, as on
//! Windows. The other launchers and the game folders (which hold Windows programs) are
//! reported unavailable, and starting an imported game is not available yet.
//! Scanning never starts a launcher or changes its files.

use anyhow::Result;
use butterpollo_session::library::{self, Game, Launch, Source};
use serde::Serialize;
use std::path::Path;

/// What the scan found out about one launcher.
#[derive(Serialize)]
pub struct SourceStatus {
    pub source: Source,
    pub name: &'static str,
    /// `disabled`, `available`, `unavailable` or `error`.
    pub state: &'static str,
    pub errors: Vec<String>,
}
/// The launchers the scan looked at and the games it found.
pub struct Discovery {
    pub sources: Vec<SourceStatus>,
    pub games: Vec<Game>,
}

impl Discovery {
    /// The sources the settings turned off, which the scan did not read.
    pub fn skipped(&self) -> impl Iterator<Item = Source> + '_ {
        self.sources
            .iter()
            .filter(|status| status.state == "disabled")
            .map(|status| status.source)
    }
}

/// Runs `work` and returns its result. On Windows the thread reads a library's files in
/// background mode; Linux lets a thread lower its priority but not raise it again without a
/// privilege, so `work` runs at the thread's own priority here.
pub fn in_background<T>(work: impl FnOnce() -> T) -> T {
    work()
}

/// A Steam game only counts while its install folder is there; its cover falls back to an
/// image in that folder.
fn add(mut game: Game, games: &mut Vec<Game>) {
    if !Path::new(&game.install_dir).is_dir() {
        return;
    }
    if game.cover.is_none() {
        game.cover = ["cover.png", "cover.jpg", "folder.jpg"]
            .iter()
            .map(|file| Path::new(&game.install_dir).join(file))
            .find(|file| file.is_file());
    }
    games.push(game);
}

/// The installed games of Steam's libraries below `roots`, without Steam's tools, each
/// started through Steam.
fn steam_games(roots: &[std::path::PathBuf], games: &mut Vec<Game>) {
    for game in butterpollo_session::steam::discover(roots) {
        if !butterpollo_session::steam::importable(&game, false) {
            continue;
        }
        let mut entry = Game::new(
            Source::Steam,
            game.appid.to_string(),
            game.name.clone(),
            Path::new(&game.install_dir),
            Some(Launch::Uri {
                uri: format!("steam://rungameid/{}", game.appid),
            }),
        );
        entry.cover = game.artwork().map(Path::to_path_buf);
        add(entry, games);
    }
}

/// Scan the launchers `settings` turns on. On Linux only Steam is read; every other source
/// that is on is reported `unavailable`, with no game.
pub fn discover(settings: &library::Settings) -> Discovery {
    let mut sources = vec![];
    let mut games = vec![];
    for source in Source::ALL {
        if !settings.enabled(source) {
            sources.push(SourceStatus {
                source,
                name: source.name(),
                state: "disabled",
                errors: vec![],
            });
            continue;
        }
        let available = match source {
            Source::Steam => {
                let roots = crate::steam::roots();
                steam_games(&roots, &mut games);
                !roots.is_empty()
            }
            Source::Epic
            | Source::Battlenet
            | Source::Ea
            | Source::Ubisoft
            | Source::Gog
            | Source::Microsoft
            | Source::Local => false,
        };
        sources.push(SourceStatus {
            source,
            name: source.name(),
            state: if available {
                "available"
            } else {
                "unavailable"
            },
            errors: vec![],
        });
    }
    Discovery {
        sources,
        games: library::finish(games),
    }
}

/// Start an imported game by its launcher address or program. Always an error on Linux,
/// where the host starts no launcher yet.
pub fn launch(
    _action: &Launch,
    _directory: Option<&Path>,
    _elevated: bool,
    _environment: &std::collections::BTreeMap<String, String>,
) -> Result<()> {
    Err(crate::unsupported("Starting a game through its launcher"))
}
