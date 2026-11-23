//! Cross-host library merging (`merge_library`, `merge_rich_library`), classic-list
//! synthesis, and merged `Entry` / `Copy` models.

use super::ClassicApps;
use crate::http::native_api::library::{Artwork, Game, Library, Store};
use crate::http::native_api::session::{Session, State};
use crate::http::GameExtra;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

pub const CLASSIC_REVISION: &str = "classic";

/// Host input consumed by the pure merge/filter/detail logic.
#[derive(Clone, Copy)]
pub struct Source<'a> {
    pub id: u64,
    pub unique_id: &'a str,
    pub name: &'a str,
    pub online: bool,
    pub paired: bool,
    pub library: Option<&'a Library>,
    pub classic: bool,
    pub last_played: Option<&'a BTreeMap<String, u64>>,
    pub playtime_minutes: Option<&'a BTreeMap<String, u64>>,
    pub collections: Option<&'a BTreeMap<String, Vec<String>>>,
    pub session: Option<&'a Session>,
    pub host_control: bool,
}

/// Extended source carrying per-game [`GameExtra`] (unadded/launchable/extended artwork) alongside [`Source`].
#[derive(Clone, Copy)]
pub struct RichSource<'a> {
    pub source: Source<'a>,
    pub extra: Option<&'a [GameExtra]>,
}

/// Whether Home should open first given user preference and available native libraries.
pub fn opens_on_home(enabled: bool, sources: &[Source<'_>]) -> bool {
    enabled && has_native_library(sources)
}

/// Whether any paired source currently has a non-empty library (native or classic fallback).
pub fn has_library(sources: &[Source<'_>]) -> bool {
    sources
        .iter()
        .any(|s| s.paired && s.library.is_some_and(|l| !l.games.is_empty()))
}

/// Whether any paired source currently has a non-empty native library.
pub fn has_native_library(sources: &[Source<'_>]) -> bool {
    sources
        .iter()
        .any(|s| s.paired && !s.classic && s.library.is_some_and(|l| !l.games.is_empty()))
}

/// Synthesizes a [`Library`] from a saved [`ClassicApps`] list and optional running classic app info.
pub fn classic_library(apps: &ClassicApps, running: Option<(i32, Option<&str>)>) -> Library {
    let games = apps
        .apps
        .iter()
        .map(|app| {
            let is_running = running.is_some_and(|(id, uuid)| {
                if let Some(u) = uuid.filter(|u| !u.is_empty())
                    && !app.uuid.is_empty()
                {
                    u.eq_ignore_ascii_case(&app.uuid)
                } else {
                    id > 0 && id == app.id
                }
            });
            let install_id = app.uuid.clone();
            let app_id = u32::try_from(app.id).unwrap_or(0);
            Game {
                install_id,
                app_id,
                title: app.name.clone(),
                game_id: None,
                store: None,
                installed: None,
                running: is_running,
                artwork: Artwork {
                    cover: (!app.art_version.is_empty()).then(|| app.id.to_string()),
                    version: app.art_version.clone(),
                },
                last_played: None,
                playtime_minutes: None,
                collections: Vec::new(),
            }
        })
        .collect();
    Library {
        revision: CLASSIC_REVISION.to_owned(),
        games,
    }
}

/// One host's copy of a game in the merged library.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Copy {
    pub host: u64,
    pub host_uid: String,
    pub host_name: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub index: usize,
    pub online: bool,
    pub app_id: u32,
    pub install_id: String,
    pub running: bool,
    pub installed: bool,
    pub installed_raw: Option<bool>,
    pub added: bool,
    pub launchable: Option<bool>,
    pub is_native: bool,
    pub store: Option<String>,
    pub store_name: Option<String>,
    pub store_id: Option<String>,
    pub game_id: Option<String>,
    pub has_cover: bool,
    pub cover_path: Option<String>,
    pub cover_version: String,
    pub hero_path: Option<String>,
    pub logo_path: Option<String>,
    pub landscape_path: Option<String>,
    pub last_played: Option<u64>,
    pub playtime_minutes: Option<u64>,
    pub collections: Vec<String>,
}

impl PartialEq for Copy {
    fn eq(&self, other: &Self) -> bool {
        self.host == other.host
            && self.host_uid == other.host_uid
            && self.host_name == other.host_name
            && self.title == other.title
            && self.online == other.online
            && self.app_id == other.app_id
            && self.install_id == other.install_id
            && self.running == other.running
            && self.installed == other.installed
            && self.installed_raw == other.installed_raw
            && self.added == other.added
            && self.launchable == other.launchable
            && self.is_native == other.is_native
            && self.store == other.store
            && self.store_name == other.store_name
            && self.store_id == other.store_id
            && self.game_id == other.game_id
            && self.has_cover == other.has_cover
            && self.cover_path == other.cover_path
            && self.cover_version == other.cover_version
            && self.hero_path == other.hero_path
            && self.logo_path == other.logo_path
            && self.landscape_path == other.landscape_path
            && self.last_played == other.last_played
            && self.playtime_minutes == other.playtime_minutes
            && self.collections == other.collections
    }
}

impl Eq for Copy {}

/// A cross-host merged game entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub key: String,
    pub title: String,
    pub copies: Vec<Copy>,
}

impl Entry {
    /// Picks the best copy to open/display first:
    /// running -> online & installed & added -> online & added -> installed -> first.
    pub fn best(&self) -> &Copy {
        self.copies
            .iter()
            .enumerate()
            .max_by_key(|(i, c)| {
                (
                    c.running,
                    c.online && c.installed && c.added,
                    c.online && c.added,
                    c.online,
                    c.installed,
                    c.has_cover,
                    std::cmp::Reverse(*i),
                )
            })
            .map(|(_, c)| c)
            .expect("merged Entry always has at least one Copy")
    }

    pub fn running(&self) -> bool {
        self.copies.iter().any(|c| c.running)
    }

    pub fn installed(&self) -> bool {
        self.copies.iter().any(|c| c.installed)
    }

    pub fn added(&self) -> bool {
        self.copies.iter().any(|c| c.added)
    }

    pub fn last_played(&self) -> Option<u64> {
        self.copies.iter().filter_map(|c| c.last_played).max()
    }

    pub fn playtime_minutes(&self) -> Option<u64> {
        self.copies.iter().filter_map(|c| c.playtime_minutes).max()
    }

    pub fn store(&self) -> Option<&str> {
        self.copies.iter().find_map(|c| c.store.as_deref())
    }

    pub fn store_name(&self) -> Option<&str> {
        self.copies.iter().find_map(|c| c.store_name.as_deref())
    }

    pub fn collections(&self) -> Vec<String> {
        let mut out = Vec::new();
        for copy in &self.copies {
            for name in &copy.collections {
                if !out.contains(name) {
                    out.push(name.clone());
                }
            }
        }
        out
    }

    pub fn cover(&self) -> Option<&Copy> {
        self.copies
            .iter()
            .filter(|c| c.has_cover)
            .max_by_key(|c| (c.online, c.installed, c.running))
    }

    pub fn game_id(&self) -> Option<&str> {
        self.copies.iter().find_map(|c| c.game_id.as_deref())
    }
}

/// Merged library snapshot split into `all`, `shown`, and `hidden`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Merged {
    all: Vec<Entry>,
    shown: Vec<Entry>,
    hidden: Vec<Entry>,
}

