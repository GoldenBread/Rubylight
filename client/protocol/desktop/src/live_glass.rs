//! SDL3 GPU render-target chain (`Chain` + `sync`) for live glass; re-exports the pure `egui`
//! live-glass types and stage machine from `nightjar-desktop-ui::live_glass`.

use sdl3_sys::everything::*;

pub use nightjar_desktop_ui::live_glass::{
    forced_by, frame_differs, frame_print, level_size, set_tier, Glass, Stage, Tier, LEVELS,
    LIVE_BACKDROP,
};
#[cfg(test)]
pub use nightjar_desktop_ui::live_glass::{find_boundary, tier};

use nightjar_desktop_media::sdl;

/// The render targets of the chain.
pub struct Chain {
    size: (i32, i32),
    scene: *mut SDL_Texture,
    levels: Vec<*mut SDL_Texture>,
    ran: bool,
}

impl Default for Chain {
    fn default() -> Self {
        Self {
            size: (0, 0),
            scene: std::ptr::null_mut(),
            levels: Vec::new(),
            ran: false,
        }
    }
}

impl Chain {
    /// The texture chrome samples, once the chain has run.
    pub fn output(&self) -> Option<*mut SDL_Texture> {
        self.levels.last().copied().filter(|_| self.ran)
    }

    /// Frees the targets: the frame has no chrome (a stream plays, or the window closes).
    pub fn release(&mut self) {
        // SAFETY: textures we created, each freed once.
        unsafe {
            if !self.scene.is_null() {
                SDL_DestroyTexture(self.scene);
            }
            for level in self.levels.drain(..) {
                if !level.is_null() {
                    SDL_DestroyTexture(level);
                }
            }
        }
        self.scene = std::ptr::null_mut();
        self.size = (0, 0);
        self.ran = false;
    }

    /// A target in the output's own colour space, so content painted through the scene comes out
    /// as content painted directly: a linear output blends in linear light in both. Null on failure.
    fn make_target(renderer: *mut SDL_Renderer, (w, h): (i32, i32)) -> *mut SDL_Texture {
        // SAFETY: SDL calls on a live renderer; the properties are ours and freed here.
        unsafe {
            let output = SDL_GetNumberProperty(
                SDL_GetRendererProperties(renderer),
                SDL_PROP_RENDERER_OUTPUT_COLORSPACE_NUMBER,
                i64::from(SDL_COLORSPACE_SRGB.0),
            );
            let (format, space) = if output == i64::from(SDL_COLORSPACE_SRGB_LINEAR.0) {
                (SDL_PIXELFORMAT_RGBA64_FLOAT, SDL_COLORSPACE_SRGB_LINEAR)
            } else {
                (SDL_PIXELFORMAT_RGBA32, SDL_COLORSPACE_SRGB)
            };
            let props = SDL_CreateProperties();
            let set = |name, value: i64| SDL_SetNumberProperty(props, name, value);
            set(SDL_PROP_TEXTURE_CREATE_FORMAT_NUMBER, i64::from(format.0));
            set(
                SDL_PROP_TEXTURE_CREATE_ACCESS_NUMBER,
                i64::from(SDL_TEXTUREACCESS_TARGET.0),
            );
            set(SDL_PROP_TEXTURE_CREATE_WIDTH_NUMBER, i64::from(w.max(1)));
            set(SDL_PROP_TEXTURE_CREATE_HEIGHT_NUMBER, i64::from(h.max(1)));
            set(
                SDL_PROP_TEXTURE_CREATE_COLORSPACE_NUMBER,
                i64::from(space.0),
            );
            let texture = SDL_CreateTextureWithProperties(renderer, props);
            SDL_DestroyProperties(props);
            if !texture.is_null() {
                SDL_SetTextureBlendMode(texture, SDL_BLENDMODE_NONE);
                SDL_SetTextureScaleMode(texture, SDL_SCALEMODE_LINEAR);
            }
            texture
        }
    }

