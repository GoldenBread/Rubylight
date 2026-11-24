//! Paired client records, per-device permissions and capability grants, and WebRTC device helpers.
use super::{
    LAUNCH_APPS, PAIRING_APPROVE_KEY, PER_SCREEN_SETTINGS, SECOND_SCREEN_SUFFIX, default_permission,
};
use anyhow::Result;
use butterpollo_crypto::crypto;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn enabled() -> bool {
    true
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Client {
    pub name: String,
    pub cert: String,
    pub uuid: String,
    #[serde(default = "default_permission", deserialize_with = "permission_number")]
    pub perm: u32,
    #[serde(default = "enabled", deserialize_with = "compatible_bool")]
    pub enabled: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
fn permission_number<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<u32, D::Error> {
    let v = Value::deserialize(d)?;
    match v {
        Value::String(s) => s.parse().map_err(serde::de::Error::custom),
        Value::Number(n) => n
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| serde::de::Error::custom("invalid permissions")),
        _ => Err(serde::de::Error::custom("invalid permissions")),
    }
}
pub(super) fn compatible_bool<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<bool, D::Error> {
    match Value::deserialize(d)? {
        Value::Bool(b) => Ok(b),
        Value::String(s) if matches!(s.as_str(), "true" | "1") => Ok(true),
        Value::String(s) if matches!(s.as_str(), "false" | "0") => Ok(false),
        Value::Number(n) if n.as_u64() == Some(0) => Ok(false),
        Value::Number(n) if n.as_u64() == Some(1) => Ok(true),
        _ => Err(serde::de::Error::custom("invalid boolean")),
    }
}
/// Third-party library tools (ApolloSync, MoonDeckBuddy) write `null` for optional strings
/// and lists in `apps.json`; read those as empty rather than rejecting the whole library.
pub(super) fn null_as_default<'de, D, T>(d: D) -> std::result::Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}
impl Client {
    /// The device as its second screen: [`second_screen_uuid`] and the name marked, with the
    /// device's certificate, permissions and settings, but the [`PER_SCREEN_SETTINGS`] that
    /// screen kept, never the first screen's. Never paired itself.
    pub fn second_screen(mut self) -> Self {
        if !self.uuid.ends_with(SECOND_SCREEN_SUFFIX) {
            for setting in PER_SCREEN_SETTINGS {
                let own = self
                    .extra
                    .remove(&format!("{setting}{SECOND_SCREEN_SUFFIX}"));
                self.extra.remove(setting);
                if let Some(own) = own {
                    self.extra.insert(setting.to_owned(), own);
                }
            }
            self.uuid.push_str(SECOND_SCREEN_SUFFIX);
            self.name.push_str(" (second screen)");
        }
        self
    }
    pub fn allows(&self, permission: u32) -> bool {
        self.enabled && self.perm & permission == permission
    }
    /// Whether this device may run its connect and disconnect commands
    /// (on unless the administrator turned it off, as in Vibepollo).
    pub fn allows_commands(&self) -> bool {
        !self
            .extra
            .get("allow_client_commands")
            .is_some_and(|v| v == false || v == "false")
    }
    /// Whether the administrator granted this device Host control (change
    /// Radeon settings, close the game). It is a separate key in `extra`, not
    /// a `perm` bit, so an all-ones permission value can't switch it on, and
    /// only `true` and `"true"` count. Pairing and migration never set it.
    pub fn allows_host_control(&self) -> bool {
        self.enabled
            && self
                .extra
                .get("host_control")
                .is_some_and(|v| v == true || v == "true")
    }
    /// Whether this device holds "Approve new devices" (`pairing_approve`): the
    /// three `/pairing` routes and nothing else. Only a JSON `true` counts. The
    /// first device with a certificate on a host gets it
    /// ([`PairedState::paired_device_extra`]), any other only from the console;
    /// Host control never reads it.
    pub fn allows_pairing_approve(&self) -> bool {
        self.enabled && self.extra.get(PAIRING_APPROVE_KEY) == Some(&Value::Bool(true))
    }
    /// Whether this device may list, approve and deny devices waiting to pair.
    pub fn may_approve_pairing(&self) -> bool {
        self.allows_host_control() || self.allows_pairing_approve()
    }
    /// Whether the administrator let this device put the host to sleep
    /// (`power_action`). Only a JSON `true` counts, as in Vibepollo, so an
    /// imported Vibepollo opt-in carries over and a permission value never
    /// grants it.
    pub fn allows_sleep(&self) -> bool {
        self.enabled && self.extra.get("allow_sleep") == Some(&Value::Bool(true))
    }
    /// Whether the administrator let this device restart and shut down the
    /// PC (`allow_restart_shutdown`). Only a JSON `true` counts: Sleep PC, Host
    /// control, a permission value and the pairing roles never grant it.
    pub fn allows_restart_shutdown(&self) -> bool {
        self.enabled && self.extra.get("allow_restart_shutdown") == Some(&Value::Bool(true))
    }
    /// Whether the administrator let this device attach USB devices to the
    /// host (`allow_usb`). Only a JSON `true` counts: no permission bit or Host
    /// control implies it.
    pub fn allows_usb(&self) -> bool {
        self.enabled && self.extra.get("allow_usb") == Some(&Value::Bool(true))
    }
    /// Whether this device may join other devices' co-op sessions (`coop_join`,
    /// `nucleus::coop::may_join`): it needs Launch apps, as joining launches the
    /// co-op entry; never set, it follows them, set, only true counts. A
    /// disabled device may not.
    pub fn allows_coop_join(&self) -> bool {
        self.enabled && crate::nucleus::coop::may_join(&self.extra, self.perm & LAUNCH_APPS != 0)
    }
    /// This device's grant for shared folder `id`, from
    /// `shared_folders: {"<id>": "read" | "write"}`. Any other value, or an id
    /// the console would refuse (a hand-edited `"../x"`), grants nothing, and a
    /// disabled device has no grants.
    pub fn folder_access(&self, id: &str) -> Option<rubylight_native_api::folders::FolderAccess> {
        use rubylight_native_api::folders::FolderAccess;
        if !self.enabled || !butterpollo_config::config::valid_folder_id(id) {
            return None;
        }
        match self.extra.get("shared_folders")?.get(id)?.as_str()? {
            "read" => Some(FolderAccess::Read),
            "write" => Some(FolderAccess::Write),
            _ => None,
        }
    }
    /// Whether this device has at least one shared folder grant.
    pub fn has_folders(&self) -> bool {
        self.enabled
            && self
                .extra
                .get("shared_folders")
                .and_then(Value::as_object)
                .is_some_and(|grants| grants.keys().any(|id| self.folder_access(id).is_some()))
    }
    pub fn der(&self) -> Result<Vec<u8>> {
        crypto::certificate_der(&self.cert)
    }
    /// The WebRTC credentials of a device paired over the WebRTC signalling
    /// channel: `(client_id, secret)` from `extra.webrtc`, only if both have
    /// the shape the host issues. Such a device has no TLS certificate.
    pub fn webrtc(&self) -> Option<(&str, &str)> {
        use crate::webrtc::auth::{CLIENT_ID_BYTES, SECRET_BYTES, is_lower_hex};
        let webrtc = self.extra.get("webrtc")?;
        let id = webrtc.get("clientId")?.as_str()?;
        let secret = webrtc.get("secret")?.as_str()?;
        (is_lower_hex(id, CLIENT_ID_BYTES) && is_lower_hex(secret, SECRET_BYTES))
            .then_some((id, secret))
    }
    /// A device the administrator approved over WebRTC signalling, with a new
    /// UUID. [`PairedState::add_webrtc`] sets its permissions.
    pub fn paired_over_webrtc(name: String, client_id: &str, secret: &str) -> Self {
        Self {
            name,
            cert: String::new(),
            uuid: uuid::Uuid::new_v4().to_string(),
            perm: default_permission(),
            enabled: true,
            extra: BTreeMap::from([(
                "webrtc".to_owned(),
                json!({"clientId": client_id, "secret": secret}),
            )]),
        }
    }
    /// A WebRTC device: no certificate, valid WebRTC credentials. It can
    /// never be matched to a TLS peer.
    pub fn is_webrtc(&self) -> bool {
        self.cert.trim().is_empty() && self.webrtc().is_some()
    }
    /// The WebRTC device the console user `username` streams as from the console's own stream
    /// page, with a new UUID. [`PairedState::add_webrtc`] gives it every permission: a console
    /// sign-in is the administrator.
    pub fn console_over_webrtc(
        name: String,
        username: &str,
        client_id: &str,
        secret: &str,
    ) -> Self {
        let mut client = Self::paired_over_webrtc(name, client_id, secret);
        client.extra.get_mut("webrtc").unwrap()[CONSOLE] = username.into();
        client
    }
    /// The console user this WebRTC device stands for, if it is a console's own device. Only the
    /// host writes `extra.webrtc`.
    pub fn console(&self) -> Option<&str> {
        if !self.is_webrtc() {
            return None;
        }
        self.extra.get("webrtc")?.get(CONSOLE)?.as_str()
    }
}
/// The key in `extra.webrtc` that names the console user a device stands for.
const CONSOLE: &str = "console";
