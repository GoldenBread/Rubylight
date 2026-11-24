//! The AV1 headers an application hands a VA-API driver: the sequence header and each frame's
//! header, as OBUs with a size field, written bit by bit as the AV1 specification (sections 5.5
//! and 5.9) lays them out. Mesa's Radeon driver reads these to learn what the stream should say,
//! then writes the headers of its output itself, so what they say has to agree with the
//! picture parameters.
//!
//! One layer, 4:2:0, 64x64 superblocks, one tile (up to 4096 pixels wide), no reordering, order hints on. A key frame
//! refreshes every reference slot; an inter frame names the slot it refers to for all seven
//! references and refreshes one.
use super::bits::Bits;

/// What the sequence header describes.
#[derive(Clone, Copy, Debug)]
pub struct Stream {
    /// The picture's size, which the sequence header and every frame's render size name. The
    /// driver codes it in whole blocks and says in the frame header where the picture ends.
    pub width: u32,
    pub height: u32,
    /// 8 or 10.
    pub depth: u8,
    pub level_idx: u8,
    /// `color_primaries`, `transfer_characteristics` and `matrix_coefficients`.
    pub color: (u8, u8, u8),
    /// `color_range`.
    pub full_range: bool,
}

/// Bits of `order_hint`: counts wrap at 256 frames.
pub const ORDER_HINT_BITS: u32 = 8;

const OBU_SEQUENCE_HEADER: u8 = 1;
const OBU_TEMPORAL_DELIMITER: u8 = 2;
const OBU_FRAME_HEADER: u8 = 3;
const OBU_METADATA: u8 = 5;

pub const FRAME_KEY: u32 = 0;
pub const FRAME_INTER: u32 = 1;

/// The frame header's contents.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub key: bool,
    pub order_hint: u8,
    /// Slots the frame is stored in; a key frame stores into all of them.
    pub refresh: u8,
    /// The slot every reference of an inter frame is.
    pub slot: u8,
    pub base_qindex: u8,
}

/// Wraps an OBU payload: the header byte (type, has a size field), the size, the payload.
fn obu(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 6);
    out.push(kind << 3 | 0b10);
    let mut size = payload.len();
    loop {
        let byte = (size & 0x7f) as u8;
        size >>= 7;
        if size == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
    out.extend_from_slice(payload);
    out
}

/// The temporal delimiter that starts each temporal unit.
pub fn temporal_delimiter() -> Vec<u8> {
    obu(OBU_TEMPORAL_DELIMITER, &[])
}

/// The lowest level (as `seq_level_idx`) that fits a picture size and rate, in main tier.
pub fn level_idx(width: u32, height: u32, fps: u32) -> u8 {
    let size = u64::from(width) * u64::from(height);
    let rate = size * u64::from(fps);
    // (seq_level_idx, MaxPicSize, MaxHSize, MaxVSize, MaxDisplayRate) from table A.3.
    const LEVELS: [(u8, u64, u32, u32, u64); 14] = [
        (0, 147_456, 2_048, 1_152, 4_423_680),
        (1, 278_784, 2_816, 1_584, 8_363_520),
        (4, 665_856, 4_352, 2_448, 19_975_680),
        (5, 1_065_024, 5_504, 3_096, 31_950_720),
        (8, 2_359_296, 6_144, 3_456, 70_778_880),
        (9, 2_359_296, 6_144, 3_456, 141_557_760),
        (12, 8_912_896, 8_192, 4_352, 267_386_880),
        (13, 8_912_896, 8_192, 4_352, 534_773_760),
        (14, 8_912_896, 8_192, 4_352, 1_069_547_520),
        (15, 8_912_896, 8_192, 4_352, 1_069_547_520),
        (16, 35_651_584, 16_384, 8_704, 1_069_547_520),
        (17, 35_651_584, 16_384, 8_704, 2_139_095_040),
        (18, 35_651_584, 16_384, 8_704, 4_278_190_080),
        (19, 35_651_584, 16_384, 8_704, 4_278_190_080),
    ];
    LEVELS
        .iter()
        .find(|&&(_, max_size, max_h, max_v, max_rate)| {
            size <= max_size && width <= max_h && height <= max_v && rate <= max_rate
        })
        .map_or(19, |&(idx, ..)| idx)
}

