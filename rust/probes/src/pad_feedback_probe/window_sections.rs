//! Probe window and absolute-mouse / multi-touch injection sections for `pad_feedback_probe`.

use crate::harness::{Load, spin_until, stats};
use anyhow::{Result, ensure};
use butterpollo_input_policy::input::Input;
use butterpollo_win_graphics::capture::Priority;
use butterpollo_win_io::input::Injector;
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::LibraryLoader::GetModuleHandleW,
        UI::{HiDpi::*, Input::Pointer::*, WindowsAndMessaging::*},
    },
    core::w,
};

/// A pointer message the probe's window received: (message, pointer id,
/// frame id, x, y, flags).
type PointerMessage = (u32, u32, u32, i32, i32, u32);
static POINTER: Mutex<Vec<PointerMessage>> = Mutex::new(Vec::new());

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if (WM_POINTERUPDATE..=WM_POINTERUP).contains(&msg) {
        let id = (wparam.0 & 0xffff) as u32;
        let mut info = POINTER_INFO::default();
        // SAFETY: the pointer id came with this message.
        if unsafe { GetPointerInfo(id, &mut info) }.is_ok() {
            POINTER.lock().unwrap().push((
                msg,
                id,
                info.frameId,
                info.ptPixelLocation.x,
                info.ptPixelLocation.y,
                info.pointerFlags.0,
            ));
        }
        return LRESULT(0);
    }
    if matches!(
        msg,
        WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP
    ) {
        BUTTONS.lock().unwrap().push((Instant::now(), msg));
        return LRESULT(0);
    }
    if msg == WM_MOUSEACTIVATE {
        return LRESULT(MA_NOACTIVATE as isize);
    }
    if msg == WM_DESTROY {
        // SAFETY: on the window's thread.
        unsafe { PostQuitMessage(0) };
        return LRESULT(0);
    }
    // SAFETY: the arguments came with the message.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Mouse buttons the probe's window received, with their arrival time.
static BUTTONS: Mutex<Vec<(Instant, u32)>> = Mutex::new(Vec::new());

