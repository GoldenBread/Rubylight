//! The H.264 parameter buffers of an encode session: sequence, rate control, picture and slice,
//! with the parameter sets and the slice header as packed headers. The counterpart of
//! [`super::session_hevc`] for High profile.
use super::{
    ffi::*,
    h264_headers::{self, Stream},
    session::Session,
};
use anyhow::Result;

/// The lowest level that fits a picture size and rate, as `level_idc` (10 times the level).
pub fn level_idc(width: u32, height: u32, fps: u32) -> u8 {
    let blocks = u64::from(width / 16) * u64::from(height / 16);
    let rate = blocks * u64::from(fps);
    // (level_idc, MaxFS, MaxMBPS) from table A-1.
    const LEVELS: [(u8, u64, u64); 11] = [
        (30, 1_620, 40_500),
        (31, 3_600, 108_000),
        (32, 5_120, 216_000),
        (40, 8_192, 245_760),
        (42, 8_704, 522_240),
        (50, 22_080, 589_824),
        (51, 36_864, 983_040),
        (52, 36_864, 2_073_600),
        (60, 139_264, 4_177_920),
        (61, 139_264, 8_355_840),
        (62, 139_264, 16_711_680),
    ];
    LEVELS
        .iter()
        .find(|&&(_, max_blocks, max_rate)| blocks <= max_blocks && rate <= max_rate)
        .map_or(62, |&(idc, _, _)| idc)
}

fn picture(id: VASurfaceID, frame_num: u32, flags: u32) -> VAPictureH264 {
    VAPictureH264 {
        picture_id: id,
        frame_idx: frame_num,
        flags,
        // Picture order follows decoding order (`pic_order_cnt_type` 2): two counts a frame.
        TopFieldOrderCnt: (frame_num * 2) as i32,
        BottomFieldOrderCnt: (frame_num * 2) as i32,
        va_reserved: [0; 4],
    }
}

impl Session {
    pub(super) fn h264_buffers(
        &mut self,
        keyframe: bool,
        rate_changed: bool,
        bits_per_second: u32,
        buffers: &mut Vec<VABufferID>,
    ) -> Result<()> {
        let va = self.va.clone();
        let (width, height) = self.coded;
        let current = self.recon[(self.frame % 2) as usize];
        let previous = self.recon[((self.frame + 1) % 2) as usize];
        let frame_num = self.order & ((1 << h264_headers::LOG2_MAX_FRAME_NUM) - 1);
        let stream = Stream {
            width,
            height,
            fps: self.fps,
            level_idc: level_idc(width, height, self.fps),
            crop: (width - self.width, height - self.height),
        };

        if keyframe {
            // SAFETY: plain data; zero is a valid start for every field.
            let mut seq: VAEncSequenceParameterBufferH264 = unsafe { std::mem::zeroed() };
            seq.level_idc = stream.level_idc;
            seq.intra_period = u32::MAX;
            seq.intra_idr_period = u32::MAX;
            seq.ip_period = 1;
            seq.bits_per_second = bits_per_second;
            seq.max_num_ref_frames = 1;
            seq.picture_width_in_mbs = (width / 16) as u16;
            seq.picture_height_in_mbs = (height / 16) as u16;
            seq.seq_fields.value = 1 // chroma_format_idc: 4:2:0
                | 1 << 2 // frame_mbs_only_flag
                | 1 << 5 // direct_8x8_inference_flag
                | (h264_headers::LOG2_MAX_FRAME_NUM - 4) << 6
                | 2 << 10; // pic_order_cnt_type
            if stream.crop != (0, 0) {
                seq.frame_cropping_flag = 1;
                seq.frame_crop_right_offset = stream.crop.0 / 2;
                seq.frame_crop_bottom_offset = stream.crop.1 / 2;
            }
            seq.vui_parameters_present_flag = 1;
            seq.vui_fields.value = 1 << 1; // timing_info_present_flag
            seq.num_units_in_tick = 1;
            seq.time_scale = self.fps * 2;
            buffers.push(va.param_buffer(
                self.context,
                VAEncSequenceParameterBufferType,
                &mut seq,
            )?);
            self.packed(
                buffers,
                VAEncPackedHeaderSequence,
                &h264_headers::sequence_set(&stream),
            )?;
            self.packed(
                buffers,
                VAEncPackedHeaderPicture,
                &h264_headers::picture_set(),
            )?;
        }
        if keyframe || rate_changed {
            self.rate_buffers(bits_per_second, buffers)?;
        }

        let invalid = VAPictureH264 {
            picture_id: VA_INVALID_SURFACE,
            frame_idx: 0,
            flags: VA_PICTURE_H264_INVALID,
            TopFieldOrderCnt: 0,
            BottomFieldOrderCnt: 0,
            va_reserved: [0; 4],
        };
        let reference = picture(
            previous,
            frame_num.wrapping_sub(1) & ((1 << h264_headers::LOG2_MAX_FRAME_NUM) - 1),
            VA_PICTURE_H264_SHORT_TERM_REFERENCE,
        );

        // SAFETY: plain data.
        let mut pic: VAEncPictureParameterBufferH264 = unsafe { std::mem::zeroed() };
        pic.CurrPic = picture(current, frame_num, 0);
        pic.ReferenceFrames = [invalid; 16];
        if !keyframe {
            pic.ReferenceFrames[0] = reference;
        }
        pic.coded_buf = self.coded_buf;
        pic.frame_num = frame_num as u16;
        pic.pic_init_qp = 26;
        pic.pic_fields.value = u32::from(keyframe) // idr_pic_flag
            | 1 << 1 // reference_pic_flag
            | u32::from(h264_headers::CABAC) << 3 // entropy_coding_mode_flag
            | 1 << 8 // transform_8x8_mode_flag
            | 1 << 9; // deblocking_filter_control_present_flag
        buffers.push(va.param_buffer(self.context, VAEncPictureParameterBufferType, &mut pic)?);

        // SAFETY: plain data.
        let mut slice: VAEncSliceParameterBufferH264 = unsafe { std::mem::zeroed() };
        slice.macroblock_address = 0;
        slice.num_macroblocks = (width / 16) * (height / 16);
        slice.slice_type = if keyframe {
            h264_headers::SLICE_I
        } else {
            h264_headers::SLICE_P
        } as u8;
        slice.idr_pic_id = (self.idr_count & 0xffff) as u16;
        slice.RefPicList0 = [invalid; 32];
        slice.RefPicList1 = [invalid; 32];
        if !keyframe {
            slice.RefPicList0[0] = reference;
        }
        self.packed(
            buffers,
            VAEncPackedHeaderSlice,
            &h264_headers::slice_header(keyframe, frame_num, self.idr_count),
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
        assert_eq!(level_idc(640, 480, 30), 30);
        assert_eq!(level_idc(1920, 1088, 60), 42);
        assert_eq!(level_idc(2560, 1440, 60), 51);
        assert_eq!(level_idc(3840, 2160, 60), 52);
    }
}
