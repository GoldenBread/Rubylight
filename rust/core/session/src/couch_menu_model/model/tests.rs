//! Tests for couch menu strings, font coverage, page layout, confirmation, and navigation.
mod interactions;

use std::time::{Duration, Instant};

use super::*;
use rubylight_native_api::radeon::{Choice, Range};

/// The `couch_menu` catalogue in English and German, copied from the host's
/// catalogue rule (Latin-1 only, "..." never "…").
pub(crate) const CATALOGUE: &[(&str, &str, &str)] = &[
    ("title", "Host menu", "Host-Menü"),
    ("back_to_game", "Back to game", "Zurück zum Spiel"),
    ("radeon", "Radeon settings", "Radeon-Einstellungen"),
    (
        "restore_radeon",
        "Restore Radeon settings",
        "Radeon-Einstellungen zurücksetzen",
    ),
    ("back", "Back", "Zurück"),
    ("profile", "Profile: ", "Profil: "),
    (
        "disconnect",
        "Disconnect (game keeps running)",
        "Trennen (Spiel läuft weiter)",
    ),
    ("end_session", "End session", "Sitzung beenden"),
    ("close_game", "Quit game", "Spiel beenden"),
    ("run", "Run", "Ausführen"),
    (
        "next_stream",
        "Applies from the next stream",
        "Gilt ab dem nächsten Stream",
    ),
    (
        "confirm",
        "Press again within 5 s to confirm",
        "Bestätigen: in 5 s erneut drücken",
    ),
    ("working", "Working...", "Wird ausgeführt..."),
    ("failed", "Failed: ", "Fehlgeschlagen: "),
    (
        "needs_host_control",
        "Needs host control",
        "Erfordert Host-Steuerung",
    ),
    ("on", "On", "Ein"),
    ("off", "Off", "Aus"),
    (
        "hint",
        "Enter/A: select  Esc/B: back",
        "Enter/A: wählen  Esc/B: zurück",
    ),
];

/// Strings the host puts into the [`Snapshot`]: preset and item labels, the
/// readings line.
pub(crate) const HOST_STRINGS: &[(&str, &str)] = &[
    ("Stream preset", "Stream-Voreinstellung"),
    ("Lowest latency", "Niedrigste Latenz"),
    ("Balanced", "Ausgewogen"),
    ("Reference quality", "Referenzqualität"),
    (
        "GPU readings are off (hardware_metrics)",
        "GPU-Werte sind aus (hardware_metrics)",
    ),
    ("Night mode", "Nachtmodus"),
    ("Headphone surround", "Kopfhörer-Surround"),
    ("Encoder focus", "Encoder-Fokus"),
    ("Still areas", "Ruhige Bereiche"),
    ("Still areas and centre", "Ruhige Bereiche und Mitte"),
    ("Automatic", "Automatisch"),
    ("Sleep after idle", "Ruhezustand nach Leerlauf"),
    ("15 min", "15 Min."),
    (
        "Not supported on this GPU",
        "Auf dieser GPU nicht unterstützt",
    ),
];

pub(crate) fn strings(german: bool) -> Strings {
    Strings::from_lookup(|path| {
        let key = path.strip_prefix("couch_menu/")?;
        let (_, en, de) = CATALOGUE.iter().find(|(k, ..)| *k == key)?;
        Some(if german { de } else { en }.to_string())
    })
    .unwrap()
}

fn choice(id: &str, label: &str, index: usize, options: &[(&str, &str)]) -> Setting {
    Setting {
        id: id.into(),
        label: label.into(),
        control: Control::Choice {
            index,
            options: options
                .iter()
                .map(|(value, label)| Choice {
                    value: value.to_string(),
                    label: label.to_string(),
                })
                .collect(),
        },
        availability: Availability::Ready,
        applies: Applies::Live,
        needs_host_control: false,
    }
}

fn toggle(id: &str, label: &str, on: bool) -> Setting {
    Setting {
        control: Control::Toggle(on),
        ..choice(id, label, 0, &[])
    }
}

fn preset() -> Setting {
    Setting {
        applies: Applies::NextStream,
        needs_host_control: true,
        ..choice(
            "stream_preset",
            "Stream preset",
            1,
            &[
                ("lowest_latency", "Lowest latency"),
                ("balanced", "Balanced"),
                ("reference", "Reference quality"),
            ],
        )
    }
}

