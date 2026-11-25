//! Unit tests for [`Injector`] startup, deadlines, touch profiles, deferred clicks, and ordering.

use super::*;

mod batching_held;
mod display_layout;

/// Serializes tests that exercise process-global `HELD` entries for mouse button 1
/// (`(false, 1)` and `(false, 1 | VIRTUAL_BUTTON_FLAG)`).
pub(super) static BUTTON1_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn input_starts_before_the_stream_display_exists_and_ignores_absolute_input() {
    // Keyboard and controller input, including a controller's one-time
    // arrival, needs an injector before the stream's display exists.
    let mut injector = Injector::new(r"\\.\DISPLAY99", "vhf").unwrap();
    // Nothing below may reach Windows: there is no display to map onto.
    assert!(injector.rect.is_none());
    for event in [
        Event::Absolute {
            x: 10,
            y: 20,
            width: 100,
            height: 100,
        },
        Event::Touch {
            event: 1,
            id: 7,
            x: 0.5,
            y: 0.5,
            pressure: 0.5,
            major: 0.1,
            minor: 0.1,
            rotation: u16::MAX,
        },
        Event::Pen {
            event: 1,
            tool: 1,
            buttons: 0,
            x: 0.5,
            y: 0.5,
            pressure: 0.5,
            rotation: u16::MAX,
            tilt: u8::MAX,
            roll: u16::MAX,
        },
    ] {
        injector.apply(&event).unwrap();
    }
    assert!(injector.absolute);
    assert!(injector.touches.is_empty());
    assert!(injector.touch_device.is_none() && injector.pen_device.is_none());
    assert_eq!(injector.pen.pointerInfo.pointerFlags, POINTER_FLAG_NONE);
    // The session's display is followed while it is still missing.
    injector.set_output(r"\\.\DISPLAY98");
    assert!(injector.rect.is_none());
    assert_eq!(injector.output, r"\\.\DISPLAY98");
    assert_eq!(injector.lookup.as_ref().unwrap().output, r"\\.\DISPLAY98");
}