impl Merged {
    pub fn from_parts(all: Vec<Entry>, shown: Vec<Entry>, hidden: Vec<Entry>) -> Self {
        Self { all, shown, hidden }
    }

    pub fn all(&self) -> &[Entry] {
        &self.all
    }

    pub fn shown(&self) -> &[Entry] {
        &self.shown
    }

    pub fn hidden(&self) -> &[Entry] {
        &self.hidden
    }
}

fn normalized_store(store: Option<&Store>) -> (Option<String>, Option<String>, Option<String>) {
    let Some(s) = store else {
        return (None, None, None);
    };
    let source = s.source.trim().to_ascii_lowercase();
    if source.is_empty() {
        return (None, None, None);
    }
    let name = s.name.trim();
    let display = if name.is_empty() {
        default_store_name(&source)
    } else {
        name.to_owned()
    };
    let id = s.id.trim();
    let store_id = (!id.is_empty()).then(|| id.to_owned());
    (Some(source), Some(display), store_id)
}

fn default_store_name(source: &str) -> String {
    match source {
        "steam" => "Steam".to_owned(),
        "epic" => "Epic Games".to_owned(),
        "gog" => "GOG".to_owned(),
        "ea" => "EA".to_owned(),
        "ubisoft" => "Ubisoft Connect".to_owned(),
        "microsoft" | "xbox" => "Xbox".to_owned(),
        "battlenet" => "Battle.net".to_owned(),
        "playnite" => "Playnite".to_owned(),
        _ => {
            let mut chars = source.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        }
    }
}

