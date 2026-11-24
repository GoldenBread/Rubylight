//! The clipboard on Linux: text and images (PNG), both ways, with a change count.
//!
//! The clipboard that applies is the newest running gamescope session of this process (its
//! Xwayland `DISPLAY`, through X11 selections: [`x11`]), else the user's Wayland desktop
//! (through `wl-copy` and `wl-paste` of wl-clipboard, which speak the data-control protocols
//! KWin, wlroots and others offer: [`wayland`]). Without a session of either, every call fails
//! saying so. The change count moves with every new selection and with every change of target,
//! so a stream that starts or ends a game session re-reads what the other side holds.

mod wayland;
mod x11;

use anyhow::{Result, bail};
use butterpollo_session::clipboard_image::{self, ImageError};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

/// The most text read from the clipboard, in bytes.
const MAX_TEXT: usize = 1024 * 1024;
/// The most text written, in bytes.
const MAX_WRITE_TEXT: usize = MAX_TEXT / 2;
/// The text formats read, best first, as MIME types (X11 adds its own names).
const TEXT_TYPES: [&str; 3] = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain"];

/// What a clipboard offers the host; one per target.
trait Backend: Send + Sync {
    /// How many selection changes this clipboard has seen.
    fn sequence(&self) -> u32;
    /// The text, `None` while there is none; at most `max` bytes.
    fn text(&self, max: usize) -> Result<Option<String>>;
    /// The PNG, `None` while there is none; fails over `max` bytes.
    fn png(&self, max: usize) -> Result<Option<Vec<u8>>>;
    fn set_text(&self, text: &str) -> Result<()>;
    fn set_png(&self, png: &[u8]) -> Result<()>;
}

/// The target's identity and its backend.
struct Target {
    key: String,
    backend: Arc<dyn Backend>,
}

