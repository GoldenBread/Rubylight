//! Tests of the AT-SPI focus watcher: events, the mapping onto UI Automation's terms and the
//! text window, and a live watch of a real text field.
use super::atspi::{Element, Event, element_id, selection_in, window};
use super::*;
use butterpollo_input_policy::text_input::{InputType, classify};

const EDITABLE: u32 = 1 << 7;
const ENABLED: u32 = 1 << 8;
const MULTI_LINE: u32 = 1 << 17;
const SENSITIVE: u32 = 1 << 24;
const SINGLE_LINE: u32 = 1 << 26;
/// `ATSPI_STATE_READ_ONLY` (43) is in the second word.
const READ_ONLY_HIGH: u32 = 1 << 11;

fn element(role: u32, states: u32, attributes: &[(&str, &str)]) -> Element {
    Element {
        sender: ":1.42".into(),
        path: "/org/a11y/atspi/accessible/7".into(),
        role,
        states: vec![states, 0],
        interfaces: vec!["org.a11y.atspi.Text".into()],
        name: String::new(),
        attributes: attributes
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
    }
}
fn kind(e: &Element) -> Option<InputType> {
    classify(&e.element()).map(|field| field.input_type)
}

#[test]
fn events_are_told_apart() {
    assert_eq!(
        Event::from_parts("StateChanged", "focused", 1),
        Some(Event::Focused(true))
    );
    assert_eq!(
        Event::from_parts("StateChanged", "focused", 0),
        Some(Event::Focused(false))
    );
    assert_eq!(
        Event::from_parts("StateChanged", "active", 1),
        Some(Event::Activated)
    );
    assert_eq!(
        Event::from_parts("StateChanged", "active", 0),
        Some(Event::Deactivated)
    );
    assert_eq!(Event::from_parts("StateChanged", "showing", 1), None);
    assert_eq!(
        Event::from_parts("TextChanged", "insert", 3),
        Some(Event::Text)
    );
    assert_eq!(
        Event::from_parts("TextCaretMoved", "", 3),
        Some(Event::Text)
    );
    assert_eq!(Event::from_parts("ChildrenChanged", "add", 0), None);
}

#[test]
fn toolkit_fields_get_their_keyboard() {
    let live = EDITABLE | ENABLED | SENSITIVE;
    // A Qt or GTK line edit, a text area, a password box, a terminal.
    assert_eq!(
        kind(&element(61, live | SINGLE_LINE, &[])),
        Some(InputType::Text)
    );
    assert_eq!(
        kind(&element(79, live | SINGLE_LINE, &[])),
        Some(InputType::Text)
    );
    assert_eq!(
        kind(&element(61, live | MULTI_LINE, &[])),
        Some(InputType::Multiline)
    );
    let password = element(40, live, &[]);
    assert_eq!(kind(&password), Some(InputType::Password));
    assert!(classify(&password.element()).unwrap().sensitive);
    assert_eq!(kind(&element(60, live, &[])), Some(InputType::Text));
    assert_eq!(kind(&element(52, live, &[])), Some(InputType::Number));
    // An editable web document takes lines.
    assert_eq!(kind(&element(95, live, &[])), Some(InputType::Multiline));
}

#[test]
fn browser_fields_say_their_type() {
    let live = EDITABLE | ENABLED | SENSITIVE | SINGLE_LINE;
    let typed = |input| kind(&element(79, live, &[("text-input-type", input)]));
    assert_eq!(typed("email"), Some(InputType::Email));
    assert_eq!(typed("url"), Some(InputType::Url));
    assert_eq!(typed("search"), Some(InputType::Search));
    assert_eq!(typed("number"), Some(InputType::Number));
    assert_eq!(typed("tel"), Some(InputType::Phone));
    assert_eq!(typed("password"), Some(InputType::Password));
    assert_eq!(typed("text"), Some(InputType::Text));
    assert_eq!(
        kind(&element(79, live, &[("xml-roles", "searchbox")])),
        Some(InputType::Search)
    );
    // A field named like a password is one, whatever it says it is.
    let mut named = element(79, live, &[]);
    named.name = "Password".into();
    assert_eq!(kind(&named), Some(InputType::Password));
}

#[test]
fn what_takes_no_text_gets_no_keyboard() {
    let live = EDITABLE | ENABLED | SENSITIVE;
    // Not editable, read-only, disabled, or not a text role at all (a push button, a label).
    assert_eq!(kind(&element(61, ENABLED | SENSITIVE, &[])), None);
    let mut read_only = element(61, live, &[]);
    read_only.states[1] = READ_ONLY_HIGH;
    assert_eq!(kind(&read_only), None);
    assert_eq!(kind(&element(61, EDITABLE, &[])), None);
    assert_eq!(kind(&element(43, live, &[])), None);
    assert_eq!(kind(&element(29, ENABLED, &[])), None);
    // A combo box takes text only when editable.
    assert_eq!(kind(&element(11, ENABLED | SENSITIVE, &[])), None);
    assert_eq!(kind(&element(11, live, &[])), Some(InputType::Text));
    // A tooltip passes over the field; a field does not.
    assert!(element(64, ENABLED, &[]).passes_over());
    assert!(!element(61, live, &[]).passes_over());
}