thread_local! {
    static CALLS: std::cell::RefCell<Vec<Vec<INPUT>>> = const { std::cell::RefCell::new(Vec::new()) };
    /// How many inputs of a call Windows takes.
    static TAKES: std::cell::Cell<usize> = const { std::cell::Cell::new(usize::MAX) };
}
/// SendInput's stand-in: nothing reaches the desktop.
fn record(inputs: &[INPUT]) -> usize {
    CALLS.with_borrow_mut(|calls| calls.push(inputs.to_vec()));
    TAKES.get().min(inputs.len())
}
fn recording() -> Injector {
    let mut injector = Injector::new(r"\\.\DISPLAY99", "vhf").unwrap();
    injector.inject = record;
    injector
}
#[test]
fn control_wait_tracks_release_and_repeat_deadlines() {
    let mut injector = recording();
    assert_eq!(injector.next_due(), None);
    injector.apply(&key(0x51, true)).unwrap();
    let repeat = injector.repeat.unwrap().3;
    assert_eq!(injector.next_due(), Some(repeat));
    let release = Instant::now() + Duration::from_millis(10);
    injector.left_release = Some(release);
    assert_eq!(injector.next_due(), Some(release.min(repeat)));
    injector.apply(&key(0x51, false)).unwrap();
    assert_eq!(injector.next_due(), Some(release));
    injector.release_all();
    assert_eq!(injector.next_due(), None);
    calls();
}
#[test]
fn rumble_feedback_obeys_forward_rumble_and_client_haptics() {
    let mut injector = recording();
    for forward in [false, true] {
        injector.policy.forward_rumble = forward;
        for haptics in [false, true] {
            injector.apply(&Event::Haptics(haptics)).unwrap();
            for kind in [0x010b, 0x5500, 0x5503, 0x5504, 0x5601] {
                assert_eq!(injector.feedback_allowed(kind), forward && haptics);
            }
            assert!(injector.feedback_allowed(0x5502));
            assert!(injector.feedback_allowed(0x5505), "Steam settings");
        }
    }
}
fn describe(input: &INPUT) -> String {
    // SAFETY: The test inputs are keyboard or mouse INPUTs with the union field matching their type initialized.
    unsafe {
        if input.r#type == INPUT_KEYBOARD {
            let ki = input.Anonymous.ki;
            format!("key {} {} {:#x}", ki.wVk.0, ki.wScan, ki.dwFlags.0)
        } else {
            let mi = input.Anonymous.mi;
            format!(
                "mouse {} {} {} {:#x}",
                mi.dx, mi.dy, mi.mouseData, mi.dwFlags.0
            )
        }
    }
}
/// The SendInput calls made since the last look.
fn calls() -> Vec<Vec<String>> {
    CALLS
        .with_borrow_mut(std::mem::take)
        .iter()
        .map(|call| call.iter().map(describe).collect())
        .collect()
}
fn described(inputs: &[INPUT]) -> Vec<String> {
    inputs.iter().map(describe).collect()
}
fn key(key: u16, down: bool) -> Event {
    Event::Keyboard {
        key,
        modifiers: 0,
        flags: 0,
        down,
    }
}
fn click(button: u8, down: bool) -> Event {
    Event::MouseButton { button, down }
}
#[test]
fn a_trackpad_touch_profile_sends_touches_as_mouse_input() {
    use butterpollo_config::config::TouchMode;
    use butterpollo_input_policy::touch_profile::Settings;
    let touch = |event, x| Event::Touch {
        event,
        id: 4,
        x,
        y: 0.5,
        pressure: 1.0,
        major: 0.0,
        minor: 0.0,
        rotation: u16::MAX,
    };
    let mut injector = recording();
    injector.set_stream_size(1000, 500);
    injector.policy.touch = Settings {
        mode: TouchMode::Trackpad,
        sensitivity: 100,
        gestures: true,
    };
    // A swipe of 0.125 of the stream's 1000 pixels moves the mouse, a tap clicks.
    let events = [touch(1, 0.25), touch(3, 0.375), touch(2, 0.375)];
    assert!(injector.apply_all(&events).is_empty());
    assert_eq!(calls(), [vec!["mouse 125 0 0 0x1".to_owned()]]);
    assert!(
        injector
            .apply_all(&[touch(1, 0.375), touch(2, 0.375)])
            .is_empty()
    );
    assert_eq!(
        calls().concat(),
        [
            described(&[Injector::button(1, true)]),
            described(&[Injector::button(1, false)])
        ]
        .concat()
    );
    assert!(injector.touches.is_empty(), "no native contact");
    // With native touch off the device emulates the mouse itself: no touch comes here.
    injector.policy.native_pen_touch = false;
    assert!(!injector.policy.allows(&touch(1, 0.25)));
}
#[test]
fn relative_mouse_releases_a_deferred_absolute_click_before_moving() {
    let _button1 = BUTTON1_LOCK.lock().unwrap();
    for batched in [false, true] {
        let mut injector = recording();
        injector.lookup = None;
        injector.rect = Some(RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        });
        let absolute = Event::Absolute {
            x: 50,
            y: 50,
            width: 100,
            height: 100,
        };
        let relative = Event::Relative { x: 3, y: 2 };
        injector.apply(&absolute).unwrap();
        let absolute_input = calls().into_iter().flatten().collect::<Vec<_>>();
        let events = [
            absolute.clone(),
            click(1, true),
            click(1, false),
            absolute.clone(),
            relative.clone(),
            click(1, true),
            absolute,
            click(1, false),
        ];
        if batched {
            assert!(injector.apply_all(&events).is_empty());
        } else {
            for event in &events {
                injector.apply(event).unwrap();
            }
        }
        // A second absolute release must not overwrite ownership retained by the first.
        *injector.left_release.as_mut().unwrap() = std::time::Instant::now();
        injector.due().unwrap();
        let first = calls().into_iter().flatten().collect::<Vec<_>>();
        injector.release_pointers().unwrap();
        let cleanup = calls();
        let retained = HELD.lock().unwrap().get(&(false, 1)).copied();
        injector.apply(&relative).unwrap();
        calls();
        injector.apply(&click(1, true)).unwrap();
        injector.apply(&click(1, false)).unwrap();
        let subsequent = calls().into_iter().flatten().collect::<Vec<_>>();
        let mut expected = absolute_input.clone();
        expected.extend(described(&[Injector::button(1, true)]));
        expected.extend(absolute_input.clone());
        expected.extend(described(&[
            Injector::button(1, false),
            Injector::mouse(3, 2, 0, MOUSEEVENTF_MOVE),
            Injector::button(1, true),
        ]));
        expected.extend(absolute_input);
        expected.extend(described(&[Injector::button(1, false)]));
        assert_eq!(
            first, expected,
            "batched={batched}; cleanup={cleanup:?}; retained={retained:?}; subsequent={subsequent:?}"
        );
        assert!(cleanup.is_empty());
        assert_eq!(retained, None);
        assert_eq!(
            subsequent,
            described(&[Injector::button(1, true), Injector::button(1, false)])
        );
        assert!(!HELD.lock().unwrap().contains_key(&(false, 1)));
    }

    let relative = Event::Relative { x: 3, y: 2 };
    let movement = Injector::mouse(3, 2, 0, MOUSEEVENTF_MOVE);
    let mut first = recording();
    let mut second = recording();
    first.absolute = true;
    first.apply(&click(1, true)).unwrap();
    first.apply(&click(1, false)).unwrap();
    assert!(first.left_release.is_some());
    second.apply(&click(1, true)).unwrap();
    calls();
    first.apply(&relative).unwrap();
    assert!(first.left_release.is_none());
    assert_eq!(calls(), [described(&[movement])]);
    assert_eq!(HELD.lock().unwrap().get(&(false, 1)), Some(&1));
    first.release_pointers().unwrap();
    assert!(calls().is_empty());
    second.apply(&click(1, false)).unwrap();
    assert_eq!(calls(), [described(&[Injector::button(1, false)])]);
    assert!(!HELD.lock().unwrap().contains_key(&(false, 1)));

    first.absolute = true;
    first.apply(&click(1, true)).unwrap();
    first.apply(&click(1, false)).unwrap();
    calls();
    TAKES.set(0);
    let errors = first.apply_all(std::slice::from_ref(&relative));
    TAKES.set(usize::MAX);
    assert_eq!(errors.len(), 2);
    assert_eq!(
        calls(),
        [
            described(&[Injector::button(1, false)]),
            described(&[movement]),
        ]
    );
    assert!(!first.absolute && first.left_release.is_none());
    assert!(!HELD.lock().unwrap().contains_key(&(false, 1)));
    first.apply(&click(1, true)).unwrap();
    first.apply(&click(1, false)).unwrap();
    assert_eq!(
        calls(),
        [
            described(&[Injector::button(1, true)]),
            described(&[Injector::button(1, false)]),
        ]
    );
    assert!(!HELD.lock().unwrap().contains_key(&(false, 1)));

    // Here, as the shared left button's other checks: with
    // absolute_left_release_delay = 0 an absolute click releases at once.
    first.policy.left_release_delay = Duration::ZERO;
    first.apply(&absolute_move()).unwrap();
    calls();
    first.apply(&click(1, true)).unwrap();
    first.apply(&click(1, false)).unwrap();
    assert!(first.absolute && first.left_release.is_none());
    assert_eq!(
        calls(),
        [
            described(&[Injector::button(1, true)]),
            described(&[Injector::button(1, false)]),
        ]
    );
    assert!(!HELD.lock().unwrap().contains_key(&(false, 1)));
}
/// An absolute move, which sets absolute mode even before the display exists.
fn absolute_move() -> Event {
    Event::Absolute {
        x: 50,
        y: 50,
        width: 100,
        height: 100,
    }
}
// HELD is shared by every injector: each test below uses its own keys
// and buttons.

