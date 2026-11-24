//! Tests for couch menu setting changes, busy/error states, pointer taps, scrolling, and PC commands.
use super::super::*;
use super::{Run, choice, layout, preset, snapshot, toggle};
use rubylight_native_api::radeon::{Choice, Range};

#[test]
fn couch_menu_model_changes_go_out_as_native_api_changes() {
    let mut run = Run::new(snapshot());
    let change = |target, id: &str, value| {
        Some(Action::Change {
            target,
            change: Change {
                id: id.into(),
                value,
            },
        })
    };
    run.focus("Stream preset");
    assert_eq!(run.focused(), "Stream preset         < Balanced >");
    let reference = Value::Choice("reference".into());
    assert_eq!(
        run.nav(Nav::Right),
        change(Target::Preset, "stream_preset", reference)
    );
    run.menu.finish(Ok(()));
    run.snap.preset.as_mut().unwrap().control = preset().control;
    let lowest = Value::Choice("lowest_latency".into());
    assert_eq!(
        run.nav(Nav::Left),
        change(Target::Preset, "stream_preset", lowest)
    );
    run.menu.finish(Ok(()));
    let Control::Choice { index, .. } = &mut run.snap.preset.as_mut().unwrap().control else {
        unreachable!()
    };
    *index = 0;
    assert_eq!(run.nav(Nav::Left), None, "the first choice stops Left");
    let balanced = Value::Choice("balanced".into());
    assert_eq!(
        run.nav(Nav::Accept),
        change(Target::Preset, "stream_preset", balanced)
    );
    run.menu.finish(Ok(()));

    run.focus("Night mode");
    let night = items::AUDIO_NIGHT_MODE;
    assert_eq!(run.nav(Nav::Left), None, "already off");
    assert_eq!(
        run.nav(Nav::Right),
        change(Target::Item, night, Value::Bool(true))
    );
    run.menu.finish(Ok(()));
    assert_eq!(
        run.nav(Nav::Accept),
        change(Target::Item, night, Value::Bool(true))
    );
    run.menu.finish(Ok(()));

    run.nav(Nav::Back);
    run.focus("Radeon settings");
    run.nav(Nav::Accept);
    run.focus("Image sharpening");
    assert_eq!(run.focused(), "Image sharpening          < 80 % >");
    let radeon = Target::Radeon { gpu: 0 };
    let sharp = "graphics.sharpness";
    assert_eq!(
        run.nav(Nav::Right),
        change(radeon, sharp, Value::Integer(90))
    );
    run.menu.finish(Ok(()));
    assert_eq!(
        run.nav(Nav::Left),
        change(radeon, sharp, Value::Integer(70))
    );
    run.menu.finish(Ok(()));
    let settings = &mut run.snap.radeon.as_mut().unwrap().settings;
    let Control::Integer { value, .. } = &mut settings[1].control else {
        unreachable!()
    };
    *value = 100;
    assert_eq!(run.nav(Nav::Right), None, "the top stops Right");
    assert_eq!(
        run.nav(Nav::Accept),
        change(radeon, sharp, Value::Integer(0)),
        "Accept wraps"
    );
    run.menu.finish(Ok(()));
    run.focus("Restore Radeon settings");
    assert_eq!(run.nav(Nav::Accept), Some(Action::RestoreRadeon { gpu: 0 }));
}

#[test]
fn couch_menu_model_working_and_failures() {
    let mut run = Run::new(snapshot());
    run.focus("Disconnect");
    assert_eq!(run.nav(Nav::Accept), Some(Action::Disconnect));
    assert!(run.menu.busy());
    assert_eq!(run.focused(), "Disconnect (game kee... Working...");
    assert_eq!(run.nav(Nav::Accept), None, "accepts wait for the action");
    run.focus("Night mode");
    assert_eq!(run.nav(Nav::Right), None);
    run.menu
        .finish(Err("access denied by the session owner".into()));
    assert!(!run.menu.busy());
    assert_eq!(run.message(), "Failed: access denied by the se...");
    run.nav(Nav::Down);
    assert_eq!(
        run.message(),
        "Failed: access denied by the se...",
        "until the next action"
    );
    run.nav(Nav::Up);
    assert!(run.nav(Nav::Accept).is_some());
    assert_eq!(run.message(), "");
}

#[test]
fn couch_menu_model_a_tap_focuses_and_accepts_its_row() {
    let mut run = Run::new(snapshot());
    let lines = run.text();
    let rect = layout().panel(lines.len() as u32);
    let at = |line: usize| Nav::Point {
        x: (rect.origin[0] + 100) as f32 / 1920.,
        y: (rect.origin[1] + 16 + line as u32 * 32 + 10) as f32 / 1080.,
    };
    let disconnect = lines
        .iter()
        .position(|l| l.starts_with("Disconnect"))
        .unwrap();
    assert_eq!(run.nav(at(disconnect)), Some(Action::Disconnect));
    assert!(run.focused().starts_with("Disconnect"));
    run.menu.finish(Ok(()));
    let note = lines
        .iter()
        .position(|l| l.starts_with("  Applies"))
        .unwrap();
    let balanced = Value::Choice("reference".into());
    assert_eq!(
        run.nav(at(note)),
        Some(Action::Change {
            target: Target::Preset,
            change: Change {
                id: "stream_preset".into(),
                value: balanced,
            },
        }),
        "a note belongs to its row"
    );
    run.menu.finish(Ok(()));
    let end = lines.iter().position(|l| l == "End session").unwrap();
    assert_eq!(run.nav(at(end)), None);
    assert_eq!(
        run.nav(at(end)),
        Some(Action::EndSession),
        "two taps confirm"
    );
    assert_eq!(run.nav(at(0)), None, "the title is no row");
}

