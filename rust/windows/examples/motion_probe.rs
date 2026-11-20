//! Test-owned moving desktop with a timestamp in the pixels. Not packaged.
//! Pair with tests/moonlight_client.c to measure picture age independently of
//! either host's processing counter. Requires an explicitly selected display.
//! Animation pacing never changes the display's resolution, refresh rate or HDR.
//!
//! The animation argument is `current` (present at the display's refresh), a
//! rate in Hz, or a script of intervals, the cadence a game would have:
//! `jitter:8,14,9,13` repeats those intervals in milliseconds, and
//! `hitch:90:100[:EVERY]` runs at 90 Hz with one 100 ms interval every EVERY
//! frames (default two seconds' worth). Every present still waits for the
//! swap chain, so a script slower than the display is exact and a faster one
//! is quantised to the display's refresh, as a vsynced game is.
//!
//! After the strip height, `noise` fills the window above the bottom 128 rows
//! with fresh noise every frame; `cuts:MS[:FRESH_ROWS]` fills it with noise
//! that changes every MS milliseconds (a scene cut), except FRESH_ROWS just
//! above the band that change every frame, sizing the frames between cuts.

#[cfg_attr(not(windows), allow(dead_code))]
mod schedule {
    use anyhow::{Result, bail};
    use std::time::Duration;

    /// Intervals between the probe's presents.
    #[derive(Clone, Debug, PartialEq)]
    pub enum Schedule {
        Fixed(Duration),
        Cycle(Vec<Duration>),
        Hitch {
            period: Duration,
            hitch: Duration,
            every: u64,
        },
    }

    fn millis(text: &str) -> Result<Duration> {
        let ms: f64 = text.trim().parse()?;
        if !(0.5..=5000.0).contains(&ms) {
            bail!("interval {ms} ms must be 0.5..5000 ms");
        }
        Ok(Duration::from_secs_f64(ms / 1000.0))
    }

    impl Schedule {
        pub fn parse(text: &str) -> Result<Self> {
            if let Some(list) = text.strip_prefix("jitter:") {
                let cycle = list.split(',').map(millis).collect::<Result<Vec<_>>>()?;
                if cycle.len() < 2 {
                    bail!("jitter needs at least two intervals, e.g. jitter:8,14,9,13");
                }
                return Ok(Self::Cycle(cycle));
            }
            if let Some(rest) = text.strip_prefix("hitch:") {
                let mut parts = rest.split(':');
                let hz: f64 = parts.next().unwrap_or("").parse()?;
                if !(1.0..=1000.0).contains(&hz) {
                    bail!("hitch rate must be 1..1000 Hz");
                }
                let hitch = millis(parts.next().unwrap_or(""))?;
                let every = match parts.next() {
                    Some(n) => n.parse()?,
                    None => (hz * 2.0).round() as u64,
                };
                if every < 2 || parts.next().is_some() {
                    bail!("usage: hitch:HZ:HITCH_MS[:EVERY_FRAMES>=2]");
                }
                return Ok(Self::Hitch {
                    period: Duration::from_secs_f64(1.0 / hz),
                    hitch,
                    every,
                });
            }
            let hz: u32 = text.parse()?;
            if !(1..=1000).contains(&hz) {
                bail!("animation rate must be 1..1000 Hz");
            }
            Ok(Self::Fixed(Duration::from_secs_f64(1.0 / f64::from(hz))))
        }

        /// The wait before frame `index` (counted from zero).
        pub fn interval(&self, index: u64) -> Duration {
            match self {
                Self::Fixed(period) => *period,
                Self::Cycle(cycle) => cycle[(index % cycle.len() as u64) as usize],
                Self::Hitch {
                    period,
                    hitch,
                    every,
                } => {
                    if index % every == every - 1 {
                        *hitch
                    } else {
                        *period
                    }
                }
            }
        }

        pub fn fixed_hz(&self) -> Option<u32> {
            match self {
                Self::Fixed(period) => Some((1.0 / period.as_secs_f64()).round() as u32),
                _ => None,
            }
        }
    }

    /// Scene cuts: the whole picture changes every `every`, and `fresh_rows`
    /// change every frame.
    #[derive(Clone, Debug, PartialEq)]
    pub struct Cuts {
        pub every: Duration,
        pub fresh_rows: u32,
    }

