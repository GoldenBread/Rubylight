//! GPU capability probing, DXGI adapter lookup, and Visual C++ runtime
//! inspection for Neural Rendering.
use super::{PROBE_ANSWER, PROBE_FLAG, PROBE_RETRY, Probe, process::run_child, stall};
use crate::state::Shared;
use butterpollo_session::neural;
use std::{
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// The settings that pick the GPU a stream uses: the display, and the
/// Graphics card setting's name and PnP identity.
pub(super) type GpuSetting = [String; 3];

/// The GPU this host streams with, or why it can't be found, and every
/// hardware GPU of the PC ([`gpus_with`]).
type StreamsAmong = (Result<neural::Adapter, String>, Vec<neural::Adapter>);

/// `butterpollo.exe --neural-probe`: the GPUs the AMD compute runtime
/// finds, as one line.
pub fn probe_main() -> i32 {
    let probe = match butterpollo_win_io::hip::gpus() {
        Ok(gpus) => Probe { gpus, error: None },
        Err(error) => Probe {
            gpus: Vec::new(),
            error: Some(format!("{error:#}")),
        },
    };
    let mut out = std::io::stdout().lock();
    let line = serde_json::to_string(&probe).unwrap_or_default();
    match writeln!(out, "{PROBE_ANSWER}{line}").and_then(|()| out.flush()) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

/// The GPU frame generation runs on, or why this PC cannot have it: the
/// GPU a stream uses, else another of the PC's ([`neural::capable`]), an
/// AMD one asked about in a helper process; then the Visual C++ runtime,
/// beside the game's program (`game`, its folder) or in System32, looked for
/// at every call, so an install shows at once. The GPU is asked once, again
/// a while after a failure, and again once the GPU or display a stream uses
/// is set otherwise. Blocks.
pub fn capability(h: &Shared, game: Option<&Path>) -> Result<neural::Capable, String> {
    capability_of(h, game).map_err(|(_, reason)| reason)
}

/// [`capability`], with which half failed: `gpu` or `runtime`.
fn capability_of(
    h: &Shared,
    game: Option<&Path>,
) -> Result<neural::Capable, (&'static str, String)> {
    let capable = gpu_capability(h, false).map_err(|reason| ("gpu", reason))?;
    neural::runtime(&vc_missing(h, game)).map_err(|reason| ("runtime", reason))?;
    Ok(capable)
}

/// [`capability`]'s GPU half, held. An answer made without the GPU that
/// streams is asked again after a while, like a failure, so its note comes
/// once that GPU is found. With `serve`, a read's, the answer held for these
/// settings is given while that asks again in the background; the first
/// answer, and one after the settings changed, is waited for.
pub(super) fn gpu_capability(h: &Shared, serve: bool) -> Result<neural::Capable, String> {
    let setting = gpu_setting(h);
    {
        let mut state = h.neural.lock();
        if let Some((at, probed, result, whole)) = &state.probe
            && *probed == setting
        {
            let result = result.clone();
            if (result.is_ok() && *whole) || at.elapsed() < PROBE_RETRY {
                return result;
            }
            if serve {
                if !std::mem::replace(&mut state.probing, true) {
                    let h = h.clone();
                    std::thread::spawn(move || {
                        /// Ends the background check however it ends.
                        struct Ending<'a>(&'a Shared);
                        impl Drop for Ending<'_> {
                            fn drop(&mut self) {
                                self.0.neural.lock().probing = false;
                            }
                        }
                        let _ending = Ending(&h);
                        let _ = probe_gpu(&h, setting);
                    });
                }
                return result;
            }
        }
    }
    probe_gpu(h, setting)
}

