//! Shared timing, load, XInput, HID discovery, and injector helpers for `pad_feedback_probe`.

use anyhow::{Context, Result, ensure};
use butterpollo_input_policy::input::Input;
use butterpollo_win_io::input::{Injector, PadReport};
use std::{
    io::{Read, Write},
    net::TcpStream,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        Storage::FileSystem::*,
        System::{Diagnostics::ToolHelp::*, Threading::*},
    },
    core::PCWSTR,
};

pub(crate) const LEVELS: [u16; 15] = [
    0x1000, 0x2000, 0x3000, 0x4000, 0x5000, 0x6000, 0x7000, 0x8000, 0x9000, 0xa000, 0xb000, 0xc000,
    0xd000, 0xe000, 0xf000,
];

const HID_INTERFACE: windows::core::GUID =
    windows::core::GUID::from_u128(0x4d1e55b2_f16f_11cf_88cb_001111000030);

pub(crate) fn stats(name: &str, mut v: Vec<f64>) {
    if v.is_empty() {
        println!("{name:52} no samples");
        return;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    let mean = v.iter().sum::<f64>() / n as f64;
    println!(
        "{name:52} n={n:5} mean {mean:8.1} p50 {:8.1} p90 {:8.1} p99 {:8.1} max {:9.1} us",
        v[n / 2],
        v[n * 90 / 100],
        v[n * 99 / 100],
        v[n - 1]
    );
}

pub(crate) struct Load(Option<std::process::Child>);
impl Load {
    /// input_path_probe's spinners: one thread per logical CPU at the named
    /// thread priority in a normal-class child.
    pub(crate) fn start(kind: &str) -> Self {
        if kind == "none" {
            return Self(None);
        }
        let probe = std::env::current_exe()
            .unwrap()
            .with_file_name("input_path_probe.exe");
        let child = std::process::Command::new(probe)
            .arg(format!("--spin={kind}"))
            .spawn()
            .expect("input_path_probe.exe beside this probe");
        std::thread::sleep(Duration::from_millis(300));
        Self(Some(child))
    }
}
impl Drop for Load {
    fn drop(&mut self) {
        if let Some(c) = &mut self.0 {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

pub(crate) fn idle_host() -> Result<()> {
    let timeout = Duration::from_secs(2);
    let mut stream = TcpStream::connect_timeout(&"127.0.0.1:47989".parse()?, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream
        .write_all(b"GET /serverinfo HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    for name in ["RustHostSessionCount", "RustHostPendingSessionCount"] {
        let value = response
            .split_once(&format!("<{name}>"))
            .and_then(|(_, tail)| tail.split_once(&format!("</{name}>")))
            .map(|(value, _)| value);
        ensure!(value == Some("0"), "host is busy: {name}={value:?}");
    }
    Ok(())
}

pub(crate) struct XInput {
    _library: libloading::Library,
    get: unsafe extern "system" fn(u32, *mut [u16; 8]) -> u32,
    set: unsafe extern "system" fn(u32, *const [u16; 2]) -> u32,
}
impl XInput {
    pub(crate) fn load() -> Result<Self> {
        let system = std::env::var("SystemRoot").context("SystemRoot")?;
        // SAFETY: xinput1_4 from System32 has no loader requirements; the
        // signatures match XInputGetState (16-byte XINPUT_STATE) and
        // XInputSetState (two-u16 XINPUT_VIBRATION), and the library is kept.
        unsafe {
            let library = libloading::Library::new(format!("{system}\\System32\\xinput1_4.dll"))?;
            let get = *library.get(b"XInputGetState\0")?;
            let set = *library.get(b"XInputSetState\0")?;
            Ok(Self {
                _library: library,
                get,
                set,
            })
        }
    }
    /// (packet number, buttons) of a connected pad.
    pub(crate) fn state(&self, index: u32) -> Option<(u32, u16)> {
        let mut state = [0u16; 8];
        // SAFETY: index is in 0..4 and state has XINPUT_STATE's size and alignment.
        (unsafe { (self.get)(index, &mut state) } == 0)
            .then(|| (u32::from(state[0]) | u32::from(state[1]) << 16, state[2]))
    }
    pub(crate) fn connected(&self) -> Vec<u32> {
        (0..4).filter(|&i| self.state(i).is_some()).collect()
    }
    /// XInputSetState's result: 0, or a Win32 error.
    pub(crate) fn vibrate(&self, index: u32, low: u16, high: u16) -> u32 {
        // SAFETY: as in state; the vibration lives through the call.
        unsafe { (self.set)(index, &[low, high]) }
    }
}

/// CPU time of this process's threads with the given name, in ms.
pub(crate) fn thread_cpu_ms(name: &str) -> f64 {
    let mut total = 0u64;
    // SAFETY: the snapshot and thread handles are closed here; the out
    // structures are sized and initialized.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) else {
            return f64::NAN;
        };
        let mut entry = THREADENTRY32 {
            dwSize: size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        let pid = GetCurrentProcessId();
        let mut more = Thread32First(snapshot, &mut entry).is_ok();
        while more {
            if entry.th32OwnerProcessID == pid
                && let Ok(thread) =
                    OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, entry.th32ThreadID)
            {
                if let Ok(description) = GetThreadDescription(thread)
                    && description.to_string().unwrap_or_default() == name
                {
                    let (mut c, mut e, mut k, mut u) = Default::default();
                    if GetThreadTimes(thread, &mut c, &mut e, &mut k, &mut u).is_ok() {
                        let ticks = |t: FILETIME| {
                            u64::from(t.dwHighDateTime) << 32 | u64::from(t.dwLowDateTime)
                        };
                        total += ticks(k) + ticks(u);
                    }
                    let _ =
                        windows::Win32::Foundation::LocalFree(Some(HLOCAL(description.0.cast())));
                }
                let _ = CloseHandle(thread);
            }
            more = Thread32Next(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }
    total as f64 / 1e4
}

pub(crate) fn controller(buttons: u32) -> Input {
    Input::Controller {
        id: 0,
        active: 1,
        buttons,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    }
}

/// The injector with the probe's switches, its pad plugged and on XInput.
pub(crate) fn plug(pad: &str, args: &[String], xinput: &XInput) -> Result<(Injector, u32)> {
    idle_host()?;
    ensure!(
        xinput.connected().is_empty(),
        "an XInput pad is already connected; refusing to write vibration"
    );
    let switch = |flag: &str| args.iter().any(|a| a == flag);
    let config = butterpollo_config::config::Config::parse(&format!(
        "gamepad_feedback_fast_poll = {}\ngamepad_driver_host_priority = {}\n",
        switch("--fast-poll"),
        switch("--driver-host-priority")
    ))?;
    let profile = if pad == "vigem" {
        "vigem_x360"
    } else {
        "vhf_xbox"
    };
    let mut injector = Injector::new_options("", profile, &config)?;
    injector.apply(&controller(0))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let index = loop {
        let found = xinput.connected();
        ensure!(found.len() <= 1, "more than one new XInput pad");
        if let Some(&i) = found.first() {
            break i;
        }
        ensure!(Instant::now() < deadline, "the pad did not reach XInput");
        std::thread::sleep(Duration::from_millis(20));
    };
    // Let PnP, HID class and readers (Steam, GameInput) settle.
    std::thread::sleep(Duration::from_secs(2));
    println!(
        "{profile} on XInput {index}; fast poll {}, driver host priority {}",
        switch("--fast-poll"),
        switch("--driver-host-priority")
    );
    Ok((injector, index))
}

pub(crate) fn unplug(injector: Injector, xinput: &XInput) {
    drop(injector);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !xinput.connected().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    println!("pad removed: {}", xinput.connected().is_empty());
}

pub(crate) fn spin_until(at: Instant) {
    while Instant::now() < at {
        std::hint::spin_loop();
    }
}

/// Rumble a report carries: Xbox reports (kind 4) start with the low and high
/// motors, little-endian, as HID rumble reports (kind 5) do.
pub(crate) fn rumble(report: &PadReport) -> Option<(u16, u16)> {
    match report {
        PadReport::Feedback { data, .. } if data.len() >= 4 => Some((
            u16::from_le_bytes([data[0], data[1]]),
            u16::from_le_bytes([data[2], data[3]]),
        )),
        _ => None,
    }
}

/// Collect the injector's pad reports with their arrival time until `until`,
/// refreshing every 8 ms as the control loop does.
pub(crate) fn collect(
    injector: &mut Injector,
    until: Instant,
    refresh_at: &mut Instant,
    out: &mut Vec<(Instant, PadReport)>,
) {
    while Instant::now() < until {
        if Instant::now() >= *refresh_at {
            *refresh_at = Instant::now() + Duration::from_millis(8);
            let _ = injector.refresh();
        }
        for report in injector.gamepad_reports() {
            out.push((Instant::now(), report));
        }
        spin_until(Instant::now() + Duration::from_micros(100));
    }
}

pub(crate) fn hid_paths() -> Result<std::collections::BTreeSet<String>> {
    use windows::Win32::Devices::DeviceAndDriverInstallation::*;
    // SAFETY: as native_touch_tests::hid_paths: sized SetupAPI structures and
    // a u64-aligned detail buffer of the queried size; the set is destroyed.
    unsafe {
        let set = SetupDiGetClassDevsW(
            Some(&HID_INTERFACE),
            PCWSTR::null(),
            None,
            DIGCF_DEVICEINTERFACE | DIGCF_PRESENT,
        )?;
        let mut paths = std::collections::BTreeSet::new();
        for index in 0.. {
            let mut interface = SP_DEVICE_INTERFACE_DATA {
                cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
                ..Default::default()
            };
            if SetupDiEnumDeviceInterfaces(set, None, &HID_INTERFACE, index, &mut interface)
                .is_err()
            {
                break;
            }
            let mut size = 0;
            let _ =
                SetupDiGetDeviceInterfaceDetailW(set, &interface, None, 0, Some(&mut size), None);
            if !(8..=65536).contains(&size) {
                continue;
            }
            let mut data = vec![0u64; (size as usize).div_ceil(8)];
            let detail = data.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            if SetupDiGetDeviceInterfaceDetailW(set, &interface, Some(detail), size, None, None)
                .is_ok()
            {
                paths.insert(
                    PCWSTR((*detail).DevicePath.as_ptr())
                        .to_string()?
                        .to_lowercase(),
                );
            }
        }
        let _ = SetupDiDestroyDeviceInfoList(set);
        Ok(paths)
    }
}

/// A new HID interface with the vendor and product, opened for writing.
pub(crate) fn new_hid(
    before: &std::collections::BTreeSet<String>,
    vendor: u16,
    product: u16,
) -> Result<HANDLE> {
    let system = std::env::var("SystemRoot").context("SystemRoot")?;
    // SAFETY: hid.dll from System32; HidD_GetAttributes takes HIDD_ATTRIBUTES,
    // which [u16; 6] matches in size and alignment (u32 size, then three u16).
    let library = unsafe { libloading::Library::new(format!("{system}\\System32\\hid.dll"))? };
    let get: libloading::Symbol<unsafe extern "system" fn(HANDLE, *mut [u16; 6]) -> u8> =
        unsafe { library.get(b"HidD_GetAttributes\0")? };
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        for path in hid_paths()?.difference(before) {
            let wide: Vec<_> = path.encode_utf16().chain([0]).collect();
            // SAFETY: a terminated path; the handle is closed unless returned.
            unsafe {
                let Ok(handle) = CreateFileW(
                    PCWSTR(wide.as_ptr()),
                    (GENERIC_READ | GENERIC_WRITE).0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    None,
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    None,
                ) else {
                    continue;
                };
                let mut attributes = [0u16; 6];
                attributes[0] = 12;
                if get(handle, &mut attributes) != 0
                    && attributes[2] == vendor
                    && attributes[3] == product
                {
                    return Ok(handle);
                }
                let _ = CloseHandle(handle);
            }
        }
        ensure!(
            Instant::now() < deadline,
            "the DualSense did not appear as HID"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}
