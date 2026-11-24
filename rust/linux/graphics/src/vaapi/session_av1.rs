//! The AV1 parameter buffers of an encode session: sequence, rate control, picture and tile
//! group, with the sequence header and the frame header as packed headers. The counterpart of
//! [`super::session_hevc`] for AV1 profile 0.
//!
//! The pictures held as references sit in the eight reference slots by their order: picture
//! `n` is stored in slot `n % held`, so a slot is overwritten just as its picture falls out of
//! the window of pictures a later one may refer to. An inter picture refers to the newest
//! picture held with all seven of its references.
use super::{
    av1_headers::{self, Frame, Stream},
    ffi::*,
    session::{Picture, Session},
};
use anyhow::Result;

/// The most reference slots the stream's level lets the stream keep in use.
pub fn dpb_frames() -> usize {
    8
}

/// The initial quantizer; rate control moves it from there.
const BASE_QINDEX: u8 = 128;

impl Session {
    pub(super) fn av1_buffers(
        &mut self,
        picture: &Picture,
        buffers: &mut Vec<VABufferID>,
    ) -> Result<()> {
        let Picture {
            keyframe,
            rate_changed,
            bits_per_second,
            current,
            held,
            hdr,
        } = *picture;
        let va = self.va.clone();
        let slots = self.references.capacity() as u32;
        let stream = Stream {
            width: self.width,
            height: self.height,
            depth: self.format.depth,
            level_idx: av1_headers::level_idx(self.width, self.height, self.fps),
            color: self.format.color(),
            full_range: self.format.full_range(),
        };

        if keyframe {
            // SAFETY: plain data; zero is a valid start for every field.
            let mut seq: VAEncSequenceParameterBufferAV1 = unsafe { std::mem::zeroed() };
            seq.seq_profile = 0;
            seq.seq_level_idx = stream.level_idx;
            seq.intra_period = u32::MAX;
            seq.ip_period = 1;
            seq.bits_per_second = bits_per_second;
            seq.seq_fields.value = 1 << 8 // enable_order_hint
                | 1 << 12 // enable_cdef
                | u32::from(stream.depth - 8) << 14 // bit_depth_minus8
                | 1 << 17 // subsampling_x
                | 1 << 18; // subsampling_y
            seq.order_hint_bits_minus_1 = (av1_headers::ORDER_HINT_BITS - 1) as u8;
            buffers.push(va.param_buffer(
                self.context,
                VAEncSequenceParameterBufferType,
                &mut seq,
            )?);
        }
        if keyframe || rate_changed {
            self.rate_buffers(bits_per_second, buffers)?;
        }

        let slot = (self.order % slots) as u8;
        let reference = held.last().map(|h| (h.order % slots) as u8);
        let frame = Frame {
            key: keyframe,
            order_hint: (self.order & 0xff) as u8,
            refresh: if keyframe { 0xff } else { 1 << slot },
            slot: reference.unwrap_or(0),
            base_qindex: BASE_QINDEX,
        };

        // SAFETY: plain data.
        let mut pic: VAEncPictureParameterBufferAV1 = unsafe { std::mem::zeroed() };
        pic.frame_width_minus_1 = (self.width - 1) as u16;
        pic.frame_height_minus_1 = (self.height - 1) as u16;
        pic.reconstructed_frame = current;
        pic.coded_buf = self.coded_buf;
        pic.reference_frames = [VA_INVALID_SURFACE; 8];
        for h in held {
            pic.reference_frames[(h.order % slots) as usize] = h.surface;
        }
        pic.ref_frame_idx = [frame.slot; 7];
        pic.primary_ref_frame = if keyframe { 7 } else { 0 };
        pic.order_hint = frame.order_hint;
        pic.refresh_frame_flags = frame.refresh;
        if !keyframe {
            pic.ref_frame_ctrl_l0.value = 1; // search_idx0: the first reference
        }
        pic.picture_flags.value = u32::from(!keyframe) // frame_type: inter or key
            | u32::from(keyframe) << 2 // error_resilient_mode
            | 1 << 9; // enable_frame_obu
        pic.base_qindex = BASE_QINDEX;
        pic.min_base_qindex = 1;
        pic.max_base_qindex = 255;
        pic.mode_control_flags.value = 2 << 7; // tx_mode: select
        let superblocks = (self.width.div_ceil(8) * 2).div_ceil(16);
        let rows = (self.height.div_ceil(8) * 2).div_ceil(16);
        pic.tile_cols = 1;
        pic.tile_rows = 1;
        pic.width_in_sbs_minus_1[0] = (superblocks - 1) as u16;
        pic.height_in_sbs_minus_1[0] = (rows - 1) as u16;
        pic.tile_group_obu_hdr_info.value = 1 << 1; // obu_has_size_field
        buffers.push(va.param_buffer(self.context, VAEncPictureParameterBufferType, &mut pic)?);

        if keyframe {
            self.packed(
                buffers,
                VAEncPackedHeaderSequence,
                &av1_headers::sequence(&stream),
            )?;
        }
        if let Some(m) = hdr {
            // AV1 takes 0.00002 chromaticity as 0.16, the luminance as 24.8 and 18.14 fixed point.
            let chroma = |value: u16| (u32::from(value) * 65536 / 50000).min(65535) as u16;
            let [r, g, b] = m.primaries.map(|[x, y]| (chroma(x), chroma(y)));
            let units = av1_headers::hdr_metadata(
                [r, g, b],
                (chroma(m.white[0]), chroma(m.white[1])),
                u32::from(m.maximum_nits) * 256,
                u32::from(m.minimum) * 16384 / 10000,
                m.max_cll,
                m.max_fall,
            );
            for unit in units {
                self.packed(buffers, VAEncPackedHeaderRawData, &unit)?;
            }
        }
        self.packed(
            buffers,
            VAEncPackedHeaderPicture,
            &av1_headers::frame_header(&stream, &frame),
        )?;
        let mut group = VAEncTileGroupBufferAV1 {
            tg_start: 0,
            tg_end: 0,
            va_reserved: [0; 4],
        };
        buffers.push(va.param_buffer(self.context, VAEncSliceParameterBufferType, &mut group)?);
        Ok(())
    }
}
