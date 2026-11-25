//! Monitor identity, registry EDID lookups, SetupAPI instance paths, and virtual/physical predicates.

use super::super::*;
use super::Topology;

fn path_parts(path: &str) -> Option<(&str, &str)> {
    let mut parts = path.split('#');
    let _ = parts.next()?;
    let hardware = parts.next()?;
    let instance = parts.next()?;
    Some((hardware, instance))
}

fn monitor_edid(path: &str) -> Vec<u8> {
    let Some((hardware, instance)) = path_parts(path) else {
        return Vec::new();
    };
    if hardware.contains(['\\', '/']) || instance.contains(['\\', '/']) {
        return Vec::new();
    }
    let key: Vec<u16> = format!(
        "SYSTEM\\CurrentControlSet\\Enum\\DISPLAY\\{hardware}\\{instance}\\Device Parameters\0"
    )
    .encode_utf16()
    .collect();
    // SAFETY: The registry path is terminated and the byte buffer is sized from the registry query.
    unsafe {
        use windows::Win32::System::Registry::*;
        let mut size = 0;
        if RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key.as_ptr()),
            windows::core::w!("EDID"),
            RRF_RT_REG_BINARY,
            None,
            None,
            Some(&mut size),
        )
        .is_err()
            || size > 65536
        {
            return Vec::new();
        }
        let mut bytes = vec![0; size as usize];
        if RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key.as_ptr()),
            windows::core::w!("EDID"),
            RRF_RT_REG_BINARY,
            None,
            Some(bytes.as_mut_ptr().cast()),
            Some(&mut size),
        )
        .is_err()
        {
            return Vec::new();
        }
        bytes.truncate(size as usize);
        bytes
    }
}
fn monitor_instance(path: &str) -> Result<String> {
    use windows::Win32::Devices::DeviceAndDriverInstallation::*;
    let guid = GUID::from_u128(0xe6f07b5f_ee97_4a90_b076_33f57bf4eaa7);
    // SAFETY: The SetupAPI structures have their required sizes; aligned buffers and terminated strings live through each call, and the device set is destroyed afterwards.
    unsafe {
        let set = SetupDiGetClassDevsW(Some(&guid), PCWSTR::null(), None, DIGCF_DEVICEINTERFACE)?;
        let result = (|| -> Result<String> {
            let path: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
            let mut interface = SP_DEVICE_INTERFACE_DATA {
                cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
                ..Default::default()
            };
            SetupDiOpenDeviceInterfaceW(set, PCWSTR(path.as_ptr()), 0, Some(&mut interface))?;
            let mut required = 0;
            let mut device = SP_DEVINFO_DATA {
                cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
                ..Default::default()
            };
            let _ = SetupDiGetDeviceInterfaceDetailW(
                set,
                &interface,
                None,
                0,
                Some(&mut required),
                None,
            );
            if !(8..=65536).contains(&required) {
                bail!("invalid monitor interface size");
            }
            let mut buffer = vec![0u64; (required as usize).div_ceil(8)];
            let detail = buffer
                .as_mut_ptr()
                .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
            (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            SetupDiGetDeviceInterfaceDetailW(
                set,
                &interface,
                Some(detail),
                required,
                None,
                Some(&mut device),
            )?;
            let _ = SetupDiGetDeviceInstanceIdW(set, &device, None, Some(&mut required));
            if required == 0 || required > 32768 {
                bail!("invalid monitor instance size");
            }
            let mut instance = vec![0u16; required as usize];
            SetupDiGetDeviceInstanceIdW(set, &device, Some(&mut instance), None)?;
            Ok(wide(&instance))
        })();
        let _ = SetupDiDestroyDeviceInfoList(set);
        result
    }
}
/// An adapter and target id a display can be listed under.
pub(in super::super) type Route = (LUID, u32);
/// Whether a monitor device path, `\\?\DISPLAY#RBY0001#1&28a6823a&0&UID258#{guid}`,
/// belongs to a Rubylight virtual display that IddCx gave target id `target`:
/// the maker code is RBY and the instance ends in the target id as its UID.
pub(in super::super) fn is_virtual_monitor(path: &str, target: u32) -> bool {
    let Some((hardware, instance)) = path_parts(path) else {
        return false;
    };
    hardware
        .get(..3)
        .is_some_and(|m| m.eq_ignore_ascii_case("RBY"))
        && instance
            .to_ascii_lowercase()
            .ends_with(&format!("&uid{target}"))
}
/// Whether a monitor device path names a Rubylight virtual display (maker RBY).
pub(in super::super) fn is_rubylight_monitor(path: &str) -> bool {
    path_parts(path)
        .and_then(|(hardware, _)| hardware.get(..3))
        .is_some_and(|maker| maker.eq_ignore_ascii_case("RBY"))
}
/// Whether a monitor device path can be read and names another maker's display.
pub(in super::super) fn is_physical_monitor(path: &str) -> bool {
    path_parts(path).is_some() && !is_rubylight_monitor(path)
}
static IDS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::BTreeMap<String, (Instant, String)>>,
> = std::sync::LazyLock::new(Default::default);

pub(super) fn monitor_id(path: &str) -> String {
    monitor_id_with(path, |p| monitor_instance(p).ok(), monitor_edid)
}

fn monitor_id_with(
    path: &str,
    probe_instance: impl FnOnce(&str) -> Option<String>,
    probe_edid: impl FnOnce(&str) -> Vec<u8>,
) -> String {
    {
        let mut ids = crate::lock_unpoisoned(&IDS);
        ids.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(1));
        if let Some((_, id)) = ids.get(path) {
            return id.clone();
        }
    }
    let id = butterpollo_media_policy::display_policy::legacy_device_id(
        path,
        probe_instance(path).as_deref(),
        &probe_edid(path),
    );
    let mut ids = crate::lock_unpoisoned(&IDS);
    if ids.len() < 512 {
        ids.insert(path.into(), (Instant::now(), id.clone()));
    }
    id
}

