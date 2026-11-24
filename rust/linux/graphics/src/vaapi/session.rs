//! One VA-API encode session: an H.264, HEVC or AV1 low-delay stream of an IDR picture and P
//! pictures that each refer to one earlier picture (the one before, or after a loss report an
//! older one, see [`super::references`]), constant bitrate, headers written by us. The
//! codec-specific parameter buffers are in [`super::session_h264`], [`super::session_hevc`] and
//! [`super::session_av1`]. Ported from the spike's `hevc.rs`.
use super::{
    display::{Va, check},
    ffi::*,
    pool::aligned,
    references::{Held, MAX_REFERENCES, References},
};
use anyhow::{Result, bail};
use butterpollo_media_policy::hdr::Metadata;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Codec {
    H264,
    Hevc,
    Av1,
}

/// The picture format a session encodes: 8 or 10-bit SDR in `colors`, or 10-bit HDR10
/// (BT.2020 primaries, PQ, limited range, mastering display metadata) when `hdr` is set.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Format {
    pub depth: u8,
    pub hdr: bool,
    /// The SDR colour space the pictures are in, which the stream declares.
    pub colors: super::vpp::Colors,
}

impl Format {
    pub const SDR: Self = Self {
        depth: 8,
        hdr: false,
        colors: super::vpp::Colors::BT709_LIMITED,
    };

    pub fn rt_format(self) -> u32 {
        if self.depth > 8 {
            VA_RT_FORMAT_YUV420_10
        } else {
            VA_RT_FORMAT_YUV420
        }
    }

    /// The fourcc of the surfaces the encoder reads.
    pub fn pixel(self) -> u32 {
        if self.depth > 8 {
            super::P010
        } else {
            super::NV12
        }
    }

    /// `colour_primaries`, `transfer_characteristics` and `matrix_coefficients` the stream
    /// declares.
    pub fn color(self) -> (u8, u8, u8) {
        if self.hdr {
            (9, 16, 9)
        } else {
            self.colors.vui()
        }
    }

    /// Whether the stream declares full-range samples.
    pub fn full_range(self) -> bool {
        !self.hdr && self.colors.full_range
    }
}

impl Codec {
    pub fn name(self) -> &'static str {
        match self {
            Self::H264 => "H.264",
            Self::Hevc => "HEVC",
            Self::Av1 => "AV1",
        }
    }
    fn profile(self, depth: u8) -> Result<VAProfile> {
        Ok(match (self, depth) {
            (Self::H264, 8) => VAProfileH264High,
            (Self::Hevc, 8) => VAProfileHEVCMain,
            (Self::Hevc, 10) => VAProfileHEVCMain10,
            (Self::Av1, 8 | 10) => VAProfileAV1Profile0,
            _ => bail!("the {depth}-bit {} encoder is not available", self.name()),
        })
    }
    /// The header kinds the encoder writes itself: the parameter sets and each slice's header.
    /// Mesa's Radeon driver emits no parameter sets without them. AV1 has no slice headers;
    /// the driver reads the sequence and frame headers to learn what the stream says.
    /// An HDR stream also hands the driver its metadata as raw headers.
    fn packed(self, hdr: bool) -> u32 {
        let kinds = match self {
            Self::H264 | Self::Hevc => VA_ENC_PACKED_HEADER_SEQUENCE | VA_ENC_PACKED_HEADER_SLICE,
            Self::Av1 => VA_ENC_PACKED_HEADER_SEQUENCE | VA_ENC_PACKED_HEADER_PICTURE,
        };
        if hdr {
            kinds | VA_ENC_PACKED_HEADER_RAW_DATA
        } else {
            kinds
        }
    }
}

/// What one picture's parameter buffers are made from.
pub(super) struct Picture<'a> {
    pub keyframe: bool,
    pub rate_changed: bool,
    pub bits_per_second: u32,
    /// The surface the picture is reconstructed into.
    pub current: VASurfaceID,
    /// The references held, oldest first; the newest is the one the picture refers to.
    pub held: &'a [Held],
    /// The HDR metadata this picture carries.
    pub hdr: Option<Metadata>,
}

pub struct Session {
    pub(super) va: Arc<Va>,
    pub(super) codec: Codec,
    pub(super) format: Format,
    /// The picture's size and the coded size (whole 16-pixel blocks) the encoder works at.
    pub width: u32,
    pub height: u32,
    pub(super) coded: (u32, u32),
    pub(super) fps: u32,
    config: VAConfigID,
    pub(super) context: VAContextID,
    /// Reconstructed pictures: the ones held as references and one to write the next into.
    pub(super) recon: Vec<VASurfaceID>,
    pub(super) references: References,
    pub(super) coded_buf: VABufferID,
    /// Pictures encoded so far.
    pub(super) frame: u64,
    /// Whether the last picture was the first after a loss report.
    pub(super) recovered: bool,
    /// Pictures since the last IDR picture: the picture order count.
    pub(super) order: u32,
    /// IDR pictures so far, to tell them apart.
    pub(super) idr_count: u32,
    /// The bitrate last sent to the driver, in bits a second.
    pub(super) sent_bitrate: u32,
    /// The HDR metadata to carry, and whether the next picture has to.
    metadata: Option<Metadata>,
    metadata_due: bool,
}