#[test]
fn couch_menu_model_long_pages_scroll_to_the_focus() {
    let mut snap = snapshot();
    snap.items = (0..20)
        .map(|i| toggle(&format!("item{i}"), &format!("Item {i}"), false))
        .collect();
    let mut run = Run::new(snap);
    for _ in 0..30 {
        let lines = run.menu.lines(&run.snap);
        assert!(lines.len() <= MAX_LINES as usize);
        assert!(
            lines
                .iter()
                .any(|l| matches!(l.style, Style::Row { focus: true, .. }))
        );
        assert!(
            lines
                .iter()
                .all(|l| l.text.chars().count() <= COLUMNS as usize)
        );
        run.nav(Nav::Down);
    }
    assert!(Layout::for_stream(640, 360).is_ok());
}

#[test]
fn couch_menu_model_odd_host_values_do_not_panic_the_worker() {
    let mut snap = snapshot();
    snap.items = vec![
        choice("empty", "Empty choice", 3, &[]),
        Setting {
            control: Control::Integer {
                value: i64::MAX,
                range: Range {
                    min: 10,
                    max: 0,
                    step: 0,
                },
                unit: None,
            },
            ..choice("inverted", "Inverted range", 0, &[])
        },
    ];
    let mut run = Run::new(snap);
    run.focus("Empty choice");
    for nav in [Nav::Accept, Nav::Left, Nav::Right] {
        assert_eq!(run.nav(nav), None, "{nav:?}");
    }
    run.focus("Inverte");
    for nav in [Nav::Accept, Nav::Left, Nav::Right] {
        run.nav(nav);
        run.menu.finish(Ok(()));
    }
    assert!(run.text().len() <= MAX_LINES as usize);
}

#[test]
fn couch_menu_setting_from_a_native_api_setting() {
    let base = radeon::Setting {
        id: "encoder.focus".into(),
        group: "encoder".into(),
        kind: Kind::Choice,
        state: ControlState::Configurable,
        value: Some(Value::Choice("still".into())),
        range: None,
        options: vec![
            Choice {
                value: "off".into(),
                label: "Off".into(),
            },
            Choice {
                value: "still".into(),
                label: "Still areas".into(),
            },
        ],
        unit: None,
        restore: None,
        reason: None,
        message: None,
    };
    let row = Setting::from_radeon(&base, "Encoder focus".into());
    assert_eq!(row.id, "encoder.focus");
    assert_eq!(row.availability, Availability::Ready);
    assert!(matches!(row.control, Control::Choice { index: 1, .. }));
    let locked = Setting::from_radeon(
        &radeon::Setting {
            state: ControlState::ReadOnly,
            reason: Some("Set by the host".into()),
            ..base.clone()
        },
        "Encoder focus".into(),
    );
    assert_eq!(
        locked.availability,
        Availability::Locked("Set by the host".into())
    );
    let unknown = Setting::from_radeon(
        &radeon::Setting {
            state: ControlState::Unsupported,
            kind: Kind::Unknown,
            value: Some(Value::Integer(3)),
            ..base.clone()
        },
        "x".into(),
    );
    assert_eq!(unknown.control, Control::Text("3".into()));
    assert_eq!(
        unknown.availability,
        Availability::Locked("Unsupported".into())
    );
    let integer = Setting::from_radeon(
        &radeon::Setting {
            kind: Kind::Integer,
            value: Some(Value::Integer(40)),
            range: Some(Range {
                min: 0,
                max: 100,
                step: 5,
            }),
            unit: Some("%".into()),
            ..base
        },
        "x".into(),
    );
    assert!(matches!(
        integer.control,
        Control::Integer { value: 40, .. }
    ));
    assert_eq!(
        items::label_path(items::ENCODER_FOCUS),
        "couch_menu/items/encoder_focus"
    );
}

#[test]
fn couch_menu_model_pc_commands_run_by_index_after_a_confirm() {
    let mut run = Run::new(Snapshot {
        commands: vec!["Lock PC".into(), "Desktop pet".into()],
        ..snapshot()
    });
    let text = run.text();
    let at = |label: &str| {
        text.iter()
            .position(|line| line.starts_with(label))
            .unwrap()
    };
    assert!(at("Night mode") < at("Lock PC"));
    assert!(at("Desktop pet") < at("Disconnect"));
    run.focus("Desktop pet");
    assert_eq!(
        run.focused().trim_end(),
        "Desktop pet                    Run"
    );
    assert_eq!(run.nav(Nav::Accept), None, "a command needs a confirm");
    assert_eq!(run.message(), "Press again within 5 s to confirm");
    assert_eq!(
        run.nav(Nav::Accept),
        Some(Action::RunCommand {
            index: 1,
            name: "Desktop pet".into()
        })
    );
    run.menu.finish(Err("The command changed.".into()));
    assert_eq!(run.message(), "Failed: The command changed.");
    // The confirm is per command row.
    run.focus("Lock PC");
    run.nav(Nav::Accept);
    run.focus("Desktop pet");
    assert_eq!(run.nav(Nav::Accept), None);
    // Without commands there are no rows.
    let run = Run::new(snapshot());
    assert!(!run.text().iter().any(|line| line.ends_with("Run")));
}
