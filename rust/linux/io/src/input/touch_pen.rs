//! Native touch and pen on the user's desktop: a multitouch touchscreen and a pen tablet on
//! `/dev/uinput`, which the compositor takes in like hardware. On KDE Plasma each is tied to the
//! streamed monitor through KWin's input-device D-Bus API (`outputName`), so a contact lands
//! where the client touched its picture; another compositor maps them as it maps any
//! touchscreen. A gamescope session has no touch input (its X server takes XTEST, which has
//! none), so there, and when the devices cannot be made, the first finger and the pen drive the
//! pointer instead ([`Emulated`]). Touch and pen events are Moonlight's: 0 hover, 1 down, 2 up,
//! 3 move, 4 cancel, 5 buttons only, 6 hover leave, 7 cancel all.

use super::{
    Event,
    evdev_desktop::{ABS_MAX, ABS_X, ABS_Y, Axis, Device, EV_ABS, EV_KEY},
};
use crate::portal::{DesktopMonitor, layout};
use anyhow::Result;

const ABS_PRESSURE: u16 = 0x18;
const ABS_TILT_X: u16 = 0x1a;
const ABS_TILT_Y: u16 = 0x1b;
const ABS_MT_SLOT: u16 = 0x2f;
const ABS_MT_TOUCH_MAJOR: u16 = 0x30;
const ABS_MT_TOUCH_MINOR: u16 = 0x31;
const ABS_MT_ORIENTATION: u16 = 0x34;
const ABS_MT_POSITION_X: u16 = 0x35;
const ABS_MT_POSITION_Y: u16 = 0x36;
const ABS_MT_TRACKING_ID: u16 = 0x39;
const ABS_MT_PRESSURE: u16 = 0x3a;
const BTN_TOOL_PEN: u16 = 0x140;
const BTN_TOOL_RUBBER: u16 = 0x141;
const BTN_TOUCH: u16 = 0x14a;
const BTN_STYLUS: u16 = 0x14b;
const BTN_STYLUS2: u16 = 0x14c;
const EV_SYN: u16 = 0;
const INPUT_PROP_DIRECT: u32 = 1;
/// Contacts held at once; a further finger is ignored until one lifts.
const SLOTS: usize = 16;
/// Pressure runs `0..=PRESSURE_MAX`. A client that sends none (0) gets half.
const PRESSURE_MAX: i32 = 4096;
/// Units per millimetre on the position axes: a screen about 330 by 330 mm, so the
/// compositor sees a plausible size. The mapping to the monitor does not depend on it.
const RESOLUTION: i32 = 200;

/// A client fraction of the picture on an axis of `0..=ABS_MAX`.
fn axis(fraction: f32) -> i32 {
    let fraction = if fraction.is_nan() {
        0.
    } else {
        fraction.clamp(0., 1.)
    };
    (fraction * ABS_MAX as f32).round() as i32
}
/// A client pressure (0 to 1, 0 when unknown) on `1..=PRESSURE_MAX`.
fn pressure(value: f32) -> i32 {
    if value.is_nan() || value <= 0. {
        PRESSURE_MAX / 2
    } else {
        ((value.min(1.) * PRESSURE_MAX as f32).round() as i32).max(1)
    }
}
/// The pen's tilt along x and y in degrees from its tilt off vertical (0 to 90, `u8::MAX`
/// unknown) and the direction it leans (degrees clockwise, `u16::MAX` unknown), as the
/// Windows host computes them.
fn tilt(tilt: u8, rotation: u16) -> Option<(i32, i32)> {
    if tilt == u8::MAX || rotation == u16::MAX {
        return None;
    }
    let angle = f32::from(rotation).to_radians();
    let tilt = f32::from(tilt.min(90)).to_radians();
    let x = ((-angle).sin() * tilt.sin()).atan2(tilt.cos()).to_degrees();
    let y = ((-angle).cos() * tilt.sin()).atan2(tilt.cos()).to_degrees();
    Some((x.round() as i32, y.round() as i32))
}

/// The pen as the tablet last reported it.
#[derive(Default, Clone, Copy, PartialEq, Debug)]
struct PenState {
    /// The tool in range (`BTN_TOOL_PEN` or `BTN_TOOL_RUBBER`).
    tool: Option<u16>,
    touching: bool,
    buttons: u8,
}

/// The desktop's virtual touchscreen and pen.
pub(super) struct TouchPen {
    screen: Device,
    pen: Device,
    /// The client contact held in each slot.
    slots: [Option<u32>; SLOTS],
    /// The tracking id the next contact gets.
    next_tracking: i32,
    pen_state: PenState,
}

