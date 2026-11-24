//! The driver settings panel (Radeon Control Center support): the settings model, the journal, write and restore logic and the worker thread, which run on Linux against any backend. Not available yet: a backend for a real driver. No vendor settings API (AMD's ADLX, NVIDIA's NVAPI) exists on a Linux host, so the host's worker lists no GPU and changes nothing.
//!
//! `model` holds the settings and their rules, `control` the journal with writes and restores,
//! `worker` the one thread that owns them in a running host, `helper` the helper-process entry
//! point and `owner` the journal's lock and owner.
mod absent;
pub mod control;
pub mod helper;
pub mod model;
pub mod owner;
pub mod worker;

pub use control::{
    BackendError, Change, Controller, Entry, ErrorKind, GpuInfo, Journal, RadeonBackend,
    RestoreReport, Scope, WriteError,
};
pub use model::{Kind, Setting, State, Writable};

/// The file name of the NVIDIA settings journal in the host's directory.
const NVIDIA_JOURNAL_FILE: &str = "nvidia-journal.json";

/// Whose driver settings the panel controls: the maker of the GPU the host streams from. The
/// controller, journal and worker are the same for both; only the backend differs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Driver {
    #[default]
    Radeon,
    Nvidia,
}

impl Driver {
    /// The stream's GPU decides. Unknown (no display, several GPUs and none chosen), a Radeon
    /// present keeps the Radeon panel, else an NVIDIA GPU gets the NVIDIA one.
    pub fn for_stream(stream: Option<u32>, vendors: &[u32]) -> Self {
        use butterpollo_media_policy::encoder_gpu::{is_amd, is_nvidia};
        match stream {
            Some(vendor) if is_nvidia(vendor) => Self::Nvidia,
            Some(vendor) if is_amd(vendor) => Self::Radeon,
            _ if !vendors.iter().any(|v| is_amd(*v)) && vendors.iter().any(|v| is_nvidia(*v)) => {
                Self::Nvidia
            }
            _ => Self::Radeon,
        }
    }

    /// The name the panel's texts give the driver.
    pub fn name(self) -> &'static str {
        match self {
            Self::Radeon => "Radeon",
            Self::Nvidia => "NVIDIA",
        }
    }

    /// The journal file in the host's directory.
    pub fn journal_file(self) -> &'static str {
        match self {
            Self::Radeon => control::JOURNAL_FILE,
            Self::Nvidia => NVIDIA_JOURNAL_FILE,
        }
    }
}
