//! Pure Android/Nightjar controller policy extracted from `ControllerHandler`:
//! stick/trigger deadzone and normalization (`deadzone`), key/button remapping
//! and emulated combos (`mapping`), arrival/capability/Steam-Controls rules
//! (`arrival`), and controller slot reservation & multi-context input fusion
//! (`slots`).

pub mod arrival;
pub mod combos;
pub mod deadzone;
pub mod mapping;
pub mod maps;
pub mod slots;

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
