//! The user's own desktop as a capture target: `desktop` (the primary monitor) and
//! `desktop:<index>` (the monitor of that number in [`displays`]), taken through the
//! xdg-desktop-portal ScreenCast session of `butterpollo_linux_io::portal`.

use super::Display;
use butterpollo_linux_io::portal::{self, Monitor, layout};

/// The capture target of the desktop's primary monitor.
pub const DESKTOP_TARGET: &str = "desktop";

/// The monitor a target names: `Some(None)` for `desktop`, `Some(Some(n))` for `desktop:<n>`,
/// `None` for any other target.
pub fn parse(target: &str) -> Option<Option<usize>> {
    portal::desktop_target(target)
}

/// The monitors of the desktop, numbered as the host's displays are: the primary is
/// `desktop:0`. Sizes and places come from the desktop (see [`layout::monitors`]); none are
/// listed without a desktop session.
pub fn displays() -> Vec<Display> {
    if !portal::desktop_session_present() {
        return Vec::new();
    }
    layout::monitors()
        .into_iter()
        .enumerate()
        .map(|(index, (name, rect))| Display {
            device_id: format!("desktop:{index}"),
            display_name: format!("desktop:{index}"),
            friendly_name: name,
            width: rect.width.max(0) as u32,
            height: rect.height.max(0) as u32,
            x: rect.x,
            y: rect.y,
            primary: index == 0,
            adapter: String::new(),
            adapter_index: 0,
            output_index: index as u32,
        })
        .collect()
}

/// Whether the desktop target `name` can be opened now: a desktop session is there and, for a
/// numbered monitor, the desktop has that many.
pub fn present(name: &str) -> bool {
    match parse(name) {
        Some(None) => portal::desktop_session_present(),
        Some(Some(index)) => displays().len() > index,
        None => false,
    }
}

/// The portal's monitor for a target: the first (leftmost) for `desktop`, else the numbered
/// one among those the user shared.
pub fn pick(monitors: &[Monitor], index: Option<usize>) -> anyhow::Result<Monitor> {
    monitors.get(index.unwrap_or(0)).copied().ok_or_else(|| {
        anyhow::anyhow!(
            "the desktop shares {} monitor(s) with Rubylight, not monitor {}",
            monitors.len(),
            index.unwrap_or(0)
        )
    })
}
