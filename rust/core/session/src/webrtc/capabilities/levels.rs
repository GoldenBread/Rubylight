//! Codec level and tier selection for WebRTC H.264, HEVC, and AV1 SDP parameters.

/// The tier of a level. Main is what a decoder is guaranteed to take; High
/// is used only when no Main-tier level has the bitrate for the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Main,
    High,
}

impl Tier {
    /// HEVC `tier-flag` and AV1 `tier` in the SDP: 0 for Main, 1 for High.
    pub fn flag(self) -> u32 {
        match self {
            Self::Main => 0,
            Self::High => 1,
        }
    }
}

/// A HEVC level as the SDP `level-id`: `general_level_idc`, which is 30 times
/// the level number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HevcLevel {
    pub name: &'static str,
    pub level_id: u32,
}

/// An AV1 `seq_level_idx` for the SDP `level-idx`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Av1Level {
    pub name: &'static str,
    pub level_idx: u32,
}

/// The limits of one level that decide whether it holds a stream.
pub(super) struct Limits {
    max_pic_size: u64,
    max_h_size: u64,
    max_v_size: u64,
    /// Luma samples per second.
    max_sample_rate: u64,
    /// Bitrate in kbps for the Main and the High tier.
    max_kbps_main: u64,
    max_kbps_high: u64,
}

impl Limits {
    pub(super) fn holds_picture(&self, width: u64, height: u64, fps: u64) -> bool {
        width * height <= self.max_pic_size
            && width <= self.max_h_size
            && height <= self.max_v_size
            && width * height * fps <= self.max_sample_rate
    }
}

/// The lowest entry, then tier, that holds the stream. Every Main-tier level is
/// tried before any High tier one.
fn pick<L: Copy>(
    levels: &[(L, Limits)],
    width: u32,
    height: u32,
    fps: u32,
    bitrate_kbps: u32,
) -> Option<(L, Tier)> {
    let (w, h, f, kbps) = (
        u64::from(width),
        u64::from(height),
        u64::from(fps),
        u64::from(bitrate_kbps),
    );
    let holds = |limits: &Limits| limits.holds_picture(w, h, f);
    levels
        .iter()
        .find(|(_, l)| holds(l) && kbps <= l.max_kbps_main)
        .map(|(level, _)| (*level, Tier::Main))
        .or_else(|| {
            levels
                .iter()
                .find(|(_, l)| holds(l) && kbps <= l.max_kbps_high)
                .map(|(level, _)| (*level, Tier::High))
        })
}

const fn hevc(
    name: &'static str,
    level_id: u32,
    max_luma_ps: u64,
    max_luma_sr: u64,
    max_kbps_main: u64,
    max_kbps_high: u64,
) -> (HevcLevel, Limits) {
    (
        HevcLevel { name, level_id },
        Limits {
            max_pic_size: max_luma_ps,
            // Each side is at most sqrt(8 * MaxLumaPs).
            max_h_size: isqrt(8 * max_luma_ps),
            max_v_size: isqrt(8 * max_luma_ps),
            max_sample_rate: max_luma_sr,
            max_kbps_main,
            max_kbps_high,
        },
    )
}

/// The integer square root, rounded down.
const fn isqrt(n: u64) -> u64 {
    let mut root = 0;
    while (root + 1) * (root + 1) <= n {
        root += 1;
    }
    root
}

/// H.265 tables A.8 and A.9, from level 4.1 up to 5.2. Below 4.1 a Samsung
/// decoder may refuse the offer, as the reference host found, and above 5.2
/// is 8K. A stream bigger than 5.2 holds has no level here.
pub(super) const HEVC_LEVELS: [(HevcLevel, Limits); 4] = [
    hevc("4.1", 123, 2_228_224, 133_693_440, 20_000, 50_000),
    hevc("5", 150, 8_912_896, 267_386_880, 25_000, 100_000),
    hevc("5.1", 153, 8_912_896, 534_773_760, 40_000, 160_000),
    hevc("5.2", 156, 8_912_896, 1_069_547_520, 60_000, 240_000),
];

const fn av1(name: &'static str, level_idx: u32, limits: Limits) -> (Av1Level, Limits) {
    (Av1Level { name, level_idx }, limits)
}

const fn limits(
    max_pic_size: u64,
    max_h_size: u64,
    max_v_size: u64,
    max_sample_rate: u64,
    max_kbps_main: u64,
    max_kbps_high: u64,
) -> Limits {
    Limits {
        max_pic_size,
        max_h_size,
        max_v_size,
        max_sample_rate,
        max_kbps_main,
        max_kbps_high,
    }
}

