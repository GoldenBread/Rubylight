//! The NVIDIA component's NGX adapter (`butterpollo_truehdr.dll`, built from
//! `rust/truehdr-runtime` with NVIDIA's RTX Video SDK) and the startup check
//! of what the GPU and driver offer. The adapter is never beside the host: it
//! is loaded by full path from the component's folder, which the host names
//! with [`set_component`], after every file there was checked
//! ([`butterpollo_core::nvidia_component::verify`]).
use anyhow::{Context, Result, bail};
use butterpollo_core::nvidia_component::{self, ABI, ADAPTER};
use butterpollo_core::nvidia_enhance::{RTX_HDR, RTX_VIDEO, ngx_adapter};
use std::{
    ffi::c_void,
    os::windows::ffi::OsStrExt,
    path::PathBuf,
    sync::{PoisonError, RwLock},
};
use windows::{
    Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_UNKNOWN,
            Direct3D11::*,
            Dxgi::{CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1},
        },
    },
    core::Interface,
};

/// `LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32`: the
/// adapter's dependencies come from its own folder and System32 only.
const SEARCH: u32 = 0x0000_0100 | 0x0000_0800;

/// The installed component: its folder, the adapter's SHA-256 and NGX's data
/// folder.
#[derive(Clone, Debug)]
pub struct Component {
    pub folder: PathBuf,
    pub adapter_sha256: String,
    pub data: PathBuf,
    /// The folders the component is checked and loaded in, held open so that none
    /// of them can be renamed or deleted while it is the installed one
    /// ([`Hold`]). A hold is not part of what a component is.
    pub hold: Hold,
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

/// Folders held open without sharing deletion. Windows refuses to rename or
/// delete a folder whose open handle does not share deletion, so a folder held
/// this way stays at its path while the hold lives; holding every folder from
/// the component's up to the host's data root keeps the component's whole path
/// fixed.
#[derive(Clone, Debug, Default)]
pub struct Hold {
    /// Held for as long as this lives, never read: each handle keeps its
    /// folder from being renamed or deleted while it is open.
    _folders: std::sync::Arc<Vec<std::fs::File>>,
}

impl Hold {
    /// `folders`, each held open: from the component's folder up to the host's
    /// data root, since renaming any of them would leave the component's path
    /// naming something else. Never follows a link.
    pub fn new<'a>(
        folders: impl IntoIterator<Item = &'a std::path::Path>,
    ) -> std::io::Result<Self> {
        let held = folders
            .into_iter()
            .map(|folder| crate::process::open_unfollowed(folder, true))
            .collect::<std::io::Result<Vec<_>>>()?;
        Ok(Self {
            _folders: std::sync::Arc::new(held),
        })
    }
}

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

/// Checks the component's files, loads the adapter by full path, checks its
/// ABI and gives it NGX's data folder.
pub(crate) fn load() -> Result<libloading::Library> {
    let component = component().context(
        "the NVIDIA components are not installed; download them in the console's Settings, Picture, Integrations",
    )?;
    nvidia_component::verify(&component.folder, &component.adapter_sha256)?;
    std::fs::create_dir_all(&component.data)
        .with_context(|| format!("cannot create {}", component.data.display()))?;
    let path = component.folder.join(ADAPTER);
    let data: Vec<u16> = component
        .data
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: The file was just checked against the SHA-256 recorded when it
    // was installed into a folder only administrators may change, and the
    // component's folders are held open ([`Hold`]), so the path still names the
    // file that was checked; its ABI function takes nothing and returns a u32
    // and is checked before any other symbol is used, and ABI 3's configure
    // takes a NUL-terminated wide path.
    unsafe {
        let library: libloading::Library =
            libloading::os::windows::Library::load_with_flags(&path, SEARCH)
                .with_context(|| format!("the NVIDIA adapter {} is unavailable", path.display()))?
                .into();
        let abi = library.get::<unsafe extern "C" fn() -> u32>(b"butterpollo_truehdr_abi\0")?;
        let found = abi();
        if found != ABI {
            bail!(
                "the NVIDIA adapter speaks ABI {found}, this host {ABI}; download the NVIDIA components again"
            );
        }
        let configure = library
            .get::<unsafe extern "C" fn(*const u16) -> u32>(b"butterpollo_ngx_configure\0")?;
        if configure(data.as_ptr()) != 1 {
            bail!(
                "the NVIDIA adapter refused NGX's data folder {}",
                component.data.display()
            );
        }
        Ok(library)
    }
}

