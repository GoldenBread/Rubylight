//! Audio routing on Linux: a stream's route captures the default output as it is and changes
//! no default. The playback devices are listed from PipeWire's registry. The client's
//! microphone is the "Rubylight Microphone" source (`mic.rs`), made the default recording
//! device while it plays and put back after, with a journal a crash leaves for
//! [`recover_microphone`].

use crate::mic::{self, graph};
use anyhow::Result;
use butterpollo_config::config::Config;
use butterpollo_session::session::AudioPreparation;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub mod hooks;

/// A playback device.
#[derive(Clone, Serialize)]
pub struct Endpoint {
    pub id: String,
    pub name: String,
    pub description: String,
    pub adapter: String,
    pub default: bool,
    pub virtual_sink: bool,
}

/// The playback devices PipeWire lists, and the virtual microphone's node (the playback end of
/// the client's microphone, `virtual_sink`) once it exists. The default flag follows PipeWire's
/// default sink.
pub fn endpoints() -> Result<Vec<Endpoint>> {
    Ok(endpoints_in(&graph::snapshot()?))
}

/// [`endpoints`] of one look at the graph.
fn endpoints_in(snapshot: &graph::Snapshot) -> Vec<Endpoint> {
    let default = snapshot.default_name(graph::DEFAULT_SINK);
    snapshot
        .nodes
        .iter()
        .filter_map(|node| {
            let own = node.prop("node.name") == mic::NODE_NAME;
            let sink = node.class().starts_with("Audio/Sink");
            if !(sink || own && node.class().starts_with("Audio/Source")) {
                return None;
            }
            let name = node.prop("node.name");
            let description = node.prop("node.description");
            let adapter = [
                "device.description",
                "device.product.name",
                "api.alsa.card.name",
            ]
            .iter()
            .map(|key| node.prop(key))
            .find(|value| !value.is_empty())
            .unwrap_or_default();
            Some(Endpoint {
                id: name.to_owned(),
                name: if description.is_empty() {
                    name
                } else {
                    description
                }
                .to_owned(),
                description: description.to_owned(),
                adapter: adapter.to_owned(),
                default: !own && default.as_deref() == Some(name),
                virtual_sink: own
                    || node.prop("node.virtual") == "true"
                    || node.prop("device.id").is_empty(),
            })
        })
        .collect()
}

/// The playback side of the virtual microphone: what is played there, apps record from
/// "Rubylight Microphone".
pub fn steam_microphone(endpoints: &[Endpoint]) -> Option<&Endpoint> {
    endpoints.iter().find(|endpoint| {
        endpoint.id == mic::NODE_NAME
            || [&endpoint.name, &endpoint.adapter].iter().any(|n| {
                n.to_ascii_lowercase()
                    .contains("steam streaming microphone")
            })
    })
}

/// A stream's audio route: the device it captures from, and the defaults it holds while it
/// lives. On Linux it captures what the default output plays, where a game in its gamescope
/// session plays too, and changes no default (`sink` empty).
pub struct Route {
    pub sink: String,
}
impl AudioPreparation<Route> for Arc<Route> {
    fn route(&self) -> Arc<Route> {
        self.clone()
    }
}
impl Route {
    /// A route that owns no device. Only for tests of how long the host keeps a route (the
    /// `test-route` feature).
    #[cfg(feature = "test-route")]
    pub fn detached(_directory: &Path) -> Self {
        Self {
            sink: String::new(),
        }
    }
    /// What the console should say about the sink this route captures.
    pub fn warning(&self, _host_audio: bool) -> Option<&'static str> {
        None
    }
    /// Route a stream's audio: on Linux, the default output's monitor.
    pub fn acquire(
        _config: &Config,
        _directory: &Path,
        _host_audio: bool,
        _channels: usize,
    ) -> Result<Self> {
        Ok(Self {
            sink: String::new(),
        })
    }
    /// Keep the virtual speakers the default device.
    pub fn maintain_default(&self) -> Result<()> {
        Ok(())
    }
    /// The device to capture from.
    pub fn capture_sink(&self, _config: &Config) -> Result<String> {
        Ok(self.sink.clone())
    }
    /// The channels the sink has for a stream of `channels`: the Linux host does not format
    /// its sink, so it returns `channels`, as the Windows host does for a sink it does not
    /// format.
    pub fn set_channels(&self, channels: usize) -> Result<usize> {
        Ok(channels)
    }
}

/// Put back the defaults an interrupted stream left. Nothing is left on Linux.
pub fn recover(_directory: &Path) -> Result<()> {
    Ok(())
}

/// Put back the defaults an interrupted microphone left.
pub fn recover_microphone(directory: &Path) -> Result<()> {
    if MICROPHONE_ACTIVE.load(Ordering::Acquire) {
        anyhow::bail!("microphone routing is in use");
    }
    restore_microphone(directory)
}

