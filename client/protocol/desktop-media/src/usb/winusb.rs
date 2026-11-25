//! Direct Windows inbox `winusb.dll` USB/IP transport with full isochronous support.
//!
//! Serves control, bulk, interrupt, and isochronous USB/IP transfers directly through Microsoft's
//! inbox `winusb.dll` (`WinUsb_RegisterIsochBuffer` / `WinUsb_ReadIsochPipeAsap` /
//! `WinUsb_WriteIsochPipeAsap`) so Windows users need zero extra drivers beyond `WinUSB`.

#![cfg_attr(not(windows), allow(dead_code))]

#[path = "winusb_ffi.rs"]
mod ffi;

pub(super) use ffi::{
    control_target_endpoint, control_target_interface, pack_winusb_iso_in, pack_winusb_iso_out,
    usbd_status_to_errno, win32_error_to_errno, UsbdIsoPacketDescriptor,
};

#[cfg(windows)]
mod platform {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
    use std::time::{Duration, Instant};

    use nightjar_core::usb::{wire, Blocking, BlockingDevice, Direction, Reply, Transfer, UsbDevice};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_IO_PENDING, GENERIC_READ, GENERIC_WRITE, HANDLE,
        WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::IO::{CancelIoEx, OVERLAPPED};
    use windows::Win32::System::Threading::WaitForSingleObject;

    use super::ffi::{
        api, control_target_endpoint, control_target_interface, pack_winusb_iso_in,
        pack_winusb_iso_out, win32_error_to_errno, IsochBufferGuard, IsochBufferHandle,
        OwnedEvent, UsbInterfaceDescriptor, UsbdIsoPacketDescriptor, WinBool, WinUsbApi,
        WinUsbHandle, WinUsbSetupPacket,
    };
    use crate::usb::iso::iso_plan;
    use crate::usb::libusb::{standard, Standard};

