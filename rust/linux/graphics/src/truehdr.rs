//! RTX HDR (NVIDIA TrueHDR): an SDR capture turned into an HDR picture before encoding. Not available on Linux yet: a filter cannot be made, so every picture is encoded as captured.
use crate::capture::GpuImage;
use anyhow::Result;

/// A converter on one capture device. None can be made on Linux yet.
pub struct Filter {
    parameters: [u32; 4],
}
impl Filter {
    /// The contrast, saturation, middle gray and peak
    /// (`butterpollo_media_policy::nvidia_enhance::rtx_parameters`).
    pub fn set_parameters(&mut self, parameters: [u32; 4]) {
        self.parameters = parameters;
    }
    /// The parameters the last conversion used.
    pub fn parameters(&self) -> [u32; 4] {
        self.parameters
    }
    /// A converter on the capture's device. Not available on Linux yet.
    pub fn new_gpu(_image: &GpuImage, _parameters: [u32; 4]) -> Result<Self> {
        Err(crate::unsupported("RTX HDR"))
    }
    /// The HDR picture for an SDR capture. Not available on Linux yet.
    pub fn apply_gpu(&mut self, _image: &GpuImage) -> Result<GpuImage> {
        Err(crate::unsupported("RTX HDR"))
    }
}
