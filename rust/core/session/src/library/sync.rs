//! Launcher sync ownership stamps, dismissal keys, app indexing, and reconciliation.
use super::{Game, Settings, Source, command_program, folder_key, name_key};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// The app field that marks an app the launcher sync added and keeps in step, as
/// `steam-managed` does for Steam's sync.
pub const MANAGED: &str = "launcher-managed";
/// The app document's list of synced games deleted from the library (`source:id`), so the
/// sync does not bring them back. An import of the game takes it off the list.
pub const DISMISSED: &str = "launcher-dismissed";

/// How a game is named in [`DISMISSED`].
pub fn dismissed_key(source: Source, id: &str) -> String {
    format!("{}:{id}", source.key())
}

/// The [`DISMISSED`] entries for an app deleted from the library, whoever added it: its Steam
/// game (`steam:<appid>`), its launcher game and its Playnite game (`playnite:<id>`).
pub fn dismissal_keys(app: &Value) -> Vec<String> {
    let mut keys = vec![];
    if let Some(id) = crate::steam::game_of(app) {
        keys.push(dismissed_key(Source::Steam, &id.to_string()));
    }
    if let Some(source) = app["launcher-source"]
        .as_str()
        .and_then(|key| Source::ALL.into_iter().find(|source| source.key() == key))
        && let Some(id) = app["launcher-id"].as_str().filter(|id| !id.is_empty())
    {
        keys.push(dismissed_key(source, id));
    }
    if let Some(id) = app["playnite-id"].as_str().filter(|id| !id.is_empty()) {
        keys.push(playnite_dismissed_key(id));
    }
    keys.dedup();
    keys
}

/// How a Playnite game is named in [`DISMISSED`]: its id in lower case.
pub fn playnite_dismissed_key(id: &str) -> String {
    format!("playnite:{}", id.to_ascii_lowercase())
}

/// The app field naming the sync that created an app in this profile and still owns it:
/// `steam`, `launcher` or `playnite`. A sync changes or removes only the apps with its own
/// stamp; the `*-managed` markers alone are not enough, because Vibepollo writes them too.
/// Every other app belongs to the user.
pub const SYNCED_BY: &str = "synced-by";
/// The app document's flag that [`stamp_synced`] ran.
pub const SYNC_OWNER_VERSION: &str = "sync-owner-version";
/// Each sync's [`SYNCED_BY`] name and its marker field.
const SYNC_MARKERS: [(&str, &str); 3] = [
    ("steam", "steam-managed"),
    ("launcher", MANAGED),
    ("playnite", "playnite-managed"),
];

/// Whether `sync` (`steam`, `launcher` or `playnite`) created `app` and owns it.
pub fn synced_by(app: &Value, sync: &str) -> bool {
    app[SYNCED_BY].as_str() == Some(sync)
}

/// A synced app saved in the console becomes the user's: `sync`'s stamp goes and its marker
/// becomes `manual`, so no sync renames or removes the app again.
pub fn hand_over(app: &mut Value, sync: &str) {
    let Some(fields) = app.as_object_mut() else {
        return;
    };
    fields.remove(SYNCED_BY);
    if let Some((_, marker)) = SYNC_MARKERS.iter().find(|(name, _)| *name == sync) {
        fields.insert((*marker).into(), json!("manual"));
    }
}

/// Apps stamped by [`stamp_synced`], by sync.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Stamped {
    pub steam: usize,
    pub launcher: usize,
    pub playnite: usize,
}

/// The one-time ownership stamp for the synced apps an earlier version left, flagged by
/// [`SYNC_OWNER_VERSION`]. `steam-managed: auto` apps are stamped only when `steam_opted_in`
/// (`sunshine.conf` holds `steam_enabled = true`): without it they came from another host and
/// belong to the user. Every `launcher-managed` and `playnite-managed: auto` app is stamped.
/// Returns `None` once it has run.
pub fn stamp_synced(document: &mut Value, steam_opted_in: bool) -> Option<Stamped> {
    let fields = document.as_object_mut()?;
    if fields.get(SYNC_OWNER_VERSION).and_then(Value::as_u64) >= Some(1) {
        return None;
    }
    fields.insert(SYNC_OWNER_VERSION.into(), json!(1));
    let mut stamped = Stamped::default();
    let apps = fields.get_mut("apps").and_then(Value::as_array_mut);
    for app in apps.into_iter().flatten() {
        if app.get(SYNCED_BY).is_some() {
            continue;
        }
        let Some((sync, _)) = SYNC_MARKERS.iter().find(|(sync, marker)| {
            app[*marker].as_str() == Some("auto") && (*sync != "steam" || steam_opted_in)
        }) else {
            continue;
        };
        app[SYNCED_BY] = json!(sync);
        match *sync {
            "steam" => stamped.steam += 1,
            "launcher" => stamped.launcher += 1,
            _ => stamped.playnite += 1,
        }
    }
    Some(stamped)
}

