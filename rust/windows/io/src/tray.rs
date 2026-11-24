//! Native notification-area UI, owned by its Windows message thread.
use anyhow::{Context, Result, bail};
use butterpollo_session::tray::{Icon, NoticeAction};
use std::{
    mem::size_of,
    path::PathBuf,
    sync::{Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
        UI::{Shell::*, WindowsAndMessaging::*},
    },
    core::{PCWSTR, w},
};

mod icon;
mod notices;

use icon::badged;
#[cfg(test)]
use notices::launch_error_notice;
use notices::{NOTICES, SHOWN, show_held};
pub use notices::{
    launch_error, notices_due, notify, notify_pairing, pairing_request_ended,
    release_overdue_notices, try_notify,
};

/// Retries adding the icon while the taskbar does not accept it yet.
const RETRY_TIMER: usize = 1;
/// The running app changed; see [`show_app`].
const APP_CHANGED: u32 = WM_APP + 2;
#[derive(Clone, Copy)]
pub enum Action {
    Open,
    /// Open the console where a waiting device's PIN goes.
    EnterPin,
    /// Open the console where the QR code that pairs a device is.
    PairDevice,
    StopSessions,
    /// Quit the running app, as the console's close button does.
    QuitApp,
    /// Look for a newer release and say what was found.
    CheckUpdates,
    /// Save the settings, without secrets, to the user's Documents folder.
    BackupSettings,
    /// Open the console where a settings backup is restored.
    RestoreSettings,
    Restart,
    Quit,
}
struct State {
    sender: mpsc::Sender<Action>,
    icon: NOTIFYICONDATAW,
    /// Idle, streaming and paused.
    icons: [HICON; 3],
    port: u16,
    taskbar: u32,
    hide_controls: bool,
    added: AddLog,
}
/// How long the taskbar may refuse the icon before the log says so again.
const ADD_REFUSED_LONG: Duration = Duration::from_secs(60);
/// What the log says of the attempts to add the icon: Windows refusing it, once, and again once
/// it keeps refusing, and the icon going in.
#[derive(Default)]
struct AddLog {
    refused_since: Option<Instant>,
    warned_long: bool,
    warned_with_taskbar: bool,
    added_once: bool,
}
#[derive(Debug, PartialEq, Eq)]
enum AddReport {
    Refused,
    StillRefused,
    /// Added, the first time or after Windows refused it for this long.
    Added(Option<Duration>),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AddOutcome {
    Added,
    Retry,
    GiveUp,
}
impl AddLog {
    fn is_added(&self) -> bool {
        self.added_once && self.refused_since.is_none()
    }
    fn note_still_refused(&mut self, taskbar: bool) {
        if taskbar {
            self.warned_with_taskbar = true;
        }
    }
    /// If retries gave up before any taskbar existed, allow a fresh retry window
    /// once Explorer broadcasts `TaskbarCreated`.
    fn on_taskbar_created(&mut self) {
        if self.warned_long && !self.warned_with_taskbar {
            self.refused_since = None;
            self.warned_long = false;
        }
    }
    fn attempt(&mut self, added: bool, now: Instant) -> Option<AddReport> {
        if added {
            let refused = self.refused_since.take().map(|since| now - since);
            self.warned_long = false;
            self.warned_with_taskbar = false;
            let first = !std::mem::replace(&mut self.added_once, true);
            return (first || refused.is_some()).then_some(AddReport::Added(refused));
        }
        match self.refused_since {
            None => {
                self.refused_since = Some(now);
                Some(AddReport::Refused)
            }
            Some(since) if !self.warned_long && now - since >= ADD_REFUSED_LONG => {
                self.warned_long = true;
                Some(AddReport::StillRefused)
            }
            Some(_) => None,
        }
    }
}
/// Add the icon, and log what [`AddLog`] says of the attempt.
unsafe fn add_icon(state: &mut State, on_taskbar: bool) -> AddOutcome {
    // Explorer can broadcast `TaskbarCreated` just after our 3 s timer already
    // added the icon; `NIM_ADD` on an already-registered icon fails with
    // `0x80004005`, so check `NIM_MODIFY` first when we believe it is present.
    if on_taskbar && state.added.is_added() {
        // SAFETY: the caller passes the live State of the tray window, whose icon data is complete.
        if unsafe { Shell_NotifyIconW(NIM_MODIFY, &state.icon).as_bool() } {
            return AddOutcome::Added;
        }
    }
    // SAFETY: the caller passes the live State of the tray window, whose icon data is complete.
    let (added, error) = unsafe {
        let added = Shell_NotifyIconW(NIM_ADD, &state.icon).as_bool();
        (added, (!added).then(|| GetLastError()))
    };
    let report = state.added.attempt(added, Instant::now());
    let session = crate::process::current_session().ok();
    // Explorer's taskbar on this desktop: without it nothing can show an icon.
    // SAFETY: both strings are static and NUL-terminated.
    let taskbar = unsafe { FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()) }.is_ok();
    match report {
        // Initial refusals (e.g. 0x80004005 before the taskbar is ready) are retried
        // every 3 s, so keep them at DEBUG until retries give up.
        Some(AddReport::Refused) if !taskbar => tracing::debug!(
            error = ?error,
            session_id = session,
            "no taskbar yet for the tray icon; adding it when the taskbar starts"
        ),
        Some(AddReport::Refused) => tracing::debug!(
            error = ?error,
            session_id = session,
            taskbar,
            "Windows refused the tray icon; adding it again every 3 s and when the taskbar starts"
        ),
        Some(AddReport::StillRefused) if !taskbar => {
            state.added.note_still_refused(false);
            tracing::info!(
                error = ?error,
                session_id = session,
                "still no taskbar for the tray icon after a minute; stopping 3 s retries until the taskbar starts"
            );
        }
        Some(AddReport::StillRefused) => {
            state.added.note_still_refused(true);
            tracing::warn!(
                error = ?error,
                session_id = session,
                taskbar,
                "Windows still refuses the tray icon after a minute although the taskbar is there; stopping 3 s retries until the taskbar restarts"
            );
        }
        Some(AddReport::Added(refused)) => tracing::info!(
            session_id = session,
            refused_s = refused.map(|refused| refused.as_secs()),
            "tray icon added"
        ),
        None => {}
    }
    if added {
        AddOutcome::Added
    } else if state.added.warned_long {
        AddOutcome::GiveUp
    } else {
        AddOutcome::Retry
    }
}
impl State {
    /// Show the running app's state in the icon and its tooltip.
    fn show(&mut self) {
        let app = APP.lock().unwrap();
        self.icon.hIcon = self.icons[match app.icon {
            Icon::Idle => 0,
            Icon::Streaming => 1,
            Icon::Paused => 2,
        }];
        let tip = match &app.summary {
            Some(summary) => format!("Rubylight — {summary}"),
            None => format!("Rubylight — web port {}", self.port),
        };
        let tip: Vec<u16> = tip.encode_utf16().take(127).collect();
        self.icon.szTip = [0; 128];
        self.icon.szTip[..tip.len()].copy_from_slice(&tip);
    }
}
/// The running app as the tray shows it.
struct Shown {
    icon: Icon,
    app: Option<String>,
    summary: Option<String>,
}
static APP: Mutex<Shown> = Mutex::new(Shown {
    icon: Icon::Idle,
    app: None,
    summary: None,
});
/// Show the running app (None when none runs) in the tray: the icon's
/// state, the tooltip and the menu's item that quits it.
pub fn show_app(icon: Icon, app: Option<&str>, summary: Option<&str>) {
    let mut shown = APP.lock().unwrap();
    let changed = shown.app.as_deref() != app;
    *shown = Shown {
        icon,
        app: app.map(str::to_owned),
        summary: summary.map(str::to_owned),
    };
    drop(shown);
    if changed {
        let held = NOTICES.lock().unwrap().app_changed();
        show_held(held);
    }
    if let Some(window) = *SHOWN.lock().unwrap() {
        // SAFETY: The tray publishes its HWND under SHOWN; this asynchronous message
        // contains only integer values and borrows no Rust data after the call returns.
        unsafe {
            let _ = PostMessageW(
                Some(HWND(window as *mut _)),
                APP_CHANGED,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}
unsafe extern "system" fn window(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: Windows supplies CREATESTRUCTW for WM_NCCREATE. The tray thread
    // keeps its boxed State alive until message dispatch ends and clears
    // GWLP_USERDATA before destroying the window. State borrows stay within
    // handlers and do not cross the reentrant popup-menu calls.
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(lparam.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
        if !state.is_null() {
            let taskbar = (*state).taskbar;
            // Explorer announces a new taskbar (at sign-in, or after it
            // restarts); until the icon is in, retry every few seconds.
            if message == taskbar || (message == WM_TIMER && wparam.0 == RETRY_TIMER) {
                let s = &mut *state;
                s.show();
                if message == taskbar {
                    s.added.on_taskbar_created();
                }
                match add_icon(s, message == taskbar) {
                    AddOutcome::Added | AddOutcome::GiveUp => {
                        let _ = KillTimer(Some(hwnd), RETRY_TIMER);
                    }
                    AddOutcome::Retry if message == taskbar => {
                        SetTimer(Some(hwnd), RETRY_TIMER, 3000, None);
                    }
                    AddOutcome::Retry => {}
                }
                return LRESULT(0);
            }
            if message == APP_CHANGED {
                let s = &mut *state;
                s.show();
                let _ = Shell_NotifyIconW(NIM_MODIFY, &s.icon);
                return LRESULT(0);
            }
            if message == WM_APP + 1 {
                match (lparam.0 & 0xffff) as u32 {
                    NIN_BALLOONUSERCLICK => {
                        let (action, held) = NOTICES.lock().unwrap().clicked(Instant::now());
                        match action {
                            Some(NoticeAction::EnterPin) => {
                                let _ = (*state).sender.send(Action::EnterPin);
                            }
                            Some(NoticeAction::QuitApp) => {
                                let _ = (*state).sender.send(Action::QuitApp);
                            }
                            None => {}
                        }
                        show_held(held);
                    }
                    NIN_BALLOONTIMEOUT => {
                        let held = NOTICES.lock().unwrap().timed_out(Instant::now());
                        show_held(held);
                    }
                    WM_LBUTTONDBLCLK => {
                        let _ = (*state).sender.send(Action::Open);
                    }
                    WM_RBUTTONUP | WM_CONTEXTMENU => {
                        let hide_controls = (*state).hide_controls;
                        if let Ok(menu) = CreatePopupMenu() {
                            let _ = AppendMenuW(menu, MF_STRING, 1, w!("Open Rubylight"));
                            let _ = AppendMenuW(menu, MF_STRING, 7, w!("Pair a device"));
                            let _ = AppendMenuW(menu, MF_STRING, 6, w!("Check for updates"));
                            if !hide_controls {
                                let _ = AppendMenuW(menu, MF_STRING, 2, w!("Disconnect clients"));
                                let app = APP.lock().unwrap().app.clone();
                                let label: Vec<u16> = match &app {
                                    // An ampersand would underline the next letter.
                                    Some(app) => format!("Quit {}", app.replace('&', "&&")),
                                    None => "No app running".into(),
                                }
                                .encode_utf16()
                                .chain(Some(0))
                                .collect();
                                let flags = if app.is_some() {
                                    MF_STRING
                                } else {
                                    MF_STRING | MF_GRAYED
                                };
                                let _ = AppendMenuW(menu, flags, 5, PCWSTR(label.as_ptr()));
                                let _ = AppendMenuW(menu, MF_STRING, 8, w!("Back up settings"));
                                let _ = AppendMenuW(menu, MF_STRING, 9, w!("Restore settings..."));
                                let _ = AppendMenuW(menu, MF_STRING, 3, w!("Restart"));
                                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
                                let _ = AppendMenuW(menu, MF_STRING, 4, w!("Quit"));
                            }
                            let mut point = POINT::default();
                            let _ = GetCursorPos(&mut point);
                            // These calls can re-enter `window`; keep only copied
                            // values and the raw state pointer across them.
                            let _ = SetForegroundWindow(hwnd);
                            let id = TrackPopupMenu(
                                menu,
                                TPM_RETURNCMD | TPM_NONOTIFY,
                                point.x,
                                point.y,
                                None,
                                hwnd,
                                None,
                            )
                            .0;
                            let action = match id {
                                1 => Some(Action::Open),
                                2 => Some(Action::StopSessions),
                                3 => Some(Action::Restart),
                                4 => Some(Action::Quit),
                                5 => Some(Action::QuitApp),
                                6 => Some(Action::CheckUpdates),
                                7 => Some(Action::PairDevice),
                                8 => Some(Action::BackupSettings),
                                9 => Some(Action::RestoreSettings),
                                _ => None,
                            };
                            if let Some(action) = action {
                                let s = &*state;
                                let _ = s.sender.send(action);
                            }
                            let _ = DestroyMenu(menu);
                            let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
                        }
                    }
                    _ => {}
                }
                return LRESULT(0);
            }
        }
        DefWindowProcW(hwnd, message, wparam, lparam)
    }
}
pub struct Tray {
    id: u32,
    worker: Option<thread::JoinHandle<()>>,
}
impl Tray {
    pub fn new(icon: PathBuf, port: u16) -> Result<(Self, mpsc::Receiver<Action>)> {
        Self::new_options(icon, port, false)
    }
    pub fn new_options(
        icon: PathBuf,
        port: u16,
        hide_controls: bool,
    ) -> Result<(Self, mpsc::Receiver<Action>)> {
        let (sender, receiver) = mpsc::channel();
        let (ready, started) = mpsc::sync_channel(1);
        let worker = thread::Builder::new().name("tray".into()).spawn(move || {
            let result = (|| -> Result<()> {
                // SAFETY: This thread creates, dispatches and destroys the window;
                // sized structures and terminated strings remain live through calls.
                // The boxed State outlives GWLP_USERDATA, owned icons are destroyed
                // once after the window, and the shared fallback icon is never freed.
                unsafe {
                    let module = GetModuleHandleW(None)?;
                    let class = WNDCLASSW {
                        lpfnWndProc: Some(window),
                        hInstance: HINSTANCE(module.0),
                        lpszClassName: w!("ButterpolloRustTray"),
                        ..Default::default()
                    };
                    if RegisterClassW(&class) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
                        bail!("cannot register tray window");
                    }
                    // The executable's own icon (resource 1, see host/build.rs),
                    // else the icon file, else the generic application icon.
                    let (width, height) =
                        (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON));
                    let path: Vec<u16> = icon
                        .to_string_lossy()
                        .encode_utf16()
                        .chain(Some(0))
                        .collect();
                    let custom = LoadImageW(
                        Some(HINSTANCE(module.0)),
                        PCWSTR(1 as _),
                        IMAGE_ICON,
                        width,
                        height,
                        LR_DEFAULTCOLOR,
                    )
                    .or_else(|_| {
                        LoadImageW(
                            None,
                            PCWSTR(path.as_ptr()),
                            IMAGE_ICON,
                            width,
                            height,
                            LR_LOADFROMFILE,
                        )
                    })
                    .ok();
                    let icon = if let Some(handle) = custom {
                        HICON(handle.0)
                    } else {
                        LoadIconW(None, IDI_APPLICATION)?
                    };
                    let icons = [
                        icon,
                        badged(icon, Icon::Streaming).unwrap_or(icon),
                        badged(icon, Icon::Paused).unwrap_or(icon),
                    ];
                    let mut state = Box::new(State {
                        sender,
                        icons,
                        port,
                        icon: NOTIFYICONDATAW {
                            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
                            uID: 1,
                            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
                            uCallbackMessage: WM_APP + 1,
                            hIcon: icon,
                            ..Default::default()
                        },
                        taskbar: RegisterWindowMessageW(w!("TaskbarCreated")),
                        hide_controls,
                        added: AddLog::default(),
                    });
                    state.show();
                    let hwnd = CreateWindowExW(
                        WINDOW_EX_STYLE(0),
                        w!("ButterpolloRustTray"),
                        w!("Butterpollo Rust"),
                        WINDOW_STYLE(0),
                        0,
                        0,
                        0,
                        0,
                        None,
                        None,
                        Some(HINSTANCE(module.0)),
                        Some((&mut *state as *mut State).cast()),
                    )?;
                    state.icon.hWnd = hwnd;
                    // The service starts the host elevated, and Windows keeps
                    // Explorer's messages from an elevated window unless they
                    // are allowed: the TaskbarCreated broadcast after sign-in
                    // never arrived, so an icon that the not-yet-ready taskbar
                    // refused at boot never appeared.
                    for message in [state.taskbar, WM_APP + 1] {
                        let _ = ChangeWindowMessageFilterEx(hwnd, message, MSGFLT_ALLOW, None);
                    }
                    if add_icon(&mut state, false) == AddOutcome::Retry {
                        SetTimer(Some(hwnd), RETRY_TIMER, 3000, None);
                    }
                    *SHOWN.lock().unwrap() = Some(hwnd.0 as isize);
                    let _ = ready.send(Ok(GetCurrentThreadId()));
                    let mut message = MSG::default();
                    loop {
                        let status = GetMessageW(&mut message, None, 0, 0).0;
                        if status <= 0 {
                            break;
                        }
                        let _ = TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                    *SHOWN.lock().unwrap() = None;
                    let _ = Shell_NotifyIconW(NIM_DELETE, &state.icon);
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    let _ = DestroyWindow(hwnd);
                    for badge in &icons[1..] {
                        if *badge != icon {
                            let _ = DestroyIcon(*badge);
                        }
                    }
                    if custom.is_some() {
                        let _ = DestroyIcon(icon);
                    }
                    Ok(())
                }
            })();
            if let Err(e) = result {
                let _ = ready.send(Err(e.to_string()));
            }
        })?;
        let id = started
            .recv_timeout(std::time::Duration::from_secs(10))
            .context("tray startup timed out")?
            .map_err(|e| anyhow::anyhow!(e))?;
        Ok((
            Self {
                id,
                worker: Some(worker),
            },
            receiver,
        ))
    }
}
impl Drop for Tray {
    fn drop(&mut self) {
        // SAFETY: id identifies the worker's message queue; WM_QUIT carries no borrowed
        // pointers, and the worker is joined before its owner finishes dropping.
        unsafe {
            let _ = PostThreadMessageW(self.id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// The console's Devices page with a waiting device's PIN field focused.
pub const PAGE_ENTER_PIN: &str = "/devices?pair=pin";
/// The console's Devices page at the QR code that pairs a device.
pub const PAGE_PAIR_QR: &str = "/devices?pair=qr";
/// The console's Troubleshoot page at the settings backup.
pub const PAGE_BACKUP: &str = "/troubleshoot#backup";

fn console_url(port: u16, page: &str) -> String {
    format!("https://localhost:{port}{page}")
}

pub fn open_web(port: u16) -> Result<()> {
    open_web_page(port, "/")
}

/// Open a page of the console in the signed-in user's browser. `page` is one
/// of the `PAGE_` paths or `/`.
pub fn open_web_page(port: u16, page: &str) -> Result<()> {
    if !matches!(page, "/" | PAGE_ENTER_PIN | PAGE_PAIR_QR | PAGE_BACKUP) {
        bail!("not a console page: {page}");
    }
    if crate::process::is_system() {
        return crate::process::Process::spawn_detached(
            &std::env::current_exe()?,
            &[
                "--open-web".into(),
                port.to_string().into(),
                "--open-web-page".into(),
                page.into(),
            ],
            crate::process::Target::User { elevated: false },
        );
    }
    let url: Vec<u16> = format!("{}\0", console_url(port, page))
        .encode_utf16()
        .collect();
    // SAFETY: The verb and URL are terminated UTF-16 strings that remain live for
    // ShellExecuteW; the unused optional pointer arguments are null.
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(url.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    if result.0 as usize <= 32 {
        bail!("Windows could not open the web interface");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
