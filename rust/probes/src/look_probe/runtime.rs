//! Win32 window, raw input, XInput polling, timing, and CSV helpers for `look_probe`.

use crate::look::{MouseHistory, qpc_us};
use anyhow::{Context, Result, bail};
use std::{
    io::{BufWriter, Write},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
use windows::Win32::{
    Foundation::*,
    System::Performance::*,
    UI::{
        Input::{KeyboardAndMouse::VK_ESCAPE, *},
        WindowsAndMessaging::*,
    },
};

pub(crate) struct Window(pub(crate) HWND);
impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.0);
        }
    }
}

pub(crate) struct Handle(pub(crate) HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub(crate) static DISPLAY_CHANGED: AtomicBool = AtomicBool::new(false);

pub(crate) unsafe extern "system" fn window(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // A game hides the pointer over its picture; a moving pointer is neither scene nor HUD.
    if message == WM_SETCURSOR && (lparam.0 & 0xFFFF) as u32 == HTCLIENT {
        unsafe { SetCursor(None) };
        return LRESULT(1);
    }
    if message == WM_DISPLAYCHANGE {
        DISPLAY_CHANGED.store(true, Ordering::Relaxed);
    }
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

pub(crate) fn now_ticks() -> i64 {
    let mut ticks = 0;
    // SAFETY: QueryPerformanceCounter writes one i64; it cannot fail since Windows XP.
    let _ = unsafe { QueryPerformanceCounter(&mut ticks) };
    ticks
}

pub(crate) fn now_us(frequency: i64) -> i64 {
    qpc_us(now_ticks(), frequency)
}

type GetState = unsafe extern "system" fn(u32, *mut [u32; 4]) -> u32;

/// XInput from System32, loaded at run time as the host's own pad code does.
pub(crate) struct XInput {
    _library: libloading::Library,
    get: GetState,
}
impl XInput {
    pub(crate) fn load() -> Result<Self> {
        let system = std::env::var("SystemRoot").context("SystemRoot is not set")?;
        // SAFETY: xinput1_4 from System32 has no loader requirements, the signature is
        // XInputGetState's (XINPUT_STATE is 16 bytes, 4-aligned), and the library is kept
        // as long as the pointer.
        unsafe {
            let library = libloading::Library::new(format!(r"{system}\System32\xinput1_4.dll"))?;
            let get = *library.get::<GetState>(b"XInputGetState\0")?;
            Ok(Self {
                _library: library,
                get,
            })
        }
    }
    /// The packet number and right stick of pad `index`, or None when it is not connected.
    pub(crate) fn right_stick(&self, index: u32) -> Option<(u32, (i16, i16))> {
        let mut state = [0u32; 4];
        // SAFETY: `state` has XINPUT_STATE's size and alignment and outlives the call.
        let result = unsafe { (self.get)(index, &mut state) };
        // dwPacketNumber; wButtons and the triggers; the left stick; the right stick.
        (result == 0).then(|| {
            let right = state[3];
            (state[0], (right as u16 as i16, (right >> 16) as u16 as i16))
        })
    }
}

/// Raw input as it arrives, and the counts the report gives.
pub(crate) struct Input {
    pub(crate) frequency: i64,
    pub(crate) mouse: MouseHistory,
    pub(crate) injected: u64,
    pub(crate) device: u64,
    pub(crate) absolute_ignored: u64,
    pub(crate) errors: u64,
    pub(crate) escape: bool,
}
impl Input {
    /// Handles every queued message. Raw input is read here, timestamped as it is handled,
    /// and still dispatched so DefWindowProc cleans up after it.
    pub(crate) fn pump(&mut self) -> Result<()> {
        let mut message = MSG::default();
        // SAFETY: message pumping on the thread that owns the window.
        unsafe {
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                if message.message == WM_QUIT {
                    bail!("test window closed");
                }
                if message.message == WM_INPUT {
                    self.raw(HRAWINPUT(message.lParam.0 as *mut _));
                }
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        Ok(())
    }

    fn raw(&mut self, handle: HRAWINPUT) {
        let at_us = now_us(self.frequency);
        let mut packet = RAWINPUT::default();
        let mut size = size_of::<RAWINPUT>() as u32;
        // SAFETY: `packet` has room for `size` bytes, which holds a mouse or keyboard
        // packet, the only devices registered; the handle is this WM_INPUT's.
        let read = unsafe {
            GetRawInputData(
                handle,
                RID_INPUT,
                Some((&raw mut packet).cast()),
                &mut size,
                size_of::<RAWINPUTHEADER>() as u32,
            )
        };
        if read == 0 || read == u32::MAX {
            self.errors += 1;
            return;
        }
        if packet.header.dwType == RIM_TYPEMOUSE.0 {
            // SAFETY: the header says the packet is a mouse's.
            let mouse = unsafe { packet.data.mouse };
            if mouse.usFlags.0 & MOUSE_MOVE_ABSOLUTE.0 != 0 {
                self.absolute_ignored += 1;
            } else if mouse.lLastX != 0 || mouse.lLastY != 0 {
                // SendInput's input has no device.
                let injected = packet.header.hDevice.0.is_null();
                if injected {
                    self.injected += 1;
                } else {
                    self.device += 1;
                }
                self.mouse.push(at_us, mouse.lLastX, mouse.lLastY, injected);
            }
        } else if packet.header.dwType == RIM_TYPEKEYBOARD.0 {
            // SAFETY: the header says the packet is a keyboard's.
            let key = unsafe { packet.data.keyboard };
            if key.VKey == VK_ESCAPE.0 && u32::from(key.Flags) & RI_KEY_BREAK == 0 {
                self.escape = true;
            }
        }
    }
}

/// Waits for `handle` while handling messages as they arrive, so raw input is timestamped
/// when it comes rather than once a frame. False on a timeout.
pub(crate) fn wait_pumping(handle: HANDLE, timeout_ms: u32, input: &mut Input) -> Result<bool> {
    loop {
        // SAFETY: the handle is a live waitable object the caller owns.
        let result = unsafe {
            MsgWaitForMultipleObjectsEx(
                Some(&[handle]),
                timeout_ms,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            )
        };
        if result == WAIT_OBJECT_0 {
            return Ok(true);
        }
        if result == WAIT_TIMEOUT {
            return Ok(false);
        }
        if result.0 != WAIT_OBJECT_0.0 + 1 {
            bail!("wait failed: {}", windows::core::Error::from_thread());
        }
        input.pump()?;
    }
}

pub(crate) struct FrameRow {
    pub(crate) render_us: i64,
    pub(crate) present_us: i64,
    pub(crate) input_us: i64,
    pub(crate) mouse: [i64; 2],
    pub(crate) stick: [f64; 2],
    pub(crate) camera: [f64; 2],
}

/// A dead zone or exponent as given, without f32's tail.
pub(crate) fn short(value: f32) -> f64 {
    (f64::from(value) * 1e6).round() / 1e6
}

pub(crate) fn write_csv(
    path: &Path,
    header: &str,
    rows: impl FnOnce(&mut dyn Write) -> std::io::Result<()>,
) -> Result<()> {
    let file =
        std::fs::File::create(path).with_context(|| format!("cannot create {}", path.display()))?;
    let mut out = BufWriter::new(file);
    writeln!(out, "{header}")?;
    rows(&mut out)?;
    out.flush()?;
    Ok(())
}
