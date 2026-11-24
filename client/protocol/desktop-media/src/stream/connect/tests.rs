//! Unit tests for desktop stream connection helpers, decoder RFI/slice
//! capabilities, stage/recovery text, and audio configuration.

use super::*;

#[test]
fn a_recovery_that_gives_up_says_why_the_last_attempt_failed() {
    assert_eq!(
        RECOVERY_STOPPED,
        nightjar_policy::recovery::RECOVERY_STOPPED
    );
    assert_eq!(
        RECOVERY_UNREACHABLE,
        nightjar_policy::recovery::RECOVERY_UNREACHABLE
    );
    let text = recovery_gave_up(Some("Could not start the stream: connection refused"));
    assert!(
        text.starts_with("Could not start the stream: connection refused\n\n"),
        "{text}"
    );
    assert!(text.ends_with(RECOVERY_STOPPED), "{text}");
    for unknown in [None, Some("")] {
        assert_eq!(
            recovery_gave_up(unknown),
            format!("{RECOVERY_UNREACHABLE}\n\n{RECOVERY_STOPPED}")
        );
    }
}

#[test]
fn a_recovery_cause_is_the_apps_own_sentence() {
    use std::io::{Error, ErrorKind};
    let http = |kind| http_cause(&nightjar_core::http::Error::Io(Error::from(kind)));
    assert_eq!(
        http(ErrorKind::TimedOut),
        Some("The PC did not answer in time.")
    );
    assert_eq!(
        http(ErrorKind::ConnectionRefused),
        Some("The PC refused the connection.")
    );
    assert_eq!(
        http(ErrorKind::ConnectionReset),
        Some("The connection to the PC was lost.")
    );
    assert_eq!(http(ErrorKind::HostUnreachable), None);
    assert_eq!(
        http_cause(&nightjar_core::http::Error::Tls("handshake".into())),
        None
    );
    assert_eq!(
        start_cause(Stage::RtspHandshake),
        Some("The PC did not finish starting the stream.")
    );
    assert_eq!(start_cause(Stage::NameResolution), None);
    assert_eq!(
        recovery_gave_up(None),
        format!("{RECOVERY_UNREACHABLE}\n\n{RECOVERY_STOPPED}")
    );
}

#[test]
fn launch_stages_are_named_in_plain_words() {
    assert_eq!(stage_text(Stage::NameResolution), "Finding the PC");
    assert_eq!(stage_text(Stage::RtspHandshake), "Setting up the stream");
    assert_eq!(stage_text(Stage::ControlStreamStart), "Starting the stream");
    assert_eq!(stage_text(Stage::VideoStreamStart), "Starting video");
    assert_eq!(stage_text(Stage::AudioStreamStart), "Starting audio");
    assert_eq!(stage_text(Stage::InputStreamStart), "Connecting input");
}

fn decoded_by(backends: &[(u32, Hardware)]) -> color_probe::Base {
    color_probe::Base {
        decodable: backends.iter().fold(0, |all, &(format, _)| all | format),
        reason: String::new(),
        backends: backends.to_vec(),
    }
}

#[test]
fn reference_frame_invalidation_is_asked_for_where_the_backend_recovers_with_it() {
    let rfi =
        CAPABILITY_REFERENCE_FRAME_INVALIDATION_HEVC | CAPABILITY_REFERENCE_FRAME_INVALIDATION_AV1;
    let device = std::ptr::dangling_mut::<std::ffi::c_void>();
    let all = |backend| {
        decoded_by(&[
            (VIDEO_FORMAT_H265, backend),
            (VIDEO_FORMAT_H265_MAIN10, backend),
            (VIDEO_FORMAT_AV1_MAIN8, backend),
            (VIDEO_FORMAT_AV1_MAIN10, backend),
        ])
    };
    for backend in [Hardware::D3d11(device), Hardware::Vaapi] {
        assert_eq!(
            video_capabilities(&all(backend), false, true, true, 16),
            rfi,
            "{}: no slices for hardware",
            backend.label()
        );
    }
    for backend in [Hardware::Vulkan(device), Hardware::V4l2m2m, Hardware::Cuda] {
        assert_eq!(
            video_capabilities(&all(backend), false, true, true, 16),
            0,
            "{}",
            backend.label()
        );
    }
    assert_eq!(rfi & CAPABILITY_REFERENCE_FRAME_INVALIDATION_AVC, 0);
    let mixed = decoded_by(&[
        (VIDEO_FORMAT_H265, Hardware::Vaapi),
        (VIDEO_FORMAT_H265_MAIN10, Hardware::Vulkan(device)),
        (VIDEO_FORMAT_AV1_MAIN8, Hardware::Vaapi),
    ]);
    assert_eq!(
        video_capabilities(&mixed, false, true, true, 16),
        CAPABILITY_REFERENCE_FRAME_INVALIDATION_AV1
    );
    assert_eq!(
        video_capabilities(
            &decoded_by(&[(VIDEO_FORMAT_H265, Hardware::Vaapi)]),
            false,
            true,
            true,
            16
        ),
        CAPABILITY_REFERENCE_FRAME_INVALIDATION_HEVC
    );
    assert_eq!(
        video_capabilities(&all(Hardware::Vaapi), false, false, true, 16),
        0
    );
}

