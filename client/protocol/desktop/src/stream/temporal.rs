//! Temporal upscaling (`Upscaling::Sgsr2`): the six fragment passes of `temporal.hlsl` (ported
//! from the Android app's `shaders/temporal_*.frag`, same math) over a luma pyramid, block motion,
//! a scene-cut mean and an output-size history kept from frame to frame. Each output uses only
//! the current picture and state from earlier pictures, so it adds no frame of delay.
//!
//! Fragment passes only, no compute shaders: they run wherever half-float render targets do,
//! which [`Temporal::new`] probes (the targets and every pass's shader); the caller falls back to
//! SGSR 1 when it refuses. No GPU time per pass is measured: SDL GPU has no timestamp queries and
//! the desktop app has no diagnostics hook to show it in.
//!
//! On Direct3D 11 the textures are the device's own (R16F luma and cut, RGBA16F motion and
//! history); on SDL GPU they are RGBA64_FLOAT SDL textures, as SDL has no one-channel float
//! format (only the red channel of the luma and cut textures is used).
use std::ptr;

use sdl3_sys::everything::*;

use super::shader::Shader;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stage {
    Prep,
    Down,
    Motion,
    Cut,
    Accumulate,
    Detail,
}

impl Stage {
    pub const ALL: [Self; 6] = [Self::Prep, Self::Down, Self::Motion, Self::Cut, Self::Accumulate, Self::Detail];

    pub fn name(self) -> &'static str {
        match self {
            Self::Prep => "prep",
            Self::Down => "down",
            Self::Motion => "motion",
            Self::Cut => "cut",
            Self::Accumulate => "accumulate",
            Self::Detail => "detail",
        }
    }

    /// The textures the pass reads, in slots 0 and on.
    pub fn inputs(self) -> u32 {
        match self {
            Self::Prep | Self::Down | Self::Cut => 1,
            Self::Motion => 7,
            Self::Accumulate => 4,
            Self::Detail => 2,
        }
    }

    /// Whether the pass reads the uniform block (`uniforms`).
    pub fn uniforms(self) -> bool {
        matches!(self, Self::Accumulate | Self::Detail)
    }

    /// Whether the pass is skipped on a reset frame: motion cannot compare against a previous
    /// scene, and `Accumulate` ignores `motion` and `cut` when `reset` is set.
    pub fn skipped_on_reset(self) -> bool {
        matches!(self, Self::Motion | Self::Cut)
    }

    #[cfg(not(target_os = "macos"))]
    pub fn spirv(self) -> &'static [u8] {
        match self {
            Self::Prep => include_bytes!("../temporal-prep.spv"),
            Self::Down => include_bytes!("../temporal-down.spv"),
            Self::Motion => include_bytes!("../temporal-motion.spv"),
            Self::Cut => include_bytes!("../temporal-cut.spv"),
            Self::Accumulate => include_bytes!("../temporal-accumulate.spv"),
            Self::Detail => include_bytes!("../temporal-detail.spv"),
        }
    }

    #[cfg(target_os = "macos")]
    pub fn metal(self) -> &'static [u8] {
        match self {
            Self::Prep => include_bytes!("../metal/temporal-prep.metal"),
            Self::Down => include_bytes!("../metal/temporal-down.metal"),
            Self::Motion => include_bytes!("../metal/temporal-motion.metal"),
            Self::Cut => include_bytes!("../metal/temporal-cut.metal"),
            Self::Accumulate => include_bytes!("../metal/temporal-accumulate.metal"),
            Self::Detail => include_bytes!("../metal/temporal-detail.metal"),
        }
    }
}

// The textures by index: the luma pyramid, motion and history twice (this frame's and the last
// one's, swapped every frame), the cut mean, then the two the upscaler owns.
const fn luma(level: usize, parity: usize) -> usize { level * 2 + parity }
const fn motion(parity: usize) -> usize { 6 + parity }
const fn history(parity: usize) -> usize { 8 + parity }
const CUT: usize = 10;
/// The pass's own textures; `FRAME` and `OUTPUT` follow them.
const OWNED: usize = 11;
/// The picture at stream size (the upscaler's RGBA8 copy of it).
const FRAME: usize = 11;
/// The upscaler's RGBA8 output texture at output size.
const OUTPUT: usize = 12;

