//! Pure controller snapshot and button-label style types shared between `nightjar-desktop-media`
//! and `nightjar-desktop-ui` without pulling `egui` or `sdl3-sys`.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::gamepad_types::{Mapping, PadSettings, Report};

/// Controller glyph family reported by SDL (`SDL_GetGamepadType`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PadBrand {
    #[default]
    Xbox,
    PlayStation,
    Nintendo,
}

impl PadBrand {
    /// `(left_click, right_click, start_label)` in the controller's own button names for
    /// controller-mouse toasts without depending on `egui`/`nightjar-desktop-ui`.
    pub fn mouse_toast_labels(brands: impl IntoIterator<Item = Self>, swapped: bool) -> (&'static str, &'static str, &'static str) {
        let style = brands
            .into_iter()
            .find_map(|b| match b {
                Self::PlayStation => Some(Self::PlayStation),
                Self::Nintendo => Some(Self::Nintendo),
                Self::Xbox => None,
            })
            .unwrap_or(Self::Xbox);
        match (style, swapped) {
            (Self::PlayStation, false) => ("×", "○", "Options"),
            (Self::PlayStation, true) => ("○", "×", "Options"),
            (Self::Nintendo, false) => ("B", "A", "+"),
            (Self::Nintendo, true) => ("A", "B", "+"),
            (Self::Xbox, false) => ("A", "B", "Start"),
            (Self::Xbox, true) => ("B", "A", "Start"),
        }
    }
}

pub type RawPadReader = Arc<dyn Fn() -> (u32, [i16; 6]) + Send + Sync>;

/// One connected controller as the launcher UI sees it.
#[derive(Clone)]
pub struct PadEntry {
    pub index: u8,
    pub key: String,
    pub name: String,
    pub brand: PadBrand,
    /// Raw live `(buttons, axes)` reader for the Controllers → Controller test preview.
    pub read_raw: Option<RawPadReader>,
}

impl std::fmt::Debug for PadEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PadEntry")
            .field("index", &self.index)
            .field("key", &self.key)
            .field("name", &self.name)
            .field("brand", &self.brand)
            .finish()
    }
}

impl PadEntry {
    pub fn preview(&self, mapping: &Mapping, settings: impl Into<PadSettings>) -> Report {
        let (buttons, axes) = self.read_raw.as_ref().map_or((0, [0; 6]), |read| read());
        mapping.apply(buttons, axes, settings.into())
    }
}

/// Pure snapshot of connected controllers and saved mappings passed inside `ui::View`.
#[derive(Clone, Debug, Default)]
pub struct PadsView {
    pub pads: Vec<PadEntry>,
    pub mappings: BTreeMap<String, Mapping>,
    pub swap_face_buttons: bool,
}
