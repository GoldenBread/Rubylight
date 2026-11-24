//! Tests of the virtual pads against the kernel, all `#[ignore]`d: they need write access to
//! `/dev/uinput` and `/dev/uhid` and read access to `/dev/input`. Run them with
//! `cargo test -p butterpollo-linux-io input::pad_tests -- --ignored --test-threads=1`.
//! They play a game: open the pad's event nodes, read what the host sends, and upload and play
//! rumble effects on them, then look for the rumble the client would be sent.

use super::{Event, GamepadThread, PadReport};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub(super) fn pads() -> (
    GamepadThread,
    std::sync::Arc<butterpollo_session::session::Warnings>,
) {
    pads_with(0, |_| {})
}

/// Pads for the `gamepad` setting `profile`, with the input policy changed by `adjust`.
pub(super) fn pads_with(
    profile: u16,
    adjust: impl FnOnce(&mut butterpollo_input_policy::input_policy::Policy),
) -> (
    GamepadThread,
    std::sync::Arc<butterpollo_session::session::Warnings>,
) {
    let warnings = std::sync::Arc::new(butterpollo_session::session::Warnings::default());
    let mut policy = butterpollo_input_policy::input_policy::Policy::resolve(
        &butterpollo_config::config::Config::default(),
    )
    .unwrap();
    adjust(&mut policy);
    (
        GamepadThread::new(profile, policy, warnings.clone()).unwrap(),
        warnings,
    )
}

/// An event the kernel delivered: type, code, value.
pub(super) type Ev = (u16, u16, i32);

pub(super) fn read_events(device: &mut File) -> Vec<Ev> {
    let mut seen = Vec::new();
    let mut buf = [0u8; 24 * 64];
    while let Ok(n) = device.read(&mut buf) {
        if n == 0 {
            break;
        }
        for chunk in buf[..n].as_chunks::<24>().0 {
            seen.push((
                u16::from_ne_bytes([chunk[16], chunk[17]]),
                u16::from_ne_bytes([chunk[18], chunk[19]]),
                i32::from_ne_bytes(chunk[20..24].try_into().unwrap()),
            ));
        }
    }
    seen
}

pub(super) fn nonblocking(file: &File) {
    // SAFETY: `file` is open; F_SETFL takes an integer.
    unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) };
}

/// Opens an event node as a game does, waiting for udev to make it accessible.
pub(super) fn open_event(sys_dir: &Path) -> File {
    try_open_event(sys_dir, Duration::from_secs(8))
        .unwrap_or_else(|| panic!("no accessible event node in {}", sys_dir.display()))
}

