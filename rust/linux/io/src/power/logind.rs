//! systemd-logind over the system bus, blocking: inhibitor locks, the power calls, the session
//! the host runs in, and the signals that announce sleep, resume and shutdown. Each call opens
//! its own connection; none is hot, and a signal listener keeps one for as long as it runs.
//! Also the user's session bus, for the display wake and the idle time.

use anyhow::{Context, Result};
use std::{sync::mpsc, time::Duration};
use zbus::{
    blocking::{Connection, Proxy},
    message::Message,
    zvariant::OwnedFd,
};

const DESTINATION: &str = "org.freedesktop.login1";
const PATH: &str = "/org/freedesktop/login1";
const MANAGER: &str = "org.freedesktop.login1.Manager";

/// The logind manager on a fresh connection to the system bus.
pub(crate) fn manager() -> Result<Proxy<'static>> {
    let connection = Connection::system().context("connecting to the system D-Bus")?;
    Proxy::new(&connection, DESTINATION, PATH, MANAGER).context("reaching systemd-logind")
}

/// The user's session bus, also when `DBUS_SESSION_BUS_ADDRESS` is not set (a service started
/// outside the session): the bus socket in the runtime folder.
pub(crate) fn session_bus() -> Result<Connection> {
    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some() {
        return Connection::session().context("connecting to the session D-Bus");
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").context("XDG_RUNTIME_DIR is not set")?;
    let address = format!("unix:path={}/bus", runtime.to_string_lossy());
    zbus::blocking::connection::Builder::address(address.as_str())?
        .build()
        .context("connecting to the session D-Bus")
}

/// A lock that blocks sleep and idle actions until dropped (`Inhibit`, mode `block`).
pub(crate) fn inhibit(what: &str, who: &str, why: &str) -> Result<OwnedFd> {
    manager()?
        .call("Inhibit", &(what, who, why, "block"))
        .context("taking a systemd-logind inhibitor")
}

/// What logind answers to `Can<action>`: `yes`, `no`, `na` or `challenge`.
pub(crate) fn can(action: &str) -> Result<String> {
    manager()?
        .call(&*format!("Can{action}"), &())
        .with_context(|| format!("asking logind whether the PC can {action}"))
}

/// Calls a power method of the manager (`Suspend`, `Reboot`, `PowerOff`) without asking for
/// interactive authentication.
pub(crate) fn power_call(method: &str) -> Result<()> {
    manager()?
        .call::<_, _, ()>(method, &(false,))
        .with_context(|| format!("logind refused {method}"))
}

/// The session this process runs in, by id, else logind's `auto` alias.
pub(crate) fn session() -> Result<Proxy<'static>> {
    let connection = Connection::system().context("connecting to the system D-Bus")?;
    let path = match std::env::var("XDG_SESSION_ID") {
        Ok(id) if !id.is_empty() => format!("{PATH}/session/{}", escape(&id)),
        _ => format!("{PATH}/session/auto"),
    };
    Proxy::new(
        &connection,
        DESTINATION,
        path,
        "org.freedesktop.login1.Session",
    )
    .context("reaching the logind session")
}

/// A session id as a D-Bus object path element: every byte outside `[A-Za-z0-9]` becomes `_`
/// and two hex digits.
fn escape(id: &str) -> String {
    id.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() {
                (byte as char).to_string()
            } else {
                format!("_{byte:02x}")
            }
        })
        .collect()
}

/// The boolean argument of a `PrepareFor*` signal: true as the PC goes down, false as it
/// comes back.
pub(crate) fn flag(message: &Message) -> Option<bool> {
    message.body().deserialize::<bool>().ok()
}

/// Runs `call` (a `Suspend`) with the manager's `PrepareForSleep` signal already subscribed,
/// and returns once the PC has gone to sleep and come back. Fails when it does not start to
/// sleep within 30 s of the call.
pub(crate) fn call_then_wait_resume(call: impl FnOnce() -> Result<()>) -> Result<()> {
    let signals = manager()?.receive_signal("PrepareForSleep")?;
    let (send, receive) = mpsc::channel();
    // The listener ends with its next signal after this function has returned.
    std::thread::spawn(move || {
        for message in signals {
            if let Some(flag) = flag(&message)
                && send.send(flag).is_err()
            {
                break;
            }
        }
    });
    call()?;
    receive
        .recv_timeout(Duration::from_secs(30))
        .ok()
        .filter(|going| *going)
        .context("the PC did not go to sleep")?;
    loop {
        if !receive.recv().context("the sleep signal stopped")? {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_ids_are_escaped_like_logind_does() {
        assert_eq!(escape("2"), "2");
        assert_eq!(escape("c1"), "c1");
        assert_eq!(escape("a-b"), "a_2db");
    }
}
