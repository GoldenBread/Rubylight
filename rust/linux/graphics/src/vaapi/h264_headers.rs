//! The H.264 headers an application hands a VA-API driver: sequence and picture parameter sets
//! and each slice's header, written bit by bit as ITU-T H.264 section 7.3 lays them out. The
//! counterpart of [`super::hevc_headers`] for High profile, 8-bit 4:2:0 streams.
//!
//! One slice per picture, no reordering: an IDR picture, then P pictures that each refer to the
//! one before. Picture order follows decoding order (`pic_order_cnt_type` 2), so a slice header
//! carries only the frame number.
use super::bits::{Bits, nal};

/// What the parameter sets describe.
#[derive(Clone, Copy, Debug)]
pub struct Stream {
    /// The coded size, whole 16-pixel blocks.
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub level_idc: u8,
    /// Pixels cut from the right and bottom of the coded picture (even).
    pub crop: (u32, u32),
    /// Pictures held as references: `max_num_ref_frames`.
    pub references: u32,
    /// `colour_primaries`, `transfer_characteristics` and `matrix_coefficients` of the VUI.
    pub color: (u8, u8, u8),
    /// `video_full_range_flag`.
    pub full_range: bool,
}

/// Bits of `frame_num`: counts wrap at 65536 pictures.
pub const LOG2_MAX_FRAME_NUM: u32 = 16;

/// Whether the stream uses CABAC entropy coding (otherwise CAVLC).
pub const CABAC: bool = true;

const NAL_SLICE: u8 = 1;
const NAL_IDR: u8 = 5;
const NAL_SPS: u8 = 7;
const NAL_PPS: u8 = 8;

pub const SLICE_P: u32 = 0;
pub const SLICE_I: u32 = 2;

fn sps(s: &Stream) -> Vec<u8> {
    let mut b = Bits::default();
    b.put(8, 100); // profile_idc: High
    b.put(8, 0); // constraint_set flags and reserved bits
    b.put(8, u32::from(s.level_idc));
    b.ue(0); // seq_parameter_set_id
    b.ue(1); // chroma_format_idc: 4:2:0
    b.ue(0); // bit_depth_luma_minus8
    b.ue(0); // bit_depth_chroma_minus8
    b.flag(false); // qpprime_y_zero_transform_bypass_flag
    b.flag(false); // seq_scaling_matrix_present_flag
    b.ue(LOG2_MAX_FRAME_NUM - 4);
    b.ue(2); // pic_order_cnt_type: output order is decoding order
    b.ue(s.references); // max_num_ref_frames
    // A decoder that never got the pictures a loss report named counts them as missing, not as
    // a broken stream.
    b.flag(true); // gaps_in_frame_num_value_allowed_flag
    b.ue(s.width / 16 - 1); // pic_width_in_mbs_minus1
    b.ue(s.height / 16 - 1); // pic_height_in_map_units_minus1
    b.flag(true); // frame_mbs_only_flag
    b.flag(true); // direct_8x8_inference_flag
    // frame_cropping_flag: the offsets count chroma samples, two luma samples each.
    b.flag(s.crop != (0, 0));
    if s.crop != (0, 0) {
        b.ue(0); // frame_crop_left_offset
        b.ue(s.crop.0 / 2); // frame_crop_right_offset
        b.ue(0); // frame_crop_top_offset
        b.ue(s.crop.1 / 2); // frame_crop_bottom_offset
    }
    b.flag(true); // vui_parameters_present_flag
    b.flag(false); // aspect_ratio_info_present_flag
    b.flag(false); // overscan_info_present_flag
    b.flag(true); // video_signal_type_present_flag
    b.put(3, 5); // video_format: unspecified
    b.flag(s.full_range); // video_full_range_flag
    b.flag(true); // colour_description_present_flag
    b.put(8, u32::from(s.color.0)); // colour_primaries
    b.put(8, u32::from(s.color.1)); // transfer_characteristics
    b.put(8, u32::from(s.color.2)); // matrix_coefficients
    b.flag(false); // chroma_loc_info_present_flag
    b.flag(true); // timing_info_present_flag
    b.put(32, 1); // num_units_in_tick
    b.put(32, s.fps * 2); // time_scale: ticks are fields
    b.flag(true); // fixed_frame_rate_flag
    b.flag(false); // nal_hrd_parameters_present_flag
    b.flag(false); // vcl_hrd_parameters_present_flag
    b.flag(false); // pic_struct_present_flag
    b.flag(true); // bitstream_restriction_flag: tells decoders not to hold pictures back
    b.flag(true); // motion_vectors_over_pic_boundaries_flag
    b.ue(0); // max_bytes_per_pic_denom
    b.ue(0); // max_bits_per_mb_denom
    b.ue(15); // log2_max_mv_length_horizontal
    b.ue(15); // log2_max_mv_length_vertical
    b.ue(0); // max_num_reorder_frames
    b.ue(s.references); // max_dec_frame_buffering
    b.align();
    b.bytes
}