/// The sequence header OBU.
pub fn sequence(s: &Stream) -> Vec<u8> {
    let mut b = Bits::default();
    b.put(3, 0); // seq_profile: main, 4:2:0 at 8 or 10 bits
    b.flag(false); // still_picture
    b.flag(false); // reduced_still_picture_header
    b.flag(false); // timing_info_present_flag
    b.flag(false); // initial_display_delay_present_flag
    b.put(5, 0); // operating_points_cnt_minus_1
    b.put(12, 0); // operating_point_idc[0]
    b.put(5, u32::from(s.level_idx)); // seq_level_idx[0]
    if s.level_idx > 7 {
        b.flag(false); // seq_tier[0]
    }
    b.put(4, 15); // frame_width_bits_minus_1
    b.put(4, 15); // frame_height_bits_minus_1
    b.put(16, s.width - 1); // max_frame_width_minus_1
    b.put(16, s.height - 1); // max_frame_height_minus_1
    b.flag(false); // frame_id_numbers_present_flag
    b.flag(false); // use_128x128_superblock
    b.flag(false); // enable_filter_intra
    b.flag(false); // enable_intra_edge_filter
    b.flag(false); // enable_interintra_compound
    b.flag(false); // enable_masked_compound
    b.flag(false); // enable_warped_motion
    b.flag(false); // enable_dual_filter
    b.flag(true); // enable_order_hint
    b.flag(false); // enable_jnt_comp
    b.flag(false); // enable_ref_frame_mvs
    b.flag(false); // seq_choose_screen_content_tools
    b.flag(false); // seq_force_screen_content_tools
    b.put(3, ORDER_HINT_BITS - 1); // order_hint_bits_minus_1
    b.flag(false); // enable_superres
    b.flag(true); // enable_cdef
    b.flag(false); // enable_restoration
    // color_config()
    b.flag(s.depth > 8); // high_bitdepth
    b.flag(false); // mono_chrome
    b.flag(true); // color_description_present_flag
    b.put(8, u32::from(s.color.0)); // color_primaries
    b.put(8, u32::from(s.color.1)); // transfer_characteristics
    b.put(8, u32::from(s.color.2)); // matrix_coefficients
    b.flag(s.full_range); // color_range
    b.put(2, 0); // chroma_sample_position: unknown
    b.flag(false); // separate_uv_delta_q
    b.flag(false); // film_grain_params_present
    b.align();
    obu(OBU_SEQUENCE_HEADER, &b.bytes)
}

/// `tile_log2(1, count)`: the smallest `k` with `1 << k >= count`.
fn tile_log2(count: u32) -> u32 {
    32 - count.saturating_sub(1).leading_zeros()
}

/// The frame header OBU of a picture of the stream `s`. The sizes, level of detail of the
/// loop filter and CDEF are the encoder's to choose; the driver replaces these values.
pub fn frame_header(s: &Stream, f: &Frame) -> Vec<u8> {
    let mut b = Bits::default();
    b.flag(false); // show_existing_frame
    b.put(2, if f.key { FRAME_KEY } else { FRAME_INTER }); // frame_type
    b.flag(true); // show_frame
    if !f.key {
        b.flag(false); // error_resilient_mode
    }
    b.flag(false); // disable_cdf_update
    b.flag(false); // frame_size_override_flag
    b.put(ORDER_HINT_BITS, u32::from(f.order_hint));
    if !f.key {
        b.put(3, 0); // primary_ref_frame: the slot of the first reference
        b.put(8, u32::from(f.refresh)); // refresh_frame_flags
    }
    if !f.key {
        b.flag(false); // frame_refs_short_signaling
        for _ in 0..7 {
            b.put(3, u32::from(f.slot)); // ref_frame_idx[i]
        }
    }
    b.flag(true); // render_and_frame_size_different
    b.put(16, s.width - 1); // render_width_minus_1
    b.put(16, s.height - 1); // render_height_minus_1
    if !f.key {
        b.flag(false); // allow_high_precision_mv
        b.flag(false); // is_filter_switchable
        b.put(2, 0); // interpolation_filter: EIGHTTAP
        b.flag(false); // is_motion_mode_switchable
    }
    b.flag(false); // disable_frame_end_update_cdf
    // tile_info(): one tile, so each log2 count stops where it starts.
    let superblocks = (s.width.div_ceil(8) * 2).div_ceil(16);
    let rows = (s.height.div_ceil(8) * 2).div_ceil(16);
    b.flag(true); // uniform_tile_spacing_flag
    if tile_log2(superblocks.min(64)) > 0 {
        b.flag(false); // increment_tile_cols_log2
    }
    if tile_log2(rows.min(64)) > 0 {
        b.flag(false); // increment_tile_rows_log2
    }
    // quantization_params()
    b.put(8, u32::from(f.base_qindex)); // base_q_idx
    b.flag(false); // DeltaQYDc delta_coded
    b.flag(false); // DeltaQUDc delta_coded
    b.flag(false); // DeltaQUAc delta_coded
    b.flag(false); // using_qmatrix
    b.flag(false); // segmentation_enabled
    if f.base_qindex > 0 {
        b.flag(false); // delta_q_present
    }
    // loop_filter_params()
    b.put(6, 0); // loop_filter_level[0]
    b.put(6, 0); // loop_filter_level[1]
    b.put(3, 0); // loop_filter_sharpness
    b.flag(false); // loop_filter_delta_enabled
    // cdef_params()
    b.put(2, 0); // cdef_damping_minus_3
    b.put(2, 0); // cdef_bits
    b.put(4, 0); // cdef_y_pri_strength[0]
    b.put(2, 0); // cdef_y_sec_strength[0]
    b.put(4, 0); // cdef_uv_pri_strength[0]
    b.put(2, 0); // cdef_uv_sec_strength[0]
    b.flag(true); // tx_mode_select
    if !f.key {
        b.flag(false); // reference_select
    }
    b.flag(false); // reduced_tx_set
    if !f.key {
        for _ in 0..7 {
            b.flag(false); // is_global
        }
    }
    b.align();
    obu(OBU_FRAME_HEADER, &b.bytes)
}

