//! Pure controller mappings, constants, dead-zone math and quit-combo masks shared between
//! saved configuration (`config`), UI settings and the SDL3 gamepad driver.

use crate::config::GamepadQuitCombo;
use crate::strings::en;
use nightjar_policy::controllers::Controllers;
use nightjar_policy::gamepad_constants::{button, capability};
use serde::{Deserialize, Serialize};

/// `LI_CTYPE_*`.
pub const CTYPE_UNKNOWN: u8 = 0;
pub const CTYPE_XBOX: u8 = 1;
pub const CTYPE_PS: u8 = 2;
pub const CTYPE_NINTENDO: u8 = 3;

/// `LI_BATTERY_STATE_*`.
pub const BATTERY_UNKNOWN: u8 = 0x00;
pub const BATTERY_NOT_PRESENT: u8 = 0x01;
pub const BATTERY_DISCHARGING: u8 = 0x02;
pub const BATTERY_CHARGING: u8 = 0x03;
pub const BATTERY_FULL: u8 = 0x05;
pub const BATTERY_PERCENT_UNKNOWN: u8 = 0xFF;

pub const MAX_PADS: usize = 16;

/// SDL3 `SDL_GamepadButton` indices (0..=10) used in `1 << index` bitmasks without importing
/// `sdl3_sys` into `nightjar-desktop-config`. Verified against `SDL_GAMEPAD_BUTTON_*` in
/// `nightjar-desktop-media::gamepad::tests`.
pub mod sdl_button_index {
    pub const SOUTH: u8 = 0;
    pub const EAST: u8 = 1;
    pub const WEST: u8 = 2;
    pub const NORTH: u8 = 3;
    pub const BACK: u8 = 4;
    pub const GUIDE: u8 = 5;
    pub const START: u8 = 6;
    pub const LEFT_SHOULDER: u8 = 9;
    pub const RIGHT_SHOULDER: u8 = 10;
}

/// SDL gamepad buttons in `SDL_GamepadButton` order, as Moonlight button flags.
pub const BUTTON_MAP: [u32; 21] = [
    button::A,
    button::B,
    button::X,
    button::Y,
    button::BACK,
    button::SPECIAL,
    button::PLAY,
    button::LS_CLK,
    button::RS_CLK,
    button::LB,
    button::RB,
    button::UP,
    button::DOWN,
    button::LEFT,
    button::RIGHT,
    button::MISC,
    button::PADDLE1,
    button::PADDLE2,
    button::PADDLE3,
    button::PADDLE4,
    button::TOUCHPAD,
];

pub const BUTTON_NAMES: [&str; 21] = [
    en("A / South"), en("B / East"), en("X / West"), en("Y / North"), en("Back / Select"), en("Guide"), en("Start"),
    en("Left stick click"), en("Right stick click"), en("Left shoulder"), en("Right shoulder"),
    en("D-pad up"), en("D-pad down"), en("D-pad left"), en("D-pad right"), en("Share / Misc"),
    en("Paddle 1"), en("Paddle 2"), en("Paddle 3"), en("Paddle 4"), en("Touchpad click"),
];
pub const AXIS_NAMES: [&str; 6] = [en("Left stick X"), en("Left stick Y"), en("Right stick X"), en("Right stick Y"), en("Left trigger"), en("Right trigger")];

