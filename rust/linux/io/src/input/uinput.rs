//! A virtual Xbox 360 pad on `/dev/uinput`: what games open as a joystick. Moonlight's
//! controller state (an XInput button mask, two sticks and two triggers) is written as evdev
//! events with the layout of the kernel's `xpad` driver and its USB ids, so SDL maps it.

use anyhow::{Context, Result};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    mem::size_of,
    os::fd::AsRawFd,
};

const fn ioc(dir: u32, kind: u8, nr: u8, size: usize) -> libc::c_ulong {
    ((dir << 30) | ((size as u32) << 16) | ((kind as u32) << 8) | nr as u32) as libc::c_ulong
}
const UI_DEV_CREATE: libc::c_ulong = ioc(0, b'U', 1, 0);
const UI_DEV_DESTROY: libc::c_ulong = ioc(0, b'U', 2, 0);
const UI_DEV_SETUP: libc::c_ulong = ioc(1, b'U', 3, size_of::<Setup>());
const UI_ABS_SETUP: libc::c_ulong = ioc(1, b'U', 4, size_of::<AbsSetup>());
const UI_SET_EVBIT: libc::c_ulong = ioc(1, b'U', 100, 4);
const UI_SET_KEYBIT: libc::c_ulong = ioc(1, b'U', 101, 4);
const UI_SET_ABSBIT: libc::c_ulong = ioc(1, b'U', 103, 4);
/// `UI_GET_SYSNAME`: the sysfs name of the new device, such as `input23`.
const UI_GET_SYSNAME: libc::c_ulong = ioc(2, b'U', 44, 64);

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_ABS: u16 = 3;
const BUS_USB: u16 = 3;

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

/// Moonlight's button mask bit and the evdev key it presses, as `xpad` names them.
const BUTTONS: [(u32, u16); 11] = [
    (0x0010, 0x13b), // Start
    (0x0020, 0x13a), // Back
    (0x0040, 0x13d), // left stick click
    (0x0080, 0x13e), // right stick click
    (0x0100, 0x136), // left bumper
    (0x0200, 0x137), // right bumper
    (0x0400, 0x13c), // Guide
    (0x1000, 0x130), // A
    (0x2000, 0x131), // B
    (0x4000, 0x133), // X
    (0x8000, 0x134), // Y
];
/// The mask's misc button, which the Windows host also sends as Guide.
const MISC: u32 = 0x20_0000;
const AXES: [(u16, i32, i32, i32, i32); 8] = [
    (0x00, -32768, 32767, 16, 128), // ABS_X
    (0x01, -32768, 32767, 16, 128), // ABS_Y
    (0x02, 0, 255, 0, 0),           // ABS_Z, left trigger
    (0x03, -32768, 32767, 16, 128), // ABS_RX
    (0x04, -32768, 32767, 16, 128), // ABS_RY
    (0x05, 0, 255, 0, 0),           // ABS_RZ, right trigger
    (0x10, -1, 1, 0, 0),            // ABS_HAT0X
    (0x11, -1, 1, 0, 0),            // ABS_HAT0Y
];

/// What a pad reports: the keys and axes in the order of [`BUTTONS`] and [`AXES`].
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub(super) struct State {
    keys: [bool; 11],
    axes: [i32; 8],
}

impl State {
    /// The state of Moonlight's controller report: the sticks' Y axes point up there and
    /// down in evdev.
    pub(super) fn from_report(buttons: u32, left: u8, right: u8, sticks: &[i16; 4]) -> Self {
        let mut keys = [false; 11];
        for (key, (bit, _)) in keys.iter_mut().zip(BUTTONS) {
            *key = buttons & bit != 0;
        }
        keys[6] |= buttons & MISC != 0;
        let down = |v: i16| i32::from(v).saturating_neg().min(32767);
        let hat = |negative: u32, positive: u32| {
            i32::from(buttons & positive != 0) - i32::from(buttons & negative != 0)
        };
        Self {
            keys,
            axes: [
                i32::from(sticks[0]),
                down(sticks[1]),
                i32::from(left),
                i32::from(sticks[2]),
                down(sticks[3]),
                i32::from(right),
                hat(0x4, 0x8),
                hat(0x1, 0x2),
            ],
        }
    }
}

/// One plugged-in virtual pad.
pub(super) struct Pad {
    file: File,
    state: State,
    /// The device's sysfs name, such as `input23`; the device tests find it with this.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) sysname: String,
}

