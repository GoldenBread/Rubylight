//! Stream-state and Win32 monitor-HDR probing with short-lived caching.

#[cfg(windows)]
use std::ffi::c_void;
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

pub(crate) const INJECT_CACHE_TTL: Duration = Duration::from_millis(250);

#[derive(Default)]
pub(crate) struct InjectCache {
    stream_active: Option<(Instant, bool)>,
    windows: BTreeMap<usize, (Instant, bool)>,
}

impl InjectCache {
    fn stream_active_cached(&self, now: Instant) -> Option<bool> {
        self.stream_active.and_then(|(at, active)| {
            (now.saturating_duration_since(at) < INJECT_CACHE_TTL).then_some(active)
        })
    }

    fn window_hdr_cached(&self, now: Instant, window: usize) -> Option<bool> {
        self.windows.get(&window).and_then(|&(at, hdr)| {
            (now.saturating_duration_since(at) < INJECT_CACHE_TTL).then_some(hdr)
        })
    }

    fn store_stream(&mut self, now: Instant, active: bool) {
        self.stream_active = Some((now, active));
    }

    fn store_window(&mut self, now: Instant, window: usize, hdr: bool) {
        if self.windows.len() >= 128 {
            self.windows
                .retain(|_, (at, _)| now.saturating_duration_since(*at) < INJECT_CACHE_TTL);
        }
        self.windows.insert(window, (now, hdr));
    }
}

#[cfg(windows)]
fn stream_active() -> bool {
    use windows::{
        Win32::{Foundation::*, System::Threading::*},
        core::w,
    };
    for name in [
        w!("Global\\ButterpolloRustVirtualHdrActive"),
        w!("Local\\ButterpolloRustVirtualHdrActive"),
    ] {
        // SAFETY: `name` is a NUL-terminated static UTF-16 literal.
        if let Ok(event) = unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, false, name) } {
            // SAFETY: `event` is the handle just opened.
            let active = unsafe { WaitForSingleObject(event, 0) == WAIT_OBJECT_0 };
            // SAFETY: `event` is open and not used afterwards.
            unsafe {
                let _ = CloseHandle(event);
            }
            if active {
                return true;
            }
        }
    }
    false
}

#[cfg(not(windows))]
fn stream_active() -> bool {
    false
}

#[cfg(windows)]
fn hdr_supported(window: usize) -> bool {
    use windows::Win32::{Devices::Display::*, Foundation::*, Graphics::Gdi::*};
    // SAFETY: every pointer passed is to a local of the size its header
    // states, and the path and mode buffers hold the counts Windows reported.
    unsafe {
        let monitor = MonitorFromWindow(HWND(window as *mut c_void), MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if monitor.is_invalid()
            || !GetMonitorInfoW(monitor, (&mut info as *mut MONITORINFOEXW).cast()).as_bool()
        {
            return false;
        }
        let (mut paths_len, mut modes_len) = (0, 0);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut paths_len, &mut modes_len)
            .is_err()
            || paths_len > 256
            || modes_len > 768
        {
            return false;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); paths_len as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); modes_len as usize];
        if QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut paths_len,
            paths.as_mut_ptr(),
            &mut modes_len,
            modes.as_mut_ptr(),
            None,
        )
        .is_err()
        {
            return false;
        }
        let hdr = |r#type, size, adapter_id, id| DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type,
            size: size as u32,
            adapterId: adapter_id,
            id,
        };
        for path in &paths[..paths_len as usize] {
            let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
                header: hdr(
                    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                    size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>(),
                    path.sourceInfo.adapterId,
                    path.sourceInfo.id,
                ),
                ..Default::default()
            };
            if DisplayConfigGetDeviceInfo(&mut source.header) != 0
                || source.viewGdiDeviceName != info.szDevice
            {
                continue;
            }
            // Windows 11 24H2 distinguishes HDR from automatic wide color.
            #[repr(C)]
            struct Advanced2 {
                header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
                flags: u32,
                encoding: u32,
                bits: u32,
                active: u32,
            }
            let mut color = Advanced2 {
                header: hdr(
                    DISPLAYCONFIG_DEVICE_INFO_TYPE(15),
                    size_of::<Advanced2>(),
                    path.targetInfo.adapterId,
                    path.targetInfo.id,
                ),
                flags: 0,
                encoding: 0,
                bits: 0,
                active: 0,
            };
            if DisplayConfigGetDeviceInfo(&mut color.header) == 0 {
                return color.flags & 16 != 0;
            }
            let mut old = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO {
                header: hdr(
                    DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
                    size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>(),
                    path.targetInfo.adapterId,
                    path.targetInfo.id,
                ),
                ..Default::default()
            };
            return DisplayConfigGetDeviceInfo(&mut old.header) == 0
                && old.Anonymous.value & 1 != 0;
        }
        false
    }
}

#[cfg(not(windows))]
fn hdr_supported(_: usize) -> bool {
    false
}

pub(crate) fn inject_with(
    surface: u64,
    now: Instant,
    probe_stream: impl FnOnce() -> bool,
    probe_hdr: impl FnOnce(usize) -> bool,
) -> bool {
    if std::env::var("DISABLE_SUNSHINE_VIRTUAL_HDR").is_ok_and(|v| v == "1") {
        return false;
    }
    let (cached_stream, window, cached_hdr) =
        super::state().lock().ok().map_or((None, None, None), |s| {
            let win = s.surfaces.get(&surface).map(|(_, w)| *w);
            (
                s.inject.stream_active_cached(now),
                win,
                win.and_then(|w| s.inject.window_hdr_cached(now, w)),
            )
        });
    let active = cached_stream.unwrap_or_else(|| {
        let v = probe_stream();
        if let Ok(mut s) = super::state().lock() {
            s.inject.store_stream(now, v);
        }
        v
    });
    if !active {
        return false;
    }
    let Some(window) = window else {
        return false;
    };
    if std::env::var("SUNSHINE_VHDR_FORCE").is_ok_and(|v| v.starts_with('1')) {
        return true;
    }
    cached_hdr.unwrap_or_else(|| {
        let v = probe_hdr(window);
        if let Ok(mut s) = super::state().lock() {
            s.inject.store_window(now, window, v);
        }
        v
    })
}

pub(crate) fn inject(surface: u64) -> bool {
    inject_with(surface, Instant::now(), stream_active, hdr_supported)
}