/// AV1 spec annex A, from level 4.1 up to 5.2. Levels 4.2 and 4.3 are
/// undefined, and 5.3 has the limits of 5.2, so 5.2 always wins.
pub(super) const AV1_LEVELS: [(Av1Level, Limits); 4] = [
    av1(
        "4.1",
        9,
        limits(2_359_296, 6_144, 3_456, 141_557_760, 20_000, 50_000),
    ),
    av1(
        "5.0",
        12,
        limits(8_912_896, 8_192, 4_352, 267_386_880, 30_000, 100_000),
    ),
    av1(
        "5.1",
        13,
        limits(8_912_896, 8_192, 4_352, 534_773_760, 40_000, 160_000),
    ),
    av1(
        "5.2",
        14,
        limits(8_912_896, 8_192, 4_352, 1_069_547_520, 60_000, 240_000),
    ),
];

/// The lowest HEVC level, not below 4.1, whose picture size, sample rate and
/// Main-tier bitrate hold the stream; if no Main-tier level has the bitrate,
/// the lowest level of the High tier that does. For the HEVC fmtp:
/// `level-id` and `tier-flag`. `None` when even 5.2 High is too small.
/// At a bitrate of 10 Mbps this gives 4.1 at 1080p60, 5 at 1440p60 or
/// 1080p120, 5.1 at 4K60 or 1440p120 and 5.2 at 4K120.
pub fn hevc_level(
    width: u32,
    height: u32,
    fps: u32,
    bitrate_kbps: u32,
) -> Option<(HevcLevel, Tier)> {
    pick(&HEVC_LEVELS, width, height, fps, bitrate_kbps)
}

/// The same for AV1: `level-idx` and `tier`.
pub fn av1_level(width: u32, height: u32, fps: u32, bitrate_kbps: u32) -> Option<(Av1Level, Tier)> {
    pick(&AV1_LEVELS, width, height, fps, bitrate_kbps)
}

/// An H.264 level as the last byte of the SDP `profile-level-id`: `level_idc`, ten times the
/// level number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct H264Level {
    pub name: &'static str,
    pub level_idc: u32,
}

/// H.264 table A-1 from level 3.1, the level the offer always had, up to 5.2: MaxMBPS
/// (macroblocks a second), MaxFS (macroblocks a frame) and MaxBR (kbps, the Baseline factor the
/// offer's profile byte names). Level 6 and above, which 4K120 would need, browsers refuse.
const H264_LEVELS: [(H264Level, u64, u64, u64); 8] = [
    (
        H264Level {
            name: "3.1",
            level_idc: 31,
        },
        108_000,
        3_600,
        14_000,
    ),
    (
        H264Level {
            name: "3.2",
            level_idc: 32,
        },
        216_000,
        5_120,
        20_000,
    ),
    (
        H264Level {
            name: "4",
            level_idc: 40,
        },
        245_760,
        8_192,
        20_000,
    ),
    (
        H264Level {
            name: "4.1",
            level_idc: 41,
        },
        245_760,
        8_192,
        50_000,
    ),
    (
        H264Level {
            name: "4.2",
            level_idc: 42,
        },
        522_240,
        8_704,
        50_000,
    ),
    (
        H264Level {
            name: "5",
            level_idc: 50,
        },
        589_824,
        22_080,
        135_000,
    ),
    (
        H264Level {
            name: "5.1",
            level_idc: 51,
        },
        983_040,
        36_864,
        240_000,
    ),
    (
        H264Level {
            name: "5.2",
            level_idc: 52,
        },
        2_073_600,
        36_864,
        240_000,
    ),
];

/// The lowest H.264 level, not below 3.1, whose frame size, macroblock rate and bitrate hold the
/// stream, for the `profile-level-id`. `None` above 5.2. At 20 Mbps this gives 4.2 at 1080p60,
/// 5.1 at 1080p90 and 120 or 1440p60, and 5.2 at 1440p90 and 120.
pub fn h264_level(width: u32, height: u32, fps: u32, bitrate_kbps: u32) -> Option<H264Level> {
    // Pictures are coded in 16x16 macroblocks; a side may be at most sqrt(8 * MaxFS) of them.
    let (w, h) = (
        u64::from(width).div_ceil(16),
        u64::from(height).div_ceil(16),
    );
    H264_LEVELS
        .iter()
        .find(|(_, max_mbps, max_fs, max_kbps)| {
            let side = isqrt(8 * max_fs);
            w * h <= *max_fs
                && w <= side
                && h <= side
                && w * h * u64::from(fps) <= *max_mbps
                && u64::from(bitrate_kbps) <= *max_kbps
        })
        .map(|(level, ..)| *level)
}
