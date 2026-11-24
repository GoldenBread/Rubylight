//! A stand-in game for the Linux host's input test during a stream: run inside the stream's
//! gamescope session, it fills the screen with a moving bar (so the client sees detail and
//! motion) and appends every input it receives to the file named by its first argument, one
//! line each: X keyboard and mouse events (`key down 38`, `button down 1`, `motion 330 245`)
//! and the events of the host's virtual pads (`pad event21 key 304 1`, `pad event21 abs 0 16000`),
//! and it asks a pad to rumble at its first A press (`pad event21 rumble asked=true`).
//!
//! usage: input_recorder OUT
use anyhow::{Context, Result};
use std::{
    io::{Read, Write},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use x11rb::{
    connection::Connection,
    protocol::{Event, xproto::*},
    wrapper::ConnectionExt as _,
};

type Log = Arc<Mutex<std::fs::File>>;

fn write(log: &Log, line: &str) {
    let mut file = log.lock().unwrap();
    let _ = writeln!(file, "{line}");
    let _ = file.flush();
}

fn main() -> Result<()> {
    let out = std::env::args()
        .nth(1)
        .context("usage: input_recorder OUT")?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&out)
        .with_context(|| format!("opening {out}"))?;
    let log: Log = Arc::new(Mutex::new(file));
    let pads = log.clone();
    std::thread::spawn(move || watch_pads(&pads));
    record_x(&log)
}

/// Shows the moving bar and records the window's keyboard and mouse events.
fn record_x(log: &Log) -> Result<()> {
    let (conn, screen) = x11rb::connect(None).context("connecting to the X server")?;
    let root = conn.setup().roots[screen].clone();
    let (width, height) = (root.width_in_pixels, root.height_in_pixels);
    let window = conn.generate_id()?;
    conn.create_window(
        root.root_depth,
        window,
        root.root,
        0,
        0,
        width,
        height,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new().event_mask(
            EventMask::KEY_PRESS
                | EventMask::KEY_RELEASE
                | EventMask::BUTTON_PRESS
                | EventMask::BUTTON_RELEASE
                | EventMask::POINTER_MOTION,
        ),
    )?;
    conn.change_property8(
        PropMode::REPLACE,
        window,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        b"input recorder",
    )?;
    conn.map_window(window)?;
    let (white, black) = (conn.generate_id()?, conn.generate_id()?);
    conn.create_gc(
        white,
        window,
        &CreateGCAux::new().foreground(root.white_pixel),
    )?;
    conn.create_gc(
        black,
        window,
        &CreateGCAux::new().foreground(root.black_pixel),
    )?;
    conn.flush()?;
    write(log, &format!("ready {width}x{height}"));
    let started = Instant::now();
    let bar = width / 8;
    loop {
        // One picture every 16 ms: white, with a black bar crossing in four seconds.
        let x = ((started.elapsed().as_secs_f64() / 4.0).fract() * f64::from(width)) as i16;
        let whole = Rectangle {
            x: 0,
            y: 0,
            width,
            height,
        };
        conn.poly_fill_rectangle(window, white, &[whole])?;
        let moving = Rectangle {
            x,
            y: 0,
            width: bar,
            height,
        };
        conn.poly_fill_rectangle(window, black, &[moving])?;
        conn.flush()?;
        let until = Instant::now() + Duration::from_millis(16);
        while Instant::now() < until {
            match conn.poll_for_event()? {
                Some(event) => {
                    if let Some(line) = describe(&event) {
                        write(log, &line);
                    }
                }
                None => std::thread::sleep(Duration::from_millis(2)),
            }
        }
    }
}

fn describe(event: &Event) -> Option<String> {
    Some(match event {
        Event::KeyPress(e) => format!("key down {}", e.detail),
        Event::KeyRelease(e) => format!("key up {}", e.detail),
        Event::ButtonPress(e) => format!("button down {}", e.detail),
        Event::ButtonRelease(e) => format!("button up {}", e.detail),
        Event::MotionNotify(e) => format!("motion {} {}", e.event_x, e.event_y),
        _ => return None,
    })
}

