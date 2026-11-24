//! Local launcher discovery, on Linux: Steam's libraries are read from its files, as on
//! Windows, and so are Heroic's (Epic, GOG, Amazon and its own programs) and Lutris'. The
//! launchers Windows has and Linux has no counterpart of (Battle.net, EA, Ubisoft Connect,
//! the Microsoft Store) are reported unavailable, and the game folders hold Windows programs
//! the host cannot start by themselves. Scanning never starts a launcher or changes its
//! files.
//!
//! Where each source's games come from:
//! - Steam: `libraryfolders.vdf` and `appmanifest_*.acf` of the Steam installations
//!   ([`crate::steam::roots`]), without Steam's tools; covers from its library cache.
//! - Epic and GOG: Heroic's installed games. Amazon games and programs added to Heroic go
//!   under the game folders (`local`), since the host has no source of its own for them.
//! - Lutris' installed games go under the game folders as well.

use anyhow::Result;
use butterpollo_session::library::{self, Game, Launch, Source};
use serde::Serialize;
use std::path::Path;

mod heroic;
mod lutris;
mod sqlite;
mod start;

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

/// A game only counts while its install folder is there; its cover falls back to an image
/// in that folder.
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

/// Marks `game` as not startable when the launcher of its address is not installed.
fn check_launcher(game: &mut Game) {
    if let Some(Launch::Uri { uri }) = &game.launch
        && let Some((scheme, _)) = uri.split_once(':')
        && !start::installed(scheme)
    {
        game.unavailable_reason = Some(format!(
            "Install {} to launch this game.",
            match scheme {
                "steam" => "Steam",
                "heroic" => "Heroic Games Launcher",
                _ => "Lutris",
            }
        ));
    }
}

/// The installed games of Steam's libraries below `roots`, without Steam's tools, each
/// started through Steam.
fn steam_games(roots: &[std::path::PathBuf], games: &mut Vec<Game>) {
    for game in crate::steam::discover(roots) {
        if !crate::steam::importable(&game) {
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
        check_launcher(&mut entry);
        add(entry, games);
    }
}

/// The Heroic games whose runner belongs to `source`.
fn heroic_games(source: Source, games: &mut Vec<Game>, errors: &mut Vec<String>) -> bool {
    let roots = heroic::roots();
    for root in &roots {
        for runner in heroic::Runner::ALL
            .into_iter()
            .filter(|runner| runner.source() == source)
        {
            for mut game in heroic::games(root, runner, errors) {
                check_launcher(&mut game);
                add(game, games);
            }
        }
    }
    !roots.is_empty()
}

/// The Lutris games, under the game folders. Lutris keeps no folder for some games, so
/// they are not checked for one.
fn lutris_games(games: &mut Vec<Game>, errors: &mut Vec<String>) -> bool {
    let roots = lutris::roots();
    for root in &roots {
        match lutris::games(root) {
            Ok(found) => {
                for mut game in found {
                    check_launcher(&mut game);
                    games.push(game);
                }
            }
            Err(error) => errors.push(format!("{error:#}")),
        }
    }
    !roots.is_empty()
}

/// Scan the launchers `settings` turns on. Steam, Epic and GOG (through Heroic) and the
/// game folders (Heroic's Amazon games and own programs, and Lutris) are read; every other
/// source that is on is reported `unavailable`, with no game.
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
        let mut errors = vec![];
        let available = match source {
            Source::Steam => {
                let roots = crate::steam::roots();
                steam_games(&roots, &mut games);
                !roots.is_empty()
            }
            Source::Epic | Source::Gog => heroic_games(source, &mut games, &mut errors),
            Source::Local => {
                let heroic = heroic_games(source, &mut games, &mut errors);
                lutris_games(&mut games, &mut errors) || heroic
            }
            Source::Battlenet | Source::Ea | Source::Ubisoft | Source::Microsoft => false,
        };
        if !errors.is_empty() {
            // One launcher's trouble never hides the others: its games found so far stay.
            tracing::warn!(
                source = source.key(),
                ?errors,
                "the launcher scan is incomplete"
            );
        }
        sources.push(SourceStatus {
            source,
            name: source.name(),
            state: if !errors.is_empty() {
                "error"
            } else if available {
                "available"
            } else {
                "unavailable"
            },
            errors,
        });
    }
    Discovery {
        sources,
        games: library::finish(games),
    }
}

/// Start an imported game by its launcher address or program. A Steam game starts through
/// the Steam client inside the stream's session; see [`start`] for why.
pub fn launch(
    action: &Launch,
    directory: Option<&Path>,
    elevated: bool,
    environment: &std::collections::BTreeMap<String, String>,
) -> Result<()> {
    start::launch(action, directory, elevated, environment)
}

#[cfg(test)]
mod tests;
