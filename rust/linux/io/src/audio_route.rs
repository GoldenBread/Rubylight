//! Audio routing on Linux: no playback or recording device can be listed or switched yet, so
//! a stream's route cannot be made, the device lists are empty, the virtual microphone is
//! never found, and there is nothing to recover.

use anyhow::Result;
use butterpollo_config::config::Config;
use butterpollo_session::session::AudioPreparation;
use serde::Serialize;
use std::{path::Path, sync::Arc};

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

/// The active playback devices. None are listed on Linux.
pub fn endpoints() -> Result<Vec<Endpoint>> {
    Ok(Vec::new())
}

/// The playback side of Steam Streaming Microphone: what is played there, apps record from
/// "Microphone (Steam Streaming Microphone)".
pub fn steam_microphone(endpoints: &[Endpoint]) -> Option<&Endpoint> {
    endpoints.iter().find(|endpoint| {
        [&endpoint.name, &endpoint.adapter].iter().any(|n| {
            n.to_ascii_lowercase()
                .contains("steam streaming microphone")
        })
    })
}

/// A stream's audio route: the device it captures from, and the defaults it holds while it
/// lives. None can be made on Linux.
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
    /// Route a stream's audio. Fails on Linux.
    pub fn acquire(
        _config: &Config,
        _directory: &Path,
        _host_audio: bool,
        _channels: usize,
    ) -> Result<Self> {
        Err(crate::unsupported("Audio routing"))
    }
    /// Keep the virtual speakers the default device.
    pub fn maintain_default(&self) -> Result<()> {
        Ok(())
    }
    /// The device to capture from.
    pub fn capture_sink(&self, _config: &Config) -> Result<String> {
        Ok(self.sink.clone())
    }
    /// Give the virtual speakers `channels` channels.
    pub fn set_channels(&self, _channels: usize) -> Result<()> {
        Ok(())
    }
}

/// Put back the defaults an interrupted stream left. Nothing is left on Linux.
pub fn recover(_directory: &Path) -> Result<()> {
    Ok(())
}

/// Put back the defaults an interrupted microphone left. Nothing is left on Linux.
pub fn recover_microphone(_directory: &Path) -> Result<()> {
    Ok(())
}

/// Temporary defaults for an opened client microphone. None can be made on Linux.
pub struct MicrophoneRoute(());
impl MicrophoneRoute {
    /// Make the client microphone the default recording device. Fails on Linux.
    pub fn acquire(_directory: &Path) -> Result<Self> {
        Err(crate::unsupported("Microphone routing"))
    }
}

/// The default playback and recording devices before a driver install. None are saved on
/// Linux.
pub struct SavedDefaults(());
impl SavedDefaults {
    /// Puts back defaults moved to a device that arrived since.
    pub fn restore(&self) -> Result<()> {
        Ok(())
    }
}

/// Install the virtual microphone's driver. Fails on Linux.
pub fn install_steam_microphone(_config: &Config) -> Result<Option<SavedDefaults>> {
    Err(crate::unsupported("Installing the virtual microphone"))
}

/// The virtual microphone's endpoint formats around [`match_steam_microphone_format`], for
/// the log: the playback side before and after, and the recording side it has to match.
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

/// Give the virtual microphone's playback endpoint `playback` the format of its recording
/// endpoint. Fails on Linux.
pub fn match_steam_microphone_format(_playback: &str) -> Result<MicrophoneFormats> {
    Err(crate::unsupported("Matching the microphone format"))
}

/// The recording level of the virtual microphone. It cannot be opened on Linux.
pub struct SteamMicrophoneGain(());
impl SteamMicrophoneGain {
    /// Open the recording level. Fails on Linux.
    pub fn open() -> Result<Self> {
        Err(crate::unsupported("The microphone level"))
    }
    /// Takes the gain off when the recording level is above 0 dB; the level it had, in dB,
    /// when it changed it.
    pub fn limit(&self) -> Result<Option<f32>> {
        Ok(None)
    }
}
