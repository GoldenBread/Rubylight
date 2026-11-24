//! Heroic Games Launcher's library, read from its own files: the games it installed from
//! Epic (legendary), GOG, Amazon (nile) and the ones the user added by hand (sideload).
//! Nothing is started and nothing is written. Heroic's launch address is
//! `heroic://launch/<runner>/<appName>`.

use butterpollo_session::library::{Game, Launch, Source};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Where a game comes from in Heroic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Runner {
    /// Epic Games, through legendary.
    Legendary,
    Gog,
    /// Amazon Games, through nile.
    Nile,
    /// A program the user added to Heroic.
    Sideload,
}

impl Runner {
    pub const ALL: [Self; 4] = [Self::Legendary, Self::Gog, Self::Nile, Self::Sideload];

    /// The runner's name in Heroic's launch address.
    pub fn key(self) -> &'static str {
        match self {
            Self::Legendary => "legendary",
            Self::Gog => "gog",
            Self::Nile => "nile",
            Self::Sideload => "sideload",
        }
    }
    /// The library source a runner's games are listed under: Epic and GOG as on Windows;
    /// Amazon and the user's own programs have no source of their own and go with the game
    /// folders.
    pub fn source(self) -> Source {
        match self {
            Self::Legendary => Source::Epic,
            Self::Gog => Source::Gog,
            Self::Nile | Self::Sideload => Source::Local,
        }
    }
    /// The game's id in the library: Epic and GOG keep the launcher's own, the others name
    /// their runner so they cannot meet a Lutris game.
    fn id(self, app_name: &str) -> String {
        match self {
            Self::Legendary | Self::Gog => app_name.to_owned(),
            Self::Nile | Self::Sideload => format!("heroic-{}:{app_name}", self.key()),
        }
    }
}

/// Heroic's configuration folders on this PC: `$XDG_CONFIG_HOME/heroic`, `~/.config/heroic`
/// and the Flatpak's.
pub fn roots() -> Vec<PathBuf> {
    let mut candidates = vec![];
    if let Some(config) = std::env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        candidates.push(PathBuf::from(config).join("heroic"));
    }
    if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        let home = PathBuf::from(home);
        candidates.push(home.join(".config/heroic"));
        candidates.push(home.join(".var/app/com.heroicgameslauncher.hgl/config/heroic"));
    }
    let mut roots: Vec<PathBuf> = vec![];
    for candidate in candidates {
        if let Ok(path) = std::fs::canonicalize(&candidate)
            && path.is_absolute()
            && path.is_dir()
            && !roots.contains(&path)
        {
            roots.push(path);
        }
    }
    roots
}

/// A game of Heroic's library as its files describe it.
struct Entry {
    app_name: String,
    title: String,
    folder: PathBuf,
    squares: Vec<String>,
}

/// The installed games of one runner in the Heroic folder `root`. A file that is missing
/// is no error; one that is damaged adds a line to `errors` and its games are left out.
pub fn games(root: &Path, runner: Runner, errors: &mut Vec<String>) -> Vec<Game> {
    let catalog = catalog(root, runner, errors);
    let mut games = vec![];
    for entry in installed(root, runner, &catalog, errors) {
        let mut game = Game::new(
            runner.source(),
            runner.id(&entry.app_name),
            entry.title,
            &entry.folder,
            Some(Launch::Uri {
                uri: format!("heroic://launch/{}/{}", runner.key(), entry.app_name),
            }),
        );
        game.cover = entry
            .squares
            .iter()
            .flat_map(|url| cached(root, url))
            .next();
        games.push(game);
    }
    games
}

/// Heroic's cached copy of an image: its images cache names a file by the SHA-256 of the
/// address, with or without the size Heroic asks the image server for.
fn cached(root: &Path, url: &str) -> Option<PathBuf> {
    ["", "?h=400&resize=1&w=300"]
        .iter()
        .map(|suffix| {
            let digest = Sha256::digest(format!("{url}{suffix}").as_bytes());
            let name: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
            root.join("images-cache").join(name)
        })
        .find(|path| path.is_file())
}

