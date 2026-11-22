//! The settings page as people use it: sections and their folds, search, reset to default and the
//! PC and game layers, driven through whole frames like the launcher's other tests.

use super::*;
use crate::ui::settings::{simple_apply, simple_caps, simple_state};
use nightjar_policy::simple_profile as simple;
use nightjar_policy::choices::LowerBitrate;
use crate::config::FramePacing;
use nightjar_policy::presets::{self, Preset};

/// Main settings at their defaults but `changed`, on a tall window where a section never scrolls,
/// with "More settings" chosen and Video open: the whole list of sections and the rows most of
/// these tests are about. `fresh_settings` is the page as a first visit finds it.
fn settings_with(changed: Settings) -> Frames {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    f.l.settings = changed;
    f.l.settings_view.rail_more = true;
    f.l.settings_view.section = "Video";
    f.l.go(Screen::Settings);
    f.settle();
    f
}

/// Main settings as a first visit finds them, at `width`.
fn fresh_settings(width: f32) -> Frames {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(width, 4000.0);
    f.l.go(Screen::Settings);
    f.settle();
    f
}

/// The entries of the list beside the rows (chips on a narrow window), top to bottom and left to
/// right: the sections, Profiles and "More settings". The page's own title is not one of them.
fn rail_entries(f: &Frames) -> Vec<&str> {
    let named = |t: &str| SETTINGS_SECTIONS.contains(&t) || t == "Profiles" || t == "More settings";
    let small = f.placed.iter().filter(|(t, ..)| named(t)).map(|p| p.1.height()).fold(f32::MAX, f32::min);
    // The title is the open section's name again, in the page title's size.
    let mut entries: Vec<(&str, Rect)> = f.placed.iter().filter(|(t, r, _)| named(t) && r.height() < small * 1.3).map(|(t, r, _)| (t.as_str(), *r)).collect();
    entries.sort_by(|a, b| (a.1.top(), a.1.left()).partial_cmp(&(b.1.top(), b.1.left())).unwrap());
    entries.dedup_by(|a, b| a.0 == b.0);
    entries.into_iter().map(|e| e.0).collect()
}

const RAIL_SIMPLE: [&str; 5] = ["Recommended", "Controls", "Audio", "App", "More settings"];
/// The list unfolded at the defaults: Debug is listed only on request
/// (`debug_is_listed_on_request_or_while_a_debug_setting_is_changed`).
const RAIL_ALL: [&str; 10] = ["Recommended", "Controls", "Audio", "App", "More settings", "Profiles", "Video", "Display & image", "Streaming", "Advanced"];

/// Simple first: a first visit opens Recommended and lists the four sections most people need;
/// More settings unfolds Profiles and the sections of detail under them, and folds them again.
#[test]
fn the_list_is_simple_first_and_more_settings_unfolds_the_rest() {
    for width in [1280.0, 800.0] {
        let mut f = fresh_settings(width);
        assert_eq!(f.l.settings_view.section, "Recommended", "at {width}");
        assert!(shows(&f, "Stream smoothness"), "{:?}", f.drawn);
        assert_eq!(rail_entries(&f), RAIL_SIMPLE, "at {width}");
        click_text(&mut f, "More settings");
        assert_eq!(rail_entries(&f), RAIL_ALL, "at {width}");
        click_text(&mut f, "Display & image");
        assert!(f.l.settings_view.section == "Display & image" && !shows(&f, "Stream smoothness"), "{:?}", f.drawn);
        // Folding while a section behind the fold is open goes back to the first section: the list
        // never hides what the rows show.
        click_text(&mut f, "More settings");
        assert_eq!(rail_entries(&f), RAIL_SIMPLE, "at {width}");
        assert_eq!(f.l.settings_view.section, "Recommended", "at {width}");
    }
}

/// More settings stays as the user left it across restarts, as on Android: choosing it saves the
/// choice and a launcher started from the saved settings opens with it unfolded. A section behind
/// the fold that a link or a search shows unfolds it for that visit and saves nothing.
#[test]
fn more_settings_is_remembered_but_a_link_or_search_unfold_is_not() {
    let mut f = fresh_settings(1280.0);
    let commands = click_text(&mut f, "More settings");
    assert!(f.l.settings.settings_more_open, "the choice is kept in settings.json");
    assert!(commands.iter().any(|c| matches!(c, Command::SaveSettings)), "and saved");
    let saved = serde_json::to_string(&f.l.settings).unwrap();
    let mut again = Frames::new(record("PC", true, false), Reach::Online);
    again.size = vec2(1280.0, 4000.0);
    again.l = launcher(false, false);
    again.l.settings = serde_json::from_str(&saved).unwrap();
    again.l.settings_view = crate::ui::settings::SettingsView::remembered(again.l.settings.settings_more_open);
    again.l.go(Screen::Settings);
    again.settle();
    assert_eq!(rail_entries(&again), RAIL_ALL, "a new start opens with it unfolded");
    assert!(Launcher::new(again.l.settings.clone(), Profiles::default(), crate::shortcuts::Shortcuts::default(), again.l.display).settings_view.rail_more, "Launcher::new reads it");
    // Folding it again is remembered too.
    click_text(&mut again, "More settings");
    assert!(!again.l.settings.settings_more_open);
    assert_eq!(rail_entries(&again), RAIL_SIMPLE);

    // A link to a section behind the fold, and a search: shown for the visit, nothing saved.
    let mut f = fresh_settings(1280.0);
    f.l.settings_view.section = "Streaming";
    let mut commands = f.settle();
    assert_eq!(rail_entries(&f), RAIL_ALL);
    f.l.settings_view.query = "bitrate".into();
    commands.extend(f.settle());
    f.l.settings_view.query.clear();
    commands.extend(f.settle());
    assert!(!f.l.settings.settings_more_open && !f.l.settings_view.rail_more, "an unfold for a visit is not the user's choice");
    assert!(!commands.iter().any(|c| matches!(c, Command::SaveSettings)), "nothing is saved for it");
    // Reset all leaves it as it is: it is no setting.
    let kept = Settings { settings_more_open: true, setup_card_host: Some(7), setup_card_seen: true, fps: 120, ..Settings::default() };
    let reset = crate::ui::settings::reset_to_defaults(&kept);
    assert_eq!((reset.fps, reset.settings_more_open, reset.setup_card_host, reset.setup_card_seen), (Settings::default().fps, true, Some(7), true));
}

/// Nothing is out of reach behind the fold: a section opened from elsewhere shows with its entry,
/// a search lists every section with its count, and a controller walks into the fold.
#[test]
fn a_section_behind_more_settings_shows_with_its_entry() {
    let mut f = fresh_settings(1280.0);
    f.l.settings_view.section = "Streaming";
    f.settle();
    assert_eq!(rail_entries(&f), RAIL_ALL);
    let mut f = fresh_settings(1280.0);
    f.l.settings_view.query = "bitrate".into();
    f.settle();
    let listed = rail_entries(&f);
    assert!(listed.contains(&"Video") && !listed.contains(&"More settings"), "a search looks in every section: {listed:?}");
    f.l.settings_view.query.clear();
    f.settle();
    assert_eq!(rail_entries(&f), RAIL_SIMPLE, "the search over, the list is as it was");
    // The D-pad: from the rows left to the list, down to More settings, A unfolds it, down again
    // reaches Profiles.
    let mut f = fresh_settings(1280.0);
    f.press(Key::ArrowLeft);
    assert!(focus_to(&mut f, Key::ArrowDown, "More settings"), "Down reaches More settings");
    f.press(Key::Enter);
    assert_eq!(rail_entries(&f), RAIL_ALL);
    assert!(focus_to(&mut f, Key::ArrowDown, "Profiles"), "Down walks into what it unfolded");
}

/// A PC's settings open on Video; the main settings, opened after them, are simple again.
#[test]
fn a_pcs_settings_do_not_unfold_the_main_list() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    assert_eq!(f.l.settings_view.section, "Video");
    f.l.go(Screen::Settings);
    f.settle();
    assert_eq!(f.l.settings_view.section, "Recommended");
    assert_eq!(rail_entries(&f), RAIL_SIMPLE);
    // The user's own choice stays: More settings and Video, away and back.
    click_text(&mut f, "More settings");
    click_text(&mut f, "Video");
    f.l.go(Screen::Home);
    f.settle();
    f.l.go(Screen::Settings);
    f.settle();
    assert_eq!(f.l.settings_view.section, "Video");
    assert_eq!(rail_entries(&f), RAIL_ALL);
}

/// A PC's, game's or profile's settings are the detail already: every section they have, no fold.
#[test]
fn a_pcs_settings_list_every_section_without_a_fold() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    // Debug by the same rule as Settings (`a_pcs_settings_list_debug_by_the_same_rule`).
    assert_eq!(rail_entries(&f), ["Video", "Display & image", "Controls", "Audio", "Streaming", "Advanced"]);
}

/// Opens every section's fold, as its "More … settings" row does.
fn open_folds(f: &mut Frames) {
    f.l.settings_view.more = [true; 9];
    f.settle();
}

/// An open section's rows stay in the card under its main rows, and the fold row is its last line:
/// "Fewer … settings" sits under every row it opened.
#[test]
fn an_open_sections_fold_row_is_its_last_line() {
    let mut f = settings_with(Settings::default());
    open_folds(&mut f);
    let words = fold_words(&f).expect("Video has a fold");
    assert!(words.starts_with("Fewer "), "{words}");
    let fold = f.placed.iter().find(|(t, ..)| *t == words).map(|p| p.1).unwrap();
    let below: Vec<&String> = f.placed.iter().filter(|(t, r, _)| *t != words && (r.left() - fold.left()).abs() < 30.0 && r.top() > fold.bottom()).map(|p| &p.0).collect();
    assert!(below.is_empty(), "nothing of the section under its fold row: {below:?}");
    assert!(f.l.settings_view.drawn.iter().any(|(_, folded)| *folded), "the opened rows are drawn");
}

/// The open section's fold row as the last frame wrote it, if it has one.
fn fold_words(f: &Frames) -> Option<String> {
    f.drawn.iter().find(|t| (t.starts_with("More ") || t.starts_with("Fewer ")) && t.contains(" settings") && *t != "More settings").cloned()
}

/// The rows the last frame drew that hold a setting, by their first key (action rows left out).
fn setting_keys(f: &Frames) -> Vec<&'static str> {
    f.l.settings_view.drawn.iter().map(|(key, _)| *key).filter(|key| !ACTION_ROWS.contains(key)).collect()
}

/// The words right of the row labelled `label`: its control's, and nothing of another row.
fn row_words<'f>(f: &'f Frames, label: &str) -> Vec<&'f str> {
    let at = f.placed.iter().find(|(t, ..)| t == label).unwrap_or_else(|| panic!("{label} is not drawn: {:?}", f.drawn)).1;
    f.placed.iter().filter(|(t, r, _)| t != label && r.left() > at.right() && (r.center().y - at.center().y).abs() < 24.0).map(|(t, ..)| t.as_str()).collect()
}

/// Clicks `text` in the row labelled `label`: its value, which opens its list.
fn click_value(f: &mut Frames, label: &str, text: &str) -> Vec<Command> {
    let row = f.placed.iter().find(|(t, ..)| t == label).unwrap_or_else(|| panic!("{label} is not drawn: {:?}", f.drawn)).1;
    let at = f.placed.iter().find(|(t, r, _)| t == text && r.left() > row.right() && (r.center().y - row.center().y).abs() < 24.0)
        .unwrap_or_else(|| panic!("{text:?} is not in {label}: {:?}", row_words(f, label))).1.center();
    f.click_at(at, egui::PointerButton::Primary)
}

/// Clicks the last `text` drawn: an open list's entry rather than a row's value.
fn click_last(f: &mut Frames, text: &str) -> Vec<Command> {
    let at = f.placed.iter().rev().find(|(t, ..)| t == text).unwrap_or_else(|| panic!("{text:?} is not drawn: {:?}", f.drawn)).1.center();
    f.click_at(at, egui::PointerButton::Primary)
}

/// Presses `key` until focus lands on the widget drawn with `text`. False when it never does.
fn focus_to(f: &mut Frames, key: Key, text: &str) -> bool {
    for _ in 0..16 {
        f.press(key);
        let focused = f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect);
        let at = f.placed.iter().find(|(t, ..)| t == text).map(|p| p.1.center());
        if focused.zip(at).is_some_and(|(r, at)| r.contains(at)) {
            return true;
        }
    }
    false
}

#[test]
fn the_d_pad_scrolls_each_row_it_reaches_into_view() {
    // The main settings and a PC's own, on a window too short for a section. Video, the first, is
    // short at its defaults (its developer rows sit in Debug, its dependent rows hide), so its fold
    // is open and Automatic quality shows the two rows that follow it.
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 600.0);
    f.l.settings = Settings { adaptive_quality: crate::config::QualityMode::Motion, ..Settings::default() };
    f.l.settings_view.more = [true; 9];
    let id = f.hosts.list[0].id;
    for page in [Screen::Settings, Screen::HostSettings(id)] {
        if page == Screen::Settings { f.l.go(page) } else { f.l.open_host_settings(id, &f.hosts.list[0]) }
        // Video is behind More settings on the main settings; a PC's own open on it.
        (f.l.settings_view.rail_more, f.l.settings_view.section) = (true, "Video");
        f.settle();
        let first = f.ctx.memory(|m| m.focused()).expect("the first row has focus");
        let top = f.ctx.read_response(first).expect("laid out").rect.top();
        let mut reached = std::collections::HashSet::new();
        for _ in 0..24 {
            f.press(Key::ArrowDown);
            // The scroll animates.
            f.settle();
            f.settle();
            let focused = f.ctx.memory(|m| m.focused()).expect("a row has focus");
            reached.insert(focused);
            let rect = f.ctx.read_response(focused).expect("laid out").rect;
            assert!(rect.top() >= 0.0 && rect.bottom() <= f.size.y, "{page:?}: the focused row {rect:?} is in the window");
        }
        let moved = top - f.ctx.read_response(first).expect("laid out").rect.top();
        assert!(reached.len() > 6 && moved > 100.0, "{page:?}: Down went past the window's edge and the page scrolled ({} rows, {moved})", reached.len());
    }
}

#[test]
fn the_search_finds_rows_in_every_section_and_says_when_nothing_matches() {
    let mut f = settings_with(Settings { microphone: true, ..Settings::default() });
    f.l.settings_view.query = "microphone".into();
    f.settle();
    // Rows behind a closed fold too.
    for text in ["Results for “microphone”", "Audio", "Send microphone to the PC", "Echo cancellation", "Microphone gain", "Controls", "USB DualSense audio and haptics"] {
        assert!(shows(&f, text), "{text} is found: {:?}", f.drawn);
    }
    assert!(!shows(&f, "Resolution") && !shows(&f, "Channels"), "rows without the word hide: {:?}", f.drawn);
    assert_eq!(fold_words(&f), None, "a search draws no fold");
    // A section's name finds its rows; every word must match somewhere.
    f.l.settings_view.query = "audio channels".into();
    f.settle();
    assert!(shows(&f, "Channels") && !shows(&f, "Send microphone to the PC"), "{:?}", f.drawn);
    f.l.settings_view.query = "zzzz".into();
    f.settle();
    assert!(shows(&f, "No settings match “zzzz”") && !shows(&f, "Channels"), "{:?}", f.drawn);
    click_text(&mut f, "Clear search");
    assert!(f.l.settings_view.query.is_empty() && shows(&f, "Resolution"), "Clear search goes back to the section: {:?}", f.drawn);
}

/// The Ctrl+F hint shows only where the empty field's placeholder leaves it room, in every language.
#[test]
fn the_ctrl_f_hint_never_covers_the_search_placeholder() {
    let mut f = settings_with(Settings::default());
    for language in crate::config::Language::ALL.into_iter().filter(|l| *l != crate::config::Language::System) {
        f.l.settings.language = language;
        for width in [640.0, 800.0, 1024.0, 1280.0] {
            f.size = vec2(width, 4000.0);
            f.settle();
            let at = |text: &str| f.placed.iter().find(|(t, ..)| t == text).map(|(_, r, _)| *r);
            let hint = at(tr("Search settings")).unwrap_or_else(|| panic!("{language:?} at {width}: no placeholder in {:?}", f.drawn));
            if let Some(key) = at(tr("Ctrl+F")) {
                assert!(hint.right() + 8.0 <= key.left(), "{language:?} at {width}: the placeholder {hint:?} runs into Ctrl+F {key:?}");
            }
        }
    }
}

#[test]
fn ctrl_f_reaches_the_search_and_escape_leaves_it_before_the_page() {
    let mut f = settings_with(Settings::default());
    f.press_with(Key::F, egui::Modifiers::COMMAND);
    assert_eq!(f.ctx.memory(|m| m.focused()), Some(Id::unique("settings-search")), "Ctrl+F focuses the search");
    f.frame(vec![egui::Event::Text("hdr".into())]);
    f.settle();
    assert_eq!(f.l.settings_view.query, "hdr");
    assert!(shows(&f, "HDR") && !shows(&f, "Resolution"), "{:?}", f.drawn);
    // Escape leaves the field, then clears the search, then leaves the page.
    let commands = f.press(Key::Escape);
    assert!(f.l.screen == Screen::Settings && commands.is_empty() && f.l.settings_view.query == "hdr", "{}", commands.len());
    let commands = f.press(Key::Escape);
    assert!(f.l.screen == Screen::Settings && commands.is_empty() && f.l.settings_view.query.is_empty(), "{}", commands.len());
    let commands = f.press(Key::Escape);
    assert!(commands.iter().any(|c| matches!(c, Command::SaveSettings)), "then Escape leaves");
}

#[test]
fn a_section_opens_from_its_entry_by_mouse_and_by_controller() {
    let mut f = settings_with(Settings::default());
    assert!(shows(&f, "Resolution") && !shows(&f, "Channels"));
    click_text(&mut f, "Audio");
    assert!(shows(&f, "Channels") && !shows(&f, "Resolution"), "{:?}", f.drawn);
    // The D-pad: from the rows left to the list, down to Streaming, A opens it.
    f.l.go(Screen::Settings);
    f.l.settings_view.section = "Video";
    f.settle();
    f.press(Key::ArrowLeft);
    let focused = f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect);
    let rows = f.placed.iter().find(|(t, ..)| t == "Resolution").expect("the first row").1;
    assert!(focused.is_some_and(|r| r.right() < rows.left()), "Left reaches the search or the list: {focused:?}");
    assert!(focus_to(&mut f, Key::ArrowDown, "Streaming"), "Down walks the list");
    f.press(Key::Enter);
    assert!(shows(&f, "Recover interrupted streams") && !shows(&f, "Resolution"), "{:?}", f.drawn);
    // Opening a section during a search ends the search.
    f.l.settings_view.query = "bitrate".into();
    f.settle();
    click_text(&mut f, "Audio");
    assert!(f.l.settings_view.query.is_empty() && shows(&f, "Channels"), "{:?}", f.drawn);
}

