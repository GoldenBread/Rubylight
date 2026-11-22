//! Controller button and capability wire constants shared between the streaming engine and
//! desktop configuration/UI.

/// Controller button flags (`*_FLAG`); the upper 16 bits are Sunshine extensions.
pub mod button {
    pub const UP: u32 = 0x0001;
    pub const DOWN: u32 = 0x0002;
    pub const LEFT: u32 = 0x0004;
    pub const RIGHT: u32 = 0x0008;
    pub const PLAY: u32 = 0x0010;
    pub const BACK: u32 = 0x0020;
    pub const LS_CLK: u32 = 0x0040;
    pub const RS_CLK: u32 = 0x0080;
    pub const LB: u32 = 0x0100;
    pub const RB: u32 = 0x0200;
    pub const SPECIAL: u32 = 0x0400;
    pub const A: u32 = 0x1000;
    pub const B: u32 = 0x2000;
    pub const X: u32 = 0x4000;
    pub const Y: u32 = 0x8000;
    pub const PADDLE1: u32 = 0x01_0000;
    pub const PADDLE2: u32 = 0x02_0000;
    pub const PADDLE3: u32 = 0x04_0000;
    pub const PADDLE4: u32 = 0x08_0000;
    pub const TOUCHPAD: u32 = 0x10_0000;
    pub const MISC: u32 = 0x20_0000;
    /// The Switch 2 Pro Controller's C button. Only a host with
    /// `ControllerIdentityVersion` knows it, so its caller drops its own C
    /// press from other hosts (the desktop does); the flag is also a Steam
    /// Controller's left grip touch, which those hosts still get.
    pub const C: u32 = rubylight_protocol::controller_identity::BUTTON_C;
}

/// Controller capabilities (`LI_CCAP_*`) of an arrival event.
pub mod capability {
    pub const ANALOG_TRIGGERS: u16 = 0x01;
    pub const RUMBLE: u16 = 0x02;
    pub const TRIGGER_RUMBLE: u16 = 0x04;
    pub const TOUCHPAD: u16 = 0x08;
    pub const ACCEL: u16 = 0x10;
    pub const GYRO: u16 = 0x20;
    pub const BATTERY_STATE: u16 = 0x40;
    pub const RGB_LED: u16 = 0x80;
    pub const DUAL_TOUCHPAD: u16 = 0x100;
    pub const HAPTICS_PCM: u16 = 0x8000;
}
