//! VA-API on Linux: the display on a render node, a pool of surfaces, video processing and the
//! H.264, HEVC and AV1 encoders that capture and the host's encoder build on. Ported from the Linux
//! host spike (`rust/linux/probes`, `docs/linux-host-spike.md`).
pub mod av1_headers;
pub mod bits;
pub mod display;
pub mod encoder;
pub mod ffi;
pub mod h264_headers;
pub mod hevc_headers;
pub mod node;
pub mod pool;
pub mod references;
pub mod session;
pub mod session_av1;
pub mod session_h264;
pub mod session_hevc;
pub mod transfer;
pub mod vpp;

pub use display::Va;

pub use display::{BGRA, BGRX, NV12, P010, Plane, RGBA, RGBX, fourcc};

#[cfg(test)]
mod recovery_tests;
#[cfg(test)]
mod tenbit_tests;
#[cfg(test)]
mod tests;
