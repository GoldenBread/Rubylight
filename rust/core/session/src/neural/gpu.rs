//! GPU capability policy, AMD RDNA / NVIDIA Turing eligibility, Visual C++
//! runtime verification, and probe payload parsing for Neural Rendering.
use serde::{Deserialize, Serialize};

/// A GPU as the AMD driver's compute runtime reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gpu {
    pub name: String,
    /// `gfx1100` and so on.
    pub arch: String,
    pub compute_units: u32,
    /// The adapter's LUID (low part, then high part), when the runtime says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub luid: Option<u64>,
}

/// What `--neural-probe` prints.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Probe {
    pub gpus: Vec<Gpu>,
    pub error: Option<String>,
}

/// Extract the GPU list from a compute-runtime probe reply.
pub fn probe_gpus(probe: Probe) -> Result<Vec<Gpu>, String> {
    match probe.error {
        Some(error) if probe.gpus.is_empty() => Err(error),
        _ => Ok(probe.gpus),
    }
}

/// A hardware GPU as DXGI names it: the one the host captures and encodes
/// with, or another in the PC.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adapter {
    pub name: String,
    /// Its LUID: the low part, then the high part.
    pub luid: u64,
    /// Its PCI vendor and device ID.
    pub vendor: u32,
    pub device: u32,
}

pub const AMD: u32 = 0x1002;
pub const NVIDIA: u32 = 0x10de;
/// The first NVIDIA device ID of the Turing generation (GeForce GTX 16 and
/// RTX 20), the oldest the provider runs on; every later one is newer.
pub const MIN_NVIDIA_DEVICE: u32 = 0x1e00;
/// The Visual C++ 2015-2022 runtime (x64) OptiScaler links: without it the
/// game starts without OptiScaler and so without frame generation.
pub const VC_RUNTIME: [&str; 4] = [
    "MSVCP140.dll",
    "MSVCP140_ATOMIC_WAIT.dll",
    "VCRUNTIME140.dll",
    "VCRUNTIME140_1.dll",
];

/// AMD device IDs of RDNA graphics: the release's `path_table.json` ranges
/// (discrete Radeon RX 5000 to 9000, Steam Deck) and the integrated RDNA 2,
/// 3 and 3.5 graphics it does not list (Rembrandt, Raphael, Mendocino,
/// Phoenix, Hawk Point, Strix Point, Strix Halo, Krackan Point). Used only
/// when the compute runtime can't say.
const RDNA: [(u32, u32); 19] = [
    (0x7310, 0x731f),
    (0x7340, 0x734f),
    (0x7360, 0x736f),
    (0x73a0, 0x73ff),
    (0x7420, 0x743f),
    (0x7440, 0x74ff),
    (0x7550, 0x759f),
    (0x163f, 0x163f),
    (0x1435, 0x1435),
    (0x1681, 0x1681),
    (0x164e, 0x164e),
    (0x1506, 0x1506),
    (0x15bf, 0x15bf),
    (0x15c8, 0x15c8),
    (0x1900, 0x1901),
    (0x150e, 0x150e),
    (0x1586, 0x1586),
    (0x1114, 0x1114),
    (0x73f0, 0x73f0),
];

/// The GPU frame generation runs on, as [`capable`] found it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capable {
    pub gpu: String,
    /// The GPU the host streams with, when it is not that one.
    pub streams: Option<String>,
}
impl Capable {
    /// What the switch says when frame generation runs on a GPU the host
    /// does not stream with: the host can't know which GPU a game renders on.
    pub fn note(&self) -> Option<String> {
        self.streams.as_ref().map(|streams| {
            format!(
                "Frame generation runs when the game renders on {}; Rubylight streams with {streams}.",
                self.gpu
            )
        })
    }
}

/// Whether devices are offered frame generation at all: any of the PC's
/// hardware GPUs (`adapters`) is AMD's or NVIDIA's, by vendor alone.
pub fn offered(adapters: &[Adapter]) -> bool {
    adapters
        .iter()
        .any(|adapter| matches!(adapter.vendor, AMD | NVIDIA))
}

/// Whether [`capable`] needs the AMD compute runtime's GPUs: an AMD GPU is
/// among them, and the GPU the host streams with is no NVIDIA one that runs
/// it already.
pub fn needs_probe(streams: &Adapter, adapters: &[Adapter]) -> bool {
    !(streams.vendor == NVIDIA && streams.device >= MIN_NVIDIA_DEVICE)
        && std::iter::once(streams)
            .chain(adapters)
            .any(|adapter| adapter.vendor == AMD)
}

