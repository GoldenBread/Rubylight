//! The HEVC parameter buffers of an encode session: sequence, rate control, picture and slice,
//! with the parameter sets and the slice header as packed headers. Ported from the spike's
//! `hevc.rs`.
use super::{
    ffi::*,
    hevc_headers::{self, Stream},
    session::{Picture, Session},
};
use anyhow::Result;

/// The lowest main-tier level that fits a picture size and rate, as `general_level_idc` (30
/// times the level).
pub fn level_idc(width: u32, height: u32, fps: u32) -> u8 {
    level(width, height, fps).0
}

/// How many pictures the level lets a decoder keep, the one being decoded included, minus
/// that one: the most references the stream can hold (table A.8's `MaxDpbSize`).
pub fn dpb_frames(width: u32, height: u32, fps: u32) -> usize {
    let samples = u64::from(width) * u64::from(height);
    let max = level(width, height, fps).1;
    let size = if samples <= max / 4 {
        16
    } else if samples <= max / 2 {
        12
    } else if samples <= max * 3 / 4 {
        8
    } else {
        6
    };
    size - 1
}

/// The level and its `MaxLumaPs`.
fn level(width: u32, height: u32, fps: u32) -> (u8, u64) {
    let samples = u64::from(width) * u64::from(height);
    let rate = samples * u64::from(fps);
    // (level_idc, MaxLumaPs, MaxLumaSr) from table A.8 and A.10.
    const LEVELS: [(u8, u64, u64); 9] = [
        (93, 552_960, 16_588_800),
        (120, 2_228_224, 66_846_720),
        (123, 2_228_224, 133_693_440),
        (150, 8_912_896, 267_386_880),
        (153, 8_912_896, 534_773_760),
        (156, 8_912_896, 1_069_547_520),
        (180, 35_651_584, 1_069_547_520),
        (183, 35_651_584, 2_139_095_040),
        (186, 35_651_584, 4_278_190_080),
    ];
    LEVELS
        .iter()
        .find(|&&(_, max_samples, max_rate)| samples <= max_samples && rate <= max_rate)
        .map_or((186, 35_651_584), |&(idc, max_samples, _)| {
            (idc, max_samples)
        })
}

