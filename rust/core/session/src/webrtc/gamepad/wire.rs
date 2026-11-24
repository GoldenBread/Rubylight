//! WebRTC gamepad wire protocol types, message parsing, and Moonlight button/axis mapping.
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use super::super::input::{MessageError, check_envelope};
use butterpollo_input_policy::input::Input;

/// `v` values accepted on gamepad messages. The TV sends 2 and also accepts 1.
const ACCEPTED_VERSIONS: [i64; 2] = [1, 2];
/// `v` of the control messages the host sends: the reference's
/// `GAMEPAD_PROTOCOL_VERSION`. The TV app accepts 1 and 2.
pub const CONTROL_VERSION: u32 = 2;
/// Controller slots in Moonlight's active gamepad mask.
pub const MAX_GAMEPADS: usize = 16;
/// The client re-sends an unchanged state at least this often. Documentation
/// only: the host keeps no timer on it.
pub const KEEPALIVE: Duration = Duration::from_millis(250);
/// Hold Start this long, then release it, to toggle mouse mode (the
/// reference's `MOUSE_EMULATION_LONG_PRESS_TIME`).
pub const MOUSE_MODE_HOLD: Duration = Duration::from_millis(750);
/// Pointer motion period in mouse mode (`MOUSE_EMULATION_POLLING_INTERVAL`).
pub const MOUSE_TICK: Duration = Duration::from_millis(50);
/// Stick-to-pointer speed factor (`MOUSE_EMULATION_MOTION_MULTIPLIER`).
const MOUSE_MOTION_MULTIPLIER: f64 = 4.0;
/// Pointer deadzone in pixels per tick (`MOUSE_EMULATION_DEADZONE`).
const MOUSE_DEADZONE: f64 = 2.0;
/// One wheel notch in Moonlight scroll units.
pub(super) const WHEEL_NOTCH: i16 = 120;
/// Moonlight's `LI_CTYPE_XBOX`. Every WebRTC gamepad is announced as one.
pub(super) const CTYPE_XBOX: u8 = 1;
/// Moonlight's `LI_CCAP_ANALOG_TRIGGERS`, `LI_CCAP_RUMBLE` and
/// `LI_CCAP_TRIGGER_RUMBLE`.
pub(super) const CCAP_ANALOG_TRIGGERS: u16 = 0x01;
pub(super) const CCAP_RUMBLE: u16 = 0x02;
pub(super) const CCAP_TRIGGER_RUMBLE: u16 = 0x04;

/// Bits of the `buttons` mask in `gamepad-state`: the Gamepad API's standard
/// buttons in the TV app's order.
pub mod button {
    pub const A: u32 = 1 << 0;
    pub const B: u32 = 1 << 1;
    pub const X: u32 = 1 << 2;
    pub const Y: u32 = 1 << 3;
    pub const LEFT_BUMPER: u32 = 1 << 4;
    pub const RIGHT_BUMPER: u32 = 1 << 5;
    pub const BACK: u32 = 1 << 6;
    pub const START: u32 = 1 << 7;
    pub const LEFT_STICK: u32 = 1 << 8;
    pub const RIGHT_STICK: u32 = 1 << 9;
    pub const DPAD_UP: u32 = 1 << 10;
    pub const DPAD_DOWN: u32 = 1 << 11;
    pub const DPAD_LEFT: u32 = 1 << 12;
    pub const DPAD_RIGHT: u32 = 1 << 13;
    pub const GUIDE: u32 = 1 << 14;
    /// Every bit a client may set.
    pub const ALL: u32 = (1 << 15) - 1;
}

/// Moonlight's controller button flags (the low and high words of
/// `Input::Controller::buttons`).
mod moonlight {
    pub const UP: u32 = 0x0001;
    pub const DOWN: u32 = 0x0002;
    pub const LEFT: u32 = 0x0004;
    pub const RIGHT: u32 = 0x0008;
    pub const PLAY: u32 = 0x0010;
    pub const BACK: u32 = 0x0020;
    pub const LEFT_STICK: u32 = 0x0040;
    pub const RIGHT_STICK: u32 = 0x0080;
    pub const LEFT_BUMPER: u32 = 0x0100;
    pub const RIGHT_BUMPER: u32 = 0x0200;
    pub const SPECIAL: u32 = 0x0400;
    pub const A: u32 = 0x1000;
    pub const B: u32 = 0x2000;
    pub const X: u32 = 0x4000;
    pub const Y: u32 = 0x8000;
}

