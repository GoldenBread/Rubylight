//! The Rubylight native API v1 (`/api/native/v1`, docs/native-api.md in the host repo): the
//! host's library, session state and actions, and Radeon state. Same pinned HTTPS and client
//! certificate as the classic calls; gate them on `Feature::NativeApi`.
//!
//! Every non-2xx answer carries an `ApiError` body, kept in [`Error::Api`] together with the
//! HTTP status. Clients branch on `ErrorCode` and fall back to the status for
//! `ErrorCode::Unknown`. A 404 from a path under the prefix that has no error body means the
//! host has no native API at all (an older Rubylight, Sunshine, Apollo).

use super::device_approval::FEATURE_PAIRING_APPROVE;
use super::session_guests::{FEATURE_GUEST_INVITES, FEATURE_SESSION_GUESTS};
use super::native_api::capabilities::Capabilities;
use super::native_api::library::{Game, Library};
use super::native_api::session::{Owner, Session, SessionApp, State};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;

pub const FEATURE_LIBRARY_UNADDED: &str = "library.unadded";
pub const FEATURE_LIBRARY_ADD: &str = "library.add";
pub const FEATURE_SESSION_CONTINUE_HERE: &str = "session.continue_here";
pub const FEATURE_USB: &str = "usb";

/// Fields on a library entry beyond the pinned `rubylight_native_api::library::Game`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameExtra {
    #[serde(default = "default_true")]
    pub added: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launchable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hero: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landscape: Option<String>,
}

fn default_true() -> bool {
    true
}

impl Default for GameExtra {
    fn default() -> Self {
        Self {
            added: true,
            launchable: None,
            hero: None,
            logo: None,
            landscape: None,
        }
    }
}

/// The library entry returned by `POST /api/native/v1/library/add`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddedGame {
    pub game: Game,
    pub extra: GameExtra,
}

/// The game another device plays, from `GET /session` (`playing_on`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayingOn {
    pub device: String,
    pub app: SessionApp,
}

/// Where this device's stream went, from `GET /session` (`moved_to`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MovedTo {
    pub device: String,
    pub since_unix_ms: u64,
}

/// The part of `GET /session` Continue here reads. Both are absent where the host does not say
/// them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamMove {
    #[serde(default)]
    pub playing_on: Option<PlayingOn>,
    #[serde(default)]
    pub moved_to: Option<MovedTo>,
    /// The running game is this device's.
    #[serde(default)]
    pub owns_game: bool,
    /// The press that took this device's stream failed, and the game is another device's.
    #[serde(default)]
    pub press_failed: bool,
}

/// The file, USB and shared folder permissions, which the pinned `Permissions` does not carry
/// yet. A host without the feature leaves them out: false.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize)]
pub struct FilePermissions {
    #[serde(default)]
    pub file_upload: bool,
    #[serde(default)]
    pub file_download: bool,
    /// The 3.0 permissions read anything but a bool as refused, so a host that gets them wrong
    /// loses only them, not the file permissions beside them.
    #[serde(default, deserialize_with = "bool_or_false")]
    pub usb: bool,
    /// The device has at least one folder grant.
    #[serde(default, deserialize_with = "bool_or_false")]
    pub shared_folders: bool,
    /// The device may restart and shut down the PC (its own grant on the host).
    #[serde(default, deserialize_with = "bool_or_false")]
    pub restart_shutdown: bool,
    /// The device may approve new devices, and nothing else (its own grant on the host).
    #[serde(default, deserialize_with = "bool_or_false")]
    pub pairing_approve: bool,
}

fn bool_or_false<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    Ok(serde_json::Value::deserialize(deserializer)?.as_bool().unwrap_or(false))
}

/// `GET /capabilities` with the fields of newer hosts, read from the same body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeCapabilities {
    pub capabilities: Capabilities,
    pub files: FilePermissions,
}