impl TouchPen {
    /// Makes both devices and, in the background, ties them to `monitor`'s output.
    pub(super) fn create(monitor: &DesktopMonitor) -> Result<Self> {
        let position = |code| Axis {
            code,
            minimum: 0,
            maximum: ABS_MAX,
            resolution: RESOLUTION,
        };
        let plain = |code, minimum, maximum| Axis {
            code,
            minimum,
            maximum,
            resolution: 0,
        };
        let screen = Device::create_axes(
            "Rubylight Touchscreen",
            &[EV_KEY, EV_ABS, EV_SYN],
            [BTN_TOUCH].into_iter(),
            &[],
            &[
                position(ABS_X),
                position(ABS_Y),
                plain(ABS_MT_SLOT, 0, SLOTS as i32 - 1),
                plain(ABS_MT_TRACKING_ID, 0, 65535),
                position(ABS_MT_POSITION_X),
                position(ABS_MT_POSITION_Y),
                plain(ABS_MT_PRESSURE, 0, PRESSURE_MAX),
                plain(ABS_MT_TOUCH_MAJOR, 0, ABS_MAX),
                plain(ABS_MT_TOUCH_MINOR, 0, ABS_MAX),
                plain(ABS_MT_ORIENTATION, 0, 359),
            ],
            &[INPUT_PROP_DIRECT],
        )?;
        let pen = Device::create_axes(
            "Rubylight Pen",
            &[EV_KEY, EV_ABS, EV_SYN],
            [
                BTN_TOOL_PEN,
                BTN_TOOL_RUBBER,
                BTN_TOUCH,
                BTN_STYLUS,
                BTN_STYLUS2,
            ]
            .into_iter(),
            &[],
            &[
                position(ABS_X),
                position(ABS_Y),
                plain(ABS_PRESSURE, 0, PRESSURE_MAX),
                plain(ABS_TILT_X, -90, 90),
                plain(ABS_TILT_Y, -90, 90),
            ],
            &[INPUT_PROP_DIRECT],
        )?;
        tracing::info!(
            touchscreen = %screen.sysname,
            pen = %pen.sysname,
            "native touch and pen go to the desktop through /dev/uinput"
        );
        map_to_output(
            vec![screen.sysname.clone(), pen.sysname.clone()],
            monitor.clone(),
        );
        Ok(Self {
            screen,
            pen,
            slots: [None; SLOTS],
            next_tracking: 0,
            pen_state: PenState::default(),
        })
    }

    /// Stages one touch or pen event; [`Self::flush`] sends them.
    pub(super) fn apply(&mut self, e: &Event) {
        match *e {
            Event::Touch {
                event,
                id,
                x,
                y,
                pressure,
                major,
                minor,
                rotation,
            } => self.touch(event, id, (x, y), pressure, (major, minor), rotation),
            Event::Pen {
                event,
                tool,
                buttons,
                x,
                y,
                pressure,
                rotation,
                tilt,
                ..
            } => self.pen(event, tool, buttons, (x, y), pressure, rotation, tilt),
            _ => {}
        }
    }

    fn touch(
        &mut self,
        event: u8,
        id: u32,
        (x, y): (f32, f32),
        force: f32,
        (major, minor): (f32, f32),
        rotation: u16,
    ) {
        if event == 7 {
            self.lift_all();
            return;
        }
        let held = self.slots.iter().position(|slot| *slot == Some(id));
        let slot = match (held, event) {
            (Some(slot), _) => slot,
            // Only a down starts a contact; a touchscreen has no hover.
            (None, 1) => match self.slots.iter().position(Option::is_none) {
                Some(free) => free,
                None => return,
            },
            (None, _) => return,
        };
        let first = self.slots.iter().all(Option::is_none);
        let screen = &mut self.screen;
        match event {
            1 | 3 => {
                screen.queue(EV_ABS, ABS_MT_SLOT, slot as i32);
                if held.is_none() {
                    screen.queue(EV_ABS, ABS_MT_TRACKING_ID, self.next_tracking);
                    self.next_tracking = (self.next_tracking + 1) % 65536;
                    self.slots[slot] = Some(id);
                }
                let (ax, ay) = (axis(x), axis(y));
                screen.queue(EV_ABS, ABS_MT_POSITION_X, ax);
                screen.queue(EV_ABS, ABS_MT_POSITION_Y, ay);
                screen.queue(EV_ABS, ABS_MT_PRESSURE, pressure(force));
                if major > 0. || minor > 0. {
                    screen.queue(EV_ABS, ABS_MT_TOUCH_MAJOR, axis(major.max(minor)));
                    screen.queue(EV_ABS, ABS_MT_TOUCH_MINOR, axis(minor.min(major)));
                }
                if rotation != u16::MAX {
                    screen.queue(EV_ABS, ABS_MT_ORIENTATION, i32::from(rotation % 360));
                }
                // The single-touch axes follow the oldest contact, as a kernel driver's do.
                if self.slots.iter().position(Option::is_some) == Some(slot) {
                    screen.queue(EV_ABS, ABS_X, ax);
                    screen.queue(EV_ABS, ABS_Y, ay);
                }
                if held.is_none() {
                    if first {
                        screen.key(BTN_TOUCH, true);
                    }
                    // A new contact is its own report, so a down and an up of one pass stay apart.
                    screen.sync();
                }
            }
            2 | 4 if held.is_some() => {
                screen.queue(EV_ABS, ABS_MT_SLOT, slot as i32);
                screen.queue(EV_ABS, ABS_MT_TRACKING_ID, -1);
                self.slots[slot] = None;
                if self.slots.iter().all(Option::is_none) {
                    screen.key(BTN_TOUCH, false);
                }
                screen.sync();
            }
            _ => {}
        }
    }

