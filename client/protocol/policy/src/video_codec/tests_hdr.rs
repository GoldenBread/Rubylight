//! Unit tests for `Sdr10Policy`, `HdrPolicy`, and `HdrFormatPolicy` in `video_codec::hdr`.

use rubylight_protocol::hdr::{
    ALL, DOLBY_VISION, DOLBY_VISION_AV1, DOLBY_VISION_HEVC, DV_81, DV_84, DV_101, HLG, PQ,
    PYROWAVE_HLG,
};

use super::hdr::*;
use super::*;

const TV_MASK: u32 = DISPLAY_HDR_DOLBY_VISION | DISPLAY_HDR_HDR10 | DISPLAY_HDR_HLG;
const TV_FORMATS: u32 = VIDEO_FORMAT_H264
    | VIDEO_FORMAT_H265
    | VIDEO_FORMAT_H265_MAIN10
    | VIDEO_FORMAT_AV1_MAIN8
    | VIDEO_FORMAT_AV1_MAIN10;
const HEVC_ONLY: u32 = TV_FORMATS & !VIDEO_FORMAT_MASK_AV1;
const AV1_ONLY: u32 = TV_FORMATS & !VIDEO_FORMAT_MASK_H265;

#[test]
fn sdr10_policy_covers_refusal_host_hdr_step_and_overlay() {
    assert!(sdr10_yuv444_wins(
        VIDEO_FORMAT_H264 | VIDEO_FORMAT_H264_HIGH8_444
    ));
    assert!(!sdr10_yuv444_wins(
        VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265_MAIN10
    ));
    assert_eq!(
        sdr10_without_ten_bit(
            VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265_MAIN10 | VIDEO_FORMAT_AV1_MAIN10
        ),
        VIDEO_FORMAT_H264
    );

    assert_eq!(
        sdr10_host_hdr_step(false, Some(true), None),
        HostHdrStep::None
    );
    assert_eq!(sdr10_host_hdr_step(true, None, None), HostHdrStep::None);
    assert_eq!(
        sdr10_host_hdr_step(true, Some(true), None),
        HostHdrStep::Say
    );
    assert_eq!(
        sdr10_host_hdr_step(true, Some(true), Some(true)),
        HostHdrStep::None
    );
    assert_eq!(
        sdr10_host_hdr_step(true, Some(false), Some(true)),
        HostHdrStep::Rearm
    );
    assert_eq!(
        sdr10_host_hdr_step(true, Some(false), None),
        HostHdrStep::Rearm
    );

    assert!(sdr10_decoder_choice_may_change(true, false, false, false));
    assert!(!sdr10_decoder_choice_may_change(true, true, false, false));

    let ok = sdr10_refusal(true, true, false, true, true, false, false);
    assert_eq!(ok, Sdr10Refusal::None);
    assert_eq!(
        sdr10_formats(Some(ok), true, true),
        VIDEO_FORMAT_H265_MAIN10 | VIDEO_FORMAT_AV1_MAIN10
    );
    assert!(!sdr10_worth_saying(Some(ok)));

    let no_main10 = sdr10_refusal(true, true, false, false, false, false, false);
    assert_eq!(no_main10, Sdr10Refusal::NoMain10);
    assert!(sdr10_worth_saying(Some(no_main10)));
    assert_eq!(sdr10_formats(Some(no_main10), false, false), 0);

    assert_eq!(sdr10_status(true, Some(true)), Sdr10Status::HostHdr);
    assert_eq!(sdr10_status(true, None), Sdr10Status::Quiet);
    assert_eq!(sdr10_overlay_color(true, true, None), "10-bit SDR");
    assert_eq!(sdr10_overlay_color(true, true, Some(true)), "HDR");
    assert_eq!(sdr10_overlay_color(true, false, None), "10-bit");
    assert_eq!(sdr10_overlay_color(true, false, Some(false)), "SDR");
    assert!(!sdr10_assume_hdr(true, true));
    assert!(sdr10_assume_hdr(true, false));
}

