//! Foreground-window tracking and competing repainter window for `motion_probe`.

use anyhow::{Context, Result, bail};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::{
            BeginPaint, DKGRAY_BRUSH, EndPaint, FillRect, GetStockObject, HBRUSH, InvalidateRect,
            PAINTSTRUCT, UpdateWindow, WHITE_BRUSH,
        },
        System::{LibraryLoader::GetModuleHandleW, Threading::*},
        UI::WindowsAndMessaging::*,
    },
    core::{PWSTR, w},
};

pub(crate) struct Window(pub(crate) HWND);
impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.0);
        }
    }
}

/// A window of the probe as the foreground window: the strip
/// (`BUTTERPOLLO_TEST_MOTION_FOREGROUND`, stage `foreground`) or the
/// repainted window in front of it (`BUTTERPOLLO_TEST_MOTION_REPAINTER`,
/// stage `repainter`). The host paces a stream by the foreground window
/// while that one animates (`capture/focus.rs`, `Freshness::observe_focus`)
/// and holds other windows' updates for its next picture then, so which
/// window is in front decides what a run measures. The window that was in
/// front gets the foreground back when the probe ends by itself; when the
/// test ends it, Windows picks the window below.
pub(crate) struct Front {
    window: HWND,
    stage: &'static str,
    before: HWND,
    /// Frames in a row the window was seen behind another window.
    behind: u32,
}
impl Front {
    /// Every `taken=false` line is a time the window was seen behind
    /// another window, which the test reads as a result that is not the
    /// one it set up.
    pub(crate) fn take(window: HWND, stage: &'static str) -> Self {
        let before = unsafe { GetForegroundWindow() };
        if butterpollo_win_io::lossless::to_front_quietly(window) {
            println!("MOTION stage={stage} taken=true");
        } else {
            Self::report(stage);
        }
        Self {
            window,
            stage,
            before,
            behind: 0,
        }
    }
    /// Each frame: another window in front for two frames is said once
    /// (one frame can be Windows moving the foreground, which reads as no
    /// window at all), and the foreground is taken back then and about
    /// once a second for as long as that window keeps it.
    pub(crate) fn keep(&mut self) {
        if unsafe { GetForegroundWindow() } == self.window {
            self.behind = 0;
            return;
        }
        self.behind += 1;
        if self.behind == 2 {
            Self::report(self.stage);
        }
        if self.behind == 2 || self.behind.is_multiple_of(64) {
            let _ = butterpollo_win_io::lossless::to_front_quietly(self.window);
        }
    }
    fn report(stage: &str) {
        let holder = Self::holder();
        println!("MOTION stage={stage} taken=false holder={holder}");
    }
    /// The program of the window in front: its file name, as a window
    /// title can be a document's.
    fn holder() -> String {
        // SAFETY: `pid` and `length` are live out-pointers, `path` holds
        // `length` UTF-16 units, and the process handle is closed once.
        unsafe {
            let window = GetForegroundWindow();
            if window.is_invalid() {
                return "none".to_owned();
            }
            let mut pid = 0;
            GetWindowThreadProcessId(window, Some(&mut pid));
            let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                return format!("process {pid}");
            };
            let mut path = [0u16; 1024];
            let mut length = path.len() as u32;
            let named = QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(path.as_mut_ptr()),
                &mut length,
            );
            let _ = CloseHandle(process);
            if named.is_err() {
                return format!("process {pid}");
            }
            let path = String::from_utf16_lossy(&path[..length as usize]);
            path.rsplit('\\').next().unwrap_or(&path).to_owned()
        }
    }
}
impl Drop for Front {
    fn drop(&mut self) {
        // SAFETY: the calls take window handles by value, and a stale one
        // just fails. Allowed while the window is the foreground window,
        // which it outlives this for.
        unsafe {
            if GetForegroundWindow() == self.window && !self.before.is_invalid() {
                let _ = SetForegroundWindow(self.before);
            }
        }
    }
}

