// Copyright (c) 2026 Chase Payne
// SPDX-License-Identifier: MIT
//
// Ported from Nonary/libvirtualgamepad include/libvirtualgamepad/protocol.h.
// Adapted for Rubylight by RamazanKara (https://github.com/RamazanKara/Rubylight).

//! Fixed-size `METHOD_BUFFERED` IOCTL ABI shared between the Rubylight host and
//! the `rubylight_vgamepad` UMDF2 VHF source driver.

/// Wire protocol version. Version 2 widened the feedback event payload to
/// accommodate 23-byte DualSense adaptive trigger programs and SC2026 reports.
pub const PROTOCOL_VERSION: u16 = 2;

/// Maximum simultaneously owned controller slots on the virtual gamepad device.
pub const MAX_CONTROLLERS: u32 = 16;

/// Private control device interface published by `rubylight_vgamepad.dll`.
/// Distinct from Vibeshine's interface so side-by-side installs never collide.
pub const DEVICE_INTERFACE_GUID: u128 = 0xb8a3e710_4f2d_4c91_9a18_6e2f5b841a01;

/// Base GUID for each virtual HID child's ContainerID (`Data4[7]` = controller index).
pub const CONTROLLER_CONTAINER_BASE: u128 = 0x9a2f1c74_6b83_4d51_a70e_351dc6840000;

/// Root-enumerated PnP hardware ID installed by `rubylight_vgamepad.inf`.
pub const ROOT_HARDWARE_ID: &str = "Root\\RubylightVirtualGamepad";

/// Prefix for per-slot child InstanceIDs (`RubylightGamepad<n>`).
pub const INSTANCE_ID_PREFIX: &str = "RubylightGamepad";

/// Maximum Steam Settings `(id, lo, hi)` triples carried in one feedback event.
pub const STEAM_SETTINGS_TRIPLES_MAX: usize = 9;

/// Sentinel length returned by [`feedback_payload_size`] for unknown types.
pub const FEEDBACK_PAYLOAD_UNKNOWN: u16 = 0xFFFF;

/// Device-wide HID capability bits advertised in [`QueryInfoResponse`].
pub mod feature_mask {
    pub const INPUT_STATE: u32 = 1 << 0;
    pub const FEEDBACK: u32 = 1 << 1;
    pub const TOUCH: u32 = 1 << 2;
    pub const MOTION: u32 = 1 << 3;
    pub const BATTERY: u32 = 1 << 4;
    pub const HID_FEATURE_REPORTS: u32 = 1 << 5;
    pub const ALL: u32 = INPUT_STATE | FEEDBACK | TOUCH | MOTION | BATTERY | HID_FEATURE_REPORTS;
}

/// Normalized controller button bitmask shared across host and driver.
pub mod button_mask {
    pub const DPAD_UP: u32 = 0x0000_0001;
    pub const DPAD_DOWN: u32 = 0x0000_0002;
    pub const DPAD_LEFT: u32 = 0x0000_0004;
    pub const DPAD_RIGHT: u32 = 0x0000_0008;
    pub const START: u32 = 0x0000_0010;
    pub const BACK: u32 = 0x0000_0020;
    pub const LEFT_STICK: u32 = 0x0000_0040;
    pub const RIGHT_STICK: u32 = 0x0000_0080;
    pub const LEFT_SHOULDER: u32 = 0x0000_0100;
    pub const RIGHT_SHOULDER: u32 = 0x0000_0200;
    pub const HOME: u32 = 0x0000_0400;
    pub const SOUTH: u32 = 0x0000_1000;
    pub const EAST: u32 = 0x0000_2000;
    pub const WEST: u32 = 0x0000_4000;
    pub const NORTH: u32 = 0x0000_8000;
    pub const PADDLE_1: u32 = 0x0001_0000;
    pub const PADDLE_2: u32 = 0x0002_0000;
    pub const PADDLE_3: u32 = 0x0004_0000;
    pub const PADDLE_4: u32 = 0x0008_0000;
    pub const TOUCHPAD: u32 = 0x0010_0000;
    pub const MISC: u32 = 0x0020_0000;
    pub const LEFT_GRIP_TOUCH: u32 = 0x0040_0000;
    pub const RIGHT_GRIP_TOUCH: u32 = 0x0080_0000;
    pub const LEFT_STICK_TOUCH: u32 = 0x0100_0000;
    pub const RIGHT_STICK_TOUCH: u32 = 0x0200_0000;
    pub const LEFT_PAD_CLICK: u32 = 0x0400_0000;
    pub const RIGHT_PAD_CLICK: u32 = 0x0800_0000;
    pub const LEFT_TRIGGER_CLICK: u32 = 0x1000_0000;
    pub const RIGHT_TRIGGER_CLICK: u32 = 0x2000_0000;
    /// Host-owned metadata marking explicit SC2026 pad/trigger clicks and touch states.
    pub const SC26_BUTTONS_EXPLICIT: u32 = 0x4000_0000;
}

