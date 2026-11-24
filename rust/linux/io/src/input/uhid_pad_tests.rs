//! Tests of the virtual pads with a kernel driver of their own (a Switch Pro Controller on
//! hid-nintendo, a Steam Deck and a Steam Controller on hid-steam, a DualShock 4 on
//! hid-playstation), all `#[ignore]`d: they need write access to `/dev/uhid`, read access to
//! `/dev/input` and the drivers' modules. Run them with
//! `cargo test -p butterpollo-linux-io input::uhid_pad_tests -- --ignored --test-threads=1`.
//! Each makes the pad as a client's controller does, checks which driver the kernel bound and
//! which input devices it made, sends a state and reads the event a game would, and writes what a
//! game or Steam writes (rumble, lights, haptics) to look for the client's feedback.

use super::{
    Event, GamepadThread, PadReport,
    pad_choice::Want,
    pad_tests::{
        Ev, Joydev, events_until, feedback_until, find_hid, find_input, hidraw, hidraw_name,
        open_event, pads, pads_with, play, raw_reports, read_events, state, try_open_event,
        upload_rumble,
    },
};
use super::{
    hid_pad::Slot,
    uhid::ReportType,
    uhid_pad::{Identity, PadModel, UhidPad},
};
use butterpollo_input_policy::{input::Input, input_policy};
use std::sync::Arc;
use std::{
    fs::OpenOptions,
    io::Write,
    path::Path,
    time::{Duration, Instant},
};

/// The kernel driver bound to `hid`, as sysfs names it.
fn driver(hid: &Path) -> String {
    std::fs::read_link(hid.join("driver"))
        .unwrap()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

/// Unplugs controller 0 and waits for its HID device to leave.
fn leave(pads: &GamepadThread, hid: &Path) {
    pads.send(Event::Controller {
        id: 0,
        active: 0,
        buttons: 0,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    });
    assert_eq!(pads.plugged_pads(), 0);
    let deadline = Instant::now() + Duration::from_secs(3);
    while hid.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!hid.exists(), "the HID device is still there");
}

