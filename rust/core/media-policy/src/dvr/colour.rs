//! Colour signalling a bitstream carries: the H.264 and HEVC VUI `video_signal_type`
//! (reached in HEVC by walking the rest of the SPS, short-term reference picture sets
//! included) and the AV1 `color_config` the sequence header parser fills in. The muxer
//! writes it as the `colr` nclx box, falling back per code point only where the stream
//! leaves it unspecified.

use super::nal::Bits;

/// SMPTE ST 2084 (PQ), the transfer that takes `mdcv` and `clli` static metadata.
pub const TRANSFER_PQ: u8 = 16;
/// ARIB STD-B67 (HLG).
pub const TRANSFER_HLG: u8 = 18;

/// ISO/IEC 23091-2 colour code points and the sample range of a stream.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Colour {
    /// `(colour_primaries, transfer_characteristics, matrix_coefficients)` when the stream
    /// describes them.
    pub description: Option<(u8, u8, u8)>,
    /// Full-range (PC) samples; limited (TV) range otherwise.
    pub full_range: bool,
}

impl Colour {
    /// The nclx code points to write: the stream's own, with each unspecified (2) or
    /// reserved one replaced by the fallback, BT.2020 + PQ for an HDR stream and BT.709
    /// otherwise. Matrix 0 (identity, GBR) is a real value and kept.
    pub fn resolve(&self, hdr: bool) -> (u8, u8, u8) {
        let fallback = if hdr { (9, TRANSFER_PQ, 9) } else { (1, 1, 1) };
        let (primaries, transfer, matrix) = self.description.unwrap_or((2, 2, 2));
        (
            if matches!(primaries, 0 | 2 | 3) {
                fallback.0
            } else {
                primaries
            },
            if matches!(transfer, 0 | 2 | 3) {
                fallback.1
            } else {
                transfer
            },
            if matches!(matrix, 2 | 3) {
                fallback.2
            } else {
                matrix
            },
        )
    }
}

/// `vui_parameters_present_flag` and the VUI up to its `video_signal_type`, the prefix
/// H.264 (E.1.1) and HEVC (E.2.1) share. No VUI or no signal type is the default colour.
pub(crate) fn vui(bits: &mut Bits<'_>) -> Option<Colour> {
    if !bits.flag()? {
        return Some(Colour::default());
    }
    // aspect_ratio_info_present_flag; aspect_ratio_idc 255 is Extended_SAR.
    if bits.flag()? && bits.u8(8)? == 255 {
        bits.skip(32)?;
    }
    // overscan_info_present_flag, overscan_appropriate_flag.
    if bits.flag()? {
        bits.skip(1)?;
    }
    if !bits.flag()? {
        return Some(Colour::default());
    }
    bits.skip(3)?;
    let full_range = bits.flag()?;
    let description = if bits.flag()? {
        Some((bits.u8(8)?, bits.u8(8)?, bits.u8(8)?))
    } else {
        None
    };
    Some(Colour {
        description,
        full_range,
    })
}

/// The HEVC SPS from `log2_max_pic_order_cnt_lsb_minus4` through the VUI colour
/// (H.265 7.3.2.2.1).
pub(crate) fn hevc(bits: &mut Bits<'_>, max_sub_layers_minus1: u8) -> Option<Colour> {
    let poc_lsb_bits = bits.ue()?.checked_add(4)?;
    if poc_lsb_bits > 16 {
        return None;
    }
    // sps_sub_layer_ordering_info_present_flag: three ue(v) per signalled sub-layer.
    let layers = if bits.flag()? {
        u32::from(max_sub_layers_minus1) + 1
    } else {
        1
    };
    for _ in 0..layers * 3 {
        bits.ue()?;
    }
    // Coding and transform block sizes, transform hierarchy depths.
    for _ in 0..6 {
        bits.ue()?;
    }
    // scaling_list_enabled_flag, sps_scaling_list_data_present_flag.
    if bits.flag()? && bits.flag()? {
        scaling_list_data(bits)?;
    }
    // amp_enabled_flag, sample_adaptive_offset_enabled_flag.
    bits.skip(2)?;
    if bits.flag()? {
        bits.skip(8)?;
        bits.ue()?;
        bits.ue()?;
        bits.skip(1)?;
    }
    let sets = bits.ue()?;
    if sets > 64 {
        return None;
    }
    let mut previous: Option<Rps> = None;
    for _ in 0..sets {
        previous = Some(st_ref_pic_set(bits, previous.as_ref())?);
    }
    // long_term_ref_pics_present_flag: lt_ref_pic_poc_lsb_sps u(v) and a used flag each.
    if bits.flag()? {
        let count = bits.ue()?;
        if count > 32 {
            return None;
        }
        for _ in 0..count {
            bits.skip(poc_lsb_bits as usize + 1)?;
        }
    }
    // sps_temporal_mvp_enabled_flag, strong_intra_smoothing_enabled_flag.
    bits.skip(2)?;
    vui(bits)
}