/// PlayStation output validity flags in [`PlayStationOutputFeedback::valid`].
pub mod ps_output_valid {
    pub const LIGHTBAR: u8 = 0x01;
    pub const TRIGGERS: u8 = 0x02;
    pub const PLAYER_LEDS: u8 = 0x04;
    pub const MICROPHONE_LED: u8 = 0x08;
}

/// Steam Controller attribute update bitmask (`ioctl_set_steam_controller_attributes`).
pub mod sc_attr_flags {
    pub const FIRMWARE_BUILD_TIME: u32 = 0x01;
    pub const BOOTLOADER_BUILD_TIME: u32 = 0x02;
    pub const BOARD_REVISION: u32 = 0x04;
    pub const CAPABILITIES: u32 = 0x08;
    pub const BUILD_ID: u32 = 0x40;
    pub const SERIALS: u32 = 0x80;
}

/// Requested virtual gamepad profile ID (`u16` wire discriminant).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum Profile {
    GenericHid = 1,
    Xbox360 = 2,
    XboxOne = 3,
    XboxSeries = 4,
    DualShock4 = 5,
    DualSense = 6,
    SwitchPro = 7,
    GenericPid = 8,
    SteamController = 9,
}

impl Profile {
    pub const fn from_raw(raw: u16) -> Option<Self> {
        match raw {
            1 => Some(Self::GenericHid),
            2 => Some(Self::Xbox360),
            3 => Some(Self::XboxOne),
            4 => Some(Self::XboxSeries),
            5 => Some(Self::DualShock4),
            6 => Some(Self::DualSense),
            7 => Some(Self::SwitchPro),
            8 => Some(Self::GenericPid),
            9 => Some(Self::SteamController),
            _ => None,
        }
    }

    pub const fn bit(self) -> u32 {
        profile_bit(self as u16)
    }
}

/// Converts a 1-based profile ID into its mask bit in `QueryInfoResponse::available_profiles`.
pub const fn profile_bit(profile_id: u16) -> u32 {
    if profile_id == 0 || profile_id > 32 {
        0
    } else {
        1u32 << (profile_id - 1)
    }
}

/// Discriminant for [`FeedbackEvent::feedback_type`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum FeedbackType {
    None = 0,
    GenericRumbleRgb = 1,
    RawOutputReport = 2,
    GenericRumble = 3,
    XboxRumble = 4,
    PlayStationOutput = 5,
    SteamHaptic = 6,
    SteamSettings = 7,
}

impl FeedbackType {
    pub const fn from_raw(raw: u16) -> Option<Self> {
        match raw {
            0 => Some(Self::None),
            1 => Some(Self::GenericRumbleRgb),
            2 => Some(Self::RawOutputReport),
            3 => Some(Self::GenericRumble),
            4 => Some(Self::XboxRumble),
            5 => Some(Self::PlayStationOutput),
            6 => Some(Self::SteamHaptic),
            7 => Some(Self::SteamSettings),
            _ => None,
        }
    }
}

