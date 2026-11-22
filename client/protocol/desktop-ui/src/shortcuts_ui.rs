//! egui key mapping and frame-input matchers for `nightjar-desktop-config::shortcuts`.

use crate::shortcuts::{Action, Chord, Key, Scope, KEYS};

/// The keys egui reports for `key` in the launcher.
pub fn key_egui(key: Key) -> &'static [egui::Key] {
    use egui::Key as E;
    match key {
        Key::Tab => &[E::Tab],
        Key::N => &[E::N],
        Key::F => &[E::F],
        Key::F1 => &[E::F1],
        Key::F2 => &[E::F2],
        Key::F10 => &[E::F10],
        Key::F11 => &[E::F11],
        Key::F12 => &[E::F12],
        Key::Question => &[E::Questionmark],
        Key::Plus => &[E::Plus, E::Equals],
        Key::Minus => &[E::Minus],
        Key::Zero => &[E::Num0],
        Key::Escape => &[E::Escape],
        Key::Enter => &[E::Enter],
        Key::Delete => &[E::Delete],
        Key::Arrows => &[E::ArrowUp, E::ArrowDown, E::ArrowLeft, E::ArrowRight],
        // Stream hotkeys only.
        Key::Vk(_) => &[],
    }
}

/// Whether egui saw the chord this frame (more modifiers may be held).
pub fn chord_pressed(chord: Chord, input: &egui::InputState) -> bool {
    let m = input.modifiers;
    (!chord.ctrl || m.ctrl || m.command)
        && (!chord.alt || m.alt)
        && (!chord.shift || m.shift)
        && key_egui(chord.key).iter().any(|key| input.key_pressed(*key))
}

/// Whether egui saw any of `action`'s chords for the launcher this frame. While a text field has
/// focus (`typing`), a chord that types a character (? alone) belongs to the field.
pub fn pressed(input: &egui::InputState, action: Action, typing: bool) -> bool {
    KEYS.iter()
        .filter(|b| b.scope.covers(Scope::Launcher) && b.action == action)
        .any(|b| b.chords.iter().any(|c| !(typing && c.types()) && chord_pressed(*c, input)))
}
