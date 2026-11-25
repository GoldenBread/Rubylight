//! Local Win32 input-desktop, SetupAPI interface, registry, and token helpers for display control.
use anyhow::{Result, bail};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows::{
    Win32::{
        Devices::DeviceAndDriverInstallation::*,
        Foundation::*,
        Security::*,
        Storage::FileSystem::*,
        System::{Registry::*, StationsAndDesktops::*, Threading::*},
        UI::WindowsAndMessaging::*,
    },
    core::{GUID, PCWSTR},
};

struct InputDesktop {
    original: HDESK,
    current: HDESK,
}
impl Drop for InputDesktop {
    fn drop(&mut self) {
        // SAFETY: The original desktop handle is borrowed from Windows; the owned current handle is closed after detaching.
        unsafe {
            let _ = SetThreadDesktop(self.original);
            let _ = CloseDesktop(self.current);
        }
    }
}
thread_local! {
    static INPUT_DESKTOP: std::cell::RefCell<Option<InputDesktop>> = const { std::cell::RefCell::new(None) };
}

fn follow_input_desktop() -> bool {
    // SAFETY: The thread-local attachment owns every opened desktop handle.
    INPUT_DESKTOP.with_borrow_mut(|attachment| unsafe {
        let Ok(desktop) = OpenInputDesktop(
            DF_ALLOWOTHERACCOUNTHOOK,
            false,
            DESKTOP_ACCESS_FLAGS(GENERIC_ALL.0),
        ) else {
            return false;
        };
        let Ok(original) = GetThreadDesktop(GetCurrentThreadId()) else {
            let _ = CloseDesktop(desktop);
            return false;
        };
        if SetThreadDesktop(desktop).is_err() {
            let _ = CloseDesktop(desktop);
            return false;
        }
        if let Some(attached) = attachment {
            let previous = std::mem::replace(&mut attached.current, desktop);
            let _ = CloseDesktop(previous);
        } else {
            *attachment = Some(InputDesktop {
                original,
                current: desktop,
            });
        }
        true
    })
}

fn desktop_name(desktop: HDESK) -> Option<String> {
    let mut name = [0u16; 256];
    // SAFETY: The desktop handle is live and `name` holds its byte capacity.
    unsafe {
        GetUserObjectInformationW(
            HANDLE(desktop.0),
            UOI_NAME,
            Some(name.as_mut_ptr().cast()),
            size_of_val(&name) as u32,
            None,
        )
    }
    .ok()?;
    Some(crate::text::from_wide(&name))
}

fn input_desktop_name() -> Option<String> {
    // SAFETY: The opened desktop handle is closed once after reading its name.
    unsafe {
        let desktop =
            OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS).ok()?;
        let name = desktop_name(desktop);
        let _ = CloseDesktop(desktop);
        name
    }
}

fn thread_desktop_name() -> Option<String> {
    // SAFETY: The current thread ID is valid; Windows owns the returned desktop handle.
    unsafe { GetThreadDesktop(GetCurrentThreadId()) }
        .ok()
        .and_then(desktop_name)
}

fn must_follow(thread: Option<&str>, input: Option<&str>) -> bool {
    matches!((thread, input), (Some(thread), Some(input)) if !thread.eq_ignore_ascii_case(input))
}

pub(crate) fn keep_on_input_desktop() {
    if must_follow(
        thread_desktop_name().as_deref(),
        input_desktop_name().as_deref(),
    ) {
        follow_input_desktop();
    }
}

pub(crate) fn on_input_desktop<T: Send>(work: impl FnOnce() -> T + Send) -> T {
    if !must_follow(
        thread_desktop_name().as_deref(),
        input_desktop_name().as_deref(),
    ) {
        return work();
    }
    let mut work = Some(work);
    let slot = &mut work;
    let done = std::thread::scope(|scope| {
        let worker = std::thread::Builder::new()
            .name("input-desktop".into())
            .spawn_scoped(scope, move || {
                if !follow_input_desktop() {
                    tracing::warn!("display work could not move to the input desktop");
                }
                slot.take().map(|work| work())
            });
        match worker {
            Ok(worker) => worker
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
            Err(error) => {
                tracing::warn!(%error, "display work could not start on the input desktop");
                None
            }
        }
    });
    done.expect("scoped input-desktop work ran")
}

