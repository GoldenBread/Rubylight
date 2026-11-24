//! Display lookup, coordinate mapping, keyboard layout, touchpad warning, and lazy gamepad tests.

use super::*;

/// An injector for the first display, if this machine has one.
fn on_first_display() -> Option<(Injector, String)> {
    let (name, _) = crate::os_helpers::active_displays_gdi()
        .into_iter()
        .next()?;
    let mut injector = Injector::new(&name, "vhf").unwrap();
    injector.inject = record;
    Some((injector, name))
}

#[test]
fn absolute_input_waits_for_the_display_lookup_it_needs() {
    let Some((mut injector, _)) = on_first_display() else {
        return;
    };
    // Created at once; the display is looked up on another thread.
    assert!(injector.rect.is_none());
    assert!(injector.lookup.as_ref().is_some_and(|l| l.thread.is_some()));
    let events = [Event::Absolute {
        x: 0,
        y: 0,
        width: 100,
        height: 100,
    }];
    assert!(injector.apply_all(&events).is_empty());
    assert!(injector.rect.is_some());
    assert_eq!(calls().len(), 1);
}

#[test]
fn returning_to_the_current_display_discards_an_obsolete_lookup() {
    let mut injector = recording();
    injector.settle_lookup(true);
    let output = injector.output.clone();
    let current = RECT {
        left: 0,
        top: 0,
        right: 100,
        bottom: 100,
    };
    injector.rect = Some(current);
    injector.lookup = Some(Lookup {
        output: r"\\.\DISPLAY98".into(),
        started: std::time::Instant::now(),
        thread: Some(std::thread::spawn(|| {
            Ok(RECT {
                left: 100,
                top: 0,
                right: 200,
                bottom: 100,
            })
        })),
    });
    injector.set_output(&output);
    assert_eq!(injector.display(), Some(current));
    assert_eq!(injector.output, output);
}

#[test]
fn a_renamed_display_is_looked_up_without_holding_up_the_pass() {
    let Some((mut injector, first)) = on_first_display() else {
        return;
    };
    injector.settle_lookup(true);
    let rect = injector.rect.unwrap();
    // The session names a display that does not exist (yet): absolute
    // input keeps the display it has until the new one is found.
    injector.set_output(r"\\.\DISPLAY99");
    let lookup = injector.lookup.as_ref().unwrap();
    assert!(lookup.thread.is_some() && lookup.output == r"\\.\DISPLAY99");
    assert_eq!(injector.output, first);
    let started = lookup.started;
    injector.settle_lookup(true);
    assert_eq!(
        (injector.rect, injector.output.as_str()),
        (Some(rect), &*first)
    );
    // A name not found is not looked up again on every pass.
    injector.set_output(r"\\.\DISPLAY99");
    if started.elapsed() < RECT_INTERVAL {
        assert!(injector.lookup.as_ref().unwrap().thread.is_none());
    }
}

#[test]
fn touch_and_pen_keep_their_place_between_mouse_moves() {
    let mut injector = recording();
    // There is no display: touch and pen reach no one, but still split
    // the pass so mouse input around them keeps its order.
    let events = [
        Event::Relative { x: 1, y: 0 },
        Event::Touch {
            event: 1,
            id: 7,
            x: 0.5,
            y: 0.5,
            pressure: 0.5,
            major: 0.,
            minor: 0.,
            rotation: u16::MAX,
        },
        Event::Relative { x: 2, y: 0 },
        Event::Pen {
            event: 0,
            tool: 1,
            buttons: 0,
            x: 0.5,
            y: 0.5,
            pressure: 0.,
            rotation: u16::MAX,
            tilt: u8::MAX,
            roll: u16::MAX,
        },
        Event::Relative { x: 3, y: 0 },
    ];
    assert!(injector.apply_all(&events).is_empty());
    assert_eq!(
        calls(),
        [1, 2, 3].map(|x| described(&[Injector::mouse(x, 0, 0, MOUSEEVENTF_MOVE)]))
    );
}