/// A host with a Radeon page, the preset and one fake extension item.
pub(crate) fn snapshot() -> Snapshot {
    let integer = |id: &str, label: &str, value, unit: &str| Setting {
        control: Control::Integer {
            value,
            range: Range {
                min: 0,
                max: 100,
                step: 10,
            },
            unit: Some(unit.into()),
        },
        needs_host_control: true,
        ..choice(id, label, 0, &[])
    };
    Snapshot {
        status: "72 °C 210 W 98% 2560x1440 120 Hz".into(),
        host_control: true,
        radeon: Some(RadeonPage {
            gpu: 0,
            name: "AMD Radeon RX 7900 XT".into(),
            profile: Some("Rubylight".into()),
            settings: vec![
                Setting {
                    needs_host_control: true,
                    ..toggle("graphics.anti_lag", "Anti-Lag", true)
                },
                integer("graphics.sharpness", "Image sharpening", 80, "%"),
                Setting {
                    availability: Availability::Locked("Not supported on this GPU".into()),
                    ..toggle("graphics.chill", "Radeon Chill", false)
                },
            ],
        }),
        preset: Some(preset()),
        items: vec![toggle(items::AUDIO_NIGHT_MODE, "Night mode", false)],
        commands: vec![],
    }
}

fn layout() -> Layout {
    Layout::for_stream(1920, 1080).unwrap()
}

struct Run {
    menu: Menu,
    snap: Snapshot,
    now: Instant,
}

impl Run {
    fn new(snap: Snapshot) -> Self {
        let mut run = Self {
            menu: Menu::new(strings(false)),
            snap,
            now: Instant::now(),
        };
        run.nav(Nav::Opened);
        run
    }
    fn nav(&mut self, nav: Nav) -> Option<Action> {
        self.menu.nav(nav, self.now, &self.snap, &layout())
    }
    fn after(&mut self, by: Duration) {
        self.now += by;
    }
    fn text(&self) -> Vec<String> {
        self.menu
            .lines(&self.snap)
            .into_iter()
            .map(|l| l.text.trim_end().to_owned())
            .collect()
    }
    fn focused(&self) -> String {
        let lines = self.menu.lines(&self.snap);
        let line = lines
            .iter()
            .find(|l| matches!(l.style, Style::Row { focus: true, .. }))
            .unwrap();
        line.text.clone()
    }
    fn message(&self) -> String {
        let lines = self.menu.lines(&self.snap);
        lines[lines.len() - 2].text.clone()
    }
    /// Moves the focus down to the row whose text starts with `label`.
    fn focus(&mut self, label: &str) {
        for _ in 0..32 {
            if self.focused().starts_with(label) {
                return;
            }
            self.nav(Nav::Down);
        }
        panic!("no row {label}");
    }
}

#[test]
fn couch_menu_strings_load_from_the_catalogue_and_fail_loudly() {
    assert_eq!(Strings::KEYS.len(), CATALOGUE.len());
    for (key, (catalogue, ..)) in Strings::KEYS.iter().zip(CATALOGUE) {
        assert_eq!(key, catalogue);
    }
    assert_eq!(strings(true).title, "Host-Menü");
    let missing = Strings::from_lookup(|path| (path != "couch_menu/hint").then(|| "x".into()));
    assert_eq!(
        missing,
        Err("couch menu: string couch_menu/hint is missing".into())
    );
    let ellipsis = Strings::from_lookup(|path| {
        Some(
            if path == "couch_menu/working" {
                "Working…"
            } else {
                "x"
            }
            .into(),
        )
    });
    assert_eq!(
        ellipsis,
        Err("couch menu: string couch_menu/working has '…', outside Latin-1".into())
    );
}

#[test]
fn couch_menu_font_covers_every_catalogue_string() {
    for (_, en, de) in CATALOGUE {
        for text in [en, de] {
            assert!(
                text.chars().count() <= COLUMNS as usize,
                "{text} is wider than a line"
            );
        }
    }
    let host = HOST_STRINGS.iter().flat_map(|(en, de)| [en, de]);
    let catalogue = CATALOGUE.iter().flat_map(|(_, en, de)| [en, de]);
    for text in catalogue.chain(host).chain(&["0123456789", "°%./:-"]) {
        assert_eq!(font::uncovered(text), None, "{text}");
    }
}