impl Session {
    pub(super) fn hevc_buffers(
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
        let (width, height) = self.coded;
        let order = self.order as i32;
        let stream = Stream {
            width,
            height,
            fps: self.fps,
            depth: self.format.depth,
            level_idc: level_idc(width, height, self.fps),
            crop: (width - self.width, height - self.height),
            references: self.references.capacity() as u32,
            color: self.format.color(),
        };

        if keyframe {
            // SAFETY: plain data; zero is a valid start for every field.
            let mut seq: VAEncSequenceParameterBufferHEVC = unsafe { std::mem::zeroed() };
            seq.general_profile_idc = if self.format.depth > 8 { 2 } else { 1 };
            seq.general_level_idc = stream.level_idc;
            seq.general_tier_flag = 0;
            seq.intra_period = u32::MAX;
            seq.intra_idr_period = u32::MAX;
            seq.ip_period = 1;
            seq.bits_per_second = bits_per_second;
            seq.pic_width_in_luma_samples = width as u16;
            seq.pic_height_in_luma_samples = height as u16;
            let depth = u32::from(self.format.depth - 8);
            seq.seq_fields.value = 1 // chroma_format_idc: 4:2:0
                | depth << 3 // bit_depth_luma_minus8
                | depth << 6 // bit_depth_chroma_minus8
                | 1 << 10 // strong_intra_smoothing_enabled_flag
                | 1 << 11 // amp_enabled_flag
                | 1 << 12 // sample_adaptive_offset_enabled_flag
                | 1 << 16; // low_delay_seq
            // 8x8 to 64x64 coding blocks, 4x4 to 32x32 transforms.
            seq.log2_min_luma_coding_block_size_minus3 = 0;
            seq.log2_diff_max_min_luma_coding_block_size = 3;
            seq.log2_min_transform_block_size_minus2 = 0;
            seq.log2_diff_max_min_transform_block_size = 3;
            seq.max_transform_hierarchy_depth_inter = 2;
            seq.max_transform_hierarchy_depth_intra = 2;
            seq.vui_parameters_present_flag = 1;
            seq.vui_fields.value = 1 << 3; // vui_timing_info_present_flag
            seq.vui_num_units_in_tick = 1;
            seq.vui_time_scale = self.fps;
            buffers.push(va.param_buffer(
                self.context,
                VAEncSequenceParameterBufferType,
                &mut seq,
            )?);
            self.packed(
                buffers,
                VAEncPackedHeaderSequence,
                &hevc_headers::parameter_sets(&stream),
            )?;
        }
        if keyframe || rate_changed {
            self.rate_buffers(bits_per_second, buffers)?;
        }

        let invalid = VAPictureHEVC {
            picture_id: VA_INVALID_SURFACE,
            pic_order_cnt: 0,
            flags: VA_PICTURE_HEVC_INVALID,
            va_reserved: [0; 4],
        };
        let orders: Vec<u32> = held.iter().map(|h| h.order).collect();
        // Every held picture is kept; the newest, the last, is the one this picture refers to.
        let frames: Vec<VAPictureHEVC> = held
            .iter()
            .enumerate()
            .map(|(index, h)| VAPictureHEVC {
                picture_id: h.surface,
                pic_order_cnt: h.order as i32,
                flags: if index + 1 == held.len() {
                    VA_PICTURE_HEVC_RPS_ST_CURR_BEFORE
                } else {
                    0
                },
                va_reserved: [0; 4],
            })
            .collect();
        let reference = frames.last().copied().unwrap_or(invalid);

        // SAFETY: plain data.
        let mut pic: VAEncPictureParameterBufferHEVC = unsafe { std::mem::zeroed() };
        pic.decoded_curr_pic = VAPictureHEVC {
            picture_id: current,
            pic_order_cnt: order,
            flags: 0,
            va_reserved: [0; 4],
        };
        pic.reference_frames = [invalid; 15];
        for (slot, frame) in pic.reference_frames.iter_mut().zip(&frames) {
            *slot = *frame;
        }
        pic.coded_buf = self.coded_buf;
        pic.collocated_ref_pic_index = 0xff;
        pic.last_picture = 0;
        pic.pic_init_qp = 26;
        pic.diff_cu_qp_delta_depth = 0;
        pic.ctu_max_bitsize_allowed = 0;
        pic.nal_unit_type = if keyframe {
            hevc_headers::NAL_IDR_W_RADL
        } else {
            hevc_headers::NAL_TRAIL_R
        };
        let coding_type: u32 = if keyframe { 1 } else { 2 };
        pic.pic_fields.value = u32::from(keyframe) // idr_pic_flag
            | coding_type << 1
            | 1 << 4 // reference_pic_flag
            | 1 << 9 // cu_qp_delta_enabled_flag: rate control needs it
            | 1 << 16; // pps_loop_filter_across_slices_enabled_flag
        buffers.push(va.param_buffer(self.context, VAEncPictureParameterBufferType, &mut pic)?);

        // SAFETY: plain data.
        let mut slice: VAEncSliceParameterBufferHEVC = unsafe { std::mem::zeroed() };
        slice.slice_segment_address = 0;
        slice.num_ctu_in_slice = width.div_ceil(64) * height.div_ceil(64);
        slice.slice_type = if keyframe {
            hevc_headers::SLICE_I
        } else {
            hevc_headers::SLICE_P
        } as u8;
        slice.ref_pic_list0 = [invalid; 15];
        slice.ref_pic_list1 = [invalid; 15];
        if !keyframe {
            slice.ref_pic_list0[0] = reference;
        }
        slice.max_num_merge_cand = 5;
        slice.slice_fields.value = 1 // last_slice_of_pic_flag
            | 1 << 5 // slice_sao_luma_flag
            | 1 << 6 // slice_sao_chroma_flag
            | 1 << 12; // slice_loop_filter_across_slices_enabled_flag
        if let Some(m) = hdr {
            // 0.00002 steps and 0.0001 nit, as the metadata already is.
            let sei = hevc_headers::hdr_sei(
                m.primaries,
                m.white,
                u32::from(m.maximum_nits) * 10_000,
                u32::from(m.minimum),
                m.max_cll,
                m.max_fall,
            );
            self.packed(buffers, VAEncPackedHeaderRawData, &sei)?;
        }
        self.packed(
            buffers,
            VAEncPackedHeaderSlice,
            &hevc_headers::slice_header(keyframe, self.order, &orders),
        )?;
        buffers.push(va.param_buffer(self.context, VAEncSliceParameterBufferType, &mut slice)?);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::level_idc;

    #[test]
    fn levels_follow_size_and_rate() {
        assert_eq!(level_idc(1920, 1080, 60), 123);
        assert_eq!(level_idc(2560, 1440, 120), 153);
        assert_eq!(level_idc(3840, 2160, 60), 153);
        assert_eq!(level_idc(3840, 2160, 120), 156);
    }

    #[test]
    fn the_level_bounds_the_pictures_held() {
        assert_eq!(super::dpb_frames(1920, 1088, 60), 5);
        assert_eq!(super::dpb_frames(3840, 2160, 60), 5);
        assert_eq!(super::dpb_frames(1280, 720, 60), 11);
    }
}