/// The size of each owned texture and whether it holds four channels (else one), for a stream
/// of `input` drawn at `output`.
fn targets(input: (u32, u32), output: (u32, u32)) -> [((u32, u32), bool); OWNED] {
    let half = |(w, h): (u32, u32)| ((w / 2).max(1), (h / 2).max(1));
    let levels = [input, half(input), half(half(input))];
    let blocks = (input.0.div_ceil(8), input.1.div_ceil(8));
    std::array::from_fn(|i| match i {
        0..6 => (levels[i / 2], false),
        6 | 7 => (blocks, true),
        8 | 9 => (output, true),
        _ => ((1, 1), false),
    })
}

/// The draws of `stage` in a frame of `parity`: the textures each reads (slot order) and the one
/// it renders.
fn bindings(stage: Stage, parity: usize) -> Vec<(Vec<usize>, usize)> {
    let (cur, prev) = (parity, 1 - parity);
    match stage {
        Stage::Prep => vec![(vec![FRAME], luma(0, cur))],
        Stage::Down => vec![(vec![luma(0, cur)], luma(1, cur)), (vec![luma(1, cur)], luma(2, cur))],
        Stage::Motion => vec![(
            vec![luma(0, cur), luma(1, cur), luma(2, cur), luma(0, prev), luma(1, prev), luma(2, prev), motion(prev)],
            motion(cur),
        )],
        Stage::Cut => vec![(vec![motion(cur)], CUT)],
        Stage::Accumulate => vec![(vec![FRAME, motion(cur), CUT, history(prev)], history(cur))],
        Stage::Detail => vec![(vec![FRAME, history(cur)], OUTPUT)],
    }
}

/// Whether a texture is sampled with linear filtering (frame, motion and history); the luma and
/// cut textures are read by texel.
fn linear(texture: usize) -> bool {
    matches!(texture, FRAME | 6..=9)
}

/// The pass's uniform block (`temporal.hlsl`), 8 floats.
fn uniforms(stage: Stage, input: (u32, u32), output: (u32, u32), reset: bool, sharpness: u32) -> [u32; 8] {
    let (w, h) = (input.0 as f32, input.1 as f32);
    let (ow, oh) = (output.0 as f32, output.1 as f32);
    let values = match stage {
        // SGSR 2's scaleRatio.y: the box grows with the area ratio, up to 20.
        Stage::Accumulate => [w, h, ow, oh, if reset { 1.0 } else { 0.0 }, ((ow * oh) / (w * h)).powi(3).min(20.0), 0.0, 0.0],
        // SGSR 1's edge sharpness, as the SGSR 1 upscaler takes it.
        Stage::Detail => [1.0 / w, 1.0 / h, w, h, ow, oh, 1.0 + sharpness.min(100) as f32 / 100.0, 0.0],
        _ => [0.0; 8],
    };
    values.map(f32::to_bits)
}

/// Whether temporal upscaling's history belongs to another picture than this one: the picture
/// does not follow the one before it, or the decode side's breaks count
/// (`Decoded::history_breaks`) is newer than the last picture drawn, so a key frame or a gap
/// happened since, in this picture or in one the pacer dropped. Pictures that only skip a turn
/// leave the count alone, so a stream shown at fewer presents than it decodes keeps its history.
/// The accumulate pass starts the history afresh instead of blending another scene.
pub(super) fn history_stale(last_drawn: u64, follows: bool, breaks: u64) -> bool {
    !follows || breaks != last_drawn
}

/// One pass's shader and its draws, every binding of parity 0 then of parity 1.
enum Pass {
    #[cfg(windows)]
    Native(super::d3d11::Filter),
    Gpu(GpuPass),
}

struct GpuPass {
    device: *mut SDL_GPUDevice,
    shader: *mut SDL_GPUShader,
    /// A render state per binding (its textures are part of it) and the texture it renders.
    states: Vec<(*mut SDL_GPURenderState, *mut SDL_Texture)>,
    uniforms: bool,
}

impl GpuPass {
    unsafe fn draw(&self, renderer: *mut SDL_Renderer, binding: usize, parameters: &[u32; 8]) -> bool {
        let Some(&(state, target)) = self.states.get(binding) else { return false };
        let previous = SDL_GetRenderTarget(renderer);
        if !SDL_SetRenderTarget(renderer, target) { return false; }
        let mut blend = SDL_BLENDMODE_NONE;
        SDL_GetRenderDrawBlendMode(renderer, &mut blend);
        SDL_SetRenderDrawBlendMode(renderer, SDL_BLENDMODE_NONE);
        let ok = (!self.uniforms || SDL_SetGPURenderStateFragmentUniforms(state, 0, parameters.as_ptr().cast(), 32))
            && SDL_SetGPURenderState(renderer, state)
            && SDL_RenderFillRect(renderer, ptr::null())
            && SDL_FlushRenderer(renderer);
        SDL_SetGPURenderState(renderer, ptr::null_mut());
        SDL_SetRenderDrawBlendMode(renderer, blend);
        SDL_SetRenderTarget(renderer, previous);
        ok
    }
}