#[test]
fn the_cursor_maps_onto_the_stream_bars_included() {
    // A 1920x1080 display right of the primary one.
    let rect = RECT {
        left: 2560,
        top: 0,
        right: 4480,
        bottom: 1080,
    };
    let at = |x, y, picture| cursor_in_stream(POINT { x, y }, rect, picture);
    assert_eq!(at(2560, 0, None), Some((0., 0.)));
    assert_eq!(at(2560 + 960, 540, None), Some((0.5, 0.5)));
    // Off the display, its right and bottom edges included.
    for (x, y) in [(2559, 10), (4480, 10), (3000, -1), (3000, 1080), (100, 100)] {
        assert_eq!(at(x, y, None), None, "{x},{y}");
    }
    // The display letterboxed into a 4:3 stream: its 16:9 picture in
    // 1600x1200 sits at (0, 150).
    let stream = (1600, 1200);
    let picture = butterpollo_media_policy::display_policy::picture((1920, 1080), stream).unwrap();
    assert_eq!(picture, (0, 150, 1600, 900));
    assert_eq!(
        at(2560, 0, Some((stream, picture))),
        Some((0., 150. / 1200.))
    );
    assert_eq!(
        at(2560 + 960, 540, Some((stream, picture))),
        Some((0.5, 0.5))
    );
    // The stream point maps back onto the same display point, as absolute
    // input does.
    let (x, y) = at(2560 + 480, 270, Some((stream, picture))).unwrap();
    let back = butterpollo_media_policy::display_policy::stream_to_picture(
        (f64::from(x), f64::from(y)),
        stream,
        picture,
    );
    assert!(
        (back.0 - 0.25).abs() < 1e-6 && (back.1 - 0.25).abs() < 1e-6,
        "{back:?}"
    );
    // A degenerate rectangle is no display.
    let empty = RECT {
        right: 2560,
        ..rect
    };
    assert_eq!(cursor_in_stream(POINT { x: 2560, y: 0 }, empty, None), None);
}

#[test]
fn a_capped_stream_maps_absolute_input_onto_the_whole_display() {
    let at = |stream: (u32, u32)| {
        let mut injector = recording();
        injector.lookup = None;
        injector.rect = Some(RECT {
            left: 0,
            top: 0,
            right: 3840,
            bottom: 2160,
        });
        injector.set_stream_size(stream.0, stream.1);
        injector
            .apply(&Event::Absolute {
                x: 50,
                y: 50,
                width: 100,
                height: 100,
            })
            .unwrap();
        calls().into_iter().flatten().collect::<Vec<_>>()
    };
    // A 4K display coded at 1440p keeps its shape, so the picture fills the
    // coded frame: the same stream point is the same display point as at native
    // 4K, never a point of the 2560x1440 frame taken as the display itself.
    assert_eq!(at((2560, 1440)), at((3840, 2160)));
    assert_eq!(at((2560, 1440)).len(), 1, "one absolute move");
}

#[test]
fn a_turned_portrait_display_maps_the_stream_upright() {
    // A 2560x1440 monitor turned to portrait left of the primary one: Windows
    // gives its desktop the turned size, and capture streams it upright.
    let rect = RECT {
        left: -1440,
        top: 0,
        right: 0,
        bottom: 2560,
    };
    let at = |x, y, picture| cursor_in_stream(POINT { x, y }, rect, picture);
    assert_eq!(at(-1440, 0, None), Some((0., 0.)));
    assert_eq!(at(-720, 1280, None), Some((0.5, 0.5)));
    assert_eq!(at(-1440, 1920, None), Some((0., 0.75)));
    // Its old landscape extent is off the display now.
    assert_eq!(at(-1440, 2560, None), None);
    assert_eq!(at(1000, 100, None), None);
    // A portrait phone's 1080x2400 stream holds the 9:16 desktop with bars
    // above and below, and a point maps there and back.
    let stream = (1080, 2400);
    let picture = butterpollo_media_policy::display_policy::picture((1440, 2560), stream).unwrap();
    assert_eq!(picture, (0, 240, 1080, 1920));
    assert_eq!(at(-1440, 0, Some((stream, picture))), Some((0., 0.1)));
    let (x, y) = at(-1440 + 360, 1920, Some((stream, picture))).unwrap();
    let back = butterpollo_media_policy::display_policy::stream_to_picture(
        (f64::from(x), f64::from(y)),
        stream,
        picture,
    );
    assert!(
        (back.0 - 0.25).abs() < 1e-6 && (back.1 - 0.75).abs() < 1e-6,
        "{back:?}"
    );
}

