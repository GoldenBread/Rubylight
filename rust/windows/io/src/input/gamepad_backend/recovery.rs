//! Bounded PnP inspection and recovery for Rubylight's virtual gamepad driver
//! (`Root\RubylightVirtualGamepad`). A transport error alone never authorizes
//! restarting a device: PnP must identify our exact stopped, failed devnode.

use anyhow::{Result, bail};
use std::{
    mem::size_of,
    sync::{Mutex, mpsc},
    time::{Duration, Instant},
};
use windows::{
    Win32::{Devices::DeviceAndDriverInstallation::*, Foundation::ERROR_NO_MORE_ITEMS},
    core::w,
};

const COOLDOWN: Duration = Duration::from_secs(60);
const PROBE_INTERVAL: Duration = Duration::from_secs(5);
const SETTLE_WINDOW: Duration = Duration::from_secs(5);
const MAX_ATTEMPTS: u8 = 3;
const HARDWARE_ID: &str = rubylight_vgamepad_proto::protocol::ROOT_HARDWARE_ID;
/// `hidvhf.inf`'s `vhfservice.NT.Services` installs `vhf` as the function
/// driver, with `WUDFRd` as the upper filter for `rubylight_vgamepad.dll`.
const SERVICE: &str = "vhf";

/// What Windows Plug and Play currently reports for `Root\RubylightVirtualGamepad`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DevnodeState {
    Missing,
    Started,
    Disabled(u32),
    RestartRequired,
    Failed(u32),
    OtherProblem(u32),
    Unknown,
}

/// Bounded recovery worker state for the virtual gamepad devnode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecoveryState {
    Idle,
    InProgress,
    RestartRequired,
    Exhausted,
}

#[derive(Default)]
struct Budget {
    attempts: u8,
    last_attempt: Option<Instant>,
    restart_required: bool,
}
impl Budget {
    fn available(&self, now: Instant) -> bool {
        !self.restart_required
            && self.attempts < MAX_ATTEMPTS
            && self
                .last_attempt
                .is_none_or(|last| now.duration_since(last) >= COOLDOWN)
    }
    fn attempted(&mut self, now: Instant, restart_required: bool) {
        self.attempts += 1;
        self.last_attempt = Some(now);
        self.restart_required |= restart_required;
    }
    fn healthy(&mut self, now: Instant) {
        self.restart_required = false;
        // A brief reopening followed by failure must not grant an endless restart loop.
        if self
            .last_attempt
            .is_some_and(|last| now.duration_since(last) >= COOLDOWN)
        {
            *self = Self::default();
        }
    }
    fn state(&self, active: bool) -> RecoveryState {
        if active {
            RecoveryState::InProgress
        } else if self.restart_required {
            RecoveryState::RestartRequired
        } else if self.attempts >= MAX_ATTEMPTS {
            RecoveryState::Exhausted
        } else {
            RecoveryState::Idle
        }
    }
}

struct Recovery {
    budget: Budget,
    probe_after: Option<Instant>,
    settle_until: Option<Instant>,
    worker: Option<mpsc::Receiver<Outcome>>,
}
impl Recovery {
    fn poll_worker(&mut self, now: Instant) {
        let Some(worker) = &self.worker else {
            return;
        };
        match worker.try_recv() {
            Ok(outcome) => {
                self.worker = None;
                if outcome.attempted {
                    self.budget.attempted(now, outcome.restart_required);
                } else if outcome.restart_required {
                    self.budget.restart_required = true;
                }
                match outcome.result {
                    Ok(()) if outcome.attempted => {
                        self.settle_until = Some(now + SETTLE_WINDOW);
                        tracing::info!(
                            "requested recovery of failed Rubylight virtual gamepad driver"
                        );
                    }
                    Err(error) => {
                        self.settle_until = None;
                        tracing::warn!(
                            %error,
                            "Rubylight virtual gamepad driver recovery did not complete"
                        );
                    }
                    _ => {}
                }
                self.probe_after = Some(now + PROBE_INTERVAL);
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.worker = None;
                self.settle_until = None;
                self.budget.attempted(now, false);
                tracing::warn!(
                    "Rubylight virtual gamepad driver recovery worker stopped unexpectedly"
                );
            }
        }
    }
    fn active(&self, now: Instant) -> bool {
        self.worker.is_some() || self.settle_until.is_some_and(|until| now < until)
    }
}
struct Outcome {
    attempted: bool,
    restart_required: bool,
    result: Result<()>,
}

