//! The HEVC headers an application hands a VA-API driver: video, sequence and
//! picture parameter sets and each slice's header, written bit by bit as
//! ITU-T H.265 section 7.3 lays them out. Mesa's Radeon driver reads these to
//! learn what the stream should say, then writes the headers of its output.
//!
//! One layer, one sub-layer, 4:2:0, no reordering: each picture refers to the
//! one before it and its reference set travels in its slice header. Ported from the spike
//! (`rust/linux/probes/src/hevc_headers.rs`), with the cropping window added for sizes that are
//! not whole 16-pixel blocks.

use super::bits::{Bits, nal};

/// What the parameter sets describe.
#[derive(Clone, Copy, Debug)]
pub struct Stream {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// 8 or 10.
    pub depth: u8,
    pub level_idc: u8,
    /// Pixels cut from the right and bottom of the coded picture, in luma samples (even).
    pub crop: (u32, u32),
    /// Pictures held as references.
    pub references: u32,
    /// `colour_primaries`, `transfer_characteristics` and `matrix_coeffs` of the VUI.
    pub color: (u8, u8, u8),
    /// `video_full_range_flag`.
    pub full_range: bool,
}

/// Bits of `slice_pic_order_cnt_lsb`: counts wrap at 65536 pictures.
pub const LOG2_MAX_POC_LSB: u32 = 16;

const NAL_VPS: u8 = 32;
const NAL_SPS: u8 = 33;
const NAL_PPS: u8 = 34;
pub const NAL_TRAIL_R: u8 = 1;
pub const NAL_IDR_W_RADL: u8 = 19;
const NAL_PREFIX_SEI: u8 = 39;

pub const SLICE_P: u32 = 1;
pub const SLICE_I: u32 = 2;

fn profile_tier_level(b: &mut Bits, s: &Stream) {
    let profile: u32 = if s.depth > 8 { 2 } else { 1 };
    b.put(2, 0); // general_profile_space
    b.flag(false); // general_tier_flag: main tier
    b.put(5, profile);
    // A Main stream is also a valid Main 10 stream.
    for j in 0..32 {
        b.flag(j == profile || (profile == 1 && j == 2));
    }
    b.flag(true); // general_progressive_source_flag
    b.flag(false); // general_interlaced_source_flag
    b.flag(true); // general_non_packed_constraint_flag
    b.flag(true); // general_frame_only_constraint_flag
    b.put(32, 0); // general_reserved_zero_43bits
    b.put(11, 0);
    b.flag(false); // general_inbld_flag
    b.put(8, u32::from(s.level_idc));
}

fn vps(s: &Stream) -> Vec<u8> {
    let mut b = Bits::default();
    b.put(4, 0); // vps_video_parameter_set_id
    b.flag(true); // vps_base_layer_internal_flag
    b.flag(true); // vps_base_layer_available_flag
    b.put(6, 0); // vps_max_layers_minus1
    b.put(3, 0); // vps_max_sub_layers_minus1
    b.flag(true); // vps_temporal_id_nesting_flag
    b.put(16, 0xffff);
    profile_tier_level(&mut b, s);
    b.flag(true); // vps_sub_layer_ordering_info_present_flag
    b.ue(s.references); // vps_max_dec_pic_buffering_minus1
    b.ue(0); // vps_max_num_reorder_pics
    b.ue(0); // vps_max_latency_increase_plus1
    b.put(6, 0); // vps_max_layer_id
    b.ue(0); // vps_num_layer_sets_minus1
    b.flag(true); // vps_timing_info_present_flag
    b.put(32, 1); // vps_num_units_in_tick
    b.put(32, s.fps); // vps_time_scale
    b.flag(false); // vps_poc_proportional_to_timing_flag
    b.ue(0); // vps_num_hrd_parameters
    b.flag(false); // vps_extension_flag
    b.align();
    b.bytes
}

