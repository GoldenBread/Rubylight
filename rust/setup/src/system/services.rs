//! Windows service management helpers for Rubylight setup.
use super::{registry_string, wide};
use crate::{host_service, log::line};
use anyhow::{Context, Result, bail, ensure};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SERVICE_MARKED_FOR_DELETE},
        System::{Registry::HKEY_LOCAL_MACHINE, Services::*},
    },
    core::{PCWSTR, PWSTR},
};

pub(super) struct ServiceHandle(pub(super) SC_HANDLE);
impl Drop for ServiceHandle {
    fn drop(&mut self) {
        // SAFETY: the handle came from OpenSCManagerW or OpenServiceW and is closed only here.
        unsafe {
            let _ = CloseServiceHandle(self.0);
        }
    }
}
fn manager() -> Result<ServiceHandle> {
    // SAFETY: null machine and database names select the local service manager.
    unsafe {
        Ok(ServiceHandle(OpenSCManagerW(
            PCWSTR::null(),
            PCWSTR::null(),
            SC_MANAGER_ALL_ACCESS,
        )?))
    }
}
pub(super) fn open_service(name: &str) -> Option<ServiceHandle> {
    let manager = manager().ok()?;
    let name = wide(name);
    // SAFETY: `manager` is a live handle and `name` is NUL-terminated UTF-16 that outlives the call.
    unsafe {
        OpenServiceW(manager.0, PCWSTR(name.as_ptr()), SERVICE_ALL_ACCESS)
            .ok()
            .map(ServiceHandle)
    }
}
/// The service's program path (without quotes or arguments), if installed.
pub fn service_program(name: &str) -> Option<PathBuf> {
    let image = registry_string(
        HKEY_LOCAL_MACHINE,
        &host_service::service_key(name),
        host_service::IMAGE_PATH,
    )?;
    Some(host_service::image_program(&image))
}
fn service_state(service: &ServiceHandle) -> Option<SERVICE_STATUS_CURRENT_STATE> {
    let mut status = SERVICE_STATUS::default();
    // SAFETY: `service` is a live handle and `status` is a valid out-pointer.
    unsafe { QueryServiceStatus(service.0, &mut status) }
        .ok()
        .map(|_| status.dwCurrentState)
}
/// Whether the service is installed and running or starting.
pub fn service_running(name: &str) -> bool {
    open_service(name)
        .and_then(|service| service_state(&service))
        .is_some_and(|state| state != SERVICE_STOPPED && state != SERVICE_STOP_PENDING)
}
/// Stop the service if it is running and wait for it (35 s, then fail).
pub fn stop_service(name: &str) -> Result<()> {
    let Some(service) = open_service(name) else {
        return Ok(());
    };
    if service_state(&service) == Some(SERVICE_STOPPED) {
        return Ok(());
    }
    line(format!("stopping service {name}"));
    let mut status = SERVICE_STATUS::default();
    // SAFETY: `service` is a live handle and `status` is a valid out-pointer.
    unsafe {
        let _ = ControlService(service.0, SERVICE_CONTROL_STOP, &mut status);
    }
    let deadline = Instant::now() + Duration::from_secs(35);
    while Instant::now() < deadline {
        if service_state(&service) == Some(SERVICE_STOPPED) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    bail!("the {name} service did not stop")
}
/// Wait until the service has stopped; false if it is still running at
/// `timeout`.
pub fn wait_stopped(name: &str, timeout: Duration) -> bool {
    let Some(service) = open_service(name) else {
        return true;
    };
    let deadline = Instant::now() + timeout;
    loop {
        if service_state(&service) == Some(SERVICE_STOPPED) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}
pub fn start_service(name: &str) -> Result<()> {
    let service = open_service(name).with_context(|| format!("the {name} service is missing"))?;
    line(format!("starting service {name}"));
    // SAFETY: `service` is a live handle; no start arguments are passed.
    unsafe { StartServiceW(service.0, None) }.with_context(|| format!("starting {name}"))
}
pub fn delete_service(name: &str) -> Result<()> {
    let Some(service) = open_service(name) else {
        return Ok(());
    };
    line(format!("deleting service {name}"));
    // SAFETY: `service` is a live handle.
    unsafe { DeleteService(service.0) }.with_context(|| format!("deleting {name}"))
}
/// What setup does about a service's startup type and about starting it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartupPlan {
    /// The startup type to give the service; `SERVICE_NO_CHANGE` keeps the
    /// current one (and with it a delayed start).
    pub start_type: SERVICE_START_TYPE,
    /// Whether setup starts the service once it is installed.
    pub start: bool,
}
/// A new service starts automatically. An existing one keeps what the user
/// set; a disabled one is not started, because Windows refuses to.
pub fn startup_plan(existing: Option<SERVICE_START_TYPE>) -> StartupPlan {
    match existing {
        None => StartupPlan {
            start_type: SERVICE_AUTO_START,
            start: true,
        },
        Some(start_type) => StartupPlan {
            start_type: SERVICE_START_TYPE(SERVICE_NO_CHANGE),
            start: start_type != SERVICE_DISABLED,
        },
    }
}
pub(super) fn service_start_type(service: &ServiceHandle) -> Result<SERVICE_START_TYPE> {
    let mut needed = 0u32;
    // SAFETY: `service` is a live handle; a null buffer asks only for the size.
    if let Err(error) = unsafe { QueryServiceConfigW(service.0, None, 0, &mut needed) } {
        // Any other error is returned as it is: install_service matches on it.
        if error.code() != ERROR_INSUFFICIENT_BUFFER.to_hresult() {
            return Err(error.into());
        }
    }
    ensure!(needed > 0, "the service configuration could not be read");
    // u64 elements keep the buffer aligned for QUERY_SERVICE_CONFIGW.
    let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
    let config = buffer.as_mut_ptr().cast::<QUERY_SERVICE_CONFIGW>();
    // SAFETY: `config` points at `needed` writable, aligned bytes; Windows fills them.
    unsafe {
        QueryServiceConfigW(service.0, Some(config), needed, &mut needed)
            .context("reading the service configuration")?;
        Ok((*config).dwStartType)
    }
}
/// Start the service unless it is disabled, which setup leaves as the user
/// set it. Returns whether it was started.
pub fn start_service_unless_disabled(name: &str) -> Result<bool> {
    let service = open_service(name).with_context(|| format!("the {name} service is missing"))?;
    if !startup_plan(Some(service_start_type(&service)?)).start {
        line(format!(
            "service {name} is disabled and stays disabled, so it was not started; set its startup type to Manual or Automatic in Services, then start it"
        ));
        return Ok(false);
    }
    drop(service);
    start_service(name)?;
    Ok(true)
}
/// Create the service, or point an existing one with this name at `program`.
/// Runs as LocalSystem and restarts after failures. A new service starts
/// automatically; an existing one keeps its startup type.
pub fn install_service(name: &str, display: &str, description: &str, program: &Path) -> Result<()> {
    // Apollo's and Sunshine's uninstallers delete this service. While
    // something still holds it open, Windows only marks it for deletion and
    // refuses to create or change one with its name until it is gone.
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match install_service_once(name, display, description, program) {
            Err(error)
                if Instant::now() < deadline
                    && error
                        .downcast_ref::<windows::core::Error>()
                        .is_some_and(|e| {
                            e.code() == ERROR_SERVICE_MARKED_FOR_DELETE.to_hresult()
                        }) =>
            {
                line(format!("service {name} is still being deleted; waiting"));
                std::thread::sleep(Duration::from_secs(1));
            }
            result => return result,
        }
    }
}
/// Renames an installed service as Windows Services shows it, changing
/// nothing else about it.
pub fn set_service_display_name(name: &str, display: &str) -> Result<()> {
    let manager = manager()?;
    let wide_name = wide(name);
    let wide_display = wide(display);
    // SAFETY: `manager` is a live handle and both strings are NUL-terminated UTF-16 that outlive the calls.
    unsafe {
        let service = ServiceHandle(OpenServiceW(
            manager.0,
            PCWSTR(wide_name.as_ptr()),
            SERVICE_CHANGE_CONFIG,
        )?);
        ChangeServiceConfigW(
            service.0,
            ENUM_SERVICE_TYPE(SERVICE_NO_CHANGE),
            SERVICE_START_TYPE(SERVICE_NO_CHANGE),
            SERVICE_ERROR(SERVICE_NO_CHANGE),
            PCWSTR::null(),
            PCWSTR::null(),
            None,
            PCWSTR::null(),
            PCWSTR::null(),
            PCWSTR::null(),
            PCWSTR(wide_display.as_ptr()),
        )?;
    }
    Ok(())
}
fn install_service_once(
    name: &str,
    display: &str,
    description: &str,
    program: &Path,
) -> Result<()> {
    let manager = manager()?;
    let binary = wide(&format!("\"{}\"", program.display()));
    let wide_name = wide(name);
    let wide_display = wide(display);
    // SAFETY: `manager` is a live handle and every string argument is NUL-terminated UTF-16 that outlives the calls.
    let service = unsafe {
        match OpenServiceW(manager.0, PCWSTR(wide_name.as_ptr()), SERVICE_ALL_ACCESS) {
            Ok(existing) => {
                line(format!("reconfiguring service {name}"));
                let existing = ServiceHandle(existing);
                // An update keeps the startup type the user chose (Manual,
                // Disabled, delayed start).
                let start_type = startup_plan(Some(service_start_type(&existing)?)).start_type;
                ChangeServiceConfigW(
                    existing.0,
                    SERVICE_WIN32_OWN_PROCESS,
                    start_type,
                    SERVICE_ERROR_NORMAL,
                    PCWSTR(binary.as_ptr()),
                    PCWSTR::null(),
                    None,
                    PCWSTR::null(),
                    windows::core::w!("LocalSystem"),
                    PCWSTR::null(),
                    PCWSTR(wide_display.as_ptr()),
                )?;
                existing
            }
            Err(_) => {
                line(format!("creating service {name}"));
                ServiceHandle(CreateServiceW(
                    manager.0,
                    PCWSTR(wide_name.as_ptr()),
                    PCWSTR(wide_display.as_ptr()),
                    SERVICE_ALL_ACCESS,
                    SERVICE_WIN32_OWN_PROCESS,
                    startup_plan(None).start_type,
                    SERVICE_ERROR_NORMAL,
                    PCWSTR(binary.as_ptr()),
                    PCWSTR::null(),
                    None,
                    PCWSTR::null(),
                    PCWSTR::null(),
                    PCWSTR::null(),
                )?)
            }
        }
    };
    let mut text = wide(description);
    let description = SERVICE_DESCRIPTIONW {
        lpDescription: PWSTR(text.as_mut_ptr()),
    };
    let mut actions = [
        SC_ACTION {
            Type: SC_ACTION_RESTART,
            Delay: 5_000,
        },
        SC_ACTION {
            Type: SC_ACTION_RESTART,
            Delay: 5_000,
        },
        SC_ACTION {
            Type: SC_ACTION_RESTART,
            Delay: 30_000,
        },
    ];
    let failure = SERVICE_FAILURE_ACTIONSW {
        dwResetPeriod: 86_400,
        lpRebootMsg: PWSTR::null(),
        lpCommand: PWSTR::null(),
        cActions: actions.len() as u32,
        lpsaActions: actions.as_mut_ptr(),
    };
    // SAFETY: `service` is a live handle and each info structure, with the strings and actions it points at, outlives its call.
    unsafe {
        ChangeServiceConfig2W(
            service.0,
            SERVICE_CONFIG_DESCRIPTION,
            Some((&description as *const SERVICE_DESCRIPTIONW).cast()),
        )?;
        ChangeServiceConfig2W(
            service.0,
            SERVICE_CONFIG_FAILURE_ACTIONS,
            Some((&failure as *const SERVICE_FAILURE_ACTIONSW).cast()),
        )?;
    }
    Ok(())
}
