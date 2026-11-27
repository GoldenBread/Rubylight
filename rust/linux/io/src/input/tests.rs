//! Tests of input injection that need a gamescope session or `/dev/uinput`, all `#[ignore]`d:
//! run them with `cargo test -p butterpollo-linux-io input:: -- --ignored --test-threads=1`.
//! The X client runs as this test binary again, started inside gamescope, and records the
//! events it receives in a file.

use super::{Event, Injector};
use std::{
    collections::BTreeMap,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};
use x11rb::{
    connection::Connection,
    protocol::{
        Event as XEvent,
        xproto::{
            AtomEnum, ConnectionExt as _, CreateGCAux, CreateWindowAux, EventMask, PropMode,
            Rectangle, WindowClass,
        },
    },
    wrapper::ConnectionExt as _,
};

const CLIENT_OUT: &str = "RUBYLIGHT_X_CLIENT_OUT";

/// The X client: a window over the whole screen that appends each key, button and motion
/// event to the file named by `RUBYLIGHT_X_CLIENT_OUT`. A test of its own only so the test
/// binary can start it inside gamescope; run by itself it does nothing.
#[test]
#[ignore = "started by the gamescope test"]
fn x_client() {
    let Some(out) = std::env::var_os(CLIENT_OUT) else {
        return;
    };
    let mut log = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(out)
        .unwrap();
    let write = |log: &mut std::fs::File, line: String| {
        use std::io::Write;
        writeln!(log, "{line}").unwrap();
    };
    let (conn, screen) = x11rb::connect(None).unwrap();
    let root = conn.setup().roots[screen].clone();
    let window = conn.generate_id().unwrap();
    conn.create_window(
        root.root_depth,
        window,
        root.root,
        0,
        0,
        root.width_in_pixels,
        root.height_in_pixels,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new().event_mask(
            EventMask::KEY_PRESS
                | EventMask::KEY_RELEASE
                | EventMask::BUTTON_PRESS
                | EventMask::BUTTON_RELEASE
                | EventMask::POINTER_MOTION
                | EventMask::EXPOSURE,
        ),
    )
    .unwrap();
    conn.change_property8(
        PropMode::REPLACE,
        window,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        b"input test",
    )
    .unwrap();
    conn.map_window(window).unwrap();
    conn.flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(40);
    while Instant::now() < deadline {
        let Some(event) = conn.poll_for_event().unwrap() else {
            std::thread::sleep(Duration::from_millis(10));
            continue;
        };
        let line = match event {
            XEvent::Expose(_) => {
                // A window with no picture is not focused by gamescope, so it paints.
                let gc = conn.generate_id().unwrap();
                conn.create_gc(gc, window, &CreateGCAux::new().foreground(root.white_pixel))
                    .unwrap();
                conn.poly_fill_rectangle(
                    window,
                    gc,
                    &[Rectangle {
                        x: 0,
                        y: 0,
                        width: root.width_in_pixels,
                        height: root.height_in_pixels,
                    }],
                )
                .unwrap();
                conn.free_gc(gc).unwrap();
                conn.flush().unwrap();
                "ready".to_owned()
            }
            XEvent::KeyPress(e) => format!("key down {}", e.detail),
            XEvent::KeyRelease(e) => format!("key up {}", e.detail),
            XEvent::ButtonPress(e) => format!("button down {}", e.detail),
            XEvent::ButtonRelease(e) => format!("button up {}", e.detail),
            XEvent::MotionNotify(e) => format!("motion {} {}", e.event_x, e.event_y),
            _ => continue,
        };
        write(&mut log, line);
    }
}