/// Merges games across `sources` (without extra per-game metadata).
pub fn merge_library(sources: &[Source<'_>]) -> Vec<Entry> {
    let rich: Vec<RichSource<'_>> = sources
        .iter()
        .copied()
        .map(|source| RichSource {
            source,
            extra: None,
        })
        .collect();
    merge_rich_library(&rich)
}

/// Merges games across `RichSource`s (preserving `added`, `launchable`, and extended artwork).
pub fn merge_rich_library(sources: &[RichSource<'_>]) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut by_key: HashMap<String, usize> = HashMap::new();

    for rich in sources {
        let source = &rich.source;
        let Some(lib) = source.library else {
            continue;
        };
        for (idx, game) in lib.games.iter().enumerate() {
            let extra = rich
                .extra
                .and_then(|list| list.get(idx))
                .cloned()
                .unwrap_or_default();
            let game_id = game
                .game_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            let key = match &game_id {
                Some(gid) => format!("game:{}", gid.to_ascii_lowercase()),
                None => {
                    let host_part = if source.unique_id.is_empty() {
                        source.id.to_string()
                    } else {
                        source.unique_id.to_owned()
                    };
                    if !game.install_id.is_empty() {
                        format!("app:{}:{}", host_part, game.install_id.to_ascii_lowercase())
                    } else if game.app_id != 0 {
                        format!("appid:{}:{}", host_part, game.app_id)
                    } else {
                        format!("unadded:{host_part}:{idx}")
                    }
                }
            };
            let (store, store_name, store_id) = normalized_store(game.store.as_ref());
            let installed_raw = game.installed;
            // Classic / manual apps without an installed bool default to installed (`true`).
            let installed = installed_raw.unwrap_or(true);
            let has_cover = game
                .artwork
                .cover
                .as_deref()
                .is_some_and(|c| !c.trim().is_empty());
            let last_played = source
                .last_played
                .and_then(|m| {
                    (!game.install_id.is_empty())
                        .then(|| m.get(&game.install_id).copied())
                        .flatten()
                })
                .or_else(|| rich.extra.is_some().then_some(game.last_played).flatten());
            let playtime_minutes = source
                .playtime_minutes
                .and_then(|m| {
                    (!game.install_id.is_empty())
                        .then(|| m.get(&game.install_id).copied())
                        .flatten()
                })
                .or_else(|| rich.extra.is_some().then_some(game.playtime_minutes).flatten());
            let collections = if let Some(cols) = source.collections {
                if game.install_id.is_empty() {
                    Vec::new()
                } else {
                    cols.get(&game.install_id)
                        .map(|list| {
                            list.iter()
                                .map(|s| s.trim())
                                .filter(|s| !s.is_empty())
                                .map(str::to_owned)
                                .collect()
                        })
                        .unwrap_or_default()
                }
            } else if rich.extra.is_some() {
                game.collections.clone()
            } else {
                Vec::new()
            };
            let session_running = source.session.is_some_and(|s| {
                matches!(
                    s.state,
                    State::Preparing | State::Launching | State::Streaming | State::Suspended
                ) && s.app.as_ref().is_some_and(|a| {
                    (!game.install_id.is_empty()
                        && !a.install_id.is_empty()
                        && a.install_id.eq_ignore_ascii_case(&game.install_id))
                        || (game.app_id != 0 && a.app_id == game.app_id)
                })
            });

            let copy = Copy {
                host: source.id,
                host_uid: source.unique_id.to_owned(),
                host_name: source.name.to_owned(),
                title: game.title.clone(),
                index: idx,
                online: source.online,
                app_id: game.app_id,
                install_id: game.install_id.clone(),
                running: source.online && (game.running || session_running),
                installed,
                installed_raw,
                added: extra.added,
                launchable: extra.launchable,
                is_native: rich.extra.is_none() || !source.classic,
                store,
                store_name,
                store_id,
                game_id,
                has_cover,
                cover_path: rich.extra.is_some().then(|| game.artwork.cover.clone()).flatten(),
                cover_version: if rich.extra.is_some() {
                    game.artwork.version.clone()
                } else {
                    String::new()
                },
                hero_path: extra.hero,
                logo_path: extra.logo,
                landscape_path: extra.landscape,
                last_played,
                playtime_minutes,
                collections,
            };

            if let Some(&pos) = by_key.get(&key) {
                entries[pos].copies.push(copy);
            } else {
                by_key.insert(key.clone(), entries.len());
                entries.push(Entry {
                    key,
                    title: game.title.clone(),
                    copies: vec![copy],
                });
            }
        }
    }

    for entry in &mut entries {
        entry.copies.sort_by_key(|a| {
            (
                !a.running,
                !(a.online && a.installed && a.added),
                !(a.online && a.added),
                !a.online,
                !a.installed,
            )
        });
        if let Some(first) = entry.copies.first()
            && first.running
        {
            entry.title = first.title.clone();
        }
    }
    entries.sort_by_key(|a| a.title.to_lowercase());
    entries
}