#[test]
fn couch_menu_all_latin1_locale_catalogues_are_valid() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/locale");
    for locale in [
        "en", "en_GB", "en_US", "de", "es", "fr", "it", "pt", "pt_BR", "sv",
    ] {
        let content = std::fs::read_to_string(dir.join(format!("{locale}.json"))).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        let lookup = |path: &str| {
            path.split('/')
                .try_fold(&json, |node, part| node.get(part))?
                .as_str()
                .map(str::to_owned)
        };
        let loaded = Strings::from_lookup(lookup).unwrap_or_else(|e| panic!("{locale}: {e}"));
        assert!(!loaded.close_game.is_empty(), "{locale}");
        for key in Strings::KEYS {
            let text = lookup(&format!("couch_menu/{key}")).unwrap();
            assert!(
                text.chars().count() <= COLUMNS as usize,
                "{locale} couch_menu/{key}: {text:?} exceeds {COLUMNS} chars"
            );
            assert!(!text.contains('…'), "{locale} couch_menu/{key} uses …");
        }
        for key in items::ALL {
            let path = items::label_path(key);
            let label = lookup(&path).unwrap_or_else(|| panic!("{locale} missing {path}"));
            assert_eq!(font::uncovered(&label), None, "{locale} {path}: {label}");
            assert!(
                label.chars().count() <= COLUMNS as usize,
                "{locale} {path}: {label:?} exceeds {COLUMNS} chars"
            );
        }
    }
}

#[test]
fn couch_menu_model_main_page_rows_in_order() {
    let run = Run::new(snapshot());
    assert_eq!(
        run.text(),
        [
            "Host menu",
            "72 °C 210 W 98% 2560x1440 120 Hz",
            "Back to game",
            "Radeon settings                  >",
            "Stream preset             Balanced",
            "  Applies from the next stream",
            "Night mode                     Off",
            "Disconnect (game keeps running)",
            "End session",
            "Quit game",
            "",
            "Enter/A: select  Esc/B: back",
        ]
    );
    assert!(run.focused().starts_with("Back to game"));
    // Without readings, Radeon or items, the page shrinks to what is left.
    let run = Run::new(Snapshot {
        status: "GPU readings are off (hardware_metrics)".into(),
        host_control: true,
        ..Snapshot::default()
    });
    assert_eq!(
        run.text()[1..6],
        [
            "GPU readings are off (hardware_...",
            "Back to game",
            "Disconnect (game keeps running)",
            "End session",
            "Quit game",
        ],
        "a long line is cut to 34 columns"
    );
}

#[test]
fn couch_menu_model_locked_rows_keep_their_reason() {
    let mut run = Run::new(snapshot());
    run.nav(Nav::Down);
    run.nav(Nav::Accept);
    run.focus("Radeon Chill");
    assert_eq!(run.message(), "Not supported on this GPU");
    assert_eq!(
        run.focused().trim_end(),
        "Radeon Chill                   Off",
        "no arrows"
    );
    for nav in [Nav::Accept, Nav::Left, Nav::Right] {
        assert_eq!(run.nav(nav), None, "{nav:?}");
    }
    assert_eq!(
        run.message(),
        "Not supported on this GPU",
        "the reason stays"
    );

    // Without host control every row that needs it is locked with that reason.
    let mut run = Run::new(Snapshot {
        host_control: false,
        ..snapshot()
    });
    run.focus("Stream preset");
    assert_eq!(run.message(), "Needs host control");
    assert_eq!(run.nav(Nav::Right), None);
    run.focus("Night mode");
    assert_eq!(
        run.message(),
        "",
        "an item that does not need it stays ready"
    );
    assert!(run.nav(Nav::Accept).is_some());
    run.menu.finish(Ok(()));
    run.focus("Quit game");
    assert_eq!(run.message(), "Needs host control");
    assert_eq!(run.nav(Nav::Accept), None);
    assert_eq!(run.nav(Nav::Accept), None);

    // A hidden item has no row at all.
    let mut snap = snapshot();
    snap.items[0].availability = Availability::Hidden;
    let run = Run::new(snap);
    assert!(!run.text().iter().any(|line| line.starts_with("Night mode")));
}

