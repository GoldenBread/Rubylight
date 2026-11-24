//! Steam sync settings, game filtering, app identity, and library reconciliation.
use super::Game;
use crate::library;
use butterpollo_config::config::Config;
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
};

/// The Steam settings: its switch and games to leave out are Steam's own; the rest is the
/// shared Library sync group (`library_*` keys), which a `steam_*` key of earlier releases
/// overrides when Steam differed at the fold. Vibepollo's defaults.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub enabled: bool,
    pub auto_sync: bool,
    pub sync_all_installed: bool,
    pub recent_games: usize,
    pub recent_max_age_days: u64,
    pub remove_uninstalled: bool,
    pub include_tools: bool,
    pub exclusions: Vec<(String, String)>,
}
impl Settings {
    pub fn from_config(config: &Config) -> Self {
        let count = |key: &str, default: i64| config.integer(key, default).max(0) as u64;
        let shared_count =
            |option: &str, default: i64| config.integer(&format!("library_{option}"), default);
        Self {
            enabled: config.boolean("steam_enabled", true),
            auto_sync: config.boolean("steam_auto_sync", config.boolean("library_auto_sync", true)),
            sync_all_installed: config.boolean(
                "steam_sync_all_installed",
                config.boolean("library_sync_all_installed", true),
            ),
            recent_games: count("steam_recent_games", shared_count("recent_games", 10)) as usize,
            recent_max_age_days: count(
                "steam_recent_max_age_days",
                shared_count("recent_max_age_days", 30),
            ),
            remove_uninstalled: config.boolean(
                "steam_autosync_remove_uninstalled",
                config.boolean("library_autosync_remove_uninstalled", true),
            ),
            include_tools: config.boolean("steam_include_tools", false),
            exclusions: id_names(config.get("steam_exclude_games", "")),
        }
    }
    /// Whether a sync removes managed apps that are no longer selected.
    pub fn removes_missing(&self) -> bool {
        !self.sync_all_installed || self.remove_uninstalled
    }
    pub fn source(&self) -> &'static str {
        if self.sync_all_installed {
            "installed"
        } else {
            "recent"
        }
    }
}
/// `[{"id":"570","name":"Dota 2"}, "730"]` or `570, 730`: plain entries are IDs.
pub fn id_names(value: &str) -> Vec<(String, String)> {
    let value = value.trim();
    if value.is_empty() {
        return vec![];
    }
    match serde_json::from_str::<Value>(value) {
        Ok(Value::Array(entries)) => entries
            .iter()
            .filter_map(|entry| match entry {
                Value::String(id) => Some((id.trim().to_owned(), String::new())),
                Value::Number(id) => Some((id.to_string(), String::new())),
                Value::Object(fields) => {
                    let text = |key: &str| match fields.get(key) {
                        Some(Value::String(s)) => s.trim().to_owned(),
                        Some(Value::Number(n)) => n.to_string(),
                        _ => String::new(),
                    };
                    Some((text("id"), text("name")))
                }
                _ => None,
            })
            .filter(|(id, name)| !id.is_empty() || !name.is_empty())
            .collect(),
        _ => value
            .split(',')
            .map(|id| (id.trim().to_owned(), String::new()))
            .filter(|(id, _)| !id.is_empty())
            .collect(),
    }
}
/// Not a game: tools, runtimes, redistributables and DLC.
pub fn importable(game: &Game, include_tools: bool) -> bool {
    if include_tools {
        return true;
    }
    if matches!(
        game.app_type.to_ascii_lowercase().as_str(),
        "tool" | "config" | "dlc" | "driver" | "music" | "video"
    ) || [228980, 1070560, 1391110, 1628350, 1824220, 1493710, 4183110].contains(&game.appid)
    {
        return false;
    }
    let name = game.name.to_ascii_lowercase();
    !name.starts_with("proton ")
        && !name.starts_with("steam linux runtime")
        && name != "steamworks common redistributables"
}
fn excluded(id: &str, name: &str, exclusions: &[(String, String)]) -> bool {
    exclusions.iter().any(|(excluded_id, excluded_name)| {
        (!excluded_id.is_empty() && excluded_id == id)
            || (!excluded_name.is_empty() && excluded_name.eq_ignore_ascii_case(name))
    })
}
pub fn is_excluded(game: &Game, exclusions: &[(String, String)]) -> bool {
    excluded(&game.appid.to_string(), &game.name, exclusions)
}
/// The games a sync keeps: every installed one, or the most recently played.
pub fn select<'a>(games: &'a [Game], settings: &Settings, now: u64) -> Vec<&'a Game> {
    let mut selected: Vec<&Game> = games
        .iter()
        .filter(|g| importable(g, settings.include_tools) && !is_excluded(g, &settings.exclusions))
        .filter(|g| g.installed && (settings.sync_all_installed || g.last_played > 0))
        .collect();
    if settings.sync_all_installed {
        return selected;
    }
    if settings.recent_games == 0 {
        return vec![];
    }
    if settings.recent_max_age_days > 0 {
        let age = settings.recent_max_age_days.saturating_mul(86400);
        selected.retain(|g| g.last_played <= now && now - g.last_played <= age);
    }
    selected.sort_by(|a, b| {
        b.last_played
            .cmp(&a.last_played)
            .then(a.appid.cmp(&b.appid))
    });
    selected.truncate(settings.recent_games);
    selected
}
/// Vibepollo's UUID for a Steam app: "STEAM" in the first fields and the
/// app ID in the last 48 bits.
pub fn canonical_uuid(appid: u32) -> String {
    format!("53544541-4d00-5000-8000-{appid:012x}")
}
/// The Steam app ID an app entry names, from `steam-id` or its UUID.
pub fn app_id_of(app: &Value) -> Option<u32> {
    match app.get("steam-id") {
        Some(Value::Number(id)) => {
            if let Some(id) = id.as_u64().and_then(|id| u32::try_from(id).ok()) {
                return Some(id);
            }
        }
        Some(Value::String(raw)) => {
            if let Ok(id) = raw.parse() {
                return Some(id);
            }
            if raw.to_ascii_lowercase().starts_with("steam-") {
                return raw[6..].parse().ok();
            }
        }
        _ => {}
    }
    let uuid = app.get("uuid")?.as_str()?.to_ascii_lowercase();
    if let Some(id) = uuid.strip_prefix("steam-") {
        return id.parse().ok();
    }
    uuid.strip_prefix("53544541-4d00-5000-8000-")
        .filter(|hex| hex.len() == 12)
        .and_then(|hex| u32::from_str_radix(hex, 16).ok())
}
/// The Steam game an app starts, by any of its marks: [`app_id_of`], an "Add games" import's
/// launcher id, or a `steam://rungameid/` or `steam://run/` command.
pub fn game_of(app: &Value) -> Option<u32> {
    app_id_of(app)
        .or_else(|| {
            (app.get("launcher-source").and_then(Value::as_str) == Some("steam"))
                .then(|| app.get("launcher-id")?.as_str()?.parse().ok())
                .flatten()
        })
        .or_else(|| command_app_id(app.get("cmd")?.as_str()?))
}
/// The app ID in a `steam://rungameid/<id>` or `steam://run/<id>` command.
fn command_app_id(command: &str) -> Option<u32> {
    let lower = command.to_ascii_lowercase();
    let rest = ["steam://rungameid/", "steam://run/"]
        .iter()
        .find_map(|prefix| lower.find(prefix).map(|at| &lower[at + prefix.len()..]))?;
    let digits = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..digits].parse().ok()
}
/// Whether an app entry starts through Steam, so the games it runs read controllers through
/// Steam Input: a Steam game by any of its marks ([`game_of`]: an imported entry's UUID or app
/// ID, or a `steam://rungameid/` command), or a command or detached command that is a
/// `steam://` link or runs Steam's own program (`steam.exe -applaunch 570`, `-gamepadui`,
/// `-bigpicture`), as the Steam Big Picture entry opens `steam://open/bigpicture`. The Desktop,
/// an entry that starts a game's own .exe and a Playnite entry (Playnite starts its game, its
/// own way) do not.
pub fn launched_through_steam(app: &Value) -> bool {
    use crate::launcher_prompt::{launcher_of, split_program};
    let starts_steam = |command: &str| {
        command.to_ascii_lowercase().contains("steam://")
            || launcher_of(split_program(command).0) == Some("steam")
    };
    game_of(app).is_some()
        || app
            .get("cmd")
            .and_then(Value::as_str)
            .is_some_and(starts_steam)
        || app
            .get("detached")
            .and_then(Value::as_array)
            .is_some_and(|commands| commands.iter().filter_map(Value::as_str).any(starts_steam))
}
fn id_of(app: &Value) -> String {
    app_id_of(app).map_or_else(
        || {
            app.get("steam-id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned()
        },
        |id| id.to_string(),
    )
}
/// How a Steam game starts: through the Steam client, so Steam's overlay,
/// cloud saves and launch options apply.
pub fn launch_command(appid: u32) -> String {
    format!("cmd /c start \"\" steam://rungameid/{appid}")
}
/// The fallback cover for Steam apps without artwork.
pub const FALLBACK_IMAGE: &str = "./assets/steam.png";
fn generic(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
fn set_or_remove(app: &mut serde_json::Map<String, Value>, key: &str, value: Option<String>) {
    match value.filter(|v| !v.is_empty()) {
        Some(value) => {
            app.insert(key.into(), Value::String(value));
        }
        None => {
            app.remove(key);
        }
    }
}
/// Write a managed app's fields for `game`; `cover` is its PNG, if any.
pub(super) fn update(app: &mut Value, game: &Game, source: &str, cover: Option<&Path>) {
    if !app.is_object() {
        *app = json!({});
    }
    let fields = app.as_object_mut().unwrap();
    let name = if game.name.is_empty() {
        format!("Steam {}", game.appid)
    } else {
        game.name.clone()
    };
    fields.insert("name".into(), json!(name));
    fields.insert("uuid".into(), json!(canonical_uuid(game.appid)));
    fields.insert("steam-id".into(), json!(game.appid.to_string()));
    fields.insert("steam-managed".into(), json!("auto"));
    fields.insert(library::SYNCED_BY.into(), json!("steam"));
    fields.insert("steam-source".into(), json!(source));
    set_or_remove(fields, "steam-app-type", Some(game.app_type.clone()));
    fields.insert("cmd".into(), json!(launch_command(game.appid)));
    fields.insert("auto-detach".into(), json!(true));
    fields.insert("wait-all".into(), json!(false));
    set_or_remove(
        fields,
        "steam-install-dir",
        Some(generic(&game.install_dir)),
    );
    set_or_remove(
        fields,
        "steam-library-path",
        Some(generic(&game.library_path)),
    );
    set_or_remove(
        fields,
        "steam-icon-path",
        game.icon_path.as_deref().map(generic),
    );
    set_or_remove(
        fields,
        "steam-header-path",
        game.header_path.as_deref().map(generic),
    );
    set_or_remove(
        fields,
        "steam-boxart-path",
        game.portrait_path.as_deref().map(generic),
    );
    let artwork = game.artwork();
    set_or_remove(fields, "steam-artwork-path", artwork.map(generic));
    set_or_remove(
        fields,
        "steam-artwork-format",
        artwork
            .and_then(|p| p.extension())
            .map(|e| e.to_string_lossy().to_ascii_lowercase()),
    );
    match cover {
        Some(cover) => {
            let cover = generic(cover);
            fields.insert("steam-artwork-client-path".into(), json!(cover));
            fields.insert("steam-artwork-client-compatible".into(), json!(true));
            fields.insert("image-path".into(), json!(cover));
            // Steam's own cover replaces one from SteamGridDB, and its mark goes with it.
            fields.remove(crate::steamgriddb::COVER_FIELD);
        }
        None => {
            // Keep a cover the user chose; replace only the one this sync set.
            let previous = fields
                .remove("steam-artwork-client-path")
                .and_then(|v| v.as_str().map(str::to_owned));
            fields.remove("steam-artwork-client-compatible");
            let image = fields
                .get("image-path")
                .and_then(Value::as_str)
                .unwrap_or("");
            if image.is_empty() || previous.as_deref() == Some(image) {
                fields.insert("image-path".into(), json!(FALLBACK_IMAGE));
            }
        }
    }
}
/// Bring the Steam apps this sync owns ([`library::SYNCED_BY`]) in `apps` in line with
/// `selected`. Every other app belongs to the user, whatever its marker or UUID: it is never
/// renamed, changed or removed, no second app is added for its game, and only a missing
/// Steam cover is filled in. Games in `dismissed` (`steam:<appid>`, deleted from the library)
/// are not added. Returns whether anything changed.
pub fn reconcile(
    apps: &mut Vec<Value>,
    selected: &[&Game],
    settings: &Settings,
    covers: &HashMap<u32, PathBuf>,
    dismissed: &[String],
) -> bool {
    let games: Vec<&Game> = selected
        .iter()
        .copied()
        .filter(|g| {
            importable(g, settings.include_tools)
                && !is_excluded(g, &settings.exclusions)
                && !dismissed.contains(&library::dismissed_key(
                    library::Source::Steam,
                    &g.appid.to_string(),
                ))
        })
        .collect();
    let by_id: HashMap<String, &Game> = games.iter().map(|g| (g.appid.to_string(), *g)).collect();
    // The games the user's own apps already have.
    let users: BTreeSet<u32> = apps
        .iter()
        .filter(|app| !library::synced_by(app, "steam"))
        .filter_map(game_of)
        .collect();
    let source = settings.source();
    let remove_missing = settings.removes_missing();
    let mut changed = false;
    let mut seen = BTreeSet::new();
    let mut index = 0;
    while index < apps.len() {
        let app = &mut apps[index];
        let steam_id = id_of(app);
        let parsed = app_id_of(app);
        if steam_id.is_empty() || !library::synced_by(app, "steam") {
            if let Some(game) = game_of(app).and_then(|id| by_id.get(&id.to_string()))
                && let Some(cover) = covers.get(&game.appid)
            {
                let image = app.get("image-path").and_then(Value::as_str).unwrap_or("");
                let previous = app
                    .get("steam-artwork-client-path")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if image.is_empty()
                    || image == FALLBACK_IMAGE
                    || (!previous.is_empty() && image == previous)
                    || crate::steamgriddb::fetched_cover(app)
                {
                    let before = app.clone();
                    let cover = generic(cover);
                    if let Some(fields) = app.as_object_mut() {
                        fields.remove(crate::steamgriddb::COVER_FIELD);
                    }
                    app["image-path"] = json!(cover);
                    app["steam-artwork-client-path"] = json!(cover);
                    app["steam-artwork-client-compatible"] = json!(true);
                    changed |= *app != before;
                }
            }
            index += 1;
            continue;
        }
        let name = app
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if excluded(&steam_id, &name, &settings.exclusions)
            || seen.contains(&steam_id)
            || parsed.is_some_and(|id| users.contains(&id))
        {
            apps.remove(index);
            changed = true;
            continue;
        }
        seen.insert(steam_id.clone());
        let before = app.clone();
        if let Some(game) = by_id.get(&steam_id) {
            update(
                app,
                game,
                source,
                covers.get(&game.appid).map(PathBuf::as_path),
            );
        } else {
            if let Some(id) = parsed {
                app["uuid"] = json!(canonical_uuid(id));
                app["steam-id"] = json!(id.to_string());
                app["steam-managed"] = json!("auto");
            }
            if remove_missing {
                apps.remove(index);
                changed = true;
                continue;
            }
        }
        changed |= *app != before;
        index += 1;
    }
    for game in games {
        let key = game.appid.to_string();
        if seen.contains(&key)
            || users.contains(&game.appid)
            || apps.iter().any(|app| id_of(app) == key)
        {
            continue;
        }
        let mut app = json!({});
        update(
            &mut app,
            game,
            source,
            covers.get(&game.appid).map(PathBuf::as_path),
        );
        apps.push(app);
        changed = true;
    }
    changed
}
