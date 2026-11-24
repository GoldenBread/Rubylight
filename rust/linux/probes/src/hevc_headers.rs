//! The HEVC headers an application hands a VA-API driver: video, sequence and
//! picture parameter sets and each slice's header, written bit by bit as
//! ITU-T H.265 section 7.3 lays them out. Mesa's Radeon driver reads these to
//! learn what the stream should say, then writes the headers of its output.
//!
//! One layer, one sub-layer, 4:2:0, no reordering: each picture refers to the
//! one before it and its reference set travels in its slice header.

/// What the parameter sets describe.
#[derive(Clone, Copy, Debug)]
pub struct Stream {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// 8 or 10.
    pub depth: u8,
    pub level_idc: u8,
}

/// Bits of `slice_pic_order_cnt_lsb`: counts wrap at 65536 pictures.
pub const LOG2_MAX_POC_LSB: u32 = 16;

const NAL_VPS: u8 = 32;
const NAL_SPS: u8 = 33;
const NAL_PPS: u8 = 34;
pub const NAL_TRAIL_R: u8 = 1;
pub const NAL_IDR_W_RADL: u8 = 19;

pub const SLICE_P: u32 = 1;
pub const SLICE_I: u32 = 2;

/// A most-significant-bit-first writer of raw header bits.
#[derive(Default)]
struct Bits {
    bytes: Vec<u8>,
    /// Bits used in the last byte, 0 when it is full or there is none.
    used: u32,
}

impl Bits {
    fn put(&mut self, count: u32, value: u32) {
        debug_assert!(count <= 32 && (count == 32 || value >> count == 0));
        for shift in (0..count).rev() {
            if self.used == 0 {
                self.bytes.push(0);
            }
            let bit = (value >> shift & 1) as u8;
            *self.bytes.last_mut().expect("a byte was just pushed") |= bit << (7 - self.used);
            self.used = (self.used + 1) % 8;
        }
    }

    fn flag(&mut self, value: bool) {
        self.put(1, u32::from(value));
    }

    /// Unsigned Exp-Golomb, `ue(v)`.
    fn ue(&mut self, value: u32) {
        let code = u64::from(value) + 1;
        let length = 64 - code.leading_zeros();
        self.put(length - 1, 0);
        // `code` has `length` bits, at most 33; write it in two parts.
        if length > 16 {
            self.put(length - 16, (code >> 16) as u32);
            self.put(16, (code & 0xffff) as u32);
        } else {
            self.put(length, code as u32);
        }
    }

    /// Signed Exp-Golomb, `se(v)`.
    fn se(&mut self, value: i32) {
        let mapped = if value > 0 {
            value as u32 * 2 - 1
        } else {
            value.unsigned_abs() * 2
        };
        self.ue(mapped);
    }

    /// `rbsp_trailing_bits()` and `byte_alignment()`: a one, then zeros to
    /// the end of the byte.
    fn align(&mut self) {
        self.put(1, 1);
        while self.used != 0 {
            self.put(1, 0);
        }
    }
}

/// Wraps header bits as a NAL unit in a byte stream: start code, the two
/// header bytes, and the payload with emulation prevention bytes inserted.
fn nal(kind: u8, payload: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(&[0, 0, 0, 1, kind << 1, 1]);
    let mut zeros = 0;
    for &byte in payload {
        if zeros >= 2 && byte <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
}

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
    b.ue(1); // vps_max_dec_pic_buffering_minus1
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
    b.flag(false); // conformance_window_flag
    b.ue(u32::from(s.depth - 8)); // bit_depth_luma_minus8
    b.ue(u32::from(s.depth - 8)); // bit_depth_chroma_minus8
    b.ue(LOG2_MAX_POC_LSB - 4);
    b.flag(true); // sps_sub_layer_ordering_info_present_flag
    b.ue(1); // sps_max_dec_pic_buffering_minus1
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
        b.flag(false); // video_full_range_flag
        b.flag(true); // colour_description_present_flag
        b.put(8, 1); // colour_primaries: BT.709
        b.put(8, 1); // transfer_characteristics: BT.709
        b.put(8, 1); // matrix_coeffs: BT.709
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
    nal(NAL_VPS, &vps(s), &mut out);
    nal(NAL_SPS, &sps(s), &mut out);
    nal(NAL_PPS, &pps(), &mut out);
    out
}

