//! The H.264 parameter buffers of an encode session: sequence, rate control, picture and slice,
//! with the parameter sets and the slice header as packed headers. The counterpart of
//! [`super::session_hevc`] for High profile.
use super::{
    ffi::*,
    h264_headers::{self, Stream},
    session::{Picture, Session},
};
use anyhow::Result;

/// The lowest level that fits a picture size and rate, as `level_idc` (10 times the level).
pub fn level_idc(width: u32, height: u32, fps: u32) -> u8 {
    level(width, height, fps).0
}

/// The level and its decoded picture buffer, in 16x16 blocks.
fn level(width: u32, height: u32, fps: u32) -> (u8, u64) {
    let blocks = u64::from(width / 16) * u64::from(height / 16);
    let rate = blocks * u64::from(fps);
    // (level_idc, MaxFS, MaxMBPS, MaxDpbMbs) from table A-1.
    const LEVELS: [(u8, u64, u64, u64); 11] = [
        (30, 1_620, 40_500, 8_100),
        (31, 3_600, 108_000, 18_000),
        (32, 5_120, 216_000, 20_480),
        (40, 8_192, 245_760, 32_768),
        (42, 8_704, 522_240, 34_816),
        (50, 22_080, 589_824, 110_400),
        (51, 36_864, 983_040, 184_320),
        (52, 36_864, 2_073_600, 184_320),
        (60, 139_264, 4_177_920, 696_320),
        (61, 139_264, 8_355_840, 696_320),
        (62, 139_264, 16_711_680, 696_320),
    ];
    LEVELS
        .iter()
        .find(|&&(_, max_blocks, max_rate, _)| blocks <= max_blocks && rate <= max_rate)
        .map_or((62, 696_320), |&(idc, _, _, dpb)| (idc, dpb))
}

/// How many pictures the level lets a decoder keep: the most references the stream can hold.
pub fn dpb_frames(width: u32, height: u32, fps: u32) -> usize {
    let blocks = (u64::from(width / 16) * u64::from(height / 16)).max(1);
    (level(width, height, fps).1 / blocks).clamp(1, 16) as usize
}

fn picture_of(id: VASurfaceID, frame_num: u32, flags: u32) -> VAPictureH264 {
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
        picture: &Picture,
        buffers: &mut Vec<VABufferID>,
    ) -> Result<()> {
        let Picture {
            keyframe,
            rate_changed,
            bits_per_second,
            current,
            held,
            hdr: _,
        } = *picture;
        let va = self.va.clone();
        let (width, height) = self.coded;
        let mask = (1 << h264_headers::LOG2_MAX_FRAME_NUM) - 1;
        let frame_num = self.order & mask;
        let stream = Stream {
            width,
            height,
            fps: self.fps,
            level_idc: level_idc(width, height, self.fps),
            crop: (width - self.width, height - self.height),
            references: self.references.capacity() as u32,
            color: self.format.color(),
            full_range: self.format.full_range(),
        };

        if keyframe {
            // SAFETY: plain data; zero is a valid start for every field.
            let mut seq: VAEncSequenceParameterBufferH264 = unsafe { std::mem::zeroed() };
            seq.level_idc = stream.level_idc;
            seq.intra_period = u32::MAX;
            seq.intra_idr_period = u32::MAX;
            seq.ip_period = 1;
            seq.bits_per_second = bits_per_second;
            seq.max_num_ref_frames = stream.references;
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
        let frames: Vec<VAPictureH264> = held
            .iter()
            .map(|h| {
                picture_of(
                    h.surface,
                    h.order & mask,
                    VA_PICTURE_H264_SHORT_TERM_REFERENCE,
                )
            })
            .collect();
        // The picture refers to the newest one held, which a loss report may have made older
        // than the one before.
        let (reference, distance) = match (frames.last(), held.last()) {
            (Some(&frame), Some(newest)) => (frame, self.order - newest.order),
            _ => (invalid, 1),
        };

        // SAFETY: plain data.
        let mut pic: VAEncPictureParameterBufferH264 = unsafe { std::mem::zeroed() };
        pic.CurrPic = picture_of(current, frame_num, 0);
        pic.ReferenceFrames = [invalid; 16];
        for (slot, frame) in pic.ReferenceFrames.iter_mut().zip(&frames) {
            *slot = *frame;
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
            &h264_headers::slice_header(keyframe, frame_num, self.idr_count, distance),
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

    #[test]
    fn the_level_bounds_the_pictures_held() {
        assert_eq!(super::dpb_frames(1920, 1088, 60), 4);
        assert_eq!(super::dpb_frames(3840, 2160, 60), 5);
        assert_eq!(super::dpb_frames(1280, 720, 60), 5);
    }
}