/// Whether a [`MicrophoneRoute`] holds the recording default.
static MICROPHONE_ACTIVE: AtomicBool = AtomicBool::new(false);

/// What a microphone route changed, kept on disk until it is undone: the recording default
/// the user had (`None` for none chosen) and the one the route set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Journal {
    before: Option<String>,
    applied: String,
}
/// What putting a journal back does to the recording default.
#[derive(Debug, PartialEq, Eq)]
enum Restore {
    Nothing,
    Set(String),
}
impl Journal {
    /// A journal for selecting `applied` over `current`. Defaults a left-over `pending`
    /// journal had set stay what its user had.
    fn new(current: Option<String>, applied: String, pending: Option<&Self>) -> Self {
        let before = match pending {
            Some(old) if current.as_deref() == Some(old.applied.as_str()) => old.before.clone(),
            _ => current,
        };
        Self { before, applied }
    }
    /// Only our own selection is undone; a default the user chose since wins. A cleared
    /// default (the node went away) goes back to what it was.
    fn restore(&self, current: Option<&str>) -> Restore {
        match (current, &self.before) {
            (Some(now), Some(before)) if now == self.applied && now != before => {
                Restore::Set(before.clone())
            }
            (None, Some(before)) => Restore::Set(before.clone()),
            _ => Restore::Nothing,
        }
    }
}
fn microphone_path(directory: &Path) -> PathBuf {
    directory.join("microphone-recovery.json")
}
fn microphone_pending(directory: &Path) -> Result<Option<Journal>> {
    match std::fs::read(microphone_path(directory)) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
/// Puts the recording default back as the journal in `directory` says, then drops the journal.
/// It stays when PipeWire cannot be reached, for the next try.
fn restore_microphone(directory: &Path) -> Result<()> {
    let Some(journal) = microphone_pending(directory)? else {
        return Ok(());
    };
    let current = graph::snapshot()?.default_name(graph::CONFIGURED_SOURCE);
    let action = journal.restore(current.as_deref());
    // A recording default that was never chosen goes back to unchosen.
    let change = |name: Option<&str>| {
        graph::session(Some(graph::Change {
            key: graph::CONFIGURED_SOURCE,
            name,
        }))
    };
    match &action {
        Restore::Set(name) => {
            change(Some(name))?;
            tracing::info!(device = %name, "microphone default restored");
        }
        Restore::Nothing
            if journal.before.is_none() && current.as_deref() == Some(journal.applied.as_str()) =>
        {
            change(None)?;
            tracing::info!("microphone default cleared");
        }
        Restore::Nothing => {}
    }
    butterpollo_session::state::write_json(&microphone_path(directory), &Option::<Journal>::None)
}

/// Temporary defaults for an opened client microphone: the virtual microphone is the default
/// recording device until this drops.
pub struct MicrophoneRoute {
    directory: PathBuf,
}
impl MicrophoneRoute {
    /// Make the client microphone the default recording device.
    pub fn acquire(directory: &Path) -> Result<Self> {
        use anyhow::Context;
        if MICROPHONE_ACTIVE.swap(true, Ordering::AcqRel) {
            anyhow::bail!("a client microphone already owns the recording defaults");
        }
        let result = (|| {
            mic::existing_device().context("the Rubylight Microphone node is not running")?;
            let current = graph::snapshot()?.default_name(graph::CONFIGURED_SOURCE);
            let pending = microphone_pending(directory)?;
            let journal = Journal::new(current, mic::NODE_NAME.to_owned(), pending.as_ref());
            // Written, and the recovery journal told, before the first change.
            butterpollo_session::state::write_json(&microphone_path(directory), &Some(&journal))?;
            hooks::mark_microphone(true)?;
            let selected = graph::session(Some(graph::Change {
                key: graph::CONFIGURED_SOURCE,
                name: Some(mic::NODE_NAME),
            }));
            if let Err(error) = selected {
                if restore_microphone(directory).is_ok() {
                    let _ = hooks::mark_microphone(false);
                }
                return Err(error);
            }
            Ok(())
        })();
        if let Err(error) = result {
            MICROPHONE_ACTIVE.store(false, Ordering::Release);
            return Err(error);
        }
        tracing::info!("client microphone selected as the PipeWire recording default");
        Ok(Self {
            directory: directory.into(),
        })
    }
}
impl Drop for MicrophoneRoute {
    fn drop(&mut self) {
        MICROPHONE_ACTIVE.store(false, Ordering::Release);
        match restore_microphone(&self.directory) {
            Ok(()) => {
                let _ = hooks::mark_microphone(false);
            }
            Err(error) => {
                tracing::warn!(%error, "microphone defaults will be retried by crash recovery")
            }
        }
    }
}

/// Nothing is saved on Linux: making the virtual microphone changes no default.
pub struct SavedDefaults(());
impl SavedDefaults {
    /// Puts back defaults moved to a device that arrived since. None move on Linux.
    pub fn restore(&self) -> Result<()> {
        Ok(())
    }
}

/// Make the virtual microphone's node, which the host then finds in [`endpoints`].
pub fn install_steam_microphone(_config: &Config) -> Result<Option<SavedDefaults>> {
    mic::device()?;
    Ok(Some(SavedDefaults(())))
}

/// The virtual microphone's formats around [`match_steam_microphone_format`], for the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MicrophoneFormats {
    pub before: String,
    pub after: String,
    pub recording: String,
}
impl MicrophoneFormats {
    /// Whether the playback format had to change.
    pub fn changed(&self) -> bool {
        self.before != self.after
    }
}