impl NativeCapabilities {
    /// Whether a device that resumes another device's game joins it as a guest, and the guests
    /// list, approval prompt and Join are offered.
    pub fn offers_session_guests(&self) -> bool {
        self.capabilities.features.iter().any(|f| f == FEATURE_SESSION_GUESTS)
    }

    /// Whether the PC makes invites to play over the internet. Its guests list and join prompts
    /// are then read too, even with `guest_join` off: a friend's device paired from an invite
    /// always waits for the player to let it in.
    pub fn offers_guest_invites(&self) -> bool {
        self.capabilities.features.iter().any(|f| f == FEATURE_GUEST_INVITES)
    }

    /// Whether this PC can approve new devices at all, for this device or another.
    pub fn offers_pairing_approve(&self) -> bool {
        self.capabilities.features.iter().any(|f| f == FEATURE_PAIRING_APPROVE)
    }

    /// Whether this device may list and approve devices waiting to pair on this PC: with
    /// "Approve new devices" or Host control.
    pub fn may_approve_pairing(&self) -> bool {
        self.offers_pairing_approve()
            && (self.files.pairing_approve || self.capabilities.permissions.host_control)
    }

    /// Whether this device may make invites to play over the internet: the PC offers them
    /// (`session.guest-invites`) and this device holds "Approve new devices" or Host control.
    pub fn may_invite(&self) -> bool {
        self.capabilities
            .features
            .iter()
            .any(|f| f == FEATURE_GUEST_INVITES)
            && (self.files.pairing_approve || self.capabilities.permissions.host_control)
    }

    /// Whether the host offers `feature` in its feature list.
    pub fn has_feature(&self, feature: &str) -> bool {
        self.capabilities.features.iter().any(|f| f == feature)
    }

    /// Whether Continue here may take a stream from another device: View + Launch + feature.
    pub fn offers_continue_here(&self) -> bool {
        self.capabilities.permissions.view
            && self.capabilities.permissions.launch
            && self.has_feature(FEATURE_SESSION_CONTINUE_HERE)
    }

    /// Whether USB forwarding is offered: both `usb` feature and `permissions.usb` grant.
    pub fn offers_usb_forwarding(&self) -> bool {
        self.files.usb && self.has_feature(FEATURE_USB)
    }
}

/// Whether `session` represents an active host session (not idle/stopping/unknown).
pub fn session_is_active(session: &Session) -> bool {
    matches!(
        session.state,
        State::Preparing | State::Launching | State::Streaming | State::Suspended
    )
}

/// Whether `session` is currently suspended.
pub fn session_is_suspended(session: &Session) -> bool {
    session.state == State::Suspended
}

/// Whether an active session is held by another device.
pub fn session_held_by_other_device(session: &Session) -> bool {
    session_is_active(session) && session.owner == Owner::OtherDevice
}

impl<'de> Deserialize<'de> for NativeCapabilities {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Files {
            permissions: FilePermissions,
        }
        let mut value = serde_json::Value::deserialize(deserializer)?;
        let files = Files::deserialize(&value).map_err(D::Error::custom)?;
        // Capabilities deserializes the 3.0 permissions strictly as bools; strip non-bools here so
        // malformed 3.0 fields read as refused (`false`) without rejecting older fields beside them.
        if let Some(perms) = value.get_mut("permissions").and_then(serde_json::Value::as_object_mut) {
            for key in ["usb", "shared_folders", "coop_join", "restart_shutdown", "pairing_approve"] {
                if perms.get(key).is_some_and(|v| !v.is_boolean()) {
                    perms.remove(key);
                }
            }
        }
        Ok(Self {
            capabilities: Capabilities::deserialize(value).map_err(D::Error::custom)?,
            files: files.permissions,
        })
    }
}

