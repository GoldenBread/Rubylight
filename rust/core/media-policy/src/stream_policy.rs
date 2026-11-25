//! Wire budget and encoder cadence used by the previous Moonlight host.

mod pacer;
mod send_faults;
mod source_phase;

pub use pacer::{
    Cadence, Pace, Pacer, Pacing, counts_toward_rate, reencode_at, resend_anchor, source_phase,
};
pub use send_faults::{
    AdapterStall, JitterLine, SendDrop, SendJitter, SendLoss, SendLossRecovery, SendOutage,
    VIDEO_SOCKET_BUFFER,
};
#[cfg(test)]
use source_phase::SourcePhase;

use crate::rtsp::Negotiated;
use butterpollo_config::config::Config;
use std::time::Duration;

/// `prefer_sdr_10bit` streams an HDR request as ten-bit SDR, without the HDR
/// format the client picked.
pub fn apply_color(stream: &mut Negotiated, config: &Config) {
    // An HDR request stays HDR when RTX HDR converts an SDR source for it.
    if stream.hdr
        && config.boolean("prefer_sdr_10bit", false)
        && !crate::nvidia_enhance::rtx_hdr_runs(config)
    {
        stream_sdr_10bit(stream);
    }
}
/// Why a packed 4:4:4 stream never carries Dolby Vision, for the log, the
/// session warning and the setting's note in the console.
pub const PACKED_DOLBY_VISION: &str = "Dolby Vision is off for packed 4:4:4 streams: the device unpacks the packed picture itself, so a Dolby Vision decoder would tone-map the packed layout instead of the picture. The stream carries static HDR10 instead";
/// Why a native 4:4:4 stream never carries Dolby Vision.
pub const NATIVE_444_DOLBY_VISION: &str = "Dolby Vision is off for YUV 4:4:4 streams: its profiles 8.1 and 8.4 are 4:2:0 only. The stream carries static HDR10 instead";
/// Why a full-range stream never carries Dolby Vision.
pub const FULL_RANGE_DOLBY_VISION: &str = "Dolby Vision is off for full-range streams: its RPU base-layer profiles assume limited-range video. The stream carries static HDR10 instead";
/// A packed 4:4:4 stream whose client picked Dolby Vision streams static
/// HDR10, from the control channel: the client decodes the packed
/// picture as plain sample data and unpacks it in its own shader, and the
/// RPU would not survive the unpack. A native 4:4:4 stream (HEVC Rext) does
/// the same: Dolby Vision profile 8 is HEVC Main10 4:2:0. A full-range
/// stream likewise falls back to static HDR10 because the RPU metadata
/// assumes limited-range video. The log and the session's warnings say so;
/// any other stream clears the warning. Returns whether it dropped it.
pub fn packed_drops_dolby_vision(
    stream: &mut Negotiated,
    warnings: &(impl crate::WarningSink + ?Sized),
) -> bool {
    let dropped = (stream.chroma_packing != 0 || stream.yuv444 || stream.full_range())
        && stream.hdr_format & rubylight_protocol::hdr::DOLBY_VISION != 0;
    if dropped {
        let reason = if stream.yuv444 {
            NATIVE_444_DOLBY_VISION
        } else if stream.chroma_packing != 0 {
            PACKED_DOLBY_VISION
        } else {
            FULL_RANGE_DOLBY_VISION
        };
        tracing::warn!(hdr_format = stream.hdr_format, "{reason}");
        warnings.set("hdr_dolby_vision", reason);
        stream.hdr_format = 0;
    } else {
        warnings.clear("hdr_dolby_vision");
    }
    dropped
}
/// Why a packed 4:4:4 stream never carries HLG, for the log and the session
/// warning.
pub const PACKED_HLG: &str = "HLG is off for packed 4:4:4 streams: the device unpacks the packed picture itself and shows it as HDR10. The stream carries static HDR10 instead";
/// A packed 4:4:4 stream whose client picked HLG streams HDR10, like one that
/// picked Dolby Vision ([`packed_drops_dolby_vision`], which drops 8.4 as
/// Dolby Vision first): the client's unpack shader shows the picture as PQ.
/// The log and the session's warnings say so; any other stream clears the
/// warning. Returns whether it dropped it.
pub fn packed_drops_hlg(
    stream: &mut Negotiated,
    warnings: &(impl crate::WarningSink + ?Sized),
) -> bool {
    let dropped = stream.chroma_packing != 0 && stream.hdr_format == rubylight_protocol::hdr::HLG;
    if dropped {
        tracing::warn!(hdr_format = stream.hdr_format, "{PACKED_HLG}");
        warnings.set("hdr_hlg", PACKED_HLG);
        stream.hdr_format = 0;
    } else {
        warnings.clear("hdr_hlg");
    }
    dropped
}
/// An HDR request streamed as ten-bit SDR: its HDR format pick goes with it.
pub fn stream_sdr_10bit(stream: &mut Negotiated) {
    stream.sdr_10bit = true;
    stream.hdr = false;
    stream.hdr_format = 0;
}
/// The protocol has no flag for ten-bit SDR: a client that wants it
/// negotiates a ten-bit format in RTSP (`dynamicRangeMode=1`) while its launch
/// did not ask for HDR. `sdr_requested` is what `Launch::ten_bit_sdr_requested`
/// decides from the query. Such a stream is ten-bit SDR, so the display stays
/// as launched. Returns whether it was inferred.
pub fn infer_sdr_10bit(stream: &mut Negotiated, sdr_requested: bool) -> bool {
    let inferred = stream.hdr && sdr_requested;
    if inferred {
        stream_sdr_10bit(stream);
    }
    inferred
}
pub fn apply(stream: &mut Negotiated, launch_millihz: u32, config: &Config) {
    apply_dscp(stream, config);
    // The stream runs at the rate its request asks for, as in Vibepollo,
    // where `limit_framerate` only aligns encoder timestamps to the launch
    // rate. Turned on here, the stream is encoded at the launch rate.
    let limit = config.boolean("limit_framerate", false);
    if limit && (1..=4_000_000).contains(&launch_millihz) {
        stream.rate_millihz = launch_millihz;
    } else if (1000..=4_000_000).contains(&launch_millihz)
        && stream.fps_millihz() == stream.fps.saturating_mul(1000)
        && launch_millihz != stream.fps_millihz()
        && stream.fps == launch_millihz.saturating_add(500) / 1000
    {
        // Launch carried a fractional refresh (e.g. 59.94 Hz) while RTSP SDP
        // only carried the rounded integer `maxFPS` (60). Preserve the launch's
        // exact millihertz rate instead of snapping to the rounded integer.
        stream.rate_millihz = launch_millihz;
    }
    let fec = config.integer("fec_percentage", 20);
    stream.launch_fec = if (0..=80).contains(&fec) {
        fec as u8
    } else {
        0
    };
    stream.bitrate_kbps = encoder_share_kbps(stream, bitrate_ceiling_kbps(config));
}
/// `max_bitrate` in Kbps, or the most a bitrate field holds.
fn bitrate_ceiling_kbps(config: &Config) -> u64 {
    match config.integer("max_bitrate", 0) {
        ceiling if ceiling > 0 => ceiling as u64,
        _ => NO_BITRATE_CEILING_KBPS,
    }
}
const NO_BITRATE_CEILING_KBPS: u64 = i32::MAX as u64 / 1000;
/// Breakdown of how a launch bitrate became the encoder target and link-rate ceiling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BitrateBudget {
    pub requested_kbps: u32,
    pub host_max_bitrate_kbps: u32,
    pub fec_reserved_kbps: u32,
    pub audio_reserved_kbps: u32,
    pub overhead_reserved_kbps: u32,
    pub encoder_kbps: u32,
    pub link_ceiling_kbps: u32,
    pub ceiling_source: &'static str,
}

