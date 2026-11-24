//! Windows per-app audio policy factory, WMI process-start watch, and recovery journal.
use super::{NoSoundYet, sessions::sound_pids};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    ffi::c_void,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use windows::{
    Win32::{Media::Audio::*, System::WinRT::RoGetActivationFactory},
    core::{GUID, HRESULT, HSTRING, IInspectable, IInspectable_Vtbl, Interface},
};

/// Windows.Media.Internal.AudioPolicyConfig's factory (EarTrumpet's and
/// Nucleus Co-op's `IAudioPolicyConfigFactory`): 19 methods this does not
/// call, then the per-process endpoint ones.
#[repr(C)]
pub(super) struct FactoryVtbl {
    base: IInspectable_Vtbl,
    unused: [usize; 19],
    pub(super) set:
        unsafe extern "system" fn(*mut c_void, u32, EDataFlow, ERole, *mut c_void) -> HRESULT,
    pub(super) get:
        unsafe extern "system" fn(*mut c_void, u32, EDataFlow, ERole, *mut HSTRING) -> HRESULT,
    clear_all: usize,
}
/// The factory's interface from Windows 10 21H2 (build 21390) on.
#[repr(transparent)]
#[derive(Clone)]
struct Factory(IInspectable);
// SAFETY: FactoryVtbl mirrors the factory's slot layout (the offset test checks it) and Factory is
// a transparent IInspectable, so the IID names an interface with exactly this vtable.
unsafe impl Interface for Factory {
    type Vtable = FactoryVtbl;
    const IID: GUID = GUID::from_u128(0xab3d4648_e242_459f_b02f_541c70306324);
}
/// The same interface under its IID before 21H2.
#[repr(transparent)]
#[derive(Clone)]
struct EarlierFactory(IInspectable);
// SAFETY: as for Factory: the same vtable under the earlier IID.
unsafe impl Interface for EarlierFactory {
    type Vtable = FactoryVtbl;
    const IID: GUID = GUID::from_u128(0x2a59116d_6c4f_45e0_a74f_707e3fef9258);
}

/// Windows' answer `result` for process `pid`: E_INVALIDARG is
/// [`NoSoundYet`] when `sessions` (asked only then) lists no sound of it.
pub(super) fn refusal(
    result: HRESULT,
    pid: u32,
    sessions: impl FnOnce() -> Result<Vec<u32>>,
) -> Result<()> {
    if result != windows::Win32::Foundation::E_INVALIDARG {
        return Ok(result.ok()?);
    }
    match sessions() {
        Ok(pids) if !pids.contains(&pid) => Err(NoSoundYet.into()),
        Ok(_) => Err(windows::core::Error::from(result))
            .context("Windows refuses the game's audio device although the game opened its sound"),
        Err(error) => Err(windows::core::Error::from(result))
            .context(format!("its sound can't be listed: {error:#}")),
    }
}

/// The calls the routing needs, so the journal logic runs against a fake.
pub(super) trait Apps {
    /// The process's own playback device for `role`: "" when it follows
    /// the default.
    fn get(&self, pid: u32, role: ERole) -> Result<String>;
    /// "" puts the process back on the default.
    fn set(&self, pid: u32, role: ERole, device: &str) -> Result<()>;
    /// Running processes of the programs `exes` ([`same_program`]), with
    /// their program paths in lower case.
    fn running(&self, exes: &[&str]) -> Result<Vec<(u32, String)>>;
}

pub(super) struct Windows(Factory);
impl Windows {
    pub(super) fn new() -> Result<Self> {
        let class = HSTRING::from("Windows.Media.Internal.AudioPolicyConfig");
        // SAFETY: RoGetActivationFactory only reads the class name and returns an owned interface
        // or an error; COM is initialised by the caller.
        let factory = unsafe {
            RoGetActivationFactory::<Factory>(&class)
                .or_else(|_| RoGetActivationFactory::<EarlierFactory>(&class).map(|f| Factory(f.0)))
        }
        .context("Windows' per-app audio device settings are unavailable")?;
        Ok(Self(factory))
    }
}
impl Apps for Windows {
    fn get(&self, pid: u32, role: ERole) -> Result<String> {
        let mut device = HSTRING::new();
        // SAFETY: `self.0` is a live factory whose slots match FactoryVtbl; `device` receives an
        // owned HSTRING.
        let result =
            unsafe { (self.0.vtable().get)(self.0.as_raw(), pid, eRender, role, &mut device) };
        refusal(result, pid, sound_pids)?;
        Ok(device.to_string_lossy())
    }
    fn set(&self, pid: u32, role: ERole, device: &str) -> Result<()> {
        let device = HSTRING::from(device);
        // SAFETY: the HSTRING is borrowed for the call (an empty one is null, which clears) and
        // outlives it; the callee does not take ownership.
        let result = unsafe {
            let raw: *mut c_void = std::mem::transmute_copy(&device);
            (self.0.vtable().set)(self.0.as_raw(), pid, eRender, role, raw)
        };
        refusal(result, pid, sound_pids)
    }
    fn running(&self, exes: &[&str]) -> Result<Vec<(u32, String)>> {
        let names: Vec<&str> = exes.iter().map(|exe| file_name(exe)).collect();
        Ok(crate::process::processes()?
            .into_iter()
            .filter(|p| names.iter().any(|n| p.name.eq_ignore_ascii_case(n)))
            .filter_map(|p| Some((p.pid, crate::process::image_path(p.pid)?.to_lowercase())))
            .filter(|(_, path)| exes.iter().any(|exe| same_program(exe, path)))
            .collect())
    }
}