impl Drop for GpuPass {
    fn drop(&mut self) {
        // SAFETY: the pass's own states and shader on its device.
        unsafe {
            for (state, _) in self.states.drain(..) { SDL_DestroyGPURenderState(state); }
            if !self.shader.is_null() { SDL_ReleaseGPUShader(self.device, self.shader); }
        }
    }
}

/// The textures the passes render, whichever API owns them.
enum Owned {
    #[cfg(windows)]
    Native {
        textures: Vec<windows::Win32::Graphics::Direct3D11::ID3D11Texture2D>,
        motion_rtvs: [Option<windows::Win32::Graphics::Direct3D11::ID3D11RenderTargetView>; 2],
    },
    Gpu { device: *mut SDL_GPUDevice, textures: Vec<*mut SDL_Texture>, samplers: [*mut SDL_GPUSampler; 2] },
}

unsafe fn clear_gpu_texture(renderer: *mut SDL_Renderer, texture: *mut SDL_Texture, flush: bool) -> Result<(), String> {
    let previous = SDL_GetRenderTarget(renderer);
    let (mut r, mut g, mut b, mut a) = (0, 0, 0, 0);
    SDL_GetRenderDrawColor(renderer, &mut r, &mut g, &mut b, &mut a);
    let cleared = SDL_SetRenderTarget(renderer, texture)
        && SDL_SetRenderDrawColor(renderer, 0, 0, 0, 0)
        && SDL_RenderClear(renderer)
        && (!flush || SDL_FlushRenderer(renderer));
    SDL_SetRenderDrawColor(renderer, r, g, b, a);
    SDL_SetRenderTarget(renderer, previous);
    if !cleared {
        return Err(format!("a half-float render target cannot be drawn to: {}", crate::sdl::error()));
    }
    Ok(())
}

pub(super) struct Temporal {
    passes: Vec<Pass>,
    owned: Owned,
    input: (u32, u32),
    output: (u32, u32),
    parity: usize,
    reset: bool,
}

impl Temporal {
    /// The passes from `frame` (the stream-size picture) to `output` (output size), both RGBA8
    /// target textures of `renderer`; refused, with the reason, when half-float targets or any
    /// pass's shader cannot be created.
    pub unsafe fn new(renderer: *mut SDL_Renderer, frame: *mut SDL_Texture, output: *mut SDL_Texture, input_size: (u32, u32),
        output_size: (u32, u32)) -> Result<Self, String> {
        let mut result = Self { passes: Vec::new(), owned: Owned::Gpu { device: ptr::null_mut(), textures: Vec::new(), samplers: [ptr::null_mut(); 2] },
            input: input_size, output: output_size, parity: 0, reset: true };
        #[cfg(windows)]
        {
            let native = |texture| SDL_GetPointerProperty(SDL_GetTextureProperties(texture), SDL_PROP_TEXTURE_D3D11_TEXTURE_POINTER, ptr::null_mut());
            let (frame, output) = (native(frame), native(output));
            if !frame.is_null() && !output.is_null() {
                result.make_native(frame, output)?;
                return Ok(result);
            }
        }
        result.make_gpu(renderer, frame, output)?;
        Ok(result)
    }