impl NativeCapabilities {
    /// Parses `GET /capabilities` while skipping any non-string entries inside `features` (used by
    /// Android callers that tolerate mixed feature lists alongside `NativeCapabilities::deserialize`).
    pub fn parse_lenient(value: &serde_json::Value) -> Result<Self, serde_json::Error> {
        let mut clean = value.clone();
        if let Some(obj) = clean.as_object_mut() {
            if !obj.get("api_version").is_some_and(|v| v.as_u64().is_some()) {
                obj.insert("api_version".into(), serde_json::Value::from(0u64));
            }
            match obj.get_mut("features") {
                Some(serde_json::Value::Array(items)) => items.retain(|item| item.is_string()),
                Some(_) | None => {
                    obj.insert("features".into(), serde_json::Value::Array(Vec::new()));
                }
            }
            let host = obj
                .entry("host")
                .or_insert_with(|| serde_json::json!({}));
            if !host.is_object() {
                *host = serde_json::json!({});
            }
            if let Some(host_obj) = host.as_object_mut() {
                for key in ["name", "version", "unique_id"] {
                    if !host_obj.get(key).is_some_and(|v| v.is_string()) {
                        host_obj.insert(key.into(), serde_json::Value::String(String::new()));
                    }
                }
            }
            let perms = obj
                .entry("permissions")
                .or_insert_with(|| serde_json::json!({}));
            if !perms.is_object() {
                *perms = serde_json::json!({});
            }
            if let Some(perm_obj) = perms.as_object_mut() {
                for key in ["list", "view", "launch", "host_control"] {
                    perm_obj
                        .entry(key)
                        .or_insert(serde_json::Value::Bool(false));
                }
            }
        }
        Self::deserialize(clean)
    }
}

/// `GET /library` with each game's `last_played`, `playtime_minutes`, `collections`, and
/// `extra` (`added`, `launchable`, `hero`/`logo`/`landscape` artwork), read from the same body:
/// the pinned `Game` predates them. Written back, the fields sit in their games again, so
/// Android's Java and a saved library read them as a host sends them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeLibrary {
    pub library: Library,
    /// When each game was last played, by install id: seconds since the epoch, `0` when the
    /// host says never played. A game the host says nothing about (an older host, a manual app)
    /// is not in it.
    pub last_played: BTreeMap<String, u64>,
    /// How long each game has been played, by install id, in minutes: `0` when the host says
    /// never played. A game the host says nothing about is not in it.
    pub playtime_minutes: BTreeMap<String, u64>,
    /// The collections each game is in, by install id, as the host names them. A game in none (and
    /// every game of an older host) is not in it.
    pub collections: BTreeMap<String, Vec<String>>,
    /// Per-game additions (`added`, `launchable`, `hero`/`logo`/`landscape`), indexed parallel to
    /// `library.games`.
    pub extra: Vec<GameExtra>,
}

impl NativeLibrary {
    /// Extra metadata for game `index`, defaulting cleanly if `extra` is shorter (e.g. manual construction).
    pub fn extra_for(&self, index: usize) -> GameExtra {
        self.extra.get(index).cloned().unwrap_or_default()
    }

    /// Whether game `index` is added to the host's library (`true` by default).
    pub fn added_at(&self, index: usize) -> bool {
        self.extra.get(index).is_none_or(|e| e.added)
    }
}

impl From<Library> for NativeLibrary {
    fn from(library: Library) -> Self {
        let mut last_played = BTreeMap::new();
        let mut playtime_minutes = BTreeMap::new();
        let mut collections = BTreeMap::new();
        for game in &library.games {
            if let Some(at) = game.last_played {
                last_played.insert(game.install_id.clone(), at);
            }
            if let Some(minutes) = game.playtime_minutes {
                playtime_minutes.insert(game.install_id.clone(), minutes);
            }
            if !game.collections.is_empty() {
                collections.insert(game.install_id.clone(), game.collections.clone());
            }
        }
        let extra = vec![GameExtra::default(); library.games.len()];
        Self {
            library,
            last_played,
            playtime_minutes,
            collections,
            extra,
        }
    }
}