/// A small topmost window in the upper left of the primary display (x 5-35 %,
/// y 10-40 %) that takes the probe's touches and clicks; every point is
/// checked to be on it before anything is injected, so nothing else on the
/// desktop (a dialog in the middle) gets them. It is closed when dropped.
struct ProbeWindow {
    hwnd: usize,
    ui: Option<std::thread::JoinHandle<()>>,
    /// The primary display's name, for the injector.
    name: String,
    monitor: RECT,
}
impl ProbeWindow {
    fn open() -> Result<Self> {
        // SAFETY: plain Win32 calls; the monitor info is sized.
        let (monitor, name) = unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let handle = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
            let mut info = MONITORINFOEXW::default();
            info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
            ensure!(GetMonitorInfoW(handle, (&mut info as *mut MONITORINFOEXW).cast()).as_bool());
            let end = info.szDevice.iter().position(|&c| c == 0).unwrap_or(32);
            (
                info.monitorInfo.rcMonitor,
                String::from_utf16_lossy(&info.szDevice[..end]),
            )
        };
        let (w, h) = (monitor.right - monitor.left, monitor.bottom - monitor.top);
        let (window_tx, window_rx) = std::sync::mpsc::channel();
        let ui = std::thread::spawn(move || {
            // SAFETY: the window class and window live on this thread, which
            // pumps their messages until WM_DESTROY.
            unsafe {
                let instance = GetModuleHandleW(None).unwrap();
                let class = WNDCLASSW {
                    lpfnWndProc: Some(window_proc),
                    hInstance: instance.into(),
                    lpszClassName: w!("PadFeedbackProbeTouch"),
                    hbrBackground: HBRUSH(GetStockObject(DKGRAY_BRUSH).0),
                    ..Default::default()
                };
                RegisterClassW(&class);
                let hwnd = CreateWindowExW(
                    WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    w!("PadFeedbackProbeTouch"),
                    w!("touch probe"),
                    WS_POPUP | WS_VISIBLE,
                    monitor.left + w * 5 / 100,
                    monitor.top + h * 10 / 100,
                    w * 30 / 100,
                    h * 30 / 100,
                    None,
                    None,
                    Some(instance.into()),
                    None,
                )
                .unwrap();
                window_tx.send(hwnd.0 as usize).unwrap();
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        });
        let hwnd = window_rx.recv()?;
        std::thread::sleep(Duration::from_millis(500));
        Ok(Self {
            hwnd,
            ui: Some(ui),
            name,
            monitor,
        })
    }
    /// Fails unless each point (normalized on the display) is on this window.
    fn ensure_owns(&self, points: &[(f32, f32)]) -> Result<()> {
        let (w, h) = (
            (self.monitor.right - self.monitor.left) as f32,
            (self.monitor.bottom - self.monitor.top) as f32,
        );
        for &(x, y) in points {
            let point = POINT {
                x: self.monitor.left + (x * w) as i32,
                y: self.monitor.top + (y * h) as i32,
            };
            // SAFETY: plain Win32 calls.
            let root = unsafe { GetAncestor(WindowFromPoint(point), GA_ROOT) };
            ensure!(
                root.0 as usize == self.hwnd,
                "another window covers the probe's window at {x},{y}; nothing injected"
            );
        }
        Ok(())
    }
}
impl Drop for ProbeWindow {
    fn drop(&mut self) {
        // SAFETY: the window belongs to the UI thread; WM_CLOSE destroys it there.
        unsafe {
            let _ = PostMessageW(
                Some(HWND(self.hwnd as *mut _)),
                WM_CLOSE,
                WPARAM(0),
                LPARAM(0),
            );
        }
        if let Some(ui) = self.ui.take() {
            let _ = ui.join();
        }
    }
}

/// absolute_left_release_delay: a click and a long press (left release, then
/// a right press at once, as Moonlight sends it) in absolute mode, into the
/// probe's window. The cursor goes back where it was.
pub(crate) fn release_section() -> Result<()> {
    println!("== absolute mode: click and long press into this probe's window ==");
    let window = ProbeWindow::open()?;
    window.ensure_owns(&[(0.2, 0.25)])?;
    let mut cursor = POINT::default();
    // SAFETY: the point is writable.
    unsafe {
        let _ = GetCursorPos(&mut cursor);
    }
    for delay in [0, 10] {
        let config = butterpollo_config::config::Config::parse(&format!(
            "absolute_left_release_delay = {delay}\n"
        ))?;
        let mut injector = Injector::new_options(&window.name, "vhf", &config)?;
        let button = |button: u8, down: bool| Input::MouseButton { button, down };
        let middle = Input::Absolute {
            x: 200,
            y: 250,
            width: 1000,
            height: 1000,
        };
        for (label, events) in [
            ("click", vec![button(1, true), button(1, false)]),
            (
                "long press",
                vec![
                    button(1, true),
                    button(1, false),
                    button(3, true),
                    button(3, false),
                ],
            ),
        ] {
            for round in 0..5 {
                injector.apply(&middle)?;
                std::thread::sleep(Duration::from_millis(30));
                BUTTONS.lock().unwrap().clear();
                let t = Instant::now();
                // A press, then the rest of the gesture in the next pass.
                injector.apply(&events[0])?;
                injector.apply_all(&events[1..]);
                while t.elapsed() < Duration::from_millis(80) {
                    injector.due()?;
                    spin_until(Instant::now() + Duration::from_micros(200));
                }
                std::thread::sleep(Duration::from_millis(30));
                let seen: Vec<_> = BUTTONS
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|(at, msg)| {
                        let name = match *msg {
                            WM_LBUTTONDOWN => "L down",
                            WM_LBUTTONUP => "L up",
                            WM_RBUTTONDOWN => "R down",
                            _ => "R up",
                        };
                        format!(
                            "{name} +{:.1} ms",
                            at.saturating_duration_since(t).as_secs_f64() * 1e3
                        )
                    })
                    .collect();
                if round < 2 {
                    println!("delay {delay} ms, {label} {round}: {}", seen.join(", "));
                }
            }
        }
        drop(injector);
    }
    // SAFETY: plain Win32 call.
    unsafe {
        let _ = SetCursorPos(cursor.x, cursor.y);
    }
    Ok(())
}

