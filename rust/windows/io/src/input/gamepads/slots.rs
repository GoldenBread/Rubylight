//! Device interface opening, host-wide virtual gamepad slot tracking, and teardown.

use super::*;

pub fn open_interface(guid: GUID) -> Result<HANDLE> {
    // SAFETY: The sized SetupAPI structures and aligned detail buffer remain live through each call; the terminated device path is consumed before the device set is destroyed.
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
                FILE_ATTRIBUTE_NORMAL,
                None,
            )?)
        })();
        SetupDiDestroyDeviceInfoList(set)?;
        result
    }
}

/// Whether a device offering this interface is present, without opening it.
pub(in crate::input) fn interface_present(guid: GUID) -> bool {
    // SAFETY: the sized interface structure stays live through the call, and
    // the device set is destroyed once, after it.
    unsafe {
        let Ok(set) = SetupDiGetClassDevsW(
            Some(&guid),
            PCWSTR::null(),
            None,
            DIGCF_DEVICEINTERFACE | DIGCF_PRESENT,
        ) else {
            return false;
        };
        let mut interface = SP_DEVICE_INTERFACE_DATA {
            cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
            ..Default::default()
        };
        let present = SetupDiEnumDeviceInterfaces(set, None, &guid, 0, &mut interface).is_ok();
        let _ = SetupDiDestroyDeviceInfoList(set);
        present
    }
}

pub(in crate::input) fn request(size: u32, id: Option<u32>) -> Vec<u8> {
    let mut b = size.to_le_bytes().to_vec();
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&0u16.to_le_bytes());
    if let Some(id) = id {
        b.extend_from_slice(&id.to_le_bytes());
    }
    b
}

pub(super) static SLOTS: std::sync::Mutex<[bool; 16]> = std::sync::Mutex::new([false; 16]);
/// Reserve a host-wide identity until its USB attachment is fully detached.
pub(in crate::input) struct Slot(u16);
/// Driver slots whose pad would not unplug when its session's gamepads closed. They stay
/// reserved in [`SLOTS`], so no other pad is plugged over one the driver may still hold,
/// until a later unplug of them succeeds.
pub(super) static STRANDED: std::sync::Mutex<BTreeSet<u16>> =
    std::sync::Mutex::new(BTreeSet::new());

/// Unplugs stranded slots until the driver refuses the handle, freeing any removed slots.
pub(super) fn reclaim_until_lost(
    slots: &mut [bool; 16],
    mut unplug: impl FnMut(u32) -> Result<()>,
) -> Option<anyhow::Error> {
    let mut lost = None;
    STRANDED.lock().unwrap().retain(|&global| {
        if lost.is_some() {
            return true;
        }
        match unplug(u32::from(global)) {
            Ok(()) => {
                tracing::info!(
                    slot = global,
                    "a virtual controller left by an earlier session was removed"
                );
                slots[global as usize] = false;
                false
            }
            Err(error) => {
                if gamepad_backend::driver_lost(&error) {
                    lost = Some(error);
                } else {
                    tracing::debug!(
                        slot = global,
                        error = format!("{error:#}"),
                        "a virtual controller left by an earlier session still will not unplug"
                    );
                }
                true
            }
        }
    });
    lost
}

#[cfg(test)]
pub(super) fn reclaim(slots: &mut [bool; 16], unplug: impl FnMut(u32) -> Result<()>) {
    let _ = reclaim_until_lost(slots, unplug);
}

/// Plugs a pad into a free slot of `slots`, reserved first while driver calls run.
/// With `every_slot`, per-slot refusals try the next slot; a lost handle bails at once.
pub(super) fn plug_in_free_slot(
    slots: &std::sync::Mutex<[bool; 16]>,
    every_slot: bool,
    mut plug: impl FnMut(u16) -> Result<()>,
) -> Result<Option<u16>> {
    let mut failed = None;
    let mut next = 0;
    loop {
        let global = {
            let mut slots = slots.lock().unwrap();
            let Some(free) = (next..16u16).find(|&global| !slots[global as usize]) else {
                break;
            };
            slots[free as usize] = true;
            free
        };
        next = global + 1;
        match plug(global) {
            Ok(()) => return Ok(Some(global)),
            Err(error) => {
                slots.lock().unwrap()[global as usize] = false;
                if !every_slot || gamepad_backend::driver_lost(&error) {
                    return Err(error);
                }
                failed = Some(error);
            }
        }
    }
    failed.map_or(Ok(None), Err)
}

impl Slot {
    pub(in crate::input) fn reserve() -> Result<Self> {
        let mut slots = SLOTS.lock().unwrap();
        let id = slots
            .iter()
            .position(|used| !used)
            .context("no free controller slots")?;
        slots[id] = true;
        Ok(Self(id as u16))
    }
    pub(in crate::input) fn id(&self) -> u16 {
        self.0
    }
}
impl Drop for Slot {
    fn drop(&mut self) {
        SLOTS.lock().unwrap()[self.0 as usize] = false;
    }
}

/// Whether an error is the driver saying its device is gone: what the VHF
/// virtual pads answer after Windows leaves D0 (sleep, hibernate), before the
/// slot is created again (`libvirtualgamepad`'s `protocol.h`). No other
/// backend has this signal: ViGEmBus pads are emulated devices, and the
/// USB/IP pads are real USB devices on usbip-win2's vhci bus that Windows
/// re-enumerates itself (see `recreate`).
pub(super) fn device_removed(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<windows::core::Error>()
            .is_some_and(|error| error.code() == ERROR_DEVICE_REMOVED.to_hresult())
    })
}

impl Drop for Gamepads {
    fn drop(&mut self) {
        if self.lost {
            // Closing the refused handle releases its pads without stranding slots.
            let mut slots = SLOTS.lock().unwrap();
            for &global in self.active.values() {
                slots[global as usize] = false;
            }
            return;
        }
        for (i, global) in self.active.clone() {
            if self.lost {
                SLOTS.lock().unwrap()[global as usize] = false;
                continue;
            }
            match self.backend.unplug(u32::from(global)) {
                Ok(()) => SLOTS.lock().unwrap()[global as usize] = false,
                Err(error) if gamepad_backend::driver_lost(&error) => {
                    self.lost = true;
                    SLOTS.lock().unwrap()[global as usize] = false;
                }
                Err(error) => {
                    tracing::warn!(
                        controller = i,
                        slot = global,
                        error = format!("{error:#}"),
                        "virtual controller removal failed as its session ended"
                    );
                    if self.backend.frees_pads_on_close() {
                        SLOTS.lock().unwrap()[global as usize] = false;
                    } else {
                        STRANDED.lock().unwrap().insert(global);
                    }
                }
            }
        }
    }
}
