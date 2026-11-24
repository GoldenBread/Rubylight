//! Per-IOSurface `SDL_Texture` cache for VideoToolbox frames on macOS.

use sdl3_sys::everything::*;
use std::ffi::c_void;

/// Maximum cached Metal `SDL_Texture` wrappers (1 active + up to 15 idle).
const MAX_TEXTURES: usize = 16;

unsafe extern "C" {
    fn CVPixelBufferGetIOSurface(pixel_buffer: *mut c_void) -> *mut c_void;
    fn IOSurfaceGetID(buffer: *mut c_void) -> u32;
}

/// Returns the non-zero `IOSurfaceID` backing `pixel_buffer`, or `None` if unavailable.
pub(super) unsafe fn surface_id(pixel_buffer: *mut c_void) -> Option<u32> {
    if pixel_buffer.is_null() {
        return None;
    }
    // SAFETY: Caller passes a live `CVPixelBufferRef`.
    unsafe {
        let surface = CVPixelBufferGetIOSurface(pixel_buffer);
        if surface.is_null() {
            return None;
        }
        let id = IOSurfaceGetID(surface);
        (id != 0).then_some(id)
    }
}

/// Texture properties that invalidate the VideoToolbox `SDL_Texture` pool when changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CacheKey {
    pub renderer: usize,
    pub format: u32,
    pub width: i32,
    pub height: i32,
    pub space: u32,
    pub white_level_bits: u32,
    pub white_point_bits: Option<u32>,
    pub headroom_bits: Option<u32>,
}

/// Caches up to `MAX_TEXTURES` Metal `SDL_Texture` wrappers keyed by `IOSurfaceID`.
///
/// The currently displayed `active` texture is owned by `VideoTexture.texture` (and destroyed by
/// `VideoTexture::destroy`); every `idle` texture is destroyed here on eviction or drop.
pub(super) struct TextureCache {
    key: CacheKey,
    active: Option<(u32, *mut SDL_Texture)>,
    idle: Vec<(u32, *mut SDL_Texture)>,
}

impl TextureCache {
    pub(super) fn new(key: CacheKey, id: u32, texture: *mut SDL_Texture) -> Self {
        Self {
            key,
            active: Some((id, texture)),
            idle: Vec::with_capacity(MAX_TEXTURES - 1),
        }
    }

    pub(super) fn matches(&self, key: CacheKey, current_texture: *mut SDL_Texture) -> bool {
        self.key == key && self.active.is_some_and(|(_, texture)| texture == current_texture)
    }

    /// Promotes a cached texture for `id` to active without creating or destroying any texture.
    pub(super) fn activate(&mut self, id: u32) -> Option<*mut SDL_Texture> {
        if let Some((active_id, texture)) = self.active
            && active_id == id
        {
            return Some(texture);
        }
        let pos = self.idle.iter().position(|(cached_id, _)| *cached_id == id)?;
        let entry = self.idle.remove(pos);
        if let Some(previous) = self.active.replace(entry) {
            self.idle.push(previous);
        }
        Some(entry.1)
    }

    /// Inserts a newly created texture for `id` as active, moving the previous active entry to
    /// `idle` and destroying the oldest idle entry if the pool exceeds `MAX_TEXTURES`.
    pub(super) unsafe fn insert(&mut self, id: u32, texture: *mut SDL_Texture) {
        if let Some(previous) = self.active.replace((id, texture)) {
            self.idle.push(previous);
        }
        while self.idle.len() >= MAX_TEXTURES {
            let (_, evicted) = self.idle.remove(0);
            if !evicted.is_null() {
                // SAFETY: Idle texture created on the matching renderer and no longer displayed.
                unsafe { SDL_DestroyTexture(evicted) };
            }
        }
    }
}

impl Drop for TextureCache {
    fn drop(&mut self) {
        // `self.active` is destroyed by `VideoTexture::destroy`; destroy only idle entries here.
        for (_, texture) in self.idle.drain(..) {
            if !texture.is_null() {
                // SAFETY: Idle texture created by `VideoTexture` and owned exclusively by this pool.
                unsafe { SDL_DestroyTexture(texture) };
            }
        }
    }
}
