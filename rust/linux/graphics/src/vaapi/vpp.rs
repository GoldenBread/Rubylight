//! VA-API video processing: one picture converted, copied or scaled into another surface on the
//! GPU, the driver doing the colour conversion. Ported from the spike's `vpp.rs`.
use super::{
    display::{Va, check},
    ffi::*,
};
use anyhow::{Result, bail};
use std::sync::Arc;

/// A YCbCr colour space: the stream's matrix as the client negotiated it (0 BT.601, 1 BT.709,
/// 2 BT.2020) and its range.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Colors {
    pub matrix: u8,
    pub full_range: bool,
}

impl Colors {
    /// What gamescope's NV12 pictures are, and what a stream is when nothing else is asked.
    pub const BT709_LIMITED: Self = Self {
        matrix: 1,
        full_range: false,
    };

    /// Describes this colour space to video processing. A named standard (`BT601`, `BT709`,
    /// ...) has the driver convert the transfer and primaries as well: Mesa 26.3 on an
    /// RX 7800 XT wrote luma 123 for an sRGB (200, 100, 50) as BT.709, where the matrix alone
    /// gives 117, and changed the gamut for BT.2020. So both sides are described explicitly,
    /// with the same primaries and transfer, and only the matrix and range differ: what the
    /// Windows host's conversion does. (`colour_tests` measures it.)
    fn describe(
        self,
        standard: &mut VAProcColorStandardType,
        properties: &mut VAProcColorProperties,
    ) {
        let coefficients = match self.matrix {
            0 => 6,
            2 => 9,
            _ => 1,
        };
        explicit(standard, properties, coefficients, self.full_range);
    }

    /// `colour_primaries`, `transfer_characteristics` and `matrix_coefficients` (ITU-T H.273)
    /// an SDR stream in this colour space declares, as the Windows host's AMF encoder does.
    pub fn vui(self) -> (u8, u8, u8) {
        match self.matrix {
            0 => (6, 6, 6),
            2 => (9, 14, 9),
            _ => (1, 1, 1),
        }
    }
}

/// Desktop RGB, described as [`Colors::describe`] describes video: full range, no matrix.
fn describe_rgb(standard: &mut VAProcColorStandardType, properties: &mut VAProcColorProperties) {
    explicit(standard, properties, 0, true);
}

/// BT.709 primaries and the sRGB transfer on every side, so neither is converted.
fn explicit(
    standard: &mut VAProcColorStandardType,
    properties: &mut VAProcColorProperties,
    matrix_coefficients: u8,
    full_range: bool,
) {
    *standard = VAProcColorStandardExplicit;
    properties.colour_primaries = 1;
    properties.transfer_characteristics = 13;
    properties.matrix_coefficients = matrix_coefficients;
    properties.color_range = if full_range {
        VA_SOURCE_RANGE_FULL
    } else {
        VA_SOURCE_RANGE_REDUCED
    } as u8;
}

impl From<butterpollo_media_policy::capture_policy::Picture> for Colors {
    fn from(picture: butterpollo_media_policy::capture_policy::Picture) -> Self {
        Self {
            matrix: picture.color_matrix,
            full_range: picture.full_range,
        }
    }
}

impl Default for Colors {
    fn default() -> Self {
        Self::BT709_LIMITED
    }
}

/// What the source picture's colours are.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    /// Desktop RGB: sRGB primaries, full range.
    Rgb,
    /// Video in this colour space.
    Video(Colors),
}

/// A video processing context for pictures that end up `width`x`height`, writing video in
/// one colour space (BT.709 limited unless [`Converter::writing`] says otherwise).
pub struct Converter {
    va: Arc<Va>,
    config: VAConfigID,
    context: VAContextID,
    output: Colors,
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
            output: Colors::BT709_LIMITED,
        })
    }

    /// The converter, writing video in `colors` (and reading it back in them, [`Self::to_rgb`]).
    pub fn writing(mut self, colors: Colors) -> Self {
        self.output = colors;
        self
    }

    pub fn colors(&self) -> Colors {
        self.output
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
        let input = (
            &mut params.surface_color_standard,
            &mut params.input_color_properties,
        );
        match source {
            Source::Rgb => describe_rgb(input.0, input.1),
            Source::Video(colors) => colors.describe(input.0, input.1),
        }
        self.output.describe(
            &mut params.output_color_standard,
            &mut params.output_color_properties,
        );
        let mut buffers =
            [self
                .va
                .param_buffer(self.context, VAProcPipelineParameterBufferType, &mut params)?];
        self.va.submit(self.context, output, &mut buffers)
    }

    /// Copies an RGB picture into another RGB surface, scaled to `output_size`, colours as they
    /// are: what the packed 4:4:4 pass reads.
    pub fn scale_rgb(
        &self,
        input: VASurfaceID,
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
        // SAFETY: plain data; zero is a valid start for every field.
        let mut params: VAProcPipelineParameterBuffer = unsafe { std::mem::zeroed() };
        params.surface = input;
        params.surface_region = &raw const from;
        params.output_region = &raw const to;
        params.surface_color_standard = VAProcColorStandardSRGB;
        params.output_color_standard = VAProcColorStandardSRGB;
        params.input_color_properties.color_range = VA_SOURCE_RANGE_FULL as u8;
        params.output_color_properties.color_range = VA_SOURCE_RANGE_FULL as u8;
        let mut buffers =
            [self
                .va
                .param_buffer(self.context, VAProcPipelineParameterBufferType, &mut params)?];
        self.va.submit(self.context, output, &mut buffers)
    }

    /// Converts a picture in the colours this converter writes into desktop RGB, for reading a
    /// captured frame back.
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
        self.output.describe(
            &mut params.surface_color_standard,
            &mut params.input_color_properties,
        );
        describe_rgb(
            &mut params.output_color_standard,
            &mut params.output_color_properties,
        );
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