    /// Lifts every finger and the pen.
    pub(super) fn release(&mut self) {
        self.lift_all();
    }
    fn lift_all(&mut self) {
        let mut lifted = false;
        for (slot, held) in self.slots.iter_mut().enumerate() {
            if held.take().is_some() {
                self.screen.queue(EV_ABS, ABS_MT_SLOT, slot as i32);
                self.screen.queue(EV_ABS, ABS_MT_TRACKING_ID, -1);
                lifted = true;
            }
        }
        if lifted {
            self.screen.key(BTN_TOUCH, false);
            self.screen.sync();
        }
        self.pen_out();
    }

    #[allow(clippy::too_many_arguments)]
    fn pen(
        &mut self,
        event: u8,
        tool: u8,
        buttons: u8,
        (x, y): (f32, f32),
        force: f32,
        rotation: u16,
        lean: u8,
    ) {
        match event {
            4 | 6 | 7 => return self.pen_out(),
            5 => {
                if self.pen_state.tool.is_some() {
                    self.pen_buttons(buttons);
                    self.pen.sync();
                }
                return;
            }
            0..=3 => {}
            _ => return,
        }
        let code = if tool == 2 {
            BTN_TOOL_RUBBER
        } else {
            BTN_TOOL_PEN
        };
        // Another tool leaves before this one comes into range.
        if self.pen_state.tool.is_some_and(|held| held != code) {
            self.pen_out();
        }
        let pen = &mut self.pen;
        pen.queue(EV_ABS, ABS_X, axis(x));
        pen.queue(EV_ABS, ABS_Y, axis(y));
        if let Some((tx, ty)) = tilt(lean, rotation) {
            pen.queue(EV_ABS, ABS_TILT_X, tx);
            pen.queue(EV_ABS, ABS_TILT_Y, ty);
        }
        if self.pen_state.tool.is_none() {
            pen.key(code, true);
            self.pen_state.tool = Some(code);
        }
        let touching = event == 1 || (event == 3 && self.pen_state.touching);
        pen.queue(
            EV_ABS,
            ABS_PRESSURE,
            if touching { pressure(force) } else { 0 },
        );
        if touching != self.pen_state.touching {
            pen.key(BTN_TOUCH, touching);
            self.pen_state.touching = touching;
        }
        self.pen_buttons(buttons);
        self.pen.sync();
        // As on Windows, a pen that lifts leaves: the client's hover brings it back.
        if event == 2 {
            self.pen_out();
        }
    }

    fn pen_buttons(&mut self, buttons: u8) {
        for (bit, code) in [(1, BTN_STYLUS), (2, BTN_STYLUS2)] {
            let down = buttons & bit != 0;
            if down != (self.pen_state.buttons & bit != 0) {
                self.pen.key(code, down);
            }
        }
        self.pen_state.buttons = buttons & 3;
    }

    /// The pen lifts and leaves range, if it is in range.
    fn pen_out(&mut self) {
        let Some(tool) = self.pen_state.tool else {
            return;
        };
        if self.pen_state.touching {
            self.pen.queue(EV_ABS, ABS_PRESSURE, 0);
            self.pen.key(BTN_TOUCH, false);
        }
        self.pen_buttons(0);
        self.pen.key(tool, false);
        self.pen.sync();
        self.pen_state = PenState::default();
    }