/// Protocol button bit and Moonlight flag, for every button.
pub(super) const BUTTON_MAP: [(u32, u32); 15] = [
    (button::A, moonlight::A),
    (button::B, moonlight::B),
    (button::X, moonlight::X),
    (button::Y, moonlight::Y),
    (button::LEFT_BUMPER, moonlight::LEFT_BUMPER),
    (button::RIGHT_BUMPER, moonlight::RIGHT_BUMPER),
    (button::BACK, moonlight::BACK),
    (button::START, moonlight::PLAY),
    (button::LEFT_STICK, moonlight::LEFT_STICK),
    (button::RIGHT_STICK, moonlight::RIGHT_STICK),
    (button::DPAD_UP, moonlight::UP),
    (button::DPAD_DOWN, moonlight::DOWN),
    (button::DPAD_LEFT, moonlight::LEFT),
    (button::DPAD_RIGHT, moonlight::RIGHT),
    (button::GUIDE, moonlight::SPECIAL),
];

/// The buttons that act as mouse buttons in mouse mode, with the Moonlight
/// mouse button each stands for: left, right, middle, X1 (back), X2 (forward).
pub(super) const MOUSE_BUTTONS: [(u32, u8); 5] = [
    (button::A, 1),
    (button::B, 3),
    (button::X, 2),
    (button::LEFT_BUMPER, 4),
    (button::RIGHT_BUMPER, 5),
];
pub(super) const MOUSE_BUTTON_MASK: u32 =
    button::A | button::B | button::X | button::LEFT_BUMPER | button::RIGHT_BUMPER;

/// A `gamepad-connected` message.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connected {
    pub controller_id: u32,
    pub id: String,
    pub mapping: String,
    /// Button count of the Gamepad API object.
    pub buttons: u64,
    #[serde(default)]
    pub axes: u64,
    #[serde(default)]
    pub dual_rumble: bool,
    #[serde(default)]
    pub trigger_rumble: bool,
}