/// The state shared by the calls: backends by target, the change in target, and the last
/// resolution (looked up at most every 500 ms: sequence is polled).
#[derive(Default)]
struct State {
    backends: HashMap<String, Arc<dyn Backend>>,
    resolved: Option<(Instant, Option<String>)>,
    last_key: Option<String>,
    epoch: u32,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// Which target applies: `x11:<display>` or `wayland:<socket>`; `None` when there is none.
fn choose(gamescope_display: Option<String>, desktop: Option<String>) -> Option<String> {
    match (gamescope_display, desktop) {
        (Some(display), _) => Some(format!("x11:{display}")),
        (None, Some(socket)) => Some(format!("wayland:{socket}")),
        _ => None,
    }
}

fn target() -> Result<(Target, u32)> {
    let mut guard = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    let state = guard.get_or_insert_with(State::default);
    let key = match &state.resolved {
        Some((at, key)) if at.elapsed() < Duration::from_millis(500) => key.clone(),
        _ => {
            let key = choose(
                crate::gamescope::current().and_then(|session| session.display),
                wayland::desktop_socket(),
            );
            state.resolved = Some((Instant::now(), key.clone()));
            key
        }
    };
    let Some(key) = key else {
        bail!("there is no clipboard: no game session runs and no Wayland desktop was found");
    };
    if state.last_key.as_deref() != Some(&key) {
        state.last_key = Some(key.clone());
        state.epoch = state.epoch.wrapping_add(1);
    }
    let backend = match state.backends.get(&key) {
        Some(backend) => backend.clone(),
        None => {
            let backend: Arc<dyn Backend> = match key.split_once(':') {
                Some(("x11", display)) => Arc::new(x11::Clipboard::open(display)?),
                Some((_, socket)) => Arc::new(wayland::Clipboard::new(socket)?),
                None => bail!("unknown clipboard target {key}"),
            };
            // A game's X server ends with it; its entry would otherwise stay.
            state
                .backends
                .retain(|k, _| k.starts_with("wayland:") || *k == key);
            state.backends.insert(key.clone(), backend.clone());
            backend
        }
    };
    Ok((Target { key, backend }, state.epoch))
}

/// Forgets a target whose connection failed, so the next call opens it afresh.
fn forget(key: &str) {
    if let Some(state) = STATE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_mut()
    {
        state.backends.remove(key);
    }
}

/// Runs `work` on the target's backend; a failure drops the backend for a fresh start.
fn with<T>(work: impl FnOnce(&dyn Backend) -> Result<T>) -> Result<T> {
    let (target, _) = target()?;
    let result = work(&*target.backend);
    if result.is_err() {
        forget(&target.key);
    }
    result
}

/// The clipboard's change count, which every new content raises. It also changes when the
/// target does, and stays 0 without one.
pub fn sequence() -> u32 {
    match target() {
        Ok((target, epoch)) => target
            .backend
            .sequence()
            .wrapping_add(epoch.wrapping_mul(0x9E37_79B1)),
        Err(_) => 0,
    }
}

/// The clipboard's text, empty while there is none.
pub fn read() -> Result<String> {
    Ok(with(|backend| backend.text(MAX_TEXT))?.unwrap_or_default())
}

/// Put `text` on the clipboard.
pub fn write(text: &str) -> Result<()> {
    if text.len() > MAX_WRITE_TEXT || text.contains('\0') {
        bail!("invalid clipboard text size or NUL character");
    }
    with(|backend| backend.set_text(text))
}

/// The clipboard's image as PNG of at most `max_bytes`, or `None` while it holds none. A PNG
/// the clipboard holds is passed on without anything after its end; an image over `max_bytes`
/// fails with [`ImageError::TooManyBytes`], a PNG cut short with [`ImageError::Malformed`].
pub fn read_png(max_bytes: usize) -> Result<Option<Vec<u8>>> {
    let Some(mut png) = with(|backend| backend.png(max_bytes))? else {
        return Ok(None);
    };
    let end = clipboard_image::png_extent(&png)
        .ok_or_else(|| ImageError::Malformed("the clipboard's PNG is cut short".into()))?;
    if end > max_bytes {
        return Err(ImageError::TooManyBytes {
            bytes: end,
            max: max_bytes,
        }
        .into());
    }
    png.truncate(end);
    Ok(Some(png))
}

/// Put the PNG `png` on the clipboard.
pub fn write_png(png: &[u8]) -> Result<()> {
    if clipboard_image::png_extent(png).is_none() {
        return Err(ImageError::Malformed("not a complete PNG".into()).into());
    }
    with(|backend| backend.set_png(png))
}

/// The best of `offered` among the text formats, `None` when it offers none.
fn pick_text<'a>(offered: &'a [String], extra: &[&str]) -> Option<&'a str> {
    TEXT_TYPES
        .iter()
        .chain(extra)
        .find_map(|wanted| offered.iter().find(|o| o == wanted))
        .map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_game_session_wins_over_the_desktop() {
        assert_eq!(
            choose(Some(":3".into()), Some("wayland-0".into())).as_deref(),
            Some("x11::3")
        );
        assert_eq!(
            choose(None, Some("wayland-0".into())).as_deref(),
            Some("wayland:wayland-0")
        );
        assert_eq!(choose(None, None), None);
    }

    #[test]
    fn the_best_text_format_is_chosen() {
        let offered = vec![
            "image/png".to_string(),
            "text/plain".into(),
            "UTF8_STRING".into(),
        ];
        assert_eq!(pick_text(&offered, &[]), Some("UTF8_STRING"));
        assert_eq!(pick_text(&offered[..1], &[]), None);
        let latin = vec!["STRING".to_string()];
        assert_eq!(pick_text(&latin, &["STRING"]), Some("STRING"));
    }

    #[test]
    fn text_over_the_limit_or_with_nul_is_refused() {
        assert!(write("a\0b").is_err());
        assert!(write(&"a".repeat(MAX_WRITE_TEXT + 1)).is_err());
    }

    #[test]
    fn a_png_that_is_not_one_is_refused() {
        assert!(write_png(b"nope").is_err());
    }
}
