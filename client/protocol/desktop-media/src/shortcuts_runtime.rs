//! SDL3 and `nightjar-core` input runtime helpers for `nightjar-desktop-config::shortcuts`:
//! scancode lookup, keyboard-layout key naming, modifier masks, hotkey dispatch and chord press/release.

use std::collections::HashSet;

use nightjar_core::input::packets::modifier;
use nightjar_core::input::{InputError, InputQueue};
use sdl3_sys::everything::*;

use crate::shortcuts::{
    self, hotkey_table, key_code, Action, Button, Chord, Key, Scope,
};
use nightjar_policy::shortcuts as policy;

/// Registers this runtime's SDL keyboard-layout character resolver with `nightjar-desktop-config::shortcuts`.
pub fn register_hooks() {
    shortcuts::register_layout_char_hook(layout_char_for_vk);
}

/// Extension methods on [`Key`] requiring SDL scancode / keymap inspection.
pub trait KeySdlExt {
    /// The physical key SDL reports (stream chords are positional, as the PC's keys are).
    fn scancode(self) -> Option<SDL_Scancode>;
}

impl KeySdlExt for Key {
    fn scancode(self) -> Option<SDL_Scancode> {
        Some(match self {
            Key::Tab => SDL_SCANCODE_TAB,
            Key::N => SDL_SCANCODE_N,
            Key::F => SDL_SCANCODE_F,
            Key::F1 => SDL_SCANCODE_F1,
            Key::F2 => SDL_SCANCODE_F2,
            Key::F10 => SDL_SCANCODE_F10,
            Key::F11 => SDL_SCANCODE_F11,
            Key::F12 => SDL_SCANCODE_F12,
            Key::Zero => SDL_SCANCODE_0,
            Key::Escape => SDL_SCANCODE_ESCAPE,
            Key::Enter => SDL_SCANCODE_RETURN,
            Key::Delete => SDL_SCANCODE_DELETE,
            // The first scancode that sends this key to the PC.
            Key::Vk(code) => {
                return (0..SDL_SCANCODE_COUNT.0)
                    .map(SDL_Scancode)
                    .find(|s| crate::keymap::virtual_key(*s, true).is_some_and(|(vk, _)| vk == code));
            }
            // Where these sit depends on the layout; the launcher reads them from egui.
            Key::Question | Key::Plus | Key::Minus | Key::Arrows => return None,
        })
    }
}

/// Resolves the character printed on the active keyboard layout at `vk`'s physical key position.
pub fn layout_char_for_vk(vk: u16) -> Option<char> {
    let scancode = Key::Vk(vk).scancode()?;
    keycode_character(layout_keycode(scancode))
}

/// The character an SDL keycode stands for; none for a key that doesn't print, or a dead key's
/// accent that would have nothing to sit on.
fn keycode_character(keycode: SDL_Keycode) -> Option<char> {
    if keycode.0 & (SDLK_SCANCODE_MASK.0 | SDLK_EXTENDED_MASK.0) != 0 {
        return None;
    }
    char::from_u32(keycode.0)
        .filter(|c| !c.is_control() && !c.is_whitespace() && !matches!(*c, '\u{0300}'..='\u{036F}'))
}

#[cfg(not(any(test, feature = "test-support")))]
fn layout_keycode(scancode: SDL_Scancode) -> SDL_Keycode {
    // SAFETY: only reads SDL's current keymap, and takes any scancode.
    unsafe { SDL_GetKeyFromScancode(scancode, SDL_KMOD_NONE, true) }
}

/// Extension methods on [`Chord`] for SDL modifier checks.
pub trait ChordSdlExt {
    /// Whether SDL's modifier state holds this chord's modifiers (more may be held).
    fn held(self, mods: SDL_Keymod) -> bool;
}

