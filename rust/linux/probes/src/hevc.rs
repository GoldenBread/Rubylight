//! A low-latency HEVC encoder on VA-API: an IDR picture, then P pictures that
//! each refer to the one before, no reordering, constant bitrate. This is the
//! shape a stream uses, kept as small as it can be for measuring.
use crate::{
    hevc_headers::{self, Stream},
    va::{Display, NV12, P010, ffi::*},
};
use anyhow::{Result, bail};

pub struct Settings {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate: u32,
    /// 8 or 10 bits a sample (Main or Main 10).
    pub depth: u8,
    /// The driver's speed and quality trade, passed through as it is. None
    /// leaves the driver's default.
    pub quality_level: Option<u32>,
    /// The decoder buffer the rate control fills, in frames of the bitrate.
    pub vbv_frames: f64,
}

pub struct Encoder<'d> {
    display: &'d Display,
    settings: Settings,
    config: VAConfigID,
    context: VAContextID,
    /// Reconstructed pictures: the one being written and the one referred to.
    recon: Vec<VASurfaceID>,
    coded: VABufferID,
    frame: u64,
    /// Pictures since the last IDR picture: the picture order count.
    order: u32,
    stream: Stream,
    /// The packed header kinds the driver takes, for the report.
    pub packed_headers: u32,
}

/// What one submitted picture was.
pub struct Submitted {
    pub keyframe: bool,
}

/// The header kinds this encoder writes itself: the parameter sets and each
/// slice's header. Mesa's Radeon driver emits no parameter sets without them.
const PACKED: u32 = VA_ENC_PACKED_HEADER_SEQUENCE | VA_ENC_PACKED_HEADER_SLICE;

impl<'d> Encoder<'d> {
    pub fn profile(depth: u8) -> VAProfile {
        if depth == 10 {
            VAProfileHEVCMain10
        } else {
            VAProfileHEVCMain
        }
    }

    pub fn input_format(depth: u8) -> (u32, u32) {
        if depth == 10 {
            (VA_RT_FORMAT_YUV420_10, P010)
        } else {
            (VA_RT_FORMAT_YUV420, NV12)
        }
    }

    pub fn new(display: &'d Display, settings: Settings) -> Result<Self> {
        if !settings.width.is_multiple_of(16) || !settings.height.is_multiple_of(16) {
            bail!(
                "{}x{} is not a multiple of 16; this probe sends no cropping",
                settings.width,
                settings.height
            );
        }
        let profile = Self::profile(settings.depth);
        let entry = VAEntrypointEncSlice;
        if !display.entrypoints(profile).contains(&entry) {
            bail!("the driver has no HEVC {}-bit encoder", settings.depth);
        }
        let rate_modes = display
            .attribute(profile, entry, VAConfigAttribRateControl)
            .unwrap_or(0);
        if rate_modes & VA_RC_CBR == 0 {
            bail!("the driver's HEVC encoder has no constant bitrate mode (0x{rate_modes:x})");
        }
        let packed_headers = display
            .attribute(profile, entry, VAConfigAttribEncPackedHeaders)
            .unwrap_or(0);
        if packed_headers & PACKED != PACKED {
            bail!(
                "the driver takes no sequence and slice headers from the application \
                 (0x{packed_headers:x}); this probe has no other way to describe the stream"
            );
        }
        let (rt_format, pixel) = Self::input_format(settings.depth);
        let attribs = [
            (VAConfigAttribRTFormat, rt_format),
            (VAConfigAttribRateControl, VA_RC_CBR),
            (VAConfigAttribEncPackedHeaders, PACKED),
        ];
        let config = display.create_config(profile, entry, &attribs)?;
        let mut recon =
            display.create_surfaces(rt_format, pixel, settings.width, settings.height, 2)?;
        let context =
            display.create_context(config, settings.width, settings.height, &mut recon)?;
        // Room for a keyframe of an incompressible picture.
        let coded = display.coded_buffer(
            context,
            settings.width as usize * settings.height as usize * 3,
        )?;
        let stream = Stream {
            width: settings.width,
            height: settings.height,
            fps: settings.fps,
            depth: settings.depth,
            level_idc: level_idc(settings.width, settings.height, settings.fps),
        };
        Ok(Self {
            display,
            settings,
            config,
            context,
            recon,
            coded,
            frame: 0,
            order: 0,
            stream,
            packed_headers,
        })
    }