fn sps(s: &Stream) -> Vec<u8> {
    let mut b = Bits::default();
    b.put(4, 0); // sps_video_parameter_set_id
    b.put(3, 0); // sps_max_sub_layers_minus1
    b.flag(true); // sps_temporal_id_nesting_flag
    profile_tier_level(&mut b, s);
    b.ue(0); // sps_seq_parameter_set_id
    b.ue(1); // chroma_format_idc: 4:2:0
    b.ue(s.width);
    b.ue(s.height);
    // conformance_window_flag: the offsets count chroma samples, two luma samples each.
    b.flag(s.crop != (0, 0));
    if s.crop != (0, 0) {
        b.ue(0); // conf_win_left_offset
        b.ue(s.crop.0 / 2); // conf_win_right_offset
        b.ue(0); // conf_win_top_offset
        b.ue(s.crop.1 / 2); // conf_win_bottom_offset
    }
    b.ue(u32::from(s.depth - 8)); // bit_depth_luma_minus8
    b.ue(u32::from(s.depth - 8)); // bit_depth_chroma_minus8
    b.ue(LOG2_MAX_POC_LSB - 4);
    b.flag(true); // sps_sub_layer_ordering_info_present_flag
    b.ue(s.references); // sps_max_dec_pic_buffering_minus1
    b.ue(0); // sps_max_num_reorder_pics
    b.ue(0); // sps_max_latency_increase_plus1
    b.ue(0); // log2_min_luma_coding_block_size_minus3: 8
    b.ue(3); // log2_diff_max_min_luma_coding_block_size: 64
    b.ue(0); // log2_min_luma_transform_block_size_minus2: 4
    b.ue(3); // log2_diff_max_min_luma_transform_block_size: 32
    b.ue(2); // max_transform_hierarchy_depth_inter
    b.ue(2); // max_transform_hierarchy_depth_intra
    b.flag(false); // scaling_list_enabled_flag
    b.flag(true); // amp_enabled_flag
    b.flag(true); // sample_adaptive_offset_enabled_flag
    b.flag(false); // pcm_enabled_flag
    b.ue(0); // num_short_term_ref_pic_sets: each slice carries its own
    b.flag(false); // long_term_ref_pics_present_flag
    b.flag(false); // sps_temporal_mvp_enabled_flag
    b.flag(true); // strong_intra_smoothing_enabled_flag
    b.flag(true); // vui_parameters_present_flag
    {
        b.flag(false); // aspect_ratio_info_present_flag
        b.flag(false); // overscan_info_present_flag
        b.flag(true); // video_signal_type_present_flag
        b.put(3, 5); // video_format: unspecified
        b.flag(s.full_range); // video_full_range_flag
        b.flag(true); // colour_description_present_flag
        b.put(8, u32::from(s.color.0)); // colour_primaries
        b.put(8, u32::from(s.color.1)); // transfer_characteristics
        b.put(8, u32::from(s.color.2)); // matrix_coeffs
        b.flag(false); // chroma_loc_info_present_flag
        b.flag(false); // neutral_chroma_indication_flag
        b.flag(false); // field_seq_flag
        b.flag(false); // frame_field_info_present_flag
        b.flag(false); // default_display_window_flag
        b.flag(true); // vui_timing_info_present_flag
        b.put(32, 1); // vui_num_units_in_tick
        b.put(32, s.fps); // vui_time_scale
        b.flag(false); // vui_poc_proportional_to_timing_flag
        b.flag(false); // vui_hrd_parameters_present_flag
        b.flag(false); // bitstream_restriction_flag
    }
    b.flag(false); // sps_extension_present_flag
    b.align();
    b.bytes
}

