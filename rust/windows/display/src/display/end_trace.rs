//! Issue #129 diagnostics: what the displays and the user's windows did while the
//! user's layout went back at a stream's end. Apps open before an exclusive stream
//! came back oversized, and the log could not show which step moved their windows.
//! Armed only for the end of the last stream ([`Trace::start`]); every hook is a
//! no-op otherwise. Nothing here changes the layout or a window, and a read that
//! fails only leaves its part of the line out.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;
use windows::Win32::{
    Foundation::{CloseHandle, HWND, LPARAM, RECT},
    Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONULL, MONITORINFO,
        MONITORINFOEXW, MonitorFromWindow,
    },
    System::Threading::{
        GetCurrentProcessId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    },
    UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI},
    UI::WindowsAndMessaging::{
        EnumWindows, GetWindowRect, GetWindowTextLengthW, GetWindowThreadProcessId, IsIconic,
        IsWindowVisible,
    },
};
use windows::core::{BOOL, PWSTR};

/// At most this many windows get a line of their own.
pub(crate) const WINDOW_LINES: usize = 20;
/// EnumWindows stops after this many windows, as the other window scans do.
const WINDOW_LIMIT: usize = 4096;
/// `MONITORINFOF_PRIMARY`.
const PRIMARY: u32 = 1;

/// A display as Windows shows it to applications.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DisplayState {
    /// The GDI name, such as `\\.\DISPLAY1`, as the window lines name it.
    pub name: String,
    /// The monitor's name, empty when unknown.
    pub label: String,
    pub virtual_display: bool,
    pub primary: bool,
    pub rect: Rect,
    /// Effective DPI, None when unreadable.
    pub dpi: Option<u32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}
impl Rect {
    fn size(self) -> (i32, i32) {
        (self.right - self.left, self.bottom - self.top)
    }
}
impl From<RECT> for Rect {
    fn from(r: RECT) -> Self {
        Self {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
        }
    }
}
impl std::fmt::Display for Rect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (width, height) = self.size();
        write!(f, "{},{} {width}x{height}", self.left, self.top)
    }
}

/// A top-level window with a title. No title is kept: titles can be private.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WindowState {
    pub handle: u64,
    pub pid: u32,
    pub visible: bool,
    /// The GDI name of the display the window is on, empty when none.
    pub monitor: String,
    pub rect: Rect,
    pub dpi: u32,
}

/// A window present before and after that changed display, DPI or size.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Change {
    pub before: WindowState,
    pub after: WindowState,
}
impl Change {
    fn resized(&self) -> bool {
        self.before.rect.size() != self.after.rect.size()
    }
    fn rescaled(&self) -> bool {
        self.before.dpi != self.after.dpi
    }
}

/// `\\.\DISPLAY1 "DELL U2720Q" 0,0 2560x1440 120dpi(125%) primary; ...`
pub(crate) fn describe_displays(displays: &[DisplayState]) -> String {
    let mut line = String::new();
    for (i, d) in displays.iter().enumerate() {
        if i > 0 {
            line.push_str("; ");
        }
        let _ = write!(line, "{}", d.name);
        if !d.label.is_empty() {
            let _ = write!(line, " \"{}\"", d.label);
        }
        if d.virtual_display {
            line.push_str(" virtual");
        }
        let _ = write!(line, " {}", d.rect);
        match d.dpi {
            Some(dpi) => {
                let _ = write!(line, " {dpi}dpi({}%)", scale_percent(dpi));
            }
            None => line.push_str(" dpi unreadable"),
        }
        if d.primary {
            line.push_str(" primary");
        }
    }
    if line.is_empty() {
        line.push_str("none");
    }
    line
}

/// The Windows scale a DPI stands for: 96 is 100 %.
pub(crate) fn scale_percent(dpi: u32) -> u32 {
    (dpi * 100 + 48) / 96
}

/// The primary display's GDI name, `none` when no display is primary.
pub(crate) fn primary_of(displays: &[DisplayState]) -> &str {
    displays
        .iter()
        .find(|d| d.primary)
        .map_or("none", |d| d.name.as_str())
}

