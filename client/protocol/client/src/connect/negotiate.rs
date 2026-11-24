//! Two-step video format, SharpColor, HDR, and 10-bit SDR negotiation shared
//! across Desktop (`desktop-media`) and Android (`nightjar-android`).
//!
//! Preserves `nightjar#169` (`SharpColor` request/pick diagnostics and native
//! 4:4:4 fallback reasons) while unifying format intersection and tie-breaking.

use crate::color;
use crate::sdp::{
    SCM_AV1_HIGH8_444, SCM_AV1_HIGH10_444, SCM_AV1_MAIN10, SCM_H264_HIGH8_444, SCM_HEVC_MAIN10,
    SCM_HEVC_REXT8_444, SCM_HEVC_REXT10_444, SCM_PYROWAVE, SCM_PYROWAVE_444, SCM_PYROWAVE_MAIN10,
    SCM_PYROWAVE_MAIN10_444, VIDEO_FORMAT_AV1_HIGH8_444, VIDEO_FORMAT_AV1_HIGH10_444,
    VIDEO_FORMAT_AV1_MAIN10, VIDEO_FORMAT_H264_HIGH8_444, VIDEO_FORMAT_H265_MAIN10,
    VIDEO_FORMAT_H265_REXT8_444, VIDEO_FORMAT_H265_REXT10_444, VIDEO_FORMAT_MASK_10BIT,
    VIDEO_FORMAT_MASK_AV1, VIDEO_FORMAT_MASK_H264, VIDEO_FORMAT_MASK_H265,
    VIDEO_FORMAT_MASK_PYROWAVE, VIDEO_FORMAT_MASK_YUV444, VIDEO_FORMAT_PYROWAVE,
    VIDEO_FORMAT_PYROWAVE_444, VIDEO_FORMAT_PYROWAVE_MAIN10, VIDEO_FORMAT_PYROWAVE_MAIN10_444,
};
use crate::sharp_color;

/// Native 4:4:4 formats (PyroWave 4:4:4 is its own codec preference).
pub const NATIVE_444_FORMATS: u32 = VIDEO_FORMAT_MASK_YUV444 & !VIDEO_FORMAT_MASK_PYROWAVE;

/// 10-bit formats added by 10-bit SDR.
pub const SDR10_FORMATS: u32 =
    VIDEO_FORMAT_H265_MAIN10 | VIDEO_FORMAT_AV1_MAIN10 | VIDEO_FORMAT_H265_REXT10_444;

/// Intersects client `formats` with host `server_formats` (`ServerCodecModeSupport`),
/// prioritizes HDR PyroWave over SDR-only PyroWave when 10-bit is present, and prevents
/// AV1/HEVC 4:2:0 8-bit from preempting an available requested 10-bit or 4:4:4 format.
pub fn negotiate_video_formats(mut formats: u32, server_formats: u32) -> u32 {
    formats &= VIDEO_FORMAT_MASK_H264
        | VIDEO_FORMAT_MASK_H265
        | VIDEO_FORMAT_MASK_AV1
        | VIDEO_FORMAT_MASK_PYROWAVE;

    const PAIRS: [(u32, u32); 11] = [
        (VIDEO_FORMAT_H265_MAIN10, SCM_HEVC_MAIN10),
        (VIDEO_FORMAT_AV1_MAIN10, SCM_AV1_MAIN10),
        (VIDEO_FORMAT_H264_HIGH8_444, SCM_H264_HIGH8_444),
        (VIDEO_FORMAT_H265_REXT8_444, SCM_HEVC_REXT8_444),
        (VIDEO_FORMAT_H265_REXT10_444, SCM_HEVC_REXT10_444),
        (VIDEO_FORMAT_AV1_HIGH8_444, SCM_AV1_HIGH8_444),
        (VIDEO_FORMAT_AV1_HIGH10_444, SCM_AV1_HIGH10_444),
        (VIDEO_FORMAT_PYROWAVE, SCM_PYROWAVE),
        (VIDEO_FORMAT_PYROWAVE_444, SCM_PYROWAVE_444),
        (VIDEO_FORMAT_PYROWAVE_MAIN10, SCM_PYROWAVE_MAIN10),
        (VIDEO_FORMAT_PYROWAVE_MAIN10_444, SCM_PYROWAVE_MAIN10_444),
    ];
    for (client_bit, host_bit) in PAIRS {
        if (server_formats & host_bit) == 0 {
            formats &= !client_bit;
        }
    }

    let mut pyrowave = formats & VIDEO_FORMAT_MASK_PYROWAVE;
    formats &= !VIDEO_FORMAT_MASK_PYROWAVE;
    if (formats & VIDEO_FORMAT_MASK_10BIT) != 0 {
        pyrowave &= VIDEO_FORMAT_MASK_10BIT;
    }

    let preferred = if (formats & VIDEO_FORMAT_MASK_10BIT) != 0 {
        VIDEO_FORMAT_MASK_10BIT
    } else {
        VIDEO_FORMAT_MASK_YUV444
    };
    if (formats & preferred) != 0 {
        if (formats & VIDEO_FORMAT_MASK_AV1 & preferred) == 0 {
            formats &= !VIDEO_FORMAT_MASK_AV1;
        }
        if (formats & VIDEO_FORMAT_MASK_H265 & preferred) == 0 {
            formats &= !VIDEO_FORMAT_MASK_H265;
        }
    }
    formats | pyrowave
}

