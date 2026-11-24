//! Active playback session inspection and endpoint-ID helpers for per-app routing.
use anyhow::Result;
use windows::{Win32::Media::Audio::*, core::Interface};

pub(super) const RENDER_INTERFACE: &str = "#{e6327cad-dcec-4949-ae8a-991e976a79d2}";
pub(super) const MMDEVAPI: &str = r"\\?\SWD#MMDEVAPI#";

/// Where the game's sound reaches the PC ([`plays_elsewhere`]).
#[derive(Debug, Default, PartialEq)]
pub struct OnThePc {
    /// The game's processes playing there.
    pub pids: Vec<u32>,
    /// One plays on the PC's default communications device, which is also
    /// its default or the game's own device: that may be the game's voice
    /// chat, which stays on it ([`super::ROLES`]).
    pub maybe_voice: bool,
}

/// The game's processes (`pids`) with an active session (`sessions`: pid
/// and endpoint ID) where the PC plays the game: its default device for
/// [`super::ROLES`] (`defaults`: console, multimedia, communications) or the
/// game's own device (`own`), not the sink, even beside a session on the
/// sink. Windows moves no stream that is already open, so a game that
/// opened its sound before it was routed keeps it on the PC's device until
/// it opens it again. Another communications device is voice chat's, never
/// counted.
pub(super) fn plays_elsewhere(
    pids: &[u32],
    sessions: &[(u32, String)],
    sink: &str,
    defaults: &[Option<String>; 3],
    own: &[&str],
) -> OnThePc {
    let [console, multimedia, communications] = defaults;
    let same = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
    let watched: Vec<&str> = [console.as_deref(), multimedia.as_deref()]
        .into_iter()
        .flatten()
        .chain(own.iter().copied().filter(|device| !device.is_empty()))
        .filter(|device| !same(device, sink))
        .collect();
    let mut found = OnThePc::default();
    for (pid, endpoint) in sessions {
        if !pids.contains(pid) || !watched.iter().any(|device| same(device, endpoint)) {
            continue;
        }
        found.pids.push(*pid);
        found.maybe_voice |= communications
            .as_deref()
            .is_some_and(|device| same(device, endpoint));
    }
    found.pids.sort_unstable();
    found.pids.dedup();
    found
}

/// The endpoint ID in a per-app device (`\\?\SWD#MMDEVAPI#{id}#{interface}`).
pub(super) fn endpoint_id(device: &str) -> &str {
    let lower = device.to_ascii_lowercase();
    let start = if lower.starts_with(&MMDEVAPI.to_ascii_lowercase()) {
        MMDEVAPI.len()
    } else {
        0
    };
    let end = if lower.ends_with(RENDER_INTERFACE) {
        device.len() - RENDER_INTERFACE.len()
    } else {
        device.len()
    };
    device.get(start..end.max(start)).unwrap_or(device)
}

/// Active playback sessions: process ID and endpoint ID.
pub(super) fn active_sessions() -> Result<Vec<(u32, String)>> {
    sessions(true)
}
/// The processes with a playback session, active or not: those that opened
/// their sound.
pub(super) fn sound_pids() -> Result<Vec<u32>> {
    Ok(sessions(false)?.into_iter().map(|(pid, _)| pid).collect())
}
/// Playback sessions (only active ones when `active`): process ID and
/// endpoint ID.
fn sessions(active: bool) -> Result<Vec<(u32, String)>> {
    let _com = crate::os_helpers::ComGuard::new()?;
    // SAFETY: COM is initialised by `_com`; every interface used is live and owned here.
    unsafe {
        let enumerator: IMMDeviceEnumerator = windows::Win32::System::Com::CoCreateInstance(
            &MMDeviceEnumerator,
            None,
            windows::Win32::System::Com::CLSCTX_ALL,
        )?;
        let list = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
        let mut sessions = vec![];
        for index in 0..list.GetCount()? {
            let device = list.Item(index)?;
            let id = crate::audio_route::device_id(&device)?;
            let manager: IAudioSessionManager2 =
                device.Activate(windows::Win32::System::Com::CLSCTX_ALL, None)?;
            let all = manager.GetSessionEnumerator()?;
            for at in 0..all.GetCount()? {
                let control: IAudioSessionControl2 = all.GetSession(at)?.cast()?;
                if !active || control.GetState()? == AudioSessionStateActive {
                    sessions.push((control.GetProcessId().unwrap_or(0), id.clone()));
                }
            }
        }
        Ok(sessions)
    }
}