static RECOVERY: Mutex<Recovery> = Mutex::new(Recovery {
    budget: Budget {
        attempts: 0,
        last_attempt: None,
        restart_required: false,
    },
    probe_after: None,
    settle_until: None,
    worker: None,
});

/// Records a healthy open of the VHF gamepad driver. Zero PnP work.
pub(crate) fn healthy() {
    let mut recovery = RECOVERY.lock().unwrap();
    recovery.settle_until = None;
    recovery.budget.healthy(Instant::now());
}

/// Current bounded recovery state, draining any finished worker without spawning one.
pub(crate) fn state() -> RecoveryState {
    let mut recovery = RECOVERY.lock().unwrap();
    let now = Instant::now();
    recovery.poll_worker(now);
    let active = recovery.active(now);
    recovery.budget.state(active)
}

/// Whether a recovery worker is running or waiting briefly for the restarted interface.
pub(crate) fn in_progress() -> bool {
    state() == RecoveryState::InProgress
}

/// Requests bounded off-thread recovery of a stopped, failed virtual gamepad devnode.
/// Never blocks on native SetupAPI; at most one worker runs at a time.
pub(crate) fn request() -> RecoveryState {
    let mut recovery = RECOVERY.lock().unwrap();
    let now = Instant::now();
    recovery.poll_worker(now);
    if recovery.active(now) {
        return RecoveryState::InProgress;
    }
    if !recovery.budget.available(now) || recovery.probe_after.is_some_and(|next| now < next) {
        return recovery.budget.state(false);
    }
    let (sender, receiver) = mpsc::channel();
    match std::thread::Builder::new()
        .name("vgamepad-recovery".into())
        .spawn(move || {
            let mut attempted = false;
            let mut restart_required = false;
            let result = restart_failed_driver(&mut attempted, &mut restart_required);
            let _ = sender.send(Outcome {
                attempted,
                restart_required,
                result,
            });
        }) {
        Ok(_) => {
            recovery.worker = Some(receiver);
            RecoveryState::InProgress
        }
        Err(error) => {
            recovery.probe_after = Some(now + PROBE_INTERVAL);
            tracing::warn!(%error, "could not start virtual gamepad driver recovery worker");
            recovery.budget.state(false)
        }
    }
}

fn is_our_device(hardware_ids: &[String]) -> bool {
    hardware_ids
        .iter()
        .any(|id| id.eq_ignore_ascii_case(HARDWARE_ID))
}

fn classify(
    hardware_ids: &[String],
    status: CM_DEVNODE_STATUS_FLAGS,
    problem: CM_PROB,
    install_flags: u32,
) -> Option<DevnodeState> {
    if !is_our_device(hardware_ids) {
        return None;
    }
    if install_flags & (DI_NEEDREBOOT | DI_NEEDRESTART).0 != 0 {
        return Some(DevnodeState::RestartRequired);
    }
    if status.contains(DN_HAS_PROBLEM) {
        return Some(match problem {
            CM_PROB_DISABLED | CM_PROB_DISABLED_SERVICE => DevnodeState::Disabled(problem.0),
            CM_PROB_NEED_RESTART => DevnodeState::RestartRequired,
            CM_PROB_FAILED_START | CM_PROB_FAILED_ADD | CM_PROB_FAILED_POST_START
                if !status.contains(DN_STARTED) =>
            {
                DevnodeState::Failed(problem.0)
            }
            _ => DevnodeState::OtherProblem(problem.0),
        });
    }
    if status.contains(DN_STARTED) {
        Some(DevnodeState::Started)
    } else {
        Some(DevnodeState::Unknown)
    }
}

fn eligible(
    hardware_ids: &[String],
    service: &str,
    status: CM_DEVNODE_STATUS_FLAGS,
    problem: CM_PROB,
) -> bool {
    is_our_device(hardware_ids)
        && service.eq_ignore_ascii_case(SERVICE)
        && status.contains(DN_HAS_PROBLEM)
        && !status.contains(DN_STARTED)
        && matches!(
            problem,
            CM_PROB_FAILED_START | CM_PROB_FAILED_ADD | CM_PROB_FAILED_POST_START
        )
}