/// The "Connect as" choices: Automatic (zero) keeps the type SDL detects, the others are the
/// `LI_CTYPE_*` the host is told instead.
pub const CONNECT_AS: [(u8, &str); 4] = [
    (CTYPE_UNKNOWN, en("Automatic")), (CTYPE_XBOX, en("Xbox")), (CTYPE_PS, en("PlayStation")), (CTYPE_NINTENDO, en("Nintendo")),
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mapping {
    pub name: String,
    pub buttons: [Option<u32>; 21],
    /// Zero disables an axis; otherwise the signed source index plus one.
    pub axes: [i8; 6],
    pub deadzone: u8,
    /// Percent of trigger travel ignored; 0 (the default) sends triggers raw.
    pub trigger_deadzone: u8,
    /// One of [`CONNECT_AS`]; mappings saved before it existed load as Automatic.
    #[serde(deserialize_with = "lenient_connect_as")]
    pub connect_as: u8,
    /// The controller gyroscope axis behind each axis games read (X up and down, Y left and right,
    /// Z tilt): the source index plus one, negative to invert, each source once. Android's
    /// Gyroscope mapping profiles, after jvanderv42's Nightjar fork (GPL-3.0).
    pub gyro_axes: [i8; 3],
    /// Left and right gyroscope speed in percent, [`MIN_GYRO_SPEED`] to [`MAX_GYRO_SPEED`].
    pub gyro_yaw: u16,
    /// Up and down gyroscope speed in percent, likewise.
    pub gyro_pitch: u16,
}

pub const MIN_GYRO_SPEED: u16 = 10;
pub const MAX_GYRO_SPEED: u16 = 300;
/// The gyroscope axes games read, in the order of [`Mapping::gyro_axes`].
pub const GYRO_AXIS_NAMES: [&str; 3] = [en("Up and down (X)"), en("Left and right (Y)"), en("Tilt (Z)")];

/// Anything but a known choice reads as Automatic, so one bad value (256, -1, "x", or a type a
/// newer Nightjar adds) never fails the whole file and with it every controller's mapping.
fn lenient_connect_as<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u8, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value
        .as_u64()
        .and_then(|v| u8::try_from(v).ok())
        .filter(|v| CONNECT_AS.iter().any(|(kind, _)| kind == v))
        .unwrap_or(CTYPE_UNKNOWN))
}

impl Default for Mapping {
    fn default() -> Self {
        Self {
            name: String::new(),
            buttons: [None; 21],
            axes: [1, 2, 3, 4, 5, 6],
            deadzone: 0,
            trigger_deadzone: 0,
            connect_as: CTYPE_UNKNOWN,
            gyro_axes: [1, 2, 3],
            gyro_yaw: 100,
            gyro_pitch: 100,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub buttons: u32,
    pub lt: u8,
    pub rt: u8,
    pub ls: (i16, i16),
    pub rs: (i16, i16),
}

/// Stick and trigger dead zones, in percent of their travel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeadZones {
    pub stick: u8,
    pub trigger: u8,
}

/// What every controller's state goes through on its way to the PC besides its own mapping.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PadSettings {
    /// Nintendo layout: A/B and X/Y swapped.
    pub swap: bool,
    /// "Back button as Guide": the physical Back (Select, View) reaches the PC as Guide.
    pub back_as_guide: bool,
    /// "Stick dead zone" and "Trigger dead zone", for a mapping that sets none of its own.
    pub dead_zones: DeadZones,
}

impl From<&crate::config::Settings> for PadSettings {
    fn from(settings: &crate::config::Settings) -> Self {
        Self {
            swap: settings.swap_face_buttons,
            // "Back opens the stream menu" keeps Back for Nightjar, which never sends it, as on Android.
            back_as_guide: settings.back_as_guide && !settings.back_opens_menu,
            dead_zones: DeadZones {
                stick: settings.deadzone.min(u32::from(MAX_STICK_DEADZONE)) as u8,
                trigger: settings.trigger_deadzone.min(u32::from(MAX_TRIGGER_DEADZONE)) as u8,
            },
        }
    }
}

/// The face button swap alone: a mapping as it is, without the settings' dead zones or Back as Guide.
impl From<bool> for PadSettings {
    fn from(swap: bool) -> Self {
        Self { swap, ..Self::default() }
    }
}

/// "Controllers" One player ([`Controllers`]): every controller is player 1, and the PC keeps
/// player 1 while none is connected (Android's `persistGamepadsAfterDisconnect`).
pub fn one_player(settings: &crate::config::Settings) -> bool {
    Controllers::from_saved(settings.controllers, settings.multi_controller) == Controllers::OnePlayer
}

/// The gamepad number a controller in host slot `index` reaches the PC as: its own, or 0 (player 1)
/// for every controller with One player.
pub fn sent_index(one_player: bool, index: u8) -> u8 {
    let players = if one_player { Controllers::OnePlayer } else { Controllers::PlayerEach };
    players.sent_index(index).unwrap_or(index)
}

/// "Back button as Guide": the physical Back as Guide, before the mapping, which then sends what it
/// sends for Guide (Android remaps Back before its button map too).
pub fn back_as_guide(raw: u32, on: bool) -> u32 {
    const BACK: u32 = 1 << sdl_button_index::BACK;
    const GUIDE: u32 = 1 << sdl_button_index::GUIDE;
    if on && raw & BACK != 0 {
        (raw & !BACK) | GUIDE
    } else {
        raw
    }
}

