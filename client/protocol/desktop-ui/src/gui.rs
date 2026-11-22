//! Pure `egui` focus-scrolling and synthetic key-tap helpers shared by `nightjar-desktop-ui`
//! and `nightjar-desktop::gui`.

use egui::{Event, Key, Modifiers};

/// A synthetic key press and release, for gamepad navigation.
pub fn tap(key: Key) -> [Event; 2] {
    let event = |pressed| Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: Modifiers::NONE,
    };
    [event(true), event(false)]
}

/// Scrolls the enclosing scroll area to the keyboard-focused widget when focus moves. egui
/// does not do this itself, so on a gamepad the stream menu's lower rows took focus out of sight.
/// Call it first in the scroll area's content, with the id it returned last time.
pub fn follow_focus(ui: &mut egui::Ui, last: &mut Option<egui::Id>) {
    follow_focus_inset(ui, last, 0.0);
}

/// [`follow_focus`] for content that scrolls beneath chrome: the widget comes to rest `top_inset`
/// below the area's top edge.
fn follow_focus_inset(ui: &mut egui::Ui, last: &mut Option<egui::Id>, top_inset: f32) {
    let focused = ui.ctx().memory(|m| m.focused());
    if focused == *last {
        return;
    }
    *last = focused;
    if let Some(response) = focused.and_then(|id| ui.ctx().read_response(id))
        && holds(ui, &response)
    {
        ui.scroll_to_rect(response.rect.with_min_y(response.rect.top() - top_inset), None);
    }
}

fn holds(ui: &egui::Ui, response: &egui::Response) -> bool {
    if response.layer_id != ui.layer_id() {
        return false;
    }
    if response.interact_rect.is_positive() {
        return ui.clip_rect().contains_rect(response.interact_rect);
    }
    let (rect, content) = (response.rect, ui.max_rect());
    content.x_range().contains(rect.center().x) && rect.center().y >= content.top()
}

/// [`follow_focus`] for a scroll area whose content passes beneath `top_inset` points of chrome,
/// without a place of its own for the last focus: egui's memory keeps it under `area`, the scroll
/// area's id salt. Call it first in the scroll area's content.
pub fn follow_focus_under(ui: &mut egui::Ui, area: impl std::hash::Hash + std::fmt::Debug, top_inset: f32) {
    let key = egui::Id::unique(("follow-focus", area));
    let mut last = ui.data(|d| d.get_temp::<Option<egui::Id>>(key)).flatten();
    follow_focus_inset(ui, &mut last, top_inset);
    ui.data_mut(|d| d.insert_temp(key, last));
}

/// Whether Text and button size is Auto, kept in egui's memory by the launcher for the passes that
/// run without it.
#[derive(Clone, Copy, Default)]
pub struct AutoZoom(pub bool);