fn pps() -> Vec<u8> {
    let mut b = Bits::default();
    b.ue(0); // pps_pic_parameter_set_id
    b.ue(0); // pps_seq_parameter_set_id
    b.flag(false); // dependent_slice_segments_enabled_flag
    b.flag(false); // output_flag_present_flag
    b.put(3, 0); // num_extra_slice_header_bits
    b.flag(false); // sign_data_hiding_enabled_flag
    b.flag(false); // cabac_init_present_flag
    b.ue(0); // num_ref_idx_l0_default_active_minus1
    b.ue(0); // num_ref_idx_l1_default_active_minus1
    b.se(0); // init_qp_minus26
    b.flag(false); // constrained_intra_pred_flag
    b.flag(false); // transform_skip_enabled_flag
    b.flag(true); // cu_qp_delta_enabled_flag
    b.ue(0); // diff_cu_qp_delta_depth
    b.se(0); // pps_cb_qp_offset
    b.se(0); // pps_cr_qp_offset
    b.flag(false); // pps_slice_chroma_qp_offsets_present_flag
    b.flag(false); // weighted_pred_flag
    b.flag(false); // weighted_bipred_flag
    b.flag(false); // transquant_bypass_enabled_flag
    b.flag(false); // tiles_enabled_flag
    b.flag(false); // entropy_coding_sync_enabled_flag
    b.flag(true); // pps_loop_filter_across_slices_enabled_flag
    b.flag(false); // deblocking_filter_control_present_flag
    b.flag(false); // pps_scaling_list_data_present_flag
    b.flag(false); // lists_modification_present_flag
    b.ue(0); // log2_parallel_merge_level_minus2
    b.flag(false); // slice_segment_header_extension_present_flag
    b.flag(false); // pps_extension_present_flag
    b.align();
    b.bytes
}

/// The three parameter sets as one byte stream, sent with each IDR picture.
pub fn parameter_sets(s: &Stream) -> Vec<u8> {
    let mut out = Vec::with_capacity(128);
    nal(&[NAL_VPS << 1, 1], &vps(s), &mut out);
    nal(&[NAL_SPS << 1, 1], &sps(s), &mut out);
    nal(&[NAL_PPS << 1, 1], &pps(), &mut out);
    out
}

/// The header of a picture's only slice. `order` counts pictures from the last IDR picture.
/// `held` are the orders of the pictures kept as references, oldest first: the picture refers
/// to the newest and only keeps the others for later pictures.
pub fn slice_header(keyframe: bool, order: u32, held: &[u32]) -> Vec<u8> {
    let mut b = Bits::default();
    b.flag(true); // first_slice_segment_in_pic_flag
    if keyframe {
        b.flag(false); // no_output_of_prior_pics_flag
    }
    b.ue(0); // slice_pic_parameter_set_id
    b.ue(if keyframe { SLICE_I } else { SLICE_P });
    if !keyframe {
        b.put(LOG2_MAX_POC_LSB, order & ((1 << LOG2_MAX_POC_LSB) - 1));
        b.flag(false); // short_term_ref_pic_set_sps_flag
        // st_ref_pic_set(0): the pictures held, newest first, the newest used by this one.
        b.ue(held.len() as u32); // num_negative_pics
        b.ue(0); // num_positive_pics
        let mut before = order;
        for (index, &poc) in held.iter().rev().enumerate() {
            b.ue(before - poc - 1); // delta_poc_s0_minus1
            b.flag(index == 0); // used_by_curr_pic_s0_flag
            before = poc;
        }
    }
    b.flag(true); // slice_sao_luma_flag
    b.flag(true); // slice_sao_chroma_flag
    if !keyframe {
        b.flag(false); // num_ref_idx_active_override_flag
        b.ue(0); // five_minus_max_num_merge_cand
    }
    b.se(0); // slice_qp_delta
    b.flag(true); // slice_loop_filter_across_slices_enabled_flag
    b.align();
    let mut out = Vec::with_capacity(16);
    let kind = if keyframe {
        NAL_IDR_W_RADL
    } else {
        NAL_TRAIL_R
    };
    nal(&[kind << 1, 1], &b.bytes, &mut out);
    out
}

