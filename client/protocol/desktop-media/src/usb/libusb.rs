//! libusb-backed USB/IP device transport, standard control requests, and configuration state.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use nightjar_core::usb::{wire, Blocking, BlockingDevice, Direction, Reply, Transfer, UsbDevice};
use rusb::constants::{LIBUSB_ERROR_NO_DEVICE, LIBUSB_ERROR_OVERFLOW, LIBUSB_ERROR_PIPE, LIBUSB_ERROR_TIMEOUT, LIBUSB_SUCCESS};
use rusb::TransferType;

/// How long one bulk or interrupt call waits before the transfer looks at its cancel flag.
const SLICE: Duration = Duration::from_millis(200);
/// How long a control transfer may take; libusb's synchronous ones can't be cancelled.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);
const URB_ZERO_PACKET: u32 = 0x40;

/// Opens `device` through libusb, claims its active configuration's interfaces, and wraps it as a
/// [`UsbDevice`].
pub(super) fn open_device(
    device: &rusb::Device<rusb::Context>,
    busid: &str,
    vid: u16,
    pid: u16,
) -> rusb::Result<Arc<dyn UsbDevice>> {
    let descriptor = device.device_descriptor()?;
    let handle = device.open()?;
    // Linux: usb-storage, usbhid and the like let go while it is attached and take it back after.
    match handle.set_auto_detach_kernel_driver(true) {
        Ok(()) | Err(rusb::Error::NotSupported) => {}
        Err(e) => return Err(e),
    }
    let configuration = handle.active_configuration()?;
    let config = device.active_config_descriptor()?;
    let active_alt = AltConfig::from_descriptor(&config);
    let mut configs = HashMap::new();
    let mut alt_configs = HashMap::new();
    for index in 0..descriptor.num_configurations() {
        match device.config_descriptor(index) {
            Ok(c) => {
                let alt = AltConfig::from_descriptor(&c);
                configs.insert(
                    c.number(),
                    Config { interfaces: c.interfaces().map(|i| i.number()).collect(), endpoints: alt.initial_endpoints() },
                );
                alt_configs.insert(c.number(), alt);
            }
            Err(e) => eprintln!("nightjar: USB {busid}: configuration descriptor {index} could not be read, so the host can't set it: {e}"),
        }
    }
    alt_configs.entry(configuration).or_insert_with(|| active_alt.clone());
    let state = DeviceState {
        inner: State {
            configuration,
            active: configuration,
            claimed: Vec::new(),
            endpoints: active_alt.initial_endpoints(),
            configs,
            closed: false,
        },
        alt_configs,
        active_alts: HashMap::new(),
    };
    let class = match descriptor.class_code() {
        0 => config.interfaces().next().and_then(|i| i.descriptors().next()).map_or(0, |s| s.class_code()),
        class => class,
    };
    let version = descriptor.device_version();
    let bcd_device = (u16::from(version.0 / 10) << 12) | (u16::from(version.0 % 10) << 8) | (u16::from(version.1) << 4) | u16::from(version.2);
    let described = wire::Device {
        path: format!("/libusb/{busid}"),
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
    let opened = Libusb {
        handle,
        described,
        class,
        state: Mutex::new(state),
        closed: AtomicBool::new(false),
        warned_iso: AtomicBool::new(false),
    };
    let interfaces: Vec<u8> = config.interfaces().map(|i| i.number()).collect();
    claim(&opened.handle, &mut opened.state().inner, &interfaces)?;
    Ok(Arc::new(Blocking::new(opened)))
}

/// libusb's speed as USB/IP numbers it: 1 low, 2 full, 3 high, 5 super, 6 super+.
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

/// Endpoints indexed by `(interface, alt_setting)` so alternate settings never overwrite each other.
#[derive(Clone, Default)]
struct AltConfig {
    /// `(interface, alt_setting)` -> endpoint address -> `(TransferType, max_packet_size)`.
    settings: HashMap<(u8, u8), HashMap<u8, (TransferType, u16)>>,
    /// Default alternate setting per interface (0 when present, else lowest setting number).
    defaults: HashMap<u8, u8>,
}

impl AltConfig {
    fn from_descriptor(config: &rusb::ConfigDescriptor) -> Self {
        let mut out = Self::default();
        for interface in config.interfaces() {
            let iface = interface.number();
            for setting in interface.descriptors() {
                let alt = setting.setting_number();
                out.add_setting(
                    iface,
                    alt,
                    setting.endpoint_descriptors().map(|ep| (ep.address(), (ep.transfer_type(), ep.max_packet_size() & 0x7ff))),
                );
            }
        }
        out
    }

    fn add_setting(&mut self, interface: u8, alt_setting: u8, endpoints: impl IntoIterator<Item = (u8, (TransferType, u16))>) {
        let default = self.defaults.entry(interface).or_insert(alt_setting);
        if alt_setting < *default { *default = alt_setting; }
        let entry = self.settings.entry((interface, alt_setting)).or_default();
        entry.extend(endpoints);
    }

    /// Active endpoints for every interface under its selected (or default alt 0) setting.
    fn active_endpoints(&self, active_alts: &HashMap<u8, u8>) -> HashMap<u8, (TransferType, u16)> {
        let mut out = HashMap::new();
        for (&iface, &default_alt) in &self.defaults {
            let alt = active_alts.get(&iface).copied().unwrap_or(default_alt);
            if let Some(eps) = self.settings.get(&(iface, alt)) {
                out.extend(eps.iter().map(|(&addr, &info)| (addr, info)));
            }
        }
        out
    }

    /// Endpoints for the initial default alternate settings (alt 0 on each interface).
    fn initial_endpoints(&self) -> HashMap<u8, (TransferType, u16)> {
        self.active_endpoints(&HashMap::new())
    }
}

/// A standard request the driver sends that changes the device's state, which libusb has to
/// do itself (Windows refuses them as plain control transfers).
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Standard {
    SetConfiguration(u8),
    SetInterface { interface: u8, setting: u8 },
    ClearHalt(u8),
    /// SET_FEATURE(PORT_RESET) to the device's port, which the host's driver sends to recover
    /// it, as Linux's USB/IP host takes it: the device is reset here.
    PortReset,
}

pub(super) fn standard(setup: &[u8; 8]) -> Option<Standard> {
    let value = u16::from_le_bytes([setup[2], setup[3]]);
    let index = u16::from_le_bytes([setup[4], setup[5]]);
    match (setup[0], setup[1]) {
        (0x00, 9) => Some(Standard::SetConfiguration(value as u8)),
        (0x01, 11) => Some(Standard::SetInterface { interface: index as u8, setting: value as u8 }),
        (0x02, 1) if value == 0 => Some(Standard::ClearHalt(index as u8)),
        (0x23, 3) if value == 4 => Some(Standard::PortReset),
        _ => None,
    }
}

/// A libusb return code as USB/IP's negated errno.
pub(super) fn errno(code: i32) -> i32 {
    match code {
        LIBUSB_ERROR_PIPE => wire::EPIPE,
        LIBUSB_ERROR_NO_DEVICE => wire::ENODEV,
        LIBUSB_ERROR_OVERFLOW => wire::EOVERFLOW,
        LIBUSB_ERROR_TIMEOUT => wire::ETIMEDOUT,
        _ => wire::EPROTO,
    }
}

pub(super) fn rusb_errno(e: rusb::Error) -> i32 {
    match e {
        rusb::Error::Pipe => wire::EPIPE,
        rusb::Error::NoDevice => wire::ENODEV,
        rusb::Error::Overflow => wire::EOVERFLOW,
        rusb::Error::Timeout => wire::ETIMEDOUT,
        _ => wire::EPROTO,
    }
}

/// Reads up to `length` bytes in [`SLICE`]s until the transfer ends or is cancelled. A slice
/// that runs out has only read whole packets (a short one would have ended it), so the next
/// slice goes on where it stopped.
pub(super) fn read_sliced(length: usize, cancelled: &AtomicBool, mut call: impl FnMut(&mut [u8]) -> (i32, usize)) -> Reply {
    let mut buffer = vec![0; length];
    let mut at = 0;
    loop {
        if cancelled.load(Ordering::SeqCst) { return Reply::failed(wire::ECONNRESET); }
        let (code, n) = call(&mut buffer[at..]);
        at = (at + n).min(length);
        match code {
            LIBUSB_SUCCESS => break,
            LIBUSB_ERROR_TIMEOUT if at < length => {}
            LIBUSB_ERROR_TIMEOUT => break,
            code => return Reply::failed(errno(code)),
        }
    }
    buffer.truncate(at);
    Reply::read(buffer)
}

/// Sends `data` in [`SLICE`]s, then a zero-length packet when `zero_packet` asks for one. An
/// empty transfer is a zero-length packet itself.
pub(super) fn write_sliced(data: &[u8], zero_packet: bool, cancelled: &AtomicBool, mut call: impl FnMut(&[u8]) -> (i32, usize)) -> Reply {
    let mut at = 0;
    let mut zero = zero_packet || data.is_empty();
    while at < data.len() || zero {
        if cancelled.load(Ordering::SeqCst) { return Reply::failed(wire::ECONNRESET); }
        let (code, n) = call(&data[at..]);
        at = (at + n).min(data.len());
        match code {
            LIBUSB_SUCCESS if at >= data.len() && zero && n == 0 => zero = false,
            LIBUSB_SUCCESS | LIBUSB_ERROR_TIMEOUT => {}
            code => return Reply::failed(errno(code)),
        }
    }
    Reply::sent(at as u32)
}

/// One of the device's configurations: its interfaces and endpoints.
#[derive(Clone)]
pub(super) struct Config {
    pub(super) interfaces: Vec<u8>,
    pub(super) endpoints: HashMap<u8, (TransferType, u16)>,
}

pub(super) struct State {
    /// The configuration the host sees; 0 after it unconfigured the device.
    pub(super) configuration: u8,
    /// The configuration the device is in: unconfiguring only hides it (see [`set_configuration`]).
    pub(super) active: u8,
    pub(super) claimed: Vec<u8>,
    pub(super) endpoints: HashMap<u8, (TransferType, u16)>,
    pub(super) configs: HashMap<u8, Config>,
    pub(super) closed: bool,
}

/// What changing the configuration and resetting the device need of a libusb handle.
pub(super) trait Configure {
    fn claim_interface(&self, interface: u8) -> rusb::Result<()>;
    fn release_interface(&self, interface: u8) -> rusb::Result<()>;
    fn set_auto_detach_kernel_driver(&self, on: bool) -> rusb::Result<()>;
    fn set_active_configuration(&self, value: u8) -> rusb::Result<()>;
    fn reset(&self) -> rusb::Result<()>;
}

impl Configure for rusb::DeviceHandle<rusb::Context> {
    fn claim_interface(&self, interface: u8) -> rusb::Result<()> {
        rusb::DeviceHandle::claim_interface(self, interface)
    }
    fn release_interface(&self, interface: u8) -> rusb::Result<()> {
        rusb::DeviceHandle::release_interface(self, interface)
    }
    fn set_auto_detach_kernel_driver(&self, on: bool) -> rusb::Result<()> {
        rusb::DeviceHandle::set_auto_detach_kernel_driver(self, on)
    }
    fn set_active_configuration(&self, value: u8) -> rusb::Result<()> {
        rusb::DeviceHandle::set_active_configuration(self, value)
    }
    fn reset(&self) -> rusb::Result<()> {
        rusb::DeviceHandle::reset(self)
    }
}

/// Claims each of `interfaces` not claimed yet; on a failure lets go of every one.
fn claim(handle: &impl Configure, state: &mut State, interfaces: &[u8]) -> rusb::Result<()> {
    for &interface in interfaces {
        if state.claimed.contains(&interface) { continue; }
        if let Err(e) = handle.claim_interface(interface) {
            for &claimed in &state.claimed { let _ = handle.release_interface(claimed); }
            state.claimed.clear();
            return Err(e);
        }
        state.claimed.push(interface);
    }
    Ok(())
}

fn release(handle: &impl Configure, state: &mut State, busid: &str) {
    for interface in std::mem::take(&mut state.claimed) {
        if let Err(e) = handle.release_interface(interface) {
            eprintln!("nightjar: USB {busid}: releasing interface {interface} failed: {e}");
        }
    }
}

/// libusb puts the kernel's drivers back on interfaces it lets go of (Linux); `on` false keeps
/// them off.
fn auto_detach(handle: &impl Configure, on: bool, busid: &str) {
    match handle.set_auto_detach_kernel_driver(on) {
        Ok(()) | Err(rusb::Error::NotSupported) => {}
        Err(e) => eprintln!("nightjar: USB {busid}: turning the kernel driver's detach {} failed: {e}", if on { "on" } else { "off" }),
    }
}

/// SET_CONFIGURATION from the host. Unconfiguring (0) only hides the configuration until the
/// host sets one again: WinUSB can't unconfigure, and Linux won't while interfaces are
/// claimed. A real change lets go of the old interfaces without giving them back to the
/// kernel's drivers (Linux refuses the change while a driver is bound), and when the device
/// refuses it claims them again, so the old configuration still works.
pub(super) fn set_configuration(handle: &impl Configure, state: &mut State, value: u8, busid: &str) -> Reply {
    if value == state.configuration { return Reply::sent(0); }
    if value == 0 {
        state.configuration = 0;
        state.endpoints.clear();
        return Reply::sent(0);
    }
    let Some(config) = state.configs.get(&value).cloned() else { return Reply::failed(wire::EINVAL) };
    if value != state.active {
        auto_detach(handle, false, busid);
        let old = state.claimed.clone();
        release(handle, state, busid);
        let set = handle.set_active_configuration(value);
        auto_detach(handle, true, busid);
        if let Err(e) = set {
            eprintln!("nightjar: USB {busid}: configuration {value} failed: {e}; staying in {}", state.active);
            if let Err(again) = claim(handle, state, &old) {
                eprintln!("nightjar: USB {busid}: configuration {}'s interfaces could not be claimed again: {again}", state.active);
            }
            return Reply::failed(rusb_errno(e));
        }
        state.active = value;
    }
    state.configuration = value;
    state.endpoints = config.endpoints;
    match claim(handle, state, &config.interfaces) {
        Ok(()) => Reply::sent(0),
        Err(e) => Reply::failed(rusb_errno(e)),
    }
}

/// A port reset from the host. libusb claims the interfaces again after a reset; claiming
/// them here too (a no-op for one claimed already) checks that it did.
pub(super) fn reset_port(handle: &impl Configure, state: &mut State, busid: &str) -> Reply {
    match handle.reset() {
        Ok(()) => {}
        // The device came back as another one, which this handle no longer reaches.
        Err(rusb::Error::NotFound | rusb::Error::NoDevice) => {
            eprintln!("nightjar: USB {busid}: the device went away when it was reset");
            return Reply::failed(wire::ENODEV);
        }
        Err(e) => {
            eprintln!("nightjar: USB {busid}: resetting the device failed: {e}");
            return Reply::failed(rusb_errno(e));
        }
    }
    let interfaces = state.configs.get(&state.active).map(|c| c.interfaces.clone()).unwrap_or_default();
    state.claimed.clear();
    match claim(handle, state, &interfaces) {
        Ok(()) => Reply::sent(0),
        Err(e) => {
            eprintln!("nightjar: USB {busid}: the interfaces could not be claimed again after a reset: {e}");
            Reply::failed(rusb_errno(e))
        }
    }
}

/// Full libusb device state: base configuration/claims plus per-`(interface, alt_setting)` endpoints.
struct DeviceState {
    inner: State,
    alt_configs: HashMap<u8, AltConfig>,
    active_alts: HashMap<u8, u8>,
}

/// Alternate-setting selection on a libusb handle (kept separate from [`Configure`] for test fakes).
trait SetInterface {
    fn set_alternate_setting(&self, interface: u8, setting: u8) -> rusb::Result<()>;
}

impl SetInterface for rusb::DeviceHandle<rusb::Context> {
    fn set_alternate_setting(&self, interface: u8, setting: u8) -> rusb::Result<()> {
        rusb::DeviceHandle::set_alternate_setting(self, interface, setting)
    }
}

/// SET_INTERFACE from the host: switches `interface` to `setting` and refreshes `state.inner.endpoints`
/// on success so transfers use the newly selected alternate setting's transfer type and packet size.
fn set_interface(handle: &impl SetInterface, state: &mut DeviceState, interface: u8, setting: u8) -> Reply {
    if let Err(e) = handle.set_alternate_setting(interface, setting) {
        return Reply::failed(rusb_errno(e));
    }
    state.active_alts.insert(interface, setting);
    let cfg = state.inner.configuration;
    if let Some(alt_config) = state.alt_configs.get(&cfg) {
        state.inner.endpoints = alt_config.active_endpoints(&state.active_alts);
    }
    Reply::sent(0)
}

/// Marks the device closed and releases all claimed interfaces once (idempotent across `close` and `Drop`).
fn close_state(handle: &impl Configure, state: &mut State, busid: &str) {
    state.closed = true;
    release(handle, state, busid);
}

struct Libusb {
    handle: rusb::DeviceHandle<rusb::Context>,
    described: wire::Device,
    class: u8,
    state: Mutex<DeviceState>,
    closed: AtomicBool,
    warned_iso: AtomicBool,
}

impl Libusb {
    fn state(&self) -> MutexGuard<'_, DeviceState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn close_inner(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let mut state = self.state();
        close_state(&self.handle, &mut state.inner, &self.described.busid);
    }

    fn control(&self, transfer: &Transfer) -> Reply {
        let setup = &transfer.setup;
        let done = |result: rusb::Result<()>| result.map_or_else(|e| Reply::failed(rusb_errno(e)), |()| Reply::sent(0));
        let busid = &self.described.busid;
        match standard(setup) {
            Some(Standard::SetConfiguration(value)) => {
                let mut state = self.state();
                let prev = state.inner.configuration;
                let reply = set_configuration(&self.handle, &mut state.inner, value, busid);
                if reply.status == 0 && value != prev {
                    state.active_alts.clear();
                }
                return reply;
            }
            Some(Standard::PortReset) => {
                let mut state = self.state();
                let reply = reset_port(&self.handle, &mut state.inner, busid);
                if reply.status == 0 {
                    state.active_alts.clear();
                    let cfg = state.inner.configuration;
                    if let Some(alt_config) = state.alt_configs.get(&cfg) {
                        state.inner.endpoints = alt_config.initial_endpoints();
                    }
                }
                return reply;
            }
            Some(Standard::SetInterface { interface, setting }) => {
                return set_interface(&self.handle, &mut self.state(), interface, setting);
            }
            Some(Standard::ClearHalt(endpoint)) => return done(self.handle.clear_halt(endpoint)),
            None => {}
        }
        let (request_type, request) = (setup[0], setup[1]);
        let value = u16::from_le_bytes([setup[2], setup[3]]);
        let index = u16::from_le_bytes([setup[4], setup[5]]);
        let length = usize::from(u16::from_le_bytes([setup[6], setup[7]]));
        if request_type & 0x80 != 0 {
            let mut buffer = vec![0; length.min(transfer.length as usize)];
            match self.handle.read_control(request_type, request, value, index, &mut buffer, CONTROL_TIMEOUT) {
                Ok(n) => {
                    buffer.truncate(n);
                    Reply::read(buffer)
                }
                Err(e) => Reply::failed(rusb_errno(e)),
            }
        } else {
            match self.handle.write_control(request_type, request, value, index, &transfer.data, CONTROL_TIMEOUT) {
                Ok(n) => Reply::sent(n as u32),
                Err(e) => Reply::failed(rusb_errno(e)),
            }
        }
    }

    /// One bulk or interrupt call of at most [`SLICE`]: libusb's code and the bytes moved.
    fn call(&self, address: u8, kind: TransferType, data: *mut u8, length: usize) -> (i32, usize) {
        let Ok(length) = i32::try_from(length) else { return (rusb::constants::LIBUSB_ERROR_INVALID_PARAM, 0) };
        let mut moved = 0;
        let run = if kind == TransferType::Interrupt { rusb::ffi::libusb_interrupt_transfer } else { rusb::ffi::libusb_bulk_transfer };
        // SAFETY: the handle is open while self lives; `data` holds `length` bytes, which libusb
        // only reads for an OUT endpoint.
        let code = unsafe { run(self.handle.as_raw(), address, data, length, &mut moved, SLICE.as_millis() as u32) };
        (code, usize::try_from(moved).unwrap_or(0))
    }
}

