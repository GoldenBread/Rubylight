//! The user's own desktop through xdg-desktop-portal: a ScreenCast session (`CreateSession`,
//! `SelectSources` for monitors with the cursor embedded, `Start`, `OpenPipeWireRemote`) that
//! hands the capture a PipeWire descriptor and the nodes of the monitors the user shared.
//!
//! The portal asks the user once, in the desktop's own dialog. The session is made persistent
//! (`persist_mode` 2) and the restore token it returns is kept in the host's state folder, so
//! the next stream starts without a question: `$XDG_STATE_HOME/rubylight/` (else
//! `~/.local/state/rubylight/`), in `screencast.token` for a single monitor and
//! `screencast-multi.token` when every monitor is asked for. The session lives as long as the
//! [`ScreenCast`] that holds its D-Bus connection.

pub mod layout;

use anyhow::{Context, Result, anyhow, bail};
use std::{
    collections::HashMap,
    os::fd::OwnedFd,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
        mpsc,
    },
    time::Duration,
};
use zbus::{
    blocking::{Connection, Proxy, proxy::Builder},
    proxy::CacheProperties,
    zvariant::{OwnedObjectPath, OwnedValue, Value},
};

const BUS_NAME: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SCREENCAST: &str = "org.freedesktop.portal.ScreenCast";
/// How long the user has to answer the portal's dialog.
pub const DIALOG_WAIT: Duration = Duration::from_secs(120);

/// One monitor the portal shares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Monitor {
    /// The PipeWire node that carries its picture.
    pub node: u32,
    /// Where it sits in the desktop and how large it is, in pixels, when the portal says
    /// (KWin and Mutter do; 0 for a size otherwise).
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// A running ScreenCast session.
pub struct ScreenCast {
    conn: Connection,
    session: OwnedObjectPath,
    /// The monitors shared, left to right then top to bottom.
    pub monitors: Vec<Monitor>,
    /// Whether a restore token from an earlier answer was offered (the portal may still ask).
    pub token_offered: bool,
}

/// Whether a desktop session with a portal may be there: a Wayland (or X) display and a
/// session bus. It does not ask the portal.
pub fn desktop_session_present() -> bool {
    (std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some())
        && (std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some()
            || std::env::var_os("XDG_RUNTIME_DIR")
                .is_some_and(|dir| std::path::Path::new(&dir).join("bus").exists()))
}

/// The desktop's monitor a capture target shows, which its input goes to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DesktopMonitor {
    /// `desktop`: the primary.
    Primary,
    /// `desktop:<n>`: the n-th in [`layout::monitors`] order.
    Index(usize),
    /// `kwin-output:<name>`: the output of that name, such as a virtual display.
    Named(String),
}

/// The monitor a desktop capture target names; `None` for any other target (a gamescope
/// session's). A KMS capture (`kms`, `kms:<output>`, the login screen's too) shows the desktop
/// of whichever compositor owns the screen.
pub fn desktop_target(target: &str) -> Option<DesktopMonitor> {
    if target == "desktop" || target == "kms" {
        return Some(DesktopMonitor::Primary);
    }
    if let Some(name) = target
        .strip_prefix("kwin-output:")
        .or_else(|| target.strip_prefix("kms:"))
        .filter(|name| !name.is_empty())
    {
        return Some(DesktopMonitor::Named(name.to_owned()));
    }
    target
        .strip_prefix("desktop:")?
        .parse()
        .ok()
        .map(DesktopMonitor::Index)
}

/// The folder the restore tokens are kept in.
pub fn state_dir() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("state"))
        })
        .unwrap_or_else(std::env::temp_dir);
    base.join("rubylight")
}

fn token_file(multiple: bool) -> PathBuf {
    state_dir().join(if multiple {
        "screencast-multi.token"
    } else {
        "screencast.token"
    })
}

fn save_token(multiple: bool, token: &str) {
    let path = token_file(multiple);
    let result = (|| -> std::io::Result<()> {
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        std::fs::create_dir_all(path.parent().unwrap_or(&path))?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)?;
        file.write_all(token.as_bytes())
    })();
    if let Err(error) = result {
        tracing::warn!(%error, path = %path.display(), "cannot keep the screen share's restore token");
    }
}