/// Each section shows its main rows (Android's `settings_tiers.xml`), then one row that says how
/// many more it holds and opens them; it closes them again. A search finds folded rows anyway.
#[test]
fn a_sections_fold_counts_its_rows_and_opens_and_closes() {
    let mut f = settings_with(Settings::default());
    let video = SETTINGS_SECTIONS.iter().position(|s| *s == "Video").unwrap();
    assert_eq!(f.l.settings_view.more, [false; 9]);
    assert!(f.l.settings_view.drawn.iter().all(|(_, folded)| !folded), "only main rows: {:?}", f.l.settings_view.drawn);
    assert!(!shows(&f, "Sharper colored text"), "{:?}", f.drawn);
    let more = fold_words(&f).expect("Video has a fold");
    click_text(&mut f, &more);
    assert!(f.l.settings_view.more[video] && shows(&f, "Fewer Video settings") && shows(&f, "Sharper colored text"), "{:?}", f.drawn);
    let drawn = f.l.settings_view.drawn.clone();
    let folded = drawn.iter().filter(|(_, folded)| *folded).count();
    assert_eq!(more, format!("More Video settings ({folded})"), "the count is the rows it opens: {drawn:?}");
    let first = drawn.iter().position(|(_, folded)| *folded).expect("a folded row");
    assert!(drawn[first..].iter().all(|(_, folded)| *folded), "the main rows come first: {drawn:?}");
    // A fold is the section's own: Audio's stays closed.
    click_text(&mut f, "Audio");
    assert!(shows(&f, "Channels") && !shows(&f, "Mute audio") && fold_words(&f).is_some_and(|w| w.starts_with("More Audio settings")), "{:?}", f.drawn);
    click_text(&mut f, "Video");
    click_text(&mut f, "Fewer Video settings");
    assert!(!f.l.settings_view.more[video] && !shows(&f, "Sharper colored text"), "{:?}", f.drawn);
    // The search finds a folded row, and draws no fold.
    f.l.settings_view.query = "colored text".into();
    f.settle();
    assert!(shows(&f, "Sharper colored text") && fold_words(&f).is_none(), "{:?}", f.drawn);
}

/// A row shows its label and value; its one-line hint comes while the pointer is over it or focus
/// is in it, without moving the rows below.
#[test]
fn a_rows_hint_shows_on_hover_and_focus_only() {
    let mut f = settings_with(Settings::default());
    let (size_hint, rate_hint) = ("The picture size the PC sends. Higher is sharper and needs more network and decoding power; lower it if the stream stutters.",
        "Frames per second (FPS). Higher looks smoother and needs a faster network and more decoding power; lower it if the stream stutters.");
    // Resolution has focus on arrival.
    assert!(shows(&f, size_hint) && !shows(&f, rate_hint), "{:?}", f.drawn);
    let y = |f: &Frames, text: &str| f.placed.iter().find(|(t, ..)| t == text).unwrap_or_else(|| panic!("{text}")).1.top();
    let below = y(&f, "Video codec");
    let rate = f.placed.iter().find(|(t, ..)| t == "Frame rate").expect("the row").1;
    f.frame(vec![egui::Event::PointerMoved(rate.center())]);
    f.settle();
    assert!(shows(&f, rate_hint), "{:?}", f.drawn);
    assert!(y(&f, "Video codec") >= below - 0.5, "the rows below never move up");
    // Under the pointer no row changes size, however narrow the room: the hint keeps one line,
    // cut (its whole text is the tooltip), and the rows below stay where they are.
    f.size = vec2(980.0, 4000.0);
    f.settle();
    let below = y(&f, "Video codec");
    let rate = f.placed.iter().find(|(t, ..)| t == "Frame rate").expect("the row").1;
    f.frame(vec![egui::Event::PointerMoved(rate.center())]);
    f.settle();
    let hint = f.placed.iter().find(|(t, ..)| t == rate_hint).expect("the hint").1;
    assert!(hint.height() < 24.0, "one line: {hint:?}");
    assert!(hint_elided(&f, rate_hint), "cut under a pointer");
    assert!((y(&f, "Video codec") - below).abs() < 0.5, "no row moves under the pointer");
}

/// Whether the galley of `text` the last frame drew was cut short with "…".
fn hint_elided(f: &Frames, text: &str) -> bool {
    fn walk(shape: &egui::Shape, text: &str, out: &mut Option<bool>) {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, text, out)),
            egui::Shape::Text(t) if t.galley.job.text == text => *out = Some(t.galley.elided),
            _ => {}
        }
    }
    let mut out = None;
    f.shapes.iter().for_each(|c| walk(&c.shape, text, &mut out));
    out.unwrap_or_else(|| panic!("{text} is not drawn"))
}

/// At 1280 x 800, as the screenshots were taken, the focused Resolution row shows its whole hint,
/// the "when to change it" clause included.
#[test]
fn the_focused_rows_hint_is_drawn_whole_at_1280_by_800() {
    let mut f = settings_with(Settings::default());
    // Keys in the hand: under a pointer the row keeps its size and the hint one line.
    f.l.input_source = crate::input_source::InputSource::Keyboard;
    f.size = vec2(1280.0, 800.0);
    f.settle();
    let size_hint = "The picture size the PC sends. Higher is sharper and needs more network and decoding power; lower it if the stream stutters.";
    assert!(shows(&f, size_hint), "Resolution has focus on arrival: {:?}", f.drawn);
    assert!(!hint_elided(&f, size_hint));
    let hint = f.placed.iter().find(|(t, ..)| t == size_hint).unwrap().1;
    let label = f.placed.iter().find(|(t, ..)| t == "Resolution").unwrap().1;
    assert!(hint.top() >= label.bottom() - 0.5, "under the label: {hint:?} {label:?}");
}

#[test]
fn a_changed_row_offers_its_default_back() {
    let mut f = settings_with(Settings { fps: 120, ..Settings::default() });
    let resets: Vec<_> = f.placed.iter().filter(|(t, ..)| t == "Reset").collect();
    assert_eq!(resets.len(), 1, "only the changed row has a Reset: {:?}", f.drawn);
    click_text(&mut f, "Reset");
    assert_eq!(f.l.settings.fps, Settings::default().fps);
    assert!(!shows(&f, "Reset") && f.l.settings_dirty, "{:?}", f.drawn);
}

#[test]
fn reset_all_asks_first_and_keeps_the_language() {
    let mut f = settings_with(Settings { fps: 120, hdr: true, language: Language::English, ..Settings::default() });
    click_text(&mut f, "App");
    let more = fold_words(&f).expect("App has a fold");
    click_text(&mut f, &more);
    click_text(&mut f, "Reset all…");
    assert!(shows(&f, "Reset all settings?") && shows(&f, "Cancel"), "{:?}", f.drawn);
    // The question says what stays.
    assert!(shows(&f, "Every setting on this page goes back to its default. Your language, PCs, games, profiles, PIN lock, controller buttons, shortcuts and anything not on this page stay as they are. This can’t be undone."), "{:?}", f.drawn);
    click_text(&mut f, "Cancel");
    assert!(f.l.settings.fps == 120 && shows(&f, "Reset all…"), "Cancel changes nothing");
    click_text(&mut f, "Reset all…");
    click_text(&mut f, "Reset all");
    assert_eq!((f.l.settings.fps, f.l.settings.hdr, f.l.settings.language), (0, false, Language::English));
    assert!(f.l.settings_dirty && shows(&f, "Reset all…"), "{:?}", f.drawn);
}

#[test]
fn a_pcs_row_says_whether_it_follows_the_main_settings_or_its_own() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    f.hosts.list[0].record.settings = Some(serde_json::from_str(r#"{"fps":120}"#).unwrap());
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    let main_fps = value_text("fps", &f.l.inherited_settings, &f.l.display, crate::glyphs::Style::Xbox);
    for text in ["1 setting is set here.", "From main settings", "For this PC", &format!("Otherwise {main_fps}")] {
        assert!(shows(&f, text), "{text}: {:?}", f.drawn);
    }
    assert!(!shows(&f, "Reset"), "a PC's rows go back with their box, not Reset");
    // Unticking the Frame rate's box gives the row back to the main settings.
    let menu = f.placed.iter().find(|(t, ..)| t == "120 fps").expect("the PC's own rate").1;
    let boxed = f.placed.iter().filter(|(t, r, _)| t == "For this PC" && r.top() >= menu.bottom()).map(|p| p.1)
        .min_by(|a, b| a.top().total_cmp(&b.top())).expect("its box");
    f.click_at(boxed.center(), egui::PointerButton::Primary);
    assert!(f.l.edit_overrides.is_empty() && shows(&f, "Nothing is set here yet."), "{:?}", f.drawn);
    // A PC's settings have no App section and no Profiles.
    assert!(!shows(&f, "App") && !shows(&f, "Profiles"), "{:?}", f.drawn);
}

/// A search set from code (or typed) after scrolling a long section shows its heading, not the
/// section's old scroll position.
#[test]
fn a_new_search_shows_its_results_from_the_top() {
    let mut f = settings_with(Settings::default());
    f.size = vec2(1280.0, 720.0);
    f.l.settings_view.section = "Controls";
    open_folds(&mut f);
    f.scroll_to_end();
    let seen = |f: &Frames, text: &str| f.placed.iter().find(|(t, ..)| t == text).is_some_and(|(_, r, clip)| r.top() >= clip.top());
    assert!(!seen(&f, "Mouse mode"), "the section scrolled: {:?}", f.drawn);
    f.l.settings_view.query = "bitrate".into();
    f.settle();
    assert!(seen(&f, "Results for “bitrate”"), "the results start at the top: {:?}", f.placed.iter().find(|(t, ..)| t.starts_with("Results")));
}

/// While searching, a section without matches shows no count, and its entry still opens it.
#[test]
fn a_section_without_matches_shows_no_count_and_opens_from_its_entry() {
    let mut f = settings_with(Settings::default());
    f.l.settings_view.query = "bitrate".into();
    f.settle();
    let found = f.l.settings_view.found;
    assert!(found.contains(&0) && found.iter().any(|&n| n > 0), "{found:?}");
    let rows_left = f.placed.iter().find(|(t, ..)| t == "Results for “bitrate”").expect("the results").1.left();
    let counts: Vec<&str> = f.placed.iter().filter(|(t, r, _)| r.right() < rows_left && t.parse::<usize>().is_ok()).map(|(t, ..)| t.as_str()).collect();
    let expected: Vec<String> = found.iter().filter(|&&n| n > 0).map(usize::to_string).collect();
    assert_eq!(counts, expected, "only the sections with matches show a count");
    click_text(&mut f, "Display & image");
    assert!(f.l.settings_view.query.is_empty() && f.l.settings_view.section == "Display & image", "{:?}", f.l.settings_view.query);
    assert!(shows(&f, "Video size") && !shows(&f, "Automatic bitrate"), "{:?}", f.drawn);
}

/// Profiles sit above the sections, a page of their own as on Android; a PC's settings have none.
#[test]
fn profiles_sit_above_the_sections_and_open_their_page() {
    let mut f = settings_with(Settings::default());
    // Above the sections of detail, under More settings.
    assert_eq!(rail_entries(&f)[4..7], ["More settings", "Profiles", "Video"], "{:?}", f.drawn);
    let commands = click_text(&mut f, "Profiles");
    assert_eq!(f.l.screen, Screen::Profiles);
    assert!(commands.iter().any(|c| matches!(c, Command::SaveSettings)), "{} commands", commands.len());
    // A narrow window has its chip first.
    f.l.go(Screen::Settings);
    f.size = vec2(800.0, 4000.0);
    f.settle();
    assert_eq!(rail_entries(&f), RAIL_ALL, "{:?}", f.drawn);
}

/// A PC's row box: unticked and ticked as a checkbox for readers, named for its row, and a click
/// keeps the row's value for the PC.
#[test]
fn a_pcs_row_box_is_a_named_checkbox_that_toggles() {
    use egui::accesskit::{Role, Toggled};
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    f.ctx.enable_accesskit();
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    let frame_rate_box = |f: &Frames| {
        f.access.as_ref().expect("AccessKit is on").nodes.iter()
            .find(|(_, n)| n.role() == Role::CheckBox && n.label() == Some("For this PC") && n.description() == Some("Frame rate"))
            .map(|(_, n)| (n.toggled(), n.bounds().expect("laid out")))
            .expect("the Frame rate row's box")
    };
    let (toggled, bounds) = frame_rate_box(&f);
    assert_eq!(toggled, Some(Toggled::False));
    assert!(bounds.y1 - bounds.y0 >= 24.0, "a box easy to hit: {bounds:?}");
    let at = Pos2::new(((bounds.x0 + bounds.x1) / 2.0) as f32, ((bounds.y0 + bounds.y1) / 2.0) as f32);
    f.click_at(at, egui::PointerButton::Primary);
    assert!(f.l.edit_overrides.has(&["fps"]), "the click keeps Frame rate for this PC");
    assert_eq!(frame_rate_box(&f).0, Some(Toggled::True));
    f.click_at(at, egui::PointerButton::Primary);
    assert!(f.l.edit_overrides.is_empty() && frame_rate_box(&f).0 == Some(Toggled::False), "a second click gives it back");
}

/// A PC's row box sits on the row's last line, beside where the value comes from, under a short
/// switch as under a menu.
#[test]
fn a_pcs_row_box_lines_up_with_where_the_value_comes_from() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    for row in ["Resolution", "Automatic bitrate", "Lower bitrate"] {
        let top = f.placed.iter().find(|(t, ..)| t == row).expect("the row").1.top();
        let below = |text: &str| f.placed.iter().filter(|(t, r, _)| t == text && r.top() > top).map(|p| p.1)
            .min_by(|a, b| a.top().total_cmp(&b.top())).unwrap_or_else(|| panic!("{text} under {row}"));
        let (state, boxed) = (below("From main settings"), below("For this PC"));
        assert!((state.center().y - boxed.center().y).abs() < 3.0, "{row}: {state:?} beside {boxed:?}");
    }
}

/// Left from a row's control lands on the open section's entry (or chip), not on the widget
/// egui finds nearest to the left: on a wide window that is the search or the top of the list,
/// on a narrow one the chip nearest the rows.
#[test]
fn left_from_the_rows_goes_to_the_open_sections_entry() {
    for (width, section) in [(1280.0, "Advanced"), (1280.0, "Audio"), (900.0, "Video"), (900.0, "Audio")] {
        let mut f = settings_with(Settings::default());
        f.size = vec2(width, 4000.0);
        f.l.go(Screen::Settings);
        f.l.settings_view.section = section;
        f.settle();
        let focused = |f: &Frames| f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect);
        let first = focused(&f).expect("the first row's control has focus");
        f.press(Key::ArrowLeft);
        // The entry or chip, in smaller type than the section's heading over the rows.
        let entry = f.placed.iter().filter(|(t, ..)| t == section).map(|p| p.1)
            .min_by(|a, b| a.height().total_cmp(&b.height())).expect("the entry");
        assert!(entry.right() < first.left() || entry.bottom() < first.top(), "{width}: {entry:?} is the entry");
        assert!(focused(&f).is_some_and(|r| r.contains(entry.center())), "{width} {section}: Left lands on {:?}, not the entry {entry:?}", focused(&f));
        if width < 940.0 {
            continue;
        }
        // Right from the list goes back into the rows.
        f.press(Key::ArrowRight);
        assert!(focused(&f).is_some_and(|r| r.left() > entry.right()), "{width} {section}: Right leaves the entry: {:?}", focused(&f));
    }
}

