// Copyright (c) 2026 Chase Payne
// SPDX-License-Identifier: MIT
//
// Ported from Nonary/libvirtualgamepad include/libvirtualgamepad/protocol.h.
// Adapted for Rubylight by RamazanKara (https://github.com/RamazanKara/Rubylight).

//! Fixed-size control IOCTL request and response wire structs for
//! `rubylight-vgamepad`.

use super::PROTOCOL_VERSION;

/// Common 8-byte header beginning every control request and response.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct RequestHeader {
    pub size: u32,
    pub version: u16,
    pub reserved: u16,
}

impl RequestHeader {
    pub const WIRE_SIZE: usize = 8;

    pub const fn new(size: u32) -> Self {
        Self {
            size,
            version: PROTOCOL_VERSION,
            reserved: 0,
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::WIRE_SIZE {
            return None;
        }
        Some(Self {
            size: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            version: u16::from_le_bytes([bytes[4], bytes[5]]),
            reserved: u16::from_le_bytes([bytes[6], bytes[7]]),
        })
    }

    pub fn write_to(&self, out: &mut [u8]) {
        out[0..4].copy_from_slice(&self.size.to_le_bytes());
        out[4..6].copy_from_slice(&self.version.to_le_bytes());
        out[6..8].copy_from_slice(&self.reserved.to_le_bytes());
    }

    pub const fn is_valid(&self, expected_size: usize) -> bool {
        self.size as usize == expected_size
            && self.version == PROTOCOL_VERSION
            && self.reserved == 0
    }
}

/// 16-byte controller creation request (`IOCTL_CREATE_CONTROLLER`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct CreateControllerRequest {
    pub header: RequestHeader,
    pub controller_id: u32,
    pub requested_profile: u16,
    pub reserved: u16,
}

impl CreateControllerRequest {
    pub const WIRE_SIZE: usize = 16;

    pub fn new(controller_id: u32, requested_profile: u16) -> Self {
        Self {
            header: RequestHeader::new(Self::WIRE_SIZE as u32),
            controller_id,
            requested_profile,
            reserved: 0,
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::WIRE_SIZE {
            return None;
        }
        let header = RequestHeader::from_bytes(bytes)?;
        if !header.is_valid(Self::WIRE_SIZE) {
            return None;
        }
        Some(Self {
            header,
            controller_id: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            requested_profile: u16::from_le_bytes([bytes[12], bytes[13]]),
            reserved: u16::from_le_bytes([bytes[14], bytes[15]]),
        })
    }

    pub fn to_bytes(&self) -> [u8; Self::WIRE_SIZE] {
        let mut out = [0u8; Self::WIRE_SIZE];
        self.header.write_to(&mut out[0..8]);
        out[8..12].copy_from_slice(&self.controller_id.to_le_bytes());
        out[12..14].copy_from_slice(&self.requested_profile.to_le_bytes());
        out[14..16].copy_from_slice(&self.reserved.to_le_bytes());
        out
    }
}

/// 8-byte device-wide info query (`IOCTL_QUERY_INFO`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct QueryInfoRequest {
    pub header: RequestHeader,
}

impl QueryInfoRequest {
    pub const WIRE_SIZE: usize = 8;

    pub fn new() -> Self {
        Self {
            header: RequestHeader::new(Self::WIRE_SIZE as u32),
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::WIRE_SIZE {
            return None;
        }
        let header = RequestHeader::from_bytes(bytes)?;
        header.is_valid(Self::WIRE_SIZE).then_some(Self { header })
    }

    pub fn to_bytes(&self) -> [u8; Self::WIRE_SIZE] {
        let mut out = [0u8; Self::WIRE_SIZE];
        self.header.write_to(&mut out);
        out
    }
}

/// Saturated per-slot discard/unknown counters returned in [`QueryInfoResponse`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct FeedbackCounters {
    pub haptic_evicted: u8,
    pub settings_evicted: u8,
    pub transitions_dropped: u8,
    pub unknown_commands: u8,
}