impl ChordSdlExt for Chord {
    fn held(self, mods: SDL_Keymod) -> bool {
        let has = |flag: SDL_Keymod| mods.0 & flag.0 != 0;
        (!self.ctrl || has(SDL_KMOD_CTRL))
            && (!self.alt || has(SDL_KMOD_ALT))
            && (!self.shift || has(SDL_KMOD_SHIFT))
    }
}

/// Extension methods on [`Button`] for converting SDL gamepad buttons.
pub trait ButtonSdlExt {
    fn from_sdl(button: SDL_GamepadButton) -> Option<Button>;
}

impl ButtonSdlExt for Button {
    fn from_sdl(button: SDL_GamepadButton) -> Option<Button> {
        Some(match button {
            SDL_GAMEPAD_BUTTON_SOUTH => Button::South,
            SDL_GAMEPAD_BUTTON_EAST => Button::East,
            SDL_GAMEPAD_BUTTON_WEST => Button::West,
            SDL_GAMEPAD_BUTTON_NORTH => Button::North,
            SDL_GAMEPAD_BUTTON_START => Button::Start,
            SDL_GAMEPAD_BUTTON_BACK => Button::Back,
            SDL_GAMEPAD_BUTTON_LEFT_SHOULDER => Button::LeftShoulder,
            SDL_GAMEPAD_BUTTON_RIGHT_SHOULDER => Button::RightShoulder,
            SDL_GAMEPAD_BUTTON_DPAD_UP
            | SDL_GAMEPAD_BUTTON_DPAD_DOWN
            | SDL_GAMEPAD_BUTTON_DPAD_LEFT
            | SDL_GAMEPAD_BUTTON_DPAD_RIGHT => Button::DPad,
            _ => return None,
        })
    }
}

fn modifier(code: u16) -> u8 {
    match code {
        0xA0 | 0xA1 => modifier::SHIFT,
        0xA2 | 0xA3 => modifier::CTRL,
        0xA4 | 0xA5 => modifier::ALT,
        0x5B | 0x5C => modifier::META,
        _ => 0,
    }
}

/// A chord pressed on the PC, its keys down until [`release`] lets go of them.
#[derive(Debug, PartialEq, Eq)]
pub struct Held {
    /// The keys this chord pressed, in their order.
    keys: Vec<u16>,
    /// The modifiers the user held on the keyboard as it was pressed.
    held_modifiers: u8,
}

/// Presses a chord's keys, modifiers first, leaving out keys the user holds; [`release`] lets go of
/// them once it has been held for [`HOLD_MS`](crate::shortcuts::HOLD_MS).
pub fn press(keys: &[String], held: &HashSet<u16>, queue: &mut InputQueue) -> Result<Held, InputError> {
    let mut keys: Vec<_> = keys
        .iter()
        .map(|name| key_code(name).ok_or(InputError::Invalid))
        .collect::<Result<_, _>>()?;
    keys.retain(|key| !held.contains(key));
    keys.sort_by_key(|code| modifier(*code) == 0);
    if queue.is_holding() {
        return Err(InputError::Stopped);
    }
    // Reserve releases too, so a chord is never pressed without room to let go of it.
    if queue.len() + keys.len() * 2 > nightjar_core::input::queue::MAX_QUEUED {
        return Err(InputError::QueueFull);
    }
    let held_modifiers = held.iter().fold(0, |mask, code| mask | modifier(*code));
    let mut pressed = Held { keys: Vec::new(), held_modifiers };
    for key in keys {
        let modifiers = pressed
            .keys
            .iter()
            .fold(held_modifiers | modifier(key), |mask, code| mask | modifier(*code));
        queue.send_keyboard(0x8000 | key, true, modifiers, 0)?;
        pressed.keys.push(key);
    }
    Ok(pressed)
}

