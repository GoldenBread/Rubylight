//! Raw C ABI bindings (AMF, NVENC, CUDA, PyroWave) and precompiled HLSL shaders.
#![cfg(windows)]

#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unused_imports,
    clippy::upper_case_acronyms
)]
pub mod amf {
    include!(concat!(env!("OUT_DIR"), "/amf.rs"));
}

#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unused_imports,
    clippy::all,
    clippy::undocumented_unsafe_blocks
)]
pub mod nvenc {
    include!(concat!(env!("OUT_DIR"), "/nvenc.rs"));
    include!(concat!(env!("OUT_DIR"), "/nvenc_guids.rs"));
}

#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unused_imports,
    clippy::all,
    clippy::undocumented_unsafe_blocks
)]
pub mod cuda {
    include!(concat!(env!("OUT_DIR"), "/cuda.rs"));
}

#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unused_imports
)]
pub mod pyrowave {
    include!(concat!(env!("OUT_DIR"), "/pyrowave.rs"));
}

/// Precompiled DXBC shaders and HLSL source fallback for colour conversion and RTX Video.
pub mod shaders {
    include!(concat!(env!("OUT_DIR"), "/shader_bytecode.rs"));

    pub const COLOR_HLSL: &str = include_str!("shaders/color.hlsl");
    pub const RTX_VIDEO_HLSL: &str = include_str!("shaders/rtx_video.hlsl");
}
