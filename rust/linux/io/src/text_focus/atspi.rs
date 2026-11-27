//! What AT-SPI says about the focused object, as the [`FocusedElement`] the Windows host fills
//! from UI Automation, so one [`classify`] decides the keyboard on both. Roles and states are
//! at-spi2-core's (`atspi-constants.h`); browsers add the field's ARIA role (`xml-roles`) and
//! input type (`text-input-type`) as object attributes.

use butterpollo_input_policy::text_input::{
    Focused, FocusedElement, Surrounding, classify, control_type, passes_over,
};
use std::collections::HashMap;
use zbus::blocking::Connection;

const ROLE_COMBO_BOX: u32 = 11;
const ROLE_PASSWORD_TEXT: u32 = 40;
const ROLE_SPIN_BUTTON: u32 = 52;
const ROLE_TERMINAL: u32 = 60;
const ROLE_TEXT: u32 = 61;
const ROLE_TOOL_TIP: u32 = 64;
const ROLE_PARAGRAPH: u32 = 73;
const ROLE_AUTOCOMPLETE: u32 = 76;
const ROLE_EDITBAR: u32 = 77;
const ROLE_ENTRY: u32 = 79;
const ROLE_DOCUMENT_FRAME: u32 = 82;
const ROLE_DOCUMENT_TEXT: u32 = 94;
const ROLE_DOCUMENT_WEB: u32 = 95;
const ROLE_DOCUMENT_EMAIL: u32 = 96;
const STATE_EDITABLE: u32 = 7;
const STATE_ENABLED: u32 = 8;
const STATE_FOCUSED: u32 = 12;
const STATE_MULTI_LINE: u32 = 17;
const STATE_SENSITIVE: u32 = 24;
const STATE_SHOWING: u32 = 25;
const STATE_READ_ONLY: u32 = 43;
const TEXT: &str = "org.a11y.atspi.Text";

/// An event of the ones the watcher asks for.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Event {
    /// The object gained (true) or lost the focus.
    Focused(bool),
    /// The window became active: a toolkit that gave a widget the focus before its window was
    /// active may never say so, so the watcher looks for it.
    Activated,
    /// The window stopped being active: the focus went to another window, which may publish
    /// nothing (a game, an application without accessibility).
    Deactivated,
    /// Its text or caret changed.
    Text,
}

impl Event {
    /// The event a signal of `org.a11y.atspi.Event.Object` named `member` carries. Its body
    /// starts with the detail and two integers (`siiva{sv}` today, `siiv(so)` before).
    pub(super) fn parse(member: &str, message: &zbus::Message) -> Option<Self> {
        let body = message.body();
        let head = body
            .deserialize::<(
                String,
                i32,
                i32,
                zbus::zvariant::OwnedValue,
                HashMap<String, zbus::zvariant::OwnedValue>,
            )>()
            .map(|(detail, gained, ..)| (detail, gained))
            .or_else(|_| {
                body.deserialize::<(
                    String,
                    i32,
                    i32,
                    zbus::zvariant::OwnedValue,
                    (String, zbus::zvariant::OwnedObjectPath),
                )>()
                .map(|(detail, gained, ..)| (detail, gained))
            })
            .ok()?;
        Self::from_parts(member, &head.0, head.1)
    }
    pub(super) fn from_parts(member: &str, detail: &str, detail1: i32) -> Option<Self> {
        match member {
            "StateChanged" if detail == "focused" => Some(Self::Focused(detail1 != 0)),
            "StateChanged" if detail == "active" => Some(if detail1 != 0 {
                Self::Activated
            } else {
                Self::Deactivated
            }),
            "TextChanged" | "TextCaretMoved" | "TextSelectionChanged" => Some(Self::Text),
            _ => None,
        }
    }
}

/// The focused object as AT-SPI describes it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct Element {
    pub(super) sender: String,
    pub(super) path: String,
    pub(super) role: u32,
    /// The state set: bit n of word n / 32.
    pub(super) states: Vec<u32>,
    pub(super) interfaces: Vec<String>,
    pub(super) name: String,
    pub(super) attributes: HashMap<String, String>,
}

/// Whether `state` is in a state set.
fn has(states: &[u32], state: u32) -> bool {
    states
        .get(state as usize / 32)
        .is_some_and(|word| word & (1 << (state % 32)) != 0)
}

