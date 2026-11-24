//! Pure connection-stage, termination, startup-failure, and launch-failure
//! classifications shared between Desktop (`desktop-media`) and Android (`nightjar-android`).
//!
//! Functions return typed enums/keys so each frontend renders its own localized
//! strings (`en`/`tr`/`trf` on Desktop, `R.string.*` on Android).

use serde::{Deserialize, Serialize};

use crate::full_chroma::PACKED_444_REFUSED;
use crate::launch::CLIENT_ERROR_DECODER;

/// Connection stage label key, unifying Desktop's `stage_text` and Android's
/// `GameConnection.stageText`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StageKey {
    FindingPc,
    SettingUpStream,
    StartingStream,
    StartingVideo,
    StartingAudio,
    ConnectingInput,
    StartingApp,
}

impl StageKey {
    /// Stable wire tag for JNI/JSON consumers.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FindingPc => "finding_pc",
            Self::SettingUpStream => "setting_up_stream",
            Self::StartingStream => "starting_stream",
            Self::StartingVideo => "starting_video",
            Self::StartingAudio => "starting_audio",
            Self::ConnectingInput => "connecting_input",
            Self::StartingApp => "starting_app",
        }
    }
}

/// Maps a protocol stage name (from `Stage::name()` or `LiGetStageName`) to its [`StageKey`].
/// Unknown or empty names fall back to [`StageKey::StartingApp`].
pub fn stage_key_from_name(name: Option<&str>) -> StageKey {
    let Some(name) = name else {
        return StageKey::StartingApp;
    };
    match name.trim().to_ascii_lowercase().as_str() {
        "platform initialization" | "name resolution" => StageKey::FindingPc,
        "audio stream initialization" | "rtsp handshake" => StageKey::SettingUpStream,
        "control stream initialization" | "control stream establishment" => {
            StageKey::StartingStream
        }
        "video stream initialization" | "video stream establishment" => StageKey::StartingVideo,
        "audio stream establishment" => StageKey::StartingAudio,
        "input stream initialization" | "input stream establishment" => StageKey::ConnectingInput,
        _ => StageKey::StartingApp,
    }
}

/// Termination reason codes shared across `nightjar-core::session::error` and `MoonBridge`.
pub const GRACEFUL_TERMINATION: i32 = 0;
pub const NO_VIDEO_TRAFFIC: i32 = -100;
pub const NO_VIDEO_FRAME: i32 = -101;
pub const UNEXPECTED_EARLY: i32 = -102;
pub const PROTECTED_CONTENT: i32 = -103;
pub const FRAME_CONVERSION: i32 = -104;
pub const DISCONNECTED: i32 = -105;
pub const VULKAN_RENDERER_FAILED: i32 = -106;
pub const HOST_GONE: i32 = -108;

/// Typed key for stream termination messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TerminationTextKey {
    Graceful,
    NoVideoTraffic,
    NoVideoFrame,
    UnexpectedEarly,
    ProtectedContent,
    FrameConversion,
    VulkanRenderer,
    Decoder,
    HostGone,
    Disconnected,
    Unexpected(i32),
}

impl TerminationTextKey {
    /// Stable wire tag for JNI/JSON consumers.
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Graceful => "graceful",
            Self::NoVideoTraffic => "no_video_traffic",
            Self::NoVideoFrame => "no_video_frame",
            Self::UnexpectedEarly => "unexpected_early",
            Self::ProtectedContent => "protected_content",
            Self::FrameConversion => "frame_conversion",
            Self::VulkanRenderer => "vulkan_renderer",
            Self::Decoder => "decoder",
            Self::HostGone => "host_gone",
            Self::Disconnected => "disconnected",
            Self::Unexpected(_) => "unexpected",
        }
    }
}

