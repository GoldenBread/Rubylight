//! The notification-area icon and its notifications on Linux. There is no tray icon yet:
//! creating one fails, so the host runs without it, no notification is shown and none waits.
//! Opening the console in the user's browser works, through `xdg-open`.
use anyhow::{Context, Result, bail};
use butterpollo_session::tray::Icon;
use std::{path::PathBuf, sync::mpsc, time::Instant};

/// What the user chose in the tray icon's menu.
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

/// Show the running app (None when none runs) in the tray: the icon's state, the tooltip
/// and the menu's item that quits it. Does nothing on Linux, which has no tray icon.
pub fn show_app(icon: Icon, app: Option<&str>, summary: Option<&str>) {
    let _ = (icon, app, summary);
}

/// The tray icon, removed when dropped. None exists on Linux yet: [`Tray::new_options`]
/// always fails.
pub struct Tray {
    _shown: (),
}

impl Tray {
    pub fn new(icon: PathBuf, port: u16) -> Result<(Self, mpsc::Receiver<Action>)> {
        Self::new_options(icon, port, false)
    }
    /// Show the tray icon and return the receiver of what the user chooses in its menu.
    /// Fails on Linux, where the host has no tray icon yet.
    pub fn new_options(
        icon: PathBuf,
        port: u16,
        hide_controls: bool,
    ) -> Result<(Self, mpsc::Receiver<Action>)> {
        let _ = (icon, port, hide_controls);
        Err(crate::unsupported("The tray icon"))
    }
}

/// Why no notification goes up: they come from the tray icon, which Linux never shows.
fn not_shown() -> anyhow::Error {
    anyhow::anyhow!("the tray icon is not shown")
}

/// Show a notification from the tray icon, if there is one. There is none on Linux, so
/// nothing is shown.
pub fn notify(title: &str, text: &str) {
    let _ = (title, text);
}

/// Show pairing request `request`'s notification. There is no tray icon on Linux, so
/// nothing is shown.
pub fn notify_pairing(request: &str, title: &str, text: &str) {
    let _ = (request, title, text);
    tracing::debug!(error = %not_shown(), "no tray notification for a pairing request");
}

/// Pairing request `request` ended, so a notification still waiting about it goes. None ever
/// waits on Linux.
pub fn pairing_request_ended(request: &str) {
    let _ = request;
}

/// Called from the host's loop: lets notifications that waited go up. None ever waits on
/// Linux.
pub fn release_overdue_notices() {}

/// When [`release_overdue_notices`] next has something to release: never on Linux.
pub fn notices_due() -> Option<Instant> {
    None
}

/// Show a notification from the tray icon, or say why it could not be shown. Always the
/// latter on Linux: there is no tray icon.
pub fn try_notify(title: &str, text: &str) -> Result<()> {
    let _ = (title, text);
    Err(not_shown())
}

/// Tell the user that `app` exited too fast with `code`. There is no tray icon on Linux, so
/// nothing is shown and the log says so.
pub fn launch_error(app: &str, code: u32) {
    let _ = (app, code);
    tracing::warn!(error = %not_shown(), "no tray notification for a launch error");
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

/// Open a page of the console in the user's browser, by handing its address to `xdg-open`.
/// `page` is one of the `PAGE_` paths or `/`. Succeeds once `xdg-open` started; whether a
/// browser came up is its business, and its failure is logged.
pub fn open_web_page(port: u16, page: &str) -> Result<()> {
    if !matches!(page, "/" | PAGE_ENTER_PIN | PAGE_PAIR_QR | PAGE_BACKUP) {
        bail!("not a console page: {page}");
    }
    let url = console_url(port, page);
    let mut opener = std::process::Command::new("xdg-open")
        .arg(&url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("xdg-open could not be started to open the web interface")?;
    // Some desktops keep xdg-open running as long as the browser it started, so it is
    // waited for off the caller's thread.
    let waiting = std::thread::Builder::new()
        .name("open-web".into())
        .spawn(move || match opener.wait() {
            Ok(status) if !status.success() => {
                tracing::warn!(%status, url, "xdg-open could not open the web interface");
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "xdg-open could not be waited for"),
        });
    if let Err(error) = waiting {
        tracing::debug!(%error, "no thread to wait for xdg-open");
    }
    Ok(())
}
