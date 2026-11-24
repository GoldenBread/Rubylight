//! A stand-in game for the Linux host's input test during a stream: run inside the stream's
//! gamescope session, it fills the screen with a moving bar (so the client sees detail and
//! motion) and appends every input it receives to the file named by its first argument, one
//! line each: X keyboard and mouse events (`key down 38`, `button down 1`, `motion 330 245`)
//! and the events of a virtual Xbox 360 pad (`pad key 304 1`, `pad abs 0 16000`).
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

/// Opens each virtual Xbox 360 pad (045e:028e) as it appears and records its buttons and axes.
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
            let Ok(file) = std::fs::File::open(&path) else {
                continue;
            };
            if pad_id(&file) != Some((0x045e, 0x028e)) {
                continue;
            }
            opened.insert(path.clone());
            write(log, &format!("pad opened {}", path.display()));
            let log = log.clone();
            std::thread::spawn(move || read_pad(file, &log));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
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

/// Records key (EV_KEY) and axis (EV_ABS) events until the device goes away.
fn read_pad(mut file: std::fs::File, log: &Log) {
    // struct input_event on 64-bit Linux: a 16-byte time, then type, code and value.
    let mut event = [0u8; 24];
    while file.read_exact(&mut event).is_ok() {
        let kind = u16::from_ne_bytes([event[16], event[17]]);
        let code = u16::from_ne_bytes([event[18], event[19]]);
        let value = i32::from_ne_bytes([event[20], event[21], event[22], event[23]]);
        let name = match kind {
            1 => "key",
            3 => "abs",
            _ => continue,
        };
        write(log, &format!("pad {name} {code} {value}"));
    }
    write(log, "pad closed");
}