#[test]
fn hdr_policy_covers_display_refusal_metadata_and_status() {
    assert_eq!(
        hdr_display_refusal(33, true, DISPLAY_HDR_HDR10, 0, 0, DrawnSurface::None),
        HdrRefusal::None
    );
    assert_eq!(
        hdr_display_refusal(33, false, DISPLAY_HDR_HDR10, 0, 0, DrawnSurface::None),
        HdrRefusal::SettingOff
    );
    assert_eq!(
        hdr_display_refusal(
            34,
            true,
            DISPLAY_HDR_HLG,
            DISPLAY_HDR_HDR10 | DISPLAY_HDR_HLG,
            0,
            DrawnSurface::None
        ),
        HdrRefusal::ModeWithoutHdr10
    );
    assert_eq!(
        hdr_display_refusal(
            33,
            true,
            DISPLAY_HDR_HLG,
            0,
            u32::from(ALL),
            DrawnSurface::PqOnly
        ),
        HdrRefusal::HlgNeedsDirectOutput
    );
    assert_eq!(
        hdr_display_refusal(
            33,
            true,
            DISPLAY_HDR_HLG,
            0,
            u32::from(ALL),
            DrawnSurface::PqHlg
        ),
        HdrRefusal::None
    );

    assert!(hdr_should_force_hlg_only(true, DISPLAY_HDR_HDR10));
    assert!(!hdr_should_force_hlg_only(false, DISPLAY_HDR_HDR10));
    assert!(!hdr_should_force_hlg_only(true, DISPLAY_HDR_DOLBY_VISION));

    assert_eq!(
        hdr_decoder_refusal(false, false, false),
        HdrRefusal::DecoderWithoutHdr10
    );
    assert_eq!(hdr_decoder_refusal(false, false, true), HdrRefusal::None);

    let mut usable = [0u8; 24];
    usable[0] = 0x34;
    usable[16] = 0xE8;
    usable[17] = 0x03;
    assert_eq!(hdr_metadata(None), HdrMetadataState::Absent);
    assert_eq!(hdr_metadata(Some(&[0u8; 23])), HdrMetadataState::Absent);
    assert_eq!(hdr_metadata(Some(&[0u8; 24])), HdrMetadataState::Empty);
    assert_eq!(hdr_metadata(Some(&usable)), HdrMetadataState::Usable);

    // Output transfer Other (e.g. COLOR_TRANSFER_LINEAR = 1)
    assert_eq!(
        hdr_status(
            false,
            true,
            Some(false),
            HdrMetadataState::Absent,
            ColorTransfer::Other,
            ColorTransfer::Pq,
            false
        ),
        HdrStatus::Quiet
    );
    assert_eq!(
        hdr_status(
            true,
            true,
            Some(true),
            HdrMetadataState::Usable,
            ColorTransfer::Other,
            ColorTransfer::Pq,
            false
        ),
        HdrStatus::OutputNotPq
    );
    assert_eq!(
        hdr_status(
            true,
            true,
            None,
            HdrMetadataState::Absent,
            ColorTransfer::None,
            ColorTransfer::Pq,
            true
        ),
        HdrStatus::PqOnHlgScreen
    );
    assert_eq!(
        hdr_status(
            true,
            true,
            Some(true),
            HdrMetadataState::Absent,
            ColorTransfer::Hlg,
            ColorTransfer::Hlg,
            true
        ),
        HdrStatus::Quiet
    );
    assert_eq!(
        hdr_status(
            true,
            true,
            Some(true),
            HdrMetadataState::Empty,
            ColorTransfer::Pq,
            ColorTransfer::Pq,
            false
        ),
        HdrStatus::NoMetadata
    );
    assert_eq!(
        hdr_drawn_status(HdrStatus::NoMetadata, true),
        HdrStatus::ShownAsSdr
    );
    assert!(hdr_hands_back_from_vulkan(true, true));
    assert!(!hdr_hands_back_from_vulkan(false, true));
    assert!(hdr_hand_over_after_vulkan_stop(false, false));
    assert!(!hdr_hand_over_after_vulkan_stop(true, false));
    assert!(hdr_reports_hdr_output(DISPLAY_HDR_HLG, ColorTransfer::Hlg));
    assert!(!hdr_reports_hdr_output(DISPLAY_HDR_HLG, ColorTransfer::Pq));
}

