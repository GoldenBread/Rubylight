//! The backend of a host with no driver settings API, which is every Linux host for now: it lists no GPU, and a read or a write says the API is not available.
use super::{BackendError, Driver, GpuInfo, RadeonBackend, Setting, model::Value};

/// The driver named by the panel, with nothing behind it.
pub(super) struct NoDriver(pub(super) Driver);

impl NoDriver {
    fn unavailable(&self) -> BackendError {
        let api = format!("The {} driver settings API", self.0.name());
        BackendError(crate::unsupported(&api).to_string())
    }
}

impl RadeonBackend for NoDriver {
    fn read(&mut self, _gpu: i32) -> Result<Vec<Setting>, BackendError> {
        Err(self.unavailable())
    }
    fn write(&mut self, _gpu: i32, _id: &str, _value: &Value) -> Result<(), BackendError> {
        Err(self.unavailable())
    }
    fn gpus(&mut self) -> Result<Vec<GpuInfo>, BackendError> {
        Ok(Vec::new())
    }
}
