//! The typed GPU device loss that survives error context. On Linux nothing reports a device as lost yet: a loss is found only where an error already carries one.

/// A lost GPU device, with the code the graphics API gave for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceLost(pub i32);

const DEVICE_REMOVED: i32 = 0x887A_0005_u32 as i32;
const DEVICE_HUNG: i32 = 0x887A_0006_u32 as i32;
const DEVICE_RESET: i32 = 0x887A_0007_u32 as i32;
const DRIVER_INTERNAL_ERROR: i32 = 0x887A_0020_u32 as i32;

impl DeviceLost {
    /// The loss `code` stands for, if it is one of the device-loss codes.
    pub fn from_code(code: i32) -> Option<Self> {
        matches!(
            code,
            DEVICE_REMOVED | DEVICE_RESET | DEVICE_HUNG | DRIVER_INTERNAL_ERROR
        )
        .then_some(Self(code))
    }
    /// The loss `error` or one of its contexts carries.
    pub fn from_error(error: &anyhow::Error) -> Option<Self> {
        error.downcast_ref::<Self>().copied()
    }
    /// Whether `device` reports itself lost. Never on Linux: no device exists to ask.
    pub fn d3d11(_device: &crate::capture::D3d11Device) -> Option<Self> {
        None
    }
}
impl std::fmt::Display for DeviceLost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self.0 {
            DEVICE_REMOVED => "DXGI_ERROR_DEVICE_REMOVED",
            DEVICE_RESET => "DXGI_ERROR_DEVICE_RESET",
            DEVICE_HUNG => "DXGI_ERROR_DEVICE_HUNG",
            DRIVER_INTERNAL_ERROR => "DXGI_ERROR_DRIVER_INTERNAL_ERROR",
            _ => "GetDeviceRemovedReason",
        };
        write!(f, "GPU device lost: {name} (0x{:08X})", self.0 as u32)
    }
}
impl std::error::Error for DeviceLost {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_only_device_loss_codes() {
        for code in [
            DEVICE_REMOVED,
            DEVICE_RESET,
            DEVICE_HUNG,
            DRIVER_INTERNAL_ERROR,
        ] {
            assert_eq!(DeviceLost::from_code(code), Some(DeviceLost(code)));
        }
        // S_OK, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT, E_FAIL.
        for code in [0, 0x887A_0026_u32, 0x887A_0027, 0x8000_4005] {
            assert_eq!(DeviceLost::from_code(code as i32), None);
        }
        assert!(DeviceLost(DEVICE_HUNG).to_string().contains("0x887A0006"));
    }

    #[test]
    fn a_loss_is_found_through_error_context() {
        assert_eq!(
            DeviceLost::from_error(&anyhow::anyhow!("DXGI_ERROR_DEVICE_REMOVED")),
            None
        );
        let loss = DeviceLost(DEVICE_HUNG);
        assert_eq!(
            DeviceLost::from_error(&anyhow::Error::new(loss).context("AMF")),
            Some(loss)
        );
        let loss = DeviceLost(DEVICE_REMOVED);
        let error = anyhow::anyhow!("opening the capture failed")
            .context(loss)
            .context("encoding failed");
        assert_eq!(DeviceLost::from_error(&error), Some(loss));
    }
}
