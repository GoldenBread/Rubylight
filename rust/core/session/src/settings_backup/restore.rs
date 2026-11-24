//! Restore summary, config/device merging, and staged archive application for settings backups.
use super::{
    ALIASES, APPS, Archive, BACKUPS, CERTIFICATE, CONFIG, COVERS, CREDENTIALS_DIR,
    CREDENTIALS_FILE, Change, DEVICES, GAME_SETTINGS, KEY, LOCATION_KEYS, PAIRED, STAGED, Summary,
    device_identity_key, keeping_sessions, named_devices, read, read_optional, secret_key,
};
use crate::state::ProfileFiles;
use anyhow::{Context, Result};
use butterpollo_config::config::Config;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn current_config(directory: &Path) -> Config {
    Config::load(&directory.join(CONFIG)).unwrap_or_default()
}

fn app_names(document: Option<&Value>) -> BTreeMap<String, String> {
    document
        .and_then(|d| d.get("apps"))
        .and_then(Value::as_array)
        .map(|apps| {
            apps.iter()
                .filter_map(|app| {
                    let key = app
                        .get("uuid")
                        .or_else(|| app.get("name"))
                        .and_then(Value::as_str)?
                        .to_owned();
                    Some((key, app.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn webhook_urls(config: &Config) -> Vec<String> {
    serde_json::from_str::<Value>(config.get("webhooks", "[]"))
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|hook| hook.get("url")?.as_str().map(str::to_owned))
        .collect()
}

/// What restoring `archive` changes on the host whose data folder is
/// `directory`.
pub fn summarize(archive: &Archive, directory: &Path) -> Summary {
    let secrets = archive.manifest.secrets;
    let current = current_config(directory);
    // As `apply` writes it, so a webhook whose secret this PC keeps does not
    // show as a change; one that gets a fresh secret does.
    let restored = merged_config(archive, &current, String::new);
    let mut changes = Vec::new();
    let keys: BTreeSet<&String> = restored
        .values
        .keys()
        .chain(current.values.keys())
        .collect();
    let changed: Vec<&str> = keys
        .into_iter()
        .filter(|key| secrets || !secret_key(key))
        .filter(|key| restored.values.get(*key) != current.values.get(*key))
        .map(String::as_str)
        .collect();
    changes.push(Change {
        item: "settings",
        detail: if changed.is_empty() {
            "No setting changes.".into()
        } else {
            let shown: Vec<&str> = changed.iter().copied().take(12).collect();
            let more = changed.len() - shown.len();
            format!(
                "{} setting{} change: {}{}.",
                changed.len(),
                if changed.len() == 1 { "" } else { "s" },
                shown.join(", "),
                if more > 0 {
                    format!(" and {more} more")
                } else {
                    String::new()
                }
            )
        },
    });
    let files = ProfileFiles::new(&current, directory);
    if archive.files.contains_key(APPS) {
        let before = app_names(
            read_optional(&files.apps)
                .ok()
                .flatten()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .as_ref(),
        );
        let after = app_names(archive.json(APPS).as_ref());
        let added = after.keys().filter(|k| !before.contains_key(*k)).count();
        let removed = before.keys().filter(|k| !after.contains_key(*k)).count();
        let edited = after
            .iter()
            .filter(|(k, v)| before.get(*k).is_some_and(|old| old != *v))
            .count();
        changes.push(Change {
            item: "apps",
            detail: format!(
                "{} apps in the backup: {added} added, {removed} removed, {edited} changed.",
                after.len()
            ),
        });
    }
    let covers = archive.under(COVERS).count();
    if covers > 0 {
        changes.push(Change {
            item: "covers",
            detail: format!("{covers} covers written; covers only on this PC stay."),
        });
    }
    let kept = archive.under(GAME_SETTINGS).count();
    if kept > 0 {
        changes.push(Change {
            item: "game_settings",
            detail: format!("{kept} game settings files written; files only on this PC stay."),
        });
    }
    let paired_now = read_optional(&files.paired)
        .ok()
        .flatten()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .unwrap_or(Value::Null);
    let here: BTreeSet<String> = named_devices(&paired_now)
        .into_iter()
        .filter_map(|d| d.get("uuid")?.as_str().map(str::to_owned))
        .collect();
    if secrets {
        let devices = archive
            .json(PAIRED)
            .map(|d| named_devices(&d).len())
            .unwrap_or(0);
        changes.push(Change {
            item: "devices",
            detail: format!(
                "Paired devices replaced: {devices} in the backup, {} paired now.",
                here.len()
            ),
        });
        changes.push(Change {
            item: "pairing",
            detail: "This PC's pairing certificate is replaced: devices paired since the backup must pair again.".into(),
        });
        changes.push(Change {
            item: "credentials",
            detail: "The console sign-in and API tokens are replaced by the backup's.".into(),
        });
    } else if let Some(devices) = archive.json(DEVICES) {
        let listed: Vec<String> = devices
            .get("devices")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|d| d.get("uuid")?.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let matched = listed.iter().filter(|uuid| here.contains(*uuid)).count();
        changes.push(Change {
            item: "devices",
            detail: format!(
                "Settings of {matched} paired devices restored; {} devices in the backup are not paired with this PC and are skipped. Pairings and permissions stay.",
                listed.len() - matched
            ),
        });
        changes.push(Change {
            item: "pairing",
            detail: "Pairing identity, console sign-in and API tokens stay as they are.".into(),
        });
    }
    let hooks = webhook_urls(&restored);
    if !hooks.is_empty() {
        let urls = webhook_urls(&current);
        let fresh = if secrets {
            0
        } else {
            hooks.iter().filter(|url| !urls.contains(url)).count()
        };
        changes.push(Change {
            item: "webhooks",
            detail: if fresh > 0 {
                format!(
                    "{} webhooks; {fresh} get a new secret, to be copied into their receivers.",
                    hooks.len()
                )
            } else {
                format!("{} webhooks.", hooks.len())
            },
        });
    }
    Summary {
        created_unix: archive.manifest.created_unix,
        host_version: archive.manifest.host_version.clone(),
        secrets,
        changes,
    }
}

/// The restored configuration: the archive's, with this PC's secret keys and
/// webhook secrets when the archive has none (a webhook this PC does not have
/// gets `fresh()`).
fn merged_config(archive: &Archive, current: &Config, mut fresh: impl FnMut() -> String) -> Config {
    let mut config = archive.config();
    // Where this PC keeps its files stays this PC's: a backup from another
    // PC, or a crafted one, never moves them or makes the restore write
    // elsewhere.
    for key in LOCATION_KEYS {
        match current.values.get(key) {
            Some(value) => config.values.insert(key.into(), value.clone()),
            None => config.values.remove(key),
        };
    }
    if archive.manifest.secrets {
        return config;
    }
    for (key, value) in &current.values {
        if secret_key(key) {
            config.values.insert(key.clone(), value.clone());
        }
    }
    let secrets: BTreeMap<String, String> =
        serde_json::from_str::<Value>(current.get("webhooks", "[]"))
            .ok()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|hook| {
                Some((
                    hook.get("url")?.as_str()?.to_owned(),
                    hook.get("secret")?.as_str()?.to_owned(),
                ))
            })
            .collect();
    if let Some(hooks) = config.values.get_mut("webhooks")
        && let Ok(Value::Array(mut list)) = serde_json::from_str::<Value>(hooks)
    {
        for hook in &mut list {
            let url = hook
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if let Some(object) = hook.as_object_mut()
                && object
                    .get("secret")
                    .and_then(Value::as_str)
                    .is_none_or(str::is_empty)
            {
                let secret = secrets
                    .get(&url)
                    .filter(|s| !s.is_empty())
                    .cloned()
                    .unwrap_or_else(&mut fresh);
                object.insert("secret".into(), Value::String(secret));
            }
        }
        *hooks = Value::Array(list).to_string();
    }
    config
}

/// The paired-devices file with each listed device's own settings replaced
/// by the backup's; certificates, permissions, secrets and devices the backup
/// does not list stay.
fn merge_devices(paired: &mut Value, backup: &Value) {
    let Some(list) = paired
        .pointer_mut("/root/named_devices")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    let restored: BTreeMap<&str, &Value> = backup
        .get("devices")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|d| Some((d.get("uuid")?.as_str()?, d)))
                .collect()
        })
        .unwrap_or_default();
    for device in list.iter_mut().filter_map(Value::as_object_mut) {
        let Some(saved) = device
            .get("uuid")
            .and_then(Value::as_str)
            .and_then(|uuid| restored.get(uuid))
        else {
            continue;
        };
        device.retain(|key, _| device_identity_key(key));
        if let Some(name) = saved.get("name").and_then(Value::as_str) {
            device.insert("name".into(), Value::String(name.to_owned()));
        }
        if let Some(settings) = saved.get("settings").and_then(Value::as_object) {
            for (key, value) in settings {
                if !device_identity_key(key) {
                    device.insert(key.clone(), value.clone());
                }
            }
        }
    }
}

/// Where an archive path such as `covers/a.png` goes under `directory`.
fn folder_target(directory: &Path, name: &str) -> PathBuf {
    name.split('/')
        .fold(directory.to_path_buf(), |path, part| path.join(part))
}

/// Writes `archive` over the settings of the host whose data folder is
/// `directory`. Files that only this PC has (covers, game settings files)
/// stay.
pub fn apply(archive: &Archive, directory: &Path, fresh: impl FnMut() -> String) -> Result<()> {
    use crate::state::atomic_write;
    let current = current_config(directory);
    let config = merged_config(archive, &current, fresh);
    atomic_write(&directory.join(CONFIG), config.text().as_bytes())?;
    let files = ProfileFiles::new(&config, directory);
    if let Some(bytes) = archive.files.get(APPS) {
        atomic_write(&files.apps, bytes)?;
    }
    if let Some(bytes) = archive.files.get(ALIASES) {
        // The target's console sign-in sessions (and API tokens when restoring without secrets)
        // stay as they are.
        let current = read_optional(&files.aliases)?;
        atomic_write(
            &files.aliases,
            &keeping_sessions(bytes, current.as_deref(), archive.manifest.secrets)?,
        )?;
    }
    for (name, bytes) in archive.under(COVERS).chain(archive.under(GAME_SETTINGS)) {
        atomic_write(&folder_target(directory, name), bytes)?;
    }
    if archive.manifest.secrets {
        if let Some(bytes) = archive.files.get(PAIRED) {
            atomic_write(&files.paired, bytes)?;
        }
        if let Some(bytes) = archive.files.get(CREDENTIALS_FILE) {
            let target = if files.credentials == files.paired {
                directory.join(CREDENTIALS_FILE)
            } else {
                files.credentials.clone()
            };
            atomic_write(&target, bytes)?;
        }
        for (name, bytes) in archive.under(CREDENTIALS_DIR) {
            atomic_write(&folder_target(directory, name), bytes)?;
        }
        for (name, path) in [(CERTIFICATE, &files.certificate), (KEY, &files.key)] {
            if let Some(bytes) = archive.files.get(name)
                && *path != folder_target(directory, name)
            {
                atomic_write(path, bytes)?;
            }
        }
    } else if let Some(backup) = archive.json(DEVICES)
        && let Some(bytes) = read_optional(&files.paired)?
    {
        let mut paired: Value =
            serde_json::from_slice(&bytes).context("the paired devices file is not JSON")?;
        merge_devices(&mut paired, &backup);
        crate::state::write_json(&files.paired, &paired)?;
    }
    Ok(())
}

/// Applies a restore the console staged, if there is one, before the host
/// loads anything: true when one was applied. A staged archive that fails is
/// moved aside as `restore-failed.zip`, and the host starts with what it had.
pub fn apply_staged(directory: &Path, fresh: impl FnMut() -> String) -> Result<bool> {
    let backups = directory.join(BACKUPS);
    let staged = backups.join(STAGED);
    if !staged.exists() {
        return Ok(false);
    }
    let result = std::fs::File::open(&staged)
        .context("the staged restore could not be opened")
        .and_then(|file| read(std::io::BufReader::new(file)))
        .and_then(|archive| apply(&archive, directory, fresh));
    match result {
        Ok(()) => {
            std::fs::remove_file(&staged)?;
            Ok(true)
        }
        Err(error) => {
            let _ = std::fs::rename(&staged, backups.join("restore-failed.zip"));
            Err(error)
        }
    }
}
