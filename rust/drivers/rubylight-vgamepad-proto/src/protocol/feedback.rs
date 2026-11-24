// Copyright (c) 2026 Chase Payne
// SPDX-License-Identifier: MIT
//
// Ported from Nonary/libvirtualgamepad include/libvirtualgamepad/protocol.h.
// Adapted for Rubylight by RamazanKara (https://github.com/RamazanKara/Rubylight).

//! Fixed-size feedback payload and event wire structs (`IOCTL_POLL_FEEDBACK`)
//! for `rubylight-vgamepad`.

use super::{FeedbackType, RequestHeader, STEAM_SETTINGS_TRIPLES_MAX};

/// 8-byte generic rumble/RGB feedback payload (`FeedbackType::GenericRumbleRgb` / `GenericRumble`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct GenericRumbleRgbFeedback {
    pub low_frequency: u16,
    pub high_frequency: u16,
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub reserved: u8,
}

impl GenericRumbleRgbFeedback {
    pub const WIRE_SIZE: usize = 8;

    pub fn to_bytes(&self) -> [u8; Self::WIRE_SIZE] {
        let mut out = [0u8; Self::WIRE_SIZE];
        out[0..2].copy_from_slice(&self.low_frequency.to_le_bytes());
        out[2..4].copy_from_slice(&self.high_frequency.to_le_bytes());
        out[4] = self.red;
        out[5] = self.green;
        out[6] = self.blue;
        out[7] = self.reserved;
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::WIRE_SIZE {
            return None;
        }
        Some(Self {
            low_frequency: u16::from_le_bytes([bytes[0], bytes[1]]),
            high_frequency: u16::from_le_bytes([bytes[2], bytes[3]]),
            red: bytes[4],
            green: bytes[5],
            blue: bytes[6],
            reserved: bytes[7],
        })
    }
}

/// 8-byte four-motor Xbox rumble feedback (`FeedbackType::XboxRumble`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct XboxRumbleFeedback {
    pub low_frequency: u16,
    pub high_frequency: u16,
    pub left_trigger: u16,
    pub right_trigger: u16,
}

impl XboxRumbleFeedback {
    pub const WIRE_SIZE: usize = 8;

    pub fn to_bytes(&self) -> [u8; Self::WIRE_SIZE] {
        let mut out = [0u8; Self::WIRE_SIZE];
        out[0..2].copy_from_slice(&self.low_frequency.to_le_bytes());
        out[2..4].copy_from_slice(&self.high_frequency.to_le_bytes());
        out[4..6].copy_from_slice(&self.left_trigger.to_le_bytes());
        out[6..8].copy_from_slice(&self.right_trigger.to_le_bytes());
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::WIRE_SIZE {
            return None;
        }
        Some(Self {
            low_frequency: u16::from_le_bytes([bytes[0], bytes[1]]),
            high_frequency: u16::from_le_bytes([bytes[2], bytes[3]]),
            left_trigger: u16::from_le_bytes([bytes[4], bytes[5]]),
            right_trigger: u16::from_le_bytes([bytes[6], bytes[7]]),
        })
    }
}

/// 11-byte DualSense adaptive trigger program.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct TriggerEffectFeedback {
    pub mode: u8,
    pub parameters: [u8; 10],
}

impl TriggerEffectFeedback {
    pub const WIRE_SIZE: usize = 11;

    pub fn write_to(&self, out: &mut [u8]) {
        out[0] = self.mode;
        out[1..11].copy_from_slice(&self.parameters);
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::WIRE_SIZE {
            return None;
        }
        let mut parameters = [0u8; 10];
        parameters.copy_from_slice(&bytes[1..11]);
        Some(Self {
            mode: bytes[0],
            parameters,
        })
    }
}

/// 32-byte PlayStation / Switch rumble, LED, and trigger-effect feedback (`FeedbackType::PlayStationOutput`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PlayStationOutputFeedback {
    pub low_frequency: u16,
    pub high_frequency: u16,
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub valid: u8,
    pub player_leds: u8,
    pub microphone_led: u8,
    pub left_trigger: TriggerEffectFeedback,
    pub right_trigger: TriggerEffectFeedback,
}

impl PlayStationOutputFeedback {
    pub const WIRE_SIZE: usize = 32;