    /// Sends what was staged.
    pub(super) fn flush(&mut self) -> Result<()> {
        let screen = self.screen.flush();
        let pen = self.pen.flush();
        screen.and(pen)
    }
}

/// The name of the output `monitor` is, as KWin names it.
fn output_name(monitor: &DesktopMonitor) -> Option<String> {
    let name = match monitor {
        DesktopMonitor::Named(name) => name.clone(),
        DesktopMonitor::Primary => layout::monitors().into_iter().next()?.0,
        DesktopMonitor::Index(index) => layout::monitors().into_iter().nth(*index)?.0,
    };
    // Names made up from a PipeWire node are not outputs.
    (!name.is_empty() && !name.starts_with("node ")).then_some(name)
}

/// Ties each device (by its sysfs name, `input23`) to `monitor`'s output through KWin, once
/// KWin lists it, in the background: KWin takes a moment to open a new device. Without KWin
/// the devices keep the compositor's own mapping.
fn map_to_output(sysnames: Vec<String>, monitor: DesktopMonitor) {
    let spawned = std::thread::Builder::new()
        .name("touch-output".into())
        .spawn(move || {
            let Some(output) = output_name(&monitor) else {
                tracing::debug!(?monitor, "no output name to tie touch and pen to");
                return;
            };
            let Ok(bus) = crate::power::logind::session_bus() else {
                return;
            };
            for sysname in sysnames {
                let Some(node) = event_node(&sysname) else {
                    continue;
                };
                let path = format!("/org/kde/KWin/InputDevice/{node}");
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                loop {
                    let set = zbus::blocking::Proxy::new(
                        &bus,
                        "org.kde.KWin",
                        path.as_str(),
                        "org.kde.KWin.InputDevice",
                    )
                    .map_err(zbus::fdo::Error::from)
                    .and_then(|proxy| proxy.set_property("outputName", output.as_str()));
                    match set {
                        Ok(()) => {
                            tracing::info!(%node, %output, "touch device tied to the streamed output");
                            break;
                        }
                        Err(error) if std::time::Instant::now() >= deadline => {
                            tracing::debug!(%node, error = %error, "KWin did not take the touch device's output");
                            break;
                        }
                        Err(_) => std::thread::sleep(std::time::Duration::from_millis(100)),
                    }
                }
            }
        });
    if let Err(error) = spawned {
        tracing::debug!(%error, "touch output mapping not started");
    }
}

/// The event node (`event7`) of the input device `sysname`.
fn event_node(sysname: &str) -> Option<String> {
    std::fs::read_dir(format!("/sys/class/input/{sysname}"))
        .ok()?
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .find(|name| name.starts_with("event"))
}

/// Touch and pen as the pointer, where they cannot be injected: the first finger (and the pen)
/// moves it and holds its left button.
#[derive(Default)]
pub(super) struct Emulated {
    /// The client contact driving the pointer.
    finger: Option<u32>,
    pen_down: bool,
}

impl Emulated {
    /// The pointer events one touch or pen event stands for.
    pub(super) fn events(&mut self, e: &Event) -> Vec<Event> {
        let at = |x: f32, y: f32| Event::Absolute {
            x: (axis(x) / 2) as i16,
            y: (axis(y) / 2) as i16,
            width: (ABS_MAX / 2) as i16,
            height: (ABS_MAX / 2) as i16,
        };
        let button = |down| Event::MouseButton { button: 1, down };
        match *e {
            Event::Touch { event: 7, .. } => {
                let held = self.finger.take().is_some();
                held.then(|| button(false)).into_iter().collect()
            }
            Event::Touch {
                event, id, x, y, ..
            } => match (event, self.finger) {
                (1, None) => {
                    self.finger = Some(id);
                    vec![at(x, y), button(true)]
                }
                (3, Some(finger)) if finger == id => vec![at(x, y)],
                (2 | 4, Some(finger)) if finger == id => {
                    self.finger = None;
                    vec![at(x, y), button(false)]
                }
                _ => vec![],
            },
            Event::Pen { event, x, y, .. } => match event {
                0 | 3 => vec![at(x, y)],
                1 if !self.pen_down => {
                    self.pen_down = true;
                    vec![at(x, y), button(true)]
                }
                2 | 4 | 6 | 7 if self.pen_down => {
                    self.pen_down = false;
                    vec![button(false)]
                }
                _ => vec![],
            },
            _ => vec![],
        }
    }
}

#[cfg(test)]
#[path = "touch_pen_tests.rs"]
mod tests;
