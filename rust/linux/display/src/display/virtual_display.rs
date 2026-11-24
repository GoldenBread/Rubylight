//! Virtual displays as the host sees them on Linux: whether they can be served, the status
//! the console shows, and the GPU they render on.
//!
//! Linux has no virtual display backend yet. Every availability check answers no, the status
//! says why, and creating a display fails.
use anyhow::Result;

/// The Windows build number. None on Linux, where there is none to read.
pub fn windows_build() -> Option<u32> {
    None
}

/// Whether this system can serve virtual displays at all. Not on Linux yet, so the host
/// defaults to streaming a display that is already there.
pub fn virtual_display_os() -> bool {
    false
}

/// Whether a virtual display can be made now. Never on Linux yet.
pub fn virtual_display_available() -> bool {
    false
}

/// Whether the virtual display driver counts as installed. Linux has none.
pub fn virtual_display_installed() -> bool {
    false
}

/// [`virtual_display_available`] for a launch, which may repair a failed driver first. There
/// is nothing to repair on Linux.
pub fn ensure_virtual_display_available() -> bool {
    false
}

/// The virtual display's state for the console and diagnostics: not capable, not ready, and
/// the reason.
pub fn virtual_display_status() -> serde_json::Value {
    serde_json::json!({
        "capable": false,
        "ready": false,
        "reason": format!("{:#}", crate::unsupported("A virtual display")),
        "protocol": "rubylight",
    })
}

/// How a virtual display is made.
#[derive(Clone, Debug)]
pub struct VirtualOptions {
    /// The peak brightness the display advertises for HDR.
    pub peak_nits: u32,
    /// The GPU to render the display on; None leaves the choice to the system.
    pub render_adapter: Option<RenderAdapter>,
    /// The stream joins a running game's display and takes it as it is
    /// ([`joinable_mode`]).
    pub joins: bool,
}

impl Default for VirtualOptions {
    fn default() -> Self {
        Self {
            peak_nits: 1000,
            render_adapter: None,
            joins: false,
        }
    }
}

/// The GPU that renders virtual displays, by name and adapter id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderAdapter {
    pub name: String,
    pub luid: (u32, i32),
}

/// The GPU the Graphics card setting names, for virtual displays to render on. `select`
/// resolves the configured adapter name and device id; a GPU it cannot find is logged and
/// leaves the choice to the system.
pub fn configured_render_adapter_with(
    config: &butterpollo_config::config::Config,
    select: impl FnOnce(&str, &str) -> Result<Option<RenderAdapter>>,
) -> Option<RenderAdapter> {
    select(
        config.get("adapter_name", ""),
        config.get("adapter_pnp_id", ""),
    )
    .inspect_err(|error| {
        tracing::warn!(error = %format!("{error:#}"), "the selected GPU was not found; the system picks the GPU that renders the virtual display")
    })
    .ok()
    .flatten()
}

/// The mode (width, height, refresh in millihertz) the live virtual display `id` runs at,
/// for a stream that joins it. None: no virtual display is live on Linux.
pub fn joinable_mode(_id: &str) -> Option<(u32, u32, u32)> {
    None
}

/// A virtual display this process made. None can be made on Linux yet.
#[non_exhaustive]
pub struct VirtualDisplay {
    pub name: String,
    /// The system left this display off and the host switched it on.
    pub switched_on: bool,
}

impl VirtualDisplay {
    /// Make the virtual display `stable_id` at this mode. Fails on Linux.
    pub fn create(_stable_id: &str, _width: u32, _height: u32, _fps: u32) -> Result<Self> {
        Err(crate::unsupported("A virtual display"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_says_virtual_displays_are_not_available() {
        let status = virtual_display_status();
        assert_eq!(status["capable"], false);
        assert_eq!(status["ready"], false);
        assert_eq!(
            status["reason"],
            "A virtual display is not available on a Linux host yet"
        );
        assert!(!virtual_display_os() && !virtual_display_available());
        assert!(!virtual_display_installed() && !ensure_virtual_display_available());
        assert!(joinable_mode("id").is_none() && windows_build().is_none());
        assert!(VirtualDisplay::create("id", 640, 480, 30).is_err());
    }

    #[test]
    fn the_render_adapter_comes_from_the_settings() {
        let config =
            butterpollo_config::config::Config::parse("adapter_name=Radeon\nadapter_pnp_id=X")
                .unwrap();
        let chosen = configured_render_adapter_with(&config, |name, pnp_id| {
            Ok(Some(RenderAdapter {
                name: format!("{name}/{pnp_id}"),
                luid: (1, 0),
            }))
        });
        assert_eq!(chosen.map(|a| a.name).as_deref(), Some("Radeon/X"));
        let missing = configured_render_adapter_with(&config, |_, _| anyhow::bail!("gone"));
        assert_eq!(missing, None);
    }
}