/// Whether HDR is streaming as non-PyroWave even though both host and client offered
/// PyroWave 10-bit (device failed to build PyroWave HDR output).
pub fn pyro_wave_hdr_lost(common_formats: u32, negotiated_formats: u32) -> bool {
    let pw_hdr = VIDEO_FORMAT_MASK_PYROWAVE & VIDEO_FORMAT_MASK_10BIT;
    (common_formats & pw_hdr) != 0
        && (negotiated_formats & pw_hdr) == 0
        && (negotiated_formats & VIDEO_FORMAT_MASK_10BIT) != 0
}

/// Formats offered by packed full chroma (HEVC/AV1 4:2:0, 10-bit only with HDR).
pub fn packed_formats(offered: u32, hdr: bool) -> u32 {
    let formats = offered & (VIDEO_FORMAT_MASK_H265 | VIDEO_FORMAT_MASK_AV1) & !NATIVE_444_FORMATS;
    if hdr {
        formats
    } else {
        formats & !VIDEO_FORMAT_MASK_10BIT
    }
}

/// Formats remaining after [`sharp_color::Pick`].
pub fn formats_after_pick(pick: &sharp_color::Pick, offered: u32, hdr: bool) -> u32 {
    match pick {
        sharp_color::Pick::Native { formats } => *formats,
        sharp_color::Pick::Packed { .. } => packed_formats(offered, hdr),
        sharp_color::Pick::Without(_) => offered & !NATIVE_444_FORMATS,
    }
}

/// Builds a [`sharp_color::Input`] preserving `nightjar#169` fields.
#[allow(clippy::too_many_arguments)]
pub fn sharp_color_input(
    offered: u32,
    sdr10: bool,
    pyrowave: bool,
    packed_engine: bool,
    host_scm: u32,
    host_packed: Option<u8>,
    packed_refused_before: bool,
) -> sharp_color::Input {
    let ten_bit_sdr = if sdr10 { SDR10_FORMATS } else { 0 };
    let decodable = offered & (NATIVE_444_FORMATS | ten_bit_sdr);
    sharp_color::Input {
        auto: true,
        hdr: !sdr10 && (offered & VIDEO_FORMAT_MASK_10BIT) != 0,
        sdr10,
        pyrowave,
        packed_engine,
        base: offered & !NATIVE_444_FORMATS & !ten_bit_sdr,
        decodable,
        decoder_reason: if (offered & NATIVE_444_FORMATS) == 0 {
            "no 4:4:4 decoder on this device".to_owned()
        } else {
            String::new()
        },
        host_scm,
        host_packed: if packed_engine { host_packed } else { None },
        packed_refused_before: packed_engine && packed_refused_before,
    }
}

/// Removes 10-bit formats when checking whether native 4:4:4 wins over 10-bit SDR.
pub const fn without_ten_bit(formats: u32) -> u32 {
    formats & !VIDEO_FORMAT_MASK_10BIT
}

