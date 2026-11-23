//! Shared host identity and record schemas plus alias normalization.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::full_chroma;

/// The longest alias a PC can be given on this device, in Unicode scalar values.
pub const ALIAS_MAX_CHARS: usize = 40;

/// Persisted client pairing identity (`identity.json`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityFile {
    pub unique_id: String,
    pub certificate_pem: String,
    pub private_key_pem: String,
}

/// Alias policy matching Android `HostNames` and Desktop `HostRecord::set_alias`.
pub struct HostNames;

impl HostNames {
    /// Trims `typed`, truncates to [`ALIAS_MAX_CHARS`] Unicode scalar values (never splitting a
    /// codepoint), trims any trailing space after truncation, and returns `None` if empty or equal
    /// to `own_name.trim()`.
    pub fn normalize_alias(typed: Option<&str>, own_name: Option<&str>) -> Option<String> {
        let typed = typed?;
        let trimmed = typed.trim();
        if trimmed.is_empty() {
            return None;
        }
        let capped: String = trimmed.chars().take(ALIAS_MAX_CHARS).collect();
        let alias = capped.trim();
        if alias.is_empty() || own_name.is_some_and(|own| alias == own.trim()) {
            return None;
        }
        Some(alias.to_owned())
    }
}

/// Persistent host fields shared across Desktop `HostRecord` and Android `HostEntry` in
/// `hosts.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SharedHostFields {
    /// The host's `uniqueid`; empty until the host answered once.
    pub unique_id: String,
    pub name: String,
    /// A name chosen on this device. The PC's own `name` keeps following what it reports.
    pub alias: Option<String>,
    /// Last address that answered, then the others it was known by.
    pub address: String,
    pub http_port: u16,
    pub https_port: u16,
    pub other_addresses: Vec<String>,
    pub other_http_ports: Vec<u16>,
    pub pinned_address: Option<String>,
    pub remote_address: Option<String>,
    /// The certificate pinned at pairing (PEM); None while unpaired.
    pub server_certificate: Option<String>,
    pub mac: Option<String>,
    pub wake_macs: Vec<String>,
    pub wake_broadcasts: Vec<String>,
    pub own_wake_macs: Vec<String>,
    pub manual: bool,
    pub quit_on_leave: BTreeSet<String>,
    pub notices: BTreeSet<String>,
    pub full_chroma: full_chroma::Memory,
    pub internet_play: Option<String>,
}

fn app_key(uuid: &str, id: i32) -> String {
    if uuid.is_empty() {
        format!("id:{id}")
    } else {
        format!("uuid:{uuid}")
    }
}

impl SharedHostFields {
    pub fn can_wake(&self) -> bool {
        self.mac.is_some() || !self.wake_macs.is_empty() || !self.own_wake_macs.is_empty()
    }

    pub fn set_alias(&mut self, text: &str) {
        self.alias = HostNames::normalize_alias(Some(text), None);
    }

    pub fn claim_notice(&mut self, notice: &str) -> bool {
        self.notices.insert(notice.to_owned())
    }

    pub fn reset_notice(&mut self, notice: &str) -> bool {
        self.notices.remove(notice)
    }

    pub fn quits_on_leave(&self, uuid: &str, id: i32) -> bool {
        if !uuid.is_empty() {
            return self.quit_on_leave.contains(&app_key(uuid, 0));
        }
        id > 0 && self.quit_on_leave.contains(&app_key("", id))
    }

    pub fn set_quit_on_leave(&mut self, uuid: &str, id: i32, enabled: bool) {
        let mut keys = Vec::new();
        if !uuid.is_empty() {
            keys.push(app_key(uuid, id));
        }
        if id > 0 {
            keys.push(app_key("", id));
        }
        for key in keys {
            if enabled {
                self.quit_on_leave.insert(key);
            } else {
                self.quit_on_leave.remove(&key);
            }
        }
    }
}