/// The node is one source with one format, so both sides always match.
pub fn match_steam_microphone_format(_playback: &str) -> Result<MicrophoneFormats> {
    let format = "1 channel, 48000 Hz, 32-bit float".to_owned();
    Ok(MicrophoneFormats {
        before: format.clone(),
        after: format.clone(),
        recording: format,
    })
}

/// The recording level of the virtual microphone.
pub struct SteamMicrophoneGain(Arc<mic::Handle>);
impl SteamMicrophoneGain {
    /// Open the recording level of the running node.
    pub fn open() -> Result<Self> {
        use anyhow::Context;
        Ok(Self(
            mic::existing_device().context("the Rubylight Microphone node is not running")?,
        ))
    }
    /// Takes the gain off when the recording level is above 0 dB; the level it had, in dB,
    /// when it changed it.
    pub fn limit(&self) -> Result<Option<f32>> {
        let level = self.0.shared.level();
        if level <= 1.0005 {
            return Ok(None);
        }
        self.0.limit_volume()?;
        Ok(Some(20. * level.log10()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graph::{Node, Snapshot};

    fn node(id: u32, props: &[(&str, &str)]) -> Node {
        Node {
            id,
            props: props
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn endpoints_are_the_sinks_and_the_virtual_microphone() {
        let snapshot = Snapshot {
            nodes: vec![
                node(
                    40,
                    &[
                        ("node.name", "alsa_output.pci"),
                        ("node.description", "Built-in Audio"),
                        ("media.class", "Audio/Sink"),
                        ("device.id", "12"),
                        ("device.description", "Family 17h HD Audio"),
                    ],
                ),
                node(
                    41,
                    &[
                        ("node.name", "alsa_input.pci"),
                        ("media.class", "Audio/Source"),
                    ],
                ),
                node(
                    42,
                    &[
                        ("node.name", "other"),
                        ("media.class", "Stream/Output/Audio"),
                    ],
                ),
                node(
                    43,
                    &[
                        ("node.name", mic::NODE_NAME),
                        ("node.description", "Rubylight Microphone"),
                        ("media.class", "Audio/Source"),
                        ("node.virtual", "true"),
                    ],
                ),
            ],
            defaults: [(
                graph::DEFAULT_SINK.to_owned(),
                graph::json_name("alsa_output.pci"),
            )]
            .into(),
        };
        let list = endpoints_in(&snapshot);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, "alsa_output.pci");
        assert_eq!(list[0].name, "Built-in Audio");
        assert_eq!(list[0].adapter, "Family 17h HD Audio");
        assert!(list[0].default && !list[0].virtual_sink);
        assert!(list[1].virtual_sink && !list[1].default);
        assert_eq!(
            steam_microphone(&list).map(|e| e.id.as_str()),
            Some(mic::NODE_NAME)
        );
    }

    fn journal(before: Option<&str>) -> Journal {
        Journal {
            before: before.map(str::to_owned),
            applied: "ours".into(),
        }
    }

    #[test]
    fn journal_undoes_only_its_own_selection() {
        let j = journal(Some("usb-mic"));
        assert_eq!(j.restore(Some("ours")), Restore::Set("usb-mic".into()));
        // The user picked another device meanwhile.
        assert_eq!(j.restore(Some("headset")), Restore::Nothing);
        // The default was cleared: the original goes back.
        assert_eq!(j.restore(None), Restore::Set("usb-mic".into()));
        assert_eq!(journal(None).restore(Some("ours")), Restore::Nothing);
    }

    #[test]
    fn a_leftover_journal_keeps_the_users_original() {
        let old = journal(Some("usb-mic"));
        // A crash left "ours" as the default: the original is still the first one.
        let again = Journal::new(Some("ours".into()), "ours".into(), Some(&old));
        assert_eq!(again.before.as_deref(), Some("usb-mic"));
        // The user chose another device since: that is the new original.
        let moved = Journal::new(Some("headset".into()), "ours".into(), Some(&old));
        assert_eq!(moved.before.as_deref(), Some("headset"));
        let fresh = Journal::new(None, "ours".into(), None);
        assert_eq!(fresh.before, None);
    }
}