fn wait_for(out: &Path, what: &str, timeout: Duration) -> String {
    let deadline = Instant::now() + timeout;
    loop {
        let text = std::fs::read_to_string(out).unwrap_or_default();
        if text.lines().any(|line| line.starts_with(what)) {
            return text;
        }
        assert!(
            Instant::now() < deadline,
            "no {what:?} in the client's events; it saw:\n{text}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Keyboard and mouse injected through XTEST reach an X client in a headless gamescope.
#[test]
#[ignore = "starts gamescope"]
fn xtest_events_reach_an_x_client() {
    let out = std::env::temp_dir().join(format!("rubylight-x-client-{}", std::process::id()));
    let _ = std::fs::remove_file(&out);
    let exe = std::env::current_exe().unwrap();
    let command = format!(
        "{CLIENT_OUT}='{}' '{}' --exact input::tests::x_client --ignored --nocapture --test-threads=1",
        out.display(),
        exe.display()
    );
    let environment = BTreeMap::from([
        ("SUNSHINE_CLIENT_WIDTH".to_owned(), "640".to_owned()),
        ("SUNSHINE_CLIENT_HEIGHT".to_owned(), "480".to_owned()),
    ]);
    let (shell, folder) = crate::gamescope::wrap(&command, &environment).unwrap();
    let mut gamescope = Command::new("sh").arg("-c").arg(&shell).spawn().unwrap();
    struct Stop(std::process::Child, std::path::PathBuf);
    impl Drop for Stop {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
            let _ = std::fs::remove_dir_all(&self.1);
        }
    }
    let _ = &mut gamescope;
    let _stop = Stop(gamescope, folder);
    wait_for(&out, "ready", Duration::from_secs(30));
    let mut injector = Injector::new("", "").unwrap();
    injector.set_stream_size(1920, 1080);
    // Other sessions may run beside this one: input goes to this test's own.
    let session = loop {
        if let Some(session) = crate::gamescope::read(&_stop.1) {
            break session;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    injector.keyboard_mouse.display = session.display;
    let keys = [
        Event::Keyboard {
            key: 0x41,
            modifiers: 0,
            flags: 0,
            down: true,
        },
        Event::Keyboard {
            key: 0x41,
            modifiers: 0,
            flags: 0,
            down: false,
        },
        Event::Absolute {
            x: 320,
            y: 240,
            width: 640,
            height: 480,
        },
        Event::Relative { x: 10, y: 5 },
        Event::MouseButton {
            button: 1,
            down: true,
        },
        Event::MouseButton {
            button: 1,
            down: false,
        },
        Event::Scroll {
            amount: 120,
            horizontal: false,
        },
    ];
    // The injector finds the session within a second; input before that is dropped.
    let start = Instant::now();
    loop {
        let errors = injector.apply_all(&keys);
        assert!(errors.is_empty(), "{errors:?}");
        if std::fs::read_to_string(&out)
            .unwrap_or_default()
            .contains("key down")
        {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "no key arrived; warnings {:?}, client saw:\n{}",
            injector.warnings.snapshot(),
            std::fs::read_to_string(&out).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(300));
    }
    let errors = injector.apply_all(&keys[2..]);
    assert!(errors.is_empty(), "{errors:?}");
    let text = wait_for(&out, "button up 4", Duration::from_secs(10));
    println!("the X client saw:\n{text}");
    // The keycode of "a" depends on the keyboard layout of the X server.
    let code = text
        .lines()
        .find_map(|line| line.strip_prefix("key down "))
        .expect("a key press");
    assert!(text.contains(&format!("key up {code}")), "{text}");
    assert!(text.contains("motion 319 239"), "{text}");
    assert!(text.contains("motion 329 244"), "{text}");
    assert!(text.contains("button down 1"), "{text}");
    assert!(text.contains("button up 1"), "{text}");
    assert!(text.contains("button down 4"), "{text}");
}

/// A virtual pad appears as an evdev device and reports a button and a stick.
#[test]
#[ignore = "needs write access to /dev/uinput and read access to /dev/input"]
fn uinput_pad_reports_input() {
    use std::io::Read;
    let warnings = std::sync::Arc::new(butterpollo_session::session::Warnings::default());
    let pads = super::GamepadThread::new(
        0,
        butterpollo_input_policy::input_policy::Policy::resolve(
            &butterpollo_config::config::Config::default(),
        )
        .unwrap(),
        warnings.clone(),
    )
    .unwrap();
    pads.send(Event::Arrival {
        id: 0,
        kind: 1,
        capabilities: 0,
        buttons: 0,
    });
    assert_eq!(pads.plugged_pads(), 1, "{:?}", warnings.snapshot());
    let sysname = pads.sysname(0).unwrap();
    // The event node appears under the device's sysfs folder once udev has seen it.
    let sys = Path::new("/sys/devices/virtual/input").join(&sysname);
    let deadline = Instant::now() + Duration::from_secs(5);
    let node = loop {
        let found = std::fs::read_dir(&sys).ok().and_then(|dir| {
            dir.flatten()
                .find(|e| e.file_name().to_string_lossy().starts_with("event"))
                .map(|e| Path::new("/dev/input").join(e.file_name()))
        });
        if let Some(node) = found.filter(|node| std::fs::File::open(node).is_ok()) {
            break node;
        }
        assert!(
            Instant::now() < deadline,
            "no readable event node for {sysname}"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    let mut device = std::fs::File::open(&node).unwrap();
    let mut id = [0u16; 4];
    let eviocgid = (2u64 << 30) | (8 << 16) | (u64::from(b'E') << 8) | 2;
    assert_eq!(
        // SAFETY: EVIOCGID writes an `input_id` (4 u16) into the buffer.
        unsafe {
            libc::ioctl(
                std::os::fd::AsRawFd::as_raw_fd(&device),
                eviocgid as _,
                id.as_mut_ptr(),
            )
        },
        0
    );
    assert_eq!((id[1], id[2]), (0x045e, 0x028e));
    // Make the device non-blocking so the drain below stops.
    // SAFETY: `fd` is the open event device; F_SETFL takes an integer.
    unsafe {
        let fd = std::os::fd::AsRawFd::as_raw_fd(&device);
        libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK);
    }
    pads.send(Event::Controller {
        id: 0,
        active: 1,
        buttons: 0x1000 | 0x8,
        left_trigger: 200,
        right_trigger: 0,
        sticks: [1000, 2000, 0, 0],
    });
    std::thread::sleep(Duration::from_millis(100));
    let mut seen = Vec::new();
    let mut buf = [0u8; 24 * 32];
    while let Ok(n) = device.read(&mut buf) {
        if n == 0 {
            break;
        }
        for chunk in buf[..n].as_chunks::<24>().0 {
            let kind = u16::from_ne_bytes([chunk[16], chunk[17]]);
            let code = u16::from_ne_bytes([chunk[18], chunk[19]]);
            let value = i32::from_ne_bytes(chunk[20..24].try_into().unwrap());
            seen.push((kind, code, value));
        }
    }
    println!("events: {seen:?}");
    assert!(seen.contains(&(1, 0x130, 1)), "A button: {seen:?}");
    assert!(seen.contains(&(3, 0x10, 1)), "d-pad right: {seen:?}");
    assert!(seen.contains(&(3, 0x02, 200)), "left trigger: {seen:?}");
    assert!(seen.contains(&(3, 0x00, 1000)), "stick X: {seen:?}");
    assert!(seen.contains(&(3, 0x01, -2000)), "stick Y: {seen:?}");
    pads.send(Event::Controller {
        id: 0,
        active: 0,
        buttons: 0,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    });
    assert_eq!(pads.plugged_pads(), 0);
}

/// Types `RUBYLIGHT_LIVE_TYPE` (letters, digits and spaces; default `rubylight`), then Enter
/// when `RUBYLIGHT_LIVE_ENTER` is set, into whatever the seat shows, through the desktop
/// devices a stream to `RUBYLIGHT_LIVE_TARGET` (default `kms`) makes: the login screen's
/// password field, for one. Needs write access to `/dev/uinput`.
#[test]
#[ignore = "types into the seat's screen; needs write access to /dev/uinput"]
fn live_typing_reaches_the_seat() {
    let target = std::env::var("RUBYLIGHT_LIVE_TARGET").unwrap_or("kms".into());
    let text = std::env::var("RUBYLIGHT_LIVE_TYPE").unwrap_or("rubylight".into());
    let mut injector = Injector::new(&target, "").unwrap();
    // The compositor takes a moment to notice new devices.
    std::thread::sleep(Duration::from_millis(1500));
    let mut press = |key: u16, modifiers: u8| {
        for down in [true, false] {
            injector
                .apply(&Event::Keyboard {
                    key,
                    modifiers,
                    flags: 0,
                    down,
                })
                .unwrap();
            std::thread::sleep(Duration::from_millis(30));
        }
    };
    for c in text.chars() {
        match c {
            'a'..='z' => press(c.to_ascii_uppercase() as u16, 0),
            'A'..='Z' => press(c as u16, 0x01),
            '0'..='9' | ' ' => press(c as u16, 0),
            other => panic!("{other:?} is not typed by this test"),
        }
    }
    if std::env::var_os("RUBYLIGHT_LIVE_ENTER").is_some() {
        press(0x0d, 0);
    }
    std::thread::sleep(Duration::from_millis(300));
}
