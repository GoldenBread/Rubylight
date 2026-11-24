//! VA-API video processing: one picture converted, copied or scaled into another surface on the
//! GPU, the driver doing the colour conversion. Ported from the spike's `vpp.rs`.
use super::{
    display::{Va, check},
    ffi::*,
};
use anyhow::{Result, bail};
use std::sync::Arc;

/// What the source picture's colours are.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    /// Desktop RGB: sRGB primaries, full range.
    Rgb,
    /// Video already in the stream's colour space (BT.709, limited range).
    Video,
}

/// A video processing context for pictures that end up `width`x`height`.
pub struct Converter {
    va: Arc<Va>,
    config: VAConfigID,
    context: VAContextID,
}

impl Converter {
    pub fn new(va: &Arc<Va>, width: u32, height: u32) -> Result<Self> {
        if !va
            .entrypoints(VAProfileNone)
            .contains(&VAEntrypointVideoProc)
        {
            bail!("the GPU's VA-API driver has no video processing");
        }
        let config = va.create_config(VAProfileNone, VAEntrypointVideoProc, &[])?;
        let context = match va.create_context(config, width, height, &mut []) {
            Ok(context) => context,
            Err(error) => {
                // SAFETY: the config was made on this display just above.
                unsafe { vaDestroyConfig(va.dpy, config) };
                return Err(error);
            }
        };
        Ok(Self {
            va: va.clone(),
            config,
            context,
        })
    }

    /// Queues the conversion of `input` into `output`. The sizes are the pictures' own, the
    /// part of each surface that is meant (padding is left alone); when they differ the picture
    /// is scaled.
    pub fn convert(
        &self,
        input: VASurfaceID,
        source: Source,
        input_size: (u32, u32),
        output: VASurfaceID,
        output_size: (u32, u32),
    ) -> Result<()> {
        let region = |(width, height): (u32, u32)| VARectangle {
            x: 0,
            y: 0,
            width: width as u16,
            height: height as u16,
        };
        let (from, to) = (region(input_size), region(output_size));
        // SAFETY: plain data; zero means no filters and no references.
        let mut params: VAProcPipelineParameterBuffer = unsafe { std::mem::zeroed() };
        params.surface = input;
        params.surface_region = &raw const from;
        params.output_region = &raw const to;
        params.surface_color_standard = match source {
            Source::Rgb => VAProcColorStandardSRGB,
            Source::Video => VAProcColorStandardBT709,
        };
        params.output_color_standard = VAProcColorStandardBT709;
        params.input_color_properties.color_range = match source {
            Source::Rgb => VA_SOURCE_RANGE_FULL,
            Source::Video => VA_SOURCE_RANGE_REDUCED,
        } as u8;
        params.output_color_properties.color_range = VA_SOURCE_RANGE_REDUCED as u8;
        let mut buffers =
            [self
                .va
                .param_buffer(self.context, VAProcPipelineParameterBufferType, &mut params)?];
        self.va.submit(self.context, output, &mut buffers)
    }

    /// Converts a BT.709 picture into desktop RGB, for reading a captured frame back.
    pub fn to_rgb(
        &self,
        input: VASurfaceID,
        input_size: (u32, u32),
        output: VASurfaceID,
    ) -> Result<()> {
        let from = VARectangle {
            x: 0,
            y: 0,
            width: input_size.0 as u16,
            height: input_size.1 as u16,
        };
        // SAFETY: plain data.
        let mut params: VAProcPipelineParameterBuffer = unsafe { std::mem::zeroed() };
        params.surface = input;
        params.surface_region = &raw const from;
        params.output_region = &raw const from;
        params.surface_color_standard = VAProcColorStandardBT709;
        params.output_color_standard = VAProcColorStandardSRGB;
        params.input_color_properties.color_range = VA_SOURCE_RANGE_REDUCED as u8;
        params.output_color_properties.color_range = VA_SOURCE_RANGE_FULL as u8;
        let mut buffers =
            [self
                .va
                .param_buffer(self.context, VAProcPipelineParameterBufferType, &mut params)?];
        self.va.submit(self.context, output, &mut buffers)
    }
}

impl Drop for Converter {
    fn drop(&mut self) {
        // SAFETY: ids made on this display in `new`.
        unsafe {
            let status = vaDestroyContext(self.va.dpy, self.context);
            let _ = check(self.va.dpy, status, "vaDestroyContext");
            vaDestroyConfig(self.va.dpy, self.config);
        }
    }
}
