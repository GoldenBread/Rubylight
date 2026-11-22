//! Decoding Moonlight's input packets (keyboard, mouse, pen, touch, gamepad
//! and haptics) into [`Input`] events.
pub use rubylight_wire::input::{
    Batch, Contact, Input, MAX_CONTACTS, SteamAttributesError, TOUCHPAD_FRAME_MAGIC,
    TOUCHPAD_MAGIC, decode,
};

/// The device permission for keyboard and text input, and for its keyboard following the
/// host's text fields (0x553B).
pub const KEYBOARD_PERMISSION: u32 = 1 << 12;

pub fn required_permission(input: &Input) -> u32 {
    match input {
        Input::Keyboard { .. } | Input::Text(_) => KEYBOARD_PERMISSION,
        Input::Touch { .. } => 1 << 9,
        Input::Pen { .. } => 1 << 10,
        Input::Relative { .. }
        | Input::Absolute { .. }
        | Input::MouseButton { .. }
        | Input::Scroll { .. }
        | Input::Touchpad { .. } => 1 << 11,
        _ => 1 << 8,
    }
}

/// The stream-card warning for input this device may not send, naming
/// the permission as the console's device page does.
pub fn denied_warning(input: &Input, device: &str) -> (String, String) {
    let (code, name) = match required_permission(input) {
        0x1000 => ("keyboard", "Keyboard"),
        0x200 => ("touch", "Touch"),
        0x400 => ("pen", "Pen"),
        0x800 => ("mouse", "Mouse"),
        _ => ("controller", "Controllers"),
    };
    (
        format!("input_permission_{code}"),
        format!(
            "{device} sends {code} input, but its device permissions do not allow it, so the host ignores it. Turn on {name} for this device under Devices in the console, then reconnect."
        ),
    )
}

#[cfg(test)]
mod tests;
