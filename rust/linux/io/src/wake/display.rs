//! Idle time and display wake on the user's desktop session. Each desktop answers in its own
//! way, so each is tried: GNOME's Mutter idle monitor and KDE's screensaver service on the
//! session bus, then logind's idle hint. A headless gamescope session has no display to wake.

use anyhow::{Context, Result, bail};
use zbus::blocking::Proxy;

use crate::power::logind;

/// Seconds without input in this session, from the first source that knows.
pub(super) fn idle_seconds() -> Result<u32> {
    if let Ok(bus) = logind::session_bus() {
        // Mutter's idle monitor, in milliseconds.
        if let Ok(proxy) = Proxy::new(
            &bus,
            "org.gnome.Mutter.IdleMonitor",
            "/org/gnome/Mutter/IdleMonitor/Core",
            "org.gnome.Mutter.IdleMonitor",
        ) && let Ok(ms) = proxy.call::<_, _, u64>("GetIdletime", &())
        {
            return Ok(seconds(ms));
        }
        // KDE answers this on X11 only; on Wayland it says it is not supported.
        if let Ok(proxy) = Proxy::new(
            &bus,
            "org.freedesktop.ScreenSaver",
            "/ScreenSaver",
            "org.freedesktop.ScreenSaver",
        ) && let Ok(ms) = proxy.call::<_, _, u32>("GetSessionIdleTime", &())
        {
            return Ok(seconds(u64::from(ms)));
        }
    }
    let session = logind::session()?;
    let idle: bool = session
        .get_property("IdleHint")
        .context("reading the session's idle hint")?;
    if !idle {
        return Ok(0);
    }
    let since: u64 = session.get_property("IdleSinceHint")?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |time| time.as_micros() as u64);
    Ok(seconds(now.saturating_sub(since) / 1000))
}

fn seconds(milliseconds: u64) -> u32 {
    u32::try_from(milliseconds / 1000).unwrap_or(u32::MAX)
}

/// Wakes the desktop's display: a simulated user activity, which KDE and others answer on
/// `org.freedesktop.ScreenSaver`, and logind's idle hint cleared for the session. Succeeds
/// when either took, and is a no-op while a gamescope session of this host runs, which has no
/// physical display.
pub(super) fn wake_console(reason: &str) -> Result<()> {
    if crate::gamescope::current().is_some() {
        tracing::debug!(
            reason,
            "a headless gamescope session runs; no display to wake"
        );
        return Ok(());
    }
    let activity = logind::session_bus().and_then(|bus| {
        Proxy::new(
            &bus,
            "org.freedesktop.ScreenSaver",
            "/ScreenSaver",
            "org.freedesktop.ScreenSaver",
        )?
        .call::<_, _, ()>("SimulateUserActivity", &())
        .context("SimulateUserActivity")
    });
    let hint = logind::session().and_then(|session| {
        session
            .call::<_, _, ()>("SetIdleHint", &(false,))
            .context("SetIdleHint")
    });
    match (activity, hint) {
        (Err(a), Err(h)) => bail!("no way to wake the display worked: {a:#}; {h:#}"),
        _ => {
            tracing::info!(
                reason,
                "simulated user activity to wake the console display"
            );
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::seconds;

    #[test]
    fn milliseconds_become_whole_seconds() {
        assert_eq!(seconds(0), 0);
        assert_eq!(seconds(61_999), 61);
        assert_eq!(seconds(u64::MAX), u32::MAX);
    }
}