    /// One small target made, drawn to and freed: can this renderer do it at all?
    pub fn check(renderer: *mut SDL_Renderer) -> Result<(), String> {
        let target = Self::make_target(renderer, (16, 16));
        if target.is_null() {
            return Err(format!("a render target could not be made: {}", sdl::error()));
        }
        // SAFETY: a target we just made on this live renderer; the frame's target comes back.
        unsafe {
            let ok = SDL_SetRenderTarget(renderer, target)
                && SDL_RenderClear(renderer)
                && SDL_SetRenderTarget(renderer, std::ptr::null_mut());
            let why = sdl::error();
            SDL_SetRenderTarget(renderer, std::ptr::null_mut());
            SDL_DestroyTexture(target);
            ok.then_some(())
                .ok_or_else(|| format!("the render target could not be set: {why}"))
        }
    }

    /// Points rendering at the scene target, its size matching the output, cleared to the frame's
    /// draw colour as the back buffer was.
    pub fn begin(&mut self, renderer: *mut SDL_Renderer) -> Result<(), String> {
        let (mut w, mut h) = (0, 0);
        // SAFETY: a live renderer and two live integers.
        unsafe { SDL_GetCurrentRenderOutputSize(renderer, &mut w, &mut h) };
        if w <= 0 || h <= 0 {
            return Err(format!("the output is {w}x{h}"));
        }
        if self.size != (w, h) {
            self.release();
            self.scene = Self::make_target(renderer, (w, h));
            self.levels = LEVELS
                .iter()
                .map(|&div| Self::make_target(renderer, level_size((w, h), div)))
                .collect();
            if self.scene.is_null() || self.levels.iter().any(|l| l.is_null()) {
                let why = sdl::error();
                self.release();
                return Err(format!("a render target could not be made: {why}"));
            }
            if let Some(&output) = self.levels.last() {
                // SAFETY: a target we just made.
                unsafe { SDL_SetTextureBlendMode(output, crate::gui::premultiplied_blend()) };
            }
            self.size = (w, h);
        }
        // SAFETY: a target we created on this renderer.
        unsafe {
            if !SDL_SetRenderTarget(renderer, self.scene) {
                return Err(format!(
                    "the render target could not be set: {}",
                    sdl::error()
                ));
            }
            SDL_SetRenderClipRect(renderer, std::ptr::null());
            SDL_RenderClear(renderer);
        }
        Ok(())
    }

    /// With the content on the scene target: runs the chain, then puts the scene on the back buffer.
    pub fn run(&mut self, renderer: *mut SDL_Renderer, timed: bool) -> Result<Option<f32>, String> {
        // SAFETY: targets we created on this renderer; the null rects mean the whole texture.
        unsafe {
            SDL_SetRenderClipRect(renderer, std::ptr::null());
            let mut timing = if timed {
                sync(renderer).map(|()| Some(std::time::Instant::now()))
            } else {
                Ok(None)
            };
            let mut from = self.scene;
            let mut ok = true;
            for &level in &self.levels {
                ok &= SDL_SetRenderTarget(renderer, level);
                ok &= SDL_RenderTexture(renderer, from, std::ptr::null(), std::ptr::null());
                from = level;
            }
            if let Ok(Some(_)) = timing {
                timing = sync(renderer).and(timing);
            }
            let ms = timing
                .map(|started| started.map(|started| started.elapsed().as_secs_f32() * 1000.0));
            ok &= SDL_SetRenderTarget(renderer, std::ptr::null_mut());
            ok &= SDL_RenderTexture(renderer, self.scene, std::ptr::null(), std::ptr::null());
            if !ok {
                return Err(format!("a chain pass failed: {}", sdl::error()));
            }
            let ms = ms?;
            self.ran = true;
            Ok(ms)
        }
    }
}

/// Blocks until the GPU finished the current target: reading one pixel back waits for it.
///
/// # Safety
/// Needs a live renderer.
unsafe fn sync(renderer: *mut SDL_Renderer) -> Result<(), String> {
    let one = SDL_Rect { x: 0, y: 0, w: 1, h: 1 };
    // SAFETY: as the function says; the surface is ours to free.
    unsafe {
        SDL_FlushRenderer(renderer);
        let surface = SDL_RenderReadPixels(renderer, &one);
        if surface.is_null() {
            let why = sdl::error();
            eprintln!("nightjar: live glass: the chain could not be timed, its read-back failed: {why}");
            return Err(format!("the chain could not be timed: {why}"));
        }
        SDL_DestroySurface(surface);
    }
    Ok(())
}