/// The HDR10 metadata of a stream as one prefix SEI NAL unit: the mastering display colour
/// volume (H.265 D.2.28) and the content light level (D.2.35). `primaries` are red, green and
/// blue in 0.00002 steps; the SEI lists green, blue and red. `maximum` and `minimum` are the
/// mastering display's luminance in 0.0001 nit.
pub fn hdr_sei(
    primaries: [[u16; 2]; 3],
    white: [u16; 2],
    maximum: u32,
    minimum: u32,
    max_cll: u16,
    max_fall: u16,
) -> Vec<u8> {
    let mut b = Bits::default();
    b.put(8, 137); // payloadType: mastering_display_colour_volume
    b.put(8, 24); // payloadSize
    for [x, y] in [primaries[1], primaries[2], primaries[0]] {
        b.put(16, u32::from(x)); // display_primaries_x
        b.put(16, u32::from(y)); // display_primaries_y
    }
    b.put(16, u32::from(white[0])); // white_point_x
    b.put(16, u32::from(white[1])); // white_point_y
    b.put(32, maximum); // max_display_mastering_luminance
    b.put(32, minimum); // min_display_mastering_luminance
    b.put(8, 144); // payloadType: content_light_level_info
    b.put(8, 4); // payloadSize
    b.put(16, u32::from(max_cll)); // max_content_light_level
    b.put(16, u32::from(max_fall)); // max_pic_average_light_level
    b.align(); // rbsp_trailing_bits
    let mut out = Vec::with_capacity(48);
    nal(&[NAL_PREFIX_SEI << 1, 1], &b.bytes, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hdr_sei_is_one_prefix_unit_with_both_messages() {
        let sei = hdr_sei(
            [[34000, 16000], [13250, 34500], [7500, 3000]],
            [15635, 16450],
            10_000_000,
            1,
            1000,
            400,
        );
        assert_eq!(&sei[..6], &[0, 0, 0, 1, NAL_PREFIX_SEI << 1, 1]);
        // 137 and its 24 bytes; 144 and its 4; the trailing byte.
        // The minimum luminance's zero bytes take one emulation prevention byte.
        assert_eq!(sei.len(), 6 + 2 + 24 + 2 + 4 + 1 + 1);
        assert_eq!(sei[6], 137);
        assert_eq!(sei[6 + 2 + 24 + 1], 144);
        // Green first: 13250.
        assert_eq!(&sei[8..10], &13250u16.to_be_bytes());
    }

    #[test]
    fn parameter_sets_come_in_order() {
        let s = Stream {
            width: 2560,
            height: 1440,
            fps: 120,
            depth: 8,
            level_idc: 153,
            crop: (0, 0),
            references: 4,
            color: (1, 1, 1),
            full_range: false,
        };
        let bytes = parameter_sets(&s);
        let kinds: Vec<u8> = bytes
            .windows(5)
            .filter(|w| w[..4] == [0, 0, 0, 1])
            .map(|w| w[4] >> 1)
            .collect();
        assert_eq!(kinds, [NAL_VPS, NAL_SPS, NAL_PPS]);
        // No start code may appear inside a unit's payload.
        let starts = bytes.windows(3).filter(|w| *w == [0, 0, 1]).count();
        assert_eq!(starts, 3);
    }

    #[test]
    fn a_cropped_picture_says_so() {
        let plain = Stream {
            width: 1920,
            height: 1088,
            fps: 60,
            depth: 8,
            level_idc: 123,
            crop: (0, 0),
            references: 4,
            color: (1, 1, 1),
            full_range: false,
        };
        let cropped = Stream {
            crop: (0, 8),
            ..plain
        };
        assert_ne!(sps(&plain), sps(&cropped));
        // The colour space the client asked for is the one the stream declares.
        let full = Stream {
            full_range: true,
            ..plain
        };
        assert_ne!(sps(&plain), sps(&full));
    }

    #[test]
    fn slice_headers_name_their_picture_type() {
        let idr = slice_header(true, 0, &[]);
        assert_eq!(idr[4] >> 1, NAL_IDR_W_RADL);
        let p = slice_header(false, 5, &[4]);
        assert_eq!(p[4] >> 1, NAL_TRAIL_R);
        // first_slice 1, pps id ue(0)=1, slice_type P ue(1)=010, then 16 bits of count.
        assert_eq!(p[6] >> 3, 0b11010);
        assert_ne!(slice_header(false, 5, &[4]), slice_header(false, 6, &[5]));
        // Holding more pictures, or only older ones, changes the set.
        assert_ne!(
            slice_header(false, 5, &[4]),
            slice_header(false, 5, &[3, 4])
        );
        assert_ne!(slice_header(false, 5, &[4]), slice_header(false, 5, &[2]));
    }
}