pub(crate) fn pinch_section(loads: &[String]) -> Result<()> {
    println!("== touch: two fingers and one finger, into this probe's window ==");
    let window = ProbeWindow::open()?;
    window.ensure_owns(&[(0.15, 0.25), (0.25, 0.25), (0.19, 0.27), (0.21, 0.27)])?;
    let name = window.name.clone();
    let loads = loads.to_vec();
    let worker = std::thread::spawn(move || -> Result<()> {
        let _p = Priority::input();
        let mut injector = Injector::new(&name, "vhf")?;
        let touch = |event: u8, id: u32, x: f32, y: f32| Input::Touch {
            event,
            id,
            x,
            y,
            pressure: 0.5,
            major: 0.,
            minor: 0.,
            rotation: u16::MAX,
        };
        for load in &loads {
            let _load = Load::start(load);
            for (label, fingers, twice) in [
                ("two fingers, A B per pass", 2u32, false),
                ("two fingers, A B A B per pass", 2, true),
                ("one finger", 1, false),
            ] {
                POINTER.lock().unwrap().clear();
                let start: Vec<_> = (0..fingers)
                    .map(|f| touch(1, 7 + f, 0.15 + 0.1 * f as f32, 0.25))
                    .collect();
                injector.apply_all(&start);
                std::thread::sleep(Duration::from_millis(20));
                let passes = 200;
                let mut times = Vec::new();
                for pass in 0..passes {
                    let step = pass as f32 / passes as f32 * 0.04;
                    let mut events = Vec::new();
                    for _ in 0..if twice { 2 } else { 1 } {
                        for f in 0..fingers {
                            let x = if f == 0 { 0.15 + step } else { 0.25 - step };
                            events.push(touch(3, 7 + f, x, 0.25 + step / 2.));
                        }
                    }
                    let t = Instant::now();
                    for error in injector.apply_all(&events) {
                        println!("touch failed: {error:#}");
                    }
                    times.push(t.elapsed().as_secs_f64() * 1e6);
                    spin_until(t + Duration::from_millis(4));
                }
                let end: Vec<_> = (0..fingers).map(|f| touch(2, 7 + f, 0.2, 0.25)).collect();
                injector.apply_all(&end);
                std::thread::sleep(Duration::from_millis(150));
                let seen = POINTER.lock().unwrap().clone();
                let updates: Vec<_> = seen.iter().filter(|m| m.0 == WM_POINTERUPDATE).collect();
                let frames: std::collections::BTreeSet<_> = updates.iter().map(|m| m.2).collect();
                // What a lone finger's window sees, without ids and frame ids,
                // to compare builds.
                let mut digest = 0xcbf29ce484222325u64;
                for m in &seen {
                    for v in [m.0 as i64, m.3 as i64, m.4 as i64, m.5 as i64] {
                        digest = (digest ^ v as u64).wrapping_mul(0x100000001b3);
                    }
                }
                stats(&format!("[{load}] {label}: apply_all per pass"), times);
                println!(
                    "[{load}] {label}: {} messages, {} updates in {} frames over {passes} passes ({:.2} frames per pass); window digest {digest:016x}",
                    seen.len(),
                    updates.len(),
                    frames.len(),
                    frames.len() as f64 / passes as f64
                );
            }
        }
        Ok(())
    });
    let result = worker.join().unwrap();
    drop(window);
    result
}
