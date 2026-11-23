//! Pure per-host metadata policies: once-per-PC notices (`HostNotice`), packed 4:4:4 mode cache
//! (`HostPackedModesState`), and normalized capability snapshots (`HostCapabilitiesSnapshot`).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::http::HostCapabilities;

/// PyroWave was chosen and a stream from this PC ran another codec.
pub const NOTICE_PYROWAVE_FALLBACK: &str = "PYROWAVE_FALLBACK";
/// Full chroma is on and a stream from this PC starts without it: the PC or this device cannot do it.
pub const NOTICE_PACKED_UNAVAILABLE: &str = "PACKED_UNAVAILABLE";
/// Frame Warp is on and a stream from this PC runs without it: the PC does not offer it.
pub const NOTICE_FRAME_WARP_UNAVAILABLE: &str = "FRAME_WARP_UNAVAILABLE";

/// Bitmask of packed full-chroma modes known to this client (`HostPackedModes.ALL_MODES`).
pub const ALL_PACKED_MODES: i32 = 0xF;

/// Every once-per-PC notice shared with Android's `HostNotices.Notice`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HostNotice {
    VirtualDisplayUnavailable,
    AutomaticQualityUnavailable,
    PyroWaveUnavailable,
    HdrUnavailable,
    PackedHdrUnavailable,
    PyroWaveHdrUnavailable,
    VulkanHdrLimit,
    FrameWarpHdrLimit,
    PyroWaveHdrLimit,
    CodecUnavailable,
    PyroWaveFallback,
    Host4kUnavailable,
    NativeTouchUnavailable,
    Sdr10Unavailable,
    Sdr10NotAccepted,
    Sdr10HostHdr,
    VulkanHdr10PlusRefused,
    VulkanBehind,
    PackedUnavailable,
    NativeTouchpadUnavailable,
}

impl HostNotice {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::VirtualDisplayUnavailable => "VIRTUAL_DISPLAY_UNAVAILABLE",
            Self::AutomaticQualityUnavailable => "AUTOMATIC_QUALITY_UNAVAILABLE",
            Self::PyroWaveUnavailable => "PYROWAVE_UNAVAILABLE",
            Self::HdrUnavailable => "HDR_UNAVAILABLE",
            Self::PackedHdrUnavailable => "PACKED_HDR_UNAVAILABLE",
            Self::PyroWaveHdrUnavailable => "PYROWAVE_HDR_UNAVAILABLE",
            Self::VulkanHdrLimit => "VULKAN_HDR_LIMIT",
            Self::FrameWarpHdrLimit => "FRAME_WARP_HDR_LIMIT",
            Self::PyroWaveHdrLimit => "PYROWAVE_HDR_LIMIT",
            Self::CodecUnavailable => "CODEC_UNAVAILABLE",
            Self::PyroWaveFallback => "PYROWAVE_FALLBACK",
            Self::Host4kUnavailable => "HOST_4K_UNAVAILABLE",
            Self::NativeTouchUnavailable => "NATIVE_TOUCH_UNAVAILABLE",
            Self::Sdr10Unavailable => "SDR10_UNAVAILABLE",
            Self::Sdr10NotAccepted => "SDR10_NOT_ACCEPTED",
            Self::Sdr10HostHdr => "SDR10_HOST_HDR",
            Self::VulkanHdr10PlusRefused => "VULKAN_HDR10_PLUS_REFUSED",
            Self::VulkanBehind => "VULKAN_BEHIND",
            Self::PackedUnavailable => "PACKED_UNAVAILABLE",
            Self::NativeTouchpadUnavailable => "NATIVE_TOUCHPAD_UNAVAILABLE",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "VIRTUAL_DISPLAY_UNAVAILABLE" => Some(Self::VirtualDisplayUnavailable),
            "AUTOMATIC_QUALITY_UNAVAILABLE" => Some(Self::AutomaticQualityUnavailable),
            "PYROWAVE_UNAVAILABLE" => Some(Self::PyroWaveUnavailable),
            "HDR_UNAVAILABLE" => Some(Self::HdrUnavailable),
            "PACKED_HDR_UNAVAILABLE" => Some(Self::PackedHdrUnavailable),
            "PYROWAVE_HDR_UNAVAILABLE" => Some(Self::PyroWaveHdrUnavailable),
            "VULKAN_HDR_LIMIT" => Some(Self::VulkanHdrLimit),
            "FRAME_WARP_HDR_LIMIT" => Some(Self::FrameWarpHdrLimit),
            "PYROWAVE_HDR_LIMIT" => Some(Self::PyroWaveHdrLimit),
            "CODEC_UNAVAILABLE" => Some(Self::CodecUnavailable),
            "PYROWAVE_FALLBACK" => Some(Self::PyroWaveFallback),
            "HOST_4K_UNAVAILABLE" => Some(Self::Host4kUnavailable),
            "NATIVE_TOUCH_UNAVAILABLE" => Some(Self::NativeTouchUnavailable),
            "SDR10_UNAVAILABLE" => Some(Self::Sdr10Unavailable),
            "SDR10_NOT_ACCEPTED" => Some(Self::Sdr10NotAccepted),
            "SDR10_HOST_HDR" => Some(Self::Sdr10HostHdr),
            "VULKAN_HDR10_PLUS_REFUSED" => Some(Self::VulkanHdr10PlusRefused),
            "VULKAN_BEHIND" => Some(Self::VulkanBehind),
            "PACKED_UNAVAILABLE" => Some(Self::PackedUnavailable),
            "NATIVE_TOUCHPAD_UNAVAILABLE" => Some(Self::NativeTouchpadUnavailable),
            _ => None,
        }
    }
}