/// Expected payload byte count for a [`FeedbackType`].
pub const fn feedback_payload_size(feedback_type: FeedbackType) -> u16 {
    match feedback_type {
        FeedbackType::None => 0,
        FeedbackType::GenericRumbleRgb | FeedbackType::GenericRumble => {
            GenericRumbleRgbFeedback::WIRE_SIZE as u16
        }
        FeedbackType::RawOutputReport => 16,
        FeedbackType::XboxRumble => XboxRumbleFeedback::WIRE_SIZE as u16,
        FeedbackType::PlayStationOutput => PlayStationOutputFeedback::WIRE_SIZE as u16,
        FeedbackType::SteamHaptic => SteamHapticFeedback::WIRE_SIZE as u16,
        FeedbackType::SteamSettings => SteamSettingsFeedback::WIRE_SIZE as u16,
    }
}

/// DualSense adaptive trigger effect mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TriggerEffectMode {
    Off = 0x00,
    Feedback = 0x01,
    Weapon = 0x02,
    Vibration = 0x06,
}

/// Normalized touch lifecycle event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TouchEvent {
    Hover = 0,
    Down = 1,
    Up = 2,
    Move = 3,
    Cancel = 4,
    CancelAll = 5,
}

impl TouchEvent {
    pub const fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            0 => Some(Self::Hover),
            1 => Some(Self::Down),
            2 => Some(Self::Up),
            3 => Some(Self::Move),
            4 => Some(Self::Cancel),
            5 => Some(Self::CancelAll),
            _ => None,
        }
    }
}

/// Motion sensor kind (`Touch/Motion` IOCTLs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MotionKind {
    Accelerometer = 1,
    Gyroscope = 2,
}

impl MotionKind {
    pub const fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            1 => Some(Self::Accelerometer),
            2 => Some(Self::Gyroscope),
            _ => None,
        }
    }
}

/// Normalized battery charging state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum BatteryState {
    Unknown = 0,
    NotPresent = 1,
    Discharging = 2,
    Charging = 3,
    Full = 4,
    NotCharging = 5,
}

impl BatteryState {
    pub const fn from_raw(raw: u8) -> Self {
        match raw {
            1 => Self::NotPresent,
            2 => Self::Discharging,
            3 => Self::Charging,
            4 => Self::Full,
            5 => Self::NotCharging,
            _ => Self::Unknown,
        }
    }
}

/// Saturates a `u32` counter to `u8` for wire diagnostics.
pub const fn saturate_counter(value: u32) -> u8 {
    if value > 0xFF { 0xFF } else { value as u8 }
}

const fn ctl_code(device_type: u32, function: u32, method: u32, access: u32) -> u32 {
    (device_type << 16) | (access << 14) | (function << 2) | method
}

const FILE_DEVICE_UNKNOWN: u32 = 0x0022;
const METHOD_BUFFERED: u32 = 0;
const FILE_READ_WRITE_ACCESS: u32 = 0x0003;

pub const IOCTL_QUERY_INFO: u32 = ctl_code(
    FILE_DEVICE_UNKNOWN,
    0x800,
    METHOD_BUFFERED,
    FILE_READ_WRITE_ACCESS,
);
pub const IOCTL_CREATE_CONTROLLER: u32 = ctl_code(
    FILE_DEVICE_UNKNOWN,
    0x801,
    METHOD_BUFFERED,
    FILE_READ_WRITE_ACCESS,
);
pub const IOCTL_DESTROY_CONTROLLER: u32 = ctl_code(
    FILE_DEVICE_UNKNOWN,
    0x802,
    METHOD_BUFFERED,
    FILE_READ_WRITE_ACCESS,
);
pub const IOCTL_SUBMIT_INPUT_STATE: u32 = ctl_code(
    FILE_DEVICE_UNKNOWN,
    0x803,
    METHOD_BUFFERED,
    FILE_READ_WRITE_ACCESS,
);
pub const IOCTL_POLL_FEEDBACK: u32 = ctl_code(
    FILE_DEVICE_UNKNOWN,
    0x804,
    METHOD_BUFFERED,
    FILE_READ_WRITE_ACCESS,
);
pub const IOCTL_SUBMIT_TOUCH_STATE: u32 = ctl_code(
    FILE_DEVICE_UNKNOWN,
    0x805,
    METHOD_BUFFERED,
    FILE_READ_WRITE_ACCESS,
);
pub const IOCTL_SUBMIT_MOTION_STATE: u32 = ctl_code(
    FILE_DEVICE_UNKNOWN,
    0x806,
    METHOD_BUFFERED,
    FILE_READ_WRITE_ACCESS,
);
pub const IOCTL_SUBMIT_BATTERY_STATE: u32 = ctl_code(
    FILE_DEVICE_UNKNOWN,
    0x807,
    METHOD_BUFFERED,
    FILE_READ_WRITE_ACCESS,
);
pub const IOCTL_SET_STEAM_CONTROLLER_ATTRIBUTES: u32 = ctl_code(
    FILE_DEVICE_UNKNOWN,
    0x808,
    METHOD_BUFFERED,
    FILE_READ_WRITE_ACCESS,
);

