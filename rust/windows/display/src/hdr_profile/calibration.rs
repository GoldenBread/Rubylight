//! Windows HDR calibration profile discovery and MHC2 peak luminance parsing.

use super::{Api, Associations, installed};
use crate::display::Monitor;
use anyhow::{Context, Result};
use std::{collections::BTreeMap, path::PathBuf, sync::Mutex};

/// MHC2 stores the calibration peak as signed 16.16 nits, in big endian.
pub fn peak_luminance(selection: &str) -> Result<Option<u32>> {
    // Effective settings are polled during streaming. A calibration file is
    // normally unchanged; avoid repeated file reads on the encode thread.
    type Peaks = BTreeMap<String, (std::time::Instant, Option<u32>)>;
    static PEAKS: std::sync::OnceLock<Mutex<Peaks>> = std::sync::OnceLock::new();
    let cache = PEAKS.get_or_init(Mutex::default);
    if let Some((at, peak)) = cache.lock().unwrap().get(selection)
        && at.elapsed() < std::time::Duration::from_secs(30)
    {
        return Ok(*peak);
    }
    let name = installed(selection)?;
    let path =
        PathBuf::from(std::env::var_os("SystemRoot").context("Windows directory unavailable")?)
            .join("System32/spool/drivers/color")
            .join(name);
    let mut file = std::fs::File::open(path)?;
    if !(132..=32 * 1024 * 1024).contains(&file.metadata()?.len()) {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(&mut file, 32 * 1024 * 1024 + 1),
        &mut bytes,
    )?;
    let peak = mhc2_peak(&bytes);
    let mut cache = cache.lock().unwrap();
    if cache.len() >= 128 {
        cache.clear();
    }
    cache.insert(selection.into(), (std::time::Instant::now(), peak));
    Ok(peak)
}

pub(super) fn mhc2_peak(bytes: &[u8]) -> Option<u32> {
    if !(132..=32 * 1024 * 1024).contains(&bytes.len()) {
        return None;
    }
    let word = |offset: usize| -> Option<u32> {
        Some(u32::from_be_bytes(
            bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
        ))
    };
    let count = word(128)? as usize;
    if count > (bytes.len() - 132) / 12 {
        return None;
    }
    for index in 0..count {
        let entry = 132 + index * 12;
        if bytes.get(entry..entry + 4)? != b"MHC2" {
            continue;
        }
        let offset = word(entry + 4)? as usize;
        let size = word(entry + 8)? as usize;
        if size < 20
            || offset.checked_add(size)? > bytes.len()
            || bytes.get(offset..offset.checked_add(4)?)? != b"MHC2"
        {
            return None;
        }
        let peak = word(offset + 16)? as i32 as f64 / 65536.;
        return (peak > 0. && peak <= 100000.).then(|| peak.round() as u32);
    }
    None
}

/// A Windows HDR Calibration profile: a display's advanced-colour profile
/// with the MHC2 tag the calibration app writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Calibration {
    /// The physical display it calibrates.
    pub display: String,
    pub profile: String,
    pub peak: u32,
}

/// The calibration of the physical HDR display a virtual display stands in
/// for, the primary display first. Read before the virtual display exists,
/// while the physical displays are on. A SYSTEM host reads the signed-in
/// user's associations as that user.
type SessionUser = fn(
    Box<dyn FnOnce() -> Result<Option<Calibration>>>,
) -> std::io::Result<Result<Option<Calibration>>>;

pub fn calibration_with(as_session_user: SessionUser) -> Result<Option<Calibration>> {
    let api = Api::load()?;
    let displays: Vec<_> = crate::display::monitors()?
        .into_iter()
        .filter(|monitor| monitor.hdr_enabled && !monitor.is_rubylight())
        .collect();
    as_session_user(Box::new(move || {
        find_calibration(&api, &displays, |name| {
            peak_luminance(name)
                .inspect_err(|error| tracing::warn!(%error, profile = name, "HDR calibration profile unreadable"))
                .ok()
                .flatten()
        })
    }))
    .context("the signed-in user's display profiles are unreadable")?
}

/// The first display with a calibration, primary first: the user's
/// association before the system's, as Windows shows a user's own choice.
pub(super) fn find_calibration(
    api: &impl Associations,
    displays: &[Monitor],
    peak: impl Fn(&str) -> Option<u32>,
) -> Result<Option<Calibration>> {
    let mut ordered: Vec<_> = displays.iter().collect();
    ordered.sort_by_key(|monitor| !monitor.primary);
    for monitor in ordered {
        for system in [false, true] {
            let Some(profile) = api.get(monitor, system)? else {
                continue;
            };
            if let Some(peak) = peak(&profile) {
                return Ok(Some(Calibration {
                    display: if monitor.friendly_name.is_empty() {
                        monitor.display_name.clone()
                    } else {
                        monitor.friendly_name.clone()
                    },
                    profile,
                    peak,
                }));
            }
        }
    }
    Ok(None)
}