impl Drop for Libusb {
    /// Releases all claimed interfaces before `self.handle` drops (`libusb_close`), so Linux's
    /// auto-detached kernel drivers reattach even when dropped without an explicit `close()`.
    fn drop(&mut self) {
        self.close_inner();
    }
}

impl BlockingDevice for Libusb {
    fn describe(&self) -> wire::Device {
        wire::Device { configuration: self.state().inner.configuration, ..self.described.clone() }
    }

    fn class(&self) -> u8 {
        self.class
    }

    fn transfer(&self, transfer: &Transfer, cancelled: &AtomicBool) -> Reply {
        if self.state().inner.closed { return Reply::failed(wire::ESHUTDOWN); }
        if transfer.is_control() { return self.control(transfer); }
        let address = transfer.endpoint | if transfer.direction == Direction::In { 0x80 } else { 0 };
        if transfer.is_isochronous() {
            if !self.state().inner.endpoints.contains_key(&address) {
                return Reply::failed(wire::EPIPE);
            }
            return super::iso::libusb_iso_transfer(
                &self.handle,
                &self.described.busid,
                address,
                transfer,
                cancelled,
                &self.closed,
                &self.warned_iso,
            );
        }
        let Some((kind, max_packet)) = self.state().inner.endpoints.get(&address).copied() else { return Reply::failed(wire::EPIPE) };
        match transfer.direction {
            Direction::In => read_sliced(transfer.length as usize, cancelled, |buffer| self.call(address, kind, buffer.as_mut_ptr(), buffer.len())),
            Direction::Out => {
                let data = &transfer.data;
                let zero = transfer.flags & URB_ZERO_PACKET != 0 && !data.is_empty() && max_packet > 0 && data.len().is_multiple_of(usize::from(max_packet));
                write_sliced(data, zero, cancelled, |rest| self.call(address, kind, rest.as_ptr().cast_mut(), rest.len()))
            }
        }
    }