/// An NGX error code in words.
pub(crate) fn describe(code: u32) -> String {
    match code {
        // The adapter's own codes.
        0xbad1_0001 => "NGX returned no parameters".into(),
        0xbad1_0002 => "the picture's format or size is not supported".into(),
        0xbad1_0003 => "the GPU or driver does not offer it".into(),
        0xbad1_0004 => "NGX could not create the feature".into(),
        0xbad1_0005 => "the adapter was not told where NGX keeps its data".into(),
        0xbad1_0006 => "the NVIDIA driver is too old".into(),
        0xbad1_0007 => "the feature asks for scratch memory, which this host does not give".into(),
        0xbad1_0008 => {
            "the GPU cannot let NGX write the picture in this format (no typed UAV support)".into()
        }
        // NVSDK_NGX_Result_FAIL_* from the RTX Video SDK's nvsdk_ngx_defs.h.
        0xbad0_0001 => "the GPU does not support it".into(),
        0xbad0_0002 => "NGX reported a platform error; update the NVIDIA driver".into(),
        0xbad0_0004 => "its NVIDIA runtime DLL is missing from the NVIDIA components".into(),
        0xbad0_0005 => "NGX rejected a parameter".into(),
        0xbad0_0007 => "NGX is not initialized".into(),
        0xbad0_0008 => "the picture's format is not supported".into(),
        0xbad0_000b => "NGX could not initialize the feature".into(),
        0xbad0_000c => "the NVIDIA driver is too old".into(),
        0xbad0_000d => "the GPU is out of memory".into(),
        0xbad0_000f => "NGX cannot write to its data folder".into(),
        0xbad0_0011 => "NGX denied access to it".into(),
        code => format!("NGX error {code:#x}"),
    }
}

/// What the first NVIDIA GPU and its driver offer (`nvidia_enhance` bits),
/// and why each missing enhancement is missing. Takes a few hundred
/// milliseconds: run it once, off any stream.
pub fn probe() -> (u8, Vec<(&'static str, String)>) {
    let mut errors = Vec::new();
    let result = (|| -> Result<u8> {
        let library = load()?;
        let device = nvidia_device()?;
        // SAFETY: ABI 3's availability check takes a live D3D11 device, a feature number and two
        // writable u32s; `library` stays loaded for the call.
        let available = unsafe {
            *library.get::<unsafe extern "C" fn(*mut c_void, u32, *mut u32, *mut u32) -> u32>(
                b"butterpollo_ngx_available\0",
            )?
        };
        let mut offered = 0;
        for (bit, feature, name) in [(RTX_HDR, 0, "rtx_hdr"), (RTX_VIDEO, 1, "rtx_video")] {
            let (mut error, mut minimum) = (0, 0);
            // SAFETY: as above; `device` is live for the call.
            if unsafe { available(device.as_raw(), feature, &mut error, &mut minimum) } == 1 {
                offered |= bit;
            } else {
                errors.push((name, unavailable(error, minimum)));
            }
        }
        Ok(offered)
    })();
    match result {
        Ok(offered) => (offered, errors),
        Err(error) => (
            0,
            vec![
                ("rtx_hdr", format!("{error:#}")),
                ("rtx_video", format!("{error:#}")),
            ],
        ),
    }
}

/// Why a feature is unavailable, with the driver it needs when NGX said.
fn unavailable(error: u32, minimum: u32) -> String {
    let (major, minor) = (minimum >> 16, minimum & 0xffff);
    if minimum != 0 {
        format!("the NVIDIA driver is too old; it needs {major}.{minor:02} or newer")
    } else {
        describe(error)
    }
}

/// DXGI's adapters with their PCI vendors, in DXGI's order; an adapter whose
/// description cannot be read counts as no vendor's.
fn adapters() -> Result<Vec<(IDXGIAdapter1, u32)>> {
    // SAFETY: The factory and adapters are live COM objects returned by these calls, and
    // GetDesc1 returns an owned struct.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        Ok((0..32)
            .map_while(|index| factory.EnumAdapters1(index).ok())
            .map(|adapter| {
                let vendor = adapter.GetDesc1().map_or(0, |desc| desc.VendorId);
                (adapter, vendor)
            })
            .collect())
    }
}

/// Whether this PC has an NVIDIA GPU, from DXGI's adapter list alone (no
/// device is made).
pub fn has_nvidia_gpu() -> bool {
    adapters().is_ok_and(|adapters| {
        let vendors: Vec<u32> = adapters.iter().map(|(_, vendor)| *vendor).collect();
        ngx_adapter(&vendors).is_some()
    })
}

/// A D3D11 device on the adapter NVIDIA's check asks ([`ngx_adapter`]: the
/// first NVIDIA one, whichever GPU drives the display).
fn nvidia_device() -> Result<ID3D11Device> {
    let adapters = adapters()?;
    let vendors: Vec<u32> = adapters.iter().map(|(_, vendor)| *vendor).collect();
    let Some(index) = ngx_adapter(&vendors) else {
        bail!("this PC has no NVIDIA GPU");
    };
    let mut device = None;
    // SAFETY: The adapter is a live COM object; the out-parameter is a live local, unwrapped
    // only after D3D11CreateDevice succeeded.
    unsafe {
        D3D11CreateDevice(
            &adapters[index].0,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )?;
    }
    device.context("D3D11 returned no device")
}

#[cfg(test)]
mod tests {
    #[test]
    fn ngx_errors_read_as_words() {
        assert!(super::describe(0xbad0_0004).contains("DLL"));
        assert!(super::describe(0xbad0_000c).contains("driver"));
        assert!(super::describe(0xbad1_0003).contains("does not offer"));
        // The adapter's output texture without a typed UAV, which NGX must write.
        assert!(super::describe(0xbad1_0008).contains("UAV"));
        assert_eq!(super::describe(0x1234), "NGX error 0x1234");
        assert_eq!(
            super::unavailable(0xbad1_0006, (570 << 16) | 2),
            "the NVIDIA driver is too old; it needs 570.02 or newer"
        );
        assert!(super::unavailable(0xbad0_0004, 0).contains("NVIDIA components"));
    }
}