/// Another window on the strip's display, repainted by a thread of its own
/// (`BUTTERPOLLO_TEST_MOTION_REPAINTER`). `EVERY_MS:BURST:GAP_MS`: a window
/// in front of the strip, the foreground window, repainted in bursts of
/// BURST repaints GAP_MS apart every EVERY_MS (a terminal printing a line).
/// The host must stream the strip behind it as without it (a 60 fps strip
/// lost 3.5 fps to 50 ms holds behind such a window: rust/PERFORMANCE.md,
/// November 20). `beside:HZ`: a window beside the strip repainted HZ times
/// a second that never takes the foreground (an app animating next to a
/// game), whose updates must not cost the strip a frame.
pub(crate) enum Paint {
    Bursts {
        every: Duration,
        burst: u32,
        gap: Duration,
    },
    Beside(Duration),
}
impl Paint {
    pub(crate) fn parse(text: &str) -> Result<Self> {
        if let Some(hz) = text.strip_prefix("beside:") {
            let hz: u32 = hz.parse().context("beside:HZ needs a whole number")?;
            if !(1..=1000).contains(&hz) {
                bail!("beside:HZ must be 1..1000");
            }
            return Ok(Self::Beside(Duration::from_secs(1) / hz));
        }
        let parts: Vec<u64> = text
            .split(':')
            .map(str::parse)
            .collect::<Result<_, _>>()
            .context("expected EVERY_MS:BURST:GAP_MS or beside:HZ")?;
        let [every, burst, gap] = parts[..] else {
            bail!("expected EVERY_MS:BURST:GAP_MS or beside:HZ");
        };
        if burst == 0 || burst > 1000 || gap == 0 || every <= (burst - 1) * gap {
            bail!("a burst must fit in EVERY_MS");
        }
        Ok(Self::Bursts {
            every: Duration::from_millis(every),
            burst: burst as u32,
            gap: Duration::from_millis(gap),
        })
    }
    pub(crate) fn in_front(&self) -> bool {
        matches!(self, Self::Bursts { .. })
    }
    /// The wait after repaint `index` (from 0).
    fn after(&self, index: u64) -> Duration {
        match *self {
            Self::Bursts { every, burst, gap } => {
                if index % u64::from(burst) + 1 < u64::from(burst) {
                    gap
                } else {
                    every - gap * (burst - 1)
                }
            }
            Self::Beside(interval) => interval,
        }
    }
}

static PAINTS: AtomicU32 = AtomicU32::new(0);

unsafe extern "system" fn painted(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_PAINT {
        // SAFETY: `paint` lives across BeginPaint and EndPaint, called once
        // each for this window's WM_PAINT.
        unsafe {
            let mut paint = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut paint);
            let brush = if PAINTS.load(Ordering::Relaxed).is_multiple_of(2) {
                WHITE_BRUSH
            } else {
                DKGRAY_BRUSH
            };
            FillRect(dc, &paint.rcPaint, HBRUSH(GetStockObject(brush).0));
            let _ = EndPaint(hwnd, &paint);
        }
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

/// The thread of the repainted window: it creates the window, holds the
/// foreground in front of the strip or stays beside it, and repaints it
/// on the schedule until `stop`.
pub(crate) fn paint(x: i32, y: i32, schedule: Paint, stop: Arc<AtomicBool>) -> Result<()> {
    // SAFETY: the window belongs to this thread, which pumps its messages
    // and destroys it (`Window`) before it ends.
    unsafe {
        let module = GetModuleHandleW(None)?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(painted),
            hInstance: HINSTANCE(module.0),
            lpszClassName: w!("ButterpolloMotionRepainted"),
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            bail!("cannot register the repainted window");
        }
        let style = if schedule.in_front() {
            WS_EX_TOOLWINDOW
        } else {
            WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW
        };
        let window = Window(CreateWindowExW(
            style,
            w!("ButterpolloMotionRepainted"),
            w!("Rubylight test repaints"),
            WS_POPUP | WS_VISIBLE,
            x + 64,
            y + 64,
            480,
            240,
            None,
            None,
            Some(HINSTANCE(module.0)),
            None,
        )?);
        let place = if schedule.in_front() {
            "in-front"
        } else {
            "beside"
        };
        println!("MOTION stage=repainted place={place}");
        let mut front = schedule
            .in_front()
            .then(|| Front::take(window.0, "repainter"));
        let timer = butterpollo_win_graphics::timing::Timer::new()?;
        let mut next = Instant::now();
        let mut index = 0;
        while !stop.load(Ordering::Relaxed) {
            timer.until(next);
            let mut message = MSG::default();
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            PAINTS.fetch_add(1, Ordering::Relaxed);
            let _ = InvalidateRect(Some(window.0), None, false);
            let _ = UpdateWindow(window.0);
            if let Some(front) = &mut front {
                front.keep();
            }
            next = (next + schedule.after(index)).max(Instant::now());
            index += 1;
        }
        println!(
            "MOTION stage=repainted paints={}",
            PAINTS.load(Ordering::Relaxed)
        );
    }
    Ok(())
}

/// Ends the repainting thread with the probe, also on an error.
pub(crate) struct Painter {
    pub(crate) stop: Arc<AtomicBool>,
    pub(crate) thread: Option<std::thread::JoinHandle<Result<()>>>,
}
impl Drop for Painter {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            match thread.join() {
                Ok(Err(error)) => println!("MOTION stage=repainted error={error:#}"),
                Err(_) => println!("MOTION stage=repainted error=panicked"),
                Ok(Ok(())) => {}
            }
        }
    }
}
