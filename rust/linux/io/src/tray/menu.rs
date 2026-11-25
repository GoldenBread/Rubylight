//! The tray icon's menu (`com.canonical.dbusmenu`): the Windows tray's items, in its order,
//! with the same ids.
use super::{APP, Action, REVISION};
use std::{collections::HashMap, sync::atomic::Ordering, sync::mpsc};
use zbus::{
    fdo,
    zvariant::{OwnedValue, StructureBuilder, Value},
};

/// An item and its children: id, properties and the children's layouts.
type Layout = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

/// The menu's root, which holds the items.
const ROOT: i32 = 0;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Entry {
    Item {
        id: i32,
        label: String,
        enabled: bool,
    },
    Separator {
        id: i32,
    },
}

impl Entry {
    fn id(&self) -> i32 {
        match self {
            Self::Item { id, .. } | Self::Separator { id } => *id,
        }
    }
    fn properties(&self) -> HashMap<String, OwnedValue> {
        let mut properties = HashMap::new();
        match self {
            Self::Item { label, enabled, .. } => {
                properties.insert("label".into(), text(label));
                properties.insert("enabled".into(), OwnedValue::from(*enabled));
            }
            Self::Separator { .. } => {
                properties.insert("type".into(), text("separator"));
            }
        }
        properties
    }
}

/// A string property.
fn text(value: &str) -> OwnedValue {
    Value::from(value)
        .try_to_owned()
        .expect("a string holds no file descriptor")
}

/// A label as the menu shows it: an underscore would underline the next letter.
fn label(text: &str) -> String {
    text.replace('_', "__")
}

/// The menu's items while `app` (None: no app) runs; without the controls that stop
/// Rubylight or its streams when they are hidden (`hide_tray_controls`).
pub(super) fn entries(hide_controls: bool, app: Option<&str>) -> Vec<Entry> {
    let item = |id, text: &str| Entry::Item {
        id,
        label: label(text),
        enabled: true,
    };
    let mut entries = vec![
        item(1, "Open Rubylight"),
        item(7, "Pair a device"),
        item(6, "Check for updates"),
    ];
    if !hide_controls {
        entries.push(item(2, "Disconnect clients"));
        entries.push(match app {
            Some(app) => item(5, &format!("Quit {app}")),
            None => Entry::Item {
                id: 5,
                label: "No app running".into(),
                enabled: false,
            },
        });
        entries.extend([
            item(8, "Back up settings"),
            item(9, "Restore settings..."),
            item(3, "Restart"),
            Entry::Separator { id: 10 },
            item(4, "Quit"),
        ]);
    }
    entries
}

/// What choosing item `id` does.
pub(super) fn action(id: i32) -> Option<Action> {
    Some(match id {
        1 => Action::Open,
        2 => Action::StopSessions,
        3 => Action::Restart,
        4 => Action::Quit,
        5 => Action::QuitApp,
        6 => Action::CheckUpdates,
        7 => Action::PairDevice,
        8 => Action::BackupSettings,
        9 => Action::RestoreSettings,
        _ => return None,
    })
}

/// Only the properties asked for; all of them when none is named.
fn wanted(
    mut properties: HashMap<String, OwnedValue>,
    names: &[String],
) -> HashMap<String, OwnedValue> {
    if !names.is_empty() {
        properties.retain(|name, _| names.contains(name));
    }
    properties
}

pub(super) struct Menu {
    hide_controls: bool,
    sender: mpsc::Sender<Action>,
}

impl Menu {
    pub(super) fn new(hide_controls: bool, sender: mpsc::Sender<Action>) -> Self {
        Self {
            hide_controls,
            sender,
        }
    }
    fn entries(&self) -> Vec<Entry> {
        entries(self.hide_controls, APP.lock().unwrap().app.as_deref())
    }
    fn choose(&self, id: i32, event: &str) {
        if event == "clicked"
            && let Some(action) = action(id)
        {
            let _ = self.sender.send(action);
        }
    }
}

fn child(entry: &Entry, names: &[String]) -> fdo::Result<OwnedValue> {
    let layout = StructureBuilder::new()
        .add_field(entry.id())
        .add_field(wanted(entry.properties(), names))
        .add_field(Vec::<OwnedValue>::new())
        .build()
        .map_err(|error| fdo::Error::Failed(error.to_string()))?;
    OwnedValue::try_from(Value::from(layout)).map_err(|error| fdo::Error::Failed(error.to_string()))
}

#[zbus::interface(name = "com.canonical.dbusmenu")]
impl Menu {
    fn get_layout(
        &self,
        parent_id: i32,
        recursion_depth: i32,
        property_names: Vec<String>,
    ) -> fdo::Result<(u32, Layout)> {
        let revision = REVISION.load(Ordering::Relaxed);
        let entries = self.entries();
        if parent_id == ROOT {
            let mut root = HashMap::new();
            root.insert("children-display".to_owned(), text("submenu"));
            let children = if recursion_depth == 0 {
                Vec::new()
            } else {
                entries
                    .iter()
                    .map(|entry| child(entry, &property_names))
                    .collect::<fdo::Result<_>>()?
            };
            return Ok((revision, (ROOT, wanted(root, &property_names), children)));
        }
        let entry = entries
            .iter()
            .find(|entry| entry.id() == parent_id)
            .ok_or_else(|| fdo::Error::InvalidArgs(format!("no menu item {parent_id}")))?;
        Ok((
            revision,
            (
                parent_id,
                wanted(entry.properties(), &property_names),
                Vec::new(),
            ),
        ))
    }
    fn get_group_properties(
        &self,
        ids: Vec<i32>,
        property_names: Vec<String>,
    ) -> Vec<(i32, HashMap<String, OwnedValue>)> {
        self.entries()
            .into_iter()
            .filter(|entry| ids.is_empty() || ids.contains(&entry.id()))
            .map(|entry| (entry.id(), wanted(entry.properties(), &property_names)))
            .collect()
    }
    fn get_property(&self, id: i32, name: String) -> fdo::Result<OwnedValue> {
        self.entries()
            .into_iter()
            .find(|entry| entry.id() == id)
            .and_then(|entry| entry.properties().remove(&name))
            .ok_or_else(|| fdo::Error::InvalidArgs(format!("no property {name} on item {id}")))
    }
    fn event(&self, id: i32, event_id: String, _data: OwnedValue, _timestamp: u32) {
        self.choose(id, &event_id);
    }
    fn event_group(&self, events: Vec<(i32, String, OwnedValue, u32)>) -> Vec<i32> {
        for (id, event, _, _) in &events {
            self.choose(*id, event);
        }
        Vec::new()
    }
    /// The menu changes only with the running app, which `LayoutUpdated` announces.
    fn about_to_show(&self, _id: i32) -> bool {
        false
    }
    fn about_to_show_group(&self, _ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
        (Vec::new(), Vec::new())
    }
    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }
    #[zbus(property)]
    fn text_direction(&self) -> &str {
        "ltr"
    }
    #[zbus(property)]
    fn status(&self) -> &str {
        "normal"
    }
    #[zbus(property)]
    fn icon_theme_path(&self) -> Vec<String> {
        Vec::new()
    }
}