/// The header of a picture's only slice. `order` counts pictures from the
/// last IDR picture; a P picture refers to the picture before it.
pub fn slice_header(keyframe: bool, order: u32) -> Vec<u8> {
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
        // st_ref_pic_set(0): the picture before this one, used by it.
        b.ue(1); // num_negative_pics
        b.ue(0); // num_positive_pics
        b.ue(0); // delta_poc_s0_minus1
        b.flag(true); // used_by_curr_pic_s0_flag
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
    nal(
        if keyframe {
            NAL_IDR_W_RADL
        } else {
            NAL_TRAIL_R
        },
        &b.bytes,
        &mut out,
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(write: impl FnOnce(&mut Bits)) -> (Vec<u8>, u32) {
        let mut b = Bits::default();
        write(&mut b);
        (b.bytes, b.used)
    }

    #[test]
    fn exp_golomb_matches_the_standard_table() {
        // H.265 table 9-2: 0 -> 1, 1 -> 010, 2 -> 011, 3 -> 00100, 7 -> 0001000.
        assert_eq!(bits(|b| b.ue(0)), (vec![0b1000_0000], 1));
        assert_eq!(bits(|b| b.ue(1)), (vec![0b0100_0000], 3));
        assert_eq!(bits(|b| b.ue(2)), (vec![0b0110_0000], 3));
        assert_eq!(bits(|b| b.ue(3)), (vec![0b0010_0000], 5));
        assert_eq!(bits(|b| b.ue(7)), (vec![0b0001_0000], 7));
        // Table 9-3: 1 -> ue 1, -1 -> ue 2, 2 -> ue 3, -2 -> ue 4.
        assert_eq!(bits(|b| b.se(1)), bits(|b| b.ue(1)));
        assert_eq!(bits(|b| b.se(-1)), bits(|b| b.ue(2)));
        assert_eq!(bits(|b| b.se(2)), bits(|b| b.ue(3)));
        assert_eq!(bits(|b| b.se(-2)), bits(|b| b.ue(4)));
    }

    #[test]
    fn wide_exp_golomb_values_keep_every_bit() {
        // 3840 + 1 = 0b1111_0000_0001: 11 zeros, then those 12 bits.
        let (bytes, used) = bits(|b| b.ue(3840));
        assert_eq!(bytes, vec![0b0000_0000, 0b0001_1110, 0b0000_0010]);
        assert_eq!(used, 7);
        // The largest value: 32 zeros, a one, 32 zeros.
        let (bytes, used) = bits(|b| b.ue(u32::MAX));
        assert_eq!(bytes.len(), 9);
        assert_eq!(used, 1);
        assert_eq!(bytes[4], 0b1000_0000);
        assert!(bytes.iter().enumerate().all(|(i, &v)| i == 4 || v == 0));
    }

    #[test]
    fn alignment_ends_on_a_byte() {
        assert_eq!(bits(|b| b.align()), (vec![0b1000_0000], 0));
        assert_eq!(
            bits(|b| {
                b.put(7, 0);
                b.align();
            }),
            (vec![0b0000_0001], 0)
        );
    }

    #[test]
    fn emulation_prevention_guards_start_codes() {
        let mut out = Vec::new();
        nal(NAL_SPS, &[0, 0, 0, 0, 1, 0, 0, 3, 0, 0, 4], &mut out);
        assert_eq!(
            out,
            [
                0,
                0,
                0,
                1,
                NAL_SPS << 1,
                1,
                0,
                0,
                3,
                0,
                0,
                3,
                1,
                0,
                0,
                3,
                3,
                0,
                0,
                4
            ]
        );
    }

    #[test]
    fn parameter_sets_come_in_order() {
        let s = Stream {
            width: 2560,
            height: 1440,
            fps: 120,
            depth: 8,
            level_idc: 153,
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
    fn slice_headers_name_their_picture_type() {
        let idr = slice_header(true, 0);
        assert_eq!(idr[4] >> 1, NAL_IDR_W_RADL);
        let p = slice_header(false, 5);
        assert_eq!(p[4] >> 1, NAL_TRAIL_R);
        // first_slice 1, pps id ue(0)=1, slice_type P ue(1)=010, then 16 bits of count.
        assert_eq!(p[6] >> 3, 0b11010);
        assert_ne!(slice_header(false, 5), slice_header(false, 6));
    }
}