    #[cfg(windows)]
    unsafe fn make_native(&mut self, frame: *mut std::ffi::c_void, output: *mut std::ffi::c_void) -> Result<(), String> {
        use windows::core::Interface;
        let mut owned = Vec::with_capacity(OWNED);
        for (size, vector) in targets(self.input, self.output) {
            let texture = super::d3d11::half_float_target(frame, size, vector)
                .map_err(|e| format!("a {} half-float render target cannot be created: {e}", if vector { "RGBA16F" } else { "R16F" }))?;
            owned.push(texture);
        }
        let mut handles: Vec<_> = owned.iter().map(|texture| texture.as_raw()).collect();
        handles.extend([frame, output]);
        let mut motion_rtvs = [None, None];
        let device = owned[motion(0)].GetDevice().map_err(|e| format!("the D3D11 device cannot be read: {e}"))?;
        for (p, rtv) in motion_rtvs.iter_mut().enumerate() {
            device.CreateRenderTargetView(&owned[motion(p)], None, Some(rtv))
                .map_err(|e| format!("the motion clear target cannot be created: {e}"))?;
        }
        self.owned = Owned::Native { textures: owned, motion_rtvs };
        for stage in Stage::ALL {
            let mut filter = super::d3d11::Filter::compiled(frame, &Shader::temporal_hlsl(stage))
                .map_err(|e| format!("the {} pass's shader cannot be created: {e}", stage.name()))?;
            for parity in 0..2 {
                for (inputs, target) in bindings(stage, parity) {
                    let inputs: Vec<_> = inputs.iter().map(|&i| handles[i]).collect();
                    filter.bind(&inputs, handles[target]).map_err(|e| format!("the {} pass's textures cannot be bound: {e}", stage.name()))?;
                }
            }
            self.passes.push(Pass::Native(filter));
        }
        Ok(())
    }

    unsafe fn make_gpu(&mut self, renderer: *mut SDL_Renderer, frame: *mut SDL_Texture, output: *mut SDL_Texture) -> Result<(), String> {
        let device: *mut SDL_GPUDevice = crate::sdl::renderer_pointer(renderer, SDL_PROP_RENDERER_GPU_DEVICE_POINTER).cast();
        if device.is_null() {
            return Err("it needs the Direct3D 11 or the SDL GPU renderer".into());
        }
        let mut textures = Vec::with_capacity(OWNED + 2);
        let mut samplers = [ptr::null_mut(); 2];
        let made = (|| {
            for (size, _) in targets(self.input, self.output) {
                let texture = SDL_CreateTexture(renderer, SDL_PIXELFORMAT_RGBA64_FLOAT, SDL_TEXTUREACCESS_TARGET, size.0 as i32, size.1 as i32);
                if texture.is_null() {
                    return Err(format!("a half-float render target cannot be created: {}", crate::sdl::error()));
                }
                textures.push(texture);
                SDL_SetTextureBlendMode(texture, SDL_BLENDMODE_NONE);
                // Last frame's motion is the first frame's predictor: zero, as the history.
                clear_gpu_texture(renderer, texture, false)?;
            }
            for (i, filter) in [SDL_GPU_FILTER_NEAREST, SDL_GPU_FILTER_LINEAR].into_iter().enumerate() {
                samplers[i] = SDL_CreateGPUSampler(device, &SDL_GPUSamplerCreateInfo {
                    min_filter: filter, mag_filter: filter, mipmap_mode: SDL_GPU_SAMPLERMIPMAPMODE_NEAREST,
                    address_mode_u: SDL_GPU_SAMPLERADDRESSMODE_CLAMP_TO_EDGE, address_mode_v: SDL_GPU_SAMPLERADDRESSMODE_CLAMP_TO_EDGE,
                    address_mode_w: SDL_GPU_SAMPLERADDRESSMODE_CLAMP_TO_EDGE, ..Default::default() });
                if samplers[i].is_null() {
                    return Err(format!("the sampler cannot be created: {}", crate::sdl::error()));
                }
            }
            Ok(())
        })();
        let mut all = textures.clone();
        self.owned = Owned::Gpu { device, textures, samplers };
        made?;
        all.extend([frame, output]);
        let gpu: Vec<*mut SDL_GPUTexture> = all.iter()
            .map(|&texture| SDL_GetPointerProperty(SDL_GetTextureProperties(texture), SDL_PROP_TEXTURE_GPU_TEXTURE_POINTER, ptr::null_mut()).cast())
            .collect();
        if gpu.iter().any(|texture| texture.is_null()) {
            return Err("a texture has no SDL GPU texture".into());
        }
        for stage in Stage::ALL {
            let shader = SDL_CreateGPUShader(device, &Shader::Temporal(stage).info());
            if shader.is_null() {
                return Err(format!("the {} pass's shader cannot be created: {}", stage.name(), crate::sdl::error()));
            }
            let mut pass = GpuPass { device, shader, states: Vec::new(), uniforms: stage.uniforms() };
            for parity in 0..2 {
                for (inputs, target) in bindings(stage, parity) {
                    let bound: Vec<_> = inputs.iter()
                        .map(|&i| SDL_GPUTextureSamplerBinding { texture: gpu[i], sampler: samplers[usize::from(linear(i))] })
                        .collect();
                    let state = SDL_CreateGPURenderState(renderer, &SDL_GPURenderStateCreateInfo {
                        fragment_shader: shader, num_sampler_bindings: bound.len() as i32, sampler_bindings: bound.as_ptr(), ..Default::default() });
                    if state.is_null() {
                        return Err(format!("the {} pass's render state cannot be created: {}", stage.name(), crate::sdl::error()));
                    }
                    pass.states.push((state, all[target]));
                }
            }
            self.passes.push(Pass::Gpu(pass));
        }
        Ok(())
    }