pub fn edid_refresh(hint: &str, targets: &[u32]) -> Result<serde_json::Value> {
    let monitors = Topology::query_all()?.basic_monitors();
    let monitor = monitors
        .iter()
        .find(|monitor| monitor.matches(hint) || monitor.friendly_name.eq_ignore_ascii_case(hint))
        .context("display device not found for EDID refresh validation")?;
    let mut bytes = Vec::new();
    if let Some((hardware, instance)) = path_parts(&monitor.monitor_device_path)
        && !hardware.contains(['\\', '/'])
        && !instance.contains(['\\', '/'])
    {
        let key = format!(
            "SYSTEM\\CurrentControlSet\\Enum\\DISPLAY\\{hardware}\\{instance}\\Device Parameters"
        );
        let key: Vec<_> = key.encode_utf16().chain(Some(0)).collect();
        let name: Vec<_> = "EDID\0".encode_utf16().collect();
        let mut size = 0u32;
        // SAFETY: The registry strings are terminated and the output buffer is resized to the queried byte count.
        unsafe {
            use windows::Win32::System::Registry::*;
            if RegGetValueW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(key.as_ptr()),
                PCWSTR(name.as_ptr()),
                RRF_RT_REG_BINARY,
                None,
                None,
                Some(&mut size),
            )
            .is_ok()
                && size <= 65536
            {
                bytes.resize(size as usize, 0);
                if RegGetValueW(
                    HKEY_LOCAL_MACHINE,
                    PCWSTR(key.as_ptr()),
                    PCWSTR(name.as_ptr()),
                    RRF_RT_REG_BINARY,
                    None,
                    Some(bytes.as_mut_ptr().cast()),
                    Some(&mut size),
                )
                .is_err()
                {
                    bytes.clear();
                }
            }
        }
    }
    let info = butterpollo_media_policy::edid::Refresh::parse(&bytes);
    let mut response = serde_json::to_value(&info)?;
    response["status"] = true.into();
    response["device_id"] = monitor.device_id.clone().into();
    response["device_label"] = monitor.friendly_name.clone().into();
    response["targets"] = targets
        .iter()
        .map(|hz| {
            let (supported, method) = info.support(*hz);
            serde_json::json!({"hz":hz,"supported":supported,"method":method})
        })
        .collect::<Vec<_>>()
        .into();
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitor_id_releases_global_cache_lock_before_probing_and_caches_hit() {
        let path = r"\\?\DISPLAY#TEST#identity-lock-test#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
        {
            crate::lock_unpoisoned(&IDS).remove(path);
        }
        let mut misses = 0usize;
        let first = monitor_id_with(
            path,
            |_| {
                misses += 1;
                assert!(
                    IDS.try_lock().is_ok(),
                    "IDS mutex must not stay locked while probing monitor instance"
                );
                Some("DISPLAY\\TEST\\INSTANCE".to_string())
            },
            |_| {
                assert!(
                    IDS.try_lock().is_ok(),
                    "IDS mutex must not stay locked while reading EDID"
                );
                Vec::new()
            },
        );
        assert_eq!(misses, 1);
        let second = monitor_id_with(
            path,
            |_| unreachable!("cache hit must not re-probe instance"),
            |_| unreachable!("cache hit must not re-read EDID"),
        );
        assert_eq!(first, second);
        crate::lock_unpoisoned(&IDS).remove(path);
    }
}