/// Whether common formats without 10-bit contain a native 4:4:4 format.
pub const fn yuv444_wins(common_without_ten_bit: u32) -> bool {
    (common_without_ten_bit & VIDEO_FORMAT_MASK_YUV444) != 0
}

/// Platform callback for the two-step video/HDR negotiation boundary.
pub trait PlatformVideoFormats {
    fn prepare_packed_modes(&mut self, probe_formats: u32) -> u8;
    fn prepare_video_formats(&mut self, common_formats: u32, host_scm: u32) -> u32;
    fn pick_hdr_format(
        &mut self,
        host_hdr_formats: i32,
        negotiated_formats: u32,
        packed_444: bool,
        frame_warp: bool,
    ) -> (i32, u32);
    fn video_capabilities(&mut self, negotiated_formats: u32, hdr_format: i32) -> u32;
}

/// Configuration for [`negotiate_two_step`].
#[derive(Clone, Copy, Debug)]
pub struct TwoStepConfig {
    pub initial_formats: u32,
    pub host_scm: u32,
    pub host_hdr_formats: i32,
    pub frame_warp: bool,
    pub sharp_color: bool,
    pub sdr10: bool,
    pub pyrowave: bool,
    pub packed_engine: bool,
    pub host_packed: Option<u8>,
    pub packed_refused_before: bool,
}

/// User-facing notice produced during two-step negotiation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NegotiationNotice {
    Sdr10GivenWayToPacked,
    Sdr10GivenWayToYuv444,
    Sdr10NotAccepted,
    Sdr10Accepted,
    HdrUnavailable,
    HdrAccepted,
    PyroWaveHdrLost,
    PyroWaveHdrAccepted,
    PackedUnavailableFromHost,
    PackedUnavailableFromDevice,
    PackedAvailable,
}

/// Result of [`negotiate_two_step`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TwoStepOutcome {
    pub offered_formats: u32,
    pub common_formats: u32,
    pub negotiated_formats: u32,
    pub negotiated_10bit: bool,
    pub negotiated_hdr: bool,
    pub hdr_format: i32,
    pub video_capabilities: u32,
    pub packed_444: bool,
    pub native_444: bool,
    pub keep_sdr10: bool,
    pub sharp_input: Option<sharp_color::Input>,
    pub sharp_pick: Option<sharp_color::Pick>,
    pub notices: Vec<NegotiationNotice>,
}

