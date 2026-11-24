//! Virtual displays as the host sees them on Linux: whether they can be served, the status
//! the console shows, and the private displays the host makes.
//!
//! A private display is a connector of Vibepollo's `vibeshine_drm` kernel module, connected
//! through its root broker and arranged with `kscreen-doctor`; the submodules do each part.
//! Without the module or the broker, every availability check answers no and the status says
//! why.
//!
//! Ported from Vibepollo (Nonary/Vibepollo, GPL-3.0), src/platform/linux/private_display*.cpp,
//! and libvirtualdisplay (Nonary/libvirtualdisplay, MIT, Copyright (c) 2026 Chase Payne),
//! src/driver/linux_control_client.cpp.
use anyhow::Result;
use butterpollo_media_policy::display_policy::Arrangement;
use std::path::Path;
use std::sync::Arc;

mod broker;
mod kscreen;
mod pool;
mod private;
#[cfg(test)]
mod tests;

pub use private::PrivateRequest;

/// The Windows build number. None on Linux, where there is none to read.
pub fn windows_build() -> Option<u32> {
    None
}

/// Whether the module's pool and the broker can serve a private display now, or why not.
fn availability(drm_root: &Path, broker: &broker::Broker) -> Result<(), String> {
    if pool::scan(drm_root).is_empty() {
        return Err("Vibepollo's virtual display module is not loaded".into());
    }
    if broker.trust == broker::Trust::Root {
        broker::check_socket(&broker.socket).map_err(|e| format!("{e:#}"))?;
    } else if !broker.socket.exists() {
        return Err(format!(
            "the broker socket {} is missing",
            broker.socket.display()
        ));
    }
    let path = std::ffi::CString::new(std::os::unix::ffi::OsStrExt::as_bytes(
        broker.socket.as_os_str(),
    ))
    .map_err(|_| "the broker socket path is not usable".to_string())?;
    // SAFETY: `path` is a NUL-terminated string that lives through the call.
    if unsafe { libc::access(path.as_ptr(), libc::R_OK | libc::W_OK) } != 0 {
        let error = std::io::Error::last_os_error();
        return Err(if error.raw_os_error() == Some(libc::EACCES) {
            "this user is not in the vibeshine-vkms group".into()
        } else {
            format!("the broker socket cannot be used: {error}")
        });
    }
    Ok(())
}

fn system_availability() -> Result<(), String> {
    let manager = private::system();
    availability(&manager.drm_root, &manager.broker)
}

/// Whether this system can serve virtual displays at all: the module is loaded and this user
/// may use the broker.
pub fn virtual_display_os() -> bool {
    system_availability().is_ok()
}

/// Whether a virtual display can be made now.
pub fn virtual_display_available() -> bool {
    system_availability().is_ok()
}

/// Whether the virtual display backend counts as installed: the module and the broker.
pub fn virtual_display_installed() -> bool {
    system_availability().is_ok()
}

/// [`virtual_display_available`] for a launch, which may repair a failed driver first. There
/// is nothing to repair on Linux.
pub fn ensure_virtual_display_available() -> bool {
    system_availability().is_ok()
}

fn status_for(result: Result<(), String>) -> serde_json::Value {
    serde_json::json!({
        "capable": result.is_ok(),
        "ready": result.is_ok(),
        "reason": result.err().unwrap_or_default(),
        "protocol": "rubylight",
    })
}

/// The virtual display's state for the console and diagnostics: capable and ready, or the
/// reason it is not.
pub fn virtual_display_status() -> serde_json::Value {
    status_for(system_availability())
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
/// for a stream that joins it.
pub fn joinable_mode(id: &str) -> Option<(u32, u32, u32)> {
    private::system().joinable_mode(id)
}

/// Put the desktop back after a host that died with a private display on, and disconnect the
/// connectors it left connected. Does nothing when no restore is pending.
pub fn recover_after_restart() -> Result<()> {
    private::system().recover()
}

/// A private display this process made. Dropping it restores the desktop (when it is the last
/// one) and gives the connector back.
#[non_exhaustive]
pub struct VirtualDisplay {
    /// The connector, such as `Virtual-1`.
    pub name: String,
    /// The system left this display off and the host switched it on.
    pub switched_on: bool,
    manager: Arc<private::Manager>,
    stable_id: String,
}

impl VirtualDisplay {
    fn live(manager: Arc<private::Manager>, stable_id: String, name: String) -> Self {
        Self {
            name,
            switched_on: true,
            manager,
            stable_id,
        }
    }

    /// Make the virtual display `stable_id` at this mode, alone on the desktop.
    pub fn create(stable_id: &str, width: u32, height: u32, fps: u32) -> Result<Self> {
        Self::create_with(&PrivateRequest {
            stable_id: stable_id.to_string(),
            width,
            height,
            refresh_millihz: fps.saturating_mul(1000),
            arrangement: Arrangement::Exclusive,
            scale_percent: None,
        })
    }

    /// Make the private display `request` describes.
    pub fn create_with(request: &PrivateRequest) -> Result<Self> {
        private::system().create(request)
    }
}

impl Drop for VirtualDisplay {
    fn drop(&mut self) {
        self.manager.release(&self.stable_id, &self.name);
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;

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
