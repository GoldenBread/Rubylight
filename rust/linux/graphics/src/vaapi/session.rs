//! One VA-API encode session: an H.264 or HEVC low-delay stream of an IDR picture and P
//! pictures that each refer to the one before, constant bitrate, headers written by us. The
//! codec-specific parameter buffers are in [`super::session_h264`] and [`super::session_hevc`].
//! Ported from the spike's `hevc.rs`.
use super::{
    display::{Va, check},
    ffi::*,
    pool::aligned,
};
use anyhow::{Result, bail};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Codec {
    H264,
    Hevc,
}

impl Codec {
    pub fn name(self) -> &'static str {
        match self {
            Self::H264 => "H.264",
            Self::Hevc => "HEVC",
        }
    }
    fn profile(self) -> VAProfile {
        match self {
            Self::H264 => VAProfileH264High,
            Self::Hevc => VAProfileHEVCMain,
        }
    }
}

/// The header kinds the encoders write themselves: the parameter sets and each slice's header.
/// Mesa's Radeon driver emits no parameter sets without them.
const PACKED: u32 = VA_ENC_PACKED_HEADER_SEQUENCE | VA_ENC_PACKED_HEADER_SLICE;

pub struct Session {
    pub(super) va: Arc<Va>,
    pub(super) codec: Codec,
    /// The picture's size and the coded size (whole 16-pixel blocks) the encoder works at.
    pub width: u32,
    pub height: u32,
    pub(super) coded: (u32, u32),
    pub(super) fps: u32,
    config: VAConfigID,
    pub(super) context: VAContextID,
    /// Reconstructed pictures: the one being written and the one referred to.
    pub(super) recon: Vec<VASurfaceID>,
    pub(super) coded_buf: VABufferID,
    /// Pictures encoded so far.
    pub(super) frame: u64,
    /// Pictures since the last IDR picture: the picture order count.
    pub(super) order: u32,
    /// IDR pictures so far, to tell them apart.
    pub(super) idr_count: u32,
    /// The bitrate last sent to the driver, in bits a second.
    pub(super) sent_bitrate: u32,
}

