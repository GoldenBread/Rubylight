//! Steam Streaming Microphone routing, driver installation, format matching, and gain capping.
use super::{
    ACTIVE, Active,
    format::{describe_format, sample_layout},
    hooks, pending,
    policy::{Endpoint, Policy, ROLES, defaults_for, endpoints_for, enumerator},
    save,
};
use anyhow::{Context, Result, bail};
use butterpollo_config::config::Config;
use std::path::{Path, PathBuf};
use windows::{
    Win32::{Media::Audio::*, System::Com::CLSCTX_ALL},
    core::PCWSTR,
};

/// The playback side of Steam Streaming Microphone: what is played there,
/// apps record from "Microphone (Steam Streaming Microphone)".
pub fn steam_microphone(endpoints: &[Endpoint]) -> Option<&Endpoint> {
    endpoints.iter().find(|endpoint| {
        [&endpoint.name, &endpoint.adapter].iter().any(|n| {
            n.to_ascii_lowercase()
                .contains("steam streaming microphone")
        })
    })
}

// Serialize microphone changes with playback routing and driver installation.
// The flag is only read/written while ACTIVE is locked. The separate journal
// outlives the playback route and is restored even if playback ends first.
static MICROPHONE_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(super) fn microphone_path(directory: &Path) -> PathBuf {
    directory.join("microphone-recovery.json")
}