fn share_breakdown(stream: &Negotiated, ceiling: u64) -> BitrateBudget {
    let configured = u64::from(stream.configured_bitrate_kbps);
    let requested = if configured > 0 {
        configured
    } else {
        u64::from(stream.bitrate_kbps)
    };
    // A stream request at a multiple of the launch rate (Artemis's Warp modes
    // ask for 2x or 4x) keeps the client's bitrate. Vibepollo multiplies it
    // by that factor, which sent two to four times the bitrate the client set:
    // more than an Onn 4K Pro's 100 Mbps port carries. Each frame gets a half
    // or a quarter of Vibepollo's budget instead.
    let mut budget = requested.min(ceiling);
    let mut fec_reserved = 0u64;
    let mut audio_reserved = 0u64;
    let mut overhead_reserved = 0u64;
    if configured > 0 {
        if stream.codec != 3 {
            let after_fec = budget * (100 - u64::from(stream.launch_fec.min(80))) / 100;
            fec_reserved = budget.saturating_sub(after_fec);
            budget = after_fec;
        }
        let audio = u64::from(stream.audio_channels) * if stream.audio_quality { 256 } else { 96 };
        if stream.codec == 3 {
            // PyroWave's bitrate is the stream's whole wire budget, as
            // Vibeshine takes it: audio with its 50% parity and control come
            // off in full, uncapped, so a calibrated total holds.
            let audio_share = (audio * 3).div_ceil(2);
            audio_reserved = audio_share.min(budget);
            let after_audio = budget.saturating_sub(audio_share);
            overhead_reserved = 500.min(after_audio);
            budget = budget.saturating_sub(audio_share + 500);
        } else {
            audio_reserved = audio.min(budget / 5);
            budget -= audio_reserved;
            overhead_reserved = 500.min(budget / 10);
            budget -= overhead_reserved;
        }
    }
    let encoder_kbps = budget.clamp(1, 2_000_000) as u32;
    let host_max_bitrate_kbps = if ceiling < NO_BITRATE_CEILING_KBPS {
        ceiling.min(u64::from(u32::MAX)) as u32
    } else {
        0
    };
    BitrateBudget {
        requested_kbps: requested.min(u64::from(u32::MAX)) as u32,
        host_max_bitrate_kbps,
        fec_reserved_kbps: fec_reserved as u32,
        audio_reserved_kbps: audio_reserved as u32,
        overhead_reserved_kbps: overhead_reserved as u32,
        encoder_kbps,
        link_ceiling_kbps: encoder_kbps,
        ceiling_source: "client_request",
    }
}