impl FeedbackCounters {
    pub const WIRE_SIZE: usize = 4;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::WIRE_SIZE {
            return None;
        }
        Some(Self {
            haptic_evicted: bytes[0],
            settings_evicted: bytes[1],
            transitions_dropped: bytes[2],
            unknown_commands: bytes[3],
        })
    }

    pub fn write_to(&self, out: &mut [u8]) {
        out[0] = self.haptic_evicted;
        out[1] = self.settings_evicted;
        out[2] = self.transitions_dropped;
        out[3] = self.unknown_commands;
    }
}

/// 28-byte capability and counters response (`IOCTL_QUERY_INFO`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct QueryInfoResponse {
    pub header: RequestHeader,
    pub minimum_protocol_version: u16,
    pub maximum_protocol_version: u16,
    pub available_profiles: u32,
    pub available_features: u32,
    pub maximum_controllers: u32,
    pub counters: FeedbackCounters,
}

impl QueryInfoResponse {
    pub const WIRE_SIZE: usize = 28;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::WIRE_SIZE {
            return None;
        }
        Some(Self {
            header: RequestHeader::from_bytes(&bytes[0..8])?,
            minimum_protocol_version: u16::from_le_bytes([bytes[8], bytes[9]]),
            maximum_protocol_version: u16::from_le_bytes([bytes[10], bytes[11]]),
            available_profiles: u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
            available_features: u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]),
            maximum_controllers: u32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]),
            counters: FeedbackCounters::from_bytes(&bytes[24..28])?,
        })
    }

    pub fn to_bytes(&self) -> [u8; Self::WIRE_SIZE] {
        let mut out = [0u8; Self::WIRE_SIZE];
        self.header.write_to(&mut out[0..8]);
        out[8..10].copy_from_slice(&self.minimum_protocol_version.to_le_bytes());
        out[10..12].copy_from_slice(&self.maximum_protocol_version.to_le_bytes());
        out[12..16].copy_from_slice(&self.available_profiles.to_le_bytes());
        out[16..20].copy_from_slice(&self.available_features.to_le_bytes());
        out[20..24].copy_from_slice(&self.maximum_controllers.to_le_bytes());
        self.counters.write_to(&mut out[24..28]);
        out
    }
}

/// 12-byte controller-addressed request (`destroy`, `poll_feedback`, per-slot `query_info`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ControllerIdRequest {
    pub header: RequestHeader,
    pub controller_id: u32,
}

impl ControllerIdRequest {
    pub const WIRE_SIZE: usize = 12;

    pub fn new(controller_id: u32) -> Self {
        Self {
            header: RequestHeader::new(Self::WIRE_SIZE as u32),
            controller_id,
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::WIRE_SIZE {
            return None;
        }
        let header = RequestHeader::from_bytes(bytes)?;
        if !header.is_valid(Self::WIRE_SIZE) {
            return None;
        }
        Some(Self {
            header,
            controller_id: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
        })
    }

    pub fn to_bytes(&self) -> [u8; Self::WIRE_SIZE] {
        let mut out = [0u8; Self::WIRE_SIZE];
        self.header.write_to(&mut out[0..8]);
        out[8..12].copy_from_slice(&self.controller_id.to_le_bytes());
        out
    }
}

/// 28-byte controller input snapshot (`IOCTL_SUBMIT_INPUT_STATE`).
///
/// Axes use Rubylight/Moonlight normalized conventions: sticks are signed `i16`
/// with **positive = up and right**, and triggers are `0..=255`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct InputStateRequest {
    pub header: RequestHeader,
    pub controller_id: u32,
    pub buttons: u32,
    pub left_x: i16,
    pub left_y: i16,
    pub right_x: i16,
    pub right_y: i16,
    pub left_trigger: u8,
    pub right_trigger: u8,
    pub reserved: u16,
}