thread_local! {
    /// Whether HELD was free while the stand-in below sent.
    static HELD_FREE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}
/// SendInput's stand-in that notes whether HELD was free meanwhile. Another
/// test's thread holds it only briefly, so a few tries tell it from this one.
fn record_held_free(inputs: &[INPUT]) -> usize {
    let free = (0..200).any(|_| {
        let free = HELD.try_lock().is_ok();
        if !free {
            std::thread::sleep(Duration::from_millis(1));
        }
        free
    });
    HELD_FREE.set(Some(free));
    record(inputs)
}
#[test]
fn a_delayed_release_is_sent_after_the_shared_key_lock_is_released() {
    let mut injector = recording();
    injector.apply(&click(6, true)).unwrap();
    calls();
    injector.inject = record_held_free;
    injector
        .held(false, 6, false, true, Injector::button(6, false))
        .unwrap();
    assert_eq!(HELD_FREE.get(), Some(true));
    assert_eq!(calls(), [described(&[Injector::button(6, false)])]);
    assert!(!HELD.lock().unwrap().contains_key(&(false, 6)));
    // A press Windows refuses is still not held.
    TAKES.set(0);
    assert!(
        injector
            .held(false, 6, true, false, Injector::button(6, true))
            .is_err()
    );
    TAKES.set(usize::MAX);
    assert!(!HELD.lock().unwrap().contains_key(&(false, 6)));
    injector.buttons.remove(&6);
    calls();
}