/// The HID device with `phys` that has input devices, if one gets them within `wait`:
/// hid-steam makes a second device with the same `phys`, hidraw-only, for Steam's client, and
/// gives the first its input devices only while no program has that client device open (a
/// program that reads Valve controllers through hidraw, as SDL's does, takes the controller
/// over, and the driver removes its input devices until that program lets go).
fn try_input_hid(phys: &str, wait: Duration) -> Option<std::path::PathBuf> {
    let deadline = Instant::now() + wait;
    loop {
        for entry in std::fs::read_dir("/sys/bus/hid/devices").unwrap().flatten() {
            let device = entry.path();
            let uevent = std::fs::read_to_string(device.join("uevent")).unwrap_or_default();
            if uevent.lines().any(|l| l == format!("HID_PHYS={phys}"))
                && device.join("input").exists()
            {
                return Some(device.canonicalize().unwrap());
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The programs that have a hidraw node of the HID device with `phys` open.
fn hidraw_holders(phys: &str) -> Vec<String> {
    let mut nodes = Vec::new();
    for entry in std::fs::read_dir("/sys/class/hidraw").unwrap().flatten() {
        let uevent =
            std::fs::read_to_string(entry.path().join("device/uevent")).unwrap_or_default();
        if uevent.lines().any(|l| l == format!("HID_PHYS={phys}")) {
            nodes.push(Path::new("/dev").join(entry.file_name()));
        }
    }
    let mut holders = Vec::new();
    for process in std::fs::read_dir("/proc").unwrap().flatten() {
        let Ok(fds) = std::fs::read_dir(process.path().join("fd")) else {
            continue;
        };
        if fds
            .flatten()
            .filter_map(|fd| std::fs::read_link(fd.path()).ok())
            .any(|target| nodes.contains(&target))
        {
            let comm = std::fs::read_to_string(process.path().join("comm")).unwrap_or_default();
            holders.push(comm.trim().to_owned());
        }
    }
    holders
}

/// The client hidraw node of the HID device with `phys`, which hid-steam makes for Steam: the
/// kernel passes the controller's raw reports to it.
fn client_hidraw(phys: &str) -> std::fs::File {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        for entry in std::fs::read_dir("/sys/class/hidraw").unwrap().flatten() {
            let uevent =
                std::fs::read_to_string(entry.path().join("device/uevent")).unwrap_or_default();
            if uevent.lines().any(|l| l == format!("HID_PHYS={phys}"))
                && let Ok(file) = std::fs::File::open(Path::new("/dev").join(entry.file_name()))
            {
                super::pad_tests::nonblocking(&file);
                return file;
            }
        }
        assert!(Instant::now() < deadline, "no hidraw node with phys {phys}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The HID device of the pad with `phys` (the one hid-steam binds its driver to).
fn hid_of(phys: &str) -> std::path::PathBuf {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        for entry in std::fs::read_dir("/sys/bus/hid/devices").unwrap().flatten() {
            let uevent = std::fs::read_to_string(entry.path().join("uevent")).unwrap_or_default();
            if uevent.lines().any(|l| l == format!("HID_PHYS={phys}"))
                && entry.path().join("driver").exists()
            {
                return entry.path().canonicalize().unwrap();
            }
        }
        assert!(Instant::now() < deadline, "no HID device with phys {phys}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The pad's gamepad event node and the events `buttons` and the left stick at 16000 make on
/// it, or `None` when no events come: another program that takes the pad over and gives it
/// back makes the driver replace the pad's input devices, so a few tries are made, each with
/// the node the driver has then (a desktop that scans Valve controllers, as KDE's System
/// Settings does, can keep doing it for seconds).
fn gamepad_events(
    pads: &GamepadThread,
    phys: &str,
    name: &str,
    buttons: u32,
) -> Option<(std::fs::File, std::path::PathBuf, Vec<Ev>)> {
    // The state is the pad's from now on, whoever has it open.
    pads.send(state(buttons, 200, [16000, 0, 0, 0]));
    for _ in 0..5 {
        let hid = try_input_hid(phys, Duration::from_secs(3))?;
        let Some(mut gamepad) = try_open_event(&find_input(&hid, name), Duration::from_secs(3))
        else {
            continue;
        };
        pads.send(state(buttons, 200, [16000, 0, 0, 0]));
        let seen = events_until(&mut gamepad, Duration::from_millis(1500), |s| {
            s.iter().any(|e| e.0 == 1 && e.2 == 1) && s.iter().any(|e| e.0 == 3 && e.1 == 0)
        });
        if !seen.is_empty() {
            return Some((gamepad, hid, seen));
        }
    }
    None
}

fn arrive(pads: &GamepadThread, kind: u8, capabilities: u16) {
    let started = Instant::now();
    pads.send(Event::Arrival {
        id: 0,
        kind,
        capabilities,
        buttons: 0,
    });
    println!("created in {:?}", started.elapsed());
}

/// A Nintendo controller arrives with a gyroscope and accelerometer: the kernel binds
/// hid-nintendo to the Switch Pro Controller, a game reads its buttons and sticks from the
/// gamepad and its motion from the IMU device, and its rumble goes back as the client's rumble.
#[test]
#[ignore = "needs write access to /dev/uhid, the hid_nintendo driver and read access to /dev/input"]
fn switch_pro_pad_is_bound_by_hid_nintendo() {
    let (pads, warnings) = pads();
    arrive(&pads, 3, 0x30);
    assert_eq!(pads.plugged_pads(), 1, "{:?}", warnings.snapshot());
    assert_eq!(pads.kind(0), Some(Want::Switch));
    let phys = pads.phys(0).expect("the arrival made a uhid pad");
    let hid = find_hid(&phys);
    assert_eq!(driver(&hid), "nintendo");
    let uevent = std::fs::read_to_string(hid.join("uevent")).unwrap();
    assert!(uevent.contains("HID_ID=0003:0000057E:00002009"), "{uevent}");
    assert!(pads.reports().any(|r| matches!(
        r,
        PadReport::Motion {
            id: 0,
            capabilities: 0x30
        }
    )));
    let mut gamepad = try_open_event(
        &find_input(&hid, "Nintendo Switch Pro Controller"),
        Duration::from_secs(8),
    )
    .expect("the gamepad's event node");
    let imu = find_input(&hid, "(IMU)");
    // Buttons, sticks and triggers: the south button is Switch's B, a stick leans right.
    pads.send(state(0x1000 | 0x8, 0, [20000, 0, 0, 0]));
    let seen = events_until(&mut gamepad, Duration::from_secs(3), |s| {
        s.iter().any(|e| e.0 == 3 && e.1 == 0 && e.2 > 0)
    });
    println!("gamepad: {seen:?}");
    assert!(
        seen.iter().any(|e| e.0 == 1 && e.2 == 1),
        "a button: {seen:?}"
    );
    assert!(
        seen.iter().any(|e| e.0 == 3 && e.1 == 0 && e.2 > 0),
        "left stick X: {seen:?}"
    );
    // The IMU device reads the motion the client sends (opened first: a repeated value is no
    // event).
    let mut motion = try_open_event(&imu, Duration::from_millis(500));
    let mut js = Joydev::open(&imu);
    pads.send(Event::Motion {
        id: 0,
        kind: 2,
        xyz: [90.0, 0.0, 0.0],
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut seen: Vec<Ev> = Vec::new();
    while Instant::now() < deadline && !seen.iter().any(|e| e.0 == 3 && e.2 != 0) {
        if let Some(device) = &mut motion {
            seen.extend(read_events(device).into_iter().filter(|e| e.0 == 3));
        }
        if let Some(device) = &mut js {
            seen.extend(device.read());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    println!(
        "motion via {}: {:?}",
        if motion.is_some() { "evdev" } else { "joydev" },
        &seen[..seen.len().min(12)]
    );
    assert!(
        seen.iter().any(|e| e.0 == 3 && e.2 < 0),
        "the gyroscope's turn: {seen:?}"
    );
    // Rumble: the game's effect on the gamepad reaches the client.
    let id = upload_rumble(&gamepad, 0xc000, 0x4000, 0);
    play(&mut gamepad, id, 1);
    let seen = feedback_until(&pads, Duration::from_secs(3), |s| !s.is_empty());
    println!("feedback: {seen:?}");
    let rumble = seen
        .iter()
        .find(|f| f.0 == 5 && f.1[..4] != [0; 4])
        .unwrap_or_else(|| panic!("no rumble in {seen:?}"));
    println!("rumble: {:?}", &rumble.1[..4]);
    play(&mut gamepad, id, 0);
    let seen = feedback_until(&pads, Duration::from_secs(3), |s| {
        s.iter().any(|f| f.0 == 5 && f.1[..4] == [0; 4])
    });
    assert!(
        seen.iter().any(|f| f.0 == 5 && f.1[..4] == [0; 4]),
        "rumble stop in {seen:?}"
    );
    drop((gamepad, motion, js));
    leave(&pads, &hid);
}

/// A Steam Deck client arrives (the setting says Deck, as when Steam runs): the kernel binds
/// hid-steam, which reads the serial number and attributes, a game reads the Deck's gamepad
/// and sensors, and Steam's rumble goes back to the client. hid-steam gives the pad its input
/// devices only while no program has its client hidraw node open; a desktop program that reads
/// Valve controllers through hidraw (SDL's HIDAPI does) takes the pad over, and the test then
/// checks the reports the kernel passes on to that node instead.
#[test]
#[ignore = "needs write access to /dev/uhid, the hid_steam driver and read access to /dev/input"]
fn steam_deck_pad_is_bound_by_hid_steam() {
    let (pads, warnings) = pads_with(0, |policy| {
        policy.steam_deck = input_policy::DeckPad::SteamDeck;
    });
    arrive(&pads, 4, 0x38);
    assert_eq!(pads.plugged_pads(), 1, "{:?}", warnings.snapshot());
    assert_eq!(pads.kind(0), Some(Want::Deck));
    let phys = pads.phys(0).expect("the arrival made a uhid pad");
    assert_eq!(driver(&hid_of(&phys)), "hid-steam");
    let uevent = std::fs::read_to_string(hid_of(&phys).join("uevent")).unwrap();
    assert!(uevent.contains("HID_ID=0003:000028DE:00001205"), "{uevent}");
    assert!(pads.reports().any(|r| matches!(
        r,
        PadReport::Motion {
            id: 0,
            capabilities: 0x38
        }
    )));
    // The Deck's report: type 9 in byte 2, the A button in byte 8, the left stick in 48.
    let raw_checked = |pads: &GamepadThread| {
        let holders = hidraw_holders(&phys);
        assert!(
            !holders.is_empty(),
            "no input events, and no program holds the pad"
        );
        eprintln!("the pad was taken over by {holders:?}: checking its raw reports");
        let reports = raw_reports(&client_hidraw(&phys), Duration::from_secs(1));
        let report = reports.iter().find(|r| r.len() == 64 && r[8] & 0x80 != 0);
        let report = report.unwrap_or_else(|| panic!("no report with A in {reports:?}"));
        assert_eq!(&report[..4], &[1, 0, 9, 64]);
        assert_eq!(i16::from_le_bytes([report[48], report[49]]), 16000);
        leave(pads, &hid_of(&phys));
    };
    // Buttons, sticks and triggers: A, the d-pad right and a leaning left stick.
    let Some((mut gamepad, hid, seen)) = gamepad_events(&pads, &phys, "Steam Deck", 0x1000 | 0x8)
    else {
        return raw_checked(&pads);
    };
    let gamepad = &mut gamepad;
    println!("gamepad: {:?}", &seen[..seen.len().min(16)]);
    assert!(seen.contains(&(1, 0x130, 1)), "A is BTN_SOUTH: {seen:?}");
    assert!(seen.contains(&(1, 0x223, 1)), "d-pad right: {seen:?}");
    assert!(
        !seen.contains(&(1, 0x13b, 1)),
        "the pad's own hold of Options stays unseen: {seen:?}"
    );
    assert!(
        seen.iter()
            .any(|e| e.0 == 3 && e.1 == 0 && (15000..17000).contains(&e.2)),
        "left stick X: {seen:?}"
    );
    assert!(
        seen.iter().any(|e| e.0 == 3 && e.1 == 0x15 && e.2 > 20000),
        "the left trigger: {seen:?}"
    );
    // The gyroscope: the sensors device reads the rate the client sends.
    let mut motion = try_open_event(
        &find_input(&hid, "Steam Deck Motion Sensors"),
        Duration::from_millis(500),
    );
    pads.send(Event::Motion {
        id: 0,
        kind: 2,
        xyz: [90.0, 0.0, 0.0],
    });
    if let Some(device) = &mut motion {
        let seen = events_until(device, Duration::from_secs(2), |s| {
            s.iter().any(|e| e.0 == 3 && e.2 != 0)
        });
        assert!(
            seen.iter().any(|e| e.0 == 3 && e.2 != 0),
            "gyro: {:?}",
            &seen[..seen.len().min(8)]
        );
    }
    // Rumble: the game's effect reaches the client as Steam writes it to the Deck.
    let id = upload_rumble(gamepad, 0xc000, 0x4000, 0);
    play(gamepad, id, 1);
    let seen = feedback_until(&pads, Duration::from_secs(3), |s| {
        s.iter().any(|f| f.0 == 4 && f.1[..4] != [0; 4])
    });
    println!("feedback: {seen:?}");
    let rumble = seen
        .iter()
        .find(|f| f.0 == 4 && f.1[..4] != [0; 4])
        .unwrap_or_else(|| panic!("no rumble in {seen:?}"));
    assert_eq!(
        &rumble.1[..4],
        &[0x00, 0xc0, 0x00, 0x40],
        "strong, then weak"
    );
    play(gamepad, id, 0);
    let seen = feedback_until(&pads, Duration::from_secs(3), |s| {
        s.iter().any(|f| f.0 == 4 && f.1[..4] == [0; 4])
    });
    assert!(
        seen.iter().any(|f| f.0 == 4 && f.1[..4] == [0; 4]),
        "stop in {seen:?}"
    );
    drop(motion);
    leave(&pads, &hid);
}

/// A DualShock 4 chosen by the `gamepad` setting: the kernel binds hid-playstation, which gives
/// games the gamepad, motion sensors and touchpad of a DualShock 4, and its rumble and
/// lightbar go back to the client.
#[test]
#[ignore = "needs write access to /dev/uhid, the hid_playstation driver and read access to /dev/input"]
fn dualshock4_pad_is_bound_by_hid_playstation() {
    let (pads, warnings) = pads_with(input_policy::VHF_DS4, |_| {});
    arrive(&pads, 1, 0x38);
    assert_eq!(pads.plugged_pads(), 1, "{:?}", warnings.snapshot());
    assert_eq!(pads.kind(0), Some(Want::DualShock4));
    let phys = pads.phys(0).expect("the arrival made a uhid pad");
    let hid = find_hid(&phys);
    assert_eq!(driver(&hid), "playstation");
    let uevent = std::fs::read_to_string(hid.join("uevent")).unwrap();
    assert!(uevent.contains("HID_ID=0003:0000054C:000009CC"), "{uevent}");
    assert!(pads.reports().any(|r| matches!(
        r,
        PadReport::Motion {
            id: 0,
            capabilities: 0x38
        }
    )));
    let mut gamepad = open_event(&find_input(&hid, "DualShock 4 Controller"));
    let motion_dir = find_input(&hid, "Motion Sensors");
    let touch_dir = find_input(&hid, "Touchpad");
    let mut motion = try_open_event(&motion_dir, Duration::from_millis(500));
    let mut motion_js = Joydev::open(&motion_dir);
    let mut touch = try_open_event(&touch_dir, Duration::from_millis(500));
    let raw = hidraw(&hid);
    // Buttons, sticks and triggers.
    pads.send(state(0x1000 | 0x8, 200, [16000, -16000, 0, 0]));
    let seen = events_until(&mut gamepad, Duration::from_secs(3), |s| {
        s.contains(&(3, 0x10, 1))
    });
    println!("gamepad: {:?}", &seen[..seen.len().min(16)]);
    assert!(
        seen.contains(&(1, 0x130, 1)),
        "cross is BTN_SOUTH: {seen:?}"
    );
    assert!(seen.contains(&(3, 0x10, 1)), "d-pad right: {seen:?}");
    assert!(seen.contains(&(3, 0x02, 200)), "left trigger: {seen:?}");
    assert!(
        seen.iter().any(|e| e.0 == 3 && e.1 == 0 && e.2 > 128 + 40),
        "left stick X right of centre: {seen:?}"
    );
    // Motion: the gyroscope and accelerometer each reach the sensors device.
    pads.send(Event::Motion {
        id: 0,
        kind: 2,
        xyz: [90.0, 0.0, 0.0],
    });
    pads.send(Event::Motion {
        id: 0,
        kind: 1,
        xyz: [0.0, 9.80665, 0.0],
    });
    if motion.is_some() || motion_js.is_some() {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut seen = Vec::new();
        let has = |s: &[Ev], axis: u16| s.iter().any(|e| e.0 == 3 && e.1 == axis && e.2 != 0);
        while Instant::now() < deadline && !(has(&seen, 3) && has(&seen, 1)) {
            if let Some(device) = &mut motion {
                seen.extend(read_events(device));
            }
            if let Some(device) = &mut motion_js {
                seen.extend(device.read());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(has(&seen, 3), "gyro RX: {:?}", &seen[..seen.len().min(12)]);
        assert!(
            has(&seen, 1),
            "accelerometer Y: {:?}",
            &seen[..seen.len().min(12)]
        );
    } else {
        // The sensors' nodes are closed to this user: what reaches the kernel is the report's
        // gyroscope (bytes 13..19) and accelerometer (19..25).
        let reports = raw_reports(&raw, Duration::from_secs(1));
        let last = reports.last().expect("hidraw reports");
        assert!(
            last.len() >= 25 && last[13..19] != [0; 6],
            "gyro in {last:?}"
        );
        assert!(last[19..25] != [0; 6], "accelerometer in {last:?}");
    }
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
    if let Some(device) = &mut touch {
        let seen = events_until(device, Duration::from_secs(2), |s| {
            s.contains(&(1, 0x14a, 1))
        });
        assert!(seen.contains(&(1, 0x14a, 1)), "BTN_TOUCH: {seen:?}");
        let x = seen.iter().find(|e| e.0 == 3 && e.1 == 0x35).map(|e| e.2);
        // The DualShock 4's touchpad is 1920 wide.
        assert!(x.is_some_and(|x| (900..1020).contains(&x)), "touch X {x:?}");
    } else {
        // What reaches the kernel: the first contact's id has its "not touching" bit clear.
        let reports = raw_reports(&raw, Duration::from_secs(1));
        assert!(
            reports
                .iter()
                .any(|r| r.len() >= 36 && r[0] == 1 && r[35] & 0x80 == 0),
            "a touching contact in {reports:?}"
        );
    }
    // Rumble: the game's effect on the gamepad reaches the client.
    let id = upload_rumble(&gamepad, 0xc000, 0x4000, 0);
    play(&mut gamepad, id, 1);
    let seen = feedback_until(&pads, Duration::from_secs(3), |s| {
        s.iter().any(|f| f.0 == 5 && f.1[..4] != [0; 4])
    });
    println!(
        "feedback: {:?}",
        seen.iter().map(|f| (f.0, &f.1[..8])).collect::<Vec<_>>()
    );
    assert!(
        seen.iter().any(|f| f.0 == 5 && f.1[..4] != [0; 4]),
        "rumble in {seen:?}"
    );
    play(&mut gamepad, id, 0);
    let seen = feedback_until(&pads, Duration::from_secs(3), |s| {
        s.iter().any(|f| f.0 == 5 && f.1[..4] == [0; 4])
    });
    assert!(
        seen.iter().any(|f| f.0 == 5 && f.1[..4] == [0; 4]),
        "rumble stop in {seen:?}"
    );
    // A game's own output report through hidraw: rumble and the lightbar, as SDL writes them.
    let mut report = [0u8; 32];
    report[0] = 5;
    report[1] = 0x07;
    report[4] = 0x10;
    report[5] = 0x20;
    report[6..9].copy_from_slice(&[10, 20, 30]);
    OpenOptions::new()
        .write(true)
        .open(Path::new("/dev").join(hidraw_name(&hid)))
        .and_then(|mut file| file.write_all(&report))
        .expect("write the output report to hidraw");
    let seen = feedback_until(&pads, Duration::from_secs(3), |s| {
        s.iter().any(|f| f.0 == 5 && f.1[4..7] == [10, 20, 30])
    });
    assert!(
        seen.iter().any(|f| f.0 == 5 && f.1[4..7] == [10, 20, 30]),
        "the lightbar colour in {seen:?}"
    );
    drop((gamepad, motion, motion_js, touch, raw));
    leave(&pads, &hid);
}

/// A Steam Controller (2026) chosen by the `gamepad` setting: hid-steam binds its unified
/// interface and gives games the "Steam Controller" gamepad with its trackpads and grips, and
/// Steam's haptic writes go back to the client. As for the Deck, a program that has taken the
/// pad over through hidraw turns the test to the raw reports.
#[test]
#[ignore = "needs write access to /dev/uhid, the hid_steam driver and read access to /dev/input"]
fn steam_controller_pad_is_bound_by_hid_steam() {
    let (pads, warnings) = pads_with(input_policy::VHF_STEAM_CONTROLLER, |_| {});
    arrive(&pads, 4, 0x38);
    assert_eq!(pads.plugged_pads(), 1, "{:?}", warnings.snapshot());
    assert_eq!(pads.kind(0), Some(Want::SteamController));
    let phys = pads.phys(0).expect("the arrival made a uhid pad");
    assert_eq!(driver(&hid_of(&phys)), "hid-steam");
    let uevent = std::fs::read_to_string(hid_of(&phys).join("uevent")).unwrap();
    assert!(uevent.contains("HID_ID=0003:000028DE:00001302"), "{uevent}");
    // The report: id 0x42, the A button in byte 2 bit 0, the left stick X in bytes 10..12.
    let raw_checked = |pads: &GamepadThread| {
        let holders = hidraw_holders(&phys);
        assert!(
            !holders.is_empty(),
            "no input events, and no program holds the pad"
        );
        eprintln!("the pad was taken over by {holders:?}: checking its raw reports");
        let reports = raw_reports(&client_hidraw(&phys), Duration::from_secs(1));
        let report = reports
            .iter()
            .find(|r| r.len() == 54 && r[0] == 0x42 && r[2] & 1 != 0);
        let report = report.unwrap_or_else(|| panic!("no report with A in {reports:?}"));
        assert_eq!(i16::from_le_bytes([report[10], report[11]]), 16000);
        leave(pads, &hid_of(&phys));
    };
    // Buttons, sticks and triggers (the driver leaves lizard mode once the pad holds Menu).
    let Some((mut gamepad, hid, seen)) =
        gamepad_events(&pads, &phys, "Steam Controller", 0x1000 | 0x8)
    else {
        return raw_checked(&pads);
    };
    let gamepad = &mut gamepad;
    println!("gamepad: {:?}", &seen[..seen.len().min(16)]);
    assert!(seen.contains(&(1, 0x130, 1)), "A is BTN_SOUTH: {seen:?}");
    assert!(
        !seen.contains(&(1, 0x13b, 1)),
        "the pad's own hold of Menu stays unseen: {seen:?}"
    );
    assert!(
        seen.iter()
            .any(|e| e.0 == 3 && e.1 == 0 && (15000..17000).contains(&e.2)),
        "left stick X: {seen:?}"
    );
    assert!(
        seen.iter().any(|e| e.0 == 3 && e.1 == 0x15 && e.2 > 20000),
        "the left trigger: {seen:?}"
    );
    // Rumble: the game's effect on the gamepad reaches the client as plain rumble, since this
    // client has no Steam Controller's own haptics.
    let id = upload_rumble(gamepad, 0xc000, 0x4000, 0);
    play(gamepad, id, 1);
    let seen = feedback_until(&pads, Duration::from_secs(3), |s| {
        s.iter().any(|f| f.0 == 4 && f.1[..4] != [0; 4])
    });
    println!("feedback: {seen:?}");
    assert!(
        seen.iter().any(|f| f.0 == 4 && f.1[..4] != [0; 4]),
        "rumble in {seen:?}"
    );
    leave(&pads, &hid);
}

/// A client's Switch 2 Pro Controller (057e:2069): no kernel driver binds it as a HID device
/// (hid-nintendo has the Switch Pro Controller, 057e:2009), so it is a Switch Pro Controller,
/// and the stream says why.
#[test]
#[ignore = "needs write access to /dev/uhid, the hid_nintendo driver and read access to /dev/input"]
fn a_switch_2_pro_controller_is_a_switch_pro_controller_with_a_warning() {
    let (pads, warnings) = pads();
    pads.send(Event::ControllerIdentity(
        rubylight_protocol::controller_identity::ControllerIdentity {
            controller: 0,
            vendor: 0x057e,
            product: 0x2069,
        },
    ));
    arrive(&pads, 3, 0x30);
    assert_eq!(pads.plugged_pads(), 1, "{:?}", warnings.snapshot());
    assert_eq!(pads.kind(0), Some(Want::Switch));
    let hid = find_hid(&pads.phys(0).unwrap());
    assert_eq!(driver(&hid), "nintendo");
    let warning = warnings
        .snapshot()
        .into_iter()
        .find(|w| w.code == "input_switch2" && w.message.contains("Switch 2 Pro Controller"))
        .unwrap_or_else(|| panic!("no warning in {:?}", warnings.snapshot()));
    println!("{}", warning.message);
    leave(&pads, &hid);
}

/// A pad whose driver does not bind fails visibly: the error names the driver and the device.
#[test]
#[ignore = "needs write access to /dev/uhid; takes the driver's bind time"]
fn a_pad_no_driver_binds_is_refused_with_the_drivers_name() {
    struct Unbound;
    impl PadModel for Unbound {
        fn identity(&self) -> Identity {
            Identity {
                name: "Rubylight Test Pad",
                vendor: 0x1209,
                product: 0x7e57,
                version: 1,
                phys: "test",
                driver: "hid_nothing",
            }
        }
        fn descriptor(&mut self) -> Vec<u8> {
            // A bare gamepad with one button.
            vec![
                0x05, 0x01, 0x09, 0x05, 0xa1, 0x01, 0x05, 0x09, 0x19, 0x01, 0x29, 0x01, 0x15, 0x00,
                0x25, 0x01, 0x75, 0x01, 0x95, 0x01, 0x81, 0x02, 0x75, 0x07, 0x95, 0x01, 0x81, 0x03,
                0xc0,
            ]
        }
        fn get_report(&mut self, _: ReportType, _: u8) -> Option<Vec<u8>> {
            None
        }
        fn set_report(&mut self, _: ReportType, _: u8, _: &[u8]) -> bool {
            false
        }
        fn output(&mut self, _: &[u8]) {}
        fn apply(&mut self, _: &Input) {}
        fn take_reports(&mut self) -> Vec<Vec<u8>> {
            Vec::new()
        }
        fn take_feedback(&mut self) -> Vec<(u16, Vec<u8>)> {
            Vec::new()
        }
        fn bound(&self) -> bool {
            false
        }
    }
    let error = UhidPad::create_in(
        0,
        Slot::reserve().unwrap(),
        Box::new(Unbound),
        Arc::default(),
    )
    .err()
    .expect("a pad no driver binds is refused");
    let text = format!("{error:#}");
    println!("{text}");
    assert!(
        text.contains("hid_nothing") && text.contains("Rubylight Test Pad"),
        "{text}"
    );
}