/// The most objects [`focused_within`] reads in one window.
const SEARCH_LIMIT: usize = 2000;

/// The focused object inside the window at `path` of `sender`, searched breadth first among
/// its shown descendants, at most [`SEARCH_LIMIT`] of them.
pub(super) fn focused_within(bus: &Connection, sender: &str, path: &str) -> Option<Element> {
    let mut queue = std::collections::VecDeque::from([(sender.to_owned(), path.to_owned())]);
    let mut read_count = 0;
    while let Some((sender, path)) = queue.pop_front() {
        read_count += 1;
        if read_count > SEARCH_LIMIT {
            return None;
        }
        let accessible = proxy(bus, &sender, &path, "org.a11y.atspi.Accessible")?;
        let Ok(states) = accessible.call::<_, _, Vec<u32>>("GetState", &()) else {
            continue;
        };
        if has(&states, STATE_FOCUSED) {
            return read(bus, &sender, &path);
        }
        // A hidden subtree holds no focus.
        if read_count > 1 && !has(&states, STATE_SHOWING) {
            continue;
        }
        let children: Vec<(String, zbus::zvariant::OwnedObjectPath)> =
            accessible.call("GetChildren", &()).unwrap_or_default();
        queue.extend(
            children
                .into_iter()
                .map(|(child_sender, child)| (child_sender, child.to_string())),
        );
    }
    None
}

impl Element {
    fn has(&self, state: u32) -> bool {
        has(&self.states, state)
    }
    fn attribute(&self, key: &str) -> &str {
        self.attributes.get(key).map_or("", String::as_str)
    }
    /// The element in UI Automation's terms.
    pub(super) fn element(&self) -> FocusedElement {
        let editable = self.has(STATE_EDITABLE) && !self.has(STATE_READ_ONLY);
        let multi_line = self.has(STATE_MULTI_LINE);
        let control_type = match self.role {
            ROLE_TOOL_TIP => control_type::TOOL_TIP,
            ROLE_SPIN_BUTTON => control_type::SPINNER,
            ROLE_COMBO_BOX => control_type::COMBO_BOX,
            // A multi-line text box is a document for the keyboard: Enter starts a new line.
            ROLE_TEXT | ROLE_ENTRY | ROLE_EDITBAR | ROLE_AUTOCOMPLETE if multi_line => {
                control_type::DOCUMENT
            }
            ROLE_TEXT | ROLE_ENTRY | ROLE_PASSWORD_TEXT | ROLE_EDITBAR | ROLE_AUTOCOMPLETE
            | ROLE_TERMINAL => control_type::EDIT,
            ROLE_PARAGRAPH | ROLE_DOCUMENT_FRAME | ROLE_DOCUMENT_TEXT | ROLE_DOCUMENT_WEB
            | ROLE_DOCUMENT_EMAIL => control_type::DOCUMENT,
            _ => 0,
        };
        // A browser's number field is a spin button to the keyboard; its telephone field
        // names itself so the phone hint finds it.
        let input_type = self.attribute("text-input-type").to_ascii_lowercase();
        let (aria_role, localized_type) = match input_type.as_str() {
            "number" => ("spinbutton".to_owned(), String::new()),
            "tel" => (
                self.attribute("xml-roles").to_owned(),
                "telephone".to_owned(),
            ),
            other => (self.attribute("xml-roles").to_owned(), other.to_owned()),
        };
        FocusedElement {
            control_type,
            enabled: self.has(STATE_ENABLED) || self.has(STATE_SENSITIVE),
            password: self.role == ROLE_PASSWORD_TEXT || input_type == "password",
            read_only: Some(!editable),
            text_pattern: self.interfaces.iter().any(|name| name == TEXT),
            aria_role,
            name: self.name.clone(),
            automation_id: self.attribute("id").to_owned(),
            class_name: self.attribute("class").to_owned(),
            localized_type,
            in_spinner: false,
        }
    }
    /// A tooltip shows over the field without taking its keys.
    pub(super) fn passes_over(&self) -> bool {
        passes_over(&self.element(), false)
    }
    /// The focused text field, or None when the element takes no typed text. With
    /// `max_text` above 0 the text around the cursor of a field that is not sensitive.
    pub(super) fn focused(&self, bus: &Connection, max_text: usize) -> Option<Focused> {
        let field = classify(&self.element())?;
        let surrounding = (max_text > 0 && !field.sensitive)
            .then(|| surrounding(bus, &self.sender, &self.path, max_text))
            .flatten();
        Some(Focused {
            field,
            element: element_id(&self.sender, &self.path),
            surrounding,
        })
    }
}