/// Reads a JSON file: `None` when it is not there, `None` and a line in `errors` when it
/// cannot be read or parsed.
fn json(path: &Path, errors: &mut Vec<String>) -> Option<Value> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            errors.push(format!("{}: {error}", path.display()));
            return None;
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(value) => Some(value),
        Err(error) => {
            errors.push(format!("{}: {error}", path.display()));
            None
        }
    }
}

/// Titles and box art of the runner's library, from Heroic's store cache, by app name.
fn catalog(root: &Path, runner: Runner, errors: &mut Vec<String>) -> Vec<(String, Value)> {
    let file = format!("{}_library.json", runner.key());
    let Some(value) = json(&root.join("store_cache").join(file), errors) else {
        return vec![];
    };
    value
        .get("library")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| Some((item.get("app_name")?.as_str()?.to_owned(), item.clone())))
        .collect()
}

fn text<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| value.get(*key)?.as_str())
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

/// The installed games of `runner`, titled and with their images from `catalog`.
fn installed(
    root: &Path,
    runner: Runner,
    catalog: &[(String, Value)],
    errors: &mut Vec<String>,
) -> Vec<Entry> {
    let records: Vec<Value> = match runner {
        Runner::Legendary => {
            let path = root.join("legendaryConfig/legendary/installed.json");
            match json(&path, errors) {
                Some(Value::Object(map)) => map.into_iter().map(|(_, value)| value).collect(),
                Some(_) => {
                    errors.push(format!("{}: not an object", path.display()));
                    vec![]
                }
                None => vec![],
            }
        }
        Runner::Gog | Runner::Nile => {
            let folder = if runner == Runner::Gog {
                "gog_store"
            } else {
                "nile_store"
            };
            let path = root.join(folder).join("installed.json");
            match json(&path, errors) {
                Some(Value::Array(list)) => list,
                Some(Value::Object(map)) => match map.get("installed") {
                    Some(Value::Array(list)) => list.clone(),
                    _ => map.into_iter().map(|(_, value)| value).collect(),
                },
                Some(_) => {
                    errors.push(format!("{}: not a list", path.display()));
                    vec![]
                }
                None => vec![],
            }
        }
        Runner::Sideload => {
            let path = root.join("sideload_apps/library.json");
            match json(&path, errors) {
                Some(value) => value
                    .get("games")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
                None => vec![],
            }
        }
    };
    let mut entries = vec![];
    for record in records {
        let flag = |key: &str| record.get(key).and_then(Value::as_bool);
        if flag("is_dlc") == Some(true) || flag("is_installed") == Some(false) {
            continue;
        }
        let Some(app_name) = text(&record, &["app_name", "appName", "id"]) else {
            continue;
        };
        let known = catalog
            .iter()
            .find(|(name, _)| name == app_name)
            .map(|(_, item)| item);
        let folder = if runner == Runner::Sideload {
            sideload_folder(&record)
        } else {
            text(&record, &["install_path", "path"]).map(PathBuf::from)
        };
        let title = text(&record, &["title"])
            .or_else(|| known.and_then(|item| text(item, &["title"])))
            .or(Some(app_name));
        let (Some(folder), Some(title)) = (folder, title) else {
            continue;
        };
        // Box art first, then the wide cover, from the game's record and the store cache.
        let items: Vec<&Value> = [&record].into_iter().chain(known).collect();
        let squares = ["art_square", "art_cover"].into_iter().flat_map(|key| {
            items
                .iter()
                .filter_map(move |item| text(item, &[key]).map(str::to_owned))
                .collect::<Vec<_>>()
        });
        entries.push(Entry {
            app_name: app_name.to_owned(),
            title: title.to_owned(),
            folder,
            squares: squares.collect(),
        });
    }
    entries
}

/// A sideloaded game's folder: the one Heroic names, else the program's own.
fn sideload_folder(record: &Value) -> Option<PathBuf> {
    if let Some(folder) = text(record, &["folder_name"]) {
        return Some(PathBuf::from(folder));
    }
    let executable = text(record.get("install")?, &["executable"])?;
    Path::new(executable).parent().map(Path::to_path_buf)
}