impl Pad {
    /// Creates the pad. Fails with the reason when `/dev/uinput` cannot be opened for writing.
    pub(super) fn create(id: u16) -> Result<Self> {
        let file = OpenOptions::new()
            .write(true)
            .open("/dev/uinput")
            .context("cannot open /dev/uinput for writing; virtual gamepads need access to it (a udev rule giving the host's user write access)")?;
        let fd = file.as_raw_fd();
        let call = |request: libc::c_ulong, arg: libc::c_ulong| {
            // SAFETY: `fd` is an open uinput file; these requests take an integer argument.
            if unsafe { libc::ioctl(fd, request, arg) } < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        };
        let pointer = |request: libc::c_ulong, arg: *const libc::c_void| {
            // SAFETY: `fd` is an open uinput file and `arg` points to the struct the request
            // reads, alive for the call.
            if unsafe { libc::ioctl(fd, request, arg) } < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        };
        let setup = || -> std::io::Result<()> {
            call(UI_SET_EVBIT, libc::c_ulong::from(EV_KEY))?;
            call(UI_SET_EVBIT, libc::c_ulong::from(EV_ABS))?;
            for (_, key) in BUTTONS {
                call(UI_SET_KEYBIT, libc::c_ulong::from(key))?;
            }
            for (code, minimum, maximum, fuzz, flat) in AXES {
                call(UI_SET_ABSBIT, libc::c_ulong::from(code))?;
                let abs = AbsSetup {
                    code,
                    value: 0,
                    minimum,
                    maximum,
                    fuzz,
                    flat,
                    resolution: 0,
                };
                pointer(UI_ABS_SETUP, (&raw const abs).cast())?;
            }
            let mut name = [0u8; 80];
            let text = format!("Microsoft X-Box 360 pad (Rubylight {id})");
            name[..text.len()].copy_from_slice(text.as_bytes());
            let device = Setup {
                bustype: BUS_USB,
                vendor: 0x045e,
                product: 0x028e,
                version: 0x0110,
                name,
                ff_effects_max: 0,
            };
            pointer(UI_DEV_SETUP, (&raw const device).cast())?;
            call(UI_DEV_CREATE, 0)
        };
        setup().context("cannot set up the virtual gamepad on /dev/uinput")?;
        let mut name = [0u8; 64];
        let sysname = if pointer(UI_GET_SYSNAME, name.as_mut_ptr().cast()).is_ok() {
            let end = name.iter().position(|b| *b == 0).unwrap_or(name.len());
            String::from_utf8_lossy(&name[..end]).into_owned()
        } else {
            String::new()
        };
        let mut pad = Self {
            file,
            state: State::default(),
            sysname,
        };
        pad.write_state(State::default())?;
        Ok(pad)
    }
    /// Reports `state`; the kernel drops what did not change.
    pub(super) fn set(&mut self, state: State) -> Result<()> {
        if state == self.state {
            return Ok(());
        }
        self.state = state;
        self.write_state(state)
    }
    fn write_state(&mut self, state: State) -> Result<()> {
        let mut bytes = Vec::with_capacity(20 * size_of::<libc::input_event>());
        let mut event = |kind: u16, code: u16, value: i32| {
            let e = libc::input_event {
                time: libc::timeval {
                    tv_sec: 0,
                    tv_usec: 0,
                },
                type_: kind,
                code,
                value,
            };
            // SAFETY: `input_event` is plain data without padding the kernel reads.
            bytes.extend_from_slice(unsafe {
                std::slice::from_raw_parts(
                    (&raw const e).cast::<u8>(),
                    size_of::<libc::input_event>(),
                )
            });
        };
        for (down, (_, key)) in state.keys.iter().zip(BUTTONS) {
            event(EV_KEY, key, i32::from(*down));
        }
        for (value, (code, ..)) in state.axes.iter().zip(AXES) {
            event(EV_ABS, code, *value);
        }
        event(EV_SYN, 0, 0);
        self.file
            .write_all(&bytes)
            .context("cannot write to the virtual gamepad")
    }
}

impl Drop for Pad {
    fn drop(&mut self) {
        // SAFETY: the file is an open uinput device; destroying it unplugs the pad.
        unsafe { libc::ioctl(self.file.as_raw_fd(), UI_DEV_DESTROY) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_to_state() {
        let state = State::from_report(0x1000 | 0x8 | 0x1, 255, 7, &[100, 200, -300, i16::MIN]);
        assert!(state.keys[7]);
        assert!(!state.keys[8]);
        assert_eq!(state.axes[0], 100);
        assert_eq!(state.axes[1], -200);
        assert_eq!(state.axes[2], 255);
        assert_eq!(state.axes[3], -300);
        assert_eq!(state.axes[4], 32767);
        assert_eq!(state.axes[5], 7);
        assert_eq!(state.axes[6], 1);
        assert_eq!(state.axes[7], -1);
    }

    #[test]
    fn ioctl_numbers_match_the_kernel() {
        assert_eq!(UI_DEV_CREATE, 0x5501);
        assert_eq!(UI_SET_EVBIT, 0x4004_5564);
        assert_eq!(UI_DEV_SETUP, 0x405c_5503);
        assert_eq!(UI_ABS_SETUP, 0x401c_5504);
        assert_eq!(UI_GET_SYSNAME, 0x8040_552c);
    }
}