/// Classifies a stream termination code into a [`TerminationTextKey`].
/// Note that on Android `-106` is `CLIENT_ERROR_VULKAN_RENDERER` (distinct from
/// `PACKED_444_REFUSED` which is `-106` in `full_chroma` during startup/stage failure,
/// never as a post-start termination code).
pub const fn classify_termination(code: i32) -> TerminationTextKey {
    match code {
        GRACEFUL_TERMINATION => TerminationTextKey::Graceful,
        NO_VIDEO_TRAFFIC => TerminationTextKey::NoVideoTraffic,
        NO_VIDEO_FRAME => TerminationTextKey::NoVideoFrame,
        UNEXPECTED_EARLY => TerminationTextKey::UnexpectedEarly,
        PROTECTED_CONTENT => TerminationTextKey::ProtectedContent,
        FRAME_CONVERSION => TerminationTextKey::FrameConversion,
        VULKAN_RENDERER_FAILED => TerminationTextKey::VulkanRenderer,
        CLIENT_ERROR_DECODER => TerminationTextKey::Decoder,
        HOST_GONE => TerminationTextKey::HostGone,
        DISCONNECTED => TerminationTextKey::Disconnected,
        other => TerminationTextKey::Unexpected(other),
    }
}

/// Whether `code` is an HRESULT-style large error code formatted in hex (`0x...`).
pub const fn is_hex_error_code(code: i32) -> bool {
    code.unsigned_abs() > 1000
}

/// Formats an error code as decimal or `0x...` hex (`Game.errorCodeText` parity).
pub fn format_error_code(code: i32) -> String {
    if is_hex_error_code(code) {
        format!("0x{:x}", code as u32)
    } else {
        code.to_string()
    }
}

/// Whether a failed stage represents a local video decoder failure rather than a network/host
/// issue (`Game.isDecoderStageFailure`).
pub fn is_decoder_stage_failure(has_decoder_failure: bool, stage: Option<&str>) -> bool {
    has_decoder_failure && stage.is_some_and(|s| s.to_ascii_lowercase().contains("video"))
}

/// Classification of startup failures (`Game.startupFailureText`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StartupFailureKind {
    DecoderFailure,
    ConnectionMessage,
    ConnectionFailed,
    HostError(i32),
}

/// Classifies a stage failure into [`StartupFailureKind`].
pub fn classify_startup_failure(
    has_decoder_failure: bool,
    stage: Option<&str>,
    has_connection_error_message: bool,
    error_code: i32,
) -> StartupFailureKind {
    if is_decoder_stage_failure(has_decoder_failure, stage) {
        StartupFailureKind::DecoderFailure
    } else if has_connection_error_message {
        StartupFailureKind::ConnectionMessage
    } else if error_code == 0 {
        StartupFailureKind::ConnectionFailed
    } else {
        StartupFailureKind::HostError(error_code)
    }
}

/// Classification of HTTP/local launch errors (`NvConnection.launchFailureMessage`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LaunchFailureKind {
    LocalOverride,
    PermissionDenied { has_reason: bool },
    HostMessage { code: i32 },
    HostError { code: i32 },
}

/// Classifies a launch HTTP error into [`LaunchFailureKind`].
pub fn classify_launch_failure(
    is_local: bool,
    code: i32,
    trimmed_reason: &str,
) -> LaunchFailureKind {
    if is_local {
        return LaunchFailureKind::LocalOverride;
    }
    let has_reason = !trimmed_reason.trim().is_empty();
    if code == 401 || code == 403 {
        LaunchFailureKind::PermissionDenied { has_reason }
    } else if has_reason {
        LaunchFailureKind::HostMessage { code }
    } else {
        LaunchFailureKind::HostError { code }
    }
}

/// Whether `error_code` is a packed 4:4:4 refusal at stage failure (`-106`).
pub const fn is_packed_444_refused(error_code: i32) -> bool {
    error_code == PACKED_444_REFUSED
}

/// Plain-word cause for a retryable HTTP I/O failure (`desktop-media` `http_cause`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HttpCauseKey {
    TimedOut,
    ConnectionRefused,
    ConnectionLost,
}

/// Plain-word cause for a failed start stage (`desktop-media` `start_cause`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StartCauseKey {
    DidNotFinish,
}