/// The game a synced app was made from, as [`dismissed_key`] names it.
pub fn managed_key(app: &Value) -> Option<String> {
    (app[MANAGED].as_str() == Some("auto"))
        .then(|| {
            Some(format!(
                "{}:{}",
                app["launcher-source"].as_str()?,
                app["launcher-id"].as_str()?
            ))
        })
        .flatten()
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncChange {
    pub added: usize,
    pub removed: usize,
    /// Apps of a launcher's game that had no cover and got the launcher's.
    pub covered: usize,
}
impl SyncChange {
    pub fn changed(&self) -> bool {
        self.added + self.removed + self.covered > 0
    }
}

/// The source an app's `launcher-source` names.
fn source_of(app: &Value) -> Option<Source> {
    app["launcher-source"]
        .as_str()
        .and_then(|key| Source::ALL.into_iter().find(|source| source.key() == key))
}

/// The games the sync keeps of each source that is on, by source and id: every installed game
/// it can start, less the ones its options leave out, or, with `sync_all_installed` off, the
/// `recent_games` most recently played within `recent_max_age_days` of `now`, as Steam's sync
/// chooses them.
pub fn chosen<'a>(games: &'a [Game], settings: &Settings, now: u64) -> HashSet<(Source, &'a str)> {
    let mut chosen = HashSet::new();
    for (&source, options) in &settings.sources {
        if !options.enabled {
            continue;
        }
        let mut candidates: Vec<&Game> = games
            .iter()
            .filter(|game| {
                game.source == source
                    && game.unavailable_reason.is_none()
                    && !options.excludes(&game.id, &game.name)
            })
            .collect();
        if !options.sync_all_installed {
            let age = options.recent_max_age_days.saturating_mul(86_400);
            candidates.retain(|game| {
                game.last_played
                    .is_some_and(|at| at > 0 && (age == 0 || (at <= now && now - at <= age)))
            });
            candidates.sort_by(|a, b| b.last_played.cmp(&a.last_played).then(a.id.cmp(&b.id)));
            candidates.truncate(options.recent_games);
        }
        chosen.extend(
            candidates
                .into_iter()
                .map(|game| (source, game.id.as_str())),
        );
    }
    chosen
}

/// The launcher sync: Steam's sync for every other launcher and the game folders, each by its
/// own [`Options`]. Each game the options keep ([`chosen`]) becomes an app marked
/// `launcher-managed: auto`, unless an app already has the game (an import, or Playnite's or
/// Steam's copy), or it was deleted from the library (`dismissed`). An app this sync owns
/// ([`SYNCED_BY`]) leaves when its game is left out (`<launcher>_exclude_games`), and, after a
/// clean read of its launcher (`read`), when the game is no longer installed (with
/// `remove_uninstalled`) or no longer among the recent games. An app of a launcher's game with
/// no cover gets the launcher's, the one thing filled in on an app the user owns. Apps added by
/// hand, imported or edited are never otherwise changed. Steam keeps its own sync and is
/// skipped here. Sync now passes the settings; the automatic sync [`Settings::automatic`].
pub fn reconcile(
    apps: &mut Vec<Value>,
    games: &[Game],
    settings: &Settings,
    read: &[Source],
    dismissed: &[String],
) -> SyncChange {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    reconcile_at(apps, games, settings, read, dismissed, now)
}