    impl Cuts {
        /// `MS[:FRESH_ROWS]`, the text after `cuts:`.
        pub fn parse(text: &str) -> Result<Self> {
            let mut parts = text.split(':');
            let every = millis(parts.next().unwrap_or(""))?;
            let fresh_rows = parts.next().map(str::parse).transpose()?.unwrap_or(0);
            if parts.next().is_some() {
                bail!("usage: cuts:MS[:FRESH_ROWS]");
            }
            Ok(Self { every, fresh_rows })
        }

        /// The picture's seed `elapsed` after the start, from 1: the same
        /// between cuts and one higher after each.
        pub fn seed(&self, elapsed: Duration) -> u32 {
            (elapsed.as_nanos() / self.every.as_nanos()) as u32 + 1
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn us(d: Duration) -> u128 {
            d.as_micros()
        }

        #[test]
        fn cuts_change_the_seed_once_per_interval() {
            let cuts = Cuts::parse("2000:96").unwrap();
            assert_eq!(cuts.fresh_rows, 96);
            assert_eq!(cuts.seed(Duration::ZERO), 1);
            assert_eq!(cuts.seed(Duration::from_millis(1999)), 1);
            assert_eq!(cuts.seed(Duration::from_millis(2000)), 2);
            assert_eq!(cuts.seed(Duration::from_millis(5000)), 3);
            assert_eq!(Cuts::parse("500").unwrap().fresh_rows, 0);
            assert!(Cuts::parse("").is_err());
            assert!(Cuts::parse("2000:x").is_err());
            assert!(Cuts::parse("2000:1:2").is_err());
        }

        #[test]
        fn a_rate_is_a_fixed_interval() {
            let fixed = Schedule::parse("90").unwrap();
            assert_eq!(fixed.fixed_hz(), Some(90));
            assert_eq!(us(fixed.interval(0)), 11_111);
            assert_eq!(us(fixed.interval(12_345)), 11_111);
            assert!(Schedule::parse("0").is_err());
            assert!(Schedule::parse("1001").is_err());
            assert!(Schedule::parse("fast").is_err());
        }

        #[test]
        fn jitter_cycles_through_its_intervals() {
            let script = Schedule::parse("jitter:8,14,9,13.5").unwrap();
            let seen: Vec<_> = (0..6).map(|i| us(script.interval(i))).collect();
            assert_eq!(seen, [8000, 14000, 9000, 13500, 8000, 14000]);
            assert_eq!(script.fixed_hz(), None);
            assert!(Schedule::parse("jitter:8").is_err());
            assert!(Schedule::parse("jitter:8,x").is_err());
            assert!(Schedule::parse("jitter:8,0.1").is_err());
        }

        #[test]
        fn a_hitch_comes_every_n_frames_and_replaces_one_interval() {
            let script = Schedule::parse("hitch:90:100:4").unwrap();
            let seen: Vec<_> = (0..8).map(|i| us(script.interval(i))).collect();
            assert_eq!(
                seen,
                [
                    11_111, 11_111, 11_111, 100_000, 11_111, 11_111, 11_111, 100_000
                ]
            );
            // Two seconds of frames by default.
            let default = Schedule::parse("hitch:90:100").unwrap();
            assert_eq!(us(default.interval(179)), 100_000);
            assert_eq!(us(default.interval(178)), 11_111);
            assert!(Schedule::parse("hitch:90").is_err());
            assert!(Schedule::parse("hitch:90:100:1").is_err());
            assert!(Schedule::parse("hitch:90:100:4:5").is_err());
        }
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This probe requires Windows.");
}

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    probe::run()
}

#[cfg(windows)]
mod probe {
    use anyhow::{Context, Result, bail};
    use butterpollo_windows::capture::{ComGuard, displays, enable_dpi_awareness};
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
            Graphics::{
                Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, Fxc::*},
                Direct3D11::*,
                Dxgi::{Common::*, *},
                Gdi::{
                    BeginPaint, DKGRAY_BRUSH, EndPaint, FillRect, GetStockObject, HBRUSH,
                    InvalidateRect, PAINTSTRUCT, UpdateWindow, WHITE_BRUSH,
                },
            },
            System::{LibraryLoader::GetModuleHandleW, Performance::*, Threading::*},
            UI::WindowsAndMessaging::*,
        },
        core::{Interface, PCSTR, PWSTR, w},
    };

    const SHADER: &str = r#"