impl Mapping {
    /// The gyroscope reading games get, in SDL's axes (X pitch, Y yaw, Z roll) and units.
    pub fn map_gyro(&self, raw: [f32; 3]) -> [f32; 3] {
        let pick = |source: i8| {
            let value = raw[usize::from(source.unsigned_abs().clamp(1, 3)) - 1];
            if source < 0 { -value } else { value }
        };
        [
            pick(self.gyro_axes[0]) * f32::from(self.gyro_pitch) / 100.0,
            pick(self.gyro_axes[1]) * f32::from(self.gyro_yaw) / 100.0,
            pick(self.gyro_axes[2]),
        ]
    }

    /// Gives `output` the source `source` (signed, as in [`Self::gyro_axes`]); the axis that had
    /// it takes `output`'s old source, so each source stays in use once.
    pub fn set_gyro_axis(&mut self, output: usize, source: i8) {
        let old = self.gyro_axes[output];
        if let Some(other) = (0..3).find(|&i| i != output && self.gyro_axes[i].unsigned_abs() == source.unsigned_abs()) {
            self.gyro_axes[other] = old.unsigned_abs() as i8 * self.gyro_axes[other].signum();
        }
        self.gyro_axes[output] = source;
    }

    pub fn valid(&self) -> bool {
        self.deadzone <= MAX_STICK_DEADZONE
            && self.trigger_deadzone <= MAX_TRIGGER_DEADZONE
            && self.buttons.iter().flatten().all(|b| *b == 0 || BUTTON_MAP.contains(b))
            && self.axes.iter().enumerate().all(|(i, source)| {
                *source == 0 || if i < 4 { (1..=4).contains(&source.unsigned_abs()) } else { (5..=6).contains(source) }
            })
            && self.gyro_axes.iter().all(|s| (1..=3).contains(&s.unsigned_abs()))
            && (1..=3).all(|axis| self.gyro_axes.iter().any(|s| s.unsigned_abs() == axis))
            && (MIN_GYRO_SPEED..=MAX_GYRO_SPEED).contains(&self.gyro_yaw)
            && (MIN_GYRO_SPEED..=MAX_GYRO_SPEED).contains(&self.gyro_pitch)
    }

    /// The type the pad arrives as: the pinned one, else `detected`. A value this version does
    /// not know counts as Automatic, so a newer Nightjar's choice never drops the mapping.
    pub fn arrival_type(&self, detected: u8) -> u8 {
        if self.connect_as != CTYPE_UNKNOWN && CONNECT_AS.iter().any(|(kind, _)| *kind == self.connect_as) {
            self.connect_as
        } else {
            detected
        }
    }

    /// Whether the pad is pinned as Xbox, which leaves out motion and the touchpad: an Xbox pad has
    /// neither, and hosts such as Rubylight turn any pad that reports them into a DualSense.
    pub fn drops_motion_and_touchpad(&self) -> bool {
        self.connect_as == CTYPE_XBOX
    }

    /// The capabilities the pad arrives with.
    pub fn arrival_capabilities(&self, caps: u16) -> u16 {
        if !self.drops_motion_and_touchpad() {
            return caps;
        }
        caps & !(capability::ACCEL | capability::GYRO | capability::TOUCHPAD | capability::DUAL_TOUCHPAD)
    }

    /// Whether Select+LB clicks the touchpad: pinned as PlayStation on a pad without a touchpad
    /// click, unless a button is already mapped to it.
    pub fn emulates_clickpad(&self, has_touchpad_click: bool) -> bool {
        self.connect_as == CTYPE_PS && !has_touchpad_click && !self.buttons.contains(&Some(button::TOUCHPAD))
    }

    pub fn map_buttons(&self, raw: u32, swap: bool) -> u32 {
        self.buttons.iter().enumerate().filter(|(i, _)| raw & (1 << i) != 0).fold(0, |buttons, (i, target)| {
            buttons | target.unwrap_or(BUTTON_MAP[if swap && i < 4 { i ^ 1 } else { i }])
        })
    }

    /// The dead zones this mapping streams with: its own, and `defaults` (the settings') where it
    /// sets none (0).
    pub fn dead_zones(&self, defaults: DeadZones) -> DeadZones {
        DeadZones {
            stick: if self.deadzone != 0 { self.deadzone } else { defaults.stick },
            trigger: if self.trigger_deadzone != 0 { self.trigger_deadzone } else { defaults.trigger },
        }
    }

