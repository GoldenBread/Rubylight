//! What the PC's screen shows, from systemd-logind: the active session of `seat0`, which is
//! this user's desktop, the login screen, or something else (another user's session, a text
//! console). A host started at boot (rubylight.service under linger) streams its user's
//! desktop through the portal only while that desktop is on screen; otherwise a Desktop stream
//! shows the screen through Rubylight's KMS broker (target `kms`), whatever owns it.
//!
//! The answer is kept for [`FRESH`], as a stream asks ten times a second. Without logind, a
//! seat or a session on it (a container, a headless box, a CI runner, a text console before
//! anyone signs in) the answer is [`Shown::Unknown`] and the host goes by its environment as
//! before. `RUBYLIGHT_SEAT` (`own`, `login`, `other` or `unknown`) sets the answer instead, for
//! tests and to try the login screen's behaviour.
use crate::power::logind;
use anyhow::{Context, Result};
use std::{
    sync::{Mutex, PoisonError},
    time::{Duration, Instant},
};
use zbus::{
    blocking::{Connection, Proxy},
    zvariant::OwnedObjectPath,
};

/// How long an answer stands.
const FRESH: Duration = Duration::from_millis(500);
const SEAT: &str = "/org/freedesktop/login1/seat/seat0";

/// What the seat's screen shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    /// This user's graphical session.
    Own,
    /// The login screen.
    LoginScreen,
    /// Another user's session, or a session of this user that is not graphical.
    Other,
    /// logind or the seat cannot be asked, or the seat has no session.
    Unknown,
}

/// The active session's user, class (`user`, `greeter`, ...) and type (`wayland`, `x11`,
/// `tty`, ...).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Active {
    uid: u32,
    class: String,
    kind: String,
}

fn classify(active: Option<&Active>, me: u32) -> Shown {
    let Some(active) = active else {
        return Shown::Unknown;
    };
    match active.class.as_str() {
        "greeter" => Shown::LoginScreen,
        "user" | "user-early" | "user-incomplete"
            if active.uid == me && matches!(active.kind.as_str(), "wayland" | "x11" | "mir") =>
        {
            Shown::Own
        }
        _ => Shown::Other,
    }
}