/// [`reconcile`] at `now` (Unix seconds), which the recent games are counted back from.
pub fn reconcile_at(
    apps: &mut Vec<Value>,
    games: &[Game],
    settings: &Settings,
    read: &[Source],
    dismissed: &[String],
    now: u64,
) -> SyncChange {
    let mut change = SyncChange::default();
    let installed: HashSet<(Source, &str)> = games
        .iter()
        .map(|game| (game.source, game.id.as_str()))
        .collect();
    let wanted = chosen(games, settings, now);
    let before = apps.len();
    apps.retain(|app| {
        let Some((source, options)) =
            source_of(app).and_then(|source| Some((source, settings.syncs(source)?)))
        else {
            return true;
        };
        if !synced_by(app, "launcher") {
            return true;
        }
        let id = app["launcher-id"].as_str().unwrap_or("");
        if options.excludes(id, app["name"].as_str().unwrap_or("")) {
            return false;
        }
        if !read.contains(&source) {
            return true;
        }
        if installed.contains(&(source, id)) {
            // Out of the recent games: it leaves, as Steam's does.
            options.sync_all_installed || wanted.contains(&(source, id))
        } else {
            !(options.remove_uninstalled || !options.sync_all_installed)
        }
    });
    change.removed = before - apps.len();
    let dismissed: HashSet<&str> = dismissed.iter().map(String::as_str).collect();
    let mut index = AppIndex::new(apps);
    for game in games {
        if !wanted.contains(&(game.source, game.id.as_str()))
            || dismissed.contains(dismissed_key(game.source, &game.id).as_str())
            || index.matches(apps, game)
        {
            continue;
        }
        if let Ok(mut app) = game.app() {
            app[MANAGED] = json!("auto");
            app[SYNCED_BY] = json!("launcher");
            // A later game finds the app just added, as it found the others.
            index.add(apps.len(), &app);
            apps.push(app);
            change.added += 1;
        }
    }
    // A cover the launcher has now, for its games' apps that have none: an app synced before
    // the launcher's covers were read, or an import without one. A cover of the user's stays.
    let covers: HashMap<(Source, &str), &Path> = games
        .iter()
        .filter(|game| settings.syncs(game.source).is_some())
        .filter_map(|game| Some(((game.source, game.id.as_str()), game.cover.as_deref()?)))
        .collect();
    for app in apps.iter_mut() {
        let Some(source) = source_of(app) else {
            continue;
        };
        let id = app["launcher-id"].as_str().unwrap_or("");
        // A cover from SteamGridDB stands in only until the launcher has one.
        if app["image-path"]
            .as_str()
            .is_some_and(|path| !path.is_empty())
            && !crate::steamgriddb::fetched_cover(app)
        {
            continue;
        }
        if let Some(cover) = covers.get(&(source, id)) {
            app["image-path"] = json!(cover.to_string_lossy().replace('\\', "/"));
            if let Some(fields) = app.as_object_mut() {
                fields.remove(crate::steamgriddb::COVER_FIELD);
            }
            change.covered += 1;
        }
    }
    change
}
/// A key [`Game::matches`] can find an app by.
#[derive(Debug, PartialEq, Eq, Hash)]
enum AppKey {
    /// `launcher-source` and `launcher-id`.
    Launcher(String, String),
    Uuid(String),
    /// The Steam app ID it starts ([`crate::steam::game_of`]).
    Steam(String),
    /// Its name, in [`name_key`] form.
    Name(String),
    /// A folder (in [`folder_key`] form) its working, install or program path is in.
    Folder(String),
}
/// The apps by every key [`Game::matches`] can find one by, so each game looks up its
/// few candidates instead of trying every app.
#[derive(Default)]
pub struct AppIndex(HashMap<AppKey, Vec<usize>>);
impl AppIndex {
    pub fn new(apps: &[Value]) -> Self {
        let mut index = Self::default();
        for (at, app) in apps.iter().enumerate() {
            index.add(at, app);
        }
        index
    }
    /// Notes `app`, found at `at` in the apps.
    pub fn add(&mut self, at: usize, app: &Value) {
        let text = |key: &str| app[key].as_str();
        let mut keys = Vec::new();
        if let (Some(source), Some(id)) = (text("launcher-source"), text("launcher-id")) {
            keys.push(AppKey::Launcher(source.into(), id.into()));
        }
        if let Some(uuid) = text("uuid") {
            keys.push(AppKey::Uuid(uuid.into()));
        }
        if let Some(id) = crate::steam::game_of(app) {
            keys.push(AppKey::Steam(id.to_string()));
        }
        let name = name_key(text("name").unwrap_or(""));
        if !name.is_empty() {
            keys.push(AppKey::Name(name));
        }
        let paths = ["working-dir", "launcher-install-dir", "steam-install-dir"]
            .map(|key| text(key).unwrap_or(""));
        for path in paths
            .into_iter()
            .chain([command_program(text("cmd").unwrap_or(""))])
        {
            // The path itself and every folder above it: what `within` finds it in.
            let path = folder_key(path);
            let above = path.match_indices('\\').map(|(end, _)| &path[..end]);
            for folder in above.chain([path.as_str()]) {
                if !folder.is_empty() {
                    keys.push(AppKey::Folder(folder.into()));
                }
            }
        }
        for key in keys {
            let found = self.0.entry(key).or_default();
            if found.last() != Some(&at) {
                found.push(at);
            }
        }
    }
    /// Whether any of `apps`, the apps this index was made from, is `game`: what
    /// `apps.iter().any(|app| game.matches(app))` says.
    pub fn matches(&self, apps: &[Value], game: &Game) -> bool {
        let mut keys = vec![
            AppKey::Launcher(game.source.key().into(), game.id.clone()),
            AppKey::Uuid(game.uuid()),
        ];
        if game.source == Source::Steam {
            keys.push(AppKey::Steam(game.id.clone()));
        }
        let name = name_key(&game.name);
        if !name.is_empty() {
            keys.push(AppKey::Name(name));
        }
        let folder = folder_key(&game.install_dir);
        if !folder.is_empty() {
            keys.push(AppKey::Folder(folder));
        }
        // Each app that can match has one of these keys; `matches` decides.
        keys.iter()
            .filter_map(|key| self.0.get(key))
            .flatten()
            .any(|&at| game.matches(&apps[at]))
    }
}
