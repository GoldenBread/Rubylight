//! Couch-menu filter navigation, repeat timing, refusal, and custom chord tests.

use super::*;

#[test]
fn couch_menu_filter_navigation_and_repeat_timings() {
    let t0 = Instant::now();
    let (_, mut filter) = setup();
    let open = open_by_pad(&mut filter, t0);
    // The opening pad lets go of the chord.
    run(&mut filter, open, &[pad(0, 0)]);
    let (_, out) = run(&mut filter, open, &[pad(0, DPAD_DOWN)]);
    assert_eq!(out.nav, [Nav::Down]);
    assert_eq!(filter.next_due(), Some(open + 400 * MS));
    let (_, out) = run(&mut filter, open + 399 * MS, &[]);
    assert!(out.nav.is_empty());
    let (_, out) = run(&mut filter, open + 400 * MS, &[]);
    assert_eq!(out.nav, [Nav::Down]);
    assert_eq!(filter.next_due(), Some(open + 520 * MS));
    let (_, out) = run(&mut filter, open + 519 * MS, &[]);
    assert!(out.nav.is_empty());
    let (_, out) = run(&mut filter, open + 520 * MS, &[]);
    assert_eq!(out.nav, [Nav::Down]);
    // Same buttons again (a stick move elsewhere) neither repeats nor resets.
    let (_, out) = run(&mut filter, open + 600 * MS, &[stick(0, DPAD_DOWN, 0, 0)]);
    assert!(out.nav.is_empty());
    assert_eq!(filter.next_due(), Some(open + 640 * MS));
    // A late wake gives one step, not a burst.
    let (_, out) = run(&mut filter, open + 2000 * MS, &[]);
    assert_eq!(out.nav, [Nav::Down]);
    assert_eq!(filter.next_due(), Some(open + 2120 * MS));
    let (_, out) = run(&mut filter, open + 2050 * MS, &[pad(0, 0)]);
    assert!(out.nav.is_empty());
    assert_eq!(filter.next_due(), None);

    // The stick: over 50% starts, under 30% ends.
    let (_, out) = run(&mut filter, open, &[stick(0, 0, 0, 16_000)]);
    assert!(out.nav.is_empty());
    let (_, out) = run(&mut filter, open, &[stick(0, 0, 0, 20_000)]);
    assert_eq!(out.nav, [Nav::Up]);
    let (_, out) = run(&mut filter, open, &[stick(0, 0, 0, 10_000)]);
    assert!(out.nav.is_empty(), "still latched above 30%");
    let (_, out) = run(
        &mut filter,
        open,
        &[stick(0, 0, 0, 9_000), stick(0, 0, -30_000, 0)],
    );
    assert_eq!(out.nav, [Nav::Left]);
    let (_, out) = run(
        &mut filter,
        open,
        &[stick(0, 0, 0, 0), stick(0, 0, 0, -20_000)],
    );
    assert_eq!(out.nav, [Nav::Down]);

    // Buttons, keys, wheel, clicks and taps.
    let (_, out) = run(
        &mut filter,
        open,
        &[
            stick(0, BUTTON_A, 0, 0),
            pad(0, BUTTON_A | BUTTON_B),
            pad(0, 0),
            key(VK_UP, 0, true),
            key(VK_UP, 0, false),
            key(VK_RIGHT, 0, true),
            key(VK_SPACE, 0, true),
            key(VK_ESCAPE, 0, true),
            key(VK_BACK, 0, true),
            Input::Scroll {
                amount: -120,
                horizontal: false,
            },
            Input::Absolute {
                x: 960,
                y: 270,
                width: 1920,
                height: 1080,
            },
            mouse(MOUSE_LEFT, true),
            mouse(MOUSE_RIGHT, true),
            touch(TOUCH_DOWN, 2, 0.25, 0.75),
        ],
    );
    assert_eq!(
        out.nav,
        [
            Nav::Accept,
            Nav::Back,
            Nav::Up,
            Nav::Right,
            Nav::Accept,
            Nav::Back,
            Nav::Back,
            Nav::Down,
            Nav::Point { x: 0.5, y: 0.25 },
            Nav::Back,
            Nav::Point { x: 0.25, y: 0.75 },
        ]
    );
    // A relative mouse clicks at the host cursor.
    let mut batch = vec![Input::Relative { x: 1, y: 1 }, mouse(MOUSE_LEFT, true)];
    let out = filter.apply(open, &mut batch, || Some((0.1, 0.2)));
    assert_eq!(out.nav, [Nav::Point { x: 0.1, y: 0.2 }]);
}