#[test]
fn hdr_format_policy_pick_vulkan_pyrowave_and_names() {
    assert_eq!(
        hdr_format_pick(
            u32::from(ALL),
            TV_MASK,
            u32::from(DOLBY_VISION),
            TV_FORMATS,
            DrawnSurface::None
        ),
        u32::from(DV_101)
    );
    assert_eq!(
        hdr_format_pick(
            u32::from(ALL),
            TV_MASK,
            u32::from(DOLBY_VISION_HEVC),
            TV_FORMATS,
            DrawnSurface::None
        ),
        u32::from(DV_81)
    );
    assert_eq!(
        hdr_format_pick(
            u32::from(DV_84 | PQ),
            TV_MASK,
            u32::from(DOLBY_VISION),
            TV_FORMATS,
            DrawnSurface::None
        ),
        u32::from(DV_84)
    );
    assert_eq!(
        hdr_format_pick(
            u32::from(ALL),
            TV_MASK,
            u32::from(DOLBY_VISION),
            TV_FORMATS,
            DrawnSurface::PqHlg
        ),
        0
    );
    assert_eq!(
        hdr_format_pick(
            u32::from(ALL),
            DISPLAY_HDR_HLG,
            0,
            TV_FORMATS,
            DrawnSurface::PqHlg
        ),
        u32::from(HLG)
    );
    assert_eq!(
        hdr_format_vulkan_limit(
            u32::from(ALL),
            TV_MASK | DISPLAY_HDR_HDR10_PLUS,
            u32::from(DOLBY_VISION),
            TV_FORMATS
        ),
        DrawnLimit::DolbyVision
    );
    assert!(hdr_format_vulkan_needs_hlg_probe(
        u32::from(ALL),
        DISPLAY_HDR_HLG,
        DrawnSurface::PqHlg
    ));
    assert_eq!(
        hdr_format_vulkan_surface(DrawnSurface::PqHlg, HlgWindow::Refused),
        DrawnSurface::PqOnly
    );
    assert!(hdr_format_passes_hdr10_plus(
        DISPLAY_HDR_HDR10_PLUS,
        ColorTransfer::Pq
    ));
    assert!(!hdr_format_passes_hdr10_plus(
        DISPLAY_HDR_HDR10_PLUS,
        ColorTransfer::Hlg
    ));

    let flagged = u32::from(ALL | PYROWAVE_HLG);
    assert_eq!(hdr_format_pyrowave_surface(flagged), DrawnSurface::PqHlg);
    assert_eq!(
        hdr_format_pyrowave_surface(u32::from(ALL)),
        DrawnSurface::PqOnly
    );
    assert!(hdr_format_pyrowave_needs_hlg_probe(
        flagged,
        DISPLAY_HDR_HLG
    ));
    assert!(hdr_format_pyrowave_hlg_refused_by_display(
        flagged,
        DISPLAY_HDR_HLG,
        HlgWindow::Refused
    ));
    assert!(hdr_format_pyrowave_hlg_lost(
        u32::from(HLG),
        Some(HlgWindow::Public),
        false,
        false
    ));
    assert!(!hdr_format_pyrowave_hlg_lost(
        u32::from(HLG),
        None,
        false,
        true
    ));
    assert!(!hdr_format_pyrowave_hlg_lost(
        u32::from(HLG),
        Some(HlgWindow::Refused),
        false,
        true
    ));
    assert_eq!(
        hdr_format_pyrowave_limit(flagged, DISPLAY_HDR_HDR10 | DISPLAY_HDR_HDR10_PLUS, 0, 0),
        DrawnLimit::Hdr10Plus
    );
    assert!(hdr_format_pyrowave_hlg_needs_host_update(
        u32::from(ALL),
        DISPLAY_HDR_HLG
    ));
    assert!(!hdr_format_pyrowave_hlg_needs_host_update(
        flagged,
        DISPLAY_HDR_HLG
    ));

    assert_eq!(
        hdr_format_carriers(u32::from(DV_81)),
        VIDEO_FORMAT_H265_MAIN10
    );
    assert_eq!(
        hdr_format_carriers(u32::from(DV_101)),
        VIDEO_FORMAT_AV1_MAIN10
    );
    assert_eq!(
        hdr_format_video_formats(u32::from(DV_81), TV_FORMATS, true),
        HEVC_ONLY
    );
    assert_eq!(
        hdr_format_video_formats(u32::from(DV_101), TV_FORMATS, true),
        AV1_ONLY
    );
    assert_eq!(
        hdr_format_codec_names(VIDEO_FORMAT_AV1_MAIN10 | VIDEO_FORMAT_H265),
        "AV1 and HEVC"
    );
    assert_eq!(hdr_format_overlay_word(0, false, true), "HDR10+");
    assert_eq!(
        hdr_format_overlay_word(u32::from(DV_84), false, false),
        "HLG (Dolby Vision decoder failed)"
    );
    assert_eq!(
        hdr_format_overlay_word(u32::from(DV_101), true, false),
        "Dolby Vision 10.1"
    );
    assert_eq!(HlgWindow::of_call(WINDOW_CALL_PUBLIC), HlgWindow::Public);
    assert_eq!(HlgWindow::of_call(WINDOW_CALL_PERFORM), HlgWindow::Perform);
    assert_eq!(HlgWindow::of_call(WINDOW_CALL_NONE), HlgWindow::Refused);
    assert_eq!(
        hdr_format_carriers(u32::from(DOLBY_VISION_HEVC)),
        VIDEO_FORMAT_H265_MAIN10
    );
    assert_eq!(
        hdr_format_carriers(u32::from(DOLBY_VISION_AV1)),
        VIDEO_FORMAT_AV1_MAIN10
    );
}