    const URB_ZERO_PACKET: u32 = 0x40;
    const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);
    const ISO_TIMEOUT: Duration = Duration::from_secs(5);
    const POLL_MS: u32 = 50;

    #[derive(Clone, Copy)]
    struct InterfaceEntry {
        winusb: WinUsbHandle,
        file: HANDLE,
        associated: bool,
    }

    // SAFETY: WinUSB handles opened with `FILE_FLAG_OVERLAPPED` are safe to submit I/O on across
    // threads; handle teardown happens in `close()` after `Blocking::close()` joins all lanes.
    unsafe impl Send for InterfaceEntry {}
    unsafe impl Sync for InterfaceEntry {}

    #[derive(Clone, Copy)]
    struct EndpointInfo {
        interface: u8,
        max_packet: u16,
    }

    struct Handles {
        interfaces: HashMap<u8, InterfaceEntry>,
        files: Vec<HANDLE>,
        default_interface: u8,
    }

    // SAFETY: Handles are freed exclusively after all I/O lanes have stopped.
    unsafe impl Send for Handles {}
    unsafe impl Sync for Handles {}

    impl Handles {
        fn close_all(&mut self, api: &WinUsbApi) {
            // WinUSB requires freeing associated interface handles before their base handle.
            for entry in self.interfaces.values().filter(|e| e.associated) {
                unsafe {
                    let _ = (api.free)(entry.winusb);
                }
            }
            for entry in self.interfaces.values().filter(|e| !e.associated) {
                unsafe {
                    let _ = (api.free)(entry.winusb);
                }
            }
            self.interfaces.clear();
            for file in self.files.drain(..) {
                unsafe {
                    let _ = CloseHandle(file);
                }
            }
        }
    }

    struct State {
        configuration: u8,
        active: u8,
        endpoints: HashMap<u8, EndpointInfo>,
        alt_endpoints: HashMap<(u8, u8), Vec<(u8, u16)>>,
        streaming: HashMap<u8, bool>,
        handles: Handles,
    }

    struct WinUsbDevice {
        api: &'static WinUsbApi,
        described: wire::Device,
        class: u8,
        closed: AtomicBool,
        state: Mutex<State>,
    }

    impl WinUsbDevice {
        fn state(&self) -> MutexGuard<'_, State> {
            self.state.lock().unwrap_or_else(PoisonError::into_inner)
        }

        fn interface_handle(&self, interface: Option<u8>) -> Option<InterfaceEntry> {
            let state = self.state();
            if let Some(num) = interface
                && let Some(&entry) = state.handles.interfaces.get(&num)
            {
                return Some(entry);
            }
            state
                .handles
                .interfaces
                .get(&state.handles.default_interface)
                .copied()
                .or_else(|| state.handles.interfaces.values().next().copied())
        }

        fn endpoint_entry(&self, address: u8) -> Option<(InterfaceEntry, EndpointInfo)> {
            let state = self.state();
            let info = *state.endpoints.get(&address)?;
            let entry = *state.handles.interfaces.get(&info.interface)?;
            Some((entry, info))
        }

        /// Waits for an overlapped WinUSB call to complete, cancelling via `CancelIoEx` and
        /// draining `WinUsb_GetOverlappedResult(..., TRUE)` before returning so caller buffers
        /// and iso registrations are never touched after return.
        fn wait_overlapped(
            &self,
            entry: InterfaceEntry,
            overlapped: &mut OVERLAPPED,
            timeout: Option<Duration>,
            cancelled: &AtomicBool,
        ) -> Result<u32, i32> {
            let deadline = timeout.map(|d| Instant::now() + d);
            let mut cancel_reason: Option<i32> = None;

            loop {
                // SAFETY: `overlapped.hEvent` is a valid manual-reset event owned by caller.
                let wait = unsafe { WaitForSingleObject(overlapped.hEvent, POLL_MS) };
                if wait == WAIT_OBJECT_0 {
                    break;
                }
                if wait != WAIT_TIMEOUT {
                    cancel_reason = Some(wire::EPROTO);
                    break;
                }
                if cancelled.load(Ordering::SeqCst) || self.closed.load(Ordering::Acquire) {
                    cancel_reason = Some(wire::ECONNRESET);
                    break;
                }
                if let Some(limit) = deadline
                    && Instant::now() >= limit
                {
                    cancel_reason = Some(wire::ETIMEDOUT);
                    break;
                }
            }

            if cancel_reason.is_some() {
                // SAFETY: `entry.file` is open and `overlapped` stays pinned until
                // `WinUsb_GetOverlappedResult(..., TRUE)` returns below.
                unsafe {
                    let _ = CancelIoEx(entry.file, Some(overlapped));
                }
            }

            let mut transferred = 0u32;
            // SAFETY: `bWait = 1` guarantees the kernel has completely finished with `overlapped`
            // and its buffers before this call returns.
            let ok = unsafe {
                (self.api.get_overlapped_result)(entry.winusb, overlapped, &mut transferred, 1)
            };
            if ok != 0 {
                return Ok(transferred);
            }
            let err = unsafe { GetLastError().0 };
            if err == 995 {
                // ERROR_OPERATION_ABORTED: preserve timeout vs cancel distinction.
                return Err(cancel_reason.unwrap_or(wire::ECONNRESET));
            }
            Err(win32_error_to_errno(err))
        }

        fn set_interface(&self, interface: u8, setting: u8) -> Reply {
            let entry = {
                let state = self.state();
                let Some(&entry) = state.handles.interfaces.get(&interface) else {
                    return Reply::failed(wire::EINVAL);
                };
                entry
            };
            // SAFETY: `entry.winusb` is an open WinUSB handle for `interface`.
            let ok = unsafe { (self.api.set_alt)(entry.winusb, setting) };
            if ok == 0 {
                let err = unsafe { GetLastError().0 };
                return Reply::failed(win32_error_to_errno(err));
            }
            let mut state = self.state();
            let old_eps: Vec<u8> = state
                .endpoints
                .iter()
                .filter_map(|(&ep, info)| (info.interface == interface).then_some(ep))
                .collect();
            for ep in old_eps {
                state.endpoints.remove(&ep);
                state.streaming.insert(ep, false);
            }
            if let Some(updated) = state.alt_endpoints.get(&(interface, setting)).cloned() {
                for (ep, max_packet) in updated {
                    state.endpoints.insert(ep, EndpointInfo { interface, max_packet });
                    state.streaming.insert(ep, false);
                }
            }
            Reply::sent(0)
        }

        fn clear_halt(&self, endpoint: u8) -> Reply {
            let Some((entry, _)) = self.endpoint_entry(endpoint) else {
                return Reply::failed(wire::EPIPE);
            };
            // SAFETY: `entry.winusb` owns `endpoint`.
            let ok = unsafe { (self.api.reset_pipe)(entry.winusb, endpoint) };
            self.state().streaming.insert(endpoint, false);
            if ok == 0 {
                let err = unsafe { GetLastError().0 };
                Reply::failed(win32_error_to_errno(err))
            } else {
                Reply::sent(0)
            }
        }

        fn reset_port(&self) -> Reply {
            let pipes: Vec<(InterfaceEntry, u8)> = {
                let mut state = self.state();
                state.streaming.clear();
                state
                    .endpoints
                    .iter()
                    .filter_map(|(&ep, info)| {
                        state.handles.interfaces.get(&info.interface).map(|&e| (e, ep))
                    })
                    .collect()
            };
            for (entry, ep) in pipes {
                unsafe {
                    let _ = (self.api.abort_pipe)(entry.winusb, ep);
                    if ep & 0x80 != 0 {
                        let _ = (self.api.flush_pipe)(entry.winusb, ep);
                    }
                    let _ = (self.api.reset_pipe)(entry.winusb, ep);
                }
            }
            Reply::sent(0)
        }

        fn control(&self, transfer: &Transfer, cancelled: &AtomicBool) -> Reply {
            let setup = &transfer.setup;
            match standard(setup) {
                Some(Standard::SetConfiguration(value)) => {
                    let mut state = self.state();
                    if value == state.configuration {
                        return Reply::sent(0);
                    }
                    if value == 0 {
                        state.configuration = 0;
                        return Reply::sent(0);
                    }
                    if value == state.active {
                        state.configuration = value;
                        return Reply::sent(0);
                    }
                    return Reply::failed(wire::EINVAL);
                }
                Some(Standard::SetInterface { interface, setting }) => {
                    return self.set_interface(interface, setting);
                }
                Some(Standard::ClearHalt(endpoint)) => return self.clear_halt(endpoint),
                Some(Standard::PortReset) => return self.reset_port(),
                None => {}
            }

            let target_iface = control_target_interface(setup).or_else(|| {
                control_target_endpoint(setup)
                    .and_then(|ep| self.state().endpoints.get(&ep).map(|i| i.interface))
            });
            let Some(entry) = self.interface_handle(target_iface) else {
                return Reply::failed(wire::ENODEV);
            };
            let packet = WinUsbSetupPacket {
                request_type: setup[0],
                request: setup[1],
                value: u16::from_le_bytes([setup[2], setup[3]]),
                index: u16::from_le_bytes([setup[4], setup[5]]),
                length: u16::from_le_bytes([setup[6], setup[7]]),
            };
            let is_in = setup[0] & 0x80 != 0;
            let len = if is_in {
                usize::from(packet.length).min(transfer.length as usize)
            } else {
                transfer.data.len()
            };
            let mut buffer = if is_in { vec![0u8; len] } else { transfer.data.clone() };
            let event = match OwnedEvent::new() {
                Ok(e) => e,
                Err(status) => return Reply::failed(status),
            };
            let mut overlapped = OVERLAPPED { hEvent: event.0, ..Default::default() };
            let mut transferred = 0u32;
            let buf_ptr = if buffer.is_empty() { std::ptr::null_mut() } else { buffer.as_mut_ptr() };
            // SAFETY: `buffer` and `overlapped` remain pinned until `wait_overlapped` returns.
            let ok = unsafe {
                (self.api.control_transfer)(
                    entry.winusb,
                    packet,
                    buf_ptr,
                    buffer.len() as u32,
                    &mut transferred,
                    &mut overlapped,
                )
            };
            let moved = if ok != 0 {
                Ok(transferred)
            } else {
                let err = unsafe { GetLastError().0 };
                if err == ERROR_IO_PENDING.0 {
                    self.wait_overlapped(entry, &mut overlapped, Some(CONTROL_TIMEOUT), cancelled)
                } else {
                    Err(win32_error_to_errno(err))
                }
            };
            match moved {
                Ok(n) if is_in => {
                    buffer.truncate((n as usize).min(buffer.len()));
                    Reply::read(buffer)
                }
                Ok(n) => Reply::sent(n),
                Err(status) => Reply::failed(status),
            }
        }

        fn pipe_transfer(
            &self,
            entry: InterfaceEntry,
            address: u8,
            max_packet: u16,
            transfer: &Transfer,
            cancelled: &AtomicBool,
        ) -> Reply {
            let submit = |buf: *mut u8, len: u32, is_in: bool| -> Result<u32, i32> {
                let event = OwnedEvent::new()?;
                let mut overlapped = OVERLAPPED { hEvent: event.0, ..Default::default() };
                let mut transferred = 0u32;
                let call = if is_in { self.api.read_pipe } else { self.api.write_pipe };
                // SAFETY: caller keeps `buf` alive until `wait_overlapped` finishes.
                let ok = unsafe {
                    call(entry.winusb, address, buf, len, &mut transferred, &mut overlapped)
                };
                if ok != 0 {
                    Ok(transferred)
                } else {
                    let err = unsafe { GetLastError().0 };
                    if err == ERROR_IO_PENDING.0 {
                        self.wait_overlapped(entry, &mut overlapped, None, cancelled)
                    } else {
                        Err(win32_error_to_errno(err))
                    }
                }
            };

            match transfer.direction {
                Direction::In => {
                    let mut buffer = vec![0u8; transfer.length as usize];
                    let ptr = if buffer.is_empty() { std::ptr::null_mut() } else { buffer.as_mut_ptr() };
                    match submit(ptr, buffer.len() as u32, true) {
                        Ok(n) => {
                            buffer.truncate((n as usize).min(buffer.len()));
                            Reply::read(buffer)
                        }
                        Err(status) => Reply::failed(status),
                    }
                }
                Direction::Out => {
                    let data = &transfer.data;
                    let ptr = if data.is_empty() { std::ptr::null_mut() } else { data.as_ptr().cast_mut() };
                    let sent = match submit(ptr, data.len() as u32, false) {
                        Ok(n) => n,
                        Err(status) => return Reply::failed(status),
                    };
                    let zero = transfer.flags & URB_ZERO_PACKET != 0
                        && !data.is_empty()
                        && max_packet > 0
                        && data.len().is_multiple_of(usize::from(max_packet));
                    if zero
                        && let Err(status) = submit(std::ptr::null_mut(), 0, false)
                    {
                        return Reply::failed(status);
                    }
                    Reply::sent(sent)
                }
            }
        }

        fn isochronous(&self, address: u8, transfer: &Transfer, cancelled: &AtomicBool) -> Reply {
            let total = match iso_plan(transfer) {
                Ok(total) => total,
                Err(status) => return Reply::failed(status),
            };
            let Some((entry, _)) = self.endpoint_entry(address) else {
                return Reply::failed(wire::EPIPE);
            };
            let mut buffer = match transfer.direction {
                Direction::In => vec![0u8; total],
                Direction::Out => transfer.data[..total].to_vec(),
            };
            let mut iso_handle: IsochBufferHandle = std::ptr::null_mut();
            // SAFETY: `buffer` stays pinned until `_iso_guard` drops after I/O finishes.
            let reg = unsafe {
                (self.api.register_isoch_buffer)(
                    entry.winusb,
                    address,
                    buffer.as_mut_ptr(),
                    total as u32,
                    &mut iso_handle,
                )
            };
            if reg == 0 || iso_handle.is_null() {
                let err = unsafe { GetLastError().0 };
                return Reply::failed(win32_error_to_errno(err));
            }
            let _iso_guard = IsochBufferGuard { api: self.api, handle: iso_handle };
            let continue_stream = self.state().streaming.get(&address).copied().unwrap_or(false);

            match transfer.direction {
                Direction::In => {
                    let mut descs = vec![UsbdIsoPacketDescriptor::default(); transfer.iso.len()];
                    let result = self.submit_iso(entry, continue_stream, cancelled, |try_cont, ov| unsafe {
                        (self.api.read_isoch_pipe_asap)(
                            iso_handle,
                            0,
                            total as u32,
                            WinBool::from(try_cont),
                            descs.len() as u32,
                            descs.as_mut_ptr(),
                            ov,
                        )
                    });
                    match result {
                        Ok(_) => {
                            self.state().streaming.insert(address, true);
                            pack_winusb_iso_in(transfer.length, &transfer.iso, &descs, &buffer)
                        }
                        Err(status) => {
                            self.state().streaming.insert(address, false);
                            Reply::failed(status)
                        }
                    }
                }
                Direction::Out => {
                    let result = self.submit_iso(entry, continue_stream, cancelled, |try_cont, ov| unsafe {
                        (self.api.write_isoch_pipe_asap)(
                            iso_handle,
                            0,
                            total as u32,
                            WinBool::from(try_cont),
                            ov,
                        )
                    });
                    match result {
                        Ok(transferred) => {
                            self.state().streaming.insert(address, true);
                            pack_winusb_iso_out(transfer.length, &transfer.iso, transferred)
                        }
                        Err(status) => {
                            self.state().streaming.insert(address, false);
                            Reply::failed(status)
                        }
                    }
                }
            }
        }

        fn submit_iso(
            &self,
            entry: InterfaceEntry,
            continue_stream: bool,
            cancelled: &AtomicBool,
            mut issue: impl FnMut(bool, *mut OVERLAPPED) -> WinBool,
        ) -> Result<u32, i32> {
            let mut try_continue = continue_stream;
            for attempt in 0..2 {
                let event = OwnedEvent::new()?;
                let mut overlapped = OVERLAPPED { hEvent: event.0, ..Default::default() };
                if issue(try_continue, &mut overlapped) != 0 {
                    let mut transferred = 0u32;
                    let _ = unsafe {
                        (self.api.get_overlapped_result)(entry.winusb, &mut overlapped, &mut transferred, 1)
                    };
                    return Ok(transferred);
                }
                let err = unsafe { GetLastError().0 };
                if err == ERROR_IO_PENDING.0 {
                    return self.wait_overlapped(entry, &mut overlapped, Some(ISO_TIMEOUT), cancelled);
                }
                // If ContinueStream failed because the stream lapsed, retry once with ContinueStream = FALSE.
                if attempt == 0 && try_continue && matches!(err, 21 | 59 | 87) {
                    try_continue = false;
                    continue;
                }
                return Err(win32_error_to_errno(err));
            }
            Err(wire::EPROTO)
        }
    }

    impl Drop for WinUsbDevice {
        fn drop(&mut self) {
            self.closed.store(true, Ordering::Release);
            self.state().handles.close_all(self.api);
        }
    }

    impl BlockingDevice for WinUsbDevice {
        fn describe(&self) -> wire::Device {
            wire::Device {
                configuration: self.state().configuration,
                ..self.described.clone()
            }
        }

        fn class(&self) -> u8 {
            self.class
        }

        fn transfer(&self, transfer: &Transfer, cancelled: &AtomicBool) -> Reply {
            if self.closed.load(Ordering::Acquire) {
                return Reply::failed(wire::ESHUTDOWN);
            }
            if transfer.is_control() {
                return self.control(transfer, cancelled);
            }
            let address = transfer.endpoint | if transfer.direction == Direction::In { 0x80 } else { 0 };
            if transfer.is_isochronous() {
                return self.isochronous(address, transfer, cancelled);
            }
            let Some((entry, info)) = self.endpoint_entry(address) else {
                return Reply::failed(wire::EPIPE);
            };
            self.pipe_transfer(entry, address, info.max_packet, transfer, cancelled)
        }

        fn close(&self) {
            self.closed.store(true, Ordering::Release);
            self.state().handles.close_all(self.api);
        }
    }

    fn speed(speed: rusb::Speed) -> u32 {
        match speed {
            rusb::Speed::Low => 1,
            rusb::Speed::Full => 2,
            rusb::Speed::High => 3,
            rusb::Speed::Super => 5,
            rusb::Speed::SuperPlus => 6,
            _ => 0,
        }
    }

    /// Opens a Windows `WinUSB` device (and all its associated/composite interfaces) directly via
    /// inbox `winusb.dll`, without opening a conflicting libusb handle.
    pub(in crate::usb) fn open_device(
        device: &rusb::Device<rusb::Context>,
        busid: &str,
        vid: u16,
        pid: u16,
        paths: &[String],
    ) -> Result<Arc<dyn UsbDevice>, String> {
        let api = api()?;
        if paths.is_empty() {
            return Err("Windows listed no WinUSB interface path for this device.".to_owned());
        }
        let descriptor = device
            .device_descriptor()
            .map_err(|e| format!("The device's USB descriptor could not be read: {e}"))?;
        let config = device
            .active_config_descriptor()
            .map_err(|e| format!("The device's active USB configuration could not be read: {e}"))?;

        let mut handles = Handles {
            interfaces: HashMap::new(),
            files: Vec::new(),
            default_interface: 0,
        };

        let open_all = (|| -> Result<(), String> {
            for path in paths {
                let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
                // SAFETY: `wide` is NUL-terminated; file handle is stored in `handles.files` immediately.
                let file = unsafe {
                    CreateFileW(
                        PCWSTR(wide.as_ptr()),
                        GENERIC_READ.0 | GENERIC_WRITE.0,
                        FILE_SHARE_READ | FILE_SHARE_WRITE,
                        None,
                        OPEN_EXISTING,
                        FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED,
                        None,
                    )
                }
                .map_err(|e| format!("Windows refused to open the WinUSB device; another program may be using it ({e})."))?;
                handles.files.push(file);

                let mut base: WinUsbHandle = std::ptr::null_mut();
                // SAFETY: `file` is an open WinUSB device handle.
                if unsafe { (api.initialize)(file, &mut base) } == 0 || base.is_null() {
                    let err = unsafe { GetLastError().0 };
                    return Err(format!("WinUsb_Initialize failed with Win32 error {err}."));
                }
                let mut desc = UsbInterfaceDescriptor::default();
                let base_num = if unsafe { (api.query_settings)(base, 0, &mut desc) } != 0 {
                    desc.b_interface_number
                } else {
                    0
                };
                if handles.interfaces.is_empty() {
                    handles.default_interface = base_num;
                }
                handles.interfaces.insert(
                    base_num,
                    InterfaceEntry { winusb: base, file, associated: false },
                );

                // Claim every associated interface exposed under the same WinUSB device handle.
                for idx in 0u8..=254 {
                    let mut assoc: WinUsbHandle = std::ptr::null_mut();
                    if unsafe { (api.get_associated)(base, idx, &mut assoc) } == 0 || assoc.is_null() {
                        break;
                    }
                    let mut assoc_desc = UsbInterfaceDescriptor::default();
                    let num = if unsafe { (api.query_settings)(assoc, 0, &mut assoc_desc) } != 0 {
                        assoc_desc.b_interface_number
                    } else {
                        base_num.saturating_add(idx + 1)
                    };
                    handles.interfaces.insert(
                        num,
                        InterfaceEntry { winusb: assoc, file, associated: true },
                    );
                }
            }
            Ok(())
        })();

        if let Err(err) = open_all {
            handles.close_all(api);
            return Err(err);
        }

        let mut endpoints = HashMap::new();
        let mut alt_endpoints: HashMap<(u8, u8), Vec<(u8, u16)>> = HashMap::new();
        for interface in config.interfaces() {
            let num = interface.number();
            for setting in interface.descriptors() {
                let alt = setting.setting_number();
                let mut list = Vec::new();
                for ep in setting.endpoint_descriptors() {
                    let addr = ep.address();
                    let max_packet = ep.max_packet_size() & 0x7ff;
                    list.push((addr, max_packet));
                    if alt == 0 {
                        endpoints.insert(addr, EndpointInfo { interface: num, max_packet });
                    }
                }
                alt_endpoints.insert((num, alt), list);
            }
        }

        let class = match descriptor.class_code() {
            0 => config
                .interfaces()
                .next()
                .and_then(|i| i.descriptors().next())
                .map_or(0, |s| s.class_code()),
            class => class,
        };
        let version = descriptor.device_version();
        let bcd_device = (u16::from(version.0 / 10) << 12)
            | (u16::from(version.0 % 10) << 8)
            | (u16::from(version.1) << 4)
            | u16::from(version.2);
        let configuration = config.number();
        let described = wire::Device {
            path: format!("/winusb/{busid}"),
            busid: busid.to_owned(),
            busnum: u32::from(device.bus_number()),
            devnum: u32::from(device.address()),
            speed: speed(device.speed()),
            vendor: vid,
            product: pid,
            bcd_device,
            class: descriptor.class_code(),
            subclass: descriptor.sub_class_code(),
            protocol: descriptor.protocol_code(),
            configuration,
            configurations: descriptor.num_configurations(),
            interfaces: config.num_interfaces(),
        };

        let device = WinUsbDevice {
            api,
            described,
            class,
            closed: AtomicBool::new(false),
            state: Mutex::new(State {
                configuration,
                active: configuration,
                endpoints,
                alt_endpoints,
                streaming: HashMap::new(),
                handles,
            }),
        };
        Ok(Arc::new(Blocking::new(device)))
    }
}

#[cfg(windows)]
pub(super) use platform::open_device;