pub(crate) fn open_interface(guid: GUID) -> Result<HANDLE> {
    // SAFETY: The SetupAPI structures and aligned detail buffer remain live through each call.
    unsafe {
        let set = SetupDiGetClassDevsW(
            Some(&guid),
            PCWSTR::null(),
            None,
            DIGCF_DEVICEINTERFACE | DIGCF_PRESENT,
        )?;
        let result = (|| -> Result<HANDLE> {
            let mut interface = SP_DEVICE_INTERFACE_DATA {
                cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
                ..Default::default()
            };
            SetupDiEnumDeviceInterfaces(set, None, &guid, 0, &mut interface)?;
            let mut size = 0;
            let _ =
                SetupDiGetDeviceInterfaceDetailW(set, &interface, None, 0, Some(&mut size), None);
            if !(8..=65536).contains(&size) {
                bail!("invalid device interface path length");
            }
            let mut data = vec![0u64; (size as usize).div_ceil(8)];
            let detail = data.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            SetupDiGetDeviceInterfaceDetailW(set, &interface, Some(detail), size, None, None)?;
            Ok(CreateFileW(
                PCWSTR((*detail).DevicePath.as_ptr()),
                GENERIC_READ.0 | GENERIC_WRITE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                None,
            )?)
        })();
        SetupDiDestroyDeviceInfoList(set)?;
        result
    }
}

pub(crate) fn is_system() -> bool {
    let mut raw_token = HANDLE::default();
    // SAFETY: `raw_token` is a live out-pointer and `buffer` is usize-aligned.
    let token = unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw_token).is_err() {
            return false;
        }
        OwnedHandle::from_raw_handle(raw_token.0)
    };
    let mut buffer = [0usize; 64];
    let mut needed = 0;
    // SAFETY: `buffer` is writable for its byte size; the SID points into `buffer`.
    unsafe {
        if GetTokenInformation(
            HANDLE(token.as_raw_handle()),
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            size_of::<[usize; 64]>() as u32,
            &mut needed,
        )
        .is_err()
        {
            return false;
        }
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        IsWellKnownSid(user.User.Sid, WinLocalSystemSid).as_bool()
    }
}

pub(crate) fn registry_string(root: HKEY, key: &str, value: &str) -> Option<String> {
    let key: Vec<u16> = key.encode_utf16().chain(Some(0)).collect();
    let value: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
    let mut buffer = [0u16; 1024];
    let mut size = (buffer.len() * 2) as u32;
    // SAFETY: `key` and `value` are NUL-terminated and outlive the call.
    unsafe {
        RegGetValueW(
            root,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
        .ok()
        .ok()?;
    }
    let text = crate::text::from_wide(&buffer);
    (!text.is_empty()).then_some(text)
}

pub(crate) fn primary_display_name() -> Option<String> {
    use windows::Win32::Graphics::Gdi::*;
    let mut index = 0u32;
    loop {
        let mut device = DISPLAY_DEVICEW {
            cb: size_of::<DISPLAY_DEVICEW>() as u32,
            ..Default::default()
        };
        // SAFETY: `device.cb` matches `DISPLAY_DEVICEW` and lives for the call.
        if !unsafe { EnumDisplayDevicesW(PCWSTR::null(), index, &mut device, 0) }.as_bool() {
            return None;
        }
        if (device.StateFlags & DISPLAY_DEVICE_PRIMARY_DEVICE) != DISPLAY_DEVICE_STATE_FLAGS(0) {
            let name = crate::text::from_wide(&device.DeviceName);
            if !name.is_empty() {
                return Some(name);
            }
        }
        index += 1;
    }
}

pub(crate) fn minimized_windows() -> std::collections::BTreeSet<isize> {
    on_input_desktop(|| {
        unsafe extern "system" fn visit(window: HWND, data: LPARAM) -> windows::core::BOOL {
            // SAFETY: `EnumWindows` passes `&mut found` as `data`, which outlives the synchronous call.
            unsafe {
                let found = &mut *(data.0 as *mut std::collections::BTreeSet<isize>);
                if IsIconic(window).as_bool() {
                    found.insert(window.0 as isize);
                }
                windows::core::BOOL(1)
            }
        }
        let mut found = std::collections::BTreeSet::new();
        // SAFETY: `found` outlives the synchronous `EnumWindows` enumeration.
        unsafe {
            let _ = EnumWindows(
                Some(visit),
                LPARAM((&mut found as *mut std::collections::BTreeSet<isize>) as isize),
            );
        }
        found
    })
}