#[test]
fn couch_menu_filter_refusal_is_reported_once_and_the_key_passes() {
    let t0 = Instant::now();
    let (shared, mut filter) = setup();
    shared.refuse(Some("couch menu: needs wgc_direct_encoder_input".into()));
    let chord = key(VK_BACK, CTRL_ALT, true);
    let (passed, out) = run(&mut filter, t0, std::slice::from_ref(&chord));
    assert_eq!(passed, std::slice::from_ref(&chord));
    assert_eq!(
        out.refused.as_deref(),
        Some("couch menu: needs wgc_direct_encoder_input")
    );
    assert!(!out.release_game);
    let (_, out) = run(&mut filter, t0, &[chord]);
    assert_eq!(out.refused, None, "once per session");
    run(&mut filter, t0, &[pad(0, PAD_CHORD)]);
    let (_, out) = run(&mut filter, t0 + 1000 * MS, &[]);
    assert_eq!(out, Outcome::default());
}

#[test]
fn couch_menu_filter_open_with_turns_ways_off() {
    let t0 = Instant::now();
    let shared = Arc::new(Shared::new());
    let none = OpenWith::parse("").unwrap();
    let mut filter = Filter::new(shared.clone(), none, 1920, 1080);
    let events = [
        key(VK_BACK, CTRL_ALT, true),
        pad(0, PAD_CHORD),
        touch(TOUCH_DOWN, 1, 0.01, 0.01),
    ];
    let (passed, _) = run(&mut filter, t0, &events);
    assert_eq!(passed, events);
    assert_eq!(filter.next_due(), None);
    let (_, out) = run(&mut filter, t0 + 2000 * MS, &[]);
    assert_eq!(out, Outcome::default());
    // Opened through the native API, Back still closes it.
    shared.open().unwrap();
    let (_, out) = run(&mut filter, t0, &[key(VK_ESCAPE, 0, true)]);
    assert_eq!(out.nav, [Nav::Opened, Nav::Back]);
}

#[test]
fn couch_menu_filter_a_second_pad_holding_the_chord_waits_for_a_change() {
    let t0 = Instant::now();
    let (_, mut filter) = setup();
    run(&mut filter, t0, &[pad(0, PAD_CHORD)]);
    run(&mut filter, t0 + 500 * MS, &[pad(1, PAD_CHORD)]);
    let (_, out) = run(&mut filter, t0 + 1000 * MS, &[]);
    assert!(out.release_game && filter.is_open(), "pad 0 opened it");
    // Pad 1 still reports L3+R3: the open cancelled its hold, and it must
    // not arm again and close the menu a second later.
    for ms in [1100, 1600, 2100, 2600, 3100] {
        let (_, out) = run(&mut filter, t0 + ms * MS, &[pad(1, PAD_CHORD)]);
        assert!(!out.nav.contains(&Nav::Closed), "at {ms} ms");
        assert!(filter.is_open(), "at {ms} ms");
    }
    // Let go and hold again: a new hold, which closes it.
    run(&mut filter, t0 + 3200 * MS, &[pad(0, 0), pad(1, 0)]);
    run(&mut filter, t0 + 3300 * MS, &[pad(1, PAD_CHORD)]);
    run(&mut filter, t0 + 4300 * MS, &[]);
    assert!(!filter.is_open());
}