/// [`gpu_capability`]'s check itself, held for `setting` unless the
/// settings changed meanwhile. Blocks.
fn probe_gpu(h: &Shared, setting: GpuSetting) -> Result<neural::Capable, String> {
    let mut whole = true;
    let result = gpus_with(h, &setting).and_then(|(streams, adapters)| {
        whole = streams.is_ok();
        // Only an AMD GPU needs the compute runtime, which no other driver has.
        let needs_probe = match &streams {
            Ok(streams) => neural::needs_probe(streams, &adapters),
            Err(_) => adapters.iter().any(|adapter| adapter.vendor == neural::AMD),
        };
        let probe = if needs_probe {
            run_probe(h).and_then(neural::probe_gpus)
        } else {
            Err("not asked: no AMD GPU".into())
        };
        let gpus = probe.as_deref().map_err(String::as_str);
        match &streams {
            Ok(streams) => neural::capable(streams, &adapters, gpus),
            // The PC's other GPUs still count, with no note on the one that streams.
            Err(unknown) => neural::capable_any(&adapters, gpus, unknown),
        }
    });
    match &result {
        Err(reason) => tracing::info!(%reason, "Neural Rendering is not available on this PC"),
        Ok(capable) => {
            if let Some(streams) = &capable.streams {
                tracing::info!(gpu = %capable.gpu, %streams, "Neural Rendering runs on a GPU Rubylight does not stream with");
            }
        }
    }
    if gpu_setting(h) == setting {
        h.neural.lock().probe = Some((Instant::now(), setting, result.clone(), whole));
    }
    result
}

/// Whether devices are offered frame generation at all: one of the PC's
/// GPUs is AMD's or NVIDIA's, by its DXGI vendor ID alone, with no probe
/// ([`neural::offered`]). GPUs that can't be listed keep it offered, and
/// the switch says why. Asked again once the GPU or display a stream uses
/// is set otherwise. Blocks.
pub fn offered(h: &Shared) -> bool {
    // Frame generation loads into a Windows game's graphics API; a Linux host has none.
    if crate::extensions::on_linux() {
        return false;
    }
    let setting = gpu_setting(h);
    if let Some((held, offered)) = &h.neural.lock().offered
        && *held == setting
    {
        return *offered;
    }
    match gpus_with(h, &setting) {
        Ok((_, adapters)) => {
            let offered = neural::offered(&adapters);
            h.neural.lock().offered = Some((setting, offered));
            offered
        }
        // Offered while the GPUs are not known, and asked again next time.
        Err(_) => true,
    }
}

fn gpu_setting(h: &Shared) -> GpuSetting {
    let config = h.config.read().unwrap();
    ["output_name", "adapter_name", "adapter_pnp_id"].map(|key| config.get(key, "").to_string())
}

/// The Visual C++ runtime files OptiScaler needs that this PC lacks: in
/// System32, and beside the game's program (`game`, its folder), where the
/// DLL OptiScaler loads as finds them first. Blocks on the game's folder.
pub fn vc_missing(h: &Shared, game: Option<&Path>) -> Vec<&'static str> {
    let system = system_missing(h);
    if system.is_empty() {
        return system;
    }
    let beside = game.map(runtime_beside).unwrap_or_default();
    without(system, &beside)
}

/// `missing` less the files in `found`.
pub fn without(missing: Vec<&'static str>, found: &[&'static str]) -> Vec<&'static str> {
    neural::without_runtime(missing, found)
}

/// The Visual C++ runtime files OptiScaler needs that System32 lacks.
#[cfg_attr(not(any(test, feature = "test-support")), allow(unused_variables))]
pub(super) fn system_missing(h: &Shared) -> Vec<&'static str> {
    #[cfg(any(test, feature = "test-support"))]
    if h.neural
        .fake_hardware
        .load(std::sync::atomic::Ordering::Acquire)
    {
        return h.neural.vc_missing.lock().unwrap().clone();
    }
    let system = std::env::var_os("SystemRoot")
        .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from)
        .join("System32");
    without(neural::VC_RUNTIME.to_vec(), &runtime_in(&system))
}

/// The Visual C++ runtime files OptiScaler needs that are in `folder`.
fn runtime_in(folder: &Path) -> Vec<&'static str> {
    neural::VC_RUNTIME
        .into_iter()
        .filter(|file| folder.join(file).is_file())
        .collect()
}

/// [`runtime_in`] the game's folder, which may be on a share that stalls. Blocks.
pub(super) fn runtime_beside(folder: &Path) -> Vec<&'static str> {
    stall::stalls(folder);
    runtime_in(folder)
}