/// The mastering display and content light level of an HDR10 picture as AV1 metadata OBUs
/// (`METADATA_TYPE_HDR_CLL` and `METADATA_TYPE_HDR_MDCV`), for the key frames to carry. The
/// chromaticities are 0.16 fixed point, `max_luminance` 24.8 and `min_luminance` 18.14.
pub fn hdr_metadata(
    primaries: [(u16, u16); 3],
    white: (u16, u16),
    max_luminance: u32,
    min_luminance: u32,
    max_cll: u16,
    max_fall: u16,
) -> [Vec<u8>; 2] {
    let mut cll = Bits::default();
    cll.put(8, 1); // metadata_type: HDR_CLL
    cll.put(16, u32::from(max_cll));
    cll.put(16, u32::from(max_fall));
    cll.align();
    let mut mdcv = Bits::default();
    mdcv.put(8, 2); // metadata_type: HDR_MDCV
    for (x, y) in primaries {
        mdcv.put(16, u32::from(x));
        mdcv.put(16, u32::from(y));
    }
    mdcv.put(16, u32::from(white.0));
    mdcv.put(16, u32::from(white.1));
    mdcv.put(32, max_luminance);
    mdcv.put(32, min_luminance);
    mdcv.align();
    [
        obu(OBU_METADATA, &cll.bytes),
        obu(OBU_METADATA, &mdcv.bytes),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream() -> Stream {
        Stream {
            width: 1920,
            height: 1080,
            depth: 8,
            level_idx: level_idx(1920, 1080, 60),
            color: (1, 1, 1),
            full_range: false,
        }
    }

    /// The OBU's type and payload, with its size checked.
    fn parse(bytes: &[u8]) -> (u8, &[u8]) {
        let kind = bytes[0] >> 3 & 0xf;
        assert_eq!(bytes[0] & 0b10, 0b10, "has a size field");
        let (mut size, mut shift, mut at) = (0usize, 0, 1);
        loop {
            let byte = bytes[at];
            at += 1;
            size |= usize::from(byte & 0x7f) << shift;
            shift += 7;
            if byte & 0x80 == 0 {
                break;
            }
        }
        assert_eq!(bytes.len(), at + size);
        (kind, &bytes[at..])
    }

    #[test]
    fn the_temporal_delimiter_is_two_bytes() {
        assert_eq!(temporal_delimiter(), [0x12, 0x00]);
    }

    #[test]
    fn levels_follow_size_and_rate() {
        assert_eq!(level_idx(1280, 720, 60), 8);
        assert_eq!(level_idx(1920, 1088, 30), 8);
        assert_eq!(level_idx(1920, 1088, 60), 9);
        assert_eq!(level_idx(1920, 1088, 120), 12);
        assert_eq!(level_idx(3840, 2160, 60), 13);
    }

    #[test]
    fn the_sequence_header_names_its_size_and_ends_in_trailing_bits() {
        let bytes = sequence(&stream());
        let (kind, payload) = parse(&bytes);
        assert_eq!(kind, OBU_SEQUENCE_HEADER);
        // seq_profile 0, still_picture 0, reduced 0, timing 0, display delay 0, count 0.
        assert_eq!(payload[0] >> 1 & 0x7f, 0);
        assert_ne!(*payload.last().unwrap(), 0, "trailing bits end in a one");
        assert_ne!(sequence(&stream()), {
            let mut ten = stream();
            ten.depth = 10;
            sequence(&ten)
        });
    }

    #[test]
    fn key_and_inter_frame_headers_differ_and_carry_the_size() {
        let key = Frame {
            key: true,
            order_hint: 0,
            refresh: 0xff,
            slot: 0,
            base_qindex: 128,
        };
        let inter = Frame {
            key: false,
            order_hint: 1,
            refresh: 2,
            slot: 0,
            base_qindex: 128,
        };
        let key_obu = frame_header(&stream(), &key);
        let (kind, key_bits) = parse(&key_obu);
        assert_eq!(kind, OBU_FRAME_HEADER);
        // show_existing_frame 0, frame_type 00, show_frame 1.
        assert_eq!(key_bits[0] >> 4, 0b0001);
        let inter_obu = frame_header(&stream(), &inter);
        let (_, inter_bits) = parse(&inter_obu);
        // frame_type 01, show_frame 1, error_resilient_mode 0.
        assert_eq!(inter_bits[0] >> 3, 0b00110);
        assert_ne!(key_bits, inter_bits);
    }

    #[test]
    fn hdr_metadata_is_two_obus() {
        let [first, rest] = hdr_metadata(
            [(34000, 16000), (13250, 34500), (7500, 3000)],
            (15635, 16450),
            10_000_000,
            1,
            1000,
            400,
        );
        assert_eq!(parse(&first).0, OBU_METADATA);
        assert_eq!(parse(&rest).0, OBU_METADATA);
        assert_eq!(first[2], 1);
        assert_eq!(rest[2], 2);
    }
}