impl Session {
    pub fn new(va: &Arc<Va>, codec: Codec, width: u32, height: u32, fps: u32) -> Result<Self> {
        if width < 64 || height < 64 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            bail!("{width}x{height} is not a size the encoder takes");
        }
        let coded = (aligned(width), aligned(height));
        let profile = codec.profile();
        let entry = VAEntrypointEncSlice;
        if !va.entrypoints(profile).contains(&entry) {
            bail!("the GPU's VA-API driver has no {} encoder", codec.name());
        }
        let rate_modes = va
            .attribute(profile, entry, VAConfigAttribRateControl)
            .unwrap_or(0);
        if rate_modes & VA_RC_CBR == 0 {
            bail!(
                "the driver's {} encoder has no constant bitrate mode (0x{rate_modes:x})",
                codec.name()
            );
        }
        let packed_headers = va
            .attribute(profile, entry, VAConfigAttribEncPackedHeaders)
            .unwrap_or(0);
        if packed_headers & PACKED != PACKED {
            bail!(
                "the driver takes no sequence and slice headers from the application \
                 (0x{packed_headers:x}) and there is no other way to describe the stream"
            );
        }
        let attribs = [
            (VAConfigAttribRTFormat, VA_RT_FORMAT_YUV420),
            (VAConfigAttribRateControl, VA_RC_CBR),
            (VAConfigAttribEncPackedHeaders, PACKED),
        ];
        let config = va.create_config(profile, entry, &attribs)?;
        let built = (|| {
            let mut recon =
                va.create_surfaces(VA_RT_FORMAT_YUV420, super::NV12, coded.0, coded.1, 2)?;
            let context = match va.create_context(config, coded.0, coded.1, &mut recon) {
                Ok(context) => context,
                Err(error) => {
                    va.destroy_surfaces(&mut recon);
                    return Err(error);
                }
            };
            // Room for a keyframe of an incompressible picture.
            match va.coded_buffer(context, coded.0 as usize * coded.1 as usize * 3) {
                Ok(buffer) => Ok((recon, context, buffer)),
                Err(error) => {
                    // SAFETY: made on this display just above.
                    unsafe { vaDestroyContext(va.dpy, context) };
                    va.destroy_surfaces(&mut recon);
                    Err(error)
                }
            }
        })();
        let (recon, context, coded_buf) = match built {
            Ok(parts) => parts,
            Err(error) => {
                // SAFETY: made on this display above.
                unsafe { vaDestroyConfig(va.dpy, config) };
                return Err(error);
            }
        };
        Ok(Self {
            va: va.clone(),
            codec,
            width,
            height,
            coded,
            fps,
            config,
            context,
            recon,
            coded_buf,
            frame: 0,
            order: 0,
            idr_count: 0,
            sent_bitrate: 0,
        })
    }

    /// Encodes the picture on `input` (an NV12 surface of the coded size) and appends it to
    /// `out` as a byte stream, after the parameter sets when it is an IDR picture. Returns
    /// whether the picture was an IDR picture: the first one always is. Waits for the encoder.
    pub fn encode(
        &mut self,
        input: VASurfaceID,
        force_idr: bool,
        bits_per_second: u32,
        out: &mut Vec<u8>,
    ) -> Result<bool> {
        let keyframe = self.frame == 0 || force_idr;
        if keyframe {
            self.order = 0;
            self.idr_count = self.idr_count.wrapping_add(1);
        }
        let mut buffers = Vec::with_capacity(12);
        let rate_changed = bits_per_second != self.sent_bitrate;
        let built = match self.codec {
            Codec::Hevc => self.hevc_buffers(keyframe, rate_changed, bits_per_second, &mut buffers),
            Codec::H264 => self.h264_buffers(keyframe, rate_changed, bits_per_second, &mut buffers),
        };
        if let Err(error) = built {
            for id in buffers {
                self.va.destroy_buffer(id);
            }
            return Err(error);
        }
        self.va.submit(self.context, input, &mut buffers)?;
        self.sent_bitrate = bits_per_second;
        self.frame += 1;
        self.order += 1;
        self.va.sync(input)?;
        self.va.read_coded(self.coded_buf, out)?;
        Ok(keyframe)
    }

    /// The constant-bitrate parameters: the rate, the decoder buffer the rate control fills (two
    /// frames of the bitrate) and the frame rate.
    pub(super) fn rate_buffers(
        &self,
        bits_per_second: u32,
        buffers: &mut Vec<VABufferID>,
    ) -> Result<()> {
        let va = &self.va;
        // SAFETY: plain data; zero is a valid start for every field.
        let mut rate: VAEncMiscParameterRateControl = unsafe { std::mem::zeroed() };
        rate.bits_per_second = bits_per_second;
        rate.target_percentage = 100;
        rate.window_size = 1000;
        rate.initial_qp = 26;
        rate.min_qp = 1;
        rate.max_qp = 51;
        buffers.push(va.misc_buffer(self.context, VAEncMiscParameterTypeRateControl, rate)?);

        // SAFETY: plain data.
        let mut hrd: VAEncMiscParameterHRD = unsafe { std::mem::zeroed() };
        let size = (f64::from(bits_per_second) / f64::from(self.fps) * 2.0) as u32;
        hrd.buffer_size = size;
        hrd.initial_buffer_fullness = size / 2;
        buffers.push(va.misc_buffer(self.context, VAEncMiscParameterTypeHRD, hrd)?);

        // SAFETY: plain data.
        let mut frame_rate: VAEncMiscParameterFrameRate = unsafe { std::mem::zeroed() };
        frame_rate.framerate = self.fps | 1 << 16;
        buffers.push(va.misc_buffer(self.context, VAEncMiscParameterTypeFrameRate, frame_rate)?);
        Ok(())
    }

    /// A header this encoder wrote, as the two buffers libva takes it in: what it is and how
    /// long, then its bytes.
    pub(super) fn packed(
        &self,
        buffers: &mut Vec<VABufferID>,
        kind: u32,
        bytes: &[u8],
    ) -> Result<()> {
        let mut param = VAEncPackedHeaderParameterBuffer {
            type_: kind,
            bit_length: bytes.len() as u32 * 8,
            has_emulation_bytes: 1,
            va_reserved: [0; 4],
        };
        buffers.push(self.va.param_buffer(
            self.context,
            VAEncPackedHeaderParameterBufferType,
            &mut param,
        )?);
        buffers.push(
            self.va
                .data_buffer(self.context, VAEncPackedHeaderDataBufferType, bytes)?,
        );
        Ok(())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let va = &self.va;
        va.destroy_buffer(self.coded_buf);
        // SAFETY: ids made on this display in `new`.
        unsafe {
            let status = vaDestroyContext(va.dpy, self.context);
            let _ = check(va.dpy, status, "vaDestroyContext");
        }
        va.destroy_surfaces(&mut self.recon);
        // SAFETY: as above.
        unsafe { vaDestroyConfig(va.dpy, self.config) };
    }
}
