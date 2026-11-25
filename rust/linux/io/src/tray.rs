//! The tray icon and its notifications on Linux, over the session bus: a
//! StatusNotifierItem (`org.kde.StatusNotifierItem`, which Plasma's system tray and the
//! AppIndicator extensions show) with its menu (`com.canonical.dbusmenu`), and desktop
//! notifications (`org.freedesktop.Notifications`). The icon, its tooltip and its menu are the
//! Windows tray's (`rust/windows/io/src/tray.rs`). A tray that starts after the host gets the
//! icon when its watcher appears, as the Windows icon goes in when Explorer's taskbar starts.
//! Opening the console in the user's browser goes through `xdg-open`.
use anyhow::{Context, Result, bail};
use butterpollo_session::tray::{Icon, QueuedNotice};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
    thread,
};
use zbus::blocking::Connection;

mod item;
mod menu;
mod notices;
mod pixmap;

pub use notices::{
    launch_error, notices_due, notify, notify_pairing, pairing_request_ended,
    release_overdue_notices, try_notify,
};

/// What the user chose in the tray icon's menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

/// The bus name of the tray that shows StatusNotifierItems (Plasma's system tray).
const WATCHER: &str = "org.kde.StatusNotifierWatcher";
/// Where the icon is on the host's connection.
const ITEM_PATH: &str = "/StatusNotifierItem";
const ITEM_INTERFACE: &str = "org.kde.StatusNotifierItem";
/// Where its menu is.
const MENU_PATH: &str = "/MenuBar";
const MENU_INTERFACE: &str = "com.canonical.dbusmenu";

/// The running app as the tray shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
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
/// The revision of the menu's layout, which changes with the running app.
static REVISION: AtomicU32 = AtomicU32::new(1);

/// What the tray's own thread does, so that no caller waits on the bus.
enum Command {
    /// Hand the icon to the tray's watcher, which just appeared or was there at the start.
    Register,
    /// The running app changed: the icon, the tooltip and the menu say so.
    AppChanged,
    Notify(QueuedNotice),
    /// Take down notification `id`.
    Close(u32),
    Stop,
}
/// The tray's thread while the icon is shown.
static COMMANDS: Mutex<Option<mpsc::Sender<Command>>> = Mutex::new(None);

/// Why no notification goes up: they come with the tray icon, as on Windows.
fn not_shown() -> anyhow::Error {
    anyhow::anyhow!("the tray icon is not shown")
}

/// Hand `command` to the tray's thread, or fail when no icon is shown.
fn command(command: Command) -> Result<()> {
    let commands = COMMANDS.lock().unwrap();
    let Some(commands) = commands.as_ref() else {
        return Err(not_shown());
    };
    commands.send(command).map_err(|_| not_shown())
}

/// Show the running app (None when none runs) in the tray: the icon's state, the tooltip
/// and the menu's item that quits it.
pub fn show_app(icon: Icon, app: Option<&str>, summary: Option<&str>) {
    let shown = Shown {
        icon,
        app: app.map(str::to_owned),
        summary: summary.map(str::to_owned),
    };
    let mut current = APP.lock().unwrap();
    if *current == shown {
        return;
    }
    let app_changed = current.app != shown.app;
    *current = shown;
    drop(current);
    if app_changed {
        notices::app_changed();
    }
    // No icon is shown: the next one starts from APP.
    let _ = command(Command::AppChanged);
}