/// What the provider runs on, as its release says: AMD RDNA (Radeon RX 5000
/// and newer, and RDNA integrated graphics) and NVIDIA Turing and newer
/// (GeForce GTX 16, RTX 20 and newer). The GPU the host streams with
/// (`streams`) counts first; else any other of the PC's hardware GPUs
/// (`adapters`, AMD or NVIDIA), since a game may render on a GPU the host
/// does not stream with, as on a laptop whose display runs on its
/// integrated graphics. An AMD GPU is judged by the compute runtime's
/// architecture (`gpus`, matched by LUID, else by a name no other GPU has),
/// else by its PCI device ID; one neither identifies is refused.
pub fn capable(
    streams: &Adapter,
    adapters: &[Adapter],
    gpus: Result<&[Gpu], &str>,
) -> Result<Capable, String> {
    let supported = SUPPORTED;
    let first = verdict(streams, gpus);
    if first == Verdict::Runs {
        return Ok(Capable {
            gpu: streams.name.clone(),
            streams: None,
        });
    }
    let others: Vec<(&Adapter, Verdict)> = adapters
        .iter()
        .filter(|adapter| adapter.luid != streams.luid && matches!(adapter.vendor, AMD | NVIDIA))
        .map(|adapter| (adapter, verdict(adapter, gpus)))
        .collect();
    if let Some((adapter, _)) = others.iter().find(|(_, verdict)| *verdict == Verdict::Runs) {
        return Ok(Capable {
            gpu: adapter.name.clone(),
            streams: Some(streams.name.clone()),
        });
    }
    let name = &streams.name;
    let mut reason = match first {
        Verdict::Older(None) => format!("{supported}; this PC streams with {name}, which is older"),
        Verdict::Older(Some(arch)) => {
            format!("{supported}; this PC streams with {name} ({arch}), which is older")
        }
        Verdict::Unknown(why) => format!(
            "Rubylight can't identify the GPU it streams with ({name}, device {:#06x}) as one frame generation runs on: {why}",
            streams.device
        ),
        Verdict::Other | Verdict::Runs => format!("{supported}; this PC streams with {name}"),
    };
    reason.push_str(&judged(&others, "also has"));
    Err(reason)
}

/// The GPUs frame generation runs on, as a refusal says them.
const SUPPORTED: &str = "frame generation runs on AMD Radeon RX 5000 and newer graphics (RDNA) and NVIDIA GeForce GTX 16, RTX 20 and newer";

/// [`capable`] when the GPU the host streams with can't be found (`unknown`
/// says why): any of the PC's AMD or NVIDIA GPUs that runs it, with no note,
/// since the host can't tell whether it streams with that one.
pub fn capable_any(
    adapters: &[Adapter],
    gpus: Result<&[Gpu], &str>,
    unknown: &str,
) -> Result<Capable, String> {
    let all: Vec<(&Adapter, Verdict)> = adapters
        .iter()
        .filter(|adapter| matches!(adapter.vendor, AMD | NVIDIA))
        .map(|adapter| (adapter, verdict(adapter, gpus)))
        .collect();
    if let Some((adapter, _)) = all.iter().find(|(_, verdict)| *verdict == Verdict::Runs) {
        return Ok(Capable {
            gpu: adapter.name.clone(),
            streams: None,
        });
    }
    Err(format!("{SUPPORTED}; {unknown}{}", judged(&all, "has")))
}

/// What a refusal says of each GPU in `judged` that doesn't run it: "; it
/// `has` <name>, which is older".
fn judged(judged: &[(&Adapter, Verdict)], has: &str) -> String {
    judged
        .iter()
        .map(|(adapter, verdict)| {
            let name = &adapter.name;
            match verdict {
                Verdict::Older(None) => format!("; it {has} {name}, which is older"),
                Verdict::Older(Some(arch)) => format!("; it {has} {name} ({arch}), which is older"),
                Verdict::Unknown(why) => format!(
                    "; it {has} {name} (device {:#06x}), which Rubylight can't identify: {why}",
                    adapter.device
                ),
                Verdict::Other | Verdict::Runs => String::new(),
            }
        })
        .collect()
}

/// The Visual C++ runtime OptiScaler needs, of which this PC lacks
/// `missing`.
pub fn runtime(missing: &[&str]) -> Result<(), String> {
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "frame generation needs the Microsoft Visual C++ 2015-2022 Redistributable (x64), which this PC lacks ({})",
            missing.join(", ")
        ))
    }
}

/// Whether frame generation runs on one GPU ([`capable`]).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Verdict {
    Runs,
    /// Older than it runs on; an AMD GPU's architecture when the runtime said.
    Older(Option<String>),
    /// An AMD GPU neither the runtime nor its device ID identifies, and why.
    Unknown(String),
    /// Neither AMD's nor NVIDIA's.
    Other,
}

fn verdict(adapter: &Adapter, gpus: Result<&[Gpu], &str>) -> Verdict {
    match adapter.vendor {
        NVIDIA if adapter.device >= MIN_NVIDIA_DEVICE => Verdict::Runs,
        NVIDIA => Verdict::Older(None),
        AMD => {
            let found = gpus.ok().and_then(|gpus| {
                gpus.iter()
                    .find(|gpu| gpu.luid == Some(adapter.luid))
                    .or_else(|| {
                        let mut named = gpus.iter().filter(|gpu| gpu.name == adapter.name);
                        match (named.next(), named.next()) {
                            (Some(gpu), None) => Some(gpu),
                            _ => None,
                        }
                    })
            });
            if let Some(gpu) = found {
                let arch = gpu.arch.to_ascii_lowercase();
                let number = arch.strip_prefix("gfx").unwrap_or("");
                let rdna =
                    number.len() >= 4 && number[..2].parse::<u32>().is_ok_and(|major| major >= 10);
                return if rdna {
                    Verdict::Runs
                } else {
                    Verdict::Older(Some(gpu.arch.clone()))
                };
            }
            if RDNA
                .iter()
                .any(|(first, last)| (*first..=*last).contains(&adapter.device))
            {
                return Verdict::Runs;
            }
            Verdict::Unknown(match gpus {
                Err(error) => format!("the AMD compute runtime can't say ({error})"),
                Ok(_) => "the AMD compute runtime doesn't list it".into(),
            })
        }
        _ => Verdict::Other,
    }
}