/// User-facing explanation for a VHF open failure using PnP and recovery state.
pub(crate) fn diagnosis(
    error: &anyhow::Error,
    recorded: Option<String>,
    devnode: DevnodeState,
    recovery: RecoveryState,
) -> String {
    const IGNORED: &str = "controller input is ignored while keyboard and mouse remain available";
    if let Some(recorded) = recorded.filter(|text| !text.trim().is_empty()) {
        return recorded;
    }
    if let DevnodeState::Disabled(code) = devnode {
        return format!(
            "Rubylight Virtual Gamepad is disabled in Windows (Code {code}; {error:#}); {IGNORED}. Re-enable Rubylight Virtual Gamepad under System devices in Device Manager to restore controllers."
        );
    }
    if devnode == DevnodeState::RestartRequired || recovery == RecoveryState::RestartRequired {
        return format!(
            "Rubylight Virtual Gamepad needs a Windows restart to recover ({error:#}); {IGNORED}. Restart Windows to restore virtual controllers."
        );
    }
    match (devnode, recovery) {
        (DevnodeState::Failed(code), RecoveryState::InProgress) => format!(
            "Rubylight Virtual Gamepad stopped (Code {code}; {error:#}); Rubylight is restarting it while keyboard and mouse remain available. Controllers will reconnect automatically once Windows restarts the driver."
        ),
        (DevnodeState::Failed(code), RecoveryState::Exhausted) => format!(
            "Rubylight Virtual Gamepad stopped (Code {code}; {error:#}) and did not recover after automatic restarts; {IGNORED}. Disable and re-enable Rubylight Virtual Gamepad under System devices in Device Manager, run Rubylight setup again, or restart Windows."
        ),
        (DevnodeState::Failed(code), RecoveryState::Idle) => format!(
            "Rubylight Virtual Gamepad stopped (Code {code}; {error:#}); {IGNORED}. Disable and re-enable Rubylight Virtual Gamepad under System devices in Device Manager, run Rubylight setup again, or restart Windows."
        ),
        (DevnodeState::OtherProblem(code), _) => format!(
            "Rubylight Virtual Gamepad reported a Windows device problem (Code {code}; {error:#}); {IGNORED}. Check Rubylight Virtual Gamepad under System devices in Device Manager or run Rubylight setup again."
        ),
        (_, RecoveryState::InProgress) => format!(
            "Virtual gamepad driver unavailable ({error:#}); Rubylight is restarting it while keyboard and mouse remain available."
        ),
        _ => format!(
            "Virtual gamepad driver unavailable ({error:#}); {IGNORED}. Run Rubylight setup again with the gamepad driver selected to restore it."
        ),
    }
}

struct DeviceSet(HDEVINFO);
impl Drop for DeviceSet {
    fn drop(&mut self) {
        // SAFETY: this value owns the SetupAPI set and closes it once.
        let _ = unsafe { SetupDiDestroyDeviceInfoList(self.0) };
    }
}

fn strings(
    set: HDEVINFO,
    device: &SP_DEVINFO_DATA,
    property: SETUP_DI_REGISTRY_PROPERTY,
) -> Result<Vec<String>> {
    let mut bytes = vec![0u8; 8192];
    let mut size = 0;
    // SAFETY: device belongs to the live set; the writable buffer has the passed size.
    unsafe {
        SetupDiGetDeviceRegistryPropertyW(
            set,
            device,
            property,
            None,
            Some(&mut bytes),
            Some(&mut size),
        )?;
    }
    anyhow::ensure!(
        size as usize <= bytes.len() && size.is_multiple_of(2),
        "invalid device identity property"
    );
    let words: Vec<u16> = bytes[..size as usize]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect();
    words
        .split(|c| *c == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf16(s).map_err(Into::into))
        .collect()
}