/// The tray icon, removed when dropped.
pub struct Tray {
    connection: Connection,
    name: String,
    stopped: Arc<AtomicBool>,
    commands: mpsc::Sender<Command>,
    worker: Option<thread::JoinHandle<()>>,
}
/// Ends what the value holds, at the point the host drops it: the icon leaves the tray and
/// notifications stop. The threads that listen to the bus end at their next signal.
impl Drop for Tray {
    fn drop(&mut self) {
        *COMMANDS.lock().unwrap() = None;
        self.stopped.store(true, Ordering::Relaxed);
        let _ = self.commands.send(Command::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let server = self.connection.object_server();
        let _ = server.remove::<item::Item, _>(ITEM_PATH);
        let _ = server.remove::<menu::Menu, _>(MENU_PATH);
        // The tray drops an item whose name goes.
        if let Err(error) = self.connection.release_name(self.name.as_str()) {
            tracing::debug!(%error, "the tray icon's bus name was not released");
        }
    }
}

impl Tray {
    pub fn new(icon: PathBuf, port: u16) -> Result<(Self, mpsc::Receiver<Action>)> {
        Self::new_options(icon, port, false)
    }
    /// Show the tray icon and return the receiver of what the user chooses in its menu.
    /// `icon` is the host's `.ico`, whose images the icon shows. Fails when the session bus
    /// cannot be reached; a desktop without a tray yet only shows the icon once it starts.
    pub fn new_options(
        icon: PathBuf,
        port: u16,
        hide_controls: bool,
    ) -> Result<(Self, mpsc::Receiver<Action>)> {
        let images = match std::fs::read(&icon) {
            Ok(bytes) => pixmap::ico_images(&bytes),
            Err(error) => {
                tracing::warn!(%error, path = %icon.display(), "the tray icon's picture could not be read");
                Vec::new()
            }
        };
        if images.is_empty() {
            tracing::warn!(path = %icon.display(), "no picture for the tray icon; it shows the desktop's generic one");
        }
        let (sender, receiver) = mpsc::channel();
        // The name the StatusNotifierItem specification gives an item's process.
        let name = format!("org.kde.StatusNotifierItem-{}-1", std::process::id());
        let connection = zbus::blocking::connection::Builder::session()
            .context("no session bus for the tray icon")?
            .name(name.as_str())?
            .serve_at(
                ITEM_PATH,
                item::Item::new(port, pixmap::pixmaps(&images), sender.clone()),
            )?
            .serve_at(MENU_PATH, menu::Menu::new(hide_controls, sender.clone()))?
            .build()
            .context("the session bus refused the tray icon")?;
        let stopped = Arc::new(AtomicBool::new(false));
        let (commands, queue) = mpsc::channel();
        watch_tray(&connection, &stopped, &commands)?;
        notices::listen(&connection, &stopped, sender)?;
        let worker = {
            let (connection, name) = (connection.clone(), name.clone());
            thread::Builder::new()
                .name("tray".into())
                .spawn(move || run(&connection, &name, &queue))?
        };
        let _ = commands.send(Command::Register);
        *COMMANDS.lock().unwrap() = Some(commands.clone());
        Ok((
            Self {
                connection,
                name,
                stopped,
                commands,
                worker: Some(worker),
            },
            receiver,
        ))
    }
}

/// Hand the icon to the tray again whenever its watcher starts: at sign-in after the host,
/// or after plasmashell restarts.
fn watch_tray(
    connection: &Connection,
    stopped: &Arc<AtomicBool>,
    commands: &mpsc::Sender<Command>,
) -> Result<()> {
    let changes = zbus::blocking::fdo::DBusProxy::new(connection)?
        .receive_name_owner_changed_with_args(&[(0, WATCHER)])
        .context("watching for the system tray")?;
    let (stopped, commands) = (stopped.clone(), commands.clone());
    thread::Builder::new()
        .name("tray-watcher".into())
        .spawn(move || {
            for change in changes {
                if stopped.load(Ordering::Relaxed) {
                    break;
                }
                let started = change.args().is_ok_and(|args| args.new_owner().is_some());
                if started && commands.send(Command::Register).is_err() {
                    break;
                }
            }
        })?;
    Ok(())
}

/// The tray's thread: everything it says on the bus, in order.
fn run(connection: &Connection, name: &str, queue: &mpsc::Receiver<Command>) {
    for command in queue {
        match command {
            Command::Register => register(connection, name),
            Command::AppChanged => {
                let revision = REVISION.fetch_add(1, Ordering::Relaxed) + 1;
                let signals = [
                    emit(connection, ITEM_PATH, ITEM_INTERFACE, "NewIcon", &()),
                    emit(connection, ITEM_PATH, ITEM_INTERFACE, "NewToolTip", &()),
                    emit(
                        connection,
                        MENU_PATH,
                        MENU_INTERFACE,
                        "LayoutUpdated",
                        &(revision, 0i32),
                    ),
                ];
                if let Some(Err(error)) = signals.into_iter().find(Result::is_err) {
                    tracing::warn!(%error, "the tray icon could not show the running app");
                }
            }
            Command::Notify(notice) => notices::show(connection, &notice),
            Command::Close(id) => notices::close(connection, id),
            Command::Stop => break,
        }
    }
}

fn emit<B>(
    connection: &Connection,
    path: &str,
    interface: &str,
    signal: &str,
    body: &B,
) -> Result<()>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    connection
        .emit_signal(None::<&str>, path, interface, signal, body)
        .with_context(|| format!("{interface}.{signal}"))
}

/// Hand the icon to the tray, and log whether it took it.
fn register(connection: &Connection, name: &str) {
    let present = zbus::blocking::fdo::DBusProxy::new(connection)
        .and_then(|bus| bus.name_has_owner(WATCHER.try_into()?).map_err(Into::into));
    match present {
        Ok(false) => {
            tracing::info!("no system tray yet; the tray icon goes in when one starts");
            return;
        }
        Ok(true) => {}
        Err(error) => tracing::debug!(%error, "could not ask whether a system tray runs"),
    }
    match connection.call_method(
        Some(WATCHER),
        "/StatusNotifierWatcher",
        Some(WATCHER),
        "RegisterStatusNotifierItem",
        &(name,),
    ) {
        Ok(_) => tracing::info!("tray icon added"),
        Err(error) => tracing::warn!(%error, "the system tray refused the tray icon"),
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

#[cfg(test)]
mod live_tests;
#[cfg(test)]
mod tests;