/// Classifies a start stage failure cause; Finding the PC (`Platform initialization` /
/// `Name resolution`) returns `None` so the give-up shows the generic unreachable message.
pub fn classify_start_cause(stage_name: &str) -> Option<StartCauseKey> {
    match stage_key_from_name(Some(stage_name)) {
        StageKey::FindingPc => None,
        _ => Some(StartCauseKey::DidNotFinish),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_names_map_to_expected_keys() {
        assert_eq!(
            stage_key_from_name(Some("name resolution")),
            StageKey::FindingPc
        );
        assert_eq!(
            stage_key_from_name(Some("Platform initialization")),
            StageKey::FindingPc
        );
        assert_eq!(
            stage_key_from_name(Some("RTSP handshake")),
            StageKey::SettingUpStream
        );
        assert_eq!(
            stage_key_from_name(Some("Audio stream initialization")),
            StageKey::SettingUpStream
        );
        assert_eq!(
            stage_key_from_name(Some("Control stream establishment")),
            StageKey::StartingStream
        );
        assert_eq!(
            stage_key_from_name(Some("video stream initialization")),
            StageKey::StartingVideo
        );
        assert_eq!(
            stage_key_from_name(Some("audio stream establishment")),
            StageKey::StartingAudio
        );
        assert_eq!(
            stage_key_from_name(Some("input stream establishment")),
            StageKey::ConnectingInput
        );
        assert_eq!(stage_key_from_name(Some("Celeste")), StageKey::StartingApp);
        assert_eq!(stage_key_from_name(None), StageKey::StartingApp);
    }

    #[test]
    fn termination_codes_classify_correctly() {
        assert_eq!(classify_termination(0), TerminationTextKey::Graceful);
        assert_eq!(
            classify_termination(-100),
            TerminationTextKey::NoVideoTraffic
        );
        assert_eq!(classify_termination(-101), TerminationTextKey::NoVideoFrame);
        assert_eq!(
            classify_termination(-102),
            TerminationTextKey::UnexpectedEarly
        );
        assert_eq!(
            classify_termination(-103),
            TerminationTextKey::ProtectedContent
        );
        assert_eq!(
            classify_termination(-104),
            TerminationTextKey::FrameConversion
        );
        assert_eq!(classify_termination(-105), TerminationTextKey::Disconnected);
        assert_eq!(
            classify_termination(-106),
            TerminationTextKey::VulkanRenderer
        );
        assert_eq!(classify_termination(-107), TerminationTextKey::Decoder);
        assert_eq!(classify_termination(-108), TerminationTextKey::HostGone);
        assert_eq!(
            classify_termination(503),
            TerminationTextKey::Unexpected(503)
        );
    }

    #[test]
    fn hex_error_formatting_matches_android() {
        assert!(!is_hex_error_code(-100));
        assert!(!is_hex_error_code(1000));
        assert!(is_hex_error_code(1001));
        assert!(is_hex_error_code(0x8000_4005_u32 as i32));
        assert_eq!(format_error_code(-105), "-105");
        assert_eq!(format_error_code(0x8000_4005_u32 as i32), "0x80004005");
    }

    #[test]
    fn launch_and_startup_failures_classify_deterministically() {
        assert_eq!(
            classify_launch_failure(true, 408, "ignored"),
            LaunchFailureKind::LocalOverride
        );
        assert_eq!(
            classify_launch_failure(false, 403, "Device disabled"),
            LaunchFailureKind::PermissionDenied { has_reason: true }
        );
        assert_eq!(
            classify_launch_failure(false, 401, "  "),
            LaunchFailureKind::PermissionDenied { has_reason: false }
        );
        assert_eq!(
            classify_launch_failure(false, 503, "Encoder failed"),
            LaunchFailureKind::HostMessage { code: 503 }
        );
        assert_eq!(
            classify_launch_failure(false, 500, "   "),
            LaunchFailureKind::HostError { code: 500 }
        );

        assert_eq!(
            classify_startup_failure(true, Some("Video stream initialization"), true, 500),
            StartupFailureKind::DecoderFailure
        );
        assert_eq!(
            classify_startup_failure(true, Some("RTSP handshake"), true, 500),
            StartupFailureKind::ConnectionMessage
        );
        assert_eq!(
            classify_startup_failure(false, Some("RTSP handshake"), false, 0),
            StartupFailureKind::ConnectionFailed
        );
        assert_eq!(
            classify_startup_failure(false, Some("RTSP handshake"), false, 400),
            StartupFailureKind::HostError(400)
        );
    }

    #[test]
    fn start_cause_omits_finding_pc() {
        assert_eq!(classify_start_cause("name resolution"), None);
        assert_eq!(classify_start_cause("Platform initialization"), None);
        assert_eq!(
            classify_start_cause("RTSP handshake"),
            Some(StartCauseKey::DidNotFinish)
        );
    }
}
