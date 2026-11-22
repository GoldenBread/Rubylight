//! `mscms.dll` dynamic loader for color profile associations.
use anyhow::{Context, Result};
use std::path::Path;
use windows::Win32::Foundation::LUID;

type Get = unsafe extern "system" fn(u32, LUID, u32, u32, u32, *mut *mut u16) -> i32;
type Add = unsafe extern "system" fn(u32, *const u16, LUID, u32, i32, i32) -> i32;
type Remove = unsafe extern "system" fn(u32, *const u16, LUID, u32, i32) -> i32;

pub(super) struct Api {
    pub(super) _library: libloading::Library,
    pub(super) get: Get,
    pub(super) add: Add,
    pub(super) remove: Remove,
}

impl Api {
    pub(super) fn load() -> Result<Self> {
        let directory = std::env::var_os("SystemRoot").context("Windows directory unavailable")?;
        // SAFETY: the Get, Add and Remove types match mscms.dll's exported signatures, and
        // `library` is kept in `Self` for as long as the copied function pointers.
        unsafe {
            let lib = libloading::Library::new(Path::new(&directory).join(r"System32\mscms.dll"))?;
            Ok(Self {
                get: *lib.get(b"ColorProfileGetDisplayDefault\0")?,
                add: *lib.get(b"ColorProfileAddDisplayAssociation\0")?,
                remove: *lib.get(b"ColorProfileRemoveDisplayAssociation\0")?,
                _library: lib,
            })
        }
    }
}