fn enumerate_root_candidates() -> Result<(DeviceSet, Vec<SP_DEVINFO_DATA>, bool)> {
    // ROOT-enumerated devices only: never enumerates physical buses (PCI/USB/HID)
    // and still finds our devnode even if its setup class changed on failure.
    // SAFETY: static enumerator literal and read-only enumeration flags.
    let set = DeviceSet(unsafe {
        SetupDiGetClassDevsW(None, w!("ROOT"), None, DIGCF_ALLCLASSES | DIGCF_PRESENT)?
    });
    let mut candidates = Vec::new();
    let mut complete = false;
    for index in 0..4096 {
        let mut device = SP_DEVINFO_DATA {
            cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
            ..Default::default()
        };
        // SAFETY: correctly sized output structure and a live device set.
        match unsafe { SetupDiEnumDeviceInfo(set.0, index, &mut device) } {
            Ok(()) => {
                if strings(set.0, &device, SPDRP_HARDWAREID).is_ok_and(|ids| is_our_device(&ids)) {
                    candidates.push(device);
                }
            }
            Err(error) if error.code() == ERROR_NO_MORE_ITEMS.to_hresult() => {
                complete = true;
                break;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok((set, candidates, complete))
}

/// Read-only inspection of `Root\RubylightVirtualGamepad` in Windows PnP.
/// Only called on VHF open errors, never on the healthy input path.
pub(crate) fn inspect() -> DevnodeState {
    let Ok((set, candidates, complete)) = enumerate_root_candidates() else {
        return DevnodeState::Unknown;
    };
    if !complete {
        return DevnodeState::Unknown;
    }
    if candidates.is_empty() {
        return DevnodeState::Missing;
    }
    if candidates.len() != 1 {
        return DevnodeState::Unknown;
    }
    let device = &candidates[0];
    let Ok(ids) = strings(set.0, device, SPDRP_HARDWAREID) else {
        return DevnodeState::Unknown;
    };
    let mut status = CM_DEVNODE_STATUS_FLAGS::default();
    let mut problem = CM_PROB::default();
    // SAFETY: both outputs are live and DevInst came from this present device set.
    let read = unsafe { CM_Get_DevNode_Status(&mut status, &mut problem, device.DevInst, 0) };
    if read != CR_SUCCESS {
        return DevnodeState::Unknown;
    }
    let mut install = SP_DEVINSTALL_PARAMS_W {
        cbSize: size_of::<SP_DEVINSTALL_PARAMS_W>() as u32,
        ..Default::default()
    };
    // SAFETY: device and correctly sized output belong to the live set.
    let flags = match unsafe { SetupDiGetDeviceInstallParamsW(set.0, Some(device), &mut install) } {
        Ok(()) => install.Flags.0,
        Err(_) => 0,
    };
    classify(&ids, status, problem, flags).unwrap_or(DevnodeState::Unknown)
}

fn read_failed(
    set: HDEVINFO,
    device: &SP_DEVINFO_DATA,
    restart_required: &mut bool,
) -> Result<bool> {
    let ids = strings(set, device, SPDRP_HARDWAREID)?;
    if !is_our_device(&ids) {
        return Ok(false);
    }
    let service = strings(set, device, SPDRP_SERVICE)?
        .into_iter()
        .next()
        .unwrap_or_default();
    let mut status = CM_DEVNODE_STATUS_FLAGS::default();
    let mut problem = CM_PROB::default();
    // SAFETY: both outputs are live and DevInst came from this present device set.
    let result = unsafe { CM_Get_DevNode_Status(&mut status, &mut problem, device.DevInst, 0) };
    anyhow::ensure!(
        result == CR_SUCCESS,
        "cannot inspect Rubylight virtual gamepad driver state ({})",
        result.0
    );
    if status.contains(DN_HAS_PROBLEM) && problem == CM_PROB_NEED_RESTART {
        *restart_required = true;
        bail!("Rubylight virtual gamepad driver requires a Windows restart");
    }
    Ok(eligible(&ids, &service, status, problem))
}

fn restart_failed_driver(attempted: &mut bool, restart_required: &mut bool) -> Result<()> {
    let (set, candidates, complete) = enumerate_root_candidates()?;
    // Never choose arbitrarily between multiple installations.
    if !complete || candidates.len() != 1 {
        return Ok(());
    }
    let device = &candidates[0];
    if !read_failed(set.0, device, restart_required)? {
        return Ok(());
    }
    let mut install = SP_DEVINSTALL_PARAMS_W {
        cbSize: size_of::<SP_DEVINSTALL_PARAMS_W>() as u32,
        ..Default::default()
    };
    // SAFETY: device and correctly sized output belong to the live set.
    unsafe {
        SetupDiGetDeviceInstallParamsW(set.0, Some(device), &mut install)?;
    }
    if install.Flags.0 & (DI_DONOTCALLCONFIGMG | DI_NEEDREBOOT | DI_NEEDRESTART).0 != 0 {
        *restart_required = true;
        bail!("Rubylight virtual gamepad driver requires setup or a Windows restart");
    }
    // Re-read immediately before the mutation: it may have recovered while enumerating.
    if !read_failed(set.0, device, restart_required)? {
        return Ok(());
    }
    let change = SP_PROPCHANGE_PARAMS {
        ClassInstallHeader: SP_CLASSINSTALL_HEADER {
            cbSize: size_of::<SP_CLASSINSTALL_HEADER>() as u32,
            InstallFunction: DIF_PROPERTYCHANGE,
        },
        StateChange: DICS_PROPCHANGE,
        Scope: DICS_FLAG_CONFIGSPECIFIC,
        HwProfile: 0,
    };
    *attempted = true;
    // SAFETY: parameters describe a restart of this one verified failed devnode. No
    // disable/enable, device installation, physical hardware operation or reboot is requested.
    unsafe {
        SetupDiSetClassInstallParamsW(
            set.0,
            Some(device),
            Some(&change.ClassInstallHeader),
            size_of::<SP_PROPCHANGE_PARAMS>() as u32,
        )?;
        SetupDiCallClassInstaller(DIF_PROPERTYCHANGE, set.0, Some(device))?;
        SetupDiGetDeviceInstallParamsW(set.0, Some(device), &mut install)?;
    }
    if install.Flags.0 & (DI_NEEDREBOOT | DI_NEEDRESTART).0 != 0 {
        *restart_required = true;
        bail!("Windows deferred virtual gamepad driver recovery until restart");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_requires_our_stopped_failed_vhf_devnode() {
        let own = vec![HARDWARE_ID.to_owned()];
        for problem in [
            CM_PROB_FAILED_START,
            CM_PROB_FAILED_ADD,
            CM_PROB_FAILED_POST_START,
        ] {
            assert!(eligible(&own, "vhf", DN_HAS_PROBLEM, problem));
            assert!(eligible(&own, "VHF", DN_HAS_PROBLEM, problem));
            assert!(!eligible(&own, "vhf", DN_HAS_PROBLEM | DN_STARTED, problem));
            assert!(!eligible(
                &own,
                "vhf",
                CM_DEVNODE_STATUS_FLAGS::default(),
                problem
            ));
            assert!(!eligible(
                &[r"Root\OtherDriver".into()],
                "vhf",
                DN_HAS_PROBLEM,
                problem
            ));
            assert!(!eligible(&own, "WUDFRd", DN_HAS_PROBLEM, problem));
            assert!(!eligible(&own, "other", DN_HAS_PROBLEM, problem));
        }
        for problem in [
            CM_PROB::default(),
            CM_PROB_DISABLED,
            CM_PROB_DISABLED_SERVICE,
            CM_PROB_NEED_RESTART,
            CM_PROB_FAILED_INSTALL,
        ] {
            assert!(!eligible(&own, "vhf", DN_HAS_PROBLEM, problem));
        }
    }

    #[test]
    fn classify_distinguishes_started_disabled_failed_and_restart_required() {
        let own = vec![HARDWARE_ID.to_owned()];
        let none = 0;
        assert_eq!(
            classify(
                &[r"ROOT\SYSTEM".into()],
                DN_STARTED,
                CM_PROB::default(),
                none
            ),
            None
        );
        assert_eq!(
            classify(&own, DN_STARTED, CM_PROB::default(), none),
            Some(DevnodeState::Started)
        );
        assert_eq!(
            classify(&own, DN_HAS_PROBLEM, CM_PROB_DISABLED, none),
            Some(DevnodeState::Disabled(CM_PROB_DISABLED.0))
        );
        assert_eq!(
            classify(&own, DN_HAS_PROBLEM, CM_PROB_DISABLED_SERVICE, none),
            Some(DevnodeState::Disabled(CM_PROB_DISABLED_SERVICE.0))
        );
        assert_eq!(
            classify(&own, DN_HAS_PROBLEM, CM_PROB_NEED_RESTART, none),
            Some(DevnodeState::RestartRequired)
        );
        assert_eq!(
            classify(
                &own,
                DN_HAS_PROBLEM,
                CM_PROB_FAILED_POST_START,
                DI_NEEDREBOOT.0
            ),
            Some(DevnodeState::RestartRequired)
        );
        assert_eq!(
            classify(&own, DN_HAS_PROBLEM, CM_PROB_FAILED_POST_START, none),
            Some(DevnodeState::Failed(CM_PROB_FAILED_POST_START.0))
        );
        assert_eq!(
            classify(
                &own,
                DN_HAS_PROBLEM | DN_STARTED,
                CM_PROB_FAILED_POST_START,
                none
            ),
            Some(DevnodeState::OtherProblem(CM_PROB_FAILED_POST_START.0))
        );
        assert_eq!(
            classify(&own, DN_HAS_PROBLEM, CM_PROB_FAILED_INSTALL, none),
            Some(DevnodeState::OtherProblem(CM_PROB_FAILED_INSTALL.0))
        );
    }

    #[test]
    fn restarts_respect_cooldown_attempt_cap_and_restart_required() {
        let mut budget = Budget::default();
        let start = Instant::now();
        for attempt in 0..MAX_ATTEMPTS {
            let now = start + COOLDOWN * u32::from(attempt);
            assert!(budget.available(now));
            budget.attempted(now, false);
            budget.healthy(now + Duration::from_secs(1));
            assert!(!budget.available(now + Duration::from_secs(1)));
        }
        assert_eq!(budget.state(false), RecoveryState::Exhausted);
        assert!(!budget.available(start + COOLDOWN * 10));
        budget.healthy(start + COOLDOWN * 10);
        assert_eq!(budget.state(false), RecoveryState::Idle);
        assert!(budget.available(start + COOLDOWN * 10));

        // A Windows-deferred restart stops further automated retries until healthy again.
        budget.attempted(start + COOLDOWN * 10, true);
        assert_eq!(budget.state(false), RecoveryState::RestartRequired);
        assert!(!budget.available(start + COOLDOWN * 20));
        budget.healthy(start + COOLDOWN * 20);
        assert_eq!(budget.state(false), RecoveryState::Idle);
        assert!(budget.available(start + COOLDOWN * 20));
    }

    #[test]
    fn diagnosis_reports_each_device_and_recovery_state_clearly() {
        let err = anyhow::anyhow!("Not data available (0x80070103)");

        assert_eq!(
            diagnosis(
                &err,
                Some("Smart App Control is on".into()),
                DevnodeState::Failed(43),
                RecoveryState::InProgress
            ),
            "Smart App Control is on"
        );

        let disabled = diagnosis(&err, None, DevnodeState::Disabled(22), RecoveryState::Idle);
        assert!(
            disabled.contains("disabled in Windows (Code 22"),
            "{disabled}"
        );
        assert!(
            disabled.contains("Re-enable Rubylight Virtual Gamepad"),
            "{disabled}"
        );

        let restart = diagnosis(
            &err,
            None,
            DevnodeState::Failed(43),
            RecoveryState::RestartRequired,
        );
        assert!(restart.contains("needs a Windows restart"), "{restart}");

        let recovering = diagnosis(
            &err,
            None,
            DevnodeState::Failed(43),
            RecoveryState::InProgress,
        );
        assert!(recovering.contains("Code 43"), "{recovering}");
        assert!(
            recovering.contains("Rubylight is restarting it"),
            "{recovering}"
        );

        let exhausted = diagnosis(
            &err,
            None,
            DevnodeState::Failed(43),
            RecoveryState::Exhausted,
        );
        assert!(exhausted.contains("Code 43"), "{exhausted}");
        assert!(
            exhausted.contains("did not recover after automatic restarts"),
            "{exhausted}"
        );

        let missing = diagnosis(&err, None, DevnodeState::Missing, RecoveryState::Idle);
        assert!(missing.contains("0x80070103"), "{missing}");
        assert!(missing.contains("Rubylight setup"), "{missing}");
    }
}