/// Runs a blocking D-Bus call on a thread of its own. Built with tokio (as when the client's
/// crates share the build), zbus's blocking calls start a runtime, which panics on a thread
/// that already drives one; the host asks from its async handlers and the programs it starts.
fn off_runtime<T: Send>(call: impl FnOnce() -> Result<T> + Send) -> Result<T> {
    std::thread::scope(|scope| {
        scope
            .spawn(call)
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

fn ask() -> Result<Option<Active>> {
    off_runtime(ask_logind)
}

fn ask_logind() -> Result<Option<Active>> {
    let connection = Connection::system().context("connecting to the system D-Bus")?;
    let seat = Proxy::new(
        &connection,
        "org.freedesktop.login1",
        SEAT,
        "org.freedesktop.login1.Seat",
    )?;
    let (id, path): (String, OwnedObjectPath) = seat
        .get_property("ActiveSession")
        .context("reading seat0's active session")?;
    if id.is_empty() || path.as_str() == "/" {
        return Ok(None);
    }
    let session = Proxy::new(
        &connection,
        "org.freedesktop.login1",
        path,
        "org.freedesktop.login1.Session",
    )?;
    let (uid, _): (u32, OwnedObjectPath) = session.get_property("User")?;
    Ok(Some(Active {
        uid,
        class: session.get_property("Class")?,
        kind: session.get_property("Type")?,
    }))
}

static LAST: Mutex<Option<(Instant, Shown)>> = Mutex::new(None);

/// What the seat shows now, at most [`FRESH`] old.
pub fn shown() -> Shown {
    let mut last = LAST.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((at, shown)) = *last
        && at.elapsed() < FRESH
    {
        return shown;
    }
    // SAFETY: getuid cannot fail.
    let me = unsafe { libc::getuid() };
    let forced = std::env::var("RUBYLIGHT_SEAT")
        .ok()
        .and_then(|value| match value.as_str() {
            "own" => Some(Shown::Own),
            "login" => Some(Shown::LoginScreen),
            "other" => Some(Shown::Other),
            "unknown" => Some(Shown::Unknown),
            _ => None,
        });
    let shown = match forced
        .map(Ok)
        .unwrap_or_else(|| ask().map(|active| classify(active.as_ref(), me)))
    {
        Ok(shown) => shown,
        Err(error) => {
            tracing::debug!(error = %format!("{error:#}"), "what the seat shows is unknown");
            Shown::Unknown
        }
    };
    let changed = last.is_some_and(|(_, before)| before != shown);
    if changed {
        tracing::info!(?shown, "the PC's screen changed hands");
    }
    *last = Some((Instant::now(), shown));
    shown
}

/// Whether logind says something else than this user's desktop is on screen: the login screen
/// or another session. What the host may stream and launch is narrowed only then, never on a
/// PC that cannot be asked.
pub fn desktop_elsewhere() -> bool {
    matches!(shown(), Shown::LoginScreen | Shown::Other)
}

/// Whether this user's desktop is the one on screen: logind's answer, or with none, whether
/// the host was started in a graphical session.
pub fn own_desktop_shown() -> bool {
    match shown() {
        Shown::Own => true,
        Shown::Unknown => {
            std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some()
        }
        Shown::LoginScreen | Shown::Other => false,
    }
}

/// The environment the user's services get now (the systemd user manager's), which the
/// desktop session adds its display to when it starts: what a program the host starts for the
/// desktop needs, as the host itself may have started before the desktop did.
pub fn user_environment() -> Result<Vec<(String, String)>> {
    off_runtime(user_manager_environment)
}

fn user_manager_environment() -> Result<Vec<(String, String)>> {
    let bus = logind::session_bus()?;
    let manager = Proxy::new(
        &bus,
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
    )?;
    let entries: Vec<String> = manager
        .get_property("Environment")
        .context("reading the user manager's environment")?;
    Ok(entries
        .into_iter()
        .filter_map(|entry| {
            let (name, value) = entry.split_once('=')?;
            Some((name.to_owned(), value.to_owned()))
        })
        .collect())
}

/// The user's desktop's environment while it is on screen: the user manager's, which the
/// desktop added its display to, else (logind not asked) this host's own. Nothing while the
/// desktop is not on screen, as a value left from a session that ended points nowhere.
pub fn desktop_environment() -> Vec<(String, String)> {
    match shown() {
        Shown::Own => user_environment().unwrap_or_else(|error| {
            tracing::debug!(error = %format!("{error:#}"), "the desktop's environment is unknown; the host's own stands in");
            std::env::vars().collect()
        }),
        Shown::Unknown => std::env::vars().collect(),
        Shown::LoginScreen | Shown::Other => Vec::new(),
    }
}

/// One variable of [`desktop_environment`], such as `WAYLAND_DISPLAY`. With the desktop on
/// screen and the user manager silent on it, this host's own value.
pub fn desktop_var(name: &str) -> Option<String> {
    let environment = desktop_environment();
    if environment.is_empty() && shown() != Shown::Own {
        return None;
    }
    environment
        .into_iter()
        .find_map(|(key, value)| (key == name).then_some(value))
        .or_else(|| std::env::var(name).ok())
        .filter(|value| !value.is_empty())
}

/// Gives a program the host starts for the desktop its environment ([`desktop_environment`]):
/// the host may have started at boot, before the desktop and its display. While the desktop
/// is not on screen the display variables are taken away, so a program does not reach for a
/// display that is gone.
pub fn for_desktop(command: &mut std::process::Command) {
    let environment = desktop_environment();
    if environment.is_empty() {
        for name in DISPLAY_VARIABLES {
            command.env_remove(name);
        }
    }
    command.envs(environment);
}

/// The variables that name the desktop's display.
const DISPLAY_VARIABLES: [&str; 3] = ["WAYLAND_DISPLAY", "DISPLAY", "XAUTHORITY"];

#[cfg(test)]
mod tests {
    use super::*;

    fn active(uid: u32, class: &str, kind: &str) -> Active {
        Active {
            uid,
            class: class.into(),
            kind: kind.into(),
        }
    }

    #[test]
    fn the_screen_is_told_apart() {
        assert_eq!(classify(None, 1000), Shown::Unknown);
        assert_eq!(
            classify(Some(&active(966, "greeter", "wayland")), 1000),
            Shown::LoginScreen
        );
        assert_eq!(
            classify(Some(&active(1000, "user", "wayland")), 1000),
            Shown::Own
        );
        assert_eq!(
            classify(Some(&active(1000, "user", "x11")), 1000),
            Shown::Own
        );
        assert_eq!(
            classify(Some(&active(1000, "user", "tty")), 1000),
            Shown::Other
        );
        assert_eq!(
            classify(Some(&active(1001, "user", "wayland")), 1000),
            Shown::Other
        );
    }

    /// Prints what the seat shows and the user manager's display variables.
    #[test]
    #[ignore = "asks this PC's logind"]
    fn live_the_seat_answers() {
        eprintln!("{:?}", shown());
        for (name, value) in user_environment().unwrap() {
            if name.contains("DISPLAY") || name.starts_with("XDG_SESSION") {
                eprintln!("{name}={value}");
            }
        }
    }
}