fn non_normalized(key: u16, down: bool) -> Event {
    Event::Keyboard {
        key,
        modifiers: 0,
        flags: 1,
        down,
    }
}
/// Sets the layout with this id and lets it take over if it can.
fn use_layout(injector: &mut Injector, klid: Option<u32>) {
    injector.set_keyboard_layout(klid.map(|klid| crate::keylayout::Layout::load(klid).unwrap()));
    injector.follow_keyboard_layout();
}
/// The scan code and flags of each key sent since the last look.
fn scans() -> Vec<(u16, u32)> {
    CALLS
        .with_borrow_mut(std::mem::take)
        .iter()
        .flatten()
        // SAFETY: The keyboard tests send only keyboard INPUTs, so each ki union field is initialized.
        .map(|input| unsafe { (input.Anonymous.ki.wScan, input.Anonymous.ki.dwFlags.0) })
        .collect()
}
const SCAN: u32 = KEYEVENTF_SCANCODE.0;
const UP: u32 = KEYEVENTF_SCANCODE.0 | KEYEVENTF_KEYUP.0;

/// Keys the client did not normalize get the scan codes of the device's layout. A new
/// layout takes over only once no key is held, so a release goes up where it went down.
#[test]
fn non_normalized_keys_follow_the_device_layout_between_presses() {
    let mut injector = recording();
    use_layout(&mut injector, Some(0x0407));
    injector.apply(&non_normalized(0x5a, true)).unwrap();
    use_layout(&mut injector, Some(0x040c));
    assert_eq!(injector.layout.as_ref().map(|l| l.klid()), Some(0x0407));
    injector.apply(&non_normalized(0x5a, false)).unwrap();
    injector.follow_keyboard_layout();
    injector.apply(&non_normalized(0x5a, true)).unwrap();
    injector.apply(&non_normalized(0x5a, false)).unwrap();
    // Normalized keys keep the US table whatever the layout.
    injector.apply(&key(0x5a, true)).unwrap();
    injector.apply(&key(0x5a, false)).unwrap();
    assert_eq!(
        scans(),
        [
            (0x15, SCAN),
            (0x15, UP),
            (0x11, SCAN),
            (0x11, UP),
            (0x2c, SCAN),
            (0x2c, UP)
        ]
    );
    // Without scan codes the layout has nothing to do: the key goes as its VK.
    injector.policy.always_send_scancodes = false;
    injector.apply(&non_normalized(0xde, true)).unwrap();
    injector.apply(&non_normalized(0xde, false)).unwrap();
    assert_eq!(scans(), [(0, 0), (0, KEYEVENTF_KEYUP.0)]);
    // A client that stops reporting leaves keys to the host's layout again.
    use_layout(&mut injector, None);
    assert!(injector.layout.is_none());
}

/// AltGr is the right Alt, an E0 key, on every layout; the key pressed with it gets
/// the device layout's scan code.
#[test]
fn altgr_combinations_use_the_device_layout() {
    let altgr = 0x38;
    let extended = KEYEVENTF_EXTENDEDKEY.0;
    // German AltGr+M types the micro sign, French AltGr+0 the at sign, UK AltGr+4
    // the euro sign.
    for (klid, key, scan) in [
        (0x0407, 0x4d, 0x32),
        (0x040c, 0x30, 0x0b),
        (0x0809, 0x34, 0x05),
    ] {
        let mut injector = recording();
        use_layout(&mut injector, Some(klid));
        for event in [
            non_normalized(0xa5, true),
            non_normalized(key, true),
            non_normalized(key, false),
            non_normalized(0xa5, false),
        ] {
            injector.apply(&event).unwrap();
        }
        assert_eq!(
            scans(),
            [
                (altgr, SCAN | extended),
                (scan, SCAN),
                (scan, UP),
                (altgr, UP | extended)
            ],
            "{klid:08X}"
        );
    }
}

