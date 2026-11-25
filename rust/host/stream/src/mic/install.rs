//! Steam Streaming Microphone driver installation and default-device restoration.

use anyhow::{Result, bail};
use butterpollo_config::config::Config;
use butterpollo_win_io::audio_route;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

/// After an install that did not bring the device, the next one waits this
/// long: each holds the audio routes while it waits for the device.
pub(super) const INSTALL_BACKOFF: Duration = Duration::from_secs(120);

/// Whether Steam Streaming Microphone may be installed now.
pub(super) struct Install {
    pub(super) installed: bool,
    /// When the last install that has not succeeded started.
    pub(super) tried: Option<Instant>,
}

impl Install {
    /// Claims an install unless one succeeded, runs, or failed recently.
    pub(super) fn begin(&mut self, now: Instant) -> bool {
        let due = !self.installed
            && self
                .tried
                .is_none_or(|at| now.saturating_duration_since(at) >= INSTALL_BACKOFF);
        if due {
            self.tried = Some(now);
        }
        due
    }
}

/// Polls for a newly installed Steam Streaming Microphone endpoint and always
/// restores the previously saved Windows default audio devices on every exit
/// path (found, timeout, or enumeration error).
pub(super) fn wait_for_installed_endpoint(
    timeout: Duration,
    poll: Duration,
    settle: Duration,
    mut find: impl FnMut() -> Result<Option<String>>,
    restore: impl FnOnce() -> Result<()>,
) -> Result<Option<String>> {
    struct RestoreGuard<F: FnOnce() -> Result<()>>(Option<F>);
    impl<F: FnOnce() -> Result<()>> RestoreGuard<F> {
        fn run(&mut self) {
            if let Some(restore) = self.0.take()
                && let Err(error) = restore()
            {
                tracing::warn!(
                    error = %format!("{error:#}"),
                    "default audio devices could not be restored after installing Steam Streaming Microphone"
                );
            }
        }
    }
    impl<F: FnOnce() -> Result<()>> Drop for RestoreGuard<F> {
        fn drop(&mut self) {
            self.run();
        }
    }
    let mut guard = RestoreGuard(Some(restore));
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !poll.is_zero() {
            thread::sleep(poll);
        }
        if let Some(id) = find()? {
            // Windows may make the new device a default when its
            // endpoints arrive; give it a moment, then put them back.
            if !settle.is_zero() {
                thread::sleep(settle);
            }
            guard.run();
            return Ok(Some(id));
        }
    }
    guard.run();
    Ok(None)
}

/// Steam Streaming Microphone's playback endpoint, installed from Steam's
/// drivers the first time a client sends its microphone, when Steam is
/// installed and "install_steam_audio_drivers" is on. A failed install is
/// tried again on a later connection.
pub(super) fn endpoint(config: &Config) -> Result<String> {
    let find = || -> Result<Option<String>> {
        Ok(audio_route::steam_microphone(&audio_route::endpoints()?).map(|e| e.id.clone()))
    };
    if let Some(id) = find()? {
        return Ok(id);
    }
    static INSTALL: Mutex<Install> = Mutex::new(Install {
        installed: false,
        tried: None,
    });
    if INSTALL.lock().unwrap().begin(Instant::now()) {
        let saved = audio_route::install_steam_microphone(config);
        if matches!(saved, Ok(None)) {
            // Nothing was tried (no Steam, or the setting is off): the next
            // connection checks again, as the advice below suggests.
            INSTALL.lock().unwrap().tried = None;
        }
        if let Some(saved) = saved?
            && let Some(id) = wait_for_installed_endpoint(
                Duration::from_secs(10),
                Duration::from_millis(250),
                Duration::from_millis(500),
                find,
                || saved.restore(),
            )?
        {
            INSTALL.lock().unwrap().installed = true;
            tracing::info!("Steam Streaming Microphone installed");
            return Ok(id);
        }
    }
    bail!(
        "Steam Streaming Microphone is not installed. It comes with Steam: install Steam on this PC and keep \"Install Steam streaming audio drivers\" on, then reconnect."
    )
}