/// Lets go of a held chord's keys in reverse. A key not taken stays in `chord` for another try.
pub fn release(chord: &mut Held, queue: &mut InputQueue) -> Result<(), InputError> {
    while let Some((&key, rest)) = chord.keys.split_last() {
        let modifiers = rest.iter().fold(chord.held_modifiers, |mask, code| mask | modifier(*code));
        queue.send_keyboard(0x8000 | key, false, modifiers, 0)?;
        chord.keys.pop();
    }
    Ok(())
}

/// The modifiers SDL reports held, in the input packets' bits (Shift 1, Ctrl 2, Alt 4, Windows 8).
fn modifier_bits(mods: SDL_Keymod) -> u8 {
    let has = |flag: SDL_Keymod| mods.0 & flag.0 != 0;
    [
        (SDL_KMOD_SHIFT, 0x01),
        (SDL_KMOD_CTRL, 0x02),
        (SDL_KMOD_ALT, 0x04),
        (SDL_KMOD_GUI, 0x08),
    ]
    .into_iter()
    .filter(|(flag, _)| has(*flag))
    .fold(0, |mask, (_, bit)| mask | bit)
}

/// The stream hotkey a key press does: its key with exactly the chord's modifiers held.
fn hotkey_action(scancode: SDL_Scancode, mods: SDL_Keymod) -> Option<Action> {
    let (vk, _) = crate::keymap::virtual_key(scancode, true)?;
    let code = (modifier_bits(mods), vk);
    hotkey_table()
        .into_iter()
        .find(|(c, _)| *c == code)
        .map(|(_, action)| action)
}

/// What a key press does in `scope`, by its physical key and the modifiers held.
pub fn key_action(scope: Scope, scancode: SDL_Scancode, mods: SDL_Keymod) -> Option<Action> {
    if scope == Scope::Stream
        && let Some(action) = hotkey_action(scancode, mods)
    {
        return Some(action);
    }
    shortcuts::KEYS
        .iter()
        .filter(|b| b.scope.covers(scope))
        .find(|b| b.chords.iter().any(|c| c.key.scancode() == Some(scancode) && c.held(mods)))
        .map(|b| b.action)
}

/// A key pressed while a hotkey is recorded, in nightjar-policy's names.
pub fn recorded_keys(scancode: SDL_Scancode, mods: SDL_Keymod) -> Option<Vec<String>> {
    let (vk, _) = crate::keymap::virtual_key(scancode, true)?;
    if policy::is_modifier(&policy::try_key_name(vk)?) {
        return None;
    }
    let held = modifier_bits(mods);
    let mut keys: Vec<String> = [(0x02, "CTRL"), (0x04, "ALT"), (0x01, "SHIFT"), (0x08, "WIN")]
        .into_iter()
        .filter(|(bit, _)| held & bit != 0)
        .map(|(_, name)| name.to_string())
        .collect();
    keys.push(policy::try_key_name(vk)?);
    Some(keys)
}

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    static LAYOUT: std::cell::Cell<&'static [(SDL_Scancode, SDL_Keycode)]> = const { std::cell::Cell::new(&[]) };
}

/// Sets this test thread's layout: the keys that differ from QWERTY.
#[cfg(any(test, feature = "test-support"))]
pub fn use_layout(keys: &'static [(SDL_Scancode, SDL_Keycode)]) {
    LAYOUT.with(|layout| layout.set(keys));
    shortcuts::set_test_layout_char_hook(Some(layout_char_for_vk));
}

#[cfg(any(test, feature = "test-support"))]
fn layout_keycode(scancode: SDL_Scancode) -> SDL_Keycode {
    let qwerty = || {
        crate::keymap::virtual_key(scancode, true)
            .and_then(|(vk, _)| shortcuts::qwerty_character(vk))
            .map_or(SDLK_UNKNOWN, |c| SDL_Keycode(c as u32))
    };
    LAYOUT.with(|layout| {
        layout
            .get()
            .iter()
            .find(|(place, _)| *place == scancode)
            .map_or_else(qwerty, |(_, keycode)| *keycode)
    })
}

#[cfg(test)]
mod tests;