/// Process-wide counter of the tokens that name requests and sessions.
fn fresh_token() -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(
        "rubylight_{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

type Results = HashMap<String, OwnedValue>;

fn portal(conn: &Connection, interface: &'static str) -> Result<Proxy<'static>> {
    Builder::new(conn)
        .destination(BUS_NAME)?
        .path(PORTAL_PATH)?
        .interface(interface)?
        .cache_properties(CacheProperties::No)
        .build()
        .with_context(|| format!("reaching {interface}"))
}

/// Calls a portal method that answers through a `Request` and waits for that answer. The
/// request's object path is known before the call, so the listener is in place first.
fn request<B>(
    conn: &Connection,
    proxy: &Proxy<'static>,
    method: &str,
    token: &str,
    body: &B,
) -> Result<Results>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    let unique = conn.unique_name().context("no D-Bus name")?;
    let sender = unique.as_str().trim_start_matches(':').replace('.', "_");
    let path = format!("{PORTAL_PATH}/request/{sender}/{token}");
    let response: Proxy<'_> = Builder::new(conn)
        .destination(BUS_NAME)?
        .path(path)?
        .interface("org.freedesktop.portal.Request")?
        .cache_properties(CacheProperties::No)
        .build()?;
    let mut signals = response.receive_signal("Response")?;
    let _handle: OwnedObjectPath = proxy
        .call(method, body)
        .with_context(|| format!("the portal's {method}"))?;
    let message = signals
        .next()
        .ok_or_else(|| anyhow!("the portal closed without answering {method}"))?;
    let (code, results): (u32, Results) = message.body().deserialize()?;
    match code {
        0 => Ok(results),
        1 => bail!("the screen share was declined in the portal's dialog ({method})"),
        _ => bail!("the portal ended {method} (response {code})"),
    }
}

impl ScreenCast {
    /// Opens a session for monitors, restoring the user's earlier choice when the token still
    /// holds. With `multiple` the user (or the token) may share several monitors. Waits up to
    /// `wait` for the portal's dialog; the session ends when the returned value drops.
    pub fn open(multiple: bool, wait: Duration) -> Result<Arc<Self>> {
        let (sender, receiver) = mpsc::channel();
        // The D-Bus calls block until the user answers, so they run apart and a dialog nobody
        // answers is reported instead of waited on forever.
        std::thread::Builder::new()
            .name("portal-screencast".into())
            .spawn(move || {
                let _ = sender.send(Self::negotiate(multiple).map(Arc::new));
            })
            .context("starting the portal thread")?;
        match receiver.recv_timeout(wait) {
            Ok(result) => result,
            Err(_) => bail!(
                "nobody answered the desktop's screen share dialog within {wait:?}; answer it once \
                 (\"remember\" keeps it from asking again) and start the stream again"
            ),
        }
    }

    fn negotiate(multiple: bool) -> Result<Self> {
        let conn = Connection::session().context("connecting to the session bus")?;
        let cast = portal(&conn, SCREENCAST)?;
        let version = cast.get_property::<u32>("version").unwrap_or(0);
        if version < 4 {
            tracing::warn!(version, "the ScreenCast portal is old: no restore token");
        }
        let token = fresh_token();
        let created = request(
            &conn,
            &cast,
            "CreateSession",
            &token,
            &(HashMap::from([
                ("handle_token", Value::from(token.clone())),
                ("session_handle_token", Value::from(fresh_token())),
            ]),),
        )
        .context("CreateSession")?;
        let session: String = created
            .get("session_handle")
            .context("the portal named no session")?
            .try_clone()?
            .try_into()
            .context("the session handle")?;
        let session = OwnedObjectPath::try_from(session)?;
        let saved = std::fs::read_to_string(token_file(multiple))
            .ok()
            .map(|token| token.trim().to_owned())
            .filter(|token| !token.is_empty());
        let token = fresh_token();
        let mut options = HashMap::from([
            ("handle_token", Value::from(token.clone())),
            // MONITOR.
            ("types", Value::from(1u32)),
            ("multiple", Value::from(multiple)),
            // Embedded: the cursor is part of the picture.
            ("cursor_mode", Value::from(2u32)),
            // Until the user revokes it.
            ("persist_mode", Value::from(2u32)),
        ]);
        if let Some(token) = &saved {
            options.insert("restore_token", Value::from(token.clone()));
        }
        request(&conn, &cast, "SelectSources", &token, &(&session, options))
            .context("SelectSources")?;
        let token = fresh_token();
        let started = request(
            &conn,
            &cast,
            "Start",
            &token,
            &(
                &session,
                "",
                HashMap::from([("handle_token", Value::from(token.clone()))]),
            ),
        )
        .context("Start")?;
        if let Some(Ok(token)) = started
            .get("restore_token")
            .and_then(|v| v.try_clone().ok())
            .map(String::try_from)
        {
            save_token(multiple, &token);
        }
        let streams: Vec<(u32, HashMap<String, OwnedValue>)> = started
            .get("streams")
            .context("the portal shared no stream")?
            .try_clone()?
            .try_into()
            .context("the stream list")?;
        let mut monitors: Vec<Monitor> = streams
            .into_iter()
            .map(|(node, props)| {
                let pair = |key: &str| -> (i32, i32) {
                    props
                        .get(key)
                        .and_then(|v| v.try_clone().ok())
                        .and_then(|v| <(i32, i32)>::try_from(v).ok())
                        .unwrap_or((0, 0))
                };
                let ((x, y), (width, height)) = (pair("position"), pair("size"));
                Monitor {
                    node,
                    x,
                    y,
                    width,
                    height,
                }
            })
            .collect();
        if monitors.is_empty() {
            bail!("the portal shared no monitor");
        }
        monitors.sort_by_key(|m| (m.x, m.y, m.node));
        layout::note(&monitors);
        Ok(Self {
            conn,
            session,
            monitors,
            token_offered: saved.is_some(),
        })
    }

    /// A new connection to the PipeWire daemon that sees only the shared nodes.
    pub fn open_remote(&self) -> Result<OwnedFd> {
        let cast = portal(&self.conn, SCREENCAST)?;
        let reply = cast
            .call_method(
                "OpenPipeWireRemote",
                &(&self.session, HashMap::<&str, Value>::new()),
            )
            .context("OpenPipeWireRemote")?;
        let fd: zbus::zvariant::OwnedFd = reply.body().deserialize()?;
        Ok(fd.into())
    }
}

impl Drop for ScreenCast {
    fn drop(&mut self) {
        // The portal ends the session with the connection; closing it politely lets the
        // desktop drop its "sharing" indicator at once.
        if let Ok(session) = Builder::<Proxy<'_>>::new(&self.conn)
            .destination(BUS_NAME)
            .and_then(|b| b.path(self.session.clone()))
            .and_then(|b| b.interface("org.freedesktop.portal.Session"))
            .map(|b| b.cache_properties(CacheProperties::No))
            .and_then(|b| b.build())
        {
            let _ = session.call_method("Close", &());
        }
    }
}

#[cfg(test)]
mod target_tests {
    use super::{DesktopMonitor, desktop_target};

    #[test]
    fn desktop_targets_name_their_monitor() {
        assert_eq!(desktop_target("desktop"), Some(DesktopMonitor::Primary));
        assert_eq!(desktop_target("desktop:1"), Some(DesktopMonitor::Index(1)));
        assert_eq!(
            desktop_target("kwin-output:Virtual-1"),
            Some(DesktopMonitor::Named("Virtual-1".into()))
        );
        assert_eq!(desktop_target("kms"), Some(DesktopMonitor::Primary));
        assert_eq!(
            desktop_target("kms:DP-1"),
            Some(DesktopMonitor::Named("DP-1".into()))
        );
        assert_eq!(desktop_target("kms:"), None);
        assert_eq!(desktop_target("gamescope"), None);
    }
}
