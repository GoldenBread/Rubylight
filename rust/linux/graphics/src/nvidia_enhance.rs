//! The NVIDIA component that RTX HDR and RTX Video enhancement run through, and the startup check of what the GPU and driver offer. On Linux the host can name an installed component, but nothing loads it: no NVIDIA GPU is reported and the check offers no enhancement.
use std::{
    any::Any,
    path::PathBuf,
    sync::{Arc, PoisonError, RwLock},
};

/// The installed component: its folder, the adapter's SHA-256 and its data folder.
#[derive(Clone, Debug)]
pub struct Component {
    pub folder: PathBuf,
    pub adapter_sha256: String,
    pub data: PathBuf,
    /// The folders the component is checked and loaded in, held open so that none
    /// of them can be renamed or deleted while it is the installed one.
    /// A hold is not part of what a component is.
    pub hold: Arc<dyn Any + Send + Sync>,
}

/// Two components are the same when they name the same files: the holds are
/// what keeps those files there, not what the component is.
impl PartialEq for Component {
    fn eq(&self, other: &Self) -> bool {
        self.folder == other.folder
            && self.adapter_sha256 == other.adapter_sha256
            && self.data == other.data
    }
}
impl Eq for Component {}

static COMPONENT: RwLock<Option<Component>> = RwLock::new(None);

/// Names the installed component, or none.
pub fn set_component(component: Option<Component>) {
    *COMPONENT.write().unwrap_or_else(PoisonError::into_inner) = component;
}

/// The installed component.
pub fn component() -> Option<Component> {
    COMPONENT
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// Whether a component is installed.
pub fn installed() -> bool {
    component().is_some()
}

/// What the GPU and its driver offer (`nvidia_enhance` bits), and why each missing
/// enhancement is missing. Nothing on Linux, with that reason for both.
pub fn probe() -> (u8, Vec<(&'static str, String)>) {
    let reason = |name: &str| format!("{:#}", crate::unsupported(name));
    (
        0,
        vec![
            ("rtx_hdr", reason("RTX HDR")),
            ("rtx_video", reason("RTX Video enhancement")),
        ],
    )
}

/// Whether this PC has an NVIDIA GPU the enhancements could run on. GPUs are not listed
/// on Linux yet, so none is reported.
pub fn has_nvidia_gpu() -> bool {
    false
}