    /// Hands one picture to the encoder and returns without waiting for it.
    pub fn submit(&mut self, input: VASurfaceID, force_keyframe: bool) -> Result<Submitted> {
        let s = &self.settings;
        let keyframe = self.frame == 0 || force_keyframe;
        let current = self.recon[(self.frame % 2) as usize];
        let previous = self.recon[((self.frame + 1) % 2) as usize];
        // The count restarts at each IDR picture.
        if keyframe {
            self.order = 0;
        }
        let order = self.order as i32;
        let mut buffers = Vec::with_capacity(8);
        let d = self.display;

        if keyframe {
            // SAFETY: plain data; zero is a valid start for every field.
            let mut seq: VAEncSequenceParameterBufferHEVC = unsafe { std::mem::zeroed() };
            seq.general_profile_idc = if s.depth == 10 { 2 } else { 1 };
            seq.general_level_idc = self.stream.level_idc;
            seq.general_tier_flag = 0;
            seq.intra_period = u32::MAX;
            seq.intra_idr_period = u32::MAX;
            seq.ip_period = 1;
            seq.bits_per_second = s.bitrate;
            seq.pic_width_in_luma_samples = s.width as u16;
            seq.pic_height_in_luma_samples = s.height as u16;
            let depth_minus8 = u32::from(s.depth - 8);
            seq.seq_fields.value = 1 // chroma_format_idc: 4:2:0
                | depth_minus8 << 3
                | depth_minus8 << 6
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
            seq.vui_time_scale = s.fps;
            buffers.push(d.param_buffer(
                self.context,
                VAEncSequenceParameterBufferType,
                &mut seq,
            )?);

            // SAFETY: plain data.
            let mut rate: VAEncMiscParameterRateControl = unsafe { std::mem::zeroed() };
            rate.bits_per_second = s.bitrate;
            rate.target_percentage = 100;
            rate.window_size = 1000;
            rate.initial_qp = 26;
            rate.min_qp = 1;
            rate.max_qp = 51;
            buffers.push(d.misc_buffer(self.context, VAEncMiscParameterTypeRateControl, rate)?);

            // SAFETY: plain data.
            let mut hrd: VAEncMiscParameterHRD = unsafe { std::mem::zeroed() };
            let buffer = (f64::from(s.bitrate) / f64::from(s.fps) * s.vbv_frames) as u32;
            hrd.buffer_size = buffer;
            hrd.initial_buffer_fullness = buffer / 2;
            buffers.push(d.misc_buffer(self.context, VAEncMiscParameterTypeHRD, hrd)?);

            // SAFETY: plain data.
            let mut rate_fps: VAEncMiscParameterFrameRate = unsafe { std::mem::zeroed() };
            rate_fps.framerate = s.fps | 1 << 16;
            buffers.push(d.misc_buffer(self.context, VAEncMiscParameterTypeFrameRate, rate_fps)?);

            self.packed(
                &mut buffers,
                VAEncPackedHeaderSequence,
                &hevc_headers::parameter_sets(&self.stream),
            )?;

            if let Some(level) = s.quality_level {
                // SAFETY: plain data.
                let mut quality: VAEncMiscParameterBufferQualityLevel =
                    unsafe { std::mem::zeroed() };
                quality.quality_level = level;
                buffers.push(d.misc_buffer(
                    self.context,
                    VAEncMiscParameterTypeQualityLevel,
                    quality,
                )?);
            }
        }

        let invalid = VAPictureHEVC {
            picture_id: VA_INVALID_SURFACE,
            pic_order_cnt: 0,
            flags: VA_PICTURE_HEVC_INVALID,
            va_reserved: [0; 4],
        };
        let reference = VAPictureHEVC {
            picture_id: previous,
            pic_order_cnt: order - 1,
            flags: VA_PICTURE_HEVC_RPS_ST_CURR_BEFORE,
            va_reserved: [0; 4],
        };

        // SAFETY: plain data.
        let mut pic: VAEncPictureParameterBufferHEVC = unsafe { std::mem::zeroed() };
        pic.decoded_curr_pic = VAPictureHEVC {
            picture_id: current,
            pic_order_cnt: order,
            flags: 0,
            va_reserved: [0; 4],
        };
        pic.reference_frames = [invalid; 15];
        if !keyframe {
            pic.reference_frames[0] = reference;
        }
        pic.coded_buf = self.coded;
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
        buffers.push(d.param_buffer(self.context, VAEncPictureParameterBufferType, &mut pic)?);

        // SAFETY: plain data.
        let mut slice: VAEncSliceParameterBufferHEVC = unsafe { std::mem::zeroed() };
        slice.slice_segment_address = 0;
        slice.num_ctu_in_slice = s.width.div_ceil(64) * s.height.div_ceil(64);
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
        self.packed(
            &mut buffers,
            VAEncPackedHeaderSlice,
            &hevc_headers::slice_header(keyframe, self.order),
        )?;
        buffers.push(d.param_buffer(self.context, VAEncSliceParameterBufferType, &mut slice)?);

        d.submit(self.context, input, &mut buffers)?;
        self.frame += 1;
        self.order += 1;
        Ok(Submitted { keyframe })
    }

    /// A header this encoder wrote, as the two buffers libva takes it in:
    /// what it is and how long, then its bytes.
    fn packed(&self, buffers: &mut Vec<VABufferID>, kind: u32, bytes: &[u8]) -> Result<()> {
        let mut param = VAEncPackedHeaderParameterBuffer {
            type_: kind,
            bit_length: bytes.len() as u32 * 8,
            has_emulation_bytes: 1,
            va_reserved: [0; 4],
        };
        buffers.push(self.display.param_buffer(
            self.context,
            VAEncPackedHeaderParameterBufferType,
            &mut param,
        )?);
        buffers.push(self.display.data_buffer(
            self.context,
            VAEncPackedHeaderDataBufferType,
            bytes,
        )?);
        Ok(())
    }

    /// Waits for the picture submitted on `input` and copies it into `out`.
    pub fn finish(&mut self, input: VASurfaceID, out: &mut Vec<u8>) -> Result<()> {
        self.display.sync(input)?;
        self.display.read_coded(self.coded, out)
    }
}

impl Drop for Encoder<'_> {
    fn drop(&mut self) {
        let d = self.display;
        d.destroy_buffer(self.coded);
        // SAFETY: ids made on this display in `new`.
        unsafe {
            vaDestroyContext(d.dpy, self.context);
        }
        d.destroy_surfaces(&mut self.recon);
        // SAFETY: as above.
        unsafe {
            vaDestroyConfig(d.dpy, self.config);
        }
    }
}

/// The lowest main-tier level that fits a picture size and rate, as
/// `general_level_idc` (30 times the level).
fn level_idc(width: u32, height: u32, fps: u32) -> u8 {
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
        .map_or(186, |&(idc, _, _)| idc)
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
}