fn pps() -> Vec<u8> {
    let mut b = Bits::default();
    b.ue(0); // pic_parameter_set_id
    b.ue(0); // seq_parameter_set_id
    b.flag(CABAC); // entropy_coding_mode_flag
    b.flag(false); // bottom_field_pic_order_in_frame_present_flag
    b.ue(0); // num_slice_groups_minus1
    b.ue(0); // num_ref_idx_l0_default_active_minus1
    b.ue(0); // num_ref_idx_l1_default_active_minus1
    b.flag(false); // weighted_pred_flag
    b.put(2, 0); // weighted_bipred_idc
    b.se(0); // pic_init_qp_minus26
    b.se(0); // pic_init_qs_minus26
    b.se(0); // chroma_qp_index_offset
    b.flag(true); // deblocking_filter_control_present_flag
    b.flag(false); // constrained_intra_pred_flag
    b.flag(false); // redundant_pic_cnt_present_flag
    b.flag(true); // transform_8x8_mode_flag
    b.flag(false); // pic_scaling_matrix_present_flag
    b.se(0); // second_chroma_qp_index_offset
    b.align();
    b.bytes
}

/// The two parameter sets as one byte stream, sent with each IDR picture.
pub fn parameter_sets(s: &Stream) -> Vec<u8> {
    let mut out = Vec::with_capacity(96);
    nal(&[3 << 5 | NAL_SPS], &sps(s), &mut out);
    nal(&[3 << 5 | NAL_PPS], &pps(), &mut out);
    out
}

/// The sequence parameter set alone, as a NAL unit.
pub fn sequence_set(s: &Stream) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    nal(&[3 << 5 | NAL_SPS], &sps(s), &mut out);
    out
}

/// The picture parameter set alone, as a NAL unit.
pub fn picture_set() -> Vec<u8> {
    let mut out = Vec::with_capacity(32);
    nal(&[3 << 5 | NAL_PPS], &pps(), &mut out);
    out
}

