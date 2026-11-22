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
use super::native_api::library::Library;
use super::native_api::session::SessionApp;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;

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

/// `GET /library` with each game's `last_played`, `playtime_minutes` and `collections`, read from
/// the same body: the pinned `Game` predates them. Written back, the times sit in their games again, so Android's Java and a saved
/// library read them as a host sends them.
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
        Self { library, last_played, playtime_minutes, collections }
    }
}

impl<'de> Deserialize<'de> for NativeLibrary {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut value = serde_json::Value::deserialize(deserializer)?;
        let mut last_played = BTreeMap::new();
        let mut playtime_minutes = BTreeMap::new();
        let mut collections = BTreeMap::new();
        for game in value.get_mut("games").and_then(serde_json::Value::as_array_mut).into_iter().flatten() {
            let Some(id) = game.get("install_id").and_then(serde_json::Value::as_str).map(str::to_owned) else { continue };
            // Anything but a whole number of seconds reads as unknown.
            if let Some(at) = game.get("last_played").and_then(serde_json::Value::as_u64) {
                last_played.insert(id.clone(), at);
            }
            // Anything but a whole number of minutes reads as unknown.
            if let Some(minutes) = game.get("playtime_minutes").and_then(serde_json::Value::as_u64) {
                playtime_minutes.insert(id.clone(), minutes);
            }
            // Anything but names reads as none.
            let names: Vec<String> = game
                .get("collections")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|name| name.as_str().map(str::to_owned))
                .collect();
            if !names.is_empty() {
                collections.insert(id, names.clone());
            }
            if let Some(fields) = game.as_object_mut() {
                if fields.get("last_played").is_some_and(|v| !v.is_u64()) {
                    fields.remove("last_played");
                }
                if fields.get("playtime_minutes").is_some_and(|v| !v.is_u64()) {
                    fields.remove("playtime_minutes");
                }
                if fields.contains_key("collections") {
                    if names.is_empty() {
                        fields.remove("collections");
                    } else {
                        fields.insert("collections".into(), names.into());
                    }
                }
            }
        }
        Ok(Self {
            library: Library::deserialize(value).map_err(D::Error::custom)?,
            last_played,
            playtime_minutes,
            collections,
        })
    }
}

impl Serialize for NativeLibrary {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::Error as _;
        let mut value = serde_json::to_value(&self.library).map_err(S::Error::custom)?;
        if let Some(games) = value["games"].as_array_mut() {
            for game in games {
                let at = game["install_id"].as_str().and_then(|id| self.last_played.get(id)).copied();
                let minutes = game["install_id"].as_str().and_then(|id| self.playtime_minutes.get(id)).copied();
                let names = game["install_id"].as_str().and_then(|id| self.collections.get(id)).cloned();
                let Some(fields) = game.as_object_mut() else { continue };
                if let Some(at) = at {
                    fields.insert("last_played".into(), at.into());
                }
                if let Some(minutes) = minutes {
                    fields.insert("playtime_minutes".into(), minutes.into());
                }
                if let Some(names) = names {
                    fields.insert("collections".into(), names.into());
                }
            }
        }
        value.serialize(serializer)
    }
}