fn non_empty_str(value: Option<&serde_json::Value>) -> Option<String> {
    value
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

type NormalizedGame = (GameExtra, Option<u64>, Option<u64>, Vec<String>);

fn normalize_game_value(
    game: &mut serde_json::Value,
    require_title: bool,
) -> Result<NormalizedGame, String> {
    let fields = game
        .as_object_mut()
        .ok_or_else(|| "game entry must be an object".to_owned())?;
    let added = fields
        .get("added")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let install_id = non_empty_str(fields.get("install_id"));
    if added && install_id.is_none() {
        return Err("added game is missing install_id".to_owned());
    }
    if !added && install_id.is_none() {
        fields.insert("install_id".into(), serde_json::Value::String(String::new()));
    }
    let app_id_ok = fields
        .get("app_id")
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|id| u32::try_from(id).is_ok());
    if !app_id_ok {
        if !added || fields.get("app_id").is_none_or(|v| v.is_null()) {
            fields.insert("app_id".into(), 0.into());
        } else {
            return Err("invalid app_id".to_owned());
        }
    }
    if !require_title && !fields.get("title").is_some_and(|v| v.is_string()) {
        fields.insert("title".into(), serde_json::Value::String(String::new()));
    }
    if !fields.contains_key("running") || !fields["running"].is_boolean() {
        fields.insert("running".into(), false.into());
    }
    if !fields.contains_key("game_id") || !fields["game_id"].is_string() {
        fields.insert("game_id".into(), serde_json::Value::Null);
    }
    if !fields.contains_key("installed") || !fields["installed"].is_boolean() {
        fields.insert("installed".into(), serde_json::Value::Null);
    }
    match fields.get_mut("store") {
        Some(serde_json::Value::Object(store)) => {
            if let Some(source) = non_empty_str(store.get("source")) {
                store.insert("source".into(), source.into());
                if !store.get("id").is_some_and(|v| v.is_string()) {
                    store.insert("id".into(), serde_json::Value::String(String::new()));
                }
                if !store.get("name").is_some_and(|v| v.is_string()) {
                    store.insert("name".into(), serde_json::Value::String(String::new()));
                }
            } else {
                fields.insert("store".into(), serde_json::Value::Null);
            }
        }
        _ => {
            fields.insert("store".into(), serde_json::Value::Null);
        }
    }

    let (hero, logo, landscape) = match fields.get_mut("artwork") {
        Some(serde_json::Value::Object(art)) => {
            let cover = non_empty_str(art.get("cover"));
            let hero = non_empty_str(art.get("hero"));
            let logo = non_empty_str(art.get("logo"));
            let landscape = non_empty_str(art.get("landscape"));
            let version = art
                .get("version")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_owned();
            if let Some(c) = cover {
                art.insert("cover".into(), c.into());
            } else {
                art.remove("cover");
            }
            art.insert("version".into(), version.into());
            (hero, logo, landscape)
        }
        _ => {
            fields.insert(
                "artwork".into(),
                serde_json::json!({ "version": "" }),
            );
            (None, None, None)
        }
    };

    let launchable = fields.get("launchable").and_then(serde_json::Value::as_bool);
    let last_played = fields.get("last_played").and_then(serde_json::Value::as_u64);
    let playtime_minutes = fields.get("playtime_minutes").and_then(serde_json::Value::as_u64);
    let collections: Vec<String> = fields
        .get("collections")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|name| name.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned))
        .collect();

    if fields.get("last_played").is_some_and(|v| !v.is_u64()) {
        fields.remove("last_played");
    }
    if fields.get("playtime_minutes").is_some_and(|v| !v.is_u64()) {
        fields.remove("playtime_minutes");
    }
    if fields.contains_key("collections") {
        if collections.is_empty() {
            fields.remove("collections");
        } else {
            fields.insert("collections".into(), collections.clone().into());
        }
    }

    let extra = GameExtra {
        added,
        launchable,
        hero,
        logo,
        landscape,
    };
    Ok((extra, last_played, playtime_minutes, collections))
}