/// What the stand-ins below sent, across the test's two threads.
static SENDS: std::sync::Mutex<Vec<&str>> = std::sync::Mutex::new(Vec::new());
static HELD_SENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// A delayed release's SendInput that takes its time.
fn slow_held_send(inputs: &[INPUT]) -> usize {
    HELD_SENDING.store(true, std::sync::atomic::Ordering::Release);
    std::thread::sleep(Duration::from_millis(100));
    SENDS.lock().unwrap().push("held");
    inputs.len()
}
fn pass_send(inputs: &[INPUT]) -> usize {
    SENDS.lock().unwrap().push("pass");
    inputs.len()
}
#[test]
fn another_injectors_pass_is_sent_after_a_delayed_release_it_followed() {
    let mut injector = recording();
    injector.apply(&click(7, true)).unwrap();
    calls();
    injector.inject = slow_held_send;
    let other = std::thread::spawn(|| {
        let mut other = recording();
        other.inject = pass_send;
        while !HELD_SENDING.load(std::sync::atomic::Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            other
                .apply_all(&[Event::Relative { x: 1, y: 0 }])
                .is_empty()
        );
    });
    injector
        .held(false, 7, false, true, Injector::button(7, false))
        .unwrap();
    other.join().unwrap();
    assert_eq!(*SENDS.lock().unwrap(), ["held", "pass"]);
    injector.buttons.remove(&7);
}

thread_local! {
    /// Touch frames sent: each contact's x and flags.
    static FRAMES: std::cell::RefCell<Vec<Vec<(i32, POINTER_FLAGS)>>> = const { std::cell::RefCell::new(Vec::new()) };
}
/// `inject_pointer`'s stand-in: nothing reaches the desktop.
unsafe fn record_touch(_: HSYNTHETICPOINTERDEVICE, frame: &[POINTER_TYPE_INFO]) -> Result<()> {
    let frame = frame
        .iter()
        // SAFETY: The injector sends touch frames only.
        .map(|contact| unsafe { contact.Anonymous.touchInfo.pointerInfo })
        .map(|info| (info.ptPixelLocation.x, info.pointerFlags))
        .collect();
    FRAMES.with_borrow_mut(|frames| frames.push(frame));
    Ok(())
}
fn frames() -> Vec<Vec<(i32, POINTER_FLAGS)>> {
    FRAMES.take()
}
fn finger(event: u8, id: u32, x: f32) -> Event {
    Event::Touch {
        event,
        id,
        x,
        y: 0.5,
        pressure: 0.5,
        major: 0.,
        minor: 0.,
        rotation: u16::MAX,
    }
}
#[test]
fn touch_moves_wait_for_the_end_of_the_pass_or_the_next_other_event() {
    let mut injector = recording();
    injector.lookup = None;
    injector.rect = Some(RECT {
        left: 0,
        top: 0,
        right: 1000,
        bottom: 1000,
    });
    injector.inject_touch = record_touch;
    injector.touch_device = Some(HSYNTHETICPOINTERDEVICE::default());
    assert!(
        injector
            .apply_all(&[finger(1, 1, 0.2), finger(1, 2, 0.6)])
            .is_empty()
    );
    let down = frames();
    assert_eq!(down.len(), 2);
    let a = down[0][0].0;
    // Moves in turn are one frame, sent at the end of the pass, with each finger
    // where it was last: A moved 0.2 and B 0.2 of the 999-pixel span.
    assert!(
        injector
            .apply_all(&[
                finger(3, 1, 0.3),
                finger(3, 2, 0.7),
                finger(3, 1, 0.4),
                finger(3, 2, 0.8),
            ])
            .is_empty()
    );
    let moved = frames();
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].len(), 2);
    assert!((199..=200).contains(&(moved[0][0].0 - a)), "{moved:?}");
    assert!(
        (399..=400).contains(&(moved[0][1].0 - moved[0][0].0)),
        "{moved:?}"
    );
    // Mouse input sends the moves before it.
    assert!(
        injector
            .apply_all(&[
                finger(3, 1, 0.5),
                Event::Relative { x: 1, y: 0 },
                finger(3, 1, 0.6)
            ])
            .is_empty()
    );
    assert_eq!(frames().len(), 2);
    assert_eq!(calls().len(), 1);
    // So does a lift, whose frame holds the other finger where it moved.
    assert!(
        injector
            .apply_all(&[finger(3, 1, 0.7), finger(2, 2, 0.8)])
            .is_empty()
    );
    let lifted = frames();
    assert_eq!(lifted.len(), 2);
    assert!(
        lifted[0]
            .iter()
            .all(|(_, flags)| flags.contains(POINTER_FLAG_UPDATE))
    );
    assert_eq!(lifted[1][0], lifted[0][0]);
    assert!(lifted[1][1].1.contains(POINTER_FLAG_UP));
    // The fake device is never destroyed.
    injector.touch_device = None;
    injector.touches.clear();
}
