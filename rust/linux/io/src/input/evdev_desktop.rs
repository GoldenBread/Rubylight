//! Keyboard and mouse for the user's own desktop: three virtual devices on `/dev/uinput` (a
//! keyboard, a relative mouse with a hi-resolution wheel, and an absolute pointer) that the
//! compositor (KWin, Mutter) takes in like any hardware, with no prompt. Used when the stream's
//! target is the desktop; a gamescope session keeps XTEST.

use super::{
    Event,
    evdev_keys::{SHIFT, evdev, typed},
};
use crate::portal::{
    DesktopMonitor,
    layout::{self, Rect},
};
use anyhow::{Context, Result, bail};
use butterpollo_input_policy::input_policy::Policy;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    io::Write,
    os::fd::AsRawFd,
};

const fn ioc(dir: u32, nr: u8, size: usize) -> libc::c_ulong {
    ((dir << 30) | ((size as u32) << 16) | ((b'U' as u32) << 8) | nr as u32) as libc::c_ulong
}
const UI_DEV_CREATE: libc::c_ulong = ioc(0, 1, 0);
const UI_DEV_DESTROY: libc::c_ulong = ioc(0, 2, 0);
const UI_DEV_SETUP: libc::c_ulong = ioc(1, 3, size_of::<Setup>());
const UI_ABS_SETUP: libc::c_ulong = ioc(1, 4, size_of::<AbsSetup>());
const UI_SET_EVBIT: libc::c_ulong = ioc(1, 100, 4);
const UI_SET_KEYBIT: libc::c_ulong = ioc(1, 101, 4);
const UI_SET_RELBIT: libc::c_ulong = ioc(1, 102, 4);
const UI_SET_ABSBIT: libc::c_ulong = ioc(1, 103, 4);
const UI_SET_PROPBIT: libc::c_ulong = ioc(1, 110, 4);
/// `UI_GET_SYSNAME`: the sysfs name of the new device, such as `input23`.
const UI_GET_SYSNAME: libc::c_ulong = ioc(2, 44, 64);

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_REL: u16 = 2;
const EV_ABS: u16 = 3;
const REL_X: u16 = 0;
const REL_Y: u16 = 1;
const REL_HWHEEL: u16 = 6;
const REL_WHEEL: u16 = 8;
const REL_WHEEL_HI_RES: u16 = 0x0b;
const REL_HWHEEL_HI_RES: u16 = 0x0c;
const ABS_X: u16 = 0;
const ABS_Y: u16 = 1;
const BTN_LEFT: u16 = 0x110;
const INPUT_PROP_POINTER: u32 = 0;
const BUS_USB: u16 = 3;
/// The absolute pointer's axes run `0..=ABS_MAX` across the whole desktop.
const ABS_MAX: i32 = 65535;
const MODIFIER_EXTENDED: u8 = 0x10;
/// Shift, Ctrl and Alt as a key-down packet's modifier bits, the virtual keys that hold them
/// and the evdev key pressed for the bit.
const MODIFIERS: [(u8, [u16; 3], u16); 3] = [
    (0x01, [0x10, 0xa0, 0xa1], SHIFT),
    (0x02, [0x11, 0xa2, 0xa3], 29),
    (0x04, [0x12, 0xa4, 0xa5], 56),
];