impl<'de> Deserialize<'de> for NativeLibrary {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut value = serde_json::Value::deserialize(deserializer)?;
        let mut last_played = BTreeMap::new();
        let mut playtime_minutes = BTreeMap::new();
        let mut collections = BTreeMap::new();
        let mut extra = Vec::new();
        for game in value
            .get_mut("games")
            .and_then(serde_json::Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            let (game_extra, lp, pt, cols) =
                normalize_game_value(game, true).map_err(D::Error::custom)?;
            extra.push(game_extra);
            let Some(id) = game
                .get("install_id")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
            else {
                continue;
            };
            if let Some(at) = lp {
                last_played.insert(id.clone(), at);
            }
            if let Some(minutes) = pt {
                playtime_minutes.insert(id.clone(), minutes);
            }
            if !cols.is_empty() {
                collections.insert(id, cols);
            }
        }
        Ok(Self {
            library: Library::deserialize(value).map_err(D::Error::custom)?,
            last_played,
            playtime_minutes,
            collections,
            extra,
        })
    }
}

impl Serialize for NativeLibrary {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::Error as _;
        let mut value = serde_json::to_value(&self.library).map_err(S::Error::custom)?;
        if let Some(games) = value["games"].as_array_mut() {
            for (idx, game) in games.iter_mut().enumerate() {
                let extra = self.extra_for(idx);
                let install_id = game["install_id"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned);
                let at = install_id
                    .as_deref()
                    .and_then(|id| self.last_played.get(id))
                    .copied();
                let minutes = install_id
                    .as_deref()
                    .and_then(|id| self.playtime_minutes.get(id))
                    .copied();
                let names = install_id
                    .as_deref()
                    .and_then(|id| self.collections.get(id))
                    .cloned()
                    .filter(|v| !v.is_empty());
                let Some(fields) = game.as_object_mut() else { continue };
                if !extra.added && install_id.is_none() {
                    fields.insert("install_id".into(), serde_json::Value::Null);
                }
                if !extra.added {
                    fields.insert("added".into(), false.into());
                } else {
                    fields.insert("added".into(), true.into());
                }
                if let Some(launchable) = extra.launchable {
                    fields.insert("launchable".into(), launchable.into());
                }
                if let Some(art) = fields.get_mut("artwork").and_then(serde_json::Value::as_object_mut) {
                    if let Some(hero) = extra.hero {
                        art.insert("hero".into(), hero.into());
                    }
                    if let Some(logo) = extra.logo {
                        art.insert("logo".into(), logo.into());
                    }
                    if let Some(landscape) = extra.landscape {
                        art.insert("landscape".into(), landscape.into());
                    }
                }
                if let Some(at) = at {
                    fields.insert("last_played".into(), at.into());
                }
                if let Some(minutes) = minutes {
                    fields.insert("playtime_minutes".into(), minutes.into());
                }
                if let Some(names) = names {
                    fields.insert("collections".into(), names.into());
                } else {
                    fields.remove("collections");
                }
            }
        }
        value.serialize(serializer)
    }
}

/// Parses a `POST /api/native/v1/library/add` reply: either a bare game object, `{"game": {...}}`,
/// or a full library listing `{"games": [...]}` containing `game_id`.
///
/// Returns `Ok(None)` when the reply says the game is still unadded (`added == false`), or
/// `Err` when an added reply lacks `install_id` or `app_id`.
pub fn parse_added(value: &serde_json::Value, game_id: &str) -> Result<Option<AddedGame>, String> {
    if let Some(games) = value.get("games").and_then(serde_json::Value::as_array) {
        for entry in games {
            let same_game = entry
                .get("game_id")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|id| id == game_id);
            let added = entry
                .get("added")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true);
            if same_game && added {
                return parse_added_object(entry);
            }
        }
        return Ok(None);
    }
    let entry = value.get("game").unwrap_or(value);
    parse_added_object(entry)
}