/// The windows present in both snapshots (the same handle of the same process; Windows
/// reuses handles) that changed display, DPI or size, and how many there are. Resized
/// windows come first, then rescaled ones, visible before hidden, each in z-order; at
/// most `cap` are returned.
pub(crate) fn changed_windows(
    before: &[WindowState],
    after: &[WindowState],
    cap: usize,
) -> (Vec<Change>, usize) {
    let earlier: HashMap<(u64, u32), &WindowState> =
        before.iter().map(|w| ((w.handle, w.pid), w)).collect();
    let mut changes: Vec<Change> = after
        .iter()
        .filter_map(|now| {
            let then = earlier.get(&(now.handle, now.pid))?;
            (then.monitor != now.monitor
                || then.dpi != now.dpi
                || then.rect.size() != now.rect.size())
            .then(|| Change {
                before: (*then).clone(),
                after: now.clone(),
            })
        })
        .collect();
    let total = changes.len();
    changes.sort_by_key(|c| (!c.resized(), !c.rescaled(), !c.after.visible));
    changes.truncate(cap);
    (changes, total)
}

/// `app.exe visible \\.\DISPLAY3 -> \\.\DISPLAY1, 0,0 1200x800 -> 0,0 1500x1000, dpi 120 -> 120`
pub(crate) fn describe_change(change: &Change, process: &str) -> String {
    let (b, a) = (&change.before, &change.after);
    let name = |m: &str| if m.is_empty() { "none" } else { m }.to_owned();
    format!(
        "{process} {} {} -> {}, {} -> {}, dpi {} -> {}",
        if a.visible { "visible" } else { "hidden" },
        name(&b.monitor),
        name(&a.monitor),
        b.rect,
        a.rect,
        b.dpi,
        a.dpi
    )
}