    fn close(&self) {
        self.close_inner();
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    struct FakeAltHandle {
        result: rusb::Result<()>,
    }

    impl SetInterface for FakeAltHandle {
        fn set_alternate_setting(&self, _interface: u8, _setting: u8) -> rusb::Result<()> {
            self.result
        }
    }

    #[derive(Default)]
    struct ReleaseLog(std::cell::RefCell<Vec<u8>>);

    impl Configure for ReleaseLog {
        fn claim_interface(&self, _interface: u8) -> rusb::Result<()> { Ok(()) }
        fn release_interface(&self, interface: u8) -> rusb::Result<()> { self.0.borrow_mut().push(interface); Ok(()) }
        fn set_auto_detach_kernel_driver(&self, _on: bool) -> rusb::Result<()> { Ok(()) }
        fn set_active_configuration(&self, _value: u8) -> rusb::Result<()> { Ok(()) }
        fn reset(&self) -> rusb::Result<()> { Ok(()) }
    }

    #[test]
    fn multi_alt_setting_initializes_from_alt_0_and_refreshes_on_set_interface() {
        let mut alt = AltConfig::default();
        // Interface 0: alt 0 has EP 0x81 Interrupt(8); alt 1 changes EP 0x81 to Bulk(512) and adds EP 0x02 Bulk(512).
        alt.add_setting(0, 0, [(0x81, (TransferType::Interrupt, 8))]);
        alt.add_setting(0, 1, [(0x81, (TransferType::Bulk, 512)), (0x02, (TransferType::Bulk, 512))]);
        // Interface 1: alt 0 has no endpoints (zero-bandwidth), alt 1 adds EP 0x83 Interrupt(64).
        alt.add_setting(1, 0, []);
        alt.add_setting(1, 1, [(0x83, (TransferType::Interrupt, 64))]);

        let initial = alt.initial_endpoints();
        assert_eq!(initial.get(&0x81), Some(&(TransferType::Interrupt, 8)), "alt 1 must not overwrite alt 0");
        assert!(!initial.contains_key(&0x02));
        assert!(!initial.contains_key(&0x83));

        let config = Config { interfaces: vec![0, 1], endpoints: initial.clone() };
        let mut dev = DeviceState {
            inner: State {
                configuration: 1,
                active: 1,
                claimed: vec![0, 1],
                endpoints: initial,
                configs: HashMap::from([(1, config)]),
                closed: false,
            },
            alt_configs: HashMap::from([(1, alt)]),
            active_alts: HashMap::new(),
        };

        // Switch interface 0 to alt 1: EP 0x81 becomes Bulk(512) and EP 0x02 appears.
        let ok = FakeAltHandle { result: Ok(()) };
        assert_eq!(set_interface(&ok, &mut dev, 0, 1).status, 0);
        assert_eq!(dev.inner.endpoints.get(&0x81), Some(&(TransferType::Bulk, 512)));
        assert_eq!(dev.inner.endpoints.get(&0x02), Some(&(TransferType::Bulk, 512)));
        assert!(!dev.inner.endpoints.contains_key(&0x83));

        // Switch interface 1 to alt 1: EP 0x83 appears while interface 0 stays on alt 1.
        assert_eq!(set_interface(&ok, &mut dev, 1, 1).status, 0);
        assert_eq!(dev.inner.endpoints.get(&0x81), Some(&(TransferType::Bulk, 512)));
        assert_eq!(dev.inner.endpoints.get(&0x83), Some(&(TransferType::Interrupt, 64)));

        // Failed SetInterface must leave active endpoints unchanged.
        let fail = FakeAltHandle { result: Err(rusb::Error::Pipe) };
        assert_eq!(set_interface(&fail, &mut dev, 0, 0).status, wire::EPIPE);
        assert_eq!(dev.inner.endpoints.get(&0x81), Some(&(TransferType::Bulk, 512)));

        // Switching interface 0 back to alt 0 restores Interrupt(8) and drops EP 0x02.
        assert_eq!(set_interface(&ok, &mut dev, 0, 0).status, 0);
        assert_eq!(dev.inner.endpoints.get(&0x81), Some(&(TransferType::Interrupt, 8)));
        assert!(!dev.inner.endpoints.contains_key(&0x02));
        assert_eq!(dev.inner.endpoints.get(&0x83), Some(&(TransferType::Interrupt, 64)));
    }

    #[test]
    fn close_state_releases_claimed_interfaces_once_for_close_and_drop() {
        let log = ReleaseLog::default();
        let mut state = State {
            configuration: 1,
            active: 1,
            claimed: vec![0, 1, 2],
            endpoints: HashMap::new(),
            configs: HashMap::new(),
            closed: false,
        };
        close_state(&log, &mut state, "1-2");
        assert!(state.closed);
        assert!(state.claimed.is_empty());
        assert_eq!(*log.0.borrow(), [0, 1, 2]);
        // Second call (e.g. Drop after close()) must be a no-op.
        close_state(&log, &mut state, "1-2");
        assert_eq!(*log.0.borrow(), [0, 1, 2]);
    }
}