impl InputStateRequest {
    pub const WIRE_SIZE: usize = 28;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::WIRE_SIZE {
            return None;
        }
        let header = RequestHeader::from_bytes(bytes)?;
        if !header.is_valid(Self::WIRE_SIZE) {
            return None;
        }
        Some(Self {
            header,
            controller_id: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            buttons: u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
            left_x: i16::from_le_bytes([bytes[16], bytes[17]]),
            left_y: i16::from_le_bytes([bytes[18], bytes[19]]),
            right_x: i16::from_le_bytes([bytes[20], bytes[21]]),
            right_y: i16::from_le_bytes([bytes[22], bytes[23]]),
            left_trigger: bytes[24],
            right_trigger: bytes[25],
            reserved: u16::from_le_bytes([bytes[26], bytes[27]]),
        })
    }

    pub fn to_bytes(&self) -> [u8; Self::WIRE_SIZE] {
        let mut out = [0u8; Self::WIRE_SIZE];
        self.header.write_to(&mut out[0..8]);
        out[8..12].copy_from_slice(&self.controller_id.to_le_bytes());
        out[12..16].copy_from_slice(&self.buttons.to_le_bytes());
        out[16..18].copy_from_slice(&self.left_x.to_le_bytes());
        out[18..20].copy_from_slice(&self.left_y.to_le_bytes());
        out[20..22].copy_from_slice(&self.right_x.to_le_bytes());
        out[22..24].copy_from_slice(&self.right_y.to_le_bytes());
        out[24] = self.left_trigger;
        out[25] = self.right_trigger;
        out[26..28].copy_from_slice(&self.reserved.to_le_bytes());
        out
    }
}

/// 22-byte normalized touch update (`IOCTL_SUBMIT_TOUCH_STATE`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct TouchStateRequest {
    pub header: RequestHeader,
    pub controller_id: u32,
    pub contact_index: u8,
    pub event_type: u8,
    pub x: u16,
    pub y: u16,
    pub pressure: u16,
    pub reserved: u16,
}

impl TouchStateRequest {
    pub const WIRE_SIZE: usize = 22;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::WIRE_SIZE {
            return None;
        }
        let header = RequestHeader::from_bytes(bytes)?;
        if !header.is_valid(Self::WIRE_SIZE) {
            return None;
        }
        Some(Self {
            header,
            controller_id: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            contact_index: bytes[12],
            event_type: bytes[13],
            x: u16::from_le_bytes([bytes[14], bytes[15]]),
            y: u16::from_le_bytes([bytes[16], bytes[17]]),
            pressure: u16::from_le_bytes([bytes[18], bytes[19]]),
            reserved: u16::from_le_bytes([bytes[20], bytes[21]]),
        })
    }

    pub fn to_bytes(&self) -> [u8; Self::WIRE_SIZE] {
        let mut out = [0u8; Self::WIRE_SIZE];
        self.header.write_to(&mut out[0..8]);
        out[8..12].copy_from_slice(&self.controller_id.to_le_bytes());
        out[12] = self.contact_index;
        out[13] = self.event_type;
        out[14..16].copy_from_slice(&self.x.to_le_bytes());
        out[16..18].copy_from_slice(&self.y.to_le_bytes());
        out[18..20].copy_from_slice(&self.pressure.to_le_bytes());
        out[20..22].copy_from_slice(&self.reserved.to_le_bytes());
        out
    }
}

/// 28-byte motion sensor sample (`IOCTL_SUBMIT_MOTION_STATE`), in milli-m/s^2 or milli-deg/s.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct MotionStateRequest {
    pub header: RequestHeader,
    pub controller_id: u32,
    pub motion_type: u8,
    pub reserved0: [u8; 3],
    pub x_milli: i32,
    pub y_milli: i32,
    pub z_milli: i32,
}