/// Executes the two-step codec/color/HDR negotiation matching `NvConnection.startApp`:
/// 1) SharpColor (`prepare_packed_modes` lazily if needed)
/// 2) 10-bit SDR vs native 4:4:4 tie-break
/// 3) Platform decoder format preparation (`prepare_video_formats`)
/// 4) HDR format pick (`pick_hdr_format`) & capabilities (`video_capabilities`)
pub fn negotiate_two_step(
    cfg: TwoStepConfig,
    platform: &mut dyn PlatformVideoFormats,
) -> TwoStepOutcome {
    let mut offered = cfg.initial_formats;
    let mut sharp_input_out = None;
    let mut sharp_pick_out = None;
    let mut packed_444 = false;
    let mut native_444 = false;
    let mut keep_sdr10 = cfg.sdr10;
    let mut notices = Vec::new();

    if cfg.sharp_color {
        let input = sharp_color_input(
            offered,
            cfg.sdr10,
            cfg.pyrowave,
            cfg.packed_engine,
            cfg.host_scm,
            cfg.host_packed,
            cfg.packed_refused_before,
        );
        let probe_formats =
            negotiate_video_formats(packed_formats(offered, input.hdr), cfg.host_scm);
        let pick = sharp_color::decide(&input, || platform.prepare_packed_modes(probe_formats));
        packed_444 = matches!(pick, sharp_color::Pick::Packed { .. });
        native_444 = matches!(pick, sharp_color::Pick::Native { .. });
        offered = formats_after_pick(&pick, offered, input.hdr);
        keep_sdr10 = cfg.sdr10 && !packed_444;
        if cfg.sdr10 && !keep_sdr10 {
            notices.push(NegotiationNotice::Sdr10GivenWayToPacked);
        }
        if let sharp_color::Pick::Without(ref why) = pick {
            if why.worth_saying() {
                if matches!(why, sharp_color::Why::Host) {
                    notices.push(NegotiationNotice::PackedUnavailableFromHost);
                } else {
                    notices.push(NegotiationNotice::PackedUnavailableFromDevice);
                }
            }
        } else {
            notices.push(NegotiationNotice::PackedAvailable);
        }
        sharp_input_out = Some(input);
        sharp_pick_out = Some(pick);
    }

    let mut common_formats = negotiate_video_formats(offered, cfg.host_scm);
    let mut yuv444_over_sdr10 = false;
    if keep_sdr10 {
        let without10 = without_ten_bit(offered);
        let common_without10 = negotiate_video_formats(without10, cfg.host_scm);
        if yuv444_wins(common_without10) {
            yuv444_over_sdr10 = true;
            offered = without10;
            common_formats = common_without10;
        }
    }

    let prepared = platform.prepare_video_formats(common_formats, cfg.host_scm);
    let mut negotiated = negotiate_video_formats(prepared, cfg.host_scm);
    let negotiated_10bit = (negotiated & VIDEO_FORMAT_MASK_10BIT) != 0;
    let negotiated_hdr = negotiated_10bit && !keep_sdr10;
    let (hdr_format, adjusted) = if negotiated_hdr {
        platform.pick_hdr_format(cfg.host_hdr_formats, negotiated, packed_444, cfg.frame_warp)
    } else {
        (0, negotiated)
    };
    if adjusted != 0 {
        negotiated = adjusted;
    }
    let video_capabilities = platform.video_capabilities(negotiated, hdr_format);

    if pyro_wave_hdr_lost(common_formats, negotiated) {
        notices.push(NegotiationNotice::PyroWaveHdrLost);
    } else if (negotiated & VIDEO_FORMAT_MASK_10BIT & VIDEO_FORMAT_MASK_PYROWAVE) != 0 {
        notices.push(NegotiationNotice::PyroWaveHdrAccepted);
    }

    if yuv444_over_sdr10 {
        notices.push(NegotiationNotice::Sdr10GivenWayToYuv444);
    } else if keep_sdr10 {
        if !negotiated_10bit {
            notices.push(NegotiationNotice::Sdr10NotAccepted);
        } else {
            notices.push(NegotiationNotice::Sdr10Accepted);
        }
    } else if !negotiated_hdr && (offered & VIDEO_FORMAT_MASK_10BIT) != 0 {
        notices.push(NegotiationNotice::HdrUnavailable);
    } else if negotiated_hdr {
        notices.push(NegotiationNotice::HdrAccepted);
    }

    TwoStepOutcome {
        offered_formats: offered,
        common_formats,
        negotiated_formats: negotiated,
        negotiated_10bit,
        negotiated_hdr,
        hdr_format,
        video_capabilities,
        packed_444,
        native_444,
        keep_sdr10,
        sharp_input: sharp_input_out,
        sharp_pick: sharp_pick_out,
        notices,
    }
}