#[test]
fn a_streamed_window_that_went_loses_its_rectangle_and_a_display_keeps_its_last() {
    let old = RECT {
        left: 10,
        top: 20,
        right: 810,
        bottom: 620,
    };
    let moved = RECT {
        left: 0,
        top: 0,
        right: 800,
        bottom: 600,
    };
    let window = r"window:0x10010/1234@\\.\DISPLAY1";
    assert_eq!(
        reread_rect(window, Some(old), None),
        None,
        "absolute input waits"
    );
    assert_eq!(reread_rect(window, Some(old), Some(moved)), Some(moved));
    assert_eq!(reread_rect(r"\\.\DISPLAY1", Some(old), None), Some(old));
    assert_eq!(
        reread_rect(r"\\.\DISPLAY1", Some(old), Some(moved)),
        Some(moved)
    );
}

#[test]
fn a_streamed_window_is_followed_more_often_than_a_display_and_a_closed_one_takes_no_input() {
    let window = r"window:0x7fff0010/4@\.\DISPLAY1";
    assert_eq!(rect_interval(window), WINDOW_RECT_INTERVAL);
    assert_eq!(rect_interval(r"\.\DISPLAY1"), RECT_INTERVAL);
    assert!(WINDOW_RECT_INTERVAL < RECT_INTERVAL);
    // No such window: absolute input waits instead of landing on a display.
    assert!(display_rect(window).is_err());
    assert!(current_rect(window).is_none());
}

#[test]
fn a_touchpad_that_does_not_work_is_reported_once_per_reason() {
    let mut injector = recording();
    injector.touchpad_unavailable("touchpad_failed", "it failed");
    let codes = |injector: &Injector| {
        injector
            .warnings
            .snapshot()
            .into_iter()
            .map(|w| w.code)
            .collect::<Vec<_>>()
    };
    assert_eq!(codes(&injector), ["touchpad_failed"]);
    // Later packets neither look the warnings up nor set it again.
    injector.warnings.clear("touchpad_failed");
    for _ in 0..1000 {
        injector.touchpad_unavailable("touchpad_failed", "it failed");
    }
    assert!(codes(&injector).is_empty());
    // Another reason is reported.
    injector.touchpad_unavailable("touchpad_not_precision", "not precision");
    assert_eq!(codes(&injector), ["touchpad_not_precision"]);
}

#[test]
fn making_an_injector_starts_no_controller_thread_until_a_controller_needs_one() {
    let mut injector = Injector::new(r"\\.\DISPLAY99", "vhf").unwrap();
    assert!(!injector.has_gamepads());
    // Nothing that only follows the controllers starts their thread either.
    injector.apply(&Event::Haptics(false)).unwrap();
    injector.set_feedback_waker(std::sync::Arc::new(
        std::net::UdpSocket::bind("127.0.0.1:0").unwrap(),
    ));
    injector.controller_audio(0, 0, &[]);
    assert!(!injector.controller_audio_ready(0));
    assert_eq!(injector.gamepad_reports().count(), 0);
    injector.release_all();
    injector.detach_gamepads();
    assert!(!injector.has_gamepads());
    assert!(!injector.haptics);
}

#[test]
fn display_does_not_block_on_rename_lookup_when_rect_is_known() {
    let mut injector = recording();
    let current = RECT {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };
    injector.rect = Some(current);
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    injector.lookup = Some(Lookup {
        output: r"\\.\DISPLAY98".into(),
        started: std::time::Instant::now(),
        thread: Some(std::thread::spawn(move || {
            let _ = rx.recv_timeout(Duration::from_secs(5));
            Ok(current)
        })),
    });
    let started = std::time::Instant::now();
    assert_eq!(injector.display(), Some(current));
    assert!(started.elapsed() < Duration::from_millis(100));
    assert!(injector.lookup.as_ref().unwrap().thread.is_some());
    drop(tx);
    injector
        .lookup
        .take()
        .unwrap()
        .thread
        .unwrap()
        .join()
        .unwrap()
        .unwrap();
}