/// Processes as they start, so a pending restore runs before the program
/// is likely to open its sound; a fake in the tests.
pub(super) trait Starts {
    /// The next process to start within `timeout`, with its program path in
    /// lower case; `None` when none did.
    fn next(&mut self, timeout: Duration) -> Result<Option<(u32, String)>>;
}

/// [`Starts`] from WMI's process start trace, which needs an administrator,
/// as the host runs. On the thread that made it, which has COM initialised.
pub(super) struct WindowsStarts(crate::wmi::Events);
impl WindowsStarts {
    pub(super) fn subscribe() -> Result<Self> {
        let wmi = crate::wmi::Wmi::connect(r"root\cimv2")?;
        Ok(Self(wmi.events("SELECT * FROM Win32_ProcessStartTrace")?))
    }
}
impl Starts for WindowsStarts {
    fn next(&mut self, timeout: Duration) -> Result<Option<(u32, String)>> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let Some(event) = self.0.next(left)? else {
                return Ok(None);
            };
            // One that already ended has no device to put back.
            if let Some(pid) = crate::wmi::number(&event, "ProcessID")?
                && let Some(exe) = crate::process::image_path(pid)
            {
                return Ok(Some((pid, exe.to_lowercase())));
            }
        }
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}
/// A Store game's package family (`Name_PublisherId` from
/// `...\WindowsApps\Name_Version_Arch__PublisherId\...`) and the rest of
/// its path: an update changes the version in the folder name.
fn packaged(exe: &str) -> Option<(String, &str)> {
    let (_, inside) = exe.split_once(r"\windowsapps\")?;
    let (folder, rest) = inside.split_once('\\')?;
    let parts: Vec<&str> = folder.split('_').collect();
    let (name, publisher) = (parts.first()?, parts.last()?);
    (parts.len() >= 3).then(|| (format!("{name}_{publisher}"), rest))
}
/// Whether two lower-case program paths are one program: the same path,
/// or the same Store package's program in another version.
pub(super) fn same_program(a: &str, b: &str) -> bool {
    a == b || matches!((packaged(a), packaged(b)), (Some(a), Some(b)) if a == b)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct App {
    /// The program's path in lower case.
    pub(super) exe: String,
    /// Its own device for each of [`super::ROLES`] before the stream: "" for none.
    pub(super) before: [String; 2],
    pub(super) applied: String,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct Journal {
    pub(super) apps: Vec<App>,
}
pub(super) fn path(directory: &Path) -> PathBuf {
    directory.join("audio-app-recovery.json")
}
/// Where an unreadable journal is moved, its program list kept: a free
/// name, so an earlier one stays.
fn unreadable(directory: &Path) -> PathBuf {
    (1..)
        .map(|n| match n {
            1 => directory.join("audio-app-recovery.unreadable.json"),
            n => directory.join(format!("audio-app-recovery.unreadable-{n}.json")),
        })
        .find(|path| !path.exists())
        .expect("a free name")
}
pub(super) fn load(directory: &Path) -> Result<Journal> {
    let file = path(directory);
    match std::fs::read(&file) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(journal) => Ok(journal),
            // Never in the way of the next stream: moved aside, and the games
            // it lists are named for a reset by hand.
            Err(error) => {
                let aside = unreadable(directory);
                std::fs::rename(&file, &aside).with_context(|| {
                    format!(
                        "the record of the games' audio devices, {}, can't be read ({error}) or moved aside",
                        file.display()
                    )
                })?;
                tracing::warn!(
                    error = %error,
                    kept = %aside.display(),
                    "the record of the games' audio devices can't be read: the games it lists may still play on the virtual playback device; reset them in Settings > System > Sound > Volume mixer"
                );
                Ok(Journal::default())
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Journal::default()),
        Err(error) => Err(error).with_context(|| file.display().to_string()),
    }
}
pub(super) fn save(directory: &Path, journal: &Journal) -> Result<()> {
    butterpollo_session::state::write_json(&path(directory), journal)
}