/// The event node of the input device `sys_dir`, if udev makes it accessible within `wait`.
/// Only joystick devices get an ACL for the logged-in user; the rest need the `input` group.
pub(super) fn try_open_event(sys_dir: &Path, wait: Duration) -> Option<File> {
    let deadline = Instant::now() + wait;
    loop {
        let found = std::fs::read_dir(sys_dir).ok().and_then(|dir| {
            dir.flatten()
                .find(|e| e.file_name().to_string_lossy().starts_with("event"))
                .map(|e| Path::new("/dev/input").join(e.file_name()))
        });
        if let Some(file) =
            found.and_then(|node| OpenOptions::new().read(true).write(true).open(node).ok())
        {
            nonblocking(&file);
            return Some(file);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The legacy joystick node (`/dev/input/jsN`, readable by all) of an input device, whose
/// events read as `(3, axis, value)` and `(1, button, value)` like an event node's when the
/// device's axes and buttons are numbered from 0 in order. A first read also reports the
/// device's state at that time (joydev's initial events).
pub(super) struct Joydev(File);
impl Joydev {
    pub(super) fn open(sys_dir: &Path) -> Option<Self> {
        let name = std::fs::read_dir(sys_dir)
            .ok()?
            .flatten()
            .find(|e| e.file_name().to_string_lossy().starts_with("js"))?
            .file_name();
        let file = File::open(Path::new("/dev/input").join(name)).ok()?;
        nonblocking(&file);
        Some(Self(file))
    }
    pub(super) fn read(&mut self) -> Vec<Ev> {
        let mut seen = Vec::new();
        let mut buf = [0u8; 8 * 64];
        while let Ok(n) = self.0.read(&mut buf) {
            if n == 0 {
                break;
            }
            for chunk in buf[..n].as_chunks::<8>().0 {
                let value = i32::from(i16::from_ne_bytes([chunk[4], chunk[5]]));
                let (kind, number) = (chunk[6] & 0x7f, u16::from(chunk[7]));
                if matches!(kind, 1 | 2) {
                    seen.push((if kind == 1 { 1 } else { 3 }, number, value));
                }
            }
        }
        seen
    }
}

const fn eviocsff() -> libc::c_ulong {
    ((1 << 30) | (48 << 16) | ((b'E' as u32) << 8) | 0x80) as libc::c_ulong
}
const fn eviocrmff() -> libc::c_ulong {
    ((1 << 30) | (4 << 16) | ((b'E' as u32) << 8) | 0x81) as libc::c_ulong
}

/// Uploads a rumble effect to the event node as a game does and returns its id.
pub(super) fn upload_rumble(device: &File, strong: u16, weak: u16, length_ms: u16) -> i16 {
    let mut effect = [0u8; 48];
    effect[..2].copy_from_slice(&0x50u16.to_ne_bytes());
    effect[2..4].copy_from_slice(&(-1i16).to_ne_bytes());
    effect[10..12].copy_from_slice(&length_ms.to_ne_bytes());
    effect[16..18].copy_from_slice(&strong.to_ne_bytes());
    effect[18..20].copy_from_slice(&weak.to_ne_bytes());
    // SAFETY: EVIOCSFF reads and writes a 48-byte `ff_effect`.
    let result = unsafe { libc::ioctl(device.as_raw_fd(), eviocsff(), effect.as_mut_ptr()) };
    assert_eq!(result, 0, "EVIOCSFF: {}", std::io::Error::last_os_error());
    i16::from_ne_bytes([effect[2], effect[3]])
}

pub(super) fn remove_effect(device: &File, id: i16) {
    // SAFETY: EVIOCRMFF takes the effect id as an integer.
    let result = unsafe { libc::ioctl(device.as_raw_fd(), eviocrmff(), libc::c_int::from(id)) };
    assert_eq!(result, 0, "EVIOCRMFF: {}", std::io::Error::last_os_error());
}

/// An `EV_FF` event that plays effect `id` `count` times (0 stops it).
pub(super) fn play(device: &mut File, id: i16, count: i32) {
    let mut event = [0u8; 24];
    event[16..18].copy_from_slice(&0x15u16.to_ne_bytes());
    event[18..20].copy_from_slice(&(id as u16).to_ne_bytes());
    event[20..24].copy_from_slice(&count.to_ne_bytes());
    device.write_all(&event).unwrap();
}

/// The feedback reports (kind, data) that arrive within `timeout` or until `done` holds.
pub(super) fn feedback_until(
    pads: &GamepadThread,
    timeout: Duration,
    done: impl Fn(&[(u16, Vec<u8>)]) -> bool,
) -> Vec<(u16, Vec<u8>)> {
    let deadline = Instant::now() + timeout;
    let mut seen = Vec::new();
    while Instant::now() < deadline && !done(&seen) {
        for report in pads.reports() {
            if let PadReport::Feedback { kind, data, .. } = report {
                seen.push((kind, data));
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    seen
}

pub(super) fn state(buttons: u32, left: u8, sticks: [i16; 4]) -> Event {
    Event::Controller {
        id: 0,
        active: 1,
        buttons,
        left_trigger: left,
        right_trigger: 0,
        sticks,
    }
}

/// The Xbox pad takes a game's rumble effects: upload, play, a stop, and an effect that ends by
/// itself, and each reaches the client as a rumble packet with the same motors.
#[test]
#[ignore = "needs write access to /dev/uinput and read access to /dev/input"]
fn uinput_pad_rumbles() {
    let (pads, warnings) = pads();
    pads.send(Event::Arrival {
        id: 0,
        kind: 1,
        capabilities: 0,
        buttons: 0,
    });
    assert_eq!(pads.plugged_pads(), 1, "{:?}", warnings.snapshot());
    let sys = Path::new("/sys/devices/virtual/input").join(pads.sysname(0).unwrap());
    let mut device = open_event(&sys);
    let rumble = |strong: u16, weak: u16| {
        let mut data = strong.to_le_bytes().to_vec();
        data.extend_from_slice(&weak.to_le_bytes());
        (0x010b, data)
    };
    // Plays until stopped.
    let id = upload_rumble(&device, 0x8000, 0x4000, 0);
    play(&mut device, id, 1);
    let seen = feedback_until(&pads, Duration::from_secs(2), |s| !s.is_empty());
    assert_eq!(seen, vec![rumble(0x8000, 0x4000)]);
    play(&mut device, id, 0);
    let seen = feedback_until(&pads, Duration::from_secs(2), |s| !s.is_empty());
    assert_eq!(seen, vec![rumble(0, 0)]);
    remove_effect(&device, id);
    // Ends after 150 ms.
    let id = upload_rumble(&device, 0xffff, 0, 150);
    let started = Instant::now();
    play(&mut device, id, 1);
    let seen = feedback_until(&pads, Duration::from_secs(3), |s| s.len() == 2);
    assert_eq!(seen, vec![rumble(0xffff, 0), rumble(0, 0)]);
    assert!(
        started.elapsed() >= Duration::from_millis(140),
        "{:?}",
        started.elapsed()
    );
    // Closing the game's handle erases its effects without a hang, and the pad unplugs.
    drop(device);
    let unplugged = Instant::now();
    pads.send(Event::Controller {
        id: 0,
        active: 0,
        buttons: 0,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    });
    assert_eq!(pads.plugged_pads(), 0);
    assert!(unplugged.elapsed() < Duration::from_secs(2));
}

/// The sysfs folder of the HID device whose `phys` is `phys`.
pub(super) fn find_hid(phys: &str) -> PathBuf {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        for entry in std::fs::read_dir("/sys/class/hidraw").unwrap().flatten() {
            let device = entry.path().join("device");
            let uevent = std::fs::read_to_string(device.join("uevent")).unwrap_or_default();
            if uevent.lines().any(|l| l == format!("HID_PHYS={phys}")) {
                return device;
            }
        }
        assert!(
            Instant::now() < deadline,
            "no hidraw device with phys {phys}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The sysfs input folder under `hid` whose name ends with `suffix`.
pub(super) fn find_input(hid: &Path, suffix: &str) -> PathBuf {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Ok(dir) = std::fs::read_dir(hid.join("input")) {
            for entry in dir.flatten() {
                let name = std::fs::read_to_string(entry.path().join("name")).unwrap_or_default();
                if name.trim().ends_with(suffix) {
                    return entry.path();
                }
            }
        }
        assert!(Instant::now() < deadline, "no input device ending {suffix}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The hidraw node of the HID device `hid` (readable by this user through the Steam rules).
pub(super) fn hidraw_name(hid: &Path) -> std::ffi::OsString {
    std::fs::read_dir(hid.join("hidraw"))
        .unwrap()
        .flatten()
        .next()
        .unwrap()
        .file_name()
}

pub(super) fn hidraw(hid: &Path) -> File {
    let name = hidraw_name(hid);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match File::open(Path::new("/dev").join(&name)) {
            Ok(file) => {
                nonblocking(&file);
                return file;
            }
            Err(error) => {
                assert!(Instant::now() < deadline, "cannot open {name:?}: {error}");
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

/// The raw input reports that arrive on `hidraw` within `wait`.
pub(super) fn raw_reports(hidraw: &File, wait: Duration) -> Vec<Vec<u8>> {
    let deadline = Instant::now() + wait;
    let mut reports = Vec::new();
    let mut file = hidraw;
    while Instant::now() < deadline {
        let mut buf = [0u8; 128];
        while let Ok(n) = file.read(&mut buf) {
            reports.push(buf[..n].to_vec());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    reports
}

/// Reads events from `device` for up to `timeout` until `done` holds.
pub(super) fn events_until(
    device: &mut File,
    timeout: Duration,
    done: impl Fn(&[Ev]) -> bool,
) -> Vec<Ev> {
    let deadline = Instant::now() + timeout;
    let mut seen = Vec::new();
    while Instant::now() < deadline && !done(&seen) {
        seen.extend(read_events(device));
        std::thread::sleep(Duration::from_millis(10));
    }
    seen
}

/// A DualSense arrives with motion sensors and a touchpad: the kernel binds hid-playstation to
/// it, the game reads its buttons, sticks, motion and touch from the three input devices, and
/// its rumble goes back as the client's rumble.
#[test]
#[ignore = "needs write access to /dev/uhid, the hid_playstation driver and read access to /dev/input"]
fn dualsense_pad_is_a_playstation_device() {
    let (pads, warnings) = pads();
    let started = Instant::now();
    pads.send(Event::Arrival {
        id: 0,
        kind: 2,
        capabilities: 0x38,
        buttons: 0,
    });
    println!("created in {:?}", started.elapsed());
    assert_eq!(pads.plugged_pads(), 1, "{:?}", warnings.snapshot());
    let phys = pads.phys(0).expect("the arrival made a DualSense");
    // The motion request goes to the client at arrival, and the driver's start-up output
    // reports (lights) come with it.
    let kinds: Vec<PadReport> = pads.reports().collect();
    assert!(
        kinds.iter().any(|r| matches!(
            r,
            PadReport::Motion {
                id: 0,
                capabilities: 0x38
            }
        )),
        "{kinds:?}"
    );
    let hid = find_hid(&phys);
    let driver = std::fs::read_link(hid.join("driver")).unwrap();
    assert_eq!(driver.file_name().unwrap(), "playstation", "{driver:?}");
    let uevent = std::fs::read_to_string(hid.join("uevent")).unwrap();
    assert!(uevent.contains("HID_ID=0003:0000054C:00000CE6"), "{uevent}");
    let mut gamepad = open_event(&find_input(&hid, "DualSense Wireless Controller"));
    let motion_dir = find_input(&hid, "Motion Sensors");
    let touch_dir = find_input(&hid, "Touchpad");
    // The motion and touchpad devices are not joysticks, so udev gives this user no ACL for
    // their event nodes: motion is read through its joystick node, the touchpad through the
    // device's hidraw node when its event node is closed to us.
    let mut motion_event = try_open_event(&motion_dir, Duration::from_millis(500));
    let mut motion_js = Joydev::open(&motion_dir);
    let mut touch_event = try_open_event(&touch_dir, Duration::from_millis(500));
    println!(
        "motion via {}, touch via {}",
        if motion_event.is_some() {
            "evdev"
        } else {
            "joydev"
        },
        if touch_event.is_some() {
            "evdev"
        } else {
            "hidraw"
        }
    );
    let abs = std::fs::read_to_string(touch_dir.join("capabilities/abs")).unwrap();
    // ABS_MT_POSITION_X (0x35) and Y (0x36) of a touchpad with contacts.
    let abs = u64::from_str_radix(abs.trim().split(' ').next_back().unwrap(), 16).unwrap();
    assert_eq!(abs >> 0x35 & 3, 3, "touchpad abs {abs:#x}");
    let raw = hidraw(&hid);
    // Buttons, sticks and triggers.
    pads.send(state(0x1000 | 0x8, 200, [1000, -2000, 0, 0]));
    let seen = events_until(&mut gamepad, Duration::from_secs(2), |s| {
        s.contains(&(3, 0x10, 1))
    });
    println!("gamepad: {seen:?}");
    assert!(
        seen.contains(&(1, 0x130, 1)),
        "cross is BTN_SOUTH: {seen:?}"
    );
    assert!(seen.contains(&(3, 0x10, 1)), "d-pad right: {seen:?}");
    assert!(seen.contains(&(3, 0x02, 200)), "left trigger: {seen:?}");
    // 1000 of 32767 is a little right of the centre (128): 128 + 1000/256 = 131.
    assert!(seen.contains(&(3, 0x00, 131)), "left stick X: {seen:?}");
    assert!(
        seen.contains(&(3, 0x01, 135)),
        "left stick Y, HID down is up: {seen:?}"
    );
    // Motion: the accelerometer reads m/s^2 at 1 g = 8192, the gyroscope 16 per degree/s.
    pads.send(Event::Motion {
        id: 0,
        kind: 2,
        xyz: [10.0, 0.0, 0.0],
    });
    pads.send(Event::Motion {
        id: 0,
        kind: 1,
        xyz: [0.0, 9.80665, 0.0],
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut seen = Vec::new();
    let has = |s: &[Ev], axis: u16| s.iter().any(|e| e.0 == 3 && e.1 == axis && e.2 != 0);
    while Instant::now() < deadline && !(has(&seen, 3) && has(&seen, 1)) {
        if let Some(device) = &mut motion_event {
            seen.extend(read_events(device));
        }
        if let Some(device) = &mut motion_js {
            seen.extend(device.read());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    println!("motion: {seen:?}");
    assert!(
        seen.iter().any(|e| e.0 == 3 && e.1 == 3 && e.2 != 0),
        "gyro RX: {seen:?}"
    );
    assert!(
        seen.iter().any(|e| e.0 == 3 && e.1 == 1 && e.2 != 0),
        "accel Y: {seen:?}"
    );
    // Touch: a finger in the middle of the touchpad.
    pads.send(Event::ControllerTouch {
        id: 0,
        event: 1,
        touchpad: 0,
        pointer: 1,
        x: 0.5,
        y: 0.25,
        pressure: 1.0,
    });
    if let Some(device) = &mut touch_event {
        let seen = events_until(device, Duration::from_secs(2), |s| {
            s.contains(&(1, 0x14a, 1))
        });
        println!("touch: {seen:?}");
        assert!(seen.contains(&(1, 0x14a, 1)), "BTN_TOUCH: {seen:?}");
        let x = seen.iter().find(|e| e.0 == 3 && e.1 == 0x35).map(|e| e.2);
        assert!(x.is_some_and(|x| (900..1020).contains(&x)), "touch X {x:?}");
    } else {
        // What reaches the kernel: the report's first contact is down, in the middle.
        let reports = raw_reports(&raw, Duration::from_secs(1));
        let last = reports.last().expect("hidraw reports");
        assert_eq!(last[33] & 0x80, 0, "contact 0 is down: {last:?}");
        let x = u16::from(last[34]) | (u16::from(last[35] & 0xf) << 8);
        assert!((950..970).contains(&x), "touch X {x}");
    }
    pads.send(Event::ControllerTouch {
        id: 0,
        event: 2,
        touchpad: 0,
        pointer: 1,
        x: 0.5,
        y: 0.25,
        pressure: 0.0,
    });
    // Rumble: the game's effect on the gamepad device reaches the client.
    let id = upload_rumble(&gamepad, 0xc000, 0x4000, 0);
    play(&mut gamepad, id, 1);
    let seen = feedback_until(&pads, Duration::from_secs(3), |s| {
        s.iter().any(|f| f.0 == 0x010b && f.1 != [0; 4])
    });
    println!("feedback: {seen:?}");
    let rumble = seen
        .iter()
        .find(|f| f.0 == 0x010b && f.1 != [0; 4])
        .unwrap_or_else(|| panic!("no rumble in {seen:?}"));
    let low = u16::from_le_bytes([rumble.1[0], rumble.1[1]]);
    let high = u16::from_le_bytes([rumble.1[2], rumble.1[3]]);
    assert!((0xb000..=0xc100).contains(&low), "strong motor {low:#x}");
    assert!((0x3000..=0x4100).contains(&high), "weak motor {high:#x}");
    play(&mut gamepad, id, 0);
    let seen = feedback_until(&pads, Duration::from_secs(3), |s| {
        s.iter().any(|f| f.0 == 0x010b && f.1 == [0; 4])
    });
    assert!(
        seen.iter().any(|f| f.0 == 0x010b && f.1 == [0; 4]),
        "rumble stop in {seen:?}"
    );
    // A game's own output report through hidraw (lights and an adaptive trigger), as Steam
    // Input and SDL write them.
    let mut report = [0u8; 63];
    report[0] = 2;
    report[1] = 4; // the right trigger's effect
    report[2] = 4; // the lightbar
    report[11..14].copy_from_slice(&[2, 0x30, 0x90]);
    report[45..48].copy_from_slice(&[10, 20, 30]);
    OpenOptions::new()
        .write(true)
        .open(Path::new("/dev").join(hidraw_name(&hid)))
        .and_then(|mut file| file.write_all(&report))
        .expect("write the output report to hidraw");
    let seen = feedback_until(&pads, Duration::from_secs(3), |s| {
        s.iter().any(|f| f.0 == 0x5503) && s.iter().any(|f| f.0 == 0x5502)
    });
    println!("hidraw feedback: {seen:?}");
    assert!(seen.contains(&(0x5502, vec![10, 20, 30])), "{seen:?}");
    let trigger = seen.iter().find(|f| f.0 == 0x5503).expect("trigger effect");
    // The mask of changed triggers (right = 4), both triggers' modes, then the parameters.
    assert_eq!(trigger.1.len(), 23);
    assert_eq!(&trigger.1[..3], &[4, 0, 2]);
    assert_eq!(&trigger.1[13..15], &[0x30, 0x90]);
    // Leaving unplugs it: its hidraw device is gone.
    drop((gamepad, motion_event, motion_js, touch_event, raw));
    pads.send(Event::Controller {
        id: 0,
        active: 0,
        buttons: 0,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    });
    assert_eq!(pads.plugged_pads(), 0);
    assert!(!hid.exists(), "the HID device is still there");
}

/// A client's controller of the Xbox type with no sensors stays an Xbox pad, and one with
/// sensors becomes a DualSense, as on Windows.
#[test]
#[ignore = "needs write access to /dev/uinput and /dev/uhid"]
fn the_pad_follows_the_controller_type() {
    let (pads, warnings) = pads();
    pads.send(Event::Arrival {
        id: 0,
        kind: 1,
        capabilities: 0,
        buttons: 0,
    });
    assert!(pads.sysname(0).is_some(), "{:?}", warnings.snapshot());
    pads.send(Event::Arrival {
        id: 1,
        kind: 1,
        capabilities: 0x10,
        buttons: 0,
    });
    assert!(pads.phys(1).is_some(), "{:?}", warnings.snapshot());
    pads.send(Event::Arrival {
        id: 2,
        kind: 2,
        capabilities: 0,
        buttons: 0,
    });
    assert!(pads.phys(2).is_some(), "{:?}", warnings.snapshot());
    // Two DualSenses at once have distinct addresses, so the driver binds both.
    let first = find_hid(&pads.phys(1).unwrap());
    let second = find_hid(&pads.phys(2).unwrap());
    assert_ne!(first, second);
    pads.request_stop();
    assert_eq!(pads.plugged_pads(), 0);
}