/// The encoder's bitrate for `stream`, at most `ceiling` Kbps of what it asks
/// for. A client that sent the bitrate its user set (`configured_bitrate_kbps`)
/// gets that number less the FEC share (`launch_fec`), audio and 500 Kbps of
/// overhead, as Sunshine takes it, so video, parity and audio together stay
/// within it; one that sent none already took its own share off
/// `bitrate_kbps`.
fn encoder_share_kbps(stream: &Negotiated, ceiling: u64) -> u32 {
    share_breakdown(stream, ceiling).encoder_kbps
}

/// Explain the effective encoder bitrate and link-rate ceiling at stream start,
/// matching the calculations in [`apply`] and [`crate::link_rate::LinkRate::new`].
pub fn explain_bitrate(stream: &Negotiated, config: &Config, max_kbps: u32) -> BitrateBudget {
    let mut breakdown = share_breakdown(stream, bitrate_ceiling_kbps(config));
    let encoder_kbps = if stream.bitrate_kbps > 0 {
        stream.bitrate_kbps
    } else {
        breakdown.encoder_kbps
    };
    breakdown.encoder_kbps = encoder_kbps;
    let codec_cap = if stream.codec == 3 {
        2_000_000
    } else {
        500_000
    };
    let link_cap = if max_kbps > 0 { max_kbps } else { u32::MAX };
    breakdown.link_ceiling_kbps = encoder_kbps.min(codec_cap).min(link_cap);
    breakdown.ceiling_source = if link_cap < encoder_kbps && link_cap < codec_cap {
        "host_max_bitrate"
    } else if codec_cap < encoder_kbps {
        "codec_limit"
    } else if breakdown.host_max_bitrate_kbps > 0
        && breakdown.host_max_bitrate_kbps < breakdown.requested_kbps
    {
        "host_max_bitrate"
    } else if breakdown.link_ceiling_kbps < breakdown.requested_kbps {
        "wire_budget_after_fec_audio"
    } else {
        "client_request"
    };
    breakdown
}
/// The encoder's bitrate for a bitrate of `kbps` a client asks for mid-stream
/// (0x553C), in the unit of its launch: the stream of a client that sent the
/// bitrate its user set gets what its launch with `kbps` would have had, under
/// today's `max_bitrate`, so a number means the same in the client's settings
/// and in its stream menu. Without that, a request for the launch's own
/// 20 Mbps raised an encoder running at 14988 Kbps to 20000. A stream launched
/// without one is asked for the encoder's own bitrate.
pub fn runtime_encoder_kbps(launched: &Negotiated, kbps: u32, config: &Config) -> u32 {
    if launched.configured_bitrate_kbps == 0 {
        return kbps;
    }
    let mut stream = launched.clone();
    stream.configured_bitrate_kbps = kbps;
    encoder_share_kbps(&stream, bitrate_ceiling_kbps(config))
}
/// [`runtime_encoder_kbps`] the other way: the bitrate in the unit of the
/// launch that an encoder running at `encoder_kbps` stands for, which is the
/// lowest that gives the encoder that much, so asking for it again leaves the
/// encoder where it is. The launch's own number where the encoder still runs
/// at the launch's bitrate; `max_bitrate` plays no part.
pub fn runtime_client_kbps(launched: &Negotiated, encoder_kbps: u32) -> u32 {
    if launched.configured_bitrate_kbps == 0 {
        return encoder_kbps;
    }
    if encoder_share_kbps(launched, NO_BITRATE_CEILING_KBPS) == encoder_kbps {
        return launched.configured_bitrate_kbps;
    }
    let mut stream = launched.clone();
    let mut reaches = |kbps: u32| {
        stream.configured_bitrate_kbps = kbps;
        encoder_share_kbps(&stream, NO_BITRATE_CEILING_KBPS) >= encoder_kbps
    };
    // The share never grows a bitrate, and it never falls as the bitrate rises.
    let (mut low, mut high) = (encoder_kbps.max(1), NO_BITRATE_CEILING_KBPS as u32);
    if !reaches(high) {
        return high;
    }
    while low < high {
        let middle = low + (high - low) / 2;
        if reaches(middle) {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    low
}
/// `dscp_tagging` (default on, as in Sunshine, LizardByte/Sunshine#6067):
/// off, the host tags neither video nor audio with DSCP and asks Windows for
/// no qWAVE flow, whatever the device asks for. Some consumer routers drop or
/// delay tagged packets, heard as crackling audio. A device that asks for no
/// tagging (a remote one) gets none either way.
pub fn apply_dscp(stream: &mut Negotiated, config: &Config) {
    if !config.boolean("dscp_tagging", true) {
        stream.video_qos = false;
        stream.audio_qos = false;
    }
}
/// Video packets always use the size the device asked for. Moonlight receives into a
/// buffer of that size, so a larger packet is cut short (#52: 1200 bytes against the 1184
/// a remote IPv6 Moonlight asks for, a green picture with no loss), and it rebuilds
/// FEC-recovered packets at that size, so a smaller one corrupts every repaired frame.
/// A `packetsize` left in the config is ignored and reported.
pub fn report_packet_size(warnings: &(impl crate::WarningSink + ?Sized), config: &Config) {
    let packet_size = config.integer("packetsize", 0);
    if packet_size != 0 {
        warnings.set("packet_size", &format!("packetsize = {packet_size} is ignored: video packets use the size the device asks for. Remove it from the config."));
    } else {
        warnings.clear("packet_size");
    }
}
/// The encoder bitrate [`apply`] would choose without `max_bitrate`.
pub fn uncapped_bitrate_kbps(stream: &Negotiated, launch_millihz: u32, config: &Config) -> u32 {
    let mut uncapped = stream.clone();
    let mut config = config.clone();
    config.values.remove("max_bitrate");
    apply(&mut uncapped, launch_millihz, &config);
    uncapped.bitrate_kbps
}
pub fn report_bitrate(
    warnings: &(impl crate::WarningSink + ?Sized),
    requested: u32,
    applied: u32,
    reason: &str,
) {
    if applied < requested {
        warnings.set("network_bitrate", &format!("Encoder bitrate reduced from {requested} to {applied} Kbps: {reason}. Picture detail may be lower; check Maximum bitrate and the client bitrate, leaving room for audio and FEC."));
    } else {
        warnings.clear("network_bitrate");
    }
}

/// The bitrate a client may set during a stream: `max_bitrate` caps it, and
/// 500 Mbps keeps it inside the encoders' rate fields, as in Vibepollo.
pub fn runtime_bitrate_kbps(config: &Config, requested: u32) -> u32 {
    let ceiling = config.integer("max_bitrate", 0);
    let applied = requested.min(500_000);
    if ceiling > 0 {
        applied.min(ceiling.min(i64::from(u32::MAX)) as u32)
    } else {
        applied
    }
}
/// How long an encoder holding a full backlog may return nothing before the
/// stream recreates it. The first stall waits 250 ms; each recreation that
/// brings no frame back doubles the wait, up to 2 s. A GPU saturated by a
/// game (99% in an RX 9070 XT report) can hold the encoder's input for well
/// over a frame; recreating it then only adds an encoder start and a large
/// keyframe to a GPU that is already behind, and a stream that should have
/// turned choppy ended instead. A fresh encoder's first 4K keyframe on the
/// RX 9070 XT's single VCN can outlast a short limit too.
pub fn encoder_stall_limit(recreations: u32) -> Duration {
    Duration::from_millis(250 << recreations.min(3))
}
#[cfg(test)]
mod tests;