/// The file name of an executable path: a full path can name the user.
pub(crate) fn exe_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// One step's outcome for the summary: `entry ok, user's layout failed`.
pub(crate) fn describe_outcomes(outcomes: &[(&'static str, bool)]) -> String {
    if outcomes.is_empty() {
        return "none".into();
    }
    outcomes
        .iter()
        .map(|(step, ok)| format!("{step} {}", if *ok { "ok" } else { "failed" }))
        .collect::<Vec<_>>()
        .join(", ")
}

struct Armed {
    phase: &'static str,
    windows_placed: u32,
}
thread_local! {
    // The end's display work runs on the thread that drops the last lease.
    static ARMED: RefCell<Option<Armed>> = const { RefCell::new(None) };
    // The windows before the end, for the settled look once the lease's lock is gone.
    static SETTLE: RefCell<Option<Vec<WindowState>>> = const { RefCell::new(None) };
}
/// How long after the end the settled look waits: apps answer WM_DPICHANGED and the
/// virtual display goes in that time.
const SETTLE_DELAY: std::time::Duration = std::time::Duration::from_secs(3);

fn arm() {
    ARMED.with_borrow_mut(|armed| {
        *armed = Some(Armed {
            phase: "before",
            windows_placed: 0,
        })
    });
}
fn disarm() -> Option<Armed> {
    ARMED.with_borrow_mut(Option::take)
}

/// The trace of one stream's end. Dropping it logs the displays and the windows that
/// changed, and leaves the windows before for [`settle_later`].
pub(crate) struct Trace {
    windows: Vec<WindowState>,
    outcomes: Vec<(&'static str, bool)>,
}
impl Trace {
    /// Log the displays, take the windows as they are and arm the hooks on this thread.
    /// Armed last, so a panic on the way leaves nothing armed.
    pub(crate) fn start() -> Self {
        log_displays("before", false);
        let trace = Self {
            windows: crate::desktop::on_input_desktop(windows),
            outcomes: Vec::new(),
        };
        arm();
        trace
    }
    /// Whether a step went through, for the summary: a failed one is tried again later,
    /// so what the windows did is then not the end of it.
    pub(crate) fn outcome(&mut self, step: &'static str, ok: bool) {
        self.outcomes.push((step, ok));
    }
}
impl Drop for Trace {
    fn drop(&mut self) {
        let windows_placed = disarm().map_or(0, |a| a.windows_placed);
        // The last commits (scale, HDR) often have no step line after them.
        log_displays("after", false);
        let after = crate::desktop::on_input_desktop(windows);
        log_window_changes(
            "immediate",
            &self.windows,
            &after,
            windows_placed,
            &describe_outcomes(&self.outcomes),
        );
        let before = std::mem::take(&mut self.windows);
        SETTLE.with_borrow_mut(|settle| *settle = Some(before));
    }
}

/// The windows before a traced end, for its settled look ([`settle_later`]).
pub(crate) struct Baseline(Vec<WindowState>);

/// The windows before the [`Trace`] that last ended on this thread, taken once.
pub(crate) fn take_baseline() -> Option<Baseline> {
    SETTLE.with_borrow_mut(Option::take).map(Baseline)
}

/// Look at the windows again [`SETTLE_DELAY`] after a traced end, from a thread of its
/// own that nothing waits for. Call once the stream's display is gone or going: with
/// the lease's lock released, and after the watch that keeps the display. Nothing
/// without a `baseline`, that is without a traced end.
pub(crate) fn settle_later(baseline: Option<Baseline>) {
    let Some(Baseline(before)) = baseline else {
        return;
    };
    let spawned = std::thread::Builder::new()
        .name("layout-end-trace".into())
        .spawn(move || {
            std::thread::sleep(SETTLE_DELAY);
            let streams = crate::display::leased_displays();
            if !settled_look_due(&streams) {
                tracing::debug!(
                    streams = streams.len(),
                    "a stream started after the end; skipping the settled look at the windows"
                );
                return;
            }
            log_displays("settled", false);
            let after = crate::desktop::on_input_desktop(windows);
            log_window_changes("settled", &before, &after, 0, "as the immediate look");
        });
    if let Err(error) = spawned {
        tracing::debug!(%error, "the settled look at the windows after a stream's end did not start");
    }
}

/// Whether the settled look says something of the end: a stream that started since
/// moves windows of its own. `streams` are the displays streams hold now.
fn settled_look_due(streams: &[String]) -> bool {
    streams.is_empty()
}

fn log_window_changes(
    look: &str,
    before: &[WindowState],
    after: &[WindowState],
    windows_placed: u32,
    steps: &str,
) {
    let (changes, total) = changed_windows(before, after, WINDOW_LINES);
    tracing::info!(
        look,
        changed = total,
        listed = changes.len(),
        windows_before = before.len(),
        windows_after = after.len(),
        windows_placed,
        steps,
        "windows that changed display, DPI or size while the user's layout went back"
    );
    for change in &changes {
        let process = process_name(change.after.pid);
        tracing::info!(look, window = %describe_change(change, &process), "a window changed across the layout's return");
    }
}

/// Name the step the next commits belong to. Whether a trace is armed on this thread.
pub(crate) fn phase(name: &'static str) -> bool {
    ARMED.with_borrow_mut(|armed| armed.as_mut().map(|armed| armed.phase = name).is_some())
}

/// A layout commit went through: log the displays if armed. `windows_placed` is the
/// fallback that left every display's position and the primary to Windows. Whether
/// a trace is armed on this thread.
pub(crate) fn committed(what: &str, windows_placed: bool) -> bool {
    let Some(phase) = note_commit(windows_placed) else {
        return false;
    };
    log_displays(&format!("{phase}: {what}"), windows_placed);
    true
}

/// The armed trace's phase, counting a Windows-placed layout; None when not armed.
fn note_commit(windows_placed: bool) -> Option<&'static str> {
    ARMED.with_borrow_mut(|armed| {
        armed.as_mut().map(|armed| {
            armed.windows_placed += u32::from(windows_placed);
            armed.phase
        })
    })
}

fn log_displays(step: &str, windows_placed: bool) {
    let displays = displays();
    tracing::info!(
        step,
        windows_placed,
        primary = %primary_of(&displays),
        displays = %describe_displays(&displays),
        "displays after a step of the stream's end"
    );
}

/// The displays Windows shows now, with the monitor names the display configuration has.
fn displays() -> Vec<DisplayState> {
    unsafe extern "system" fn visit(
        monitor: HMONITOR,
        _dc: HDC,
        _rect: *mut RECT,
        context: LPARAM,
    ) -> BOOL {
        // SAFETY: EnumDisplayMonitors passes the `&mut Vec<HMONITOR>` from displays as
        // `context`, which outlives the enumeration.
        unsafe { (*(context.0 as *mut Vec<HMONITOR>)).push(monitor) };
        true.into()
    }
    let mut handles: Vec<HMONITOR> = Vec::new();
    // SAFETY: `handles` outlives the synchronous enumeration that fills it through `visit`.
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(visit),
            LPARAM((&mut handles as *mut Vec<HMONITOR>) as isize),
        );
    }
    let known = super::monitors().unwrap_or_default();
    handles
        .into_iter()
        .filter_map(|handle| {
            let (name, rect, flags) = monitor_info(handle)?;
            let monitor = known
                .iter()
                .find(|m| m.display_name.eq_ignore_ascii_case(&name));
            let (mut x, mut y) = (0, 0);
            // SAFETY: both out-pointers are live locals; a stale handle only fails the call.
            let dpi = unsafe { GetDpiForMonitor(handle, MDT_EFFECTIVE_DPI, &mut x, &mut y) }
                .ok()
                .map(|()| x);
            Some(DisplayState {
                label: monitor.map(|m| m.friendly_name.clone()).unwrap_or_default(),
                virtual_display: monitor.is_some_and(|m| m.is_rubylight()),
                name,
                primary: flags & PRIMARY != 0,
                rect,
                dpi,
            })
        })
        .collect()
}