    pub fn to_bytes(&self) -> [u8; Self::WIRE_SIZE] {
        let mut out = [0u8; Self::WIRE_SIZE];
        out[0..2].copy_from_slice(&self.low_frequency.to_le_bytes());
        out[2..4].copy_from_slice(&self.high_frequency.to_le_bytes());
        out[4] = self.red;
        out[5] = self.green;
        out[6] = self.blue;
        out[7] = self.valid;
        out[8] = self.player_leds;
        out[9] = self.microphone_led;
        self.left_trigger.write_to(&mut out[10..21]);
        self.right_trigger.write_to(&mut out[21..32]);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::WIRE_SIZE {
            return None;
        }
        Some(Self {
            low_frequency: u16::from_le_bytes([bytes[0], bytes[1]]),
            high_frequency: u16::from_le_bytes([bytes[2], bytes[3]]),
            red: bytes[4],
            green: bytes[5],
            blue: bytes[6],
            valid: bytes[7],
            player_leds: bytes[8],
            microphone_led: bytes[9],
            left_trigger: TriggerEffectFeedback::from_bytes(&bytes[10..21])?,
            right_trigger: TriggerEffectFeedback::from_bytes(&bytes[21..32])?,
        })
    }
}

/// 16-byte Steam Controller haptic output payload (`FeedbackType::SteamHaptic`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct SteamHapticFeedback {
    pub length: u8,
    pub reserved: [u8; 3],
    pub report: [u8; 12],
}

impl SteamHapticFeedback {
    pub const WIRE_SIZE: usize = 16;

    pub fn to_bytes(&self) -> [u8; Self::WIRE_SIZE] {
        let mut out = [0u8; Self::WIRE_SIZE];
        out[0] = self.length;
        out[1..4].copy_from_slice(&self.reserved);
        out[4..16].copy_from_slice(&self.report);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::WIRE_SIZE {
            return None;
        }
        let mut report = [0u8; 12];
        report.copy_from_slice(&bytes[4..16]);
        Some(Self {
            length: bytes[0],
            reserved: [bytes[1], bytes[2], bytes[3]],
            report,
        })
    }
}

/// 32-byte Steam Controller settings command chunk (`FeedbackType::SteamSettings`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct SteamSettingsFeedback {
    pub command: u8,
    pub count: u8,
    pub reserved: [u8; 3],
    pub triples: [u8; STEAM_SETTINGS_TRIPLES_MAX * 3],
}

impl SteamSettingsFeedback {
    pub const WIRE_SIZE: usize = 32;

    pub fn to_bytes(&self) -> [u8; Self::WIRE_SIZE] {
        let mut out = [0u8; Self::WIRE_SIZE];
        out[0] = self.command;
        out[1] = self.count;
        out[2..5].copy_from_slice(&self.reserved);
        out[5..32].copy_from_slice(&self.triples);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::WIRE_SIZE {
            return None;
        }
        let mut triples = [0u8; 27];
        triples.copy_from_slice(&bytes[5..32]);
        Some(Self {
            command: bytes[0],
            count: bytes[1],
            reserved: [bytes[2], bytes[3], bytes[4]],
            triples,
        })
    }
}

/// 48-byte feedback event dequeued via `IOCTL_POLL_FEEDBACK`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct FeedbackEvent {
    pub header: RequestHeader,
    pub controller_id: u32,
    pub feedback_type: u16,
    pub payload_size: u16,
    pub payload: [u8; 32],
}

impl FeedbackEvent {
    pub const WIRE_SIZE: usize = 48;

    pub fn new(controller_id: u32, feedback_type: FeedbackType, payload: &[u8]) -> Self {
        let mut out = Self {
            header: RequestHeader::new(Self::WIRE_SIZE as u32),
            controller_id,
            feedback_type: feedback_type as u16,
            payload_size: payload.len() as u16,
            payload: [0; 32],
        };
        let copy_len = if payload.len() <= 32 {
            payload.len()
        } else {
            32
        };
        out.payload[..copy_len].copy_from_slice(&payload[..copy_len]);
        out
    }

    pub fn to_bytes(&self) -> [u8; Self::WIRE_SIZE] {
        let mut out = [0u8; Self::WIRE_SIZE];
        self.header.write_to(&mut out[0..8]);
        out[8..12].copy_from_slice(&self.controller_id.to_le_bytes());
        out[12..14].copy_from_slice(&self.feedback_type.to_le_bytes());
        out[14..16].copy_from_slice(&self.payload_size.to_le_bytes());
        out[16..48].copy_from_slice(&self.payload);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::WIRE_SIZE {
            return None;
        }
        let mut payload = [0u8; 32];
        payload.copy_from_slice(&bytes[16..48]);
        Some(Self {
            header: RequestHeader::from_bytes(&bytes[0..8])?,
            controller_id: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            feedback_type: u16::from_le_bytes([bytes[12], bytes[13]]),
            payload_size: u16::from_le_bytes([bytes[14], bytes[15]]),
            payload,
        })
    }
}