cbuffer Frame : register(b0) {
    uint frame; uint ticksLow; uint ticksHigh; uint noise; uint cutSeed; uint freshRows; uint pad0; uint pad1;
};
float4 vertex(uint id : SV_VertexID) : SV_Position {
    float2 p = float2((id << 1) & 2, id & 2);
    return float4(p * float2(2, -2) + float2(-1, 1), 0, 1);
}
float4 picture(float4 p : SV_Position) : SV_Target {
    // With noise, the band below stays the window's bottom 128 pixels, as
    // receivers read it, and fresh noise above it costs the encoder bits.
    // With cuts, the noise changes only at each cut, except a strip of
    // freshRows just above the band that changes every frame.
    float y = p.y - (noise ? float(noise - 1) : 0);
    if (y < 0) {
        uint seed = (cutSeed != 0 && y < -float(freshRows)) ? cutSeed : frame;
        uint h = uint(p.x) * 1973u + uint(p.y) * 9277u + seed * 26699u;
        h = (h ^ (h >> 13)) * 1274126177u;
        h ^= h >> 16;
        return float4(float3(h & 255, (h >> 8) & 255, (h >> 16) & 255) / 255.0, 1);
    }
    // Four rows: sequence, QPC low, QPC high, and a signature. Calibration
    // patches at x=12/36 let the decoder read SDR and PQ HDR with the same test.
    if (y >= 8 && y < 104) {
        if (p.x < 24) return float4(0, 0, 0, 1);
        if (p.x < 48) return float4(1.25, 1.25, 1.25, 1);
        if (p.x >= 64 && p.x < 576) {
            uint row = uint(y - 8) / 24;
            uint word = row == 0 ? frame : row == 1 ? ticksLow : row == 2 ? ticksHigh : 0xB17E2212;
            float value = ((word >> (uint(p.x - 64) / 16)) & 1) * 1.25;
            return float4(value, value, value, 1);
        }
    }
    // Deterministic motion with edges, gradients and HDR highlights. No
    // expensive scene rendering that would obscure the host's own costs.
    float2 at = p.xy + float2(frame * 4, frame * 2);
    float grid = fmod(floor(at.x / 32) + floor(at.y / 32), 2);
    float3 color = lerp(float3(0.02, 0.06, 0.12), float3(0.4, 0.22, 0.08), grid);
    float bar = fmod(at.x, 512);
    color += bar / 1024;
    if (bar < 8) color = float3(4, 2, 1);
    return float4(color, 1);
}
"#;

    struct Window(HWND);
    impl Drop for Window {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }
    struct Wait(HANDLE);
    impl Drop for Wait {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
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
    struct Front {
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
        fn take(window: HWND, stage: &'static str) -> Self {
            let before = unsafe { GetForegroundWindow() };
            if butterpollo_windows::lossless::to_front_quietly(window) {
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
        fn keep(&mut self) {
            if unsafe { GetForegroundWindow() } == self.window {
                self.behind = 0;
                return;
            }
            self.behind += 1;
            if self.behind == 2 {
                Self::report(self.stage);
            }
            if self.behind == 2 || self.behind.is_multiple_of(64) {
                let _ = butterpollo_windows::lossless::to_front_quietly(self.window);
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
    enum Paint {
        Bursts {
            every: Duration,
            burst: u32,
            gap: Duration,
        },
        Beside(Duration),
    }
    impl Paint {
        fn parse(text: &str) -> Result<Self> {
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
        fn in_front(&self) -> bool {
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
    fn paint(x: i32, y: i32, schedule: Paint, stop: Arc<AtomicBool>) -> Result<()> {
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
            let timer = butterpollo_windows::timing::Timer::new()?;
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
    struct Painter {
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<Result<()>>>,
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
    static DISPLAY_CHANGED: AtomicBool = AtomicBool::new(false);

    unsafe extern "system" fn window(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == WM_DISPLAYCHANGE {
            DISPLAY_CHANGED.store(true, Ordering::Relaxed);
        }
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }
    unsafe fn compile(entry: &'static [u8], target: &'static [u8]) -> Result<Vec<u8>> {
        unsafe {
            let mut code = None;
            let mut errors = None;
            let result = D3DCompile(
                SHADER.as_ptr().cast(),
                SHADER.len(),
                PCSTR::null(),
                None,
                None,
                PCSTR(entry.as_ptr()),
                PCSTR(target.as_ptr()),
                D3DCOMPILE_OPTIMIZATION_LEVEL3,
                0,
                &mut code,
                Some(&mut errors),
            );
            if let Err(error) = result {
                let details = errors
                    .map(|blob| {
                        String::from_utf8_lossy(std::slice::from_raw_parts(
                            blob.GetBufferPointer().cast(),
                            blob.GetBufferSize(),
                        ))
                        .into_owned()
                    })
                    .unwrap_or_default();
                bail!("motion shader: {error}: {details}");
            }
            let code = code.context("shader compiler returned no bytecode")?;
            Ok(
                std::slice::from_raw_parts(code.GetBufferPointer().cast(), code.GetBufferSize())
                    .to_vec(),
            )
        }
    }

    pub fn run() -> Result<()> {
        let mut args = std::env::args().skip(1);
        let display_name = args.next().context(
            "usage: motion_probe DISPLAY SECONDS [REPORT.json [ANIMATION_HZ|current|SCRIPT [STRIP_HEIGHT [noise|cuts:MS[:FRESH_ROWS]]]]]",
        )?;
        let seconds: u64 = args.next().context("duration is required")?.parse()?;
        let report = args.next();
        let animation_text = args.next().filter(|s| s != "current");
        let animation = animation_text
            .as_deref()
            .map(crate::schedule::Schedule::parse)
            .transpose()?;
        let animation_hz = animation.as_ref().and_then(|a| a.fixed_hz());
        let strip_height: Option<u32> = args.next().map(|s| s.parse()).transpose()?;
        // Per-pixel noise above the bottom 128 rows, new every frame: a
        // high-bitrate picture for encoder and network tests. With cuts, the
        // noise is new only every MS milliseconds (a scene cut the encoder
        // can't predict) and FRESH_ROWS above the band change every frame.
        let noise_text = args.next();
        let cuts = noise_text
            .as_deref()
            .and_then(|text| text.strip_prefix("cuts:"))
            .map(crate::schedule::Cuts::parse)
            .transpose()?;
        let noise = match noise_text.as_deref() {
            None => false,
            Some("noise") => true,
            Some(_) if cuts.is_some() => true,
            Some(other) => bail!("unknown option {other}; expected noise or cuts:MS[:FRESH_ROWS]"),
        };
        if !(1..=3600).contains(&seconds) || args.next().is_some() {
            bail!("duration must be 1..3600 seconds");
        }
        enable_dpi_awareness();
        let _com = ComGuard::new()?;
        let mut display = displays()?
            .into_iter()
            .find(|d| d.display_name.eq_ignore_ascii_case(&display_name))
            .context("explicitly selected display is not active")?;
        let window_height = strip_height.unwrap_or(display.height);
        if !(128..=display.height).contains(&window_height) {
            bail!("window height must be 128..display height");
        }
        let mut window_y = display.y + (display.height - window_height) as i32;
        let source_mode = butterpollo_windows::display::mode(&display.display_name)?;
        let timer = butterpollo_windows::timing::Timer::new()?;
        println!("MOTION stage=display name={}", display.display_name);
        if display.width < 640 || display.height < 128 {
            bail!("the motion barcode needs a display of at least 640 by 128");
        }
        unsafe {
            let module = GetModuleHandleW(None)?;
            let class = WNDCLASSW {
                lpfnWndProc: Some(window),
                hInstance: HINSTANCE(module.0),
                lpszClassName: w!("ButterpolloMotionProbe"),
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 {
                bail!("cannot register test window");
            }
            let window = Window(CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                w!("ButterpolloMotionProbe"),
                w!("Rubylight test motion"),
                WS_POPUP | WS_VISIBLE,
                display.x,
                window_y,
                display.width as i32,
                window_height as i32,
                None,
                None,
                Some(HINSTANCE(module.0)),
                None,
            )?);
            SetWindowPos(
                window.0,
                Some(HWND_TOPMOST),
                display.x,
                window_y,
                display.width as i32,
                window_height as i32,
                SWP_NOACTIVATE,
            )?;
            println!("MOTION stage=window");
            let repainted = std::env::var("BUTTERPOLLO_TEST_MOTION_REPAINTER")
                .ok()
                .map(|text| Paint::parse(&text))
                .transpose()?;
            let behind = repainted.as_ref().is_some_and(Paint::in_front);
            // After `window`, so it ends first and hands the foreground back
            // while the strip still has it. A window repainted in front of
            // the strip takes the foreground instead.
            let mut front = (std::env::var_os("BUTTERPOLLO_TEST_MOTION_FOREGROUND").is_some()
                && !behind)
                .then(|| Front::take(window.0, "foreground"));
            let _painter = repainted.map(|schedule| {
                let stop = Arc::new(AtomicBool::new(false));
                let (x, y, flag) = (display.x, display.y, stop.clone());
                Painter {
                    stop,
                    thread: Some(std::thread::spawn(move || paint(x, y, schedule, flag))),
                }
            });
            let factory: IDXGIFactory2 = CreateDXGIFactory1()?;
            let adapter = factory.EnumAdapters1(display.adapter_index)?;
            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
            let device = device.context("no D3D11 device")?;
            let context = context.context("no D3D11 context")?;
            println!("MOTION stage=device");
            let swap = factory.CreateSwapChainForHwnd(
                &device,
                window.0,
                &DXGI_SWAP_CHAIN_DESC1 {
                    Width: display.width,
                    Height: window_height,
                    Format: DXGI_FORMAT_R16G16B16A16_FLOAT,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                    BufferCount: 2,
                    SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                    Scaling: DXGI_SCALING_STRETCH,
                    AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                    Flags: DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32,
                    ..Default::default()
                },
                None,
                None,
            )?;
            let swap: IDXGISwapChain3 = swap.cast()?;
            println!("MOTION stage=swap-chain");
            swap.SetColorSpace1(DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709)?;
            swap.SetMaximumFrameLatency(1)?;
            let wait = Wait(swap.GetFrameLatencyWaitableObject());
            if wait.0.is_invalid() {
                bail!("swap chain has no presentation event");
            }
            let texture: ID3D11Texture2D = swap.GetBuffer(0)?;
            let mut target = None;
            device.CreateRenderTargetView(&texture, None, Some(&mut target))?;
            let mut vertex = None;
            let mut picture = None;
            device.CreateVertexShader(
                &compile(b"vertex\0", b"vs_5_0\0")?,
                None,
                Some(&mut vertex),
            )?;
            device.CreatePixelShader(
                &compile(b"picture\0", b"ps_5_0\0")?,
                None,
                Some(&mut picture),
            )?;
            let mut constants = None;
            device.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: 32,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut constants),
            )?;
            let constants = constants.unwrap();
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(vertex.as_ref(), None);
            context.PSSetShader(picture.as_ref(), None);
            context.PSSetConstantBuffers(0, Some(&[Some(constants.clone())]));
            context.RSSetViewports(Some(&[D3D11_VIEWPORT {
                Width: display.width as f32,
                Height: window_height as f32,
                MinDepth: 0.,
                MaxDepth: 1.,
                ..Default::default()
            }]));
            let mut frequency = 0;
            QueryPerformanceFrequency(&mut frequency)?;
            println!("MOTION stage=render-ready");
            let start = Instant::now();
            let mut progress_at = start + Duration::from_secs(5);
            let mut next_frame = start;
            let mut frames = Vec::new();
            let presentation_stats = std::env::var_os("BUTTERPOLLO_TEST_PRESENT_STATS").is_some();
            while start.elapsed() < Duration::from_secs(seconds) {
                if WaitForSingleObject(wait.0, 1000) != WAIT_OBJECT_0 {
                    bail!("presentation event timed out");
                }
                let planned = animation.as_ref().map(|a| a.interval(frames.len() as u64));
                if let Some(interval) = planned {
                    timer.until(next_frame);
                    // Bound catch-up after a stall to one frame. A delayed
                    // renderer must not manufacture a burst of fresh pictures.
                    next_frame = (next_frame + interval).max(Instant::now());
                }
                let mut message = MSG::default();
                while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    if message.message == WM_QUIT {
                        bail!("test window closed");
                    }
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                if DISPLAY_CHANGED.swap(false, Ordering::Relaxed)
                    && let Some(current) = displays()?
                        .into_iter()
                        .find(|d| d.display_name == display.display_name)
                {
                    if (current.width, current.height) != (display.width, display.height) {
                        bail!("the motion probe's selected display changed size");
                    }
                    // A layout change can move the output without moving this
                    // popup, leaving its barcode outside the captured desktop.
                    if (current.x, current.y) != (display.x, display.y) {
                        window_y = current.y + (current.height - window_height) as i32;
                        SetWindowPos(
                            window.0,
                            None,
                            current.x,
                            window_y,
                            0,
                            0,
                            SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOSIZE,
                        )?;
                        display = current;
                    }
                }
                let mut ticks = 0;
                QueryPerformanceCounter(&mut ticks)?;
                let frame = frames.len() as u32 + 1;
                let values = [
                    frame,
                    ticks as u32,
                    (ticks as u64 >> 32) as u32,
                    if noise { window_height - 127 } else { 0 },
                    cuts.as_ref().map_or(0, |c| c.seed(start.elapsed())),
                    cuts.as_ref().map_or(0, |c| c.fresh_rows),
                    0,
                    0,
                ];
                context.UpdateSubresource(&constants, 0, None, values.as_ptr().cast(), 0, 0);
                context.OMSetRenderTargets(Some(&[target.clone()]), None);
                context.Draw(3, 0);
                swap.Present(1, DXGI_PRESENT(0)).ok()?;
                if frame == 1 {
                    println!("MOTION stage=first-present");
                }
                let mut sample = serde_json::json!({"frame":frame,"qpc":ticks});
                if let Some(interval) = planned {
                    // The gap planned from this present to the next one: it is
                    // added to `next_frame` after this frame's own wait.
                    sample["planned_us"] = serde_json::json!(interval.as_micros());
                }
                if presentation_stats {
                    sample["present_id"] = serde_json::json!(swap.GetLastPresentCount().ok());
                    let mut stats = DXGI_FRAME_STATISTICS::default();
                    sample["displayed"] = swap.GetFrameStatistics(&mut stats).ok().map_or(
                        serde_json::Value::Null,
                        |()| {
                            serde_json::json!({
                                "present_id": stats.PresentCount,
                                "refresh_count": stats.PresentRefreshCount,
                                "sync_refresh_count": stats.SyncRefreshCount,
                                "sync_qpc": stats.SyncQPCTime,
                            })
                        },
                    );
                }
                frames.push(sample);
                if Instant::now() >= progress_at {
                    println!(
                        "MOTION_PROGRESS frames={frame} seconds={:.3}",
                        start.elapsed().as_secs_f64()
                    );
                    progress_at = Instant::now() + Duration::from_secs(5);
                }
                if let Some(front) = &mut front {
                    front.keep();
                }
            }
            let data = serde_json::json!({
                "scope":"render timestamp to independent decode; excludes remote display scanout",
                "display":display,"animation_hz":animation_hz,"animation":animation_text,
                "display_refresh_hz":source_mode.dmDisplayFrequency,"seconds":start.elapsed().as_secs_f64(),
                "window_height":window_height,"window_y":window_y,
                "cut_ms":cuts.as_ref().map(|c| c.every.as_millis() as u64),
                "fresh_rows":cuts.as_ref().map(|c| c.fresh_rows),
                "qpc_frequency":frequency,"presented_frames":frames.len(),"frames":frames
            });
            if let Some(path) = report {
                std::fs::write(path, serde_json::to_vec_pretty(&data)?)?;
            }
            println!(
                "MOTION frames={} seconds={:.3} display={}",
                frames.len(),
                start.elapsed().as_secs_f64(),
                display.display_name
            );
        }
        Ok(())
    }
}