impl Session {
    pub fn new(
        va: &Arc<Va>,
        codec: Codec,
        format: Format,
        width: u32,
        height: u32,
        fps: u32,
    ) -> Result<Self> {
        if width < 64 || height < 64 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            bail!("{width}x{height} is not a size the encoder takes");
        }
        let coded = (aligned(width), aligned(height));
        let profile = codec.profile(format.depth)?;
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
        let wanted = codec.packed(format.hdr);
        if packed_headers & wanted != wanted {
            bail!(
                "the driver takes none of the headers the stream needs from the application \
                 (0x{packed_headers:x} of 0x{wanted:x}) and there is no other way to describe it"
            );
        }
        let rt_format = format.rt_format();
        let rt_formats = va
            .attribute(profile, entry, VAConfigAttribRTFormat)
            .unwrap_or(0);
        if rt_formats & rt_format == 0 {
            bail!(
                "the driver's {} encoder takes no {}-bit pictures (0x{rt_formats:x})",
                codec.name(),
                format.depth
            );
        }
        let attribs = [
            (VAConfigAttribRTFormat, rt_format),
            (VAConfigAttribRateControl, VA_RC_CBR),
            (VAConfigAttribEncPackedHeaders, wanted),
        ];
        let config = va.create_config(profile, entry, &attribs)?;
        let references = MAX_REFERENCES.min(match codec {
            Codec::H264 => super::session_h264::dpb_frames(coded.0, coded.1, fps),
            Codec::Hevc => super::session_hevc::dpb_frames(coded.0, coded.1, fps),
            Codec::Av1 => super::session_av1::dpb_frames(),
        });
        let built = (|| {
            let mut recon =
                va.create_surfaces(rt_format, format.pixel(), coded.0, coded.1, references + 1)?;
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
            format,
            width,
            height,
            coded,
            fps,
            config,
            context,
            recon,
            references: References::new(references),
            coded_buf,
            frame: 0,
            recovered: false,
            order: 0,
            idr_count: 0,
            sent_bitrate: 0,
            metadata: None,
            metadata_due: false,
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
        let held = if keyframe {
            Vec::new()
        } else {
            self.references.held_for(self.order)
        };
        let current = self.references.free(&self.recon);
        let hdr = if self.format.hdr && (keyframe || self.metadata_due) {
            self.metadata_due = false;
            self.metadata
        } else {
            None
        };
        let picture = Picture {
            keyframe,
            rate_changed,
            bits_per_second,
            current,
            held: &held,
            hdr,
        };
        let built = match self.codec {
            Codec::Hevc => self.hevc_buffers(&picture, &mut buffers),
            Codec::H264 => self.h264_buffers(&picture, &mut buffers),
            Codec::Av1 => self.av1_buffers(&picture, &mut buffers),
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
        self.recovered = self.references.commit(current, self.order, keyframe);
        self.order += 1;
        self.va.sync(input)?;
        self.va.read_coded(self.coded_buf, out)?;
        if self.codec == Codec::Av1 {
            // The driver writes the frame's OBUs; the temporal unit's delimiter is the
            // encoder's to put first, as every decoder and the wire format expect.
            out.splice(0..0, super::av1_headers::temporal_delimiter());
        }
        Ok(keyframe)
    }

    /// Whether the driver lets pictures refer to older ones than the one before. Every session
    /// holds several pictures, so this is whether it holds more than one.
    pub fn supports_invalidation(&self) -> bool {
        self.references.capacity() > 1
    }

    /// Pictures numbered `first` to `last` on the wire were lost: the next picture refers to
    /// one before them. False when no held picture is that old and a keyframe is needed.
    pub fn invalidate(&mut self, first: u64, last: u64) -> bool {
        self.references.invalidate(first, last)
    }

    /// The mastering display and content light level an HDR stream carries: with every key
    /// frame, and with the next picture when they change.
    pub fn set_hdr_metadata(&mut self, metadata: Metadata) {
        if self.metadata != Some(metadata) {
            self.metadata = Some(metadata);
            self.metadata_due = true;
        }
    }

    /// The wire number the next picture gets.
    pub fn set_next_frame(&mut self, wire: u64) {
        self.references.set_next(wire);
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
