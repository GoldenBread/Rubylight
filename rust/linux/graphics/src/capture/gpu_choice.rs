//! Which GPU a stream and a virtual display use: the one the Graphics card setting names, or
//! the one Automatic pins; the console's note on an automatic GPU switch, and the foreground
//! window, as the Windows capture names them.
use super::Gpu;
use anyhow::{Result, bail};
use butterpollo_media_policy::encoder_gpu;

/// The GPU the Graphics card setting names, matched as capture and the
/// encoders match it: by the stored PnP identity when there is one, otherwise
/// by name. None on Automatic.
pub fn select_gpu<'a>(
    gpus: &'a [Gpu],
    adapter_name: &str,
    pnp_id: &str,
) -> Result<Option<&'a Gpu>> {
    if adapter_name.is_empty() && pnp_id.is_empty() {
        return Ok(None);
    }
    let mut matches = gpus.iter().filter(|gpu| {
        if pnp_id.is_empty() {
            gpu.name == adapter_name
        } else {
            gpu.pnp_id
                .as_deref()
                .is_some_and(|id| id.eq_ignore_ascii_case(pnp_id))
        }
    });
    let gpu = matches.next();
    if gpu.is_none() || (!pnp_id.is_empty() && matches.next().is_some()) {
        bail!("configured GPU adapter was not resolved uniquely: {adapter_name} {pnp_id}");
    }
    Ok(gpu)
}

/// The GPU choice a stream that encodes on the capture device opens it with.
/// A chosen GPU stays. With none chosen on a PC that mixes vendors it is the
/// AMD or NVIDIA GPU with the most video memory, unless the display is already
/// on one (`display_vendor`).
pub(super) fn pinned_choice(
    gpus: &[Gpu],
    display_vendor: Option<u32>,
    adapter_name: &str,
    pnp_id: &str,
) -> (String, String) {
    if adapter_name.is_empty() && pnp_id.is_empty() {
        let adapters: Vec<encoder_gpu::Ranked> = gpus
            .iter()
            .map(|gpu| encoder_gpu::Ranked {
                vendor: gpu.vendor,
                dedicated_memory: gpu.dedicated_memory,
                preference: gpu.preference,
            })
            .collect();
        if let Some(gpu) = encoder_gpu::pinned(&adapters, display_vendor).and_then(|i| gpus.get(i))
        {
            return (gpu.name.clone(), gpu.pnp_id.clone().unwrap_or_default());
        }
    }
    (adapter_name.to_string(), pnp_id.to_string())
}

/// The console's words for a GPU chosen under Graphics card that is not listed now.
pub fn missing_gpu_text(adapter_name: &str, pnp_id: &str) -> String {
    let named = if adapter_name.is_empty() {
        pnp_id
    } else {
        adapter_name
    };
    format!(
        "The GPU chosen under Graphics card ({named}) is not available: the system lists no such GPU now, as when it is unplugged or its driver is reloading. Rubylight does not switch to another GPU on its own: reconnect it, or choose another GPU or Automatic under Graphics card, then reconnect."
    )
}

/// The GPU a virtual display renders on: the chosen one, else the one Automatic pins on a PC
/// with several GPUs; None leaves it to the system. A chosen GPU that is missing is an error
/// that says so.
pub fn render_gpu<'a>(
    gpus: &'a [Gpu],
    adapter_name: &str,
    pnp_id: &str,
) -> Result<Option<&'a Gpu>> {
    let (name, pnp) = pinned_choice(gpus, None, adapter_name, pnp_id);
    if name.is_empty() && pnp.is_empty() {
        return Ok(None);
    }
    match select_gpu(gpus, &name, &pnp) {
        Ok(gpu) => Ok(gpu),
        Err(error) => {
            tracing::debug!(error = %format!("{error:#}"), "the GPU for the virtual display");
            bail!("{}", missing_gpu_text(adapter_name, pnp_id))
        }
    }
}

/// The GPU Automatic pins streams to although the system lists another first. Linux lists
/// GPUs in no preference order, so a pin is never such a switch.
pub fn automatic_switch() -> Option<String> {
    None
}

/// The foreground window of the user's desktop. No window is listed on Linux yet
/// ([`super::app_windows`]), so none is in front.
pub fn foreground_window() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gpu(name: &str, vendor: u32, memory: u64) -> Gpu {
        Gpu {
            name: name.into(),
            vendor,
            dedicated_memory: memory,
            device: 0,
            luid: (128, 0),
            pnp_id: Some(format!("PCI\\VEN_{vendor:04X}")),
            preference: None,
        }
    }

    #[test]
    fn a_chosen_gpu_renders_and_a_missing_one_says_so() {
        let gpus = [gpu("Radeon", 0x1002, 16 << 30)];
        assert_eq!(render_gpu(&gpus, "", "").unwrap().map(|g| &*g.name), None);
        let chosen = render_gpu(&gpus, "Radeon", "").unwrap().unwrap();
        assert_eq!(chosen.name, "Radeon");
        let Err(missing) = render_gpu(&gpus, "GeForce", "") else {
            panic!("a GPU that is not listed renders");
        };
        let missing = missing.to_string();
        assert!(missing.contains("(GeForce) is not available"), "{missing}");
        assert_eq!(automatic_switch(), None);
        assert_eq!(foreground_window(), None);
    }
}