#[test]
fn elements_have_their_own_ids() {
    let a = element_id(":1.42", "/org/a11y/atspi/accessible/7");
    assert_ne!(a, 0);
    assert_eq!(a, element_id(":1.42", "/org/a11y/atspi/accessible/7"));
    assert_ne!(a, element_id(":1.43", "/org/a11y/atspi/accessible/7"));
    assert_ne!(a, element_id(":1.42", "/org/a11y/atspi/accessible/8"));
}

#[test]
fn the_text_window_sits_around_the_caret() {
    assert_eq!(window(100, 50, 10), (40, 60));
    assert_eq!(window(100, 5, 10), (0, 15));
    assert_eq!(window(100, 98, 10), (88, 100));
    // An unknown or stray caret is the end, where a single-line field has it.
    assert_eq!(window(100, -1, 10), (90, 100));
    assert_eq!(window(100, 500, 10), (90, 100));
    assert_eq!(window(-4, 0, 10), (0, 0));
    assert_eq!(window(5, 2, usize::MAX), (0, 5));
}

#[test]
fn the_selection_is_counted_in_utf16_units() {
    // Characters 10.. : "a😀b"; the emoji takes two UTF-16 units.
    let text = "a😀bc";
    assert_eq!(selection_in(text, 10, 13, None), Some((4, 4)));
    assert_eq!(selection_in(text, 10, 13, Some((11, 12))), Some((1, 3)));
    // A selection before the window starts at its start; none and no caret is unknown.
    assert_eq!(selection_in(text, 10, -1, Some((2, 11))), Some((0, 1)));
    assert_eq!(selection_in(text, 10, -1, None), None);
    // A reversed selection is ignored for the caret.
    assert_eq!(selection_in(text, 10, 12, Some((12, 11))), Some((3, 3)));
}

/// Activates the windows of `class` afresh through a KWin script: a window started from a
/// terminal in the background is not given the focus.
fn activate(class: &str) {
    let folder = std::env::temp_dir().join(format!("rubylight-activate-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let script = folder.join("activate.js");
    std::fs::write(
        &script,
        // Minimized and shown again, so each call is a fresh activation: a toolkit that
        // started a moment ago may have missed the first.
        format!("for (const w of workspace.windowList()) {{ if (w.resourceClass == \"{class}\") {{ w.minimized = true; w.minimized = false; workspace.activeWindow = w; }} }}"),
    )
    .unwrap();
    let bus = crate::power::logind::session_bus().unwrap();
    let scripting =
        Proxy::new(&bus, "org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting").unwrap();
    let id: i32 = scripting
        .call(
            "loadScript",
            &(script.to_string_lossy().as_ref(), "rubylight-activate"),
        )
        .unwrap();
    Proxy::new(
        &bus,
        "org.kde.KWin",
        format!("/Scripting/Script{id}"),
        "org.kde.kwin.Script",
    )
    .unwrap()
    .call_method("run", &())
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));
    let _ = scripting.call_method("unloadScript", &("rubylight-activate",));
    let _ = std::fs::remove_dir_all(&folder);
}

/// The watcher reports a GTK entry when it takes the focus, with its text, and lowers when
/// the dialog goes. Needs a KDE Plasma session (`WAYLAND_DISPLAY` set) with at-spi2-core and
/// `zenity`.
#[test]
#[ignore = "needs a graphical session with at-spi2-core and zenity"]
fn live_a_gtk_entry_is_reported() {
    let (sender, received) = std::sync::mpsc::channel();
    let watch = FocusWatch::start(
        64,
        move |focused| {
            let _ = sender.send(focused);
        },
        |error| panic!("{error:#}"),
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_secs(1));
    let mut dialog = std::process::Command::new("zenity")
        .args(["--entry", "--entry-text", "rubylight"])
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut seen = None;
    while std::time::Instant::now() < deadline {
        // Until its window shows and takes the focus.
        activate("zenity");
        if let Ok(Some(focused)) = received.recv_timeout(std::time::Duration::from_millis(500)) {
            eprintln!("{focused:?}");
            seen = Some(focused);
            break;
        }
    }
    let _ = dialog.kill();
    let _ = dialog.wait();
    let focused = seen.expect("no text field was reported");
    assert_eq!(focused.field.input_type, InputType::Text);
    assert_eq!(
        focused.surrounding.as_ref().map(|s| s.text.as_str()),
        Some("rubylight")
    );
    // The entry goes with its dialog: the focus leaves it, its window or its application. The
    // same field may be reported again first (its focus and its window's activation), which the
    // session sends once.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut lowered = false;
    while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
        match received.recv_timeout(left) {
            Ok(None) => {
                lowered = true;
                break;
            }
            Ok(Some(again)) => assert_eq!(again.element, focused.element),
            Err(_) => break,
        }
    }
    assert!(lowered, "the keyboard was not lowered when the dialog went");
    drop(watch);
}