/// Convenience wrapper over [`color::av1_given_up`] and [`color::offer`] for Desktop.
pub fn offered_formats_with_color(
    mut base: u32,
    wants: color::Wants,
    decodable: u32,
    decoder_reason: &str,
    host_scm: u32,
) -> color::Offer {
    if color::av1_given_up(base, wants, decodable, host_scm) != 0 {
        base &= !crate::sdp::VIDEO_FORMAT_AV1_MAIN8;
    }
    color::offer(base, wants, decodable, decoder_reason, host_scm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdp::{
        SCM_AV1_MAIN8, SCM_H264, SCM_HEVC, VIDEO_FORMAT_AV1_MAIN8, VIDEO_FORMAT_H264,
        VIDEO_FORMAT_H265,
    };

    #[test]
    fn negotiate_formats_keeps_h265_when_only_h265_has_preferred_10bit() {
        let offered = VIDEO_FORMAT_H264
            | VIDEO_FORMAT_H265
            | VIDEO_FORMAT_H265_MAIN10
            | VIDEO_FORMAT_AV1_MAIN8;
        let scm = SCM_H264 | SCM_HEVC | SCM_HEVC_MAIN10 | SCM_AV1_MAIN8;
        let out = negotiate_video_formats(offered, scm);
        assert_eq!(out & VIDEO_FORMAT_MASK_AV1, 0);
        assert_ne!(out & VIDEO_FORMAT_H265_MAIN10, 0);
    }

    #[test]
    fn pyrowave_hdr_precedes_sdr_when_10bit_present() {
        let offered = VIDEO_FORMAT_H264
            | VIDEO_FORMAT_H265_MAIN10
            | VIDEO_FORMAT_PYROWAVE
            | VIDEO_FORMAT_PYROWAVE_MAIN10;
        let scm = SCM_H264 | SCM_HEVC_MAIN10 | SCM_PYROWAVE | SCM_PYROWAVE_MAIN10;
        let out = negotiate_video_formats(offered, scm);
        assert_eq!(out & VIDEO_FORMAT_PYROWAVE, 0);
        assert_ne!(out & VIDEO_FORMAT_PYROWAVE_MAIN10, 0);
    }

    struct StubPlatform;
    impl PlatformVideoFormats for StubPlatform {
        fn prepare_packed_modes(&mut self, _probe_formats: u32) -> u8 {
            0
        }
        fn prepare_video_formats(&mut self, common_formats: u32, _host_scm: u32) -> u32 {
            common_formats
        }
        fn pick_hdr_format(&mut self, _: i32, negotiated: u32, _: bool, _: bool) -> (i32, u32) {
            (1, negotiated)
        }
        fn video_capabilities(&mut self, _: u32, _: i32) -> u32 {
            0x42
        }
    }

    #[test]
    fn two_step_negotiation_picks_native_444_when_both_support_it() {
        let mut p = StubPlatform;
        let out = negotiate_two_step(
            TwoStepConfig {
                initial_formats: VIDEO_FORMAT_H264
                    | VIDEO_FORMAT_H265
                    | VIDEO_FORMAT_H265_REXT8_444,
                host_scm: SCM_H264 | SCM_HEVC | SCM_HEVC_REXT8_444,
                host_hdr_formats: 0,
                frame_warp: false,
                sharp_color: true,
                sdr10: false,
                pyrowave: false,
                packed_engine: true,
                host_packed: None,
                packed_refused_before: false,
            },
            &mut p,
        );
        assert!(out.native_444);
        assert!(!out.packed_444);
        assert_eq!(out.video_capabilities, 0x42);
    }

    struct PackedStubPlatform;
    impl PlatformVideoFormats for PackedStubPlatform {
        fn prepare_packed_modes(&mut self, _probe_formats: u32) -> u8 {
            1
        }
        fn prepare_video_formats(&mut self, common_formats: u32, _host_scm: u32) -> u32 {
            common_formats
        }
        fn pick_hdr_format(&mut self, _: i32, negotiated: u32, _: bool, _: bool) -> (i32, u32) {
            (0, negotiated)
        }
        fn video_capabilities(&mut self, _: u32, _: i32) -> u32 {
            0
        }
    }

    #[test]
    fn two_step_sdr10_with_packed_pick_does_not_emit_sdr10_not_accepted() {
        let mut p = PackedStubPlatform;
        let out = negotiate_two_step(
            TwoStepConfig {
                initial_formats: VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10,
                host_scm: SCM_H264 | SCM_HEVC | SCM_HEVC_MAIN10,
                host_hdr_formats: 0,
                frame_warp: false,
                sharp_color: true,
                sdr10: true,
                pyrowave: false,
                packed_engine: true,
                host_packed: Some(1),
                packed_refused_before: false,
            },
            &mut p,
        );
        assert!(out.packed_444);
        assert!(!out.keep_sdr10);
        assert!(!out.negotiated_10bit);
        assert!(!out.negotiated_hdr);
        assert!(
            out.notices
                .contains(&NegotiationNotice::Sdr10GivenWayToPacked)
        );
        assert!(out.notices.contains(&NegotiationNotice::PackedAvailable));
        assert!(!out.notices.contains(&NegotiationNotice::Sdr10NotAccepted));
        assert!(!out.notices.contains(&NegotiationNotice::HdrUnavailable));
    }
}