pub(super) fn microphone_pending(
    directory: &Path,
) -> Result<Option<butterpollo_media_policy::audio_defaults::MicrophoneDefaults>> {
    match std::fs::read(microphone_path(directory)) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn microphone_owns_recording(directory: &Path) -> bool {
    MICROPHONE_ACTIVE.load(std::sync::atomic::Ordering::Relaxed)
        || microphone_pending(directory)
            .map(|pending| pending.is_some())
            .unwrap_or(true)
}

fn restore_microphone(directory: &Path) -> Result<()> {
    use butterpollo_media_policy::audio_defaults::MicrophoneDefaults;
    let Some(journal) = microphone_pending(directory)? else {
        return Ok(());
    };
    let policy = Policy::new()?;
    let current = defaults_for(eCapture)?;
    let mut failed = None;
    for (index, target) in journal.restore(&current) {
        match policy.set_default(&target, ROLES[index]) {
            Ok(()) => {
                tracing::info!(role = butterpollo_media_policy::audio_defaults::ROLE_NAMES[index], device = %target, "microphone default restored")
            }
            Err(error) => {
                failed.get_or_insert(error);
            }
        }
    }
    // Keep the original journal on any failure. Successful roles no longer
    // point to our endpoint and are therefore left alone on the next retry.
    if let Some(error) = failed {
        return Err(error.context("restoring the previous microphone"));
    }
    butterpollo_session::state::write_json(
        &microphone_path(directory),
        &Option::<MicrophoneDefaults>::None,
    )
}

/// Temporary defaults for a successfully opened client microphone renderer.
/// Dropped on mute/idle, disconnect, handover, output failure or host shutdown.
pub struct MicrophoneRoute {
    directory: PathBuf,
}
impl MicrophoneRoute {
    pub fn acquire(directory: &Path) -> Result<Self> {
        use butterpollo_media_policy::audio_defaults::MicrophoneDefaults;
        let _com = crate::os_helpers::ComGuard::new()?;
        let active = ACTIVE.lock().unwrap();
        if MICROPHONE_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
            bail!("a client microphone already owns the recording defaults");
        }
        if active
            .as_ref()
            .is_some_and(|route| route.directory != directory)
        {
            bail!("audio is owned by another host configuration");
        }
        let available = endpoints_for(eCapture)?;
        let selected = steam_microphone(&available)
            .context("Steam Streaming Microphone recording endpoint is unavailable")?;
        let policy = Policy::new()?;
        let current = defaults_for(eCapture)?;
        let pending_mic = microphone_pending(directory)?;
        let journal =
            MicrophoneDefaults::new(current.clone(), selected.id.clone(), pending_mic.as_ref());
        // Persist and arm crash recovery before the first Windows mutation.
        butterpollo_session::state::write_json(&microphone_path(directory), &Some(&journal))?;
        hooks::mark_microphone(true)?;
        let result = (|| -> Result<()> {
            // Once forwarding owns recording restoration, the older playback
            // snapshot must not undo the choices saved at microphone start.
            if let Some(mut playback) = pending(directory) {
                playback.capture_before = Default::default();
                save(directory, &playback)?;
            }
            for index in journal.apply(&current) {
                policy.set_default(&selected.id, ROLES[index])?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            if restore_microphone(directory).is_ok() {
                let _ = hooks::mark_microphone(false);
            }
            return Err(error);
        }
        MICROPHONE_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
        tracing::info!(device = %selected.name, "client microphone selected as the Windows recording default");
        Ok(Self {
            directory: directory.into(),
        })
    }
}
impl Drop for MicrophoneRoute {
    fn drop(&mut self) {
        let _active = ACTIVE.lock().unwrap();
        MICROPHONE_ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
        let result =
            crate::os_helpers::ComGuard::new().and_then(|_| restore_microphone(&self.directory));
        if let Err(error) = result {
            tracing::warn!(%error, "microphone defaults will be retried by crash recovery");
        } else {
            let _ = hooks::mark_microphone(false);
        }
    }
}

pub fn recover_microphone(directory: &Path) -> Result<()> {
    let _active = ACTIVE
        .try_lock()
        .map_err(|_| anyhow::anyhow!("audio routing is busy"))?;
    if MICROPHONE_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
        bail!("microphone routing is in use");
    }
    let _com = crate::os_helpers::ComGuard::new()?;
    restore_microphone(directory)
}

/// The default playback and recording devices before a driver install.
pub struct SavedDefaults {
    defaults: [[Option<String>; 3]; 2],
    // A stream must not restore its defaults before driver arrival is undone.
    _active: std::sync::MutexGuard<'static, Option<Active>>,
}
impl SavedDefaults {
    /// Puts back defaults Windows moved to a device that arrived since.
    /// Attempts every playback and recording role so one unavailable endpoint
    /// never skips restoring the remaining roles; returns the first error.
    pub fn restore(&self) -> Result<()> {
        let policy = Policy::new()?;
        restore_saved_defaults(&self.defaults, defaults_for, |device, role| {
            policy.set_default(device, role)
        })
    }
}

pub(super) fn restore_saved_defaults(
    defaults: &[[Option<String>; 3]; 2],
    mut current_for: impl FnMut(EDataFlow) -> Result<[Option<String>; 3]>,
    mut set_default: impl FnMut(&str, ERole) -> Result<()>,
) -> Result<()> {
    let mut first_error = None;
    for (flow, saved) in [eRender, eCapture].into_iter().zip(defaults) {
        let current = match current_for(flow) {
            Ok(current) => current,
            Err(error) => {
                tracing::warn!(
                    flow = flow.0,
                    error = %format!("{error:#}"),
                    "saved audio defaults could not be read for restore"
                );
                first_error.get_or_insert(error);
                continue;
            }
        };
        for ((role, now), before) in ROLES.into_iter().zip(current).zip(saved) {
            if let Some(before) = before
                && now.as_ref() != Some(before)
                && let Err(error) = set_default(before, role)
            {
                tracing::warn!(
                    flow = flow.0,
                    role = role.0,
                    device = %before,
                    error = %format!("{error:#}"),
                    "saved audio default could not be restored"
                );
                first_error.get_or_insert(error);
            }
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// Install Steam Streaming Microphone when Steam provides it, as with the
/// speakers. Returns the defaults from before, to put back once its
/// endpoints arrive: Windows can make a new device the default, and the
/// install must not choose the PC's microphone or speakers. None when the
/// driver was not installed.
pub fn install_steam_microphone(config: &Config) -> Result<Option<SavedDefaults>> {
    if !config.boolean("install_steam_audio_drivers", true) {
        return Ok(None);
    }
    let Some(directory) = std::env::var_os("CommonProgramFiles(x86)") else {
        return Ok(None);
    };
    let inf =
        PathBuf::from(directory).join("Steam/drivers/Windows10/x64/SteamStreamingMicrophone.inf");
    if !inf.is_file() {
        return Ok(None);
    }
    let active = ACTIVE.lock().unwrap();
    let saved = SavedDefaults {
        defaults: [defaults_for(eRender)?, defaults_for(eCapture)?],
        _active: active,
    };
    let file = crate::text::to_wide(&inf.to_string_lossy());
    // SAFETY: `file` is NUL-terminated and outlives the call.
    unsafe {
        use windows::Win32::Devices::DeviceAndDriverInstallation::{
            DIIRFLAG_FORCE_INF, DiInstallDriverW,
        };
        DiInstallDriverW(None, PCWSTR(file.as_ptr()), DIIRFLAG_FORCE_INF, None)?;
    }
    Ok(Some(saved))
}

/// Steam Streaming Microphone hands what plays on "Speakers (Steam Streaming
/// Microphone)" to "Microphone (Steam Streaming Microphone)" sample for
/// sample, without converting. With its own defaults (stereo 48 kHz playback,
/// mono 44.1 kHz recording) a voice reached apps at 0.46 times its pitch and
/// in pieces (issue 56). The playback format to set, the
/// recording side's, or None when the two already agree.
pub(super) fn matching_playback_format(playback: &[u8], recording: &[u8]) -> Option<Vec<u8>> {
    (sample_layout(playback) != sample_layout(recording)).then(|| recording.to_vec())
}

/// Steam Streaming Microphone's endpoint formats around
/// `match_steam_microphone_format`, for the log: the playback side before and
/// after, and the recording side it has to match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MicrophoneFormats {
    pub before: String,
    pub after: String,
    pub recording: String,
}
impl MicrophoneFormats {
    pub(super) fn new(before: &[u8], after: &[u8], recording: &[u8]) -> Self {
        Self {
            before: describe_format(before),
            after: describe_format(after),
            recording: describe_format(recording),
        }
    }
    /// Whether the playback format had to change.
    pub fn changed(&self) -> bool {
        self.before != self.after
    }
}

/// The playback format read back after setting it: an error unless it now
/// matches the recording side, since Windows can accept a format and keep
/// the old one.
pub(super) fn check_matched(after: &[u8], recording: &[u8]) -> Result<()> {
    if matching_playback_format(after, recording).is_some() {
        bail!(
            "its playback format is still {} after setting it to the recording format {}",
            describe_format(after),
            describe_format(recording)
        );
    }
    Ok(())
}

/// Gives Steam Streaming Microphone's playback endpoint `playback` the format
/// of its recording endpoint (`matching_playback_format`); Windows converts the
/// host's stream to it. Returns the formats before and after, read back from
/// Windows, whether or not one changed.
pub fn match_steam_microphone_format(playback: &str) -> Result<MicrophoneFormats> {
    let recording = steam_microphone(&endpoints_for(eCapture)?)
        .context("Microphone (Steam Streaming Microphone) is not present")?
        .id
        .clone();
    let policy = Policy::new()?;
    let before = policy.format(playback)?;
    let recording = policy.format(&recording)?;
    let Some(desired) = matching_playback_format(&before, &recording) else {
        return Ok(MicrophoneFormats::new(&before, &before, &recording));
    };
    policy.set_format(playback, &desired).with_context(|| {
        format!(
            "setting its playback format to {}",
            describe_format(&desired)
        )
    })?;
    let after = policy.format(playback)?;
    check_matched(&after, &recording)?;
    Ok(MicrophoneFormats::new(&before, &after, &recording))
}

/// The recording level, in dB, to put "Microphone (Steam Streaming
/// Microphone)" back to, or None to leave it. Its slider runs from -96 to
/// +30 dB with 0 dB at about 54%: at 100%, or where an app's automatic gain
/// pushed it, a client's voice came out 30 dB too loud and clipped into
/// noise (issue 56). The client sets the voice's level; the host never adds
/// gain on top, while a lower level stays the user's choice.
pub(super) fn unity_cap(level_db: f32) -> Option<f32> {
    // Above half a slider step: the endpoint rounds levels to 1.5 dB steps.
    (level_db > 0.75).then_some(0.0)
}

/// The recording level of "Microphone (Steam Streaming Microphone)", opened
/// once so that checking it again costs the render thread one call.
pub struct SteamMicrophoneGain {
    volume: windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume,
}
impl SteamMicrophoneGain {
    /// COM must be initialised on the calling thread, as for `limit`.
    pub fn open() -> Result<Self> {
        let recording = steam_microphone(&endpoints_for(eCapture)?)
            .context("Microphone (Steam Streaming Microphone) is not present")?
            .id
            .clone();
        let wide = crate::text::to_wide(&recording);
        // SAFETY: the caller initialised COM, and `wide` is NUL-terminated and outlives the call.
        unsafe {
            let device = enumerator()?.GetDevice(PCWSTR(wide.as_ptr()))?;
            Ok(Self {
                volume: device.Activate(CLSCTX_ALL, None)?,
            })
        }
    }
    /// Takes the gain off when the recording level is above 0 dB
    /// (`unity_cap`). The level it had, in dB, when it changed it.
    pub fn limit(&self) -> Result<Option<f32>> {
        // SAFETY: `self.volume` is a live endpoint volume on a thread with COM initialised.
        unsafe {
            let level = self.volume.GetMasterVolumeLevel()?;
            let Some(target) = unity_cap(level) else {
                return Ok(None);
            };
            self.volume
                .SetMasterVolumeLevel(target, std::ptr::null())
                .with_context(|| {
                    format!("lowering its recording level from {level:.1} dB to {target} dB")
                })?;
            Ok(Some(level))
        }
    }
}