impl MotionStateRequest {
    pub const WIRE_SIZE: usize = 28;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::WIRE_SIZE {
            return None;
        }
        let header = RequestHeader::from_bytes(bytes)?;
        if !header.is_valid(Self::WIRE_SIZE) {
            return None;
        }
        Some(Self {
            header,
            controller_id: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            motion_type: bytes[12],
            reserved0: [bytes[13], bytes[14], bytes[15]],
            x_milli: i32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]),
            y_milli: i32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]),
            z_milli: i32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]),
        })
    }

    pub fn to_bytes(&self) -> [u8; Self::WIRE_SIZE] {
        let mut out = [0u8; Self::WIRE_SIZE];
        self.header.write_to(&mut out[0..8]);
        out[8..12].copy_from_slice(&self.controller_id.to_le_bytes());
        out[12] = self.motion_type;
        out[13..16].copy_from_slice(&self.reserved0);
        out[16..20].copy_from_slice(&self.x_milli.to_le_bytes());
        out[20..24].copy_from_slice(&self.y_milli.to_le_bytes());
        out[24..28].copy_from_slice(&self.z_milli.to_le_bytes());
        out
    }
}

/// 16-byte battery state update (`IOCTL_SUBMIT_BATTERY_STATE`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct BatteryStateRequest {
    pub header: RequestHeader,
    pub controller_id: u32,
    pub percent: u8,
    pub flags: u8,
    pub reserved: u16,
}

impl BatteryStateRequest {
    pub const WIRE_SIZE: usize = 16;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::WIRE_SIZE {
            return None;
        }
        let header = RequestHeader::from_bytes(bytes)?;
        if !header.is_valid(Self::WIRE_SIZE) {
            return None;
        }
        Some(Self {
            header,
            controller_id: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            percent: bytes[12],
            flags: bytes[13],
            reserved: u16::from_le_bytes([bytes[14], bytes[15]]),
        })
    }

    pub fn to_bytes(&self) -> [u8; Self::WIRE_SIZE] {
        let mut out = [0u8; Self::WIRE_SIZE];
        self.header.write_to(&mut out[0..8]);
        out[8..12].copy_from_slice(&self.controller_id.to_le_bytes());
        out[12] = self.percent;
        out[13] = self.flags;
        out[14..16].copy_from_slice(&self.reserved.to_le_bytes());
        out
    }
}

/// 80-byte real-hardware Steam Controller firmware attribute update (`IOCTL_SET_STEAM_CONTROLLER_ATTRIBUTES`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct SteamControllerAttributesRequest {
    pub header: RequestHeader,
    pub controller_id: u32,
    pub flags: u32,
    pub firmware_build_time: u32,
    pub bootloader_build_time: u32,
    pub board_revision: u32,
    pub capabilities: u32,
    pub build_id: [u8; 16],
    pub unit_serial: [u8; 16],
    pub board_serial: [u8; 16],
}

impl SteamControllerAttributesRequest {
    pub const WIRE_SIZE: usize = 80;

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::WIRE_SIZE {
            return None;
        }
        let header = RequestHeader::from_bytes(bytes)?;
        if !header.is_valid(Self::WIRE_SIZE) {
            return None;
        }
        let mut out = Self {
            header,
            controller_id: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            flags: u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
            firmware_build_time: u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]),
            bootloader_build_time: u32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]),
            board_revision: u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]),
            capabilities: u32::from_le_bytes([bytes[28], bytes[29], bytes[30], bytes[31]]),
            ..Default::default()
        };
        out.build_id.copy_from_slice(&bytes[32..48]);
        out.unit_serial.copy_from_slice(&bytes[48..64]);
        out.board_serial.copy_from_slice(&bytes[64..80]);
        Some(out)
    }

    pub fn to_bytes(&self) -> [u8; Self::WIRE_SIZE] {
        let mut out = [0u8; Self::WIRE_SIZE];
        self.header.write_to(&mut out[0..8]);
        out[8..12].copy_from_slice(&self.controller_id.to_le_bytes());
        out[12..16].copy_from_slice(&self.flags.to_le_bytes());
        out[16..20].copy_from_slice(&self.firmware_build_time.to_le_bytes());
        out[20..24].copy_from_slice(&self.bootloader_build_time.to_le_bytes());
        out[24..28].copy_from_slice(&self.board_revision.to_le_bytes());
        out[28..32].copy_from_slice(&self.capabilities.to_le_bytes());
        out[32..48].copy_from_slice(&self.build_id);
        out[48..64].copy_from_slice(&self.unit_serial);
        out[64..80].copy_from_slice(&self.board_serial);
        out
    }
}