mod feedback;
mod requests;

pub use feedback::{
    FeedbackEvent, GenericRumbleRgbFeedback, PlayStationOutputFeedback, SteamHapticFeedback,
    SteamSettingsFeedback, TriggerEffectFeedback, XboxRumbleFeedback,
};
pub use requests::{
    BatteryStateRequest, ControllerIdRequest, CreateControllerRequest, FeedbackCounters,
    InputStateRequest, MotionStateRequest, QueryInfoRequest, QueryInfoResponse, RequestHeader,
    SteamControllerAttributesRequest, TouchStateRequest,
};

/// Formats `RubylightGamepad<index>` as a null-terminated UTF-16 buffer for
/// `VHF_CONFIG.InstanceID` and returns the slice including its trailing NUL.
pub fn format_instance_id_utf16(index: u32, out: &mut [u16; 32]) -> usize {
    *out = [0u16; 32];
    let prefix = INSTANCE_ID_PREFIX.as_bytes();
    let mut pos = 0usize;
    for &b in prefix {
        out[pos] = u16::from(b);
        pos += 1;
    }
    if index >= 10 {
        out[pos] = u16::from(b'0' + ((index / 10) % 10) as u8);
        pos += 1;
    }
    out[pos] = u16::from(b'0' + (index % 10) as u8);
    pos += 1;
    // `pos + 1` includes the trailing UTF-16 NUL already in `out[pos]`.
    pos + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ioctl_codes_match_wire_contract() {
        assert_eq!(IOCTL_QUERY_INFO, 0x0022_E000);
        assert_eq!(IOCTL_CREATE_CONTROLLER, 0x0022_E004);
        assert_eq!(IOCTL_DESTROY_CONTROLLER, 0x0022_E008);
        assert_eq!(IOCTL_SUBMIT_INPUT_STATE, 0x0022_E00C);
        assert_eq!(IOCTL_POLL_FEEDBACK, 0x0022_E010);
        assert_eq!(IOCTL_SUBMIT_TOUCH_STATE, 0x0022_E014);
        assert_eq!(IOCTL_SUBMIT_MOTION_STATE, 0x0022_E018);
        assert_eq!(IOCTL_SUBMIT_BATTERY_STATE, 0x0022_E01C);
        assert_eq!(IOCTL_SET_STEAM_CONTROLLER_ATTRIBUTES, 0x0022_E020);
    }

    #[test]
    fn wire_sizes_and_counter_saturation() {
        assert_eq!(QueryInfoResponse::default().to_bytes().len(), 28);
        assert_eq!(FeedbackEvent::default().to_bytes().len(), 48);
        assert_eq!(
            SteamControllerAttributesRequest::default().to_bytes().len(),
            80
        );
        assert_eq!(feedback_payload_size(FeedbackType::PlayStationOutput), 32);
        assert_eq!(feedback_payload_size(FeedbackType::SteamSettings), 32);
        assert_eq!(feedback_payload_size(FeedbackType::SteamHaptic), 16);
        assert_eq!(saturate_counter(0), 0);
        assert_eq!(saturate_counter(255), 255);
        assert_eq!(saturate_counter(256), 255);
        assert_eq!(saturate_counter(u32::MAX), 255);
    }
}