/// Per-UUID state of shown notices (`HostNotices`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostNoticesState {
    pub hosts: BTreeMap<String, BTreeSet<String>>,
}

impl HostNoticesState {
    pub fn claim(&mut self, uuid: &str, notice: &str) -> bool {
        self.hosts
            .entry(uuid.to_owned())
            .or_default()
            .insert(notice.to_owned())
    }

    pub fn reset(&mut self, uuid: &str, notice: &str) -> bool {
        let Some(set) = self.hosts.get_mut(uuid) else {
            return false;
        };
        let removed = set.remove(notice);
        if set.is_empty() {
            self.hosts.remove(uuid);
        }
        removed
    }

    pub fn reset_everywhere(&mut self, notice: &str) -> bool {
        let mut changed = false;
        self.hosts.retain(|_, set| {
            if set.remove(notice) {
                changed = true;
            }
            !set.is_empty()
        });
        changed
    }

    pub fn forget(&mut self, uuid: &str) -> bool {
        self.hosts.remove(uuid).is_some()
    }

    pub fn import_csv(&mut self, uuid: &str, csv: &str) -> bool {
        let mut changed = false;
        let set = self.hosts.entry(uuid.to_owned()).or_default();
        for token in csv.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            if set.insert(token.to_owned()) {
                changed = true;
            }
        }
        if set.is_empty() {
            self.hosts.remove(uuid);
        }
        changed
    }
}

/// Per-UUID packed 4:4:4 mode cache (`HostPackedModes`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HostPackedEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<u8>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub refused: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stamp: Option<String>,
}

impl HostPackedEntry {
    pub fn is_empty(&self) -> bool {
        self.mask.is_none() && !self.refused && self.stamp.is_none()
    }
}

/// Pure policy state machine for `HostPackedModes`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostPackedModesState {
    pub hosts: BTreeMap<String, HostPackedEntry>,
}

impl HostPackedModesState {
    pub fn cached(&self, uuid: &str) -> i32 {
        let Some(entry) = self.hosts.get(uuid) else {
            return -1;
        };
        let modes = i32::from(entry.mask.unwrap_or(0)) & ALL_PACKED_MODES;
        if modes > 0 { modes } else { -1 }
    }

    pub fn store(&mut self, uuid: &str, describe_modes: i32) -> bool {
        if describe_modes < 0 {
            return false;
        }
        let masked = (describe_modes & ALL_PACKED_MODES) as u8;
        if masked > 0 {
            let entry = self.hosts.entry(uuid.to_owned()).or_default();
            if entry.mask == Some(masked) && !entry.refused {
                return false;
            }
            entry.mask = Some(masked);
            entry.refused = false;
            true
        } else if let Some(entry) = self.hosts.get_mut(uuid)
            && entry.mask.is_some()
        {
            entry.mask = None;
            if entry.is_empty() {
                self.hosts.remove(uuid);
            }
            true
        } else {
            false
        }
    }

    pub fn mark_refused(&mut self, uuid: &str) -> bool {
        let entry = self.hosts.entry(uuid.to_owned()).or_default();
        if entry.refused {
            return false;
        }
        entry.refused = true;
        true
    }

    pub fn refused(&self, uuid: &str) -> bool {
        self.hosts.get(uuid).is_some_and(|e| e.refused)
    }

    pub fn stamp(app_version: &str, host_version: Option<&str>) -> String {
        format!("{app_version}/{}", host_version.unwrap_or(""))
    }

    pub fn renew(&mut self, uuid: &str, stamp: &str) -> bool {
        let entry = self.hosts.entry(uuid.to_owned()).or_default();
        if entry.stamp.as_deref() == Some(stamp) {
            return false;
        }
        entry.stamp = Some(stamp.to_owned());
        entry.refused = false;
        true
    }

    pub fn clear_refusals(&mut self) -> bool {
        let mut changed = false;
        self.hosts.retain(|_, entry| {
            if entry.refused {
                entry.refused = false;
                changed = true;
            }
            !entry.is_empty()
        });
        changed
    }

    pub fn forget(&mut self, uuid: &str) -> bool {
        self.hosts.remove(uuid).is_some()
    }

    pub const fn known(server_info_modes: i32, cached_modes: i32) -> i32 {
        if server_info_modes >= 0 {
            server_info_modes
        } else {
            cached_modes
        }
    }
}

/// Normalized capability snapshot so identical capability writes are no-ops and damaged entries
/// degrade to `None` (never seen).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostCapabilitiesSnapshot {
    caps: HostCapabilities,
    canonical_json: String,
}

impl HostCapabilitiesSnapshot {
    /// Parses a versioned capabilities JSON string. Returns `None` if unknown or malformed.
    pub fn parse(json: &str) -> Option<Self> {
        let caps = HostCapabilities::from_versioned_json(json);
        let canonical_json = caps.to_versioned_json()?;
        Some(Self {
            caps,
            canonical_json,
        })
    }

    pub fn capabilities(&self) -> &HostCapabilities {
        &self.caps
    }

    pub fn canonical_json(&self) -> &str {
        &self.canonical_json
    }
}
