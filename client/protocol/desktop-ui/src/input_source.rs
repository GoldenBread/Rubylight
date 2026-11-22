//! The device in the hand, from the launcher's last SDL event: what the button hints name. A stick
//! at rest drifts, so it counts only past the point where it moves the focus; a device that SDL calls
//! a gamepad but has no stick is no pad.


use crate::glyphs::{self, Piece, Style};
use crate::shortcuts::{self, Action, Button, Chord, Key};

/// How far a stick goes before it counts as the device in the hand: where it starts to move the focus.
pub const STICK_HELD: i32 = 24000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputSource {
    /// A mouse or a finger: no hints.
    #[default]
    None,
    Keyboard,
    Pad(Style),
}

impl InputSource {
    /// The key or button that does what has focus.
    pub fn go(self, swap_face_buttons: bool) -> Vec<Piece> {
        self.pieces(Action::Select, Key::Enter, swap_face_buttons)
    }

    /// The key or button that opens what has focus's options.
    pub fn more(self, swap_face_buttons: bool) -> Vec<Piece> {
        self.pieces(Action::Options, Key::F, swap_face_buttons)
    }

    fn pieces(self, action: Action, key: Key, swap: bool) -> Vec<Piece> {
        match self {
            InputSource::None => Vec::new(),
            InputSource::Keyboard => glyphs::chord(Chord::key(key), false),
            // North before West: both open the options, and the hint names North.
            InputSource::Pad(style) => [Button::South, Button::East, Button::North, Button::West]
                .into_iter()
                .find(|button| shortcuts::pad_action(*button, swap) == Some(action))
                .map(|button| vec![style.piece(button)])
                .unwrap_or_default(),
        }
    }
}