    /// Whether the output texture holds a drawn picture (one `draw` since creation).
    pub fn has_output(&self) -> bool {
        !self.reset
    }

    /// Makes the next [`draw`](Self::draw)'s accumulate pass ignore the history: the picture it
    /// would blend with is from another scene, before a key frame or across a lost frame's gap
    /// (`history_stale`).
    pub fn reset(&mut self) {
        self.reset = true;
    }

    /// Which half of the ping-pong state the next frame writes.
    #[cfg(any(test, all(target_os = "macos", feature = "demo-library")))]
    pub fn parity(&self) -> usize {
        self.parity
    }

    unsafe fn clear_motion(&self, renderer: *mut SDL_Renderer) -> Result<(), String> {
        match &self.owned {
            #[cfg(windows)]
            Owned::Native { textures, motion_rtvs } => {
                let device = textures[motion(self.parity)].GetDevice()
                    .map_err(|e| format!("the D3D11 device cannot be read: {e}"))?;
                let context = device.GetImmediateContext()
                    .map_err(|e| format!("the D3D11 context cannot be read: {e}"))?;
                context.ClearRenderTargetView(motion_rtvs[self.parity].as_ref().unwrap(), &[0.0; 4]);
                Ok(())
            }
            Owned::Gpu { textures, .. } => clear_gpu_texture(renderer, textures[motion(self.parity)], true),
        }
    }

    /// Runs the passes on the picture now in the frame texture, leaving the result in the output
    /// texture, and swaps this frame's state for the next.
    pub unsafe fn draw(&mut self, renderer: *mut SDL_Renderer, sharpness: u32) -> Result<(), String> {
        if !SDL_FlushRenderer(renderer) {
            return Err(format!("the renderer cannot flush: {}", crate::sdl::error()));
        }
        if self.reset {
            self.clear_motion(renderer)?;
        }
        for (stage, pass) in Stage::ALL.into_iter().zip(&self.passes) {
            if self.reset && stage.skipped_on_reset() {
                continue;
            }
            let parameters = uniforms(stage, self.input, self.output, self.reset, sharpness);
            let count = bindings(stage, 0).len();
            for binding in self.parity * count..(self.parity + 1) * count {
                let drawn = match pass {
                    #[cfg(windows)]
                    Pass::Native(filter) => filter.draw_binding(binding, &parameters, None),
                    Pass::Gpu(pass) => pass.draw(renderer, binding, &parameters),
                };
                if !drawn {
                    return Err(format!("the {} pass failed to draw: {}", stage.name(), crate::sdl::error()));
                }
            }
        }
        self.parity = 1 - self.parity;
        self.reset = false;
        Ok(())
    }
}