/// A `gamepad-state` snapshot in the client's units.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub controller_id: u32,
    pub seq: u64,
    pub buttons: u64,
    pub left_trigger: f64,
    pub right_trigger: f64,
    pub left_stick_x: f64,
    pub left_stick_y: f64,
    pub right_stick_x: f64,
    pub right_stick_y: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GamepadMessage {
    Connected(Connected),
    Disconnected { controller_id: u32 },
    State(State),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Disconnect {
    controller_id: u32,
}

const TYPES: [&str; 3] = ["gamepad-connected", "gamepad-disconnected", "gamepad-state"];

/// Parse a message from the `gamepad` or control DataChannel.
pub fn parse(text: &str) -> Result<GamepadMessage, MessageError> {
    let value: Value =
        serde_json::from_str(text).map_err(|e| MessageError::Malformed(e.to_string()))?;
    check_envelope(&value, &TYPES, &ACCEPTED_VERSIONS)?;
    let malformed = |e: serde_json::Error| MessageError::Malformed(e.to_string());
    let kind = value["type"].as_str().unwrap_or_default().to_owned();
    match kind.as_str() {
        "gamepad-connected" => {
            let connected: Connected = serde_json::from_value(value).map_err(malformed)?;
            if connected.id.len() > 512 || connected.mapping.len() > 128 {
                return Err(MessageError::Invalid("controller id or mapping too long"));
            }
            if connected.buttons > 64 || connected.axes > 16 {
                return Err(MessageError::Invalid("too many buttons or axes"));
            }
            Ok(GamepadMessage::Connected(connected))
        }
        "gamepad-disconnected" => {
            let Disconnect { controller_id } = serde_json::from_value(value).map_err(malformed)?;
            Ok(GamepadMessage::Disconnected { controller_id })
        }
        _ => {
            let state: State = serde_json::from_value(value).map_err(malformed)?;
            if state.buttons > u64::from(button::ALL) {
                return Err(MessageError::Invalid("unknown button bits"));
            }
            Ok(GamepadMessage::State(state))
        }
    }
}

/// Moonlight button flags for a protocol button mask.
pub fn map_buttons(protocol: u32) -> u32 {
    BUTTON_MAP
        .iter()
        .filter(|(from, _)| protocol & from != 0)
        .fold(0, |flags, (_, to)| flags | to)
}

/// A trigger, 0..1, as 0..255.
pub fn map_trigger(value: f64) -> u8 {
    (value.clamp(0., 1.) * 255.).round() as u8
}

/// A stick axis, -1..1, as an `i16`. The Gamepad API's Y is down positive and
/// Moonlight's is up positive, so Y is inverted.
pub fn map_stick(value: f64, invert: bool) -> i16 {
    let value = value.clamp(-1., 1.);
    ((if invert { -value } else { value }) * 32767.).round() as i16
}

/// The Moonlight buttons a controller with `count` Gamepad API buttons has,
/// for the arrival packet.
pub fn supported_buttons(count: u64) -> u32 {
    // Gamepad API index and Moonlight flag. Indices 6 and 7 are the triggers.
    const BY_INDEX: [(u64, u32); 15] = [
        (0, moonlight::A),
        (1, moonlight::B),
        (2, moonlight::X),
        (3, moonlight::Y),
        (4, moonlight::LEFT_BUMPER),
        (5, moonlight::RIGHT_BUMPER),
        (8, moonlight::BACK),
        (9, moonlight::PLAY),
        (10, moonlight::LEFT_STICK),
        (11, moonlight::RIGHT_STICK),
        (12, moonlight::UP),
        (13, moonlight::DOWN),
        (14, moonlight::LEFT),
        (15, moonlight::RIGHT),
        (16, moonlight::SPECIAL),
    ];
    BY_INDEX
        .iter()
        .filter(|(index, _)| count > *index)
        .fold(0, |flags, (_, flag)| flags | flag)
}

/// Pointer motion for one mouse-mode tick from Moonlight-mapped sticks
/// `[left x, left y, right x, right y]` (Y up positive): whichever stick is
/// pushed further drives it, on a cubic curve with a deadzone.
///
/// The reference subtracts its deadzone from negative values too, so left and
/// up move faster than right and down. This version is symmetric.
pub fn mouse_delta(sticks: [i16; 4]) -> (i16, i16) {
    let [lx, ly, rx, ry] = sticks.map(i32::from);
    // Screen Y is down positive.
    let (ly, ry) = (-ly, -ry);
    let (x, y) = if lx.abs() + ly.abs() > rx.abs() + ry.abs() {
        (lx, ly)
    } else {
        (rx, ry)
    };
    let curve = |raw: i32| {
        let delta = (f64::from(raw) / 32766. * MOUSE_MOTION_MULTIPLIER).powi(3);
        let delta = if delta.abs() > MOUSE_DEADZONE {
            delta - MOUSE_DEADZONE.copysign(delta)
        } else {
            0.
        };
        delta
            .round()
            .clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16
    };
    (curve(x), curve(y))
}

/// A rumble motor speed, 0..65535, as 0..1.
pub fn rumble_magnitude(value: u16) -> f64 {
    f64::from(value) / 65535.
}

/// A message for the client's control DataChannel.
#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    MouseMode {
        controller_id: u32,
        slot: u8,
        active: bool,
    },
    /// All four motors, 0..1. Strong is the host's low-frequency motor.
    Rumble {
        controller_id: u32,
        slot: u8,
        strong: f64,
        weak: f64,
        left_trigger: f64,
        right_trigger: f64,
    },
}

impl Control {
    pub fn to_json(&self) -> String {
        match self {
            Self::MouseMode {
                controller_id,
                slot,
                active,
            } => json!({
                "v": CONTROL_VERSION,
                "type": "mouse-mode",
                "controllerId": controller_id,
                "controllerSlot": slot,
                "active": active,
            }),
            Self::Rumble {
                controller_id,
                slot,
                strong,
                weak,
                left_trigger,
                right_trigger,
            } => json!({
                "v": CONTROL_VERSION,
                "type": "rumble",
                "controllerId": controller_id,
                "controllerSlot": slot,
                "strongMagnitude": strong,
                "weakMagnitude": weak,
                "leftTrigger": left_trigger,
                "rightTrigger": right_trigger,
            }),
        }
        .to_string()
    }
}

/// What a gamepad event asks the host to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Inject into the input dispatcher, as if from the ENet control path.
    Input(Input),
    /// Send on the client's control DataChannel.
    Control(Control),
}

/// What handling one message did.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Outcome {
    /// The message was valid and current. Stale snapshots, unknown
    /// controllers and a full slot table are not.
    pub accepted: bool,
    pub actions: Vec<Action>,
}