/// The header of a picture's only slice, as bits before the byte it is wrapped in:
/// the NAL unit's bytes are returned with the header's last byte unaligned when the entropy
/// coder is CABAC, so the picture data that follows starts on a byte boundary (the
/// `cabac_alignment_one_bit` bits are written here).
///
/// `frame_num` counts pictures from the last IDR picture; `idr_id` tells successive IDR
/// pictures apart. A P picture refers to the one `distance` pictures before it: 1 for the
/// previous picture, which is the first in the default order, more for an older one.
pub fn slice_header(keyframe: bool, frame_num: u32, idr_id: u32, distance: u32) -> Vec<u8> {
    let mut b = Bits::default();
    b.ue(0); // first_mb_in_slice
    b.ue(if keyframe { SLICE_I } else { SLICE_P });
    b.ue(0); // pic_parameter_set_id
    b.put(
        LOG2_MAX_FRAME_NUM,
        frame_num & ((1 << LOG2_MAX_FRAME_NUM) - 1),
    );
    if keyframe {
        b.ue(idr_id & 0xffff); // idr_pic_id
    } else {
        b.flag(false); // num_ref_idx_active_override_flag
        if distance > 1 {
            b.flag(true); // ref_pic_list_modification_flag_l0
            b.ue(0); // modification_of_pic_nums_idc: subtract from the current picture
            b.ue(distance - 1); // abs_diff_pic_num_minus1
            b.ue(3); // end of the modifications
        } else {
            b.flag(false); // ref_pic_list_modification_flag_l0
        }
    }
    // dec_ref_pic_marking(): every picture is a reference.
    if keyframe {
        b.flag(false); // no_output_of_prior_pics_flag
        b.flag(false); // long_term_reference_flag
    } else {
        b.flag(false); // adaptive_ref_pic_marking_mode_flag: slide the window
    }
    if CABAC && !keyframe {
        b.ue(0); // cabac_init_idc
    }
    b.se(0); // slice_qp_delta
    b.ue(0); // disable_deblocking_filter_idc: filter on
    b.put(0, 0);
    // The slice data starts on a byte boundary: CABAC pads with ones, CAVLC needs no padding
    // from the header.
    if CABAC {
        while b.used != 0 {
            b.put(1, 1);
        }
    }
    let mut out = Vec::with_capacity(16);
    let header = if keyframe {
        3 << 5 | NAL_IDR
    } else {
        2 << 5 | NAL_SLICE
    };
    nal(&[header], &b.bytes, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream() -> Stream {
        Stream {
            width: 1920,
            height: 1088,
            fps: 60,
            level_idc: 42,
            crop: (0, 8),
            references: 4,
            color: (1, 1, 1),
            full_range: false,
        }
    }

    /// The colour space the client asked for is the one the stream declares.
    #[test]
    fn the_sequence_set_declares_the_colour_space() {
        let bt601 = Stream {
            color: (6, 6, 6),
            ..stream()
        };
        let full = Stream {
            full_range: true,
            ..stream()
        };
        assert_ne!(sps(&bt601), sps(&stream()));
        assert_ne!(sps(&full), sps(&stream()));
    }

    #[test]
    fn parameter_sets_come_in_order() {
        let bytes = parameter_sets(&stream());
        let kinds: Vec<u8> = bytes
            .windows(5)
            .filter(|w| w[..4] == [0, 0, 0, 1])
            .map(|w| w[4] & 0x1f)
            .collect();
        assert_eq!(kinds, [NAL_SPS, NAL_PPS]);
        // High profile, then the level.
        assert_eq!(bytes[5], 100);
        assert_eq!(bytes[7], 42);
        assert_eq!(bytes.windows(3).filter(|w| *w == [0, 0, 1]).count(), 2);
    }

    #[test]
    fn cropping_changes_the_sequence_set() {
        let plain = Stream {
            crop: (0, 0),
            ..stream()
        };
        assert_ne!(sps(&plain), sps(&stream()));
    }

    #[test]
    fn slice_headers_name_their_picture_type() {
        let idr = slice_header(true, 0, 0, 1);
        assert_eq!(idr[4] & 0x1f, NAL_IDR);
        let p = slice_header(false, 5, 0, 1);
        assert_eq!(p[4] & 0x1f, NAL_SLICE);
        assert_ne!(slice_header(false, 5, 0, 1), slice_header(false, 6, 0, 1));
        assert_ne!(slice_header(true, 0, 0, 1), slice_header(true, 0, 1, 1));
    }

    #[test]
    fn an_older_reference_is_named_in_the_slice_header() {
        let plain = slice_header(false, 9, 0, 1);
        let older = slice_header(false, 9, 0, 3);
        assert!(older.len() >= plain.len());
        assert_ne!(plain, older);
    }

    #[test]
    fn a_slice_header_ends_on_a_byte_with_ones_for_cabac() {
        // The CABAC alignment bits are ones: the header's last byte ends in them.
        let p = slice_header(false, 1, 0, 1);
        assert!(!CABAC || p.last().is_some_and(|last| last & 1 == 1));
    }
}