#[repr(C)]
struct Setup {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
    name: [u8; 80],
    ff_effects_max: u32,
}
#[repr(C)]
struct AbsSetup {
    code: u16,
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

/// One virtual input device and the events queued for its next report.
pub(crate) struct Device {
    file: File,
    /// The sysfs name the kernel gave it (`input23`).
    pub(crate) sysname: String,
    queued: Vec<libc::input_event>,
}

impl Device {
    fn create(
        name: &str,
        events: &[u16],
        keys: impl Iterator<Item = u16>,
        rels: &[u16],
        abs: &[(u16, i32)],
        props: &[u32],
    ) -> Result<Self> {
        let file = OpenOptions::new()
            .write(true)
            .open("/dev/uinput")
            .context("opening /dev/uinput (is the user allowed to write it?)")?;
        let fd = file.as_raw_fd();
        let set = |request: libc::c_ulong, value: u32| -> Result<()> {
            // SAFETY: a uinput request that takes an integer, on a descriptor this function owns.
            let status = unsafe { libc::ioctl(fd, request, value as libc::c_int) };
            if status < 0 {
                bail!(
                    "uinput request {request:#x}: {}",
                    std::io::Error::last_os_error()
                );
            }
            Ok(())
        };
        for &event in events {
            set(UI_SET_EVBIT, u32::from(event))?;
        }
        for key in keys {
            set(UI_SET_KEYBIT, u32::from(key))?;
        }
        for &rel in rels {
            set(UI_SET_RELBIT, u32::from(rel))?;
        }
        for &(axis, _) in abs {
            set(UI_SET_ABSBIT, u32::from(axis))?;
        }
        for &prop in props {
            set(UI_SET_PROPBIT, prop)?;
        }
        for &(code, maximum) in abs {
            let setup = AbsSetup {
                code,
                value: 0,
                minimum: 0,
                maximum,
                fuzz: 0,
                flat: 0,
                resolution: 0,
            };
            // SAFETY: the request reads one `AbsSetup`, which outlives the call.
            if unsafe { libc::ioctl(fd, UI_ABS_SETUP, &raw const setup) } < 0 {
                bail!("UI_ABS_SETUP: {}", std::io::Error::last_os_error());
            }
        }
        let mut setup = Setup {
            bustype: BUS_USB,
            vendor: 0x1209,
            product: 0x5201,
            version: 1,
            name: [0; 80],
            ff_effects_max: 0,
        };
        setup.name[..name.len().min(79)].copy_from_slice(&name.as_bytes()[..name.len().min(79)]);
        // SAFETY: the request reads one `Setup`, which outlives the call.
        if unsafe { libc::ioctl(fd, UI_DEV_SETUP, &raw const setup) } < 0 {
            bail!("UI_DEV_SETUP: {}", std::io::Error::last_os_error());
        }
        // SAFETY: a request without argument.
        if unsafe { libc::ioctl(fd, UI_DEV_CREATE) } < 0 {
            bail!("UI_DEV_CREATE: {}", std::io::Error::last_os_error());
        }
        let mut sysname = [0u8; 64];
        // SAFETY: the request writes a NUL-terminated name of at most 64 bytes.
        let status = unsafe { libc::ioctl(fd, UI_GET_SYSNAME, sysname.as_mut_ptr()) };
        let sysname = if status < 0 {
            String::new()
        } else {
            let end = sysname.iter().position(|&b| b == 0).unwrap_or(0);
            String::from_utf8_lossy(&sysname[..end]).into_owned()
        };
        Ok(Self {
            file,
            sysname,
            queued: Vec::new(),
        })
    }
    fn queue(&mut self, kind: u16, code: u16, value: i32) {
        // SAFETY: `input_event` is plain data; the kernel stamps the time itself.
        let mut event: libc::input_event = unsafe { std::mem::zeroed() };
        (event.type_, event.code, event.value) = (kind, code, value);
        self.queued.push(event);
    }
    fn key(&mut self, code: u16, down: bool) {
        self.queue(EV_KEY, code, i32::from(down));
    }
    /// Ends a report: what was queued becomes visible to the desktop together.
    fn sync(&mut self) {
        if !self.queued.is_empty() && self.queued.last().is_some_and(|e| e.type_ != EV_SYN) {
            self.queue(EV_SYN, 0, 0);
        }
    }
    /// Writes the queued events, closing the last report.
    fn flush(&mut self) -> Result<()> {
        self.sync();
        if self.queued.is_empty() {
            return Ok(());
        }
        // SAFETY: the events are plain data and the slice is viewed as the bytes it is.
        let bytes = unsafe {
            std::slice::from_raw_parts(
                self.queued.as_ptr().cast::<u8>(),
                self.queued.len() * size_of::<libc::input_event>(),
            )
        };
        let result = self.file.write_all(bytes);
        self.queued.clear();
        result.context("writing to the virtual input device")
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: a request without argument, on the descriptor this device owns.
        unsafe { libc::ioctl(self.file.as_raw_fd(), UI_DEV_DESTROY) };
    }
}

/// The desktop's virtual keyboard, mouse and absolute pointer.
pub(crate) struct Devices {
    pub(crate) keyboard: Device,
    pub(crate) mouse: Device,
    pub(crate) pointer: Device,
}

const BUTTONS: [u16; 5] = [
    BTN_LEFT,
    BTN_LEFT + 2,
    BTN_LEFT + 1,
    BTN_LEFT + 3,
    BTN_LEFT + 4,
];

impl Devices {
    pub(crate) fn create() -> Result<Self> {
        Ok(Self {
            keyboard: Device::create(
                "Rubylight Desktop Keyboard",
                &[EV_KEY, EV_SYN],
                1..=248,
                &[],
                &[],
                &[],
            )?,
            mouse: Device::create(
                "Rubylight Desktop Mouse",
                &[EV_KEY, EV_REL, EV_SYN],
                BUTTONS.into_iter(),
                &[
                    REL_X,
                    REL_Y,
                    REL_WHEEL,
                    REL_HWHEEL,
                    REL_WHEEL_HI_RES,
                    REL_HWHEEL_HI_RES,
                ],
                &[],
                &[],
            )?,
            pointer: Device::create(
                "Rubylight Desktop Absolute Pointer",
                &[EV_KEY, EV_ABS, EV_SYN],
                BUTTONS.into_iter(),
                &[],
                &[(ABS_X, ABS_MAX), (ABS_Y, ABS_MAX)],
                &[INPUT_PROP_POINTER],
            )?,
        })
    }
}

/// The desktop input of one stream.
pub(super) struct DesktopInput {
    /// The monitor streamed.
    monitor: DesktopMonitor,
    pub(super) devices: Option<Devices>,
    /// Why the devices could not be made, so the error is raised once.
    failed: bool,
    /// The monitor's rectangle and the desktop's, in the compositor's pixels.
    area: Option<(Rect, Rect)>,
    /// Host keys held by virtual key, with the evdev key pressed.
    keys: BTreeMap<u16, u16>,
    /// Client keys by the host key they hold.
    sources: BTreeMap<u16, u16>,
    buttons: BTreeSet<u8>,
    scroll: [i32; 2],
}

impl DesktopInput {
    pub(super) fn new(monitor: DesktopMonitor) -> Self {
        let mut input = Self {
            monitor,
            devices: None,
            failed: false,
            area: None,
            keys: BTreeMap::new(),
            sources: BTreeMap::new(),
            buttons: BTreeSet::new(),
            scroll: [0; 2],
        };
        // Made now, not at the first key: the desktop takes a moment to notice a new device,
        // and input sent before that is lost.
        input.ensure();
        input
    }
    /// Reads the monitor's place in the desktop again.
    pub(super) fn refresh_layout(&mut self) {
        let monitors = layout::monitors();
        let rects: Vec<Rect> = monitors.iter().map(|(_, rect)| *rect).collect();
        let index = match &self.monitor {
            DesktopMonitor::Primary => Some(0),
            DesktopMonitor::Index(index) => Some(*index),
            DesktopMonitor::Named(name) => monitors.iter().position(|(found, _)| found == name),
        };
        self.area = index
            .and_then(|index| rects.get(index))
            .zip(layout::bounds(&rects))
            .map(|(monitor, all)| (*monitor, all));
    }
    fn ensure(&mut self) -> Option<&mut Devices> {
        if self.devices.is_none() && !self.failed {
            match Devices::create() {
                Ok(devices) => {
                    tracing::info!(
                        keyboard = %devices.keyboard.sysname,
                        mouse = %devices.mouse.sysname,
                        pointer = %devices.pointer.sysname,
                        "keyboard and mouse input goes to the desktop through /dev/uinput"
                    );
                    self.devices = Some(devices);
                    self.refresh_layout();
                }
                Err(error) => {
                    self.failed = true;
                    tracing::warn!(error = %format!("{error:#}"), "no keyboard and mouse input for the desktop");
                }
            }
        }
        self.devices.as_mut()
    }
    /// Where a client position lands on the absolute pointer's axes.
    fn absolute(&self, x: i16, y: i16, width: i16, height: i16) -> (i32, i32) {
        let fraction = |v: i16, size: i16| f64::from(v.max(0)) / f64::from(size.max(1));
        let (fx, fy) = (fraction(x, width).min(1.0), fraction(y, height).min(1.0));
        let (monitor, all) = self.area.unwrap_or_else(|| {
            let whole = Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            };
            (whole, whole)
        });
        // The far edge lands on the monitor's last pixel.
        let place = |at: i32, size: i32, f: f64, origin: i32, span: i32| {
            let pixel = f64::from(at) + f * f64::from((size - 1).max(0)) - f64::from(origin);
            let axis = (pixel + 0.5) / f64::from(span.max(1)) * f64::from(ABS_MAX);
            (axis.round() as i32).clamp(0, ABS_MAX)
        };
        (
            place(monitor.x, monitor.width, fx, all.x, all.width),
            place(monitor.y, monitor.height, fy, all.y, all.height),
        )
    }
    /// Stage one event. Nothing is sent before [`Self::finish`].
    pub(super) fn apply(&mut self, e: &Event, policy: &Policy) -> Result<()> {
        if self.ensure().is_none() {
            return Ok(());
        }
        match e {
            Event::Keyboard {
                key,
                modifiers,
                down,
                ..
            } => self.key(*key, *modifiers, *down, policy),
            Event::Relative { x, y } => {
                let Some(devices) = &mut self.devices else {
                    return Ok(());
                };
                if *x != 0 {
                    devices.mouse.queue(EV_REL, REL_X, i32::from(*x));
                }
                if *y != 0 {
                    devices.mouse.queue(EV_REL, REL_Y, i32::from(*y));
                }
                Ok(())
            }
            Event::Absolute {
                x,
                y,
                width,
                height,
            } => {
                let (ax, ay) = self.absolute(*x, *y, *width, *height);
                if let Some(devices) = &mut self.devices {
                    devices.pointer.queue(EV_ABS, ABS_X, ax);
                    devices.pointer.queue(EV_ABS, ABS_Y, ay);
                }
                Ok(())
            }
            Event::MouseButton { button, down } => {
                let Some(&code) = usize::from(*button)
                    .checked_sub(1)
                    .and_then(|index| BUTTONS.get(index))
                else {
                    return Ok(());
                };
                if *down == self.buttons.contains(button) {
                    return Ok(());
                }
                if let Some(devices) = &mut self.devices {
                    // The desktop has one cursor, wherever motion or position last put it.
                    devices.mouse.key(code, *down);
                }
                if *down {
                    self.buttons.insert(*button);
                } else {
                    self.buttons.remove(button);
                }
                Ok(())
            }
            Event::Scroll { amount, horizontal } => {
                let Some(devices) = &mut self.devices else {
                    return Ok(());
                };
                let (hi, low) = if *horizontal {
                    (REL_HWHEEL_HI_RES, REL_HWHEEL)
                } else {
                    (REL_WHEEL_HI_RES, REL_WHEEL)
                };
                devices.mouse.queue(EV_REL, hi, i32::from(*amount));
                let rest = &mut self.scroll[usize::from(*horizontal)];
                let notches = super::keymap::scroll_notches(rest, *amount);
                if notches != 0 {
                    devices.mouse.queue(EV_REL, low, notches);
                }
                Ok(())
            }
            Event::Text(text) => {
                let Some(devices) = &mut self.devices else {
                    return Ok(());
                };
                for c in text.chars() {
                    let Some((code, shift)) = typed(c) else {
                        tracing::debug!(%c, "no key types this character on the desktop");
                        continue;
                    };
                    if shift {
                        devices.keyboard.key(SHIFT, true);
                    }
                    devices.keyboard.key(code, true);
                    devices.keyboard.sync();
                    devices.keyboard.key(code, false);
                    if shift {
                        devices.keyboard.key(SHIFT, false);
                    }
                    devices.keyboard.sync();
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
    fn key(&mut self, client: u16, modifiers: u8, down: bool, policy: &Policy) -> Result<()> {
        let Some(devices) = &mut self.devices else {
            return Ok(());
        };
        if !down {
            let Some(key) = self.sources.remove(&client) else {
                return Ok(());
            };
            if self.sources.values().any(|held| *held == key) {
                return Ok(());
            }
            if let Some(code) = self.keys.remove(&key) {
                devices.keyboard.key(code, false);
            }
            return Ok(());
        }
        if self.sources.contains_key(&client) {
            return Ok(());
        }
        let key = policy.key(client);
        let keypad = key == 0x0d && modifiers & MODIFIER_EXTENDED != 0;
        let Some(code) = evdev(key, keypad) else {
            tracing::debug!(
                key = format_args!("{key:#x}"),
                "no evdev key for this virtual key"
            );
            return Ok(());
        };
        self.sources.insert(client, key);
        if self.keys.contains_key(&key) {
            return Ok(());
        }
        // Shift, Ctrl and Alt the client reported without pressing them go around this key only.
        let mut around = Vec::new();
        if !matches!(key, 0x10..=0x12 | 0xa0..=0xa5 | 0x5b | 0x5c) {
            for (bit, alternatives, press) in MODIFIERS {
                if modifiers & bit != 0 && !alternatives.iter().any(|vk| self.keys.contains_key(vk))
                {
                    around.push(press);
                }
            }
        }
        for modifier in &around {
            devices.keyboard.key(*modifier, true);
        }
        devices.keyboard.key(code, true);
        for modifier in around.iter().rev() {
            devices.keyboard.key(*modifier, false);
        }
        self.keys.insert(key, code);
        Ok(())
    }
    /// Sends what the events of a pass queued.
    pub(super) fn finish(&mut self) -> Result<()> {
        let Some(devices) = &mut self.devices else {
            return Ok(());
        };
        let results = [
            devices.keyboard.flush(),
            devices.mouse.flush(),
            devices.pointer.flush(),
        ];
        if results.iter().any(Result::is_err) {
            // A broken device is made again at the next event.
            self.devices = None;
            self.failed = false;
            self.keys.clear();
            self.sources.clear();
            self.buttons.clear();
        }
        results.into_iter().collect()
    }
    /// Lets go of every held key and button.
    pub(super) fn release(&mut self) -> Result<()> {
        let keys = std::mem::take(&mut self.keys);
        let buttons = std::mem::take(&mut self.buttons);
        self.sources.clear();
        self.scroll = [0; 2];
        if let Some(devices) = &mut self.devices {
            for code in keys.into_values() {
                devices.keyboard.key(code, false);
            }
            for button in buttons {
                if let Some(&code) = usize::from(button)
                    .checked_sub(1)
                    .and_then(|i| BUTTONS.get(i))
                {
                    devices.mouse.key(code, false);
                }
            }
        }
        self.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Read, os::unix::fs::OpenOptionsExt, time::Duration};

    fn idle(area: Option<(Rect, Rect)>) -> DesktopInput {
        DesktopInput {
            monitor: DesktopMonitor::Primary,
            devices: None,
            failed: true,
            area,
            keys: BTreeMap::new(),
            sources: BTreeMap::new(),
            buttons: BTreeSet::new(),
            scroll: [0; 2],
        }
    }
    fn rect(x: i32, y: i32, width: i32, height: i32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn absolute_positions_fill_the_only_monitor() {
        let screen = rect(0, 0, 1920, 1080);
        let input = idle(Some((screen, screen)));
        assert_eq!(input.absolute(0, 0, 1280, 720), (17, 30));
        let (x, y) = input.absolute(1280, 720, 1280, 720);
        assert!(
            (65500..=65535).contains(&x) && (65500..=65535).contains(&y),
            "{x} {y}"
        );
        let (x, y) = input.absolute(640, 360, 1280, 720);
        assert!(
            (32700..=32800).contains(&x) && (32700..=32800).contains(&y),
            "{x} {y}"
        );
    }

    #[test]
    fn absolute_positions_stay_on_their_monitor() {
        // A second monitor to the right: the desktop is 3840 wide.
        let (first, second) = (rect(0, 0, 1920, 1080), rect(1920, 0, 1920, 1080));
        let all = rect(0, 0, 3840, 1080);
        let on_first = idle(Some((first, all))).absolute(1280, 0, 1280, 720).0;
        let on_second_start = idle(Some((second, all))).absolute(0, 0, 1280, 720).0;
        let on_second_end = idle(Some((second, all))).absolute(1280, 0, 1280, 720).0;
        assert!((32700..=32800).contains(&on_first), "{on_first}");
        assert!(
            (32760..=32850).contains(&on_second_start),
            "{on_second_start}"
        );
        assert!(on_second_end >= 65500, "{on_second_end}");
        // Off the picture is clamped to it.
        assert_eq!(idle(Some((first, all))).absolute(-9, 9999, 1280, 720).0, 9);
    }

    /// Makes the three virtual devices, finds the mouse as an event node, moves the pointer one
    /// pixel right and back, and reads both reports from the node. The desktop sees the same
    /// two reports: a net movement of zero. No key, button or wheel is sent.
    #[test]
    #[ignore = "makes virtual input devices and moves the real pointer by one pixel and back"]
    fn the_virtual_mouse_moves_and_reports_back() {
        let mut devices = Devices::create().expect("the three uinput devices");
        assert!(!devices.keyboard.sysname.is_empty() && !devices.pointer.sysname.is_empty());
        let node = (0..100)
            .find_map(|_| {
                std::thread::sleep(Duration::from_millis(50));
                std::fs::read_dir(format!(
                    "/sys/devices/virtual/input/{}",
                    devices.mouse.sysname
                ))
                .ok()?
                .flatten()
                .find_map(|e| {
                    let name = e.file_name().into_string().ok()?;
                    name.starts_with("event")
                        .then(|| std::path::PathBuf::from("/dev/input").join(name))
                })
                .filter(|path| path.exists())
            })
            .expect("the mouse's event node");
        // The kernel's own record of what the mouse can do.
        let rel = std::fs::read_to_string(format!(
            "/sys/devices/virtual/input/{}/capabilities/rel",
            devices.mouse.sysname
        ))
        .expect("the mouse's capabilities");
        let rel = u64::from_str_radix(rel.trim(), 16).unwrap();
        for axis in [
            REL_X,
            REL_Y,
            REL_WHEEL,
            REL_HWHEEL,
            REL_WHEEL_HI_RES,
            REL_HWHEEL_HI_RES,
        ] {
            assert!(rel & (1 << axis) != 0, "REL {axis} missing from {rel:#x}");
        }
        let reader = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&node);
        // Give the desktop time to take the device in, as a stream's start does.
        std::thread::sleep(Duration::from_millis(
            std::env::var("RUBYLIGHT_UINPUT_SETTLE_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(500),
        ));
        for dx in [1, -1] {
            devices.mouse.queue(EV_REL, REL_X, dx);
            devices.mouse.flush().unwrap();
            std::thread::sleep(Duration::from_millis(
                std::env::var("RUBYLIGHT_UINPUT_PAUSE_MS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(300),
            ));
        }
        let Ok(mut reader) = reader else {
            // Event nodes belong to the `input` group; only the desktop reads them here.
            eprintln!(
                "{} is not readable by this user: the events were sent, not read back",
                node.display()
            );
            return;
        };
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).ok();
        let size = size_of::<libc::input_event>();
        let events: Vec<(u16, u16, i32)> = bytes
            .chunks_exact(size)
            .map(|chunk| {
                // SAFETY: a whole `input_event` read from the kernel; unaligned in the buffer.
                let event: libc::input_event =
                    unsafe { std::ptr::read_unaligned(chunk.as_ptr().cast()) };
                (event.type_, event.code, event.value)
            })
            .collect();
        assert_eq!(
            events,
            [
                (EV_REL, REL_X, 1),
                (EV_SYN, 0, 0),
                (EV_REL, REL_X, -1),
                (EV_SYN, 0, 0)
            ]
        );
    }

    /// Sends the absolute pointer to the place it is already at (`RUBYLIGHT_ABS_AT=x,y`, the
    /// cursor's position in the primary monitor's pixels), so the pointer must not move when
    /// the mapping from a picture of that monitor to the desktop is right.
    #[test]
    #[ignore = "makes a virtual pointer and places the real one (where it is)"]
    fn the_absolute_pointer_lands_where_asked() {
        let Some((x, y)) = std::env::var("RUBYLIGHT_ABS_AT").ok().and_then(|at| {
            let (x, y) = at.split_once(',')?;
            Some((x.parse::<i16>().ok()?, y.parse::<i16>().ok()?))
        }) else {
            eprintln!("skipped: RUBYLIGHT_ABS_AT=x,y names where the pointer is now");
            return;
        };
        let mut input = DesktopInput::new(DesktopMonitor::Primary);
        assert!(input.devices.is_some(), "no virtual devices");
        let (monitor, all) = input.area.expect("the desktop layout");
        eprintln!("monitor {monitor:?} of {all:?}");
        std::thread::sleep(Duration::from_millis(1500));
        let policy = butterpollo_input_policy::input_policy::Policy::resolve(
            &butterpollo_config::config::Config::default(),
        )
        .unwrap();
        for _ in 0..3 {
            input
                .apply(
                    &Event::Absolute {
                        x,
                        y,
                        width: (monitor.width - 1) as i16,
                        height: (monitor.height - 1) as i16,
                    },
                    &policy,
                )
                .unwrap();
            input.finish().unwrap();
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}
