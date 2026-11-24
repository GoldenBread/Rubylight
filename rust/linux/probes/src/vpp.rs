//! VA-API video processing: one captured RGB picture converted into the
//! encoder's 4:2:0 format on the GPU, the driver doing the colour conversion.
use crate::va::{Display, check, ffi::*};
use anyhow::{Result, bail};
use std::ptr;

pub struct Converter<'d> {
    display: &'d Display,
    config: VAConfigID,
    context: VAContextID,
    /// Converted pictures, used in turn: the encoder reads one while the next
    /// conversion writes another.
    outputs: Vec<VASurfaceID>,
    next: usize,
}

impl<'d> Converter<'d> {
    pub fn new(
        display: &'d Display,
        width: u32,
        height: u32,
        rt_format: u32,
        pixel: u32,
    ) -> Result<Self> {
        if !display
            .entrypoints(VAProfileNone)
            .contains(&VAEntrypointVideoProc)
        {
            bail!("the driver has no video processing");
        }
        let config = display.create_config(VAProfileNone, VAEntrypointVideoProc, &[])?;
        let mut outputs = display.create_surfaces(rt_format, pixel, width, height, 3)?;
        let context = display.create_context(config, width, height, &mut outputs)?;
        Ok(Self {
            display,
            config,
            context,
            outputs,
            next: 0,
        })
    }

    /// Queues the conversion of `input` and returns the surface it lands in.
    pub fn convert(&mut self, input: VASurfaceID) -> Result<VASurfaceID> {
        let output = self.outputs[self.next];
        self.next = (self.next + 1) % self.outputs.len();
        // SAFETY: plain data; zero means the whole surface, no filters.
        let mut params: VAProcPipelineParameterBuffer = unsafe { std::mem::zeroed() };
        params.surface = input;
        params.surface_region = ptr::null();
        params.output_region = ptr::null();
        params.surface_color_standard = VAProcColorStandardSRGB;
        params.output_color_standard = VAProcColorStandardBT709;
        let mut buffers = [self.display.param_buffer(
            self.context,
            VAProcPipelineParameterBufferType,
            &mut params,
        )?];
        self.display.submit(self.context, output, &mut buffers)?;
        Ok(output)
    }
}

impl Drop for Converter<'_> {
    fn drop(&mut self) {
        let d = self.display;
        // SAFETY: ids made on this display in `new`.
        unsafe {
            let status = vaDestroyContext(d.dpy, self.context);
            let _ = check(d.dpy, status, "vaDestroyContext");
        }
        d.destroy_surfaces(&mut self.outputs);
        // SAFETY: as above.
        unsafe {
            vaDestroyConfig(d.dpy, self.config);
        }
    }
}