impl Drop for Temporal {
    fn drop(&mut self) {
        // The render states go before the textures they bind.
        self.passes.clear();
        match &mut self.owned {
            #[cfg(windows)]
            Owned::Native { .. } => {}
            // SAFETY: the upscaler's own textures and samplers.
            Owned::Gpu { device, textures, samplers } => unsafe {
                for texture in textures.drain(..) { SDL_DestroyTexture(texture); }
                for sampler in samplers { if !sampler.is_null() { SDL_ReleaseGPUSampler(*device, *sampler); } }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_textures_follow_the_stream_and_output_sizes() {
        let sizes = targets((1281, 721), (2560, 1440));
        assert_eq!(sizes[luma(0, 1)], ((1281, 721), false));
        assert_eq!(sizes[luma(1, 0)], ((640, 360), false), "floor(w / 2)");
        assert_eq!(sizes[luma(2, 1)], ((320, 180), false), "floor again");
        assert_eq!(sizes[motion(0)], ((161, 91), true), "ceil(w / 8)");
        assert_eq!(sizes[history(1)], ((2560, 1440), true));
        assert_eq!(sizes[CUT], ((1, 1), false));
    }

    #[test]
    fn each_frame_reads_the_last_frames_state_and_writes_its_own() {
        for parity in 0..2 {
            let (cur, prev) = (parity, 1 - parity);
            for stage in Stage::ALL {
                for (inputs, target) in bindings(stage, parity) {
                    assert_eq!(inputs.len() as u32, stage.inputs(), "{stage:?}");
                    assert!(!inputs.contains(&target), "{stage:?} reads its own target");
                }
            }
            let motion_inputs = &bindings(Stage::Motion, parity)[0].0;
            assert_eq!(motion_inputs[3..], [luma(0, prev), luma(1, prev), luma(2, prev), motion(prev)]);
            assert_eq!(bindings(Stage::Accumulate, parity)[0], (vec![FRAME, motion(cur), CUT, history(prev)], history(cur)));
            assert_eq!(bindings(Stage::Detail, parity)[0], (vec![FRAME, history(cur)], OUTPUT));
        }
        assert!(linear(FRAME) && linear(motion(1)) && linear(history(0)) && !linear(luma(2, 0)) && !linear(CUT));
    }

    /// The history is kept across pictures the pacer drops, and starts afresh only for a real
    /// gap or a key frame, even when the picture carrying that break was the one dropped.
    #[test]
    fn the_history_keeps_across_pacer_skips_and_resets_on_a_break() {
        // A stream whose only break is its first picture (there was no history before it), every
        // other picture dropped as on a 60 Hz display showing a 120 fps stream.
        let mut drawn = 0;
        let mut resets = Vec::new();
        for (i, breaks) in [1u64, 1, 1, 1, 1, 1].into_iter().enumerate() {
            if i % 2 == 0 {
                if history_stale(drawn, true, breaks) {
                    resets.push(i);
                }
                drawn = breaks;
            }
        }
        assert_eq!(resets, [0], "the first picture starts the history; skips keep it");
        assert!(!history_stale(1, true, 1), "a picture that follows keeps it");
        assert!(history_stale(1, false, 1), "one that does not follow resets it");
        assert!(history_stale(1, true, 2), "a lost frame's gap resets the next picture drawn");
        assert!(!history_stale(2, true, 2), "and the pictures after it keep the new history");
        assert!(history_stale(2, true, 3), "even when the picture that broke it was dropped");
    }

    #[test]
    fn the_uniforms_carry_reset_box_and_sharpness() {
        let floats = |u: [u32; 8]| u.map(f32::from_bits);
        let accumulate = floats(uniforms(Stage::Accumulate, (1280, 720), (2560, 1440), true, 50));
        assert_eq!(accumulate[..5], [1280.0, 720.0, 2560.0, 1440.0, 1.0]);
        assert_eq!(accumulate[5], 20.0, "(4x the area)^3 is capped at 20");
        let accumulate = floats(uniforms(Stage::Accumulate, (1600, 900), (1920, 1080), false, 50));
        assert_eq!(accumulate[4], 0.0);
        assert!((accumulate[5] - 1.44f32.powi(3)).abs() < 1e-4);
        let detail = floats(uniforms(Stage::Detail, (1280, 720), (2560, 1440), false, 50));
        assert_eq!(detail[2..], [1280.0, 720.0, 2560.0, 1440.0, 1.5, 0.0]);
    }

    #[test]
    fn reset_frames_skip_motion_and_cut_only() {
        let kept: Vec<_> = Stage::ALL.into_iter().filter(|s| !s.skipped_on_reset()).collect();
        assert_eq!(kept, [Stage::Prep, Stage::Down, Stage::Accumulate, Stage::Detail]);
    }

    /// Direct3D 11 compiles the passes when a stream starts; a pass FXC refuses would fall back
    /// to SGSR 1 on every PC.
    #[cfg(windows)]
    #[test]
    fn the_passes_compile_for_direct3d_11() {
        for stage in Stage::ALL {
            let source = Shader::temporal_hlsl(stage);
            // SAFETY: plain compiler calls on the pass's source.
            unsafe {
                for (entry, profile) in [(windows::core::s!("vertex"), windows::core::s!("vs_5_0")), (windows::core::s!("fragment"), windows::core::s!("ps_5_0"))] {
                    super::super::d3d11::compile(&source, entry, profile).unwrap_or_else(|e| panic!("{stage:?}: {e}"));
                }
            }
        }
    }
}
