//! Shared stream connect, launch, negotiation, preflight, post-connect action,
//! and lifecycle orchestration used by both Desktop (`desktop-media`) and
//! Android (`nightjar-android`).

pub mod actions;
pub mod launch;
pub mod lifecycle;
pub mod negotiate;
pub mod preflight;

use crate::http;
use crate::session::Stage;
use nightjar_policy::connect_text::{self, HttpCauseKey, StageKey, StartCauseKey};

/// Maps a session [`Stage`] to its pure policy [`StageKey`].
pub fn stage_key(stage: Stage) -> StageKey {
    connect_text::stage_key_from_name(Some(stage.name()))
}

/// Maps a retryable HTTP I/O error into its plain-word [`HttpCauseKey`].
pub fn http_cause_key(error: &http::Error) -> Option<HttpCauseKey> {
    use std::io::ErrorKind;
    let http::Error::Io(io_err) = error else {
        return None;
    };
    match io_err.kind() {
        ErrorKind::TimedOut => Some(HttpCauseKey::TimedOut),
        ErrorKind::ConnectionRefused => Some(HttpCauseKey::ConnectionRefused),
        ErrorKind::ConnectionReset
        | ErrorKind::ConnectionAborted
        | ErrorKind::NotConnected
        | ErrorKind::BrokenPipe
        | ErrorKind::UnexpectedEof => Some(HttpCauseKey::ConnectionLost),
        _ => None,
    }
}

/// Maps a failed start [`Stage`] to its plain-word [`StartCauseKey`].
pub fn start_cause_key(stage: Stage) -> Option<StartCauseKey> {
    connect_text::classify_start_cause(stage.name())
}

/// Client-local launch stop reason (not a host status error).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalLaunchFailure {
    RecoveryOnly,
    SessionChanged,
    BrowserStream,
    ConfirmRejected,
    ConfirmTimedOut,
    ConfirmCancelled,
}

impl LocalLaunchFailure {
    /// HTTP-equivalent status code used for recovery classification.
    pub const fn code(self) -> i32 {
        match self {
            Self::RecoveryOnly | Self::SessionChanged | Self::ConfirmRejected => 409,
            Self::BrowserStream => 410,
            Self::ConfirmTimedOut => 408,
            Self::ConfirmCancelled => 499,
        }
    }

    pub const fn message(self) -> &'static str {
        match self {
            Self::RecoveryOnly => "Recovery can only resume the previous session.",
            Self::SessionChanged => "The previous session is no longer available.",
            Self::BrowserStream => {
                "Another device is playing this game in a browser, and this stream can’t run beside it. End that stream first."
            }
            Self::ConfirmRejected => {
                "The host session changed or confirmation expired. Start the tile again."
            }
            Self::ConfirmTimedOut => "Confirmation timed out. Start the tile again to retry.",
            Self::ConfirmCancelled => "Launch cancelled.",
        }
    }
}

/// Structured connection/launch failure shared across platforms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectError {
    pub message: String,
    pub retryable: bool,
    pub asks_replace: bool,
    pub moved_to: Option<String>,
    pub packed_refused: bool,
    pub http_cause: Option<HttpCauseKey>,
    pub start_cause: Option<StartCauseKey>,
    pub http_code: Option<i32>,
    pub local_failure: Option<LocalLaunchFailure>,
    pub session_ended: bool,
}

impl ConnectError {
    pub fn fatal(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
            asks_replace: false,
            moved_to: None,
            packed_refused: false,
            http_cause: None,
            start_cause: None,
            http_code: None,
            local_failure: None,
            session_ended: false,
        }
    }

    pub fn local(kind: LocalLaunchFailure) -> Self {
        Self {
            message: kind.message().to_owned(),
            retryable: false,
            asks_replace: false,
            moved_to: None,
            packed_refused: false,
            http_cause: None,
            start_cause: None,
            http_code: Some(kind.code()),
            local_failure: Some(kind),
            session_ended: false,
        }
    }
}

impl From<String> for ConnectError {
    fn from(message: String) -> Self {
        Self::fatal(message)
    }
}

impl From<&str> for ConnectError {
    fn from(message: &str) -> Self {
        Self::fatal(message)
    }
}
