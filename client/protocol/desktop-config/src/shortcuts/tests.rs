use super::*;

#[test]
fn android_document_round_trips_and_refusals_are_worded_without_replacing_entries() {
    let text = r#"{"version":1,"data":[{"id":"desktop","name":"Task view","keys":["WIN","TAB"]}]}"#;
    let mut shortcuts = parse(text).unwrap();
    assert_eq!(parse(&shortcuts.encode()).unwrap(), shortcuts);
    let before = shortcuts.clone();
    assert_eq!(save(&mut shortcuts, Some("desktop"), "Bad", "CTRL + CTRL_LEFT"), Err(trf("Repeated key: {}", &[&"CTRL_LEFT"])));
    assert_eq!(shortcuts, before);
    assert_eq!(parse(&text.replace("\"version\":1", "\"version\":2")), Err(tr("Unsupported shortcut file version.").to_owned()));
    save(&mut shortcuts, None, "Portable aliases", "VK_LCONTROL + PRINTSCREEN + F01").unwrap();
    assert_eq!((shortcuts.data[1].id.as_str(), &shortcuts.data[1].keys[..]), ("desktop-1", &["CTRL".to_owned(), "SYSRQ".into(), "F1".into()][..]));
}

#[test]
fn controller_buttons_follow_the_list_and_the_nintendo_layout() {
    use crate::gamepad_types::{quit_mask, sdl_button_index};
    assert_eq!(pad_action(Button::South, false), Some(Action::Select));
    assert_eq!(pad_action(Button::East, false), Some(Action::Back));
    assert_eq!(pad_action(Button::South, true), Some(Action::Back));
    assert_eq!(pad_action(Button::East, true), Some(Action::Select));
    assert_eq!(pad_action(Button::Back, true), Some(Action::Back));
    assert_eq!(pad_action(Button::West, false), Some(Action::Options));
    assert_eq!(pad_action(Button::North, false), Some(Action::Options), "as on Android, and the hints name it");
    assert!(PAD.iter().all(|b| b.action != Action::Unstar), "favourites go through the options");
    assert!(KEYS.iter().any(|b| b.action == Action::Unstar), "the keyboard keeps Delete");
    assert_eq!(pad_action(Button::Start, false), Some(Action::OpenSettings));
    assert_eq!(pad_action(Button::LeftShoulder, false), Some(Action::PreviousTab));
    assert_eq!(pad_action(Button::RightShoulder, true), Some(Action::NextTab));
    assert_eq!(stream_controls_mask(), 1 << sdl_button_index::START | 1 << sdl_button_index::BACK);
    assert_eq!(quit_buttons(GamepadQuitCombo::StartSelectLbRb), [Button::Start, Button::Back, Button::LeftShoulder, Button::RightShoulder]);
    for combo in GamepadQuitCombo::ALL {
        let mask = quit_buttons(combo).iter().fold(0, |m, b| m | b.bit());
        assert_eq!(mask, quit_mask(combo), "{combo:?}");
    }
}

#[test]
fn chords_read_in_the_language_in_use_and_typing_keys_stay_in_text_fields() {
    assert_eq!(Chord::of_keys(&["CTRL".into(), "ALT".into(), "SHIFT".into(), "Q".into()]).text(), "Ctrl+Alt+Shift+Q");
    crate::strings::set(crate::strings::Lang::De);
    assert_eq!(Chord::of_keys(&["CTRL".into(), "ALT".into(), "SHIFT".into(), "Q".into()]).text(), "Strg+Alt+Umschalt+Q");
    assert_eq!(Chord::key(Key::Delete).text(), "Entf");
    crate::strings::set(crate::strings::Lang::En);
    assert!(Chord::key(Key::Question).types());
    assert!(!Chord::key(Key::F1).types());
    assert!(!Chord::ctrl(Key::N).types());
    assert_eq!(Chord::of_keys(&["CTRL".into(), "ALT".into(), "SHIFT".into(), "TAB".into()]).text(), "Ctrl+Alt+Shift+Tab");
    assert_eq!(Chord::of_keys(&["CTRL".into(), "PAGE_UP".into()]).text(), "Ctrl+PgUp");
}
