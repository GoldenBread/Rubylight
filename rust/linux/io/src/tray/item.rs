//! The icon itself: a StatusNotifierItem (`org.kde.StatusNotifierItem`). A click opens the
//! console, as a double click on the Windows icon does; its menu is [`super::menu`].
use super::{APP, Action, MENU_PATH, pixmap::Pixmap, pixmap::Pixmaps};
use butterpollo_session::tray::Icon;
use std::sync::mpsc;
use zbus::zvariant::OwnedObjectPath;

/// The tooltip: icon name, icon pictures, title and text.
type ToolTip = (String, Vec<Pixmap>, String, String);

pub(super) struct Item {
    port: u16,
    pixmaps: Pixmaps,
    sender: mpsc::Sender<Action>,
}

impl Item {
    pub(super) fn new(port: u16, pixmaps: Pixmaps, sender: mpsc::Sender<Action>) -> Self {
        Self {
            port,
            pixmaps,
            sender,
        }
    }
}

/// The tooltip's text while `summary` (None: no app) runs, as the Windows tooltip says it.
pub(super) fn tool_tip_text(port: u16, summary: Option<&str>) -> String {
    match summary {
        Some(summary) => summary.to_owned(),
        None => format!("Web port {port}"),
    }
}

#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    #[zbus(property)]
    fn category(&self) -> &str {
        "ApplicationStatus"
    }
    #[zbus(property)]
    fn id(&self) -> &str {
        "rubylight"
    }
    #[zbus(property)]
    fn title(&self) -> &str {
        "Rubylight"
    }
    #[zbus(property)]
    fn status(&self) -> &str {
        "Active"
    }
    #[zbus(property)]
    fn window_id(&self) -> i32 {
        0
    }
    /// A desktop icon only when the host's own picture could not be read.
    #[zbus(property)]
    fn icon_name(&self) -> &str {
        if self.pixmaps[0].is_empty() {
            "application-x-executable"
        } else {
            ""
        }
    }
    #[zbus(property)]
    fn icon_pixmap(&self) -> Vec<Pixmap> {
        let state = match APP.lock().unwrap().icon {
            Icon::Idle => 0,
            Icon::Streaming => 1,
            Icon::Paused => 2,
        };
        self.pixmaps[state].clone()
    }
    #[zbus(property)]
    fn overlay_icon_name(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn overlay_icon_pixmap(&self) -> Vec<Pixmap> {
        Vec::new()
    }
    #[zbus(property)]
    fn attention_icon_name(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn attention_icon_pixmap(&self) -> Vec<Pixmap> {
        Vec::new()
    }
    #[zbus(property)]
    fn attention_movie_name(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn tool_tip(&self) -> ToolTip {
        let summary = APP.lock().unwrap().summary.clone();
        (
            String::new(),
            Vec::new(),
            "Rubylight".into(),
            tool_tip_text(self.port, summary.as_deref()),
        )
    }
    /// A click is [`Item::activate`], not the menu.
    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn menu(&self) -> OwnedObjectPath {
        OwnedObjectPath::try_from(MENU_PATH).expect("a valid object path")
    }

    /// A click on the icon opens the console.
    fn activate(&self, _x: i32, _y: i32) {
        let _ = self.sender.send(Action::Open);
    }
    fn secondary_activate(&self, _x: i32, _y: i32) {}
    /// Trays show the menu from [`Item::menu`]; nothing else is asked for.
    fn context_menu(&self, _x: i32, _y: i32) {}
    fn scroll(&self, _delta: i32, _orientation: &str) {}
}