/// H.265 7.3.4 `scaling_list_data()`.
fn scaling_list_data(bits: &mut Bits<'_>) -> Option<()> {
    for size in 0..4u32 {
        let matrices = if size == 3 { 2 } else { 6 };
        for _ in 0..matrices {
            if bits.flag()? {
                if size > 1 {
                    bits.se()?;
                }
                for _ in 0..(1usize << (4 + 2 * size)).min(64) {
                    bits.se()?;
                }
            } else {
                bits.ue()?;
            }
        }
    }
    Some(())
}

/// The delta POCs of one short-term reference picture set, `S0` (negative) and `S1`
/// (positive), in their derived order.
#[derive(Default)]
struct Rps {
    negative: Vec<i64>,
    positive: Vec<i64>,
}

/// Entries a set may hold before it is taken as corrupt (the DPB holds at most 16).
const MAX_RPS_ENTRIES: usize = 32;

/// H.265 7.3.7 `st_ref_pic_set()` as it appears in the SPS (a predicted set refers to the
/// one before it), with the 7-61 and 7-62 derivation the next set's syntax depends on.
fn st_ref_pic_set(bits: &mut Bits<'_>, previous: Option<&Rps>) -> Option<Rps> {
    let mut set = Rps::default();
    if let Some(reference) = previous
        && bits.flag()?
    {
        let negative_sign = bits.flag()?;
        let magnitude = i64::from(bits.ue()?) + 1;
        let delta = if negative_sign { -magnitude } else { magnitude };
        let (before, after) = (&reference.negative, &reference.positive);
        let count = before.len() + after.len();
        // use_delta_flag, inferred 1 when used_by_curr_pic_flag is set.
        let mut used = Vec::with_capacity(count + 1);
        for _ in 0..=count {
            let current = bits.flag()?;
            used.push(current || bits.flag()?);
        }
        let n = before.len();
        for (j, &poc) in after.iter().enumerate().rev() {
            if poc + delta < 0 && used[n + j] {
                set.negative.push(poc + delta);
            }
        }
        if delta < 0 && used[count] {
            set.negative.push(delta);
        }
        for (j, &poc) in before.iter().enumerate() {
            if poc + delta < 0 && used[j] {
                set.negative.push(poc + delta);
            }
        }
        for (j, &poc) in before.iter().enumerate().rev() {
            if poc + delta > 0 && used[j] {
                set.positive.push(poc + delta);
            }
        }
        if delta > 0 && used[count] {
            set.positive.push(delta);
        }
        for (j, &poc) in after.iter().enumerate() {
            if poc + delta > 0 && used[n + j] {
                set.positive.push(poc + delta);
            }
        }
    } else {
        let negatives = bits.ue()? as usize;
        let positives = bits.ue()? as usize;
        if negatives.saturating_add(positives) > MAX_RPS_ENTRIES {
            return None;
        }
        let mut poc = 0i64;
        for _ in 0..negatives {
            poc -= i64::from(bits.ue()?) + 1;
            bits.skip(1)?;
            set.negative.push(poc);
        }
        poc = 0;
        for _ in 0..positives {
            poc += i64::from(bits.ue()?) + 1;
            bits.skip(1)?;
            set.positive.push(poc);
        }
    }
    (set.negative.len() + set.positive.len() <= MAX_RPS_ENTRIES).then_some(set)
}