/// Opens each of the host's virtual pads as it appears (the uinput Xbox 360 pad, 045e:028e, or
/// an input device of a HID device the host names `rubylight-...`: a DualSense, DualShock 4,
/// Switch Pro or Steam pad over uhid, each with its motion and touchpad devices, whose own
/// `phys` the kernel's drivers leave empty) and records its buttons and axes. A device this
/// user may not open is logged as such.
fn watch_pads(log: &Log) {
    let mut opened = std::collections::BTreeSet::new();
    loop {
        for entry in std::fs::read_dir("/dev/input")
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = entry.path();
            let is_event = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("event"));
            if !is_event || opened.contains(&path) {
                continue;
            }
            let node = path.file_name().unwrap().to_string_lossy().into_owned();
            let phys = hid_phys(&node).unwrap_or_default();
            // Read and write, for rumble; a device that refuses writing is only read.
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .or_else(|_| std::fs::File::open(&path));
            let file = match file {
                Ok(file) => file,
                Err(error) if phys.starts_with("rubylight-") => {
                    opened.insert(path.clone());
                    write(log, &format!("pad refused {node} {phys}: {error}"));
                    continue;
                }
                Err(_) => continue,
            };
            let id = pad_id(&file);
            if id != Some((0x045e, 0x028e)) && !phys.starts_with("rubylight-") {
                continue;
            }
            opened.insert(path.clone());
            let (vendor, product) = id.unwrap_or_default();
            let name = text(&file, 0x06).unwrap_or_default();
            write(
                log,
                &format!("pad opened {node} {vendor:04x}:{product:04x} {name:?} {phys}"),
            );
            let log = log.clone();
            std::thread::spawn(move || read_pad(file, &node, &log));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The physical path of the HID device an input device `node` (`event7`) belongs to, from its
/// uevent (`HID_PHYS`); None for a device that is not HID.
fn hid_phys(node: &str) -> Option<String> {
    let uevent =
        std::fs::read_to_string(format!("/sys/class/input/{node}/device/device/uevent")).ok()?;
    uevent
        .lines()
        .find_map(|line| line.strip_prefix("HID_PHYS="))
        .map(str::to_owned)
}

/// The device's vendor and product (`EVIOCGID`).
fn pad_id(file: &std::fs::File) -> Option<(u16, u16)> {
    use std::os::fd::AsRawFd;
    const EVIOCGID: libc::c_ulong = 0x8008_4502;
    let mut id = [0u16; 4];
    // SAFETY: EVIOCGID writes one `struct input_id` (four u16) into `id`.
    let result = unsafe { libc::ioctl(file.as_raw_fd(), EVIOCGID, id.as_mut_ptr()) };
    (result >= 0).then_some((id[1], id[2]))
}

/// A text the device reports: its name (`EVIOCGNAME`, `number` 0x06) or physical path
/// (`EVIOCGPHYS`, 0x07).
fn text(file: &std::fs::File, number: libc::c_ulong) -> Option<String> {
    use std::os::fd::AsRawFd;
    let mut buffer = [0u8; 256];
    // _IOC(_IOC_READ, 'E', number, 256)
    let request = 0x8000_0000 | (256 << 16) | (0x45 << 8) | number;
    // SAFETY: the ioctl writes at most 256 bytes into `buffer`.
    let result = unsafe { libc::ioctl(file.as_raw_fd(), request, buffer.as_mut_ptr()) };
    (result > 0).then(|| {
        let end = buffer
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(buffer.len());
        String::from_utf8_lossy(&buffer[..end]).into_owned()
    })
}

/// Asks the pad to rumble as a game does: uploads a half-second strong and weak rumble
/// (`EVIOCSFF`, `FF_RUMBLE`) and plays it once. False when the device has no force feedback.
fn rumble(file: &mut std::fs::File) -> bool {
    use std::os::fd::AsRawFd;
    const EVIOCSFF: libc::c_ulong = 0x4030_4580;
    // struct ff_effect on 64-bit Linux: type, id, direction, trigger (2 u16), replay (length,
    // delay), then the effect's union at offset 16; a rumble is its strong and weak magnitudes.
    let mut effect = [0u8; 48];
    effect[0..2].copy_from_slice(&0x50u16.to_ne_bytes());
    effect[2..4].copy_from_slice(&(-1i16).to_ne_bytes());
    effect[10..12].copy_from_slice(&500u16.to_ne_bytes());
    effect[16..18].copy_from_slice(&0xC000u16.to_ne_bytes());
    effect[18..20].copy_from_slice(&0x8000u16.to_ne_bytes());
    // SAFETY: EVIOCSFF reads one `struct ff_effect` from `effect` and writes its id back.
    if unsafe { libc::ioctl(file.as_raw_fd(), EVIOCSFF, effect.as_mut_ptr()) } < 0 {
        return false;
    }
    let id = i16::from_ne_bytes([effect[2], effect[3]]);
    // An EV_FF event with the effect's id and a count of 1 plays it.
    let mut play = [0u8; 24];
    play[16..18].copy_from_slice(&0x15u16.to_ne_bytes());
    play[18..20].copy_from_slice(&(id as u16).to_ne_bytes());
    play[20..24].copy_from_slice(&1i32.to_ne_bytes());
    std::io::Write::write_all(file, &play).is_ok()
}

/// Records key (EV_KEY) and axis (EV_ABS) events until the device goes away. The first press
/// of the south button (A, cross, B on a Switch pad) asks the pad to rumble ([`rumble`]).
fn read_pad(mut file: std::fs::File, node: &str, log: &Log) {
    // struct input_event on 64-bit Linux: a 16-byte time, then type, code and value.
    let mut event = [0u8; 24];
    let mut rumbled = false;
    while file.read_exact(&mut event).is_ok() {
        let kind = u16::from_ne_bytes([event[16], event[17]]);
        let code = u16::from_ne_bytes([event[18], event[19]]);
        let value = i32::from_ne_bytes([event[20], event[21], event[22], event[23]]);
        let name = match kind {
            1 => "key",
            3 => "abs",
            _ => continue,
        };
        write(log, &format!("pad {node} {name} {code} {value}"));
        if kind == 1 && code == 304 && value == 1 && !rumbled {
            rumbled = true;
            let sent = rumble(&mut file);
            write(log, &format!("pad {node} rumble asked={sent}"));
        }
    }
    write(log, &format!("pad {node} closed"));
}