fn monitor_info(handle: HMONITOR) -> Option<(String, Rect, u32)> {
    if handle.is_invalid() {
        return None;
    }
    let mut info = MONITORINFOEXW {
        monitorInfo: MONITORINFO {
            cbSize: size_of::<MONITORINFOEXW>() as u32,
            ..Default::default()
        },
        ..Default::default()
    };
    // SAFETY: `info` is a live local whose size is set to the extended struct's; a stale
    // handle only fails the call.
    let found = unsafe { GetMonitorInfoW(handle, (&mut info as *mut MONITORINFOEXW).cast()) };
    found.as_bool().then(|| {
        (
            crate::text::from_wide(&info.szDevice),
            info.monitorInfo.rcMonitor.into(),
            info.monitorInfo.dwFlags,
        )
    })
}

/// The titled top-level windows of other processes, hidden ones included (the issue's
/// apps sat in the tray), leaving out minimised and empty ones. Only calls that send no
/// message to the window, so a hung app cannot hold the stream's end.
fn windows() -> Vec<WindowState> {
    struct Scan {
        own: u32,
        seen: usize,
        found: Vec<WindowState>,
    }
    unsafe extern "system" fn visit(window: HWND, context: LPARAM) -> BOOL {
        // SAFETY: EnumWindows passes the `&mut Scan` from windows as `context`, which outlives
        // the enumeration; every out-pointer is a live local.
        unsafe {
            let scan = &mut *(context.0 as *mut Scan);
            scan.seen += 1;
            if scan.seen > WINDOW_LIMIT {
                return false.into();
            }
            let mut pid = 0;
            GetWindowThreadProcessId(window, Some(&mut pid));
            // GetWindowTextLength sends a message only to the caller's own windows.
            if pid == 0 || pid == scan.own || GetWindowTextLengthW(window) == 0 {
                return true.into();
            }
            if IsIconic(window).as_bool() {
                return true.into();
            }
            let mut rect = RECT::default();
            if GetWindowRect(window, &mut rect).is_err() {
                return true.into();
            }
            let rect = Rect::from(rect);
            if rect.size().0 <= 0 || rect.size().1 <= 0 {
                return true.into();
            }
            let monitor = MonitorFromWindow(window, MONITOR_DEFAULTTONULL);
            scan.found.push(WindowState {
                handle: window.0 as usize as u64,
                pid,
                visible: IsWindowVisible(window).as_bool(),
                monitor: monitor_info(monitor)
                    .map(|(name, _, _)| name)
                    .unwrap_or_default(),
                rect,
                dpi: GetDpiForWindow(window),
            });
            true.into()
        }
    }
    let mut scan = Scan {
        // SAFETY: GetCurrentProcessId has no preconditions.
        own: unsafe { GetCurrentProcessId() },
        seen: 0,
        found: Vec::new(),
    };
    // SAFETY: `scan` outlives the synchronous EnumWindows call that reads it through `visit`.
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM((&mut scan as *mut Scan) as isize));
    }
    scan.found
}

/// The executable's file name, `?` when the process is gone or out of reach.
fn process_name(pid: u32) -> String {
    // SAFETY: the process handle is closed once after the query, and `count` is the capacity
    // of `name` in UTF-16 units, updated to the length written.
    let path = unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return "?".into();
        };
        let mut name = [0u16; 1024];
        let mut count = name.len() as u32;
        let queried = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(name.as_mut_ptr()),
            &mut count,
        );
        let _ = CloseHandle(process);
        if queried.is_err() {
            return "?".into();
        }
        String::from_utf16_lossy(&name[..count as usize])
    };
    exe_name(&path).to_owned()
}

#[cfg(test)]
mod tests;
