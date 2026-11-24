//! The libva bindings, generated at build time from the system headers (`build.rs`), so every
//! encode parameter structure matches the installed libva.
#![allow(
    non_upper_case_globals,
    non_camel_case_types,
    non_snake_case,
    dead_code,
    unsafe_op_in_unsafe_fn,
    clippy::all,
    clippy::undocumented_unsafe_blocks
)]
include!(concat!(env!("OUT_DIR"), "/va.rs"));
