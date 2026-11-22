//! SDL3 key-event translation for the launcher's stream-hotkey recorder dialog (`Launcher::hotkey_input`)
//! and controller menu hotkey mapping (`stream_menu::offered_hotkey`).

use sdl3_sys::everything::*;

use nightjar_desktop_media::shortcuts_runtime::recorded_keys;
use nightjar_desktop_ui::platform_hooks::{HotkeyInput, MenuNavButton};

/// Whether `scancode` with `mods` moves focus or confirms in the hotkey recorder dialog instead
/// of being recorded (arrow keys or Enter with no modifier).
pub fn moves_in_hotkey_dialog(scancode: SDL_Scancode, mods: SDL_Keymod) -> bool {
    let any_mod = SDL_KMOD_CTRL.0 | SDL_KMOD_ALT.0 | SDL_KMOD_SHIFT.0 | SDL_KMOD_GUI.0;
    mods.0 & any_mod == 0
        && matches!(
            scancode,
            SDL_SCANCODE_UP
                | SDL_SCANCODE_DOWN
                | SDL_SCANCODE_LEFT
                | SDL_SCANCODE_RIGHT
                | SDL_SCANCODE_RETURN
                | SDL_SCANCODE_KP_ENTER
        )
}

/// Converts an SDL3 key-down event into [`HotkeyInput`] for `Launcher::hotkey_input`.
pub fn hotkey_input(scancode: SDL_Scancode, mods: SDL_Keymod, repeat: bool) -> HotkeyInput {
    if repeat {
        return HotkeyInput::ModifierOnly;
    }
    if moves_in_hotkey_dialog(scancode, mods) {
        return HotkeyInput::Navigate;
    }
    let any_mod = SDL_KMOD_CTRL.0 | SDL_KMOD_ALT.0 | SDL_KMOD_SHIFT.0 | SDL_KMOD_GUI.0;
    if scancode == SDL_SCANCODE_ESCAPE && mods.0 & any_mod == 0 {
        return HotkeyInput::Cancel;
    }
    if nightjar_desktop_media::keymap::virtual_key(scancode, true).is_none() {
        let is_modifier = matches!(
            scancode,
            SDL_SCANCODE_LCTRL
                | SDL_SCANCODE_RCTRL
                | SDL_SCANCODE_LALT
                | SDL_SCANCODE_RALT
                | SDL_SCANCODE_LSHIFT
                | SDL_SCANCODE_RSHIFT
                | SDL_SCANCODE_LGUI
                | SDL_SCANCODE_RGUI
        );
        if is_modifier {
            return HotkeyInput::ModifierOnly;
        }
        // SAFETY: SDL_GetScancodeName returns a static NUL-terminated string.
        let name = unsafe {
            let ptr = SDL_GetScancodeName(scancode);
            if ptr.is_null() {
                format!("key {}", scancode.0)
            } else {
                std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned()
            }
        };
        return HotkeyInput::Unsupported { name };
    }
    match recorded_keys(scancode, mods) {
        Some(keys) => HotkeyInput::Recorded(keys),
        None => HotkeyInput::ModifierOnly,
    }
}

/// Maps an [`SDL_GamepadButton`] to a pure [`MenuNavButton`] for `stream_menu::offered_hotkey`.
pub fn menu_nav_button(button: SDL_GamepadButton) -> MenuNavButton {
    match button {
        SDL_GAMEPAD_BUTTON_SOUTH => MenuNavButton::South,
        SDL_GAMEPAD_BUTTON_EAST => MenuNavButton::East,
        SDL_GAMEPAD_BUTTON_WEST => MenuNavButton::West,
        SDL_GAMEPAD_BUTTON_NORTH => MenuNavButton::North,
        SDL_GAMEPAD_BUTTON_LEFT_SHOULDER => MenuNavButton::LeftShoulder,
        SDL_GAMEPAD_BUTTON_RIGHT_SHOULDER => MenuNavButton::RightShoulder,
        _ => MenuNavButton::Other,
    }
}