fn custom(controller: &str, hold_ms: &str, keyboard: &str) -> Filter {
    let open_with = OpenWith {
        pad_chord: PadButtons::parse(controller).unwrap(),
        pad_hold: parse_hold("couch_menu_hold_ms", hold_ms, PAD_HOLD_MS).unwrap(),
        key_chord: KeyChord::parse(keyboard).unwrap(),
        ..OpenWith::default()
    };
    Filter::new(Arc::new(Shared::new()), open_with, 1920, 1080)
}

#[test]
fn couch_menu_filter_custom_single_button_hold_opens_at_its_time_and_closes() {
    const SELECT: u32 = 0x0020;
    let t0 = Instant::now();
    let mut filter = custom("select", "300", "ctrl+alt+backspace");
    // L3+R3 is an ordinary press now.
    run(&mut filter, t0, &[pad(0, PAD_CHORD)]);
    assert_eq!(filter.next_due(), None);
    let (passed, _) = run(&mut filter, t0, &[pad(0, SELECT)]);
    assert_eq!(passed, [pad(0, SELECT)], "the game sees the press");
    assert_eq!(filter.next_due(), Some(t0 + 300 * MS));
    let (_, out) = run(&mut filter, t0 + 299 * MS, &[]);
    assert_eq!(out, Outcome::default());
    let (_, out) = run(&mut filter, t0 + 300 * MS, &[]);
    assert!(out.release_game && filter.is_open());
    // Still held: no close until it is let go and held again.
    run(&mut filter, t0 + 900 * MS, &[pad(0, SELECT)]);
    assert!(filter.is_open());
    run(&mut filter, t0 + 1000 * MS, &[pad(0, 0)]);
    run(&mut filter, t0 + 1100 * MS, &[pad(0, SELECT)]);
    run(&mut filter, t0 + 1399 * MS, &[]);
    assert!(filter.is_open());
    let (_, out) = run(&mut filter, t0 + 1400 * MS, &[]);
    assert_eq!(out.nav, [Nav::Closed]);
    // An extra button cancels a hold.
    run(&mut filter, t0 + 2000 * MS, &[pad(0, 0), pad(0, SELECT)]);
    run(&mut filter, t0 + 2100 * MS, &[pad(0, SELECT | BUTTON_A)]);
    let (_, out) = run(&mut filter, t0 + 3000 * MS, &[]);
    assert_eq!(out, Outcome::default());
    assert!(!filter.is_open());
}

#[test]
fn couch_menu_filter_custom_key_chord_replaces_the_default() {
    const VK_HOME: u16 = 0x24;
    let t0 = Instant::now();
    let mut filter = custom("l3+r3", "1000", "ctrl+alt+home");
    let old = key(VK_BACK, CTRL_ALT, true);
    let (passed, out) = run(&mut filter, t0, std::slice::from_ref(&old));
    assert_eq!(passed, [old], "Ctrl+Alt+Backspace goes to the game");
    assert!(!out.release_game && !filter.is_open());
    let shifted = key(VK_HOME, CTRL_ALT | MODIFIER_SHIFT, true);
    let (passed, _) = run(&mut filter, t0, std::slice::from_ref(&shifted));
    assert_eq!(passed, [shifted]);
    let (passed, out) = run(
        &mut filter,
        t0,
        &[
            key(VK_HOME, CTRL_ALT | MODIFIER_SHIFT, false),
            key(VK_HOME, CTRL_ALT, true),
        ],
    );
    assert_eq!(passed, [key(VK_HOME, CTRL_ALT | MODIFIER_SHIFT, false)]);
    assert!(out.release_game && filter.is_open());
    // In the open menu Backspace is Back, and the custom chord closes.
    let (_, out) = run(&mut filter, t0, &[key(VK_BACK, CTRL_ALT, true)]);
    assert_eq!(out.nav, [Nav::Back]);
    let (_, out) = run(
        &mut filter,
        t0,
        &[key(VK_HOME, CTRL_ALT, false), key(VK_HOME, CTRL_ALT, true)],
    );
    assert_eq!(out.nav, [Nav::Closed]);
}

