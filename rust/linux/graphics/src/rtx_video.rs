//! RTX Video enhancement: NVIDIA's video super resolution over a capture at its own size, before encoding. Not available on Linux yet: a filter cannot be made, so every picture is encoded as captured.
use crate::capture::GpuImage;
use anyhow::Result;

/// An enhancer on one capture device. None can be made on Linux yet.
pub struct Filter {
    _private: (),
}
impl Filter {
    /// An enhancer on the capture's device at quality level `quality`
    /// (`butterpollo_media_policy::nvidia_enhance::rtx_video_quality`). Not available on
    /// Linux yet.
    pub fn new_gpu(_image: &GpuImage, _quality: u32) -> Result<Self> {
        Err(crate::unsupported("RTX Video enhancement"))
    }
    /// The quality level of the next pictures.
    pub fn set_quality(&mut self, _quality: u32) {}
    /// The enhanced picture, in the capture's format. Not available on Linux yet.
    pub fn apply_gpu(&mut self, _image: &GpuImage) -> Result<GpuImage> {
        Err(crate::unsupported("RTX Video enhancement"))
    }
}
