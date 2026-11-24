//! Pure Android/Nightjar controller policy extracted from `ControllerHandler`:
//! stick/trigger deadzone and normalization (`deadzone`), key/button remapping
//! and emulated combos (`mapping`), arrival/capability/Steam-Controls rules
//! (`arrival`), controller slot reservation & multi-context input fusion
//! (`slots`), combos/GuideHold/mouse/menu-capture state machines (`state_machine`),
//! and rumble/sensor/gyro/battery policies (`haptics_sensor`).

pub mod arrival;
pub mod combos;
pub mod deadzone;
pub mod haptics_sensor;
pub mod mapping;
pub mod maps;
pub mod slots;
pub mod state_machine;

/// Maximum number of controller slots tracked in the 16-bit active gamepad mask.
pub const MAX_GAMEPADS: usize = 16;

/// Wire controller types (`MoonBridge.LI_CTYPE_*`).
pub mod controller_type {
    pub const UNKNOWN: u8 = 0x00;
    pub const XBOX: u8 = 0x01;
    pub const PS: u8 = 0x02;
    pub const NINTENDO: u8 = 0x03;
    pub const STEAM: u8 = rubylight_protocol::steam_controller::LI_CTYPE_STEAM;
}