/// The profile notice's line sits beside its button's middle, not at the button's top.
#[test]
fn the_profile_notice_lines_up_with_its_button() {
    let mut f = settings_with(Settings::default());
    let travel = f.l.profiles.create("Travel", serde_json::from_str(r#"{"bitrate_kbps":20000}"#).unwrap()).unwrap();
    f.l.profiles.active = Some(travel);
    f.settle();
    let at = |text: &str| f.placed.iter().find(|(t, ..)| t.starts_with(text)).unwrap_or_else(|| panic!("{text}: {:?}", f.drawn)).1.center().y;
    let (line, button) = (at("Streams on this network use the profile Travel."), at("Manage profiles"));
    assert!((line - button).abs() < 2.0, "the notice at {line}, its button at {button}");
}

/// SECTION_KEYS lists every setting row in the order drawn with the folds open, so a PC's count
/// and Reset all miss none.
#[test]
fn the_section_key_table_is_what_the_page_draws() {
    // Every setting another one shows on.
    let changed = Settings {
        bitrate_kbps: 30_000,
        adaptive_quality: crate::config::QualityMode::Motion,
        upscaling: crate::config::Upscaling::Fsr1,
        virtual_display: true,
        controller_mouse: true,
        microphone: true,
        overlay: true,
        client_roaming: true,
        dual_screen: true,
        pyrowave_decoder: crate::config::PyroWaveDecoder::Gpu,
        decoder: DecoderPreference::Software,
        ..Settings::default()
    };
    let only_here = |key: &str, vrr: bool| match key {
        "check_updates" => (crate::platform_hooks::hooks().self_update_checks)(),
        "network_priority" | "receive_coalescing" => cfg!(any(target_os = "linux", windows)),
        // It needs HDR, which hides Less color banding.
        "hdr_desktop_brightness" => false,
        // A PC's own settings, for a host that reports them (NvidiaEnhancements).
        "rtx_hdr" | "rtx_video" => false,
        // Only where a saved PC offers more than stock packets; the fixture PC has no serverinfo.
        "jumbo_frames" => false,
        "frame_pacing" => !vrr,
        // One row with Fractional V-Sync, saying why the even cadence holds nothing (VRR, macOS, X11).
        "vsync" => true,
        // Everywhere, saying why where it holds nothing (VRR, macOS, X11).
        "fractional_vsync" => true,
        "renderer" | "display_output" | "video_planes" | "high_performance_gpu_power" | "vulkan_video_first" | "vulkan_rfi" => cfg!(target_os = "linux"),
        "videotoolbox_rfi" => cfg!(target_os = "macos"),
        // A Mac's GPU decoder uses the graphics queue of a device of its own.
        "pyrowave_async_compute" => !cfg!(target_os = "macos"),
        "graphics_card" => graphics_card_here(),
        // Windows' wait for the scan line, with Variable refresh rate On.
        "vrr_wait" => cfg!(windows) && vrr,
        _ => true,
    };
    for vrr in [false, true] {
        let mut f = settings_with(Settings { vrr, ..changed.clone() });
        f.l.tearing_supported = true;
        open_folds(&mut f);
        for (at, section) in SETTINGS_SECTIONS.iter().enumerate() {
            f.l.settings_view.section = section;
            f.settle();
            let mut drawn = setting_keys(&f);
            drawn.dedup();
            let listed: Vec<&str> = SECTION_KEYS[at].iter().map(|keys| keys[0]).filter(|k| only_here(k, vrr)).collect();
            assert_eq!(drawn, listed, "{section}, VRR {vrr}");
        }
    }
}

/// The settings for developers and diagnosis sit in Debug, the last section, and in no other, on
/// the main settings and a PC's own; the search still finds them there. Advanced holds the
/// renderer, the game hint and the link and pace tests; Video position stays in Display & image.
#[test]
fn the_developer_rows_and_diagnostics_are_in_debug() {
    // Android's main rows in its order, then PyroWave's, the decoder's, the cursor and Diagnostics.
    const DEBUG: [&str; 11] = ["error_correction", "tell_host_wireless",
        "network_priority", "unbatched_input", "pyrowave_decoder", "pyrowave_async_compute", "decoder",
        "vulkan_video_first", "vulkan_rfi", "videotoolbox_rfi", "cursor_shapes"];
    assert_eq!(SETTINGS_SECTIONS.last(), Some(&"Debug"));
    // Prioritize stream traffic does something on Linux and Windows, so only they draw it; Vulkan
    // video is Linux's; the compute queue is the GPU decoder's, macOS's default is the processor.
    let wanted = DEBUG.into_iter()
        .filter(|k| *k != "network_priority" || cfg!(any(target_os = "linux", windows)))
        .filter(|k| !matches!(*k, "vulkan_video_first" | "vulkan_rfi") || cfg!(target_os = "linux"))
        .filter(|k| *k != "videotoolbox_rfi" || cfg!(target_os = "macos"))
        .filter(|k| *k != "pyrowave_async_compute" || (Settings::default().pyrowave_decoder == crate::config::PyroWaveDecoder::Gpu && !cfg!(target_os = "macos")))
        .collect::<Vec<_>>();
    let mut f = settings_with(Settings::default());
    open_folds(&mut f);
    let id = f.hosts.list[0].id;
    for screen in [Screen::Settings, Screen::HostSettings(id)] {
        f.l.go(screen);
        for section in SETTINGS_SECTIONS {
            if screen != Screen::Settings && section == "App" { continue; }
            f.l.settings_view.section = section;
            f.settle();
            let drawn = setting_keys(&f);
            match section {
                "Debug" => {
                    // Diagnostics, last; decode time after streaming belongs to the whole app.
                    let mut wanted = wanted.clone();
                    wanted.push("link_probe");
                    if screen == Screen::Settings { wanted.push("post_stream_decode_time"); }
                    assert_eq!(drawn, wanted, "{screen:?}: Debug holds exactly these")
                }
                _ => assert!(!drawn.iter().any(|k| DEBUG.contains(k)), "{screen:?} {section}: {drawn:?}"),
            }
            assert_eq!(drawn.contains(&"align_video_top"), section == "Display & image", "{screen:?} {section}: {drawn:?}");
            // Full color range is a display-matching choice: Display & image, not Debug.
            assert_eq!(drawn.contains(&"full_range"), section == "Display & image", "{screen:?} {section}: {drawn:?}");
            if section == "Advanced" {
                // The renderer is Linux's choice for the whole app, so a PC's settings do not have it.
                assert_eq!(drawn.contains(&"renderer"), cfg!(target_os = "linux") && screen == Screen::Settings, "{drawn:?}");
                // So is where it shows itself, and whether a stream goes on a display plane there.
                for key in ["display_output", "video_planes", "high_performance_gpu_power"] {
                    assert_eq!(drawn.contains(&key), cfg!(target_os = "linux") && screen == Screen::Settings, "{key}: {drawn:?}");
                }
                assert!(drawn.contains(&"sops") && !drawn.contains(&"link_probe"), "{screen:?}: {drawn:?}");
            }
        }
    }
    f.l.go(Screen::Settings);
    for (query, row) in [("error correction", "Error correction"), ("local cursor", "Local cursor")] {
        f.l.settings_view.query = query.into();
        f.settle();
        // A search lists "Debug" with the switch off; the section's count says the row was found there.
        assert!(shows(&f, row) && f.l.settings_view.found[SETTINGS_SECTIONS.len() - 1] > 0,
            "{query} finds {row} in Debug: {:?} {:?}", f.l.settings_view.found, f.drawn);
    }
}

/// Main settings like `settings_with`, with the AccessKit tree on.
fn settings_read_with(changed: Settings) -> Frames {
    let mut f = settings_with(changed);
    f.ctx.enable_accesskit();
    f.settle();
    f
}

/// Frame pacing is drawn with Variable refresh rate off, and VRR's way of showing frames is its own
/// row's choice, with no rows of its own: a gate over some rows never carries over to the rows after
/// them. V-Sync is one row with its even cadence, so it stays and says why it holds nothing under
/// VRR; where the screen can't tear, VRR has no row and works as off.
#[test]
fn the_vrr_rows_follow_their_own_gates() {
    let (vrr_row, pacing, vsync) = ("Variable refresh rate", "Frame pacing", "V-Sync");
    let display = |s: Settings, tearing: bool| {
        let mut f = settings_with(s);
        f.l.tearing_supported = tearing;
        f.l.settings_view.section = "Display & image";
        open_folds(&mut f);
        f
    };
    for vrr in [false, true] {
        let f = display(Settings { vrr, frame_pacing: crate::config::FramePacing::Latency, ..Settings::default() }, true);
        assert!(shows(&f, vrr_row), "{:?}", f.drawn);
        // V-Sync is one row with its even cadence, which says why it holds nothing under VRR.
        assert_eq!((shows(&f, pacing), shows(&f, vsync)), (!vrr, true), "VRR {vrr}: {:?}", f.drawn);
        assert_eq!(shows(&f, "Not used with Variable refresh rate"), vrr, "VRR {vrr}: {:?}", f.drawn);
        assert!(!shows(&f, "Real VRR") && !shows(&f, "Tear-free VRR"), "VRR {vrr}: one row: {:?}", f.drawn);
        assert!(shows(&f, "Upscaling"), "VRR {vrr}: the next group is its own");
    }
    let f = display(Settings { vrr: false, frame_pacing: crate::config::FramePacing::Balanced, ..Settings::default() }, true);
    assert!(shows(&f, pacing) && shows(&f, vsync), "a pacing that waits keeps V-Sync on, its cadence still a choice: {:?}", f.drawn);
    if cfg!(windows) {
        let f = display(Settings { vrr: true, frame_pacing: crate::config::FramePacing::Latency, ..Settings::default() }, false);
        assert!(!shows(&f, vrr_row) && shows(&f, pacing) && shows(&f, vsync), "{:?}", f.drawn);
    }
}

/// Variable refresh rate is one choice for its three keys: On and Fastest write all three, Off
/// only `vrr`. A mix none of them writes reads Custom, which the list never offers.
#[test]
fn variable_refresh_rate_is_one_choice_for_its_three_keys() {
    let row = "Variable refresh rate";
    let mut f = settings_with(Settings::default());
    f.l.tearing_supported = true;
    f.l.settings_view.section = "Display & image";
    f.settle();
    assert_eq!(row_words(&f, row), ["On"], "the defaults are On");
    click_value(&mut f, row, "On");
    assert!(shows(&f, "Fastest") && shows(&f, "Off"), "the list is open: {:?}", f.drawn);
    click_last(&mut f, "Fastest");
    let triple = |f: &Frames| (f.l.settings.vrr, f.l.settings.present_at_host_times, f.l.settings.vrr_tear_free);
    assert_eq!(triple(&f), (true, false, false), "Fastest writes all three");
    click_value(&mut f, row, "Fastest");
    click_last(&mut f, "Off");
    assert_eq!(triple(&f), (false, false, false), "Off writes only vrr");
    click_value(&mut f, row, "Off");
    click_last(&mut f, "On");
    assert_eq!(triple(&f), (true, true, true), "On writes all three");

    // A mix from the old switches or a hand-edited file: shown honestly, not offered.
    let mixed = Settings { present_at_host_times: false, vrr_tear_free: true, ..Settings::default() };
    let mut f = settings_with(mixed.clone());
    f.l.tearing_supported = true;
    f.l.settings_view.section = "Display & image";
    f.settle();
    let shown = if vrr_tear_free_applies() { "Custom" } else { "Fastest" };
    let words = row_words(&f, row);
    assert!(words.contains(&shown) && words.contains(&"Reset"), "{words:?}");
    assert_eq!(f.l.settings, mixed, "drawing it changes nothing");
    click_value(&mut f, row, "Reset");
    assert_eq!(triple(&f), (true, true, true), "Reset puts all three back");
    f.l.settings = mixed.clone();
    f.settle();
    let customs = |f: &Frames| f.placed.iter().filter(|(t, ..)| t == "Custom").count();
    let before = customs(&f);
    click_value(&mut f, row, shown);
    assert!(shows(&f, "Off") && shows(&f, "On"), "the list is open: {:?}", f.drawn);
    assert_eq!(customs(&f), before, "Custom is not among the choices: {:?}", f.drawn);
    click_last(&mut f, "On");
    assert_eq!(triple(&f), (true, true, true));
}

/// A PC's own Variable refresh rate keeps all three keys together, and its box gives all three
/// back.
#[test]
fn a_pcs_variable_refresh_rate_keeps_its_three_keys_together() {
    use egui::accesskit::{Role, Toggled};
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    f.ctx.enable_accesskit();
    f.l.tearing_supported = true;
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.l.settings_view.section = "Display & image";
    f.settle();
    let row = "Variable refresh rate";
    click_value(&mut f, row, "On");
    click_last(&mut f, "Fastest");
    for key in ["vrr", "present_at_host_times", "vrr_tear_free"] {
        assert!(f.l.edit_overrides.has(&[key]), "the PC keeps {key}: {:?}", f.l.edit_overrides);
    }
    let edit = &f.l.edit_settings;
    assert_eq!((edit.vrr, edit.present_at_host_times, edit.vrr_tear_free), (true, false, false));
    let row_box = |f: &Frames| {
        f.access.as_ref().expect("AccessKit is on").nodes.iter()
            .find(|(_, n)| n.role() == Role::CheckBox && n.label() == Some("For this PC") && n.description() == Some(row))
            .map(|(_, n)| (n.toggled(), n.bounds().expect("laid out")))
            .expect("the row's box")
    };
    let (toggled, bounds) = row_box(&f);
    assert_eq!(toggled, Some(Toggled::True));
    f.click_at(Pos2::new(((bounds.x0 + bounds.x1) / 2.0) as f32, ((bounds.y0 + bounds.y1) / 2.0) as f32), egui::PointerButton::Primary);
    assert!(f.l.edit_overrides.is_empty(), "the box gives all three back: {:?}", f.l.edit_overrides);
    let edit = &f.l.edit_settings;
    assert_eq!((edit.vrr, edit.present_at_host_times, edit.vrr_tear_free), (true, true, true), "the main settings' On again");
}

/// Sharper colored text is one choice for its two keys, Off or On; under PyroWave it has
/// no row, nor has Less color banding or Video decoding (in Debug).
#[test]
fn sharper_colored_text_is_one_choice_hidden_under_pyrowave() {
    let mut f = settings_with(Settings { experimental_packed_444: true, ..Settings::default() });
    open_folds(&mut f);
    assert!(row_words(&f, "Sharper colored text").contains(&"On") && shows(&f, "Less color banding"), "{:?}", f.drawn);
    click_value(&mut f, "Sharper colored text", "On");
    let commands = click_last(&mut f, "Off");
    assert!(!f.l.settings.prefer_yuv444 && !f.l.settings.experimental_packed_444, "Off turns both off");
    assert!(commands.iter().any(|c| matches!(c, Command::AskFullChromaAgain)), "{} commands", commands.len());
    click_value(&mut f, "Sharper colored text", "Off");
    click_last(&mut f, "On");
    assert!(f.l.settings.prefer_yuv444 && f.l.settings.experimental_packed_444, "On writes both");
    f.l.settings_view.section = "Debug";
    f.settle();
    assert!(shows(&f, "Video decoding"), "{:?}", f.drawn);

    let mut f = settings_with(Settings { codec: CodecPreference::PyroWave, experimental_packed_444: true, ..Settings::default() });
    open_folds(&mut f);
    assert!(!shows(&f, "Sharper colored text") && !shows(&f, "Less color banding"), "{:?}", f.drawn);
    f.l.settings_view.section = "Debug";
    f.settle();
    assert!(!shows(&f, "Video decoding") && shows(&f, "PyroWave decoder"), "{:?}", f.drawn);
}

/// A PC's own value on a row its codec hides (Sharper colored text under PyroWave) still counts;
/// with the codec given back, the row shows with its box ticked, and the box gives it back.
#[test]
fn a_pcs_own_value_on_a_hidden_row_comes_back_with_its_row() {
    use egui::accesskit::{Role, Toggled};
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    f.ctx.enable_accesskit();
    f.l.settings_view.more = [true; 9];
    f.hosts.list[0].record.settings = Some(serde_json::from_str(r#"{"codec":"PyroWave","prefer_yuv444":true}"#).unwrap());
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    let row_box = |f: &Frames, row: &str| {
        f.access.as_ref().expect("AccessKit is on").nodes.iter()
            .find(|(_, n)| n.role() == Role::CheckBox && n.label() == Some("For this PC") && n.description() == Some(row))
            .map(|(_, n)| (n.toggled(), n.bounds().expect("laid out")))
    };
    assert!(row_box(&f, "Sharper colored text").is_none() && shows(&f, "2 settings are set here."), "{:?}", f.drawn);
    let click = |f: &mut Frames, bounds: egui::accesskit::Rect| {
        f.click_at(Pos2::new(((bounds.x0 + bounds.x1) / 2.0) as f32, ((bounds.y0 + bounds.y1) / 2.0) as f32), egui::PointerButton::Primary);
    };
    let (_, codec) = row_box(&f, "Video codec").expect("the codec's box");
    click(&mut f, codec);
    let (toggled, bounds) = row_box(&f, "Sharper colored text").expect("the row is back");
    assert_eq!(toggled, Some(Toggled::True), "with the PC's own value");
    click(&mut f, bounds);
    assert!(f.l.edit_overrides.is_empty(), "{:?}", f.l.edit_overrides);
}

/// Left from any row stays in that row (its Reset, a slider) or goes to the open section's
/// entry, never to a Reset or control of another row, with changed rows above and below.
#[test]
fn left_never_lands_on_another_rows_reset() {
    let focused = |f: &Frames| f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect);
    let changed = [Settings { fps: 120, ..Settings::default() }, Settings { fps: 60, bitrate_kbps: 40_000, ..Settings::default() }];
    for width in [1280.0, 900.0] {
        for (case, settings) in changed.iter().enumerate() {
            for step in 0..8 {
                let mut f = settings_with(settings.clone());
                f.size = vec2(width, 4000.0);
                f.l.go(Screen::Settings);
                f.l.settings_view.section = "Video";
                f.settle();
                for _ in 0..step {
                    f.press(Key::ArrowDown);
                }
                let from = focused(&f).expect("a row's control has focus");
                let entry = f.placed.iter().filter(|(t, ..)| t == "Video").map(|p| p.1)
                    .min_by(|a, b| a.height().total_cmp(&b.height())).expect("the entry");
                if from.contains(entry.center()) {
                    continue;
                }
                f.press(Key::ArrowLeft);
                let to = focused(&f).expect("Left keeps a focus");
                let same_row = to.top() < from.bottom() && from.top() < to.bottom();
                assert!(to.contains(entry.center()) || same_row, "{width} case {case} step {step}: Left from {from:?} lands on {to:?}, not the entry {entry:?} or its row");
            }
        }
    }
    // A changed row's own Reset is still left of its control.
    let mut f = settings_with(Settings { width: 1280, height: 720, ..Settings::default() });
    let from = focused(&f).expect("Resolution has focus");
    f.press(Key::ArrowLeft);
    let reset = f.placed.iter().find(|(t, ..)| t == "Reset").expect("Resolution's Reset").1;
    assert!(focused(&f).is_some_and(|r| r.contains(reset.center())), "Left from {from:?} reaches the row's Reset {reset:?}, not {:?}", focused(&f));
}

/// Escape (B) in the search field leaves it for the first row found, so the D-pad carries on.
#[test]
fn escape_in_the_search_moves_focus_to_the_rows() {
    let mut f = settings_with(Settings::default());
    f.press_with(Key::F, egui::Modifiers::COMMAND);
    f.frame(vec![egui::Event::Text("hdr".into())]);
    f.settle();
    f.press(Key::Escape);
    let focused = |f: &Frames| f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect);
    let results = f.placed.iter().find(|(t, ..)| t == "Results for “hdr”").expect("the results").1;
    assert_eq!(f.l.settings_view.query, "hdr", "the first Escape keeps the search");
    assert!(focused(&f).is_some_and(|r| r.left() > results.left() && r.top() > results.bottom()), "a row found has focus: {:?}", focused(&f));
    // With nothing found, the open section's entry takes it.
    f.press_with(Key::F, egui::Modifiers::COMMAND);
    f.frame(vec![egui::Event::Text("zzzz".into())]);
    f.settle();
    f.press(Key::Escape);
    let entry = f.placed.iter().filter(|(t, ..)| t == "Video").map(|p| p.1).min_by(|a, b| a.height().total_cmp(&b.height())).expect("the entry");
    assert!(focused(&f).is_some_and(|r| r.contains(entry.center())), "the entry has focus: {:?}", focused(&f));
}

/// Ctrl+F is the page's: with a dialog open it leaves the focus where it is.
#[test]
fn ctrl_f_leaves_an_open_dialog_alone() {
    let mut f = settings_with(Settings::default());
    f.l.ask(Dialog::Message { title: "Network test failed".into(), body: "The PC did not answer.".into() });
    f.settle();
    let before = f.ctx.memory(|m| m.focused());
    assert!(before.is_some(), "the dialog's button has focus");
    f.press_with(Key::F, egui::Modifiers::COMMAND);
    assert_eq!(f.ctx.memory(|m| m.focused()), before, "the dialog keeps the focus");
}

/// A PC's own value without a row on the page (beam racing) still counts, and Clear all clears it.
#[test]
fn a_pcs_value_without_a_row_counts_and_clears() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    f.hosts.list[0].record.settings = Some(serde_json::from_str(r#"{"beam_race":true}"#).unwrap());
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    assert!(shows(&f, "1 setting is set here.") && shows(&f, "1 of them has no row on this page; Clear all clears it too."), "{:?}", f.drawn);
    click_text(&mut f, "Clear all for this PC");
    click_text(&mut f, "Clear all");
    assert!(f.l.edit_overrides.is_empty() && shows(&f, "Nothing is set here yet."), "{:?}", f.drawn);
}

/// Bitrate on mobile data is the profiles' cap, Android's list: on the main settings once this
/// device was on mobile data or a cap is set, and not a setting of a PC.
#[test]
fn bitrate_on_mobile_data_is_the_profiles_cap() {
    let arrays = include_str!("../../../../app/src/main/res/values/arrays.xml");
    let values = arrays.split("<string-array name=\"mobile_bitrate_values\"").nth(1).and_then(|a| a.split("</string-array>").next()).expect("Android's list");
    let android: Vec<u32> = values.split("<item>").skip(1).filter_map(|i| i.split("</item>").next()?.trim().parse().ok()).collect();
    assert_eq!(android, MOBILE_BITRATES_KBPS, "the same choices as Android");
    let mut f = settings_with(Settings::default());
    assert!(!shows(&f, "Bitrate on mobile data"), "not on mobile data, no cap: {:?}", f.drawn);
    f.l.profiles.mobile_limit_kbps = 5_000;
    f.settle();
    assert_eq!(row_words(&f, "Bitrate on mobile data"), ["5 Mbps"], "a main row with its value");
    assert!(f.l.settings_view.drawn.contains(&("mobile_bitrate", false)));
    click_value(&mut f, "Bitrate on mobile data", "5 Mbps");
    let commands = click_last(&mut f, "10 Mbps");
    assert_eq!(f.l.profiles.mobile_limit_kbps, 10_000);
    assert!(commands.iter().any(|c| matches!(c, Command::SaveProfiles)), "{} commands", commands.len());
    assert_eq!(f.l.settings, Settings::default(), "a cap, not a setting");
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    assert!(!shows(&f, "Bitrate on mobile data"), "{:?}", f.drawn);
}

/// Controls opens the Controller buttons page from two rows, the test from its own; Controllers
/// is one choice, and with it off the controller rows have nothing to set.
#[test]
fn the_controller_rows_open_their_page_and_controllers_is_one_choice() {
    let mut f = settings_with(Settings::default());
    f.l.settings_view.section = "Controls";
    f.settle();
    for (button, test) in [("Controller buttons", false), ("Test", true)] {
        f.l.controller_test = !test;
        // The button, right of the row's label.
        let at = f.placed.iter().filter(|(t, ..)| t == button).map(|p| p.1).max_by(|a, b| a.left().total_cmp(&b.left())).expect("the button");
        let commands = f.click_at(at.center(), egui::PointerButton::Primary);
        assert!(commands.iter().any(|c| matches!(c, Command::ControllerSettings)), "{button}: {} commands", commands.len());
        assert_eq!(f.l.controller_test, test, "{button}");
        f.l.go(Screen::Settings);
        f.settle();
    }
    open_folds(&mut f);
    assert!(shows(&f, "Nintendo button layout") && shows(&f, "Stick dead zone"), "{:?}", f.drawn);
    click_value(&mut f, "Controllers", "A player each");
    click_last(&mut f, "One player");
    assert_eq!((f.l.settings.controllers, f.l.settings.multi_controller), (true, false));
    click_value(&mut f, "Controllers", "One player");
    click_last(&mut f, "Off");
    assert!(!f.l.settings.controllers, "Off sends none");
    assert!(!shows(&f, "Nintendo button layout") && !shows(&f, "Stick dead zone") && shows(&f, "Send system keys"), "{:?}", f.drawn);
}

/// When Nightjar opens is one choice for Home and the start screen; A chosen game is listed once a
/// game was chosen in its menu.
#[test]
fn when_nightjar_opens_is_one_choice() {
    let mut f = settings_with(Settings::default());
    f.l.settings_view.section = "App";
    f.settle();
    assert_eq!(row_words(&f, "When Nightjar opens"), ["Home"]);
    click_value(&mut f, "When Nightjar opens", "Home");
    assert!(shows(&f, "Show my PCs") && shows(&f, "Continue my last game") && !shows(&f, "A chosen game"), "{:?}", f.drawn);
    click_last(&mut f, "Show my PCs");
    assert_eq!((f.l.settings.open_on_start, f.l.settings.console_home), (crate::config::OpenOnStart::Pcs, false));
    f.l.open_game = Some(crate::config::OpenGame { host: "uid".into(), install_id: "a".into(), app_id: 7, title: "Racing Game".into() });
    f.settle();
    click_value(&mut f, "When Nightjar opens", "Show my PCs");
    click_last(&mut f, "Racing Game");
    assert_eq!(f.l.settings.open_on_start, crate::config::OpenOnStart::Chosen);
    assert!(row_words(&f, "When Nightjar opens").contains(&"Racing Game"), "{:?}", f.drawn);
}

/// The Controller buttons page: one switch for the test, in the rows' style, and no sentence
/// repeated under every axis and button.
#[test]
fn the_controller_buttons_page_has_one_test_switch_and_no_repeated_hints() {
    use egui::accesskit::Role;
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    f.ctx.enable_accesskit();
    f.l.go(Screen::Controllers);
    f.l.controller_key = "pad".into();
    f.l.controller_mapping = crate::gamepad_types::Mapping { name: "Pad".into(), ..Default::default() };
    f.settle();
    assert!(shows(&f, "Controller buttons") && shows(&f, "Controller test"), "{:?}", f.drawn);
    assert!(!f.drawn.iter().any(|t| t.starts_with("Choose the physical axis") || t.starts_with("Default follows the Nintendo") || t.starts_with("Test input")), "{:?}", f.drawn);
    let switch = f.access.as_ref().expect("AccessKit is on").nodes.iter()
        .find(|(_, n)| n.role() != Role::Label && n.label() == Some("Controller test"))
        .and_then(|(_, n)| n.bounds())
        .expect("the test's switch");
    f.click_at(Pos2::new(((switch.x0 + switch.x1) / 2.0) as f32, ((switch.y0 + switch.y1) / 2.0) as f32), egui::PointerButton::Primary);
    assert!(f.l.controller_test);
}

/// Never survives every Recommended choice but Save data, which turns Nightjar's own lowering on;
/// the page does not show the row.
#[test]
fn never_survives_every_recommended_choice_but_save_data() {
    let display = Display { width: 2560, height: 1440, refresh_hz: 120.0, refresh_reported: true };
    let caps = simple_caps(&display, false, true);
    let recommended = simple::recommended(&caps);
    let mut s = Settings { lower_bitrate: LowerBitrate::Keep, ..Settings::default() };
    for choices in [
        simple::Choices { smoothness: simple::Smoothness::Smoothest, ..recommended },
        simple::Choices { hdr: simple::Hdr::Off, ..recommended },
        simple::Choices { picture: simple::Picture::Auto, ..recommended },
        simple::Choices { picture: simple::Picture::Best, ..recommended },
    ].into_iter().enumerate() {
        simple_apply(&mut s, &display, &caps, choices.1);
        assert_eq!(s.lower_bitrate, LowerBitrate::Keep, "choice {}", choices.0);
    }
    simple_apply(&mut s, &display, &caps, simple::Choices { picture: simple::Picture::SaveData, ..recommended });
    assert_eq!(s.lower_bitrate, LowerBitrate::WhenFramesLost);
    // Leaving Save data takes back the lowering it turned on.
    simple_apply(&mut s, &display, &caps, simple::Choices { picture: simple::Picture::Auto, ..recommended });
    assert_eq!(s.lower_bitrate, LowerBitrate::Off);
}

/// A PC's, game's or profile's Lower bitrate saved as the old switches (an imported profile too)
/// reads as its own choice over a main Never.
#[test]
fn an_old_switch_override_over_never_reads_as_its_own_choice() {
    let never = Settings { lower_bitrate: LowerBitrate::Keep, ..Settings::default() };
    for (json, choice) in [(r#"{"adaptive_bitrate":true}"#, LowerBitrate::WhenFramesLost), (r#"{"adaptive_bitrate":false,"client_hint_bitrate":false}"#, LowerBitrate::Off),
        (r#"{"client_hint_bitrate":true}"#, LowerBitrate::WhenFramesLost)] {
        let layer: Overrides = serde_json::from_str(json).unwrap();
        assert_eq!(layer.apply(&never).lower_bitrate, choice, "{json}");
    }
    // A layer without the row leaves the main Never; one saved with Never keeps it.
    let other: Overrides = serde_json::from_str(r#"{"fps":60}"#).unwrap();
    assert_eq!(other.apply(&never).lower_bitrate, LowerBitrate::Keep);
    let kept: Overrides = serde_json::from_str(r#"{"adaptive_bitrate":false,"client_hint_bitrate":false,"keep_bitrate":true}"#).unwrap();
    assert_eq!(kept.apply(&Settings::default()).lower_bitrate, LowerBitrate::Keep);
}

/// Lower bitrate's list open on the Video section, at 1920x1080:
/// `NIGHTJAR_SHOTS=<dir> cargo test -p nightjar-desktop lower_bitrate_open_screenshot -- --ignored`.
#[test]
#[ignore = "writes PNGs; set NIGHTJAR_SHOTS"]
fn lower_bitrate_open_screenshot() {
    let Some(dir) = std::env::var_os("NIGHTJAR_SHOTS").map(std::path::PathBuf::from) else { return };
    std::fs::create_dir_all(&dir).expect("the folder for the screenshots");
    let mut f = home_with_games(1);
    f.size = vec2(1920.0, 1080.0);
    f.shots = true;
    f.l.logo = None;
    f.l.go(Screen::Settings);
    f.l.settings_view.rail_more = true;
    f.l.settings_view.section = "Video";
    f.l.settings_view.to_top = true;
    f.settle();
    let at = f.placed.iter().find(|(t, ..)| t == "The PC decides").expect("the row's value").1.center();
    f.click_at(at, egui::PointerButton::Primary);
    f.settle();
    assert!(f.placed.iter().any(|(t, ..)| t == "Never: keep my bitrate"), "the list is open");
    assert!(!f.placed.iter().any(|(t, ..)| t == "When the network is busy"), "no longer a choice");
    f.shoot(&dir, "settings-lower-bitrate-open-1920x1080");
}

/// The settings page in every state worth a look, at 1280x800 (150 %) and 1920x1080: every
/// section with its main rows (`settings-section-N`) and with its fold open (`-more`, `-more-end`),
/// and Variable refresh rate where the display offers it (`settings-vrr`, `-list`, `-custom`).
/// `NIGHTJAR_SHOTS=<dir> cargo test -p nightjar-desktop settings_screenshots -- --ignored`.
#[test]
#[ignore = "writes PNGs; set NIGHTJAR_SHOTS"]
fn settings_screenshots() {
    let Some(dir) = std::env::var_os("NIGHTJAR_SHOTS").map(std::path::PathBuf::from) else { return };
    std::fs::create_dir_all(&dir).expect("the folder for the screenshots");
    for (width, height, ppp) in [(1280.0, 800.0, 1.5), (1920.0, 1080.0, 1.0)] {
        for high in [false, true] {
            let size = format!("{}x{}{}", width as u32, height as u32, if high { "-high-contrast" } else { "" });
            let fresh = || {
                let mut f = home_with_games(1);
                f.size = vec2(width, height);
                f.native_ppp = ppp;
                f.shots = true;
                // The logo went up in frames before shots were kept; load it again so they see it.
                f.l.logo = None;
                f.l.settings.high_contrast = high;
                f.l.settings.fps = 60;
                f.l.settings.bitrate_kbps = 40_000;
                f
            };
            let shoot = |f: &mut Frames, page: &str| {
                f.settle();
                f.settle();
                f.shoot(&dir, &format!("{page}-{size}"));
            };
            let mut f = fresh();
            f.l.go(Screen::Settings);
            shoot(&mut f, "settings");
            // More settings chosen: Profiles and the sections of detail under the simple ones.
            f.l.settings_view.rail_more = true;
            shoot(&mut f, "settings-more");
            f.l.settings_view.section = "Video";
            f.l.settings_view.to_top = true;
            f.settle();
            // A row's hint, under the pointer.
            let rate = f.placed.iter().find(|(t, ..)| t == "Frame rate").expect("the row").1.center();
            f.frame(vec![egui::Event::PointerMoved(rate)]);
            shoot(&mut f, "settings-hint");
            f.frame(vec![egui::Event::PointerGone]);
            f.l.settings_view.section = "Controls";
            f.l.settings_view.to_top = true;
            shoot(&mut f, "settings-controls");
            if high {
                continue;
            }
            // Every section: its main rows, then its fold open, top and end.
            for (at, section) in SETTINGS_SECTIONS.iter().enumerate() {
                f.l.settings_view.section = section;
                f.l.settings_view.more = [false; 9];
                f.l.settings_view.to_top = true;
                shoot(&mut f, &format!("settings-section-{at}"));
                f.l.settings_view.more[at] = true;
                f.l.settings_view.to_top = true;
                shoot(&mut f, &format!("settings-section-{at}-more"));
                f.scroll_to_end();
                shoot(&mut f, &format!("settings-section-{at}-more-end"));
            }
            f.l.settings_view.more = [true; 9];
            // Dual screen mode on: Second screen resolution can be changed.
            f.l.settings.dual_screen = true;
            f.l.settings_view.section = "Streaming";
            f.l.settings_view.to_top = true;
            f.settle();
            f.scroll_to_end();
            shoot(&mut f, "settings-dual-screen");
            f.l.settings.dual_screen = false;
            f.l.settings_view.query = "bitrate".into();
            shoot(&mut f, "settings-search");
            f.l.settings_view.query = "teleport".into();
            shoot(&mut f, "settings-search-nothing");
            f.l.settings_view.query.clear();
            f.l.settings_view.section = "App";
            f.l.settings_view.confirm_reset = true;
            f.l.settings_view.to_top = true;
            f.settle();
            f.scroll_to_end();
            shoot(&mut f, "settings-reset-all");
            f.l.settings_view.confirm_reset = false;
            f.l.settings_view.section = "Video";
            // Variable refresh rate on a display that offers it: the row, its list, and a mix of
            // its keys the list does not offer.
            let mut f = fresh();
            f.l.tearing_supported = true;
            f.l.go(Screen::Settings);
            f.l.settings_view.section = "Display & image";
            f.l.settings_view.to_top = true;
            shoot(&mut f, "settings-vrr");
            click_value(&mut f, "Variable refresh rate", "On");
            shoot(&mut f, "settings-vrr-list");
            click_last(&mut f, "Fastest");
            f.l.settings.vrr_tear_free = true;
            shoot(&mut f, "settings-vrr-custom");
            // Presets at the defaults (Native marked), and the question a chip asks.
            let mut f = fresh();
            (f.l.settings.fps, f.l.settings.bitrate_kbps) = (0, 0);
            f.l.go(Screen::Settings);
            shoot(&mut f, "settings-presets-native");
            f.l.ask(Dialog::ConfirmPreset(Preset::BatterySaver));
            shoot(&mut f, "dialog-preset");
            f.l.dialog = Dialog::None;
            // The microphone's rows, and Frame pacing's PC's frame times (V-Sync's row gives way).
            f.l.settings.microphone = true;
            f.l.settings.host_frame_times = true;
            f.l.settings_view.more = [true; 9];
            f.l.settings_view.section = "Audio";
            f.l.settings_view.to_top = true;
            shoot(&mut f, "settings-microphone");
            f.l.settings_view.section = "Display & image";
            f.l.settings_view.to_top = true;
            f.settle();
            f.scroll_to_end();
            shoot(&mut f, "settings-frame-pacing-host");
            // Frame pacing's list on a screen without VRR, the PC's frame times among its choices.
            let mut f = fresh();
            f.l.go(Screen::Settings);
            f.l.settings_view.section = "Display & image";
            f.l.settings_view.to_top = true;
            f.settle();
            click_value(&mut f, "Frame pacing", "Lowest latency");
            shoot(&mut f, "settings-frame-pacing-list");
            // About: the version, the source link and the credits, top and end.
            let mut f = fresh();
            f.l.go(Screen::Settings);
            f.l.open(Screen::About);
            shoot(&mut f, "about");
            f.scroll_to_end();
            shoot(&mut f, "about-end");
            // The tests in the main settings: no PC that runs them, one, and two with the PC's list
            // and a saved pace.
            for count in 0..3 {
                let mut f = fresh();
                f.hosts = network_test_pcs(&[("Desk", true, Reach::Online), ("Den", true, Reach::Offline)][..count]);
                if count == 2 {
                    f.hosts.list[0].record.pace = Some(crate::config::PaceCalibration { pace_mbps: 180, client_link_mbps: 1000, host_link_mbps: 2500, wifi: false, ssid: None, qualified: true });
                }
                f.l.go(Screen::Settings);
                f.l.settings_view.more = [true; 9];
                f.l.settings_view.section = "Streaming";
                f.l.settings_view.to_top = true;
                shoot(&mut f, &format!("settings-connection-test-{count}"));
                f.l.settings_view.section = "Advanced";
                f.l.settings_view.to_top = true;
                shoot(&mut f, &format!("settings-frame-pace-{count}"));
            }
            // Bitrate on mobile data, with a cap set on the Profiles page.
            let mut f = fresh();
            f.l.profiles.mobile_limit_kbps = 10_000;
            f.l.go(Screen::Settings);
            shoot(&mut f, "settings-mobile-bitrate");
            // A PC's settings with two of its own values, over a profile.
            let mut f = fresh();
            let pc = f.hosts.list[0].id;
            let travel = f.l.profiles.create("Travel", serde_json::from_str(r#"{"bitrate_kbps":20000}"#).unwrap()).unwrap();
            f.l.profiles.active = Some(travel);
            f.hosts.list[0].record.settings = Some(serde_json::from_str(r#"{"fps":120,"hdr":true}"#).unwrap());
            f.l.open_host_settings(pc, &f.hosts.list[0]);
            shoot(&mut f, "pc-settings");
            f.l.settings_view.section = "Debug";
            f.l.settings_view.to_top = true;
            shoot(&mut f, "pc-settings-debug");
            f.l.settings_view.section = "Video";
            // A game's settings: one of its own, the rest from the PC, the profile and the main settings.
            let app = nightjar_policy::http::App { name: "Racing Game".into(), uuid: "install-0".into(), ..game(1000) };
            f.l.open_app_settings(pc, &f.hosts.list[0], app);
            f.l.edit_overrides.capture(&Settings { codec: CodecPreference::Hevc, ..Settings::default() }, &["codec"]);
            f.l.edit_settings = f.l.edit_overrides.apply(&f.l.inherited_settings);
            shoot(&mut f, "game-settings");
            // The main settings while a profile is in use.
            f.l.go(Screen::Settings);
            shoot(&mut f, "settings-profile-in-use");
        }
    }
}

/// These PCs, each paired or not and answering or not, every one with both network tests.
fn network_test_pcs(pcs: &[(&str, bool, Reach)]) -> Hosts {
    let answer = nightjar_policy::http::ServerInfo::parse("<root status_code=\"200\"><uniqueid>uid</uniqueid><state>SUNSHINE_SERVER_FREE</state>\
        <PairStatus>1</PairStatus><RustHostVersion>3.0.0</RustHostVersion><MaxVideoPacket>8192</MaxVideoPacket><PyroWaveBandwidthProbeBytes>33554432</PyroWaveBandwidthProbeBytes>\
        <PyroWaveUdpProbeVersion>1</PyroWaveUdpProbeVersion><PyroWaveUdpHandshakeVersion>1</PyroWaveUdpHandshakeVersion>\
        <PyroWaveUdpProbeBurstVersion>1</PyroWaveUdpProbeBurstVersion><PyroWaveWireBudgetVersion>1</PyroWaveWireBudgetVersion></root>", 47989).expect("the answer parses");
    let mut hosts = Hosts::without_discovery(pcs.iter().map(|(name, paired, _)| record(name, *paired, false)).collect(), Arc::new(|| {}));
    for (host, (.., reach)) in hosts.list.iter_mut().zip(pcs) {
        (host.reach, host.info) = (*reach, Some(answer.clone()));
    }
    hosts
}

/// The main settings with `pcs` (see `network_test_pcs`), every fold open, on a tall window.
fn settings_with_pcs(pcs: &[(&str, bool, Reach)]) -> Frames {
    let mut f = settings_read_with(Settings::default());
    f.hosts = network_test_pcs(pcs);
    open_folds(&mut f);
    f
}

fn in_section(f: &mut Frames, section: &'static str) {
    f.l.settings_view.section = section;
    f.settle();
}

/// Jumbo video packets appear only when a saved PC offers more than stock 1400-byte packets;
/// the row's own switch is the `jumbo_frames` setting, off by default.
#[test]
fn jumbo_video_packets_are_offered_only_by_a_pc_that_offers_them() {
    use egui::accesskit::Role;
    let mut plain = settings_read_with(Settings::default());
    in_section(&mut plain, "Streaming");
    assert!(!shows(&plain, "Jumbo video packets"), "{:?}", plain.drawn);

    let mut f = settings_with_pcs(&[("Desk", true, Reach::Online)]);
    in_section(&mut f, "Streaming");
    assert!(shows(&f, "Jumbo video packets"), "{:?}", f.drawn);
    assert!(!f.l.settings.jumbo_frames, "off by default");
    let bounds = f.access.as_ref().expect("AccessKit is on").nodes.iter()
        .find(|(_, n)| n.role() == Role::Switch && n.label() == Some("Jumbo video packets"))
        .map(|(_, n)| n.bounds().expect("laid out"))
        .expect("the row's switch");
    let at = Pos2::new(((bounds.x0 + bounds.x1) / 2.0) as f32, ((bounds.y0 + bounds.y1) / 2.0) as f32);
    f.click_at(at, egui::PointerButton::Primary);
    assert!(f.l.settings.jumbo_frames, "the row turns the setting on");

    // A PC's own page edits no global network rows; the setting is in the main settings.
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    in_section(&mut f, "Streaming");
    assert!(!shows(&f, "Jumbo video packets"), "{:?}", f.drawn);
}

/// A customer looked for the PyroWave calibration in Settings: Connection test is one of
/// Streaming's main rows, the frame pace is under Debug › Diagnostics, as on a PC's own
/// settings, and the search finds both.
#[test]
fn the_connection_test_and_frame_pace_are_in_the_main_settings_and_a_pcs_own() {
    let mut f = settings_with_pcs(&[("Desk", true, Reach::Online)]);
    let id = f.hosts.list[0].id;
    for page in [Screen::Settings, Screen::HostSettings(id)] {
        if page != Screen::Settings {
            f.l.open_host_settings(id, &f.hosts.list[0]);
        }
        in_section(&mut f, "Streaming");
        assert!(f.l.settings_view.drawn.contains(&("connection_test", false)), "{page:?}: a main row: {:?}", f.l.settings_view.drawn);
        // One PC is the PC: nothing to choose. An action: no box of a PC's own, no Reset.
        assert_eq!(row_words(&f, "Connection test"), ["Test"], "{page:?}");
        let commands = click_text(&mut f, "Test");
        assert!(matches!(commands[..], [Command::NetworkTest { host, calibrate: false }] if host == id), "{page:?}: {} commands", commands.len());
        in_section(&mut f, "Debug");
        assert!(shows(&f, "Diagnostics"), "{page:?}: {:?}", f.drawn);
        assert_eq!(row_words(&f, "PyroWave frame pace"), ["Calibrate"], "{page:?}: nothing to clear before a calibration");
        let commands = click_text(&mut f, "Calibrate");
        assert!(matches!(commands[..], [Command::NetworkTest { host, calibrate: true }] if host == id), "{page:?}: {} commands", commands.len());
    }
    // A saved pace says so and can be cleared, from the main settings too.
    f.hosts.list[0].record.pace = Some(crate::config::PaceCalibration { pace_mbps: 180, client_link_mbps: 1000, host_link_mbps: 2500, wifi: false, ssid: None, qualified: true });
    f.l.go(Screen::Settings);
    in_section(&mut f, "Debug");
    assert!(shows(&f, "180 Mbps · confirmed"), "{:?}", f.drawn);
    let commands = click_text(&mut f, "Clear");
    assert!(matches!(commands[..], [Command::ClearPace(host)] if host == id), "{} commands", commands.len());

    let at = |name: &str| SETTINGS_SECTIONS.iter().position(|s| *s == name).unwrap();
    for (query, row, section) in [("connection test", "Connection test", "Streaming"), ("download speed", "Connection test", "Streaming"),
        ("pyrowave frame pace", "PyroWave frame pace", "Debug"), ("diagnostics", "PyroWave frame pace", "Debug")] {
        f.l.settings_view.query = query.into();
        f.settle();
        assert!(shows(&f, row) && f.l.settings_view.found[at(section)] > 0, "{query}: {:?} {:?}", f.l.settings_view.found, f.drawn);
    }
    f.l.settings_view.query = "pyrowave frame pace".into();
    f.settle();
    assert!(!shows(&f, "Connection test"), "{:?}", f.drawn);
}

/// With several paired PCs a list beside each test chooses the one tested: the first that is
/// online until the user picks another, never one that is not paired. One off the network says so.
#[test]
fn the_main_settings_tests_choose_among_several_paired_pcs() {
    let mut f = settings_with_pcs(&[("Loft", false, Reach::Online), ("Desk", true, Reach::Offline), ("Den", true, Reach::Online)]);
    let (loft, desk, den) = (f.hosts.list[0].id, f.hosts.list[1].id, f.hosts.list[2].id);
    in_section(&mut f, "Streaming");
    let words = row_words(&f, "Connection test");
    assert!(words.contains(&"Test") && words.contains(&"Den"), "the paired PC that is online: {words:?}");
    assert_eq!(f.l.settings_view.test_pc, None, "nothing is chosen until the user does");
    let commands = click_text(&mut f, "Test");
    assert!(matches!(commands[..], [Command::NetworkTest { host, calibrate: false }] if host == den), "{} commands", commands.len());

    // The list has the paired PCs only; choosing one moves both tests to it.
    click_text(&mut f, "Den");
    assert!(shows(&f, "Desk") && !shows(&f, "Loft"), "{:?}", f.drawn);
    click_last(&mut f, "Desk");
    assert_eq!(f.l.settings_view.test_pc, Some(desk));
    // It is off the network: both tests wait for it.
    let words = row_words(&f, "Connection test");
    assert!(words.contains(&"Desk") && words.contains(&"Offline") && !words.contains(&"Test"), "{words:?}");
    in_section(&mut f, "Debug");
    let words = row_words(&f, "PyroWave frame pace");
    assert!(words.contains(&"Desk") && words.contains(&"Offline") && !words.contains(&"Calibrate"), "{words:?}");
    f.hosts.list[1].reach = Reach::Online;
    f.settle();
    let commands = click_text(&mut f, "Calibrate");
    assert!(matches!(commands[..], [Command::NetworkTest { host, calibrate: true }] if host == desk), "{} commands", commands.len());

    // The choice is the launcher's, not a setting, and holds only while that PC is listed.
    assert_eq!(f.l.settings, Settings::default());
    let pc = |f: &Frames, chosen| network_test_pc(&test_pcs(Screen::Settings, &f.hosts, Host::bandwidth_test), chosen).map(|h| h.id);
    assert_eq!(pc(&f, Some(den)), Some(den));
    assert_eq!(pc(&f, Some(loft)), Some(desk), "not paired: the first paired PC that is online");
    assert_eq!(pc(&f, Some(u64::MAX)), Some(desk), "forgotten");
    f.hosts.list[1].reach = Reach::Offline;
    f.hosts.list[2].reach = Reach::Unknown;
    assert_eq!(pc(&f, None), Some(desk), "none online: the first paired one");

    // One paired PC beside one that is not: nothing to choose.
    let mut f = settings_with_pcs(&[("Loft", false, Reach::Online), ("Desk", true, Reach::Online)]);
    in_section(&mut f, "Streaming");
    assert_eq!(row_words(&f, "Connection test"), ["Test"], "{:?}", f.drawn);
}

/// The tests are drawn only with a paired PC that runs them: none without one; the rows around
/// them stay. Connection test measures ping, jitter and loss on any paired PC, so a PC whose host
/// has no speed test has its row (and the delay alone), as on Android.
#[test]
fn the_tests_have_no_row_without_a_pc_that_runs_them() {
    for pcs in [&[][..], &[("Loft", false, Reach::Online)][..]] {
        let mut f = settings_with_pcs(pcs);
        in_section(&mut f, "Streaming");
        assert!(!shows(&f, "Connection test") && shows(&f, "Recover interrupted streams") && shows(&f, "Keep streaming when the network changes"), "{:?}", f.drawn);
        in_section(&mut f, "Debug");
        assert!(!shows(&f, "PyroWave frame pace") && shows(&f, "Test the link before starting"), "{:?}", f.drawn);
        assert!(test_pcs(Screen::Settings, &f.hosts, Host::bandwidth_test).is_empty());
    }
    // A paired PC that never said it has a speed test (stock hosts): the delay test is there.
    let mut f = settings_with(Settings::default());
    open_folds(&mut f);
    in_section(&mut f, "Streaming");
    assert_eq!(row_words(&f, "Connection test"), ["Test"], "{:?}", f.drawn);
    let id = f.hosts.list[0].id;
    let commands = click_text(&mut f, "Test");
    assert!(matches!(commands[..], [Command::NetworkTest { host, calibrate: false }] if host == id), "{} commands", commands.len());
    in_section(&mut f, "Debug");
    assert!(!shows(&f, "PyroWave frame pace"), "no calibration without PyroWave: {:?}", f.drawn);
}

/// Nightjar #87: system keys can go to the PC only while the stream is full screen. One row with
/// Send system keys' choices, saved in its two switches as before.
#[test]
fn system_keys_only_in_full_screen_is_a_choice_of_send_system_keys() {
    let defaults = Settings::default();
    assert!(defaults.capture_system_keys && !defaults.system_keys_fullscreen_only, "as before: in a window too");
    let old: Settings = serde_json::from_str(r#"{"capture_system_keys":false}"#).unwrap();
    assert!(!old.capture_system_keys && !old.system_keys_fullscreen_only, "a settings file from before");
    let saved = serde_json::to_value(Settings { system_keys_fullscreen_only: true, ..Settings::default() }).unwrap();
    assert_eq!((saved["capture_system_keys"].as_bool(), saved["system_keys_fullscreen_only"].as_bool()), (Some(true), Some(true)));
    let mut f = settings_with(Settings::default());
    f.l.settings_view.section = "Controls";
    open_folds(&mut f);
    assert!(shows(&f, "Send system keys") && !shows(&f, "System keys only in full screen"), "{:?}", f.drawn);
    let rows = setting_keys(&f);
    let at = rows.iter().position(|key| *key == "capture_system_keys").unwrap();
    assert_eq!(rows[at + 1], "right_alt_as_meta", "one row");
}

/// "Keyboard follows text fields", as on Android: on by default, also for a settings file from
/// before it, a main row of Controls beside Clipboard sync, and a PC's own page has it too.
#[test]
fn keyboard_follows_text_fields_is_a_controls_setting_on_by_default() {
    assert!(Settings::default().keyboard_follows_text_fields);
    let old: Settings = serde_json::from_str("{}").unwrap();
    assert!(old.keyboard_follows_text_fields, "a settings file from before");
    let off: Settings = serde_json::from_str(r#"{"keyboard_follows_text_fields":false}"#).unwrap();
    assert!(!off.keyboard_follows_text_fields);
    let mut f = settings_with(Settings::default());
    let id = f.hosts.list[0].id;
    for screen in [Screen::Settings, Screen::HostSettings(id)] {
        f.l.go(screen);
        f.l.settings_view.section = "Controls";
        f.settle();
        let rows = setting_keys(&f);
        let at = rows.iter().position(|key| *key == "keyboard_follows_text_fields").unwrap_or_else(|| panic!("{screen:?}: {rows:?}"));
        assert_eq!(rows[at + 1], "clipboard_sync", "{screen:?}: a main row, as on Android");
        assert!(shows(&f, "Keyboard follows text fields"), "{:?}", f.drawn);
    }
    assert!(ANDROID_KEYS.contains(&("keyboard_follows_text_fields", "checkbox_keyboard_follows_text_fields")));
}

/// A row that does nothing with another setting's value, or on this computer, says why under its
/// label, as Android's tags do; it says nothing where it works.
#[test]
fn rows_that_do_nothing_here_say_why() {
    use crate::config::MouseMode;
    let mut f = settings_with(Settings { mouse_mode: MouseMode::Absolute, ..Settings::default() });
    open_folds(&mut f);
    in_section(&mut f, "Controls");
    assert!(shows(&f, "Not used with Desktop pointer"), "Mouse speed: {:?}", f.drawn);
    assert!(shows(&f, "Not used: this computer has no on-screen keyboard"), "{:?}", f.drawn);
    f.l.screen_keyboard = true;
    f.l.settings.mouse_mode = MouseMode::Capture;
    f.settle();
    assert!(!shows(&f, "Not used: this computer has no on-screen keyboard") && !shows(&f, "Not used with"), "{:?}", f.drawn);
    // Local cursor draws in Desktop pointer mode only.
    in_section(&mut f, "Debug");
    assert!(shows(&f, "Not used with Captured"), "Local cursor: {:?}", f.drawn);
    f.l.settings.mouse_mode = MouseMode::Absolute;
    f.settle();
    assert!(!shows(&f, "Not used with Captured"), "{:?}", f.drawn);
    if cfg!(target_os = "linux") {
        f.l.settings.decoder = DecoderPreference::Software;
        f.settle();
        assert!(shows(&f, "Not used with Software"), "Vulkan video rows: {:?}", f.drawn);
    }
}

/// The rows that were two switches each are one choice, and a value saved by an earlier version
/// reads as the stream used it: "Back opens the stream menu" won over "Back button as Guide",
/// full screen only and mobile data counted only with their first switch on, and frame loss took
/// the place of the busy network wherever it ran.
#[test]
fn paired_switches_read_as_one_choice() {
    let display = crate::stream_types::Display { width: 1920, height: 1080, refresh_hz: 60.0, refresh_reported: true };
    let value = |json: &str, key: &str| {
        let s: Settings = serde_json::from_str(json).unwrap();
        value_text(key, &s, &display, crate::glyphs::Style::Xbox)
    };
    assert_eq!(value("{}", "back_opens_menu"), "Back");
    assert_eq!(value(r#"{"back_as_guide":true}"#, "back_opens_menu"), "Guide");
    assert_eq!(value(r#"{"back_opens_menu":true,"back_as_guide":true}"#, "back_as_guide"), "Stream menu");
    assert_eq!(value("{}", "capture_system_keys"), "Always");
    assert_eq!(value(r#"{"system_keys_fullscreen_only":true}"#, "capture_system_keys"), "In full screen");
    assert_eq!(value(r#"{"capture_system_keys":false,"system_keys_fullscreen_only":true}"#, "capture_system_keys"), "Off");
    assert_eq!(value("{}", "client_roaming"), "Except mobile data");
    assert_eq!(value(r#"{"roaming_metered":true}"#, "roaming_metered"), "Any network");
    assert_eq!(value(r#"{"client_roaming":false,"roaming_metered":true}"#, "client_roaming"), "Off");
    assert_eq!(value("{}", "lower_bitrate"), "The PC decides");
    assert_eq!(value(r#"{"lower_bitrate":"sooner"}"#, "lower_bitrate"), "Lower it sooner");
    assert_eq!(value(r#"{"lower_bitrate":"keep"}"#, "lower_bitrate"), "Never: keep my bitrate");
    // A PC's own value carries the key.
    let mut own_rate = Overrides::default();
    own_rate.capture(&Settings { lower_bitrate: LowerBitrate::Keep, ..Settings::default() }, &["lower_bitrate"]);
    assert_eq!(own_rate.apply(&Settings::default()).lower_bitrate, LowerBitrate::Keep, "Never for this PC over the main setting");
    let json = serde_json::to_string(&own_rate).unwrap();
    assert_eq!(serde_json::from_str::<Overrides>(&json).unwrap().apply(&Settings::default()).lower_bitrate, LowerBitrate::Keep);

    // A PC's own value keeps both switches of its choice, so the layer reads whole.
    let mut own = Overrides::default();
    let chosen = Settings { back_opens_menu: false, back_as_guide: true, ..Settings::default() };
    own.capture(&chosen, &["back_opens_menu", "back_as_guide"]);
    let main = Settings { back_opens_menu: true, ..Settings::default() };
    let applied = own.apply(&main);
    assert!(!applied.back_opens_menu && applied.back_as_guide, "Guide for this PC over the main Stream menu");
    // The old rows are gone from every section.
    for gone in ["back_as_guide", "system_keys_fullscreen_only", "roaming_metered", "adaptive_bitrate", "client_hint_bitrate", "keep_bitrate"] {
        assert!(SECTION_KEYS.iter().all(|rows| rows.iter().all(|keys| keys[0] != gone)), "{gone} names no row");
    }
}

/// Runs `draw` in a bare frame `width` wide.
fn bare_frame<R>(width: f32, draw: impl FnOnce(&mut Ui) -> R) -> R {
    let ctx = egui::Context::default();
    let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, 800.0))), ..Default::default() };
    let (mut draw, mut out) = (Some(draw), None);
    let mut output = ctx.run_ui(input, |ui| {
        if let Some(draw) = draw.take() {
            out = Some(draw(ui));
        }
    });
    output.textures_delta.clear();
    out.expect("the frame ran")
}

/// Rows placed one after another are one group across the whole column; a gap starts another.
#[test]
fn rows_that_follow_each_other_are_one_group_across_the_column() {
    let (a, b, c, column_width) = bare_frame(1000.0, |ui| {
        let column_width = ui.available_width();
        let a = group_row(ui, |ui| { ui.label("a"); });
        let b = group_row(ui, |ui| { ui.label("b"); });
        ui.add_space(30.0);
        let c = group_row(ui, |ui| { ui.label("c"); });
        (a, b, c, column_width)
    });
    assert!((a.bottom() - b.top()).abs() < 0.5, "{a:?} then {b:?}");
    assert!((a.width() - column_width).abs() < 0.5, "{} of {column_width}", a.width());
    assert!(c.top() - b.bottom() >= 30.0, "{b:?} then {c:?}");
}

/// A column as wide as a page may ever want still ends with the page: its controls stay on screen.
#[test]
fn a_column_ends_with_the_page_and_stops_at_its_widest_on_a_wide_window() {
    for (window, most) in [(800.0, 800.0), (3000.0, SETTINGS_W)] {
        let right = bare_frame(window, |ui| {
            let mut right = 0.0;
            column(ui, SETTINGS_W, |ui| right = group_row(ui, |ui| { ui.label("a"); }).right());
            right
        });
        assert!(right <= most + 8.0 && right > most - 60.0, "at {window}: {right}");
    }
}

/// A group's heading is the section type as written, not a shouted label, and no group is a
/// catch-all "Fine-tuning".
#[test]
fn a_group_heading_is_written_as_it_reads() {
    // The Refresh rate group's rows show with Variable refresh rate off.
    let mut f = settings_with(Settings { vrr: false, ..Settings::default() });
    f.l.settings_view.section = "Display & image";
    open_folds(&mut f);
    assert!(shows(&f, "Refresh rate") && shows(&f, "PC display"), "{:?}", f.drawn);
    assert!(!shows(&f, "REFRESH RATE") && !shows(&f, "Fine-tuning"), "{:?}", f.drawn);
}

/// The switch and the list choice take the sizes the design gives every control.
#[test]
fn a_switch_and_a_list_choice_have_the_designed_size() {
    let (switch, choice) = bare_frame(1000.0, |ui| {
        let switch = toggle(ui, &mut true).rect;
        let choice = Combo::from_id_salt("choice").selected_text("Automatic").show_ui(ui, |_| {}).response.rect;
        (switch, choice)
    });
    assert_eq!(switch.size(), vec2(design_tokens::SIZE_SWITCH_W, design_tokens::SIZE_SWITCH_H));
    assert_eq!(choice.height(), 40.0);
}

/// The Graphics card row is drawn only on a Windows PC with two GPUs.
fn graphics_card_here() -> bool {
    #[cfg(windows)]
    {
        (crate::platform_hooks::hooks().gpu_pair)().is_some()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Presets sit above the main settings' rows: the defaults read Native, a chip asks before it
/// replaces the stream settings and sets them on OK, and a changed value reads Custom. A PC's
/// settings and a search have none.
#[test]
fn presets_ask_first_set_their_values_and_read_custom_once_changed() {
    let mut f = settings_with(Settings::default());
    assert!(shows(&f, "Presets") && shows(&f, "Native (recommended)") && shows(&f, "Battery saver"), "{:?}", f.drawn);
    assert!(shows(&f, "Native"), "the defaults are Native: {:?}", f.drawn);
    click_text(&mut f, "Battery saver");
    assert!(matches!(f.l.dialog, Dialog::ConfirmPreset(Preset::BatterySaver)));
    assert!(shows(&f, "This replaces resolution, frame rate, bitrate, frame pacing, upscaling, VRR and performance settings with this preset’s values. Codec and HDR choices stay as they are."));
    click_text(&mut f, "Cancel");
    assert!(matches!(f.l.dialog, Dialog::None) && f.l.settings == Settings::default(), "Cancel changes nothing");
    f.l.settings.codec = CodecPreference::Hevc;
    f.l.settings.hdr = true;
    f.settle();
    click_text(&mut f, "Battery saver");
    let commands = click_text(&mut f, "OK");
    assert!(commands.iter().any(|c| matches!(c, Command::SaveSettings)), "saved at once");
    let s = &f.l.settings;
    assert_eq!((s.width, s.height, s.fps, s.bitrate_kbps, s.frame_pacing), (0, 0, 30, 0, crate::config::FramePacing::CapFps));
    assert!(s.codec == CodecPreference::Hevc && s.hdr, "codec and HDR stay");
    f.settle();
    assert!(shows(&f, "Battery saver") && !shows(&f, "Custom"), "{:?}", f.drawn);
    f.l.settings.bitrate_kbps = 20_000;
    f.settle();
    assert!(shows(&f, "Custom"), "{:?}", f.drawn);
    f.l.settings_view.query = "bitrate".into();
    f.settle();
    assert!(!shows(&f, "Presets"), "a search shows rows only");
    f.l.settings_view.query.clear();
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    assert!(!shows(&f, "Presets"), "a PC's settings have none");
}

/// The presets sit only on the sections whose rows they change, and their chips are one size
/// whichever is chosen.
#[test]
fn presets_show_only_on_the_sections_they_change_with_chips_of_one_size() {
    let changed: Vec<&str> = SETTINGS_SECTIONS.iter().copied().filter(|s| preset_section(s)).collect();
    assert_eq!(changed, ["Video", "Display & image", "Debug"]);
    let mut f = settings_with(Settings::default());
    let height = |f: &Frames, label: &str| {
        f.placed.iter().find(|(t, ..)| t == label).map(|p| p.1.height()).unwrap_or_else(|| panic!("{label}: {:?}", f.drawn))
    };
    assert_eq!(height(&f, "Native (recommended)"), height(&f, "Battery saver"), "the chosen chip's type is the other's");
    for section in ["Controls", "Audio", "Streaming", "App", "Advanced"] {
        f.l.settings_view.section = SETTINGS_SECTIONS.iter().copied().find(|s| *s == section).unwrap();
        f.settle();
        assert!(!shows(&f, "Presets") && !shows(&f, "Battery saver"), "{section}: {:?}", f.drawn);
    }
    f.l.settings_view.section = SETTINGS_SECTIONS[1];
    f.settle();
    assert!(shows(&f, "Presets"), "{:?}", f.drawn);
}

/// Native puts back what the presets set, the desktop's defaults among them, and turns on every
/// latency switch the desktop has; Battery saver leaves those. VRR the screen can't use never
/// makes a preset Custom, as on Android.
#[test]
fn a_preset_sets_and_reads_the_desktops_settings() {
    let mut s = Settings {
        width: 1280, height: 720, fps: 144, bitrate_kbps: 50_000, vrr: false,
        upscaling: crate::config::Upscaling::Fsr1, frame_pacing: crate::config::FramePacing::Smoothness,
        drop_late_frames: false, unbatched_input: false, network_priority: false, host_frame_times: true,
        codec: CodecPreference::Av1, prefer_yuv444: true,
        ..Settings::default()
    };
    assert_eq!(presets::chosen(&preset_current(&s, true)), None);
    // The PC's frame times are no preset's pacing, whatever pacing is under them.
    let host_times = Settings { host_frame_times: true, ..Settings::default() };
    assert_eq!(presets::chosen(&preset_current(&host_times, true)), None);
    apply_preset(&mut s, Preset::Native);
    assert_eq!(presets::chosen(&preset_current(&s, true)), Some(Preset::Native));
    assert_eq!(s, Settings { codec: CodecPreference::Av1, prefer_yuv444: true, ..Settings::default() }, "the rest are the defaults");
    assert_eq!(presets::chosen(&preset_current(&Settings::default(), true)), Some(Preset::Native), "the defaults are Native");
    s.unbatched_input = false;
    apply_preset(&mut s, Preset::BatterySaver);
    assert_eq!(presets::chosen(&preset_current(&s, true)), Some(Preset::BatterySaver));
    assert!(!s.unbatched_input, "Battery saver leaves the latency switches");
    s.vrr = false;
    assert_eq!(presets::chosen(&preset_current(&s, true)), None, "VRR off where it is offered is Custom");
    assert_eq!(presets::chosen(&preset_current(&s, false)), Some(Preset::BatterySaver), "VRR the screen lacks reads as its default");
    assert_eq!(preset_value(Preset::Native), "Native");
    assert_eq!(preset_value(Preset::BatterySaver), "Battery saver");
}

/// Frame pacing is one row over its three keys, as on Android: Balanced's newest frame and the
/// PC's frame times are among its choices. Every saved combination reads as the pacing a stream
/// runs, and saving that choice back changes nothing a stream uses.
#[test]
fn frame_pacing_reads_and_writes_its_three_keys_as_one_choice() {
    use crate::config::FramePacing;
    for pacing in FramePacing::ALL {
        for drop_late_frames in [true, false] {
            for host_frame_times in [true, false] {
                let s = Settings { frame_pacing: pacing, drop_late_frames, host_frame_times, ..Settings::default() };
                let choice = frame_pacing_choice(&s);
                let expected = match (host_frame_times, pacing, drop_late_frames) {
                    (true, ..) => FramePacingChoice::HostTimes,
                    (_, FramePacing::Latency, _) => FramePacingChoice::Latency,
                    (_, FramePacing::Balanced, true) => FramePacingChoice::Balanced,
                    (_, FramePacing::Balanced, false) => FramePacingChoice::BalancedEvery,
                    (_, FramePacing::CapFps, _) => FramePacingChoice::CapFps,
                    (_, FramePacing::Smoothness, _) => FramePacingChoice::Smoothness,
                };
                assert_eq!(choice, expected, "{pacing:?} {drop_late_frames} {host_frame_times}");
                let mut saved = s.clone();
                set_frame_pacing_choice(&mut saved, choice);
                assert_eq!(saved, s, "saving {choice:?} back changes nothing");
                let paced = |s: &Settings| (crate::stream_types::without_vrr(s), s.host_frame_times);
                assert_eq!(paced(&saved), paced(&s));
            }
        }
    }
    for choice in FramePacingChoice::ALL {
        let mut s = Settings { frame_pacing: FramePacing::Smoothness, host_frame_times: true, ..Settings::default() };
        set_frame_pacing_choice(&mut s, choice);
        assert_eq!(frame_pacing_choice(&s), choice);
    }
    // The PC's frame times leave the pacing under them; another pacing leaves Balanced's choice.
    let mut s = Settings { frame_pacing: FramePacing::Balanced, drop_late_frames: false, ..Settings::default() };
    set_frame_pacing_choice(&mut s, FramePacingChoice::HostTimes);
    assert_eq!((s.frame_pacing, s.drop_late_frames), (FramePacing::Balanced, false));
    set_frame_pacing_choice(&mut s, FramePacingChoice::Smoothness);
    assert!(!s.host_frame_times && !s.drop_late_frames);
    // Its value line names the choice, under each of its keys.
    let host = Settings { host_frame_times: true, ..Settings::default() };
    for key in ["frame_pacing", "drop_late_frames", "host_frame_times"] {
        assert_eq!(value_text(key, &host, &crate::stream_types::Display { width: 1920, height: 1080, refresh_hz: 120.0, refresh_reported: true }, crate::glyphs::Style::Xbox), "PC’s frame times");
    }
}

/// The row shows while VRR is off, with no row of its own for Balanced's newest frame; V-Sync, one
/// row with its even cadence, stays beside a pacing that waits for the refresh and the PC's frame times.
#[test]
fn frame_pacing_is_one_row_and_the_pcs_frame_times_wait_for_the_refresh() {
    for (vrr, host_frame_times, pacing, frame_pacing_shown, vsync_shown) in [
        (false, false, crate::config::FramePacing::Latency, true, true),
        (false, false, crate::config::FramePacing::Balanced, true, true),
        (false, true, crate::config::FramePacing::Latency, true, true),
        (true, true, crate::config::FramePacing::Latency, false, true),
    ] {
        let mut f = settings_with(Settings { vrr, host_frame_times, frame_pacing: pacing, ..Settings::default() });
        f.l.tearing_supported = true;
        open_folds(&mut f);
        f.l.settings_view.section = "Display & image";
        f.settle();
        let what = format!("VRR {vrr} host {host_frame_times} {pacing:?}: {:?}", f.drawn);
        assert_eq!(shows(&f, "Frame pacing"), frame_pacing_shown, "{what}");
        assert_eq!(shows(&f, "V-Sync"), vsync_shown, "{what}");
        assert!(!shows(&f, "Keep only the newest frame"), "{what}");
        if frame_pacing_shown {
            assert!(setting_keys(&f).contains(&"frame_pacing"), "{what}");
            assert!(!setting_keys(&f).contains(&"drop_late_frames"), "a key of the Frame pacing row, not a row: {what}");
        }
    }
}

/// Noise suppression and Automatic gain follow the microphone, after Echo cancellation.
#[test]
fn the_microphones_noise_suppression_and_gain_follow_it() {
    let mut f = settings_with(Settings::default());
    open_folds(&mut f);
    f.l.settings_view.section = "Audio";
    f.settle();
    assert!(!shows(&f, "Noise suppression") && !shows(&f, "Automatic gain"), "{:?}", f.drawn);
    f.l.settings.microphone = true;
    f.settle();
    let keys = setting_keys(&f);
    let at = keys.iter().position(|k| *k == "mic_echo_cancel").unwrap();
    assert_eq!(&keys[at + 1..at + 3], ["mic_noise_suppression", "mic_auto_gain"], "{keys:?}");
    assert!(Settings::default().mic_noise_suppression && !Settings::default().mic_auto_gain, "Android's defaults");
}

/// App: Theme first; Check for updates now where this copy checks; Use small box art is the
/// card size; About opens its page. Advanced: the decode time, the latency benchmark and the
/// latency CSV, on the main settings only.
#[test]
fn the_app_and_diagnostics_rows_from_android_do_what_they_say() {
    let mut f = settings_read_with(Settings::default());
    open_folds(&mut f);
    f.l.settings_view.section = "App";
    f.settle();
    assert_eq!(setting_keys(&f).first(), Some(&"theme"), "Theme comes first");
    assert_eq!(row_words(&f, "Theme"), ["Dark"]);
    assert_eq!(shows(&f, "Check for updates now"), (crate::platform_hooks::hooks().self_update_checks)());
    if (crate::platform_hooks::hooks().self_update_checks)() {
        let commands = click_text(&mut f, "Check now");
        assert!(matches!(commands[..], [Command::CheckUpdatesNow]), "{} commands", commands.len());
    }
    assert_eq!(f.l.settings.home_card_size, crate::config::CardSize::Medium);
    let small_box_art = |f: &Frames| {
        f.access.as_ref().expect("AccessKit is on").nodes.iter()
            .find(|(_, n)| n.label() == Some("Use small box art") && n.toggled().is_some())
            .map(|(_, n)| (n.toggled(), n.bounds().expect("laid out")))
            .expect("the Use small box art switch")
    };
    let (toggled, bounds) = small_box_art(&f);
    assert_eq!(toggled, Some(egui::accesskit::Toggled::False));
    let at = Pos2::new(((bounds.x0 + bounds.x1) / 2.0) as f32, ((bounds.y0 + bounds.y1) / 2.0) as f32);
    f.click_at(at, egui::PointerButton::Primary);
    assert_eq!(f.l.settings.home_card_size, crate::config::CardSize::Small, "on is Home's Small cards");
    f.click_at(at, egui::PointerButton::Primary);
    assert_eq!(f.l.settings.home_card_size, crate::config::CardSize::Medium, "off is Medium");
    f.l.settings.home_card_size = crate::config::CardSize::Large;
    f.settle();
    assert_eq!(small_box_art(&f).0, Some(egui::accesskit::Toggled::False), "Large is not small");
    f.l.settings.home_card_size = crate::config::CardSize::Medium;
    f.settle();
    let about = f.placed.iter().find(|(t, ..)| t == "About").expect("the About row").1;
    let open = f.placed.iter().find(|(t, r, _)| t == "Open" && (r.center().y - about.center().y).abs() < 24.0).expect("its button").1.center();
    f.click_at(open, egui::PointerButton::Primary);
    f.settle();
    assert_eq!(f.l.screen, Screen::About);
    assert!(shows(&f, &format!("Version {} · Based on Moonlight (GPL-3.0)", crate::VERSION)) && shows(&f, "Source and licenses"), "{:?}", f.drawn);
    assert!(shows(&f, "Moonlight Android") && shows(&f, "Cameron Gutman, Diego Waxemberg, Aaron Neyer, Andrew Hennessy and contributors · GPL-3.0"), "{:?}", f.drawn);
    let commands = click_text(&mut f, "Source and licenses");
    assert!(matches!(commands[..], [Command::OpenSource]), "{} commands", commands.len());
    f.l.back(&mut Vec::new(), false);
    f.settle();
    assert_eq!(f.l.screen, Screen::Settings);

    f.l.settings_view.section = "Debug";
    f.settle();
    assert!(setting_keys(&f).contains(&"post_stream_decode_time"), "{:?}", setting_keys(&f));
    let commands = click_text(&mut f, "Export");
    assert!(matches!(commands[..], [Command::ExportLatencyCsv]), "{} commands", commands.len());
    let bench = f.placed.iter().find(|(t, ..)| t == "Latency benchmark").expect("the row").1;
    let open = f.placed.iter().find(|(t, r, _)| t == "Open" && (r.center().y - bench.center().y).abs() < 24.0).expect("its button").1.center();
    f.click_at(open, egui::PointerButton::Primary);
    f.settle();
    assert_eq!(f.l.screen, Screen::LatencyBenchmark);

    let mut f = settings_with(Settings::default());
    open_folds(&mut f);
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.l.settings_view.section = "Debug";
    f.settle();
    assert!(!shows(&f, "Show decode time after streaming") && !shows(&f, "Latency benchmark") && !shows(&f, "Export latency CSV"), "{:?}", f.drawn);
}

/// A game's own settings offer the PC's image enhancements as the PC's own settings do, and
/// only when its host reports them (NvidiaEnhancements).
#[test]
fn a_games_settings_offer_the_rtx_enhancements_its_host_reports() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    let id = f.hosts.list[0].id;
    let at = SECTION_KEYS.iter().position(|keys| keys.iter().any(|k| k[0] == "rtx_hdr")).unwrap();
    let rows = |f: &mut Frames, page: Screen| {
        f.l.go(Screen::Apps(id));
        if page == Screen::HostSettings(id) { f.l.open_host_settings(id, &f.hosts.list[0]) } else { f.l.open_app_settings(id, &f.hosts.list[0], game(7)) }
        f.l.settings_view.section = SETTINGS_SECTIONS[at];
        open_folds(f);
        f.settle();
        let keys = setting_keys(f);
        (keys.contains(&"rtx_hdr"), keys.contains(&"rtx_video"))
    };
    let mut info = answer(true, 0);
    info.capabilities = nightjar_policy::http::HostCapabilities::from_evidence(nightjar_policy::http::Evidence { nvidia_enhancements: 0, ..Default::default() });
    f.hosts.list[0].info = Some(info.clone());
    assert_eq!(rows(&mut f, Screen::AppSettings(id)), (false, false), "a host without them offers neither");
    info.capabilities = nightjar_policy::http::HostCapabilities::from_evidence(nightjar_policy::http::Evidence { nvidia_enhancements: rubylight_protocol::nvidia_enhance::ALL, ..Default::default() });
    f.hosts.list[0].info = Some(info);
    for page in [Screen::HostSettings(id), Screen::AppSettings(id)] {
        assert_eq!(rows(&mut f, page), (true, true), "{page:?} offers RTX HDR and RTX Video");
    }
}

/// V-Sync's even cadence says why where it holds nothing (client/AGENTS.md): VRR, the PC's
/// frame times, and a desktop that tells no time a picture was shown.
#[test]
fn the_even_cadence_says_why_where_it_holds_nothing() {
    let mut f = settings_with(Settings { vsync: true, vrr: false, ..Settings::default() });
    f.l.tearing_supported = true;
    open_folds(&mut f);
    in_section(&mut f, "Display & image");
    assert!(shows(&f, "V-Sync") && !shows(&f, "Fractional V-Sync"), "{:?}", f.drawn);
    let desktop_note = !fractional_vsync_applies(&video_driver());
    assert_eq!(shows(&f, "Not used: this desktop doesn’t report when pictures are shown"), desktop_note, "{:?}", f.drawn);
    f.l.settings.host_frame_times = true;
    f.settle();
    assert!(shows(&f, "V-Sync") && shows(&f, "Not used with PC’s frame times"), "{:?}", f.drawn);
    f.l.settings.host_frame_times = false;
    f.l.settings.vrr = true;
    f.settle();
    assert!(shows(&f, "V-Sync") && shows(&f, "Not used with Variable refresh rate"), "{:?}", f.drawn);
    f.l.settings.vrr = false;
    f.l.settings.vsync = false;
    f.settle();
    assert!(shows(&f, "V-Sync") && !shows(&f, "Not used: this desktop doesn’t report when pictures are shown"), "V-Sync off: no cadence note: {:?}", f.drawn);
}

/// One list over the two stored fields: Off, On, On with an even cadence.
#[test]
fn v_sync_is_one_choice_over_the_two_stored_fields() {
    let mut s = Settings { vsync: true, fractional_vsync: false, ..Settings::default() };
    assert_eq!(vsync_choice(&s), VsyncChoice::On);
    set_vsync_choice(&mut s, VsyncChoice::EvenCadence);
    assert!(s.vsync && s.fractional_vsync);
    assert_eq!(vsync_choice(&s), VsyncChoice::EvenCadence);
    set_vsync_choice(&mut s, VsyncChoice::Off);
    assert!(!s.vsync && s.fractional_vsync, "Off keeps the cadence field");
    assert_eq!(vsync_choice(&s), VsyncChoice::Off, "V-Sync off wins over a stored cadence");
    set_vsync_choice(&mut s, VsyncChoice::On);
    assert!(s.vsync && !s.fractional_vsync);
    // Stored files read as before: both fields keep their names and meaning.
    let old: Settings = serde_json::from_str(r#"{"vsync":true,"fractional_vsync":true}"#).unwrap();
    assert_eq!(vsync_choice(&old), VsyncChoice::EvenCadence);
    let off: Settings = serde_json::from_str(r#"{"vsync":false,"fractional_vsync":true}"#).unwrap();
    assert_eq!(vsync_choice(&off), VsyncChoice::Off);
    assert_eq!(VSYNC_CHOICES.map(vsync_name), ["Off", "On", "On, even cadence"]);
}

/// The display's refresh has to be 2 to 4 times the frame rate, and Windows' Fullscreen without VRR
/// switches the display to the stream's rate where it has a mode at it.
#[test]
fn fractional_vsync_names_the_refresh_where_it_holds_nothing() {
    let at = |hz: f32| crate::stream_types::Display { width: 1920, height: 1080, refresh_hz: hz, refresh_reported: true };
    let s = |fps: u32, window_mode: WindowMode| Settings { fps, window_mode, ..Settings::default() };
    let borderless = WindowMode::Borderless;
    assert_eq!(fractional_vsync_display_reason(&s(60, borderless), &at(120.0), &[], true, false), None, "twice");
    assert_eq!(fractional_vsync_display_reason(&s(30, borderless), &at(119.88), &[], true, false), None, "four times, within the tolerance");
    assert_eq!(fractional_vsync_display_reason(&s(60, borderless), &at(144.0), &[], true, false).as_deref(),
        Some("Not used at this display’s 144 Hz: that isn’t 2 to 4 times the frame rate"));
    assert!(fractional_vsync_display_reason(&s(60, borderless), &at(60.0), &[], true, false).is_some(), "the stream's own rate");
    assert!(fractional_vsync_display_reason(&s(0, borderless), &at(120.0), &[], true, false).is_some(), "Display: the display's rate");
    assert!(fractional_vsync_display_reason(&s(24, borderless), &at(120.0), &[], true, false).is_some(), "five times is past the hold's 4");
    let unreported = crate::stream_types::Display { refresh_reported: false, ..at(144.0) };
    assert_eq!(fractional_vsync_display_reason(&s(60, borderless), &unreported, &[], true, false), None, "no refresh known");

    // Windows' Fullscreen takes the mode closest to the stream's size and rate (SDL's order).
    let full = WindowMode::Fullscreen;
    let modes = |w: i32, h: i32, rates: &[f32]| rates.iter().map(|&refresh_hz| FullscreenMode { w, h, refresh_hz, bytes_per_pixel: 4 }).collect::<Vec<_>>();
    let fhd = |rates: &[f32]| modes(1920, 1080, rates);
    let reason = |fps, desktop: f32, list: &[FullscreenMode], windows, kms| {
        fractional_vsync_display_reason(&s(fps, full), &at(desktop), list, windows, kms)
    };
    assert_eq!(reason(60, 120.0, &fhd(&[120.0, 60.0]), true, false).as_deref(),
        Some("Not used at the 60 Hz Fullscreen switches to: that isn’t 2 to 4 times the frame rate"));
    assert_eq!(reason(30, 120.0, &fhd(&[120.0, 60.0]), true, false), None, "no 30 Hz mode: the closest, 60 Hz, holds 2");
    assert_eq!(reason(30, 144.0, &fhd(&[144.0, 120.0, 60.0]), true, false), None, "60 Hz, the closest to 30, holds 2");
    assert_eq!(reason(40, 120.0, &fhd(&[120.0, 60.0, 50.0]), true, false).as_deref(),
        Some("Not used at the 50 Hz Fullscreen switches to: that isn’t 2 to 4 times the frame rate"), "50 Hz, the closest to 40");
    assert_eq!(reason(50, 144.0, &fhd(&[60.0, 40.0]), true, false).as_deref(),
        Some("Not used at the 40 Hz Fullscreen switches to: that isn’t 2 to 4 times the frame rate"), "a tie goes to the later, slower mode");
    assert_eq!(reason(60, 120.0, &fhd(&[120.0]), true, false), None, "the closest is the desktop's own 120 Hz, which holds 2");
    // The size comes first: the smallest that holds the stream, its rates alone.
    let sizes = [modes(2560, 1440, &[144.0, 60.0, 30.0]), fhd(&[60.0])].concat();
    assert_eq!(reason(30, 144.0, &sizes, true, false), None, "1080p's 60 Hz, not 1440p's 30 Hz");
    // No mode holds the stream: SDL keeps the desktop's mode.
    let small = modes(1280, 720, &[60.0]);
    assert_eq!(reason(60, 144.0, &small, true, false).as_deref(), Some("Not used at this display’s 144 Hz: that isn’t 2 to 4 times the frame rate"));
    assert_eq!(reason(60, 120.0, &small, true, false), None);
    assert_eq!(reason(60, 120.0, &fhd(&[120.0, 60.0]), false, false), None, "Wayland's Fullscreen keeps the display's refresh");
    // KMS's planes switch to a mode of the connector's, which the settings can't list.
    let borderless_kms = fractional_vsync_display_reason(&s(60, borderless), &at(144.0), &fhd(&[144.0, 120.0]), false, true);
    assert_eq!(borderless_kms, None, "KMS: 144 Hz now, but the planes may switch to 120 Hz and hold");
}

/// SDL's own choice (`SDL_GetClosestFullscreenDisplayMode`), mode for mode.
#[test]
fn the_closest_fullscreen_mode_is_sdls() {
    let m = |w, h, refresh_hz, bytes_per_pixel| FullscreenMode { w, h, refresh_hz, bytes_per_pixel };
    let list = [m(3840, 2160, 60.0, 4), m(2560, 1440, 144.0, 4), m(2560, 1440, 60.0, 4), m(1920, 1080, 120.0, 4), m(1920, 1080, 60.0, 4), m(1920, 1080, 60.0, 2)];
    assert_eq!(closest_fullscreen_mode(&list, 1920, 1080, 60.0, 144.0), Some(list[4]), "the deeper format of two at 60 Hz");
    assert_eq!(closest_fullscreen_mode(&list, 1920, 1080, 90.0, 144.0), Some(list[4]), "equally close: the later");
    assert_eq!(closest_fullscreen_mode(&list, 1920, 1080, 0.0, 144.0), Some(list[3]), "0 asks for the desktop's rate");
    assert_eq!(closest_fullscreen_mode(&list, 2000, 1100, 60.0, 144.0), Some(list[2]), "the smallest size that holds it");
    assert_eq!(closest_fullscreen_mode(&list, 1920, 1200, 60.0, 144.0), Some(list[2]), "taller than 1080: 1440p");
    assert_eq!(closest_fullscreen_mode(&list, 5120, 2880, 60.0, 144.0), None, "none holds it");
    // A wider ratio's mode that holds the stream gives way to one closer to the stream's.
    let ratios = [m(3440, 1440, 100.0, 4), m(2560, 1440, 144.0, 4)];
    assert_eq!(closest_fullscreen_mode(&ratios, 2560, 1440, 60.0, 144.0), Some(ratios[1]));
}

#[test]
fn fractional_vsync_is_offered_only_where_it_holds_pictures() {
    for driver in ["windows", "wayland", "kmsdrm", "x11", "cocoa"] {
        let expected = cfg!(windows) || (cfg!(target_os = "linux") && driver != "x11");
        assert_eq!(fractional_vsync_applies(driver), expected, "{driver}");
    }
    if cfg!(target_os = "linux") {
        assert!(!fractional_vsync_applies("x11"), "X11 tells no time a picture was shown");
        assert!(fractional_vsync_applies("wayland"));
    }
}

#[test]
fn display_arrangement_shows_every_saved_pair_as_the_choice_that_streams_the_same() {
    use crate::config::HostDisplay;
    let sent = |(mode, virtual_display): (HostDisplay, bool)| nightjar_policy::http::HostDisplay::from(mode).requests_virtual_display(virtual_display);
    for mode in [HostDisplay::HostDefault, HostDisplay::Exclusive, HostDisplay::ExtendedPrimary, HostDisplay::Extended,
        HostDisplay::ExtendedIsolated, HostDisplay::ExtendedPrimaryIsolated, HostDisplay::Physical] {
        for virtual_display in [false, true] {
            let s = Settings { host_display: mode, virtual_display, ..Settings::default() };
            // With display modes the arrangement is sent, and the switch only counts with the PC's own setting.
            let shown = display_choice(&s, true);
            assert!(DISPLAY_CHOICES.contains(&shown), "{mode:?}/{virtual_display}");
            assert_eq!(shown.0, mode, "{mode:?}/{virtual_display}");
            assert_eq!(sent(shown), sent((mode, virtual_display)), "{mode:?}/{virtual_display}");
            // Without display modes only the switch is sent.
            assert_eq!(display_choice(&s, false), (HostDisplay::HostDefault, virtual_display));
        }
    }
    // Each choice reads back as itself; only the PC's own setting and Virtual display set the switch.
    for before in [false, true] {
        for choice in DISPLAY_CHOICES {
            let mut s = Settings { virtual_display: before, ..Settings::default() };
            pick_display_choice(&mut s, choice);
            assert_eq!(display_choice(&s, true), choice);
            assert_eq!(s.virtual_display, if choice.0 == HostDisplay::HostDefault { choice.1 } else { before });
        }
    }
    assert_eq!(display_choice_name((HostDisplay::HostDefault, true)), "Virtual display");
    assert_eq!(display_choice_name((HostDisplay::HostDefault, false)), "The PC’s own setting");
}

#[test]
fn display_arrangement_holds_virtual_display_and_a_pick_writes_both_keys() {
    use crate::config::HostDisplay;
    let row = "Display arrangement";
    // A pair the old rows could leave: shown as it streams, and left as it is until a pick.
    let saved = Settings { host_display: HostDisplay::Physical, virtual_display: true, ..Settings::default() };
    let mut f = settings_with(saved.clone());
    f.l.settings_view.section = "Display & image";
    f.l.settings_view.more = [true; 9];
    f.settle();
    assert_eq!(row_words(&f, row), ["Physical display", "Reset"], "Reset puts both keys back");
    assert!(!shows(&f, "PC render resolution"), "Physical display has no virtual display: {:?}", f.drawn);
    assert_eq!(f.l.settings, saved, "drawing it changes nothing");
    click_value(&mut f, row, "Physical display");
    for choice in DISPLAY_CHOICES {
        assert!(shows(&f, display_choice_name(choice)), "{choice:?} is offered: {:?}", f.drawn);
    }
    click_last(&mut f, "Virtual display");
    f.settle();
    assert_eq!((f.l.settings.host_display, f.l.settings.virtual_display), (HostDisplay::HostDefault, true));
    assert!(shows(&f, "PC render resolution"));
    click_value(&mut f, row, "Virtual display");
    click_last(&mut f, "Virtual display only");
    f.settle();
    assert_eq!((f.l.settings.host_display, f.l.settings.virtual_display), (HostDisplay::Exclusive, true));
    click_value(&mut f, row, "Virtual display only");
    click_last(&mut f, "The PC’s own setting");
    f.settle();
    assert_eq!((f.l.settings.host_display, f.l.settings.virtual_display), (HostDisplay::HostDefault, false));
    assert!(!shows(&f, "Virtual display"), "no row of its own: {:?}", f.drawn);
}

#[test]
fn a_pc_without_display_modes_offers_only_the_virtual_display_switch() {
    use crate::config::HostDisplay;
    let xml = "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\"><hostname>PC</hostname><uniqueid>uid</uniqueid><HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort><PairStatus>1</PairStatus><currentgame>0</currentgame><state>SUNSHINE_SERVER_FREE</state><Permission>1</Permission></root>";
    let info = nightjar_policy::http::ServerInfo::parse(xml, 47989).expect("the answer parses");
    // Settings ask for Exclusive, which this PC never receives: its page shows what it streams.
    let mut f = Frames::with(record("PC", true, false), Reach::Online, Some(info), Vec::new());
    f.size = vec2(1280.0, 12000.0);
    f.l.settings = Settings { host_display: HostDisplay::Exclusive, virtual_display: false, ..Settings::default() };
    f.l.settings_view.section = "Display & image";
    f.l.settings_view.more = [true; 9];
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    let row = "Display arrangement";
    let note = "This PC doesn’t take display arrangements: it streams as shown here, not as Virtual display only";
    assert_eq!(row_words(&f, row), ["The PC’s own setting"]);
    assert!(shows(&f, note), "the row says why: {:?}", f.drawn);
    click_value(&mut f, row, "The PC’s own setting");
    assert!(shows(&f, "Virtual display") && !shows(&f, "Virtual display only"), "two choices: {:?}", f.drawn);
    click_last(&mut f, "Virtual display");
    f.settle();
    assert_eq!(row_words(&f, row), ["Virtual display"]);
    assert!(!shows(&f, note), "a choice made here clears it");
}

#[test]
fn a_layer_holding_one_key_of_a_row_keeps_it_alone_until_a_value_is_chosen() {
    use crate::config::HostDisplay;
    // As the separate Virtual display row saved it: the PC's own switch, its arrangement inherited.
    let mut pc = record("PC", true, false);
    pc.settings = Some(serde_json::from_str(r#"{"virtual_display":true}"#).unwrap());
    let mut f = Frames::new(pc, Reach::Online);
    f.size = vec2(1280.0, 12000.0);
    f.l.settings = Settings { host_display: HostDisplay::Exclusive, ..Settings::default() };
    f.l.settings_view.section = "Display & image";
    f.l.settings_view.more = [true; 9];
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    f.settle();
    let row = "Display arrangement";
    assert!(row_words(&f, row).contains(&"Virtual display only"), "it streams the inherited arrangement: {:?}", row_words(&f, row));
    assert!(!f.l.edit_overrides.has(&["host_display"]), "drawing the page adds nothing to the PC's layer");
    assert!(f.l.edit_overrides.has(&["virtual_display"]));
    // A pick writes both keys into the PC's layer.
    click_value(&mut f, row, "Virtual display only");
    click_last(&mut f, "Virtual display");
    f.settle();
    assert!(f.l.edit_overrides.has(&["host_display"]) && f.l.edit_overrides.has(&["virtual_display"]));
    assert_eq!((f.l.edit_settings.host_display, f.l.edit_settings.virtual_display), (HostDisplay::HostDefault, true));
}


#[test]
fn an_arrangement_picked_on_a_pcs_page_leaves_its_virtual_display_switch_to_the_settings() {
    use crate::config::HostDisplay;
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 12000.0);
    f.l.settings_view.section = "Display & image";
    f.l.settings_view.more = [true; 9];
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    let row = "Display arrangement";
    // An arrangement: only host_display goes into the PC's settings, so Remote Monitor there keeps
    // following the main settings' switch, as before the merge.
    click_value(&mut f, row, "The PC’s own setting");
    click_last(&mut f, "Extend");
    f.settle();
    assert_eq!(f.l.edit_settings.host_display, HostDisplay::Extended);
    assert!(f.l.edit_overrides.has(&["host_display"]));
    assert!(!f.l.edit_overrides.has(&["virtual_display"]), "the switch stays the main settings'");
    // Virtual display and the PC's own setting set the switch too, as on Android.
    click_value(&mut f, row, "Extend");
    click_last(&mut f, "Virtual display");
    f.settle();
    assert!(f.l.edit_overrides.has(&["host_display"]) && f.l.edit_overrides.has(&["virtual_display"]));
    assert_eq!((f.l.edit_settings.host_display, f.l.edit_settings.virtual_display), (HostDisplay::HostDefault, true));
}

#[test]
fn frames_waiting_for_the_decoder_show_their_choice() {
    let mut s = Settings::default();
    assert_eq!(decode_queue_choice(s.decode_queue_frames), 15);
    for (saved, shown) in [(0, 15), (1, 3), (3, 3), (4, 5), (8, 8), (9, 15), (40, 15)] {
        assert_eq!(decode_queue_choice(saved), shown, "{saved}");
    }
    assert!(DECODE_QUEUE_CHOICES.iter().all(|&c| c == decode_queue_choice(c)));
    let row = ANDROID_KEYS.iter().find(|(desktop, _)| *desktop == "decode_queue_frames");
    assert_eq!(row, Some(&("decode_queue_frames", "list_decode_queue_frames")));
    s.decode_queue_frames = 5;
    assert_eq!(value_text("decode_queue_frames", &s, &crate::stream_types::Display { width: 1920, height: 1080, refresh_hz: 60.0, refresh_reported: true }, crate::glyphs::Style::Xbox), "5 frames");
}

/// Whole frames under VRR shows where Variable refresh rate does, with its On: not where Windows
/// cannot present with tearing (the VRR row is hidden and streams use no VRR), nor with Fastest.
#[test]
fn whole_frames_under_vrr_shows_only_with_vrr_on_and_offered() {
    let drawn = |s: Settings, tearing: bool| {
        let mut f = settings_with(s);
        f.l.tearing_supported = tearing;
        open_folds(&mut f);
        f.l.settings_view.section = "Display & image";
        f.settle();
        setting_keys(&f).contains(&"vrr_wait")
    };
    assert_eq!(drawn(Settings::default(), true), cfg!(windows));
    assert!(!drawn(Settings::default(), false), "no tearing: no VRR on Windows, no scan-line wait elsewhere");
    assert!(!drawn(Settings { present_at_host_times: false, vrr_tear_free: false, ..Settings::default() }, true), "Fastest");
    assert!(!drawn(Settings { vrr: false, ..Settings::default() }, true), "VRR off");
}

#[test]
fn the_simple_layer_writes_and_reads_the_settings_it_stands_for() {
    let display = Display { width: 2560, height: 1440, refresh_hz: 120.0, refresh_reported: true };
    let caps = simple_caps(&display, false, true);
    let mut s = Settings::default();
    let recommended = simple::recommended(&caps);
    // Applying Auto after "Fastest" writes VRR's three keys together, so the Variable refresh rate
    // row reads On rather than Custom next to these rows.
    let mut from_fastest = Settings { present_at_host_times: false, vrr_tear_free: false, ..Settings::default() };
    simple_apply(&mut from_fastest, &display, &caps, recommended);
    assert_eq!((from_fastest.vrr, from_fastest.present_at_host_times, from_fastest.vrr_tear_free),
        (true, true, true), "Auto leaves the VRR row On, not Custom");
    simple_apply(&mut s, &display, &caps, recommended);
    assert_eq!((s.width, s.height, s.fps), (0, 0, 0), "the screen's own size and rate");
    assert_eq!(s.frame_pacing, FramePacing::Latency);
    assert!(s.vrr && s.present_at_host_times);
    assert_eq!(s.bitrate_kbps, 0, "the bitrate for the size and rate");
    assert!(!s.hdr);
    let (choices, custom) = simple_state(&s, &display, &caps, true);
    assert!(!custom);
    assert_eq!(choices, recommended);
    // One changed advanced setting reads as Custom.
    s.frame_pacing = FramePacing::CapFps;
    assert!(simple_state(&s, &display, &caps, true).1);
    // Save data lowers the bitrate and lets it fall when the link struggles, whatever "Never" was.
    s.lower_bitrate = LowerBitrate::Keep;
    simple_apply(&mut s, &display, &caps, simple::Choices { picture: simple::Picture::SaveData, ..recommended });
    assert!(s.lower_bitrate == LowerBitrate::WhenFramesLost && s.adaptive_quality == crate::config::QualityMode::Battery,
        "Save data is never overridden by an old Never");
    assert!(s.bitrate_kbps > 0 && s.bitrate_kbps < stream_mode(&Settings::default(), display).3);
    let (choices, custom) = simple_state(&s, &display, &caps, true);
    assert!(!custom && choices.picture == simple::Picture::SaveData);
    // A saved PyroWave codec does not make Best store PyroWave's own bitrate.
    let mut pyro = Settings { codec: CodecPreference::PyroWave, ..Settings::default() };
    simple_apply(&mut pyro, &display, &caps, simple::Choices { picture: simple::Picture::Best, ..recommended });
    assert_eq!(pyro.codec, CodecPreference::Auto);
    assert!(pyro.bitrate_kbps < 100_000, "Best uses the conventional codec's bitrate, not PyroWave's {}", pyro.bitrate_kbps);
    assert!(!simple_state(&pyro, &display, &caps, true).1);
}

#[test]
fn a_non_vrr_first_launch_reads_auto_not_custom() {
    let display = Display { width: 1920, height: 1080, refresh_hz: 60.0, refresh_reported: true };
    let caps = simple_caps(&display, false, false);
    let s = Settings::default();
    let (choices, custom) = simple_state(&s, &display, &caps, false);
    assert!(!custom, "a saved VRR the swap chain cannot tear is not Custom");
    assert_eq!(choices.smoothness, simple::Smoothness::Auto);
}

#[test]
fn the_hdr_row_value_reads_off_until_the_switch_is_on() {
    let display = Display { width: 1920, height: 1080, refresh_hz: 120.0, refresh_reported: true };
    let value = |s: &Settings| value_text("simple_hdr", s, &display, crate::glyphs::Style::Xbox);
    assert_eq!(value(&Settings::default()), "Off", "a fresh install streams SDR");
    assert_eq!(value(&Settings { hdr: true, ..Settings::default() }), "Auto (recommended)");
    assert_eq!(
        value(&Settings { simple_hdr: simple::Hdr::Auto, ..Settings::default() }),
        "Off",
        "the stored Auto does not make the row claim HDR while the switch is off"
    );
}

#[test]
fn a_fresh_hdr_screen_reads_hdr_off_like_the_stream() {    let display = Display { width: 1920, height: 1080, refresh_hz: 120.0, refresh_reported: true };
    let caps = simple_caps(&display, true, true);
    let s = Settings::default();
    assert!(!s.hdr, "the stream stays SDR: the HDR switch is at its app default");
    let (choices, custom) = simple_state(&s, &display, &caps, true);
    assert!(!custom, "the row shows what the stream does, not Custom");
    assert_eq!(choices.hdr, simple::Hdr::Off, "a fresh install streams SDR, so the row reads Off");
    // A pick in another row keeps HDR off (it writes the whole profile from the rows).
    let mut picked = s.clone();
    simple_apply(&mut picked, &display, &caps, simple::Choices { smoothness: simple::Smoothness::Balanced, ..choices });
    assert!(!picked.hdr, "a pick in another row must not turn HDR on");
    assert_eq!(simple_state(&picked, &display, &caps, true).0.hdr, simple::Hdr::Off);
    // The HDR row's own Auto turns it on, and a later pick elsewhere leaves it on.
    simple_apply(&mut picked, &display, &caps, simple::Choices { hdr: simple::Hdr::Auto, ..choices });
    assert!(picked.hdr);
    let (after, custom) = simple_state(&picked, &display, &caps, true);
    assert!(!custom);
    assert_eq!(after.hdr, simple::Hdr::Auto);
    simple_apply(&mut picked, &display, &caps, simple::Choices { picture: simple::Picture::Best, ..after });
    assert!(picked.hdr, "a pick in another row must not turn HDR off");
}

#[test]
fn a_custom_row_elsewhere_still_carries_the_streams_own_hdr() {
    // The rest of the settings match no profile, so the rows read Custom; their choices still carry
    // the stream's HDR state, so a pick in one row cannot flip it.
    let display = Display { width: 1920, height: 1080, refresh_hz: 120.0, refresh_reported: true };
    let caps = simple_caps(&display, true, true);
    let mut off = Settings { frame_pacing: FramePacing::CapFps, ..Settings::default() };
    let (choices, custom) = simple_state(&off, &display, &caps, true);
    assert!(custom);
    assert_eq!(choices.hdr, simple::Hdr::Off);
    simple_apply(&mut off, &display, &caps, simple::Choices { picture: simple::Picture::Best, ..choices });
    assert!(!off.hdr, "a pick in another row must not turn HDR on");
    let mut on = Settings { frame_pacing: FramePacing::CapFps, hdr: true, ..Settings::default() };
    let (choices, custom) = simple_state(&on, &display, &caps, true);
    assert!(custom);
    assert_eq!(choices.hdr, simple::Hdr::Auto);
    simple_apply(&mut on, &display, &caps, simple::Choices { picture: simple::Picture::Best, ..choices });
    assert!(on.hdr, "a pick in another row must not turn HDR off");
}

#[test]
fn a_hand_change_that_equals_another_profile_reads_as_custom() {
    let display = Display { width: 1920, height: 1080, refresh_hz: 120.0, refresh_reported: true };
    let caps = simple_caps(&display, false, true);
    let mut s = Settings::default();
    simple_apply(&mut s, &display, &caps, simple::recommended(&caps));
    // Auto writes the newest frame at Latency; Balanced's pacing now matches everything else Auto
    // writes, but the stored choice is still Auto, so the rows must not claim Balanced.
    s.frame_pacing = FramePacing::Balanced;
    assert!(simple_state(&s, &display, &caps, true).1, "a hand change equal to another profile is Custom");
}

#[test]
fn the_setup_section_shows_the_simple_layer_and_back_applies_it() {
    let mut f = settings_with(Settings::default());
    f.l.tearing_supported = true;
    f.l.settings_view.section = "Recommended";
    f.settle();
    for row in ["Stream smoothness", "Picture quality", "Screen", "HDR", "Console mode", "Back to recommended"] {
        assert!(shows(&f, row), "{row} is not drawn: {:?}", f.drawn);
    }
    assert!(!row_words(&f, "Stream smoothness").contains(&"Custom"), "{:?}", row_words(&f, "Stream smoothness"));
    f.l.settings.frame_pacing = FramePacing::CapFps;
    assert!(simple_state(&f.l.settings, &f.l.display, &simple_caps(&f.l.display, false, true), true).1);
    f.settle();
    assert!(row_words(&f, "Stream smoothness").contains(&"Custom"), "{:?}", row_words(&f, "Stream smoothness"));
    // Picking the stored choice while Custom applies the profile (it is what the row shows).
    let _ = click_value(&mut f, "Stream smoothness", "Custom");
    let _ = click_last(&mut f, "Auto (recommended)");
    f.settle();
    assert_eq!(FramePacing::Latency, f.l.settings.frame_pacing);
    f.l.settings.frame_pacing = FramePacing::CapFps;
    f.settle();
    let _ = click_text(&mut f, "Back to recommended");
    f.settle();
    assert_eq!(f.l.settings.frame_pacing, FramePacing::Latency);
    assert!(!row_words(&f, "Stream smoothness").contains(&"Custom"), "{:?}", row_words(&f, "Stream smoothness"));
}

/// How many rows the main settings show at their defaults with every fold open, per section listed
/// ("More settings" chosen): the number the settings-simplification work counts, 108 before rows
/// that can do nothing were left out. `--nocapture` prints them by section.
#[test]
fn row_counts_at_defaults() {
    let mut f = settings_with(Settings::default());
    f.size = vec2(1280.0, 20000.0);
    open_folds(&mut f);
    let listed: Vec<String> = rail_entries(&f).into_iter().map(str::to_owned).collect();
    let mut total = 0;
    for section in SETTINGS_SECTIONS {
        if !listed.iter().any(|l| l == section) {
            println!("{section:18} not listed");
            continue;
        }
        in_section(&mut f, section);
        let rows = f.l.settings_view.drawn.len();
        total += rows;
        println!("{section:18} {rows:3} {:?}", f.l.settings_view.drawn.iter().map(|(id, _)| *id).collect::<Vec<_>>());
    }
    println!("total {total}");
    assert_eq!(total, ROWS_AT_DEFAULTS, "rows shown at the defaults");
}

/// The main settings' rows at their defaults, every fold open, the new "Show debug settings" included.
/// Each platform lists only the rows its build can use, so the number is its own: macOS has no
/// "Group incoming video packets" and none of Linux' renderer, display output, video planes and
/// GPU power rows; Windows has rows of its own in their place.
const ROWS_AT_DEFAULTS: usize = if cfg!(windows) {
    98
} else if cfg!(target_os = "macos") {
    95
} else {
    100
};

/// Debug is listed only with "Show debug settings", while one of its settings is changed, while it
/// is open or while a search runs; its rows keep their values and search finds them.
#[test]
fn debug_is_listed_on_request_or_while_a_debug_setting_is_changed() {
    let mut f = settings_with(Settings::default());
    assert!(!rail_entries(&f).contains(&"Debug"), "not by default: {:?}", rail_entries(&f));
    f.l.settings.show_debug_settings = true;
    f.settle();
    assert_eq!(rail_entries(&f).last(), Some(&"Debug"), "with the switch, last");
    f.l.settings.show_debug_settings = false;
    f.l.settings.link_probe = true;
    f.settle();
    assert!(rail_entries(&f).contains(&"Debug"), "a changed Debug setting keeps it in sight");
    f.l.settings.link_probe = false;
    f.settle();
    assert!(!rail_entries(&f).contains(&"Debug"));
    // A search looks everywhere, Debug included.
    f.l.settings_view.query = "error correction".into();
    f.settle();
    assert!(shows(&f, "Error correction") && rail_entries(&f).contains(&"Debug"), "{:?}", f.drawn);
    // A link that leads into it keeps it listed while it shows.
    f.l.settings_view.query.clear();
    f.l.settings_view.section = "Debug";
    f.settle();
    assert!(rail_entries(&f).contains(&"Debug") && shows(&f, "Error correction"));
}

/// A PC's settings list Debug by the same rule, and while the PC sets one of its settings.
#[test]
fn a_pcs_settings_list_debug_by_the_same_rule() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    assert!(!rail_entries(&f).contains(&"Debug"), "{:?}", rail_entries(&f));
    f.l.edit_overrides = serde_json::from_str(r#"{"link_probe":true}"#).unwrap();
    f.settle();
    assert!(rail_entries(&f).contains(&"Debug"), "the PC sets one: {:?}", rail_entries(&f));
}

/// "Show debug settings" is an App row behind its fold, off by default, and its value is kept in
/// settings.json.
#[test]
fn show_debug_settings_is_an_app_row_and_its_value_is_kept() {
    assert!(!Settings::default().show_debug_settings);
    assert!(!main_row("show_debug_settings"), "behind App's fold");
    let saved = serde_json::to_string(&Settings { show_debug_settings: true, ..Settings::default() }).unwrap();
    let read: Settings = serde_json::from_str(&saved).unwrap();
    assert!(read.show_debug_settings, "read back");
    let older: Settings = serde_json::from_str("{}").unwrap();
    assert!(!older.show_debug_settings, "off for settings saved before it");
    let mut f = settings_with(Settings::default());
    open_folds(&mut f);
    in_section(&mut f, "App");
    assert!(shows(&f, "Show debug settings"), "{:?}", f.drawn);
}

/// The rows a PC's, game's or profile's settings or the stream menu can make act are never left
/// out: while the main settings leave them nothing to do they say why, and they lose the note once
/// their mode comes on.
#[test]
fn rows_another_layer_can_make_act_stay_with_their_reason() {
    type Change = fn(&mut Settings);
    let rows: [(&str, &str, &str, Change); 5] = [
        ("Video", "Minimum resolution", "Not used without Automatic quality", |s| s.adaptive_quality = crate::config::QualityMode::Motion),
        ("Video", "Minimum frame rate", "Not used without Automatic quality", |s| s.adaptive_quality = crate::config::QualityMode::Motion),
        ("Display & image", "Upscaling sharpness", "Not used with Bilinear", |s| s.upscaling = crate::config::Upscaling::Fsr1),
        ("Streaming", "Overlay position", "Not used without Performance overlay", |s| s.overlay = true),
        ("Streaming", "Second screen resolution", "Not used without Dual screen mode", |s| s.dual_screen = true),
    ];
    for (section, label, reason, on) in rows {
        let base = Settings { upscaling: crate::config::Upscaling::Bilinear, disable_rumble: true, ..Settings::default() };
        let mut f = settings_with(base);
        open_folds(&mut f);
        in_section(&mut f, section);
        assert!(shows(&f, label) && shows(&f, reason), "{label} stays, saying why: {:?}", f.drawn);
        on(&mut f.l.settings);
        f.settle();
        assert!(shows(&f, label) && !shows(&f, reason), "{label} acts now: {:?}", f.drawn);
    }
}

/// The IGDB client secret does nothing until the client ID is set: it is left out until then, comes
/// back on the next frame, stays with a secret of its own saying why, and search finds it.
#[test]
fn the_igdb_secret_is_left_out_until_the_client_id_is_set() {
    let (label, reason) = ("IGDB client secret", "Not used without IGDB client ID");
    let mut f = settings_with(Settings::default());
    open_folds(&mut f);
    in_section(&mut f, "Advanced");
    assert!(!shows(&f, label), "{:?}", f.drawn);
    f.l.game_info_keys.igdb_client_id = "id".into();
    f.settle();
    assert!(shows(&f, label), "{:?}", f.drawn);
    let mut f = settings_with(Settings::default());
    f.l.game_info_keys.igdb_client_secret = "secret".into();
    open_folds(&mut f);
    in_section(&mut f, "Advanced");
    assert!(shows(&f, label) && shows(&f, reason), "{:?}", f.drawn);
    let mut f = settings_with(Settings::default());
    f.l.settings_view.query = label.to_lowercase();
    f.settle();
    assert!(shows(&f, label) && shows(&f, reason), "search finds it: {:?}", f.drawn);
}

/// A row shown only because it holds a value stays while its own control is in use: clearing the
/// IGDB client secret (client ID empty) keeps the row under the caret until the field loses focus.
#[test]
fn a_row_set_back_to_its_default_stays_while_its_field_has_focus() {
    use egui::accesskit::Role;
    let label = "IGDB client secret";
    let mut f = settings_with(Settings::default());
    f.ctx.enable_accesskit();
    f.l.game_info_keys.igdb_client_secret = "secret".into();
    open_folds(&mut f);
    in_section(&mut f, "Advanced");
    assert!(shows(&f, label), "held by its secret: {:?}", f.drawn);
    let field = f.access.as_ref().expect("AccessKit is on").nodes.iter()
        .find(|(_, n)| n.role() == Role::TextInput && n.label() == Some(label))
        .map(|(_, n)| n.bounds().expect("laid out"))
        .expect("the secret's field");
    f.click_at(Pos2::new(((field.x0 + field.x1) / 2.0) as f32, ((field.y0 + field.y1) / 2.0) as f32), egui::PointerButton::Primary);
    assert!(f.ctx.text_edit_focused(), "the field has focus");
    // Cleared while it keeps focus: the row stays, the caret with it.
    for _ in 0..12 {
        f.press(egui::Key::Backspace);
    }
    assert!(f.l.game_info_keys.igdb_client_secret.is_empty(), "{:?}", f.l.game_info_keys.igdb_client_secret);
    assert!(f.ctx.text_edit_focused() && shows(&f, label), "kept while in use: {:?}", f.drawn);
    // Focus leaves: the row does nothing without the client ID and holds nothing, so it goes.
    f.press(egui::Key::Escape);
    f.settle();
    assert!(!f.ctx.text_edit_focused());
    assert!(!shows(&f, label), "gone once focus left: {:?}", f.drawn);
}
/// Vibration is one list over the two stored fields: Off keeps the feel, a feel turns vibration on.
#[test]
fn vibration_is_one_choice_over_the_two_stored_fields() {
    use nightjar_policy::rumble::RumbleFeel;
    let mut s = Settings::default();
    assert_eq!(vibration_choice(&s), Vibration::Feel(RumbleFeel::Standard));
    set_vibration_choice(&mut s, Vibration::Feel(RumbleFeel::Strong));
    assert!(!s.disable_rumble && s.rumble_feel == RumbleFeel::Strong);
    set_vibration_choice(&mut s, Vibration::Off);
    assert!(s.disable_rumble && s.rumble_feel == RumbleFeel::Strong, "Off keeps the feel");
    assert_eq!(vibration_choice(&s), Vibration::Off);
    // A settings file that had vibration off with any feel reads Off; on, the feel.
    let old: Settings = serde_json::from_str(r#"{"disable_rumble":true,"rumble_feel":"subtle"}"#).unwrap();
    assert_eq!(vibration_choice(&old), Vibration::Off);
    set_vibration_choice(&mut s, Vibration::Feel(RumbleFeel::Subtle));
    assert!(!s.disable_rumble);
    assert_eq!(VIBRATION_CHOICES.map(vibration_name), ["Off", "Subtle", "Standard", "Strong"]);
    // One row for both keys, in Controls, and the old rows are gone.
    let mut f = settings_with(Settings::default());
    open_folds(&mut f);
    in_section(&mut f, "Controls");
    assert!(shows(&f, "Vibration") && !shows(&f, "Turn off vibration") && !shows(&f, "Rumble feel"), "{:?}", f.drawn);
}
