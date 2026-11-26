//! Configuration serialization, transactional patching (`update_with`), and pre-save checks.
use super::catalog_gen;
use super::retired;
use super::{
    BITRATE_ADAPTATION, Config, HDR_OPTION, HDR_REQUEST, LIBRARY_SYNC_OPTIONS, PACKET_PACING,
    REMOVE_REMOTE_MONITOR, RENAMED_KEYS, UPDATES, current_name, fold_bitrate_adaptation,
    fold_disconnect_behavior, fold_min_qp, fold_packet_pacing, fold_remove_remote_monitor,
    fold_updates, folded_hdr, folded_in_layer, removed, replacement, unquote,
};
use anyhow::{Result, bail};
use std::path::Path;

impl Config {
    pub fn text(&self) -> String {
        self.values
            .iter()
            .map(|(k, v)| format!("{k} = {v}\n"))
            .collect()
    }
    pub fn json(&self) -> serde_json::Value {
        serde_json::Value::Object(
            self.values
                .iter()
                .map(|(k, v)| {
                    let parsed = serde_json::from_str(v)
                        .unwrap_or_else(|_| serde_json::Value::String(v.clone()));
                    (k.clone(), parsed)
                })
                .collect(),
        )
    }
    /// Fold the retired switches of a layer of settings (an app's or a
    /// device's overrides, or an earlier console's save) over what this
    /// configuration holds, which is the layers below it.
    pub fn fold_layer(&self, layer: &mut serde_json::Map<String, serde_json::Value>) {
        fold_bitrate_adaptation(layer, self.get(BITRATE_ADAPTATION, "on"));
        fold_packet_pacing(layer, self.get(PACKET_PACING, "auto"));
        fold_remove_remote_monitor(layer, self.get(REMOVE_REMOTE_MONITOR, "never"));
    }
    pub fn update_with(
        &mut self,
        object: &serde_json::Map<String, serde_json::Value>,
        extra_validate: impl FnOnce(&Config, &dyn Fn(&str) -> bool) -> Result<()>,
    ) -> Result<()> {
        // A setting sent under an earlier name is saved under its current
        // one, unless that is sent too.
        let mut object = object.clone();
        for &(old, new) in RENAMED_KEYS {
            if let Some(value) = object.remove(old)
                && !object.contains_key(new)
            {
                object.insert(new.to_owned(), value);
            }
        }
        // The retired HDR request folds into Display HDR as it is sent with
        // it, or else over the Display HDR held: the configuration's own, or
        // for an app's or device's overrides the layers below them. An empty
        // or null request is Follow the device, as its reset was.
        if let Some(request) = object.remove(HDR_REQUEST) {
            let request = match request.as_str().map(|text| unquote(text.trim())) {
                Some(text) if !text.is_empty() => text.to_owned(),
                _ => "auto".to_owned(),
            };
            let folded = match object.get(HDR_OPTION) {
                Some(option) => folded_in_layer(option.as_str().unwrap_or_default(), &request),
                None => {
                    let held = self.get(HDR_OPTION, "auto");
                    folded_hdr(Some(held), &request).or_else(|| {
                        // Follow the device ends an earlier Always on or off.
                        (request == "auto"
                            && matches!(unquote(held.trim()), "force_on" | "force_off"))
                        .then_some("auto")
                    })
                }
            };
            if let Some(folded) = folded {
                object.insert(HDR_OPTION.to_owned(), folded.into());
            }
        }
        // The bitrate, pacing and remote monitor switches, as an earlier
        // console or an API caller sends them, fold into Adaptive bitrate,
        // Packet pacing and Remove remote monitor unless those are sent too.
        self.fold_layer(&mut object);
        // Likewise the two disconnect settings fold into the disconnect behavior.
        fold_disconnect_behavior(&mut object);
        fold_min_qp(&mut object, |key| self.values.contains_key(key), true);
        fold_updates(&mut object, self.get(UPDATES, "tell_me"));
        object.retain(|key, _| !removed(key));
        let object = &object;
        let mut next = self.clone();
        for (key, value) in object {
            if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || key.is_empty() {
                bail!("invalid configuration key");
            }
            // Legacy PATCH and POST both reset a setting when its value is
            // null/empty. Keeping the literal "null" silently defeats defaults.
            if value.is_null() || value.as_str() == Some("") {
                next.values.remove(key);
                continue;
            }
            let value = match value {
                serde_json::Value::String(v) => v.clone(),
                _ => value.to_string(),
            };
            if value.contains('\0') {
                bail!("configuration cannot contain NUL");
            }
            if value.contains(['\r', '\n'])
                && !(value.trim().starts_with('[') || value.trim().starts_with('{'))
            {
                bail!("configuration values cannot inject new keys");
            }
            // A client or an old console sending a retired value saves its
            // replacement, as the file would be read.
            let value = replacement(key, &value).map_or(value, str::to_owned);
            next.values.insert(key.clone(), value);
        }
        // A PyroWave share equal to Error correction is that share.
        if let Some(pyrowave) = next.values.get(retired::PYROWAVE_FEC)
            && retired::pyrowave_fec_is_default(
                Some(pyrowave),
                next.values.get("fec_percentage").map(String::as_str),
            )
        {
            next.values.remove(retired::PYROWAVE_FEC);
        }
        // Changing a shared Library sync setting re-decides it for every
        // launcher: the per-launcher keys of earlier releases go.
        for &(option, default, prefixes) in LIBRARY_SYNC_OPTIONS {
            let shared = format!("library_{option}");
            let Some(sent) = object.get(&shared) else {
                continue;
            };
            let sent = match sent {
                serde_json::Value::String(text) => text.clone(),
                serde_json::Value::Null => String::new(),
                other => other.to_string(),
            };
            let held = self
                .values
                .get(&shared)
                .map(String::as_str)
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(default);
            if held == sent {
                continue;
            }
            for prefix in prefixes {
                next.values.remove(&format!("{prefix}_{option}"));
            }
        }
        // Only what changes is checked: a value the file already had (the
        // host reads it leniently) must not block every other save.
        let changed = |key: &str| object.contains_key(key) && next.values.contains_key(key);
        catalog_gen::validate_changed(&next, &changed)?;
        extra_validate(&next, &changed)?;
        *self = next;
        Ok(())
    }
    /// Before `keys` are written to sunshine.conf: values the file would not
    /// give back, and settings that would keep the host from starting.
    pub fn check_saved_with<'a>(
        &self,
        keys: impl IntoIterator<Item = &'a String>,
        mut extra_check: impl FnMut(&str, &str) -> Result<()>,
    ) -> Result<()> {
        for key in keys {
            let key = current_name(key);
            let Some(value) = self.values.get(key) else {
                continue;
            };
            // '#' starts a comment and a value opening [ or { runs until it
            // closes, so such a value would be cut short, or swallow the
            // settings after it, on the next start. Check syntax as written
            // without re-folding against missing sibling keys.
            let expected = value.lines().map(str::trim).collect::<Vec<_>>().join("\n");
            let (stored, _) = Self::parse_names_as_written(&format!("{key} = {value}\n"))?;
            if stored.values.get(key).map(String::as_str) != Some(expected.trim()) {
                bail!(
                    "{key} cannot be saved: in the configuration file # starts a comment and a value opening [ or {{ has to close it"
                );
            }
            let value = unquote(value.trim());
            if key == "bind_address" && value.parse::<std::net::IpAddr>().is_err() {
                bail!("bind_address must be an IP address of this PC, or blank for all of them");
            }
            if key == "log_path"
                && (value.ends_with(['\\', '/'])
                    || (Path::new(value).is_absolute() && Path::new(value).is_dir()))
            {
                bail!("log_path must name a file, not a folder");
            }
            extra_check(key, value)?;
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn update(
        &mut self,
        object: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<()> {
        self.update_with(object, |_, _| Ok(()))
    }
    #[cfg(test)]
    pub(crate) fn check_saved<'a>(&self, keys: impl IntoIterator<Item = &'a String>) -> Result<()> {
        self.check_saved_with(keys, |_, _| Ok(()))
    }
}