    /// The physical `buttons` (bits of `1 << SDL_GamepadButton`) and `axes` as the PC gets them.
    pub fn apply(&self, buttons: u32, axes: [i16; 6], settings: PadSettings) -> Report {
        let mut mapped = [0; 6];
        for (value, source) in mapped.iter_mut().zip(self.axes) {
            if source != 0 {
                *value = axes[source.unsigned_abs() as usize - 1];
                if source < 0 {
                    *value = invert(*value);
                }
            }
        }
        let zones = self.dead_zones(settings.dead_zones);
        let trigger = |v: i16| trigger(v, zones.trigger);
        Report {
            buttons: self.map_buttons(back_as_guide(buttons, settings.back_as_guide), settings.swap),
            lt: trigger(mapped[4]),
            rt: trigger(mapped[5]),
            ls: deadzone((mapped[0], invert(mapped[1])), zones.stick),
            rs: deadzone((mapped[2], invert(mapped[3])), zones.stick),
        }
    }
}

/// Select+LB pressed together, and nothing else, sends the touchpad click instead while both stay
/// held; `swallowed` holds those of the two not yet let go, which stay silent until released.
/// The Android client does the same for a pad pinned as PlayStation.
pub fn clickpad(buttons: u32, swallowed: &mut u32) -> u32 {
    let combo = button::BACK | button::LB;
    if *swallowed == 0 && buttons == combo {
        *swallowed = combo;
    }
    *swallowed &= buttons;
    (buttons & !*swallowed) | if *swallowed == combo { button::TOUCHPAD } else { 0 }
}

pub const MAX_TRIGGER_DEADZONE: u8 = 30;
/// The largest stick dead zone in percent, a mapping's or the settings'.
pub const MAX_STICK_DEADZONE: u8 = 50;

/// A trigger as 0-255: travel inside the dead zone reads as released, the rest scales smoothly to
/// full, like the stick dead zone.
pub fn trigger(value: i16, percent: u8) -> u8 {
    let value = i32::from(value.max(0));
    let edge = i32::from(percent) * 32767 / 100;
    if value <= edge {
        return 0;
    }
    ((value - edge) * 255 / (32767 - edge)) as u8
}

pub fn deadzone(value: (i16, i16), percent: u8) -> (i16, i16) {
    if percent == 0 {
        return value;
    }
    let (x, y) = (f32::from(value.0), f32::from(value.1));
    let length = x.hypot(y);
    let edge = f32::from(percent) * 32767.0 / 100.0;
    if length <= edge {
        return (0, 0);
    }
    let scale = ((length - edge) / (32767.0 - edge)).min(1.0) * 32767.0 / length;
    ((x * scale).round() as i16, (y * scale).round() as i16)
}

/// The physical buttons of a quit combo, as bits of `1 << SDL_GamepadButton`. Face buttons are
/// positions (A is south) like the rest of the local chords, so the Nintendo layout swap leaves
/// them where they are.
pub fn quit_mask(combo: GamepadQuitCombo) -> u32 {
    use GamepadQuitCombo as Combo;
    let bits = |buttons: &[u8]| buttons.iter().fold(0u32, |mask, b| mask | 1 << b);
    let shoulders = [sdl_button_index::LEFT_SHOULDER, sdl_button_index::RIGHT_SHOULDER];
    bits(&shoulders)
        | match combo {
            Combo::StartSelectLbRb => bits(&[sdl_button_index::START, sdl_button_index::BACK]),
            Combo::SelectLbRbY => bits(&[sdl_button_index::BACK, sdl_button_index::NORTH]),
            Combo::StartLbRbA => bits(&[sdl_button_index::START, sdl_button_index::SOUTH]),
            Combo::StartLbRbB => bits(&[sdl_button_index::START, sdl_button_index::EAST]),
            Combo::LbRbXY => bits(&[sdl_button_index::WEST, sdl_button_index::NORTH]),
            Combo::LbRbAB => bits(&[sdl_button_index::SOUTH, sdl_button_index::EAST]),
        }
}

pub fn invert(value: i16) -> i16 {
    -(value.max(-32767))
}

/// Which button flags go to the host: a host without `ControllerIdentityVersion` sees a Switch 2
/// controller's C button as a Steam Controller's left grip touch (`button::C`), so drop it there.
pub fn host_buttons(buttons: u32, controller_identity: bool) -> u32 {
    if controller_identity {
        buttons
    } else {
        buttons & !button::C
    }
}