/// The GPU this host captures and encodes with (the Graphics card
/// setting's, or the one that drives the display it streams), or why it
/// can't be found, and every hardware GPU of the PC, that one among them.
#[cfg_attr(not(any(test, feature = "test-support")), allow(unused_variables))]
fn gpus_with(h: &Shared, setting: &GpuSetting) -> Result<StreamsAmong, String> {
    #[cfg(any(test, feature = "test-support"))]
    {
        if *h.neural.streams_unknown.lock().unwrap() {
            let adapters = h
                .neural
                .adapters
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_default();
            return Ok((
                Err("the GPU Rubylight streams with can't be found: in this test".into()),
                adapters,
            ));
        }
        if let Some(streams) = h.neural.streams.lock().unwrap().clone() {
            let adapters = h
                .neural
                .adapters
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| vec![streams.clone()]);
            return Ok((Ok(streams), adapters));
        }
        if h.neural.probe.lock().unwrap().is_some() {
            let probe = run_probe(h)?;
            let gpu = probe.gpus.first().ok_or("no GPU in this test")?;
            let streams = neural::Adapter {
                name: gpu.name.clone(),
                luid: gpu.luid.unwrap_or_default(),
                vendor: neural::AMD,
                device: 0,
            };
            let adapters = h
                .neural
                .adapters
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| vec![streams.clone()]);
            return Ok((Ok(streams), adapters));
        }
        if let Some(adapters) = h.neural.adapters.lock().unwrap().clone() {
            let streams = adapters
                .first()
                .cloned()
                .ok_or_else(|| "no GPU in this test".to_string());
            return Ok((streams, adapters));
        }
        if h.neural
            .fake_hardware
            .load(std::sync::atomic::Ordering::Acquire)
        {
            let probe = run_probe(h)?;
            let gpu = probe.gpus.first().ok_or("no GPU in this test")?;
            let streams = neural::Adapter {
                name: gpu.name.clone(),
                luid: gpu.luid.unwrap_or_default(),
                vendor: neural::AMD,
                device: 0,
            };
            return Ok((Ok(streams.clone()), vec![streams]));
        }
    }
    let [display, adapter_name, pnp_id] = setting;
    let stream_gpu = butterpollo_win_graphics::capture::stream_gpu(display, adapter_name, pnp_id)
        .map_err(|e| format!("the GPU Rubylight streams with can't be found: {e:#}"));
    let gpus = butterpollo_win_graphics::capture::gpus().map_err(|e| {
        let listed = format!("the GPUs can't be listed: {e:#}");
        match &stream_gpu {
            Err(unknown) => format!("{unknown}; {listed}"),
            Ok(_) => listed,
        }
    })?;
    let luid = |(low, high): (u32, i32)| (u64::from(high as u32) << 32) | u64::from(low);
    let streams = stream_gpu.and_then(|(name, (low, high))| {
        let gpu = gpus
            .iter()
            .find(|gpu| gpu.luid == (low, high))
            .ok_or_else(|| {
                format!("the GPU Rubylight streams with ({name}) can't be identified")
            })?;
        Ok(neural::Adapter {
            name,
            luid: luid((low, high)),
            vendor: gpu.vendor,
            device: gpu.device,
        })
    });
    let adapters = gpus
        .iter()
        .map(|gpu| neural::Adapter {
            name: gpu.name.clone(),
            luid: luid(gpu.luid),
            vendor: gpu.vendor,
            device: gpu.device,
        })
        .collect();
    Ok((streams, adapters))
}

#[cfg_attr(not(any(test, feature = "test-support")), allow(unused_variables))]
fn run_probe(h: &Shared) -> Result<Probe, String> {
    #[cfg(any(test, feature = "test-support"))]
    {
        let gate = h.neural.probe_gate.lock().unwrap().take();
        if let Some(gate) = gate {
            let _ = gate.recv();
        }
        if let Some(probe) = h.neural.probe.lock().unwrap().clone() {
            return Ok(probe);
        }
        if h.neural
            .fake_hardware
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err("no probe in this test".into());
        }
    }
    let mut answer = None;
    run_child(PROBE_FLAG, "", Duration::from_secs(60), |line| {
        if let Some(json) = line.strip_prefix(PROBE_ANSWER) {
            answer = serde_json::from_str::<Probe>(json).ok();
        }
    })
    .map_err(|e| format!("the GPU check failed: {e:#}"))?;
    answer.ok_or_else(|| "the GPU check gave no answer".into())
}