#[test]
fn couch_menu_filter_key_chord_repeats_do_not_toggle_again() {
    let t0 = Instant::now();
    let (shared, mut filter) = setup();
    let chord = key(VK_BACK, CTRL_ALT, true);
    let (passed, out) = run(&mut filter, t0, &[chord.clone(), chord.clone()]);
    assert!(passed.is_empty());
    assert_eq!(
        out.nav,
        [Nav::Opened],
        "the repeat neither closes nor goes Back"
    );
    // Auto-repeat keeps sending key-downs while the key is held.
    for ms in [500, 533, 566] {
        let (passed, out) = run(&mut filter, t0 + ms * MS, std::slice::from_ref(&chord));
        assert!(passed.is_empty() && out.nav.is_empty(), "at {ms} ms");
        assert!(
            filter.is_open() && Shared::is_open(shared.load()),
            "at {ms} ms"
        );
    }
    // Let go and press again: that closes, and its repeats do not reopen.
    let (_, out) = run(
        &mut filter,
        t0 + 600 * MS,
        &[key(VK_BACK, CTRL_ALT, false), chord.clone(), chord.clone()],
    );
    assert_eq!(out.nav, [Nav::Closed]);
    let (passed, out) = run(&mut filter, t0 + 700 * MS, std::slice::from_ref(&chord));
    assert!(passed.is_empty(), "the held key stays masked");
    assert_eq!(out, Outcome::default());
    assert!(!filter.is_open());
}

#[test]
fn couch_menu_filter_closing_chord_presses_no_menu_button() {
    // Every chord the host accepts leaves out the buttons the open menu
    // reads, so holding it to close never presses Accept on Disconnect.
    let navigation = BUTTON_A | BUTTON_B | DPAD_UP | DPAD_DOWN | DPAD_LEFT | DPAD_RIGHT;
    for name in ["a", "b", "up", "down", "left", "right"] {
        assert!(
            PadButtons::parse(&format!("l1+r1+{name}")).is_err(),
            "{name}"
        );
    }
    for name in [
        "select", "l3", "r3", "l1", "r1", "guide", "x", "y", "paddle1", "paddle2", "paddle3",
        "paddle4", "touchpad", "misc",
    ] {
        assert_eq!(
            PadButtons::parse(name).unwrap().buttons() & navigation,
            0,
            "{name}"
        );
    }
    let t0 = Instant::now();
    let mut filter = custom("l1+r1+y", "1000", "ctrl+alt+backspace");
    let chord = PadButtons::parse("l1+r1+y").unwrap().buttons();
    run(&mut filter, t0, &[pad(0, chord)]);
    let (_, out) = run(&mut filter, t0 + 1000 * MS, &[]);
    assert!(out.release_game && filter.is_open());
    run(&mut filter, t0 + 1100 * MS, &[pad(0, 0)]);
    // Pressed one button at a time, as a hand does.
    let (_, out) = run(
        &mut filter,
        t0 + 2000 * MS,
        &[
            pad(0, chord & 0x0100),
            pad(0, chord & 0x0300),
            pad(0, chord),
        ],
    );
    assert!(out.nav.is_empty(), "{:?}", out.nav);
    let (_, out) = run(&mut filter, t0 + 3000 * MS, &[]);
    assert_eq!(out.nav, [Nav::Closed]);
}

#[test]
fn couch_menu_filter_custom_corner_hold() {
    let t0 = Instant::now();
    let open_with = OpenWith {
        corner_hold: 500 * MS,
        ..OpenWith::default()
    };
    let mut filter = Filter::new(Arc::new(Shared::new()), open_with, 1920, 1080);
    run(&mut filter, t0, &[touch(TOUCH_DOWN, 1, 0.01, 0.01)]);
    assert_eq!(filter.next_due(), Some(t0 + 500 * MS));
    let (_, out) = run(&mut filter, t0 + 499 * MS, &[]);
    assert_eq!(out, Outcome::default());
    let (_, out) = run(&mut filter, t0 + 500 * MS, &[]);
    assert!(out.release_game && filter.is_open());
}
