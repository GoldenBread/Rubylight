//! Local Win32 COM, build-number, System32 DLL loader, input thread priority, and high-res timer helpers for I/O modules.
use anyhow::{Result, bail};
use std::time::Instant;
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::{Com::*, LibraryLoader::*, Performance::*, SystemInformation::*, Threading::*},
    },
    core::PCWSTR,
};

pub(crate) struct ComGuard;
impl ComGuard {
    pub(crate) fn new() -> Result<Self> {
        // SAFETY: CoInitializeEx takes no pointers here; only a successful call yields the guard.
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        }
        Ok(Self)
    }
}
impl Drop for ComGuard {
    fn drop(&mut self) {
        // SAFETY: CoUninitialize balances the successful CoInitializeEx in `new`.
        unsafe {
            CoUninitialize();
        }
    }
}

pub(crate) fn windows_build() -> Option<u32> {
    static BUILD: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
    *BUILD
        // SAFETY: The registry names are terminated and the output size matches the live UTF-16 buffer.
        .get_or_init(|| unsafe {
            use windows::Win32::System::Registry::*;
            let mut text = [0u16; 32];
            let mut size = (text.len() * 2) as u32;
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                windows::core::w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"),
                windows::core::w!("CurrentBuildNumber"),
                RRF_RT_REG_SZ,
                None,
                Some(text.as_mut_ptr().cast()),
                Some(&mut size),
            )
            .ok()
            .ok()?;
            crate::text::from_wide(&text).trim().parse().ok()
        })
}

pub(crate) fn system_library(name: &str) -> Result<libloading::Library> {
    let mut directory = [0u16; 32768];
    // SAFETY: `directory` is a live writable UTF-16 buffer.
    let length = unsafe { GetSystemDirectoryW(Some(&mut directory)) } as usize;
    if length == 0 || length >= directory.len() {
        bail!("cannot locate the Windows system directory");
    }
    let path = std::path::PathBuf::from(String::from_utf16(&directory[..length])?).join(name);
    // SAFETY: `path` is an absolute System32 path; loading runs the installed driver's initialisers.
    Ok(unsafe {
        libloading::os::windows::Library::load_with_flags(path, LOAD_LIBRARY_SEARCH_SYSTEM32.0)?
    }
    .into())
}

#[cfg(not(test))]
pub(crate) struct InputPriority {
    handle: HANDLE,
}
#[cfg(not(test))]
impl InputPriority {
    pub(crate) fn enter() -> Self {
        // SAFETY: `name` is NUL-terminated UTF-16 alive for the call; `index` is a live local.
        unsafe {
            let name: Vec<u16> = "Games\0".encode_utf16().collect();
            let mut index = 0;
            let handle = AvSetMmThreadCharacteristicsW(PCWSTR(name.as_ptr()), &mut index)
                .unwrap_or_default();
            if !handle.is_invalid() {
                let _ = AvSetMmThreadPriority(handle, AVRT_PRIORITY_HIGH);
            } else {
                let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL);
            }
            Self { handle }
        }
    }
}
#[cfg(not(test))]
impl Drop for InputPriority {
    fn drop(&mut self) {
        if !self.handle.is_invalid() {
            // SAFETY: `self.handle` was returned by AvSetMmThreadCharacteristicsW and closed once.
            unsafe {
                let _ = AvRevertMmThreadCharacteristics(self.handle);
            }
        }
    }
}

pub(crate) fn qpc_ms() -> f64 {
    let (mut ticks, mut frequency) = (0, 0);
    // SAFETY: both calls write only to the live locals.
    let ok = unsafe {
        QueryPerformanceCounter(&mut ticks).is_ok()
            && QueryPerformanceFrequency(&mut frequency).is_ok()
    };
    if ok && frequency > 0 {
        ticks as f64 * 1000. / frequency as f64
    } else {
        0.
    }
}

pub(crate) struct Timer(HANDLE);
impl Timer {
    pub(crate) fn new() -> Result<Self> {
        // SAFETY: Null attributes and name create a new unnamed timer; Timer closes it in Drop.
        unsafe {
            let timer = CreateWaitableTimerExW(
                None,
                PCWSTR::null(),
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS.0,
            )
            .or_else(|_| CreateWaitableTimerExW(None, PCWSTR::null(), 0, TIMER_ALL_ACCESS.0))?;
            Ok(Self(timer))
        }
    }
    pub(crate) fn until(&self, deadline: Instant) {
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return;
            }
            let ticks = -(remaining.as_nanos().div_ceil(100).min(i64::MAX as u128) as i64);
            // SAFETY: `self.0` is live for this borrow and `ticks` outlives the call.
            let armed = unsafe { SetWaitableTimer(self.0, &ticks, 0, None, None, false) }.is_ok();
            if armed {
                // SAFETY: `self.0` is live and closed only on Drop.
                let res = unsafe { WaitForSingleObject(self.0, INFINITE) };
                if res != WAIT_OBJECT_0 {
                    std::thread::sleep(remaining);
                    return;
                }
            } else {
                std::thread::sleep(remaining);
                return;
            }
        }
    }
}
impl Drop for Timer {
    fn drop(&mut self) {
        // SAFETY: `self.0` is owned by this Timer and closed exactly once here.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub(crate) fn active_displays_gdi() -> Vec<(String, RECT)> {
    let mut result = Vec::new();
    let mut index = 0u32;
    loop {
        let mut device = DISPLAY_DEVICEW {
            cb: size_of::<DISPLAY_DEVICEW>() as u32,
            ..Default::default()
        };
        // SAFETY: `device.cb` matches `DISPLAY_DEVICEW`.
        if !unsafe { EnumDisplayDevicesW(PCWSTR::null(), index, &mut device, 0) }.as_bool() {
            break;
        }
        index += 1;
        if (device.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP) == DISPLAY_DEVICE_STATE_FLAGS(0)
        {
            continue;
        }
        let name = crate::text::from_wide(&device.DeviceName);
        if let Some(rect) = current_display_rect_gdi(&name) {
            result.push((name, rect));
        }
    }
    result
}

pub(crate) fn current_display_rect_gdi(name: &str) -> Option<RECT> {
    if !name.starts_with(r"\\.\") {
        return None;
    }
    let wide = crate::text::to_wide(name);
    let mut mode = DEVMODEW {
        dmSize: size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    // SAFETY: `wide` is NUL-terminated and `mode.dmSize` matches `DEVMODEW`.
    if !unsafe { EnumDisplaySettingsW(PCWSTR(wide.as_ptr()), ENUM_CURRENT_SETTINGS, &mut mode) }
        .as_bool()
    {
        return None;
    }
    // SAFETY: `EnumDisplaySettingsW` fills `dmPosition` for display modes.
    let pos = unsafe { mode.Anonymous1.Anonymous2.dmPosition };
    Some(RECT {
        left: pos.x,
        top: pos.y,
        right: pos.x + mode.dmPelsWidth as i32,
        bottom: pos.y + mode.dmPelsHeight as i32,
    })
}
