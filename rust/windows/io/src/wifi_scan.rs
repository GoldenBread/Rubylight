//! Wi-Fi adapter media streaming mode to reduce periodic background scans during a stream.
use windows::Win32::{Foundation::HANDLE, NetworkManagement::WiFi::*, System::LibraryLoader::*};

/// Guard holding a WLAN client handle that keeps connected Wi-Fi adapters in media streaming mode.
pub struct WifiStreamingGuard(usize);

impl Drop for WifiStreamingGuard {
    fn drop(&mut self) {
        close_wlan(self.0);
    }
}

/// Enable media streaming mode on every connected Wi-Fi adapter; returns `None` if unavailable.
pub fn enter_streaming_mode() -> Option<WifiStreamingGuard> {
    wifi_streaming_mode().map(WifiStreamingGuard)
}

fn wifi_streaming_mode() -> Option<usize> {
    type Open =
        unsafe extern "system" fn(u32, *const core::ffi::c_void, *mut u32, *mut HANDLE) -> u32;
    type Enumerate = unsafe extern "system" fn(
        HANDLE,
        *const core::ffi::c_void,
        *mut *mut WLAN_INTERFACE_INFO_LIST,
    ) -> u32;
    type Set = unsafe extern "system" fn(
        HANDLE,
        *const windows::core::GUID,
        WLAN_INTF_OPCODE,
        u32,
        *const core::ffi::c_void,
        *const core::ffi::c_void,
    ) -> u32;
    type Free = unsafe extern "system" fn(*const core::ffi::c_void);
    type Close = unsafe extern "system" fn(HANDLE, *const core::ffi::c_void) -> u32;
    // SAFETY: Each pointer is resolved from wlanapi.dll, never freed, with its documented
    // signature; `list` is non-null and freed only after its dwNumberOfItems entries are read.
    unsafe {
        let module = LoadLibraryW(windows::core::w!("wlanapi.dll")).ok()?;
        let open: Open =
            std::mem::transmute(GetProcAddress(module, windows::core::s!("WlanOpenHandle"))?);
        let enumerate: Enumerate = std::mem::transmute(GetProcAddress(
            module,
            windows::core::s!("WlanEnumInterfaces"),
        )?);
        let set: Set = std::mem::transmute(GetProcAddress(
            module,
            windows::core::s!("WlanSetInterface"),
        )?);
        let free: Free =
            std::mem::transmute(GetProcAddress(module, windows::core::s!("WlanFreeMemory"))?);
        let close: Close = std::mem::transmute(GetProcAddress(
            module,
            windows::core::s!("WlanCloseHandle"),
        )?);
        let (mut version, mut handle) = (0, HANDLE::default());
        if open(2, std::ptr::null(), &mut version, &mut handle) != 0 {
            return None;
        }
        let mut list = std::ptr::null_mut();
        if enumerate(handle, std::ptr::null(), &mut list) != 0 || list.is_null() {
            close(handle, std::ptr::null());
            return None;
        }
        let interfaces = std::slice::from_raw_parts(
            (&raw const (*list).InterfaceInfo).cast::<WLAN_INTERFACE_INFO>(),
            (*list).dwNumberOfItems as usize,
        );
        let enabled = windows::core::BOOL::from(true);
        let mut any = false;
        for interface in interfaces
            .iter()
            .filter(|i| i.isState == wlan_interface_state_connected)
        {
            if set(
                handle,
                &interface.InterfaceGuid,
                wlan_intf_opcode_media_streaming_mode,
                size_of::<windows::core::BOOL>() as u32,
                (&enabled as *const windows::core::BOOL).cast(),
                std::ptr::null(),
            ) == 0
            {
                any = true;
            }
        }
        free(list.cast());
        if !any {
            close(handle, std::ptr::null());
            return None;
        }
        tracing::info!("Wi-Fi adapter in media streaming mode while streaming");
        Some(handle.0 as usize)
    }
}

fn close_wlan(handle: usize) {
    type Close = unsafe extern "system" fn(HANDLE, *const core::ffi::c_void) -> u32;
    // SAFETY: wlanapi.dll was loaded and never freed, WlanCloseHandle has this signature, and
    // `handle` came from WlanOpenHandle; WifiStreamingGuard hands it here only once.
    unsafe {
        if let Ok(module) = GetModuleHandleW(windows::core::w!("wlanapi.dll"))
            && let Some(close) = GetProcAddress(module, windows::core::s!("WlanCloseHandle"))
        {
            let close: Close = std::mem::transmute(close);
            close(HANDLE(handle as *mut _), std::ptr::null());
        }
    }
}