/// The [`Focused::element`] of the object at `path` of the application `sender`, never 0: an
/// FNV-1a hash of both, which stays the same while the object lives.
pub(super) fn element_id(sender: &str, path: &str) -> u32 {
    let hash = sender
        .bytes()
        .chain([0])
        .chain(path.bytes())
        .fold(0x811C_9DC5_u32, |hash, byte| {
            (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
        });
    hash.max(1)
}

fn proxy<'a>(
    bus: &'a Connection,
    sender: &'a str,
    path: &'a str,
    interface: &'a str,
) -> Option<zbus::blocking::Proxy<'a>> {
    zbus::blocking::proxy::Builder::new(bus)
        .destination(sender)
        .ok()?
        .path(path)
        .ok()?
        .interface(interface)
        .ok()?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .ok()
}

/// What the application `sender` says about its object at `path`; None when it does not
/// answer.
pub(super) fn read(bus: &Connection, sender: &str, path: &str) -> Option<Element> {
    let accessible = proxy(bus, sender, path, "org.a11y.atspi.Accessible")?;
    let role: u32 = accessible.call("GetRole", &()).ok()?;
    let states: Vec<u32> = accessible.call("GetState", &()).ok()?;
    let interfaces: Vec<String> = accessible.call("GetInterfaces", &()).unwrap_or_default();
    let attributes: HashMap<String, String> =
        accessible.call("GetAttributes", &()).unwrap_or_default();
    let name: String = accessible.get_property("Name").unwrap_or_default();
    Some(Element {
        sender: sender.to_owned(),
        path: path.to_owned(),
        role,
        states,
        interfaces,
        name,
        attributes,
    })
}

/// Up to `max_text` characters on each side of the caret of the text object at `path`, with
/// the selection (or the caret) in UTF-16 code units into that text; [`bound`] cuts it to the
/// client's bytes.
///
/// [`bound`]: butterpollo_input_policy::text_input::bound
pub(super) fn surrounding(
    bus: &Connection,
    sender: &str,
    path: &str,
    max_text: usize,
) -> Option<Surrounding> {
    let text = proxy(bus, sender, path, TEXT)?;
    let count: i32 = text.get_property("CharacterCount").ok()?;
    let caret: i32 = text.get_property("CaretOffset").unwrap_or(-1);
    let selected: i32 = text.call("GetNSelections", &()).unwrap_or(0);
    let selection: Option<(i32, i32)> = if selected > 0 {
        text.call("GetSelection", &(0i32,)).ok()
    } else {
        None
    };
    let (start, end) = window(count, caret, max_text);
    let content: String = text.call("GetText", &(start, end)).ok()?;
    Some(Surrounding {
        selection: selection_in(&content, start, caret, selection),
        text: content,
    })
}

/// The characters to read: `max_text` on each side of the caret (the end when unknown).
pub(super) fn window(count: i32, caret: i32, max_text: usize) -> (i32, i32) {
    let count = count.max(0);
    let span = i32::try_from(max_text).unwrap_or(i32::MAX);
    let at = if (0..=count).contains(&caret) {
        caret
    } else {
        count
    };
    (
        at.saturating_sub(span).max(0),
        at.saturating_add(span).min(count),
    )
}

/// The selection, else the caret, as UTF-16 code units into `text`, which starts at character
/// `start`; None when neither is known.
pub(super) fn selection_in(
    text: &str,
    start: i32,
    caret: i32,
    selection: Option<(i32, i32)>,
) -> Option<(usize, usize)> {
    let (from, to) = selection
        .filter(|(from, to)| from <= to && *from >= 0)
        .or((caret >= 0).then_some((caret, caret)))?;
    let units = |offset: i32| -> usize {
        let chars = usize::try_from(offset.saturating_sub(start).max(0)).unwrap_or(0);
        text.chars().take(chars).map(char::len_utf16).sum()
    };
    Some((units(from), units(to)))
}