#[test]
fn couch_menu_model_confirm_window_is_5_s() {
    let mut run = Run::new(snapshot());
    run.focus("End session");
    assert_eq!(run.nav(Nav::Accept), None);
    assert_eq!(run.message(), "Press again within 5 s to confirm");
    assert_eq!(run.menu.next_due(), Some(run.now + CONFIRM));
    run.after(Duration::from_millis(4_999));
    assert_eq!(run.nav(Nav::Accept), Some(Action::EndSession));
    run.menu.finish(Ok(()));

    // At 5 s the window has shut: the press arms it again.
    run.focus("Quit game");
    assert_eq!(run.nav(Nav::Accept), None);
    run.after(CONFIRM);
    assert!(run.menu.tick(run.now), "the expiry redraws");
    assert_eq!(run.message(), "");
    assert_eq!(run.menu.next_due(), None);
    assert_eq!(run.nav(Nav::Accept), None);
    assert_eq!(run.nav(Nav::Accept), Some(Action::CloseGame));
    run.menu.finish(Ok(()));

    // Moving away cancels it, and a confirm is per row.
    run.focus("End session");
    run.nav(Nav::Accept);
    run.nav(Nav::Down);
    assert_eq!(run.message(), "");
    assert_eq!(
        run.nav(Nav::Accept),
        None,
        "Quit game needs its own two presses"
    );
    run.nav(Nav::Up);
    assert_eq!(run.nav(Nav::Accept), None, "End session starts over");
    assert_eq!(run.nav(Nav::Accept), Some(Action::EndSession));
}

#[test]
fn couch_menu_model_back_and_close() {
    let mut run = Run::new(snapshot());
    assert_eq!(run.nav(Nav::Accept), Some(Action::Close), "Back to game");
    assert!(!run.menu.busy(), "closing is not an action to wait for");
    assert_eq!(
        run.nav(Nav::Back),
        Some(Action::Close),
        "Back on the main page"
    );
    // Back on the Radeon page goes to the main page, focus on its row.
    run.nav(Nav::Down);
    run.nav(Nav::Accept);
    assert_eq!(run.text()[0], "Radeon settings");
    assert_eq!(run.text()[1], "Profile: Rubylight");
    assert_eq!(run.nav(Nav::Back), None);
    assert_eq!(run.text()[0], "Host menu");
    assert!(run.focused().starts_with("Radeon settings"));
    // So does its Back row.
    run.nav(Nav::Accept);
    assert!(run.focused().starts_with("Back"));
    assert_eq!(run.nav(Nav::Accept), None);
    assert!(run.focused().starts_with("Radeon settings"));
    // A tap outside the panel closes; one on its padding does nothing.
    assert_eq!(
        run.nav(Nav::Point { x: 0.05, y: 0.05 }),
        Some(Action::Close)
    );
    let rect = layout().panel(run.text().len() as u32);
    let (x, y) = (
        (rect.origin[0] + 4) as f32 / 1920.,
        (rect.origin[1] + 4) as f32 / 1080.,
    );
    assert_eq!(run.nav(Nav::Point { x, y }), None);
    // The Radeon page going away mid-page falls back to the main page.
    run.nav(Nav::Accept);
    run.snap.radeon = None;
    assert_eq!(run.text()[0], "Host menu");
    assert_eq!(run.nav(Nav::Back), Some(Action::Close));
    // Opened and Closed reset to the main page's first row.
    run.snap = snapshot();
    run.focus("End session");
    run.nav(Nav::Accept);
    run.nav(Nav::Closed);
    run.nav(Nav::Opened);
    assert!(run.focused().starts_with("Back to game"));
    assert_eq!(run.message(), "");
}

/// A value too wide for its row is cut, never the menu, and the label keeps its first
/// characters; a row that fits is drawn as before.
#[test]
fn a_wide_value_is_cut_and_the_label_keeps_its_first_characters() {
    let width = COLUMNS as usize;
    let mut values: Vec<String> = [32, 34, 40, 26, 28].map(|n| "x".repeat(n)).to_vec();
    values.push(format!("< {} >", "y".repeat(30)));
    for value in &values {
        for (label, start) in [("Label", "Label"), ("Fullscreen optimizations", "Fulls")] {
            let line = columns(label, value);
            assert!(line.chars().count() <= width, "{line:?}");
            assert!(line.starts_with(start), "{line:?}");
        }
    }
    // The value gives way, not the label.
    let line = columns("Label", &"x".repeat(40));
    assert_eq!(line, format!("Label {}...", "x".repeat(25)));
    // Rows that fit are unchanged.
    assert_eq!(columns("Label", "On"), format!("Label{}On", " ".repeat(27)));
    assert_eq!(
        columns("Label", &"x".repeat(26)),
        format!("Label   {}", "x".repeat(26))
    );
    let long = "A label far too long to fit in its row";
    assert_eq!(columns(long, "Off"), format!("{}... Off", &long[..27]));
    assert_eq!(columns("", &"x".repeat(34)), "x".repeat(34));
    // Never wider than asked, even below the width of "...".
    for width in 0..=3 {
        assert!(fit("abcdef", width).chars().count() <= width);
    }
    assert_eq!(fit("abcdef", 2), "ab");
}