#[test]
fn vulkan_video_takes_rfi_only_when_switched_on() {
    assert!(!crate::config::Settings::default().vulkan_rfi);
    let device = std::ptr::dangling_mut::<std::ffi::c_void>();
    assert!(!early::takes_rfi_when(
        Hardware::Vulkan(device),
        false,
        true
    ));
    assert!(early::takes_rfi_when(Hardware::Vulkan(device), true, false));
    for backend in [Hardware::V4l2m2m, Hardware::Cuda] {
        assert!(
            !early::takes_rfi_when(backend, true, true),
            "{}",
            backend.label()
        );
    }
    for backend in [Hardware::D3d11(device), Hardware::Vaapi, Hardware::None] {
        assert!(
            early::takes_rfi_when(backend, false, false),
            "{}",
            backend.label()
        );
    }
}

#[test]
fn videotoolbox_takes_rfi_only_when_switched_on() {
    assert!(!crate::config::Settings::default().videotoolbox_rfi);
    #[cfg(target_os = "macos")]
    {
        assert!(!early::takes_rfi_when(Hardware::VideoToolbox, false, false));
        assert!(
            !early::takes_rfi_when(Hardware::VideoToolbox, true, false),
            "the Vulkan setting is not VideoToolbox's"
        );
        assert!(early::takes_rfi_when(Hardware::VideoToolbox, false, true));
    }
    assert!(!early::takes_rfi_when(Hardware::V4l2m2m, false, true));
}

#[test]
fn software_decoding_asks_for_a_slice_per_decoding_thread_up_to_4() {
    let software = color_probe::Base::software();
    let rfi =
        CAPABILITY_REFERENCE_FRAME_INVALIDATION_HEVC | CAPABILITY_REFERENCE_FRAME_INVALIDATION_AV1;
    assert_eq!(
        video_capabilities(&software, true, true, true, 16),
        rfi | capability_slices_per_frame(4)
    );
    assert_eq!(
        video_capabilities(&software, true, true, true, 2),
        rfi | capability_slices_per_frame(2)
    );
    assert_eq!(video_capabilities(&software, true, true, true, 0) >> 24, 1);
    assert!(!crate::config::Settings::default().software_slices);
    assert_eq!(video_capabilities(&software, true, true, false, 16), rfi);
}

fn reports_714(xml: &str) -> bool {
    ServerInfo::parse(xml, 47989)
        .expect("the answer parses")
        .capabilities
        .has(Feature::Surround714)
}

#[test]
fn only_a_pc_that_reports_7_1_4_is_asked_for_it() {
    let rubylight =
        include_str!("../../../../../app/src/test/resources/protocol/rubylight-30-serverinfo.xml");
    let rubylight_without = include_str!(
        "../../../../../app/src/test/resources/protocol/rubylight-30-refused-serverinfo.xml"
    );
    let sunshine =
        include_str!("../../../../../app/src/test/resources/protocol/sunshine-serverinfo.xml");
    assert!(reports_714(rubylight));
    assert!(!reports_714(rubylight_without), "Rubylight with 7.1.4 off");
    assert!(!reports_714(sunshine));

    let asked = |host: &str, output: bool| {
        audio_configuration(AudioChannels::Surround714, reports_714(host), output)
    };
    assert_eq!(asked(rubylight, true), AudioConfiguration::SURROUND_714);
    assert_eq!(asked(sunshine, true), AudioConfiguration::SURROUND_71);
    assert_eq!(
        asked(rubylight_without, true),
        AudioConfiguration::SURROUND_71
    );
    assert_eq!(asked(rubylight, false), AudioConfiguration::SURROUND_71);
    assert_eq!(
        audio_configuration(AudioChannels::Surround71, false, false),
        AudioConfiguration::SURROUND_71
    );
    assert_eq!(
        audio_configuration(AudioChannels::Surround51, false, false),
        AudioConfiguration::SURROUND_51
    );
    assert_eq!(
        audio_configuration(AudioChannels::Stereo, true, true),
        AudioConfiguration::STEREO
    );
}

#[test]
fn the_7_1_4_choice_round_trips_through_the_settings_file() {
    let saved = serde_json::to_string(&AudioChannels::Surround714).unwrap();
    assert_eq!(
        serde_json::from_str::<AudioChannels>(&saved).unwrap(),
        AudioChannels::Surround714
    );
}