fn parse_added_object(raw: &serde_json::Value) -> Result<Option<AddedGame>, String> {
    let mut entry = raw.clone();
    if entry.get("added").and_then(serde_json::Value::as_bool) == Some(false) {
        return Ok(None);
    }
    let install_id = non_empty_str(entry.get("install_id"));
    let app_id = entry
        .get("app_id")
        .and_then(serde_json::Value::as_u64)
        .filter(|&id| id > 0 && u32::try_from(id).is_ok());
    if install_id.is_none() || app_id.is_none() {
        return Err("added entry missing install_id or app_id".to_owned());
    }
    let (extra, _, _, _) = normalize_game_value(&mut entry, false)?;
    let game = Game::deserialize(entry).map_err(|e| e.to_string())?;
    Ok(Some(AddedGame { game, extra }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_and_unadded_and_extra_art_round_trip() {
        let json = r#"{
            "revision":"r1",
            "games":[
                {"install_id":"i1","app_id":5,"title":"Portal"},
                {"install_id":"i2","app_id":6,"title":"Hades","game_id":"epic:h","added":true,"launchable":false,"last_played":1761300000,"playtime_minutes":125,"collections":["VR","  ","Couch"],"artwork":{"cover":"/c/i2","version":"v3","hero":"/h/i2","logo":"/l/i2","landscape":"/w/i2"}},
                {"install_id":null,"title":"Celeste","game_id":"gog:c","store":{"source":"gog","id":"c","name":"GOG"},"installed":true,"added":false}
            ]
        }"#;
        let lib: NativeLibrary = serde_json::from_str(json).unwrap();
        assert_eq!(lib.library.games.len(), 3);
        assert!(lib.added_at(0));
        assert_eq!(lib.extra_for(1).launchable, Some(false));
        assert_eq!(lib.extra_for(1).hero.as_deref(), Some("/h/i2"));
        assert_eq!(lib.extra_for(1).logo.as_deref(), Some("/l/i2"));
        assert_eq!(lib.extra_for(1).landscape.as_deref(), Some("/w/i2"));
        assert_eq!(lib.collections.get("i2").unwrap(), &vec!["VR", "Couch"]);
        assert!(!lib.added_at(2));
        assert_eq!(lib.library.games[2].install_id, "");
        assert_eq!(lib.library.games[2].app_id, 0);

        let re_encoded = serde_json::to_string(&lib).unwrap();
        let reparsed: NativeLibrary = serde_json::from_str(&re_encoded).unwrap();
        assert_eq!(lib, reparsed);
    }

    #[test]
    fn added_game_still_requires_install_id() {
        let missing = r#"{"revision":"r1","games":[{"app_id":5,"title":"Portal"}]}"#;
        assert!(serde_json::from_str::<NativeLibrary>(missing).is_err());
    }

    #[test]
    fn parse_added_accepts_bare_wrapped_or_library_and_rejects_missing_ids() {
        let bare: serde_json::Value = serde_json::from_str(r#"{"install_id":"i9","app_id":9}"#).unwrap();
        let parsed = parse_added(&bare, "gog:c").unwrap().unwrap();
        assert_eq!(parsed.game.install_id, "i9");
        assert_eq!(parsed.game.app_id, 9);
        assert_eq!(parsed.game.title, "");

        let unadded: serde_json::Value = serde_json::from_str(r#"{"install_id":null,"added":false}"#).unwrap();
        assert!(parse_added(&unadded, "gog:c").unwrap().is_none());

        let invalid: serde_json::Value = serde_json::from_str(r#"{"title":"Celeste","added":true}"#).unwrap();
        assert!(parse_added(&invalid, "gog:c").is_err());
    }
}

