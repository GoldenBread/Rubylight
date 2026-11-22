//! Session action execution and permission checks shared by `couch_menu`, `native_api`, and
//! `reconnect_tests`, decoupled from `axum` response types.

use crate::state::Shared;
use butterpollo_session::{
    native::{self, LAUNCH, VIEW},
    session_state::Refusal,
    state::Client,
};
use rubylight_native_api::error::ErrorCode;

/// How soon a client retries after `session_busy`.
const BUSY_RETRY_MS: u64 = 1000;

/// A refused or failed request, answered with the error body of the native API spec.
#[derive(Debug)]
pub struct Failure {
    pub code: ErrorCode,
    pub message: String,
    pub setting: Option<String>,
    pub retry_after_ms: Option<u64>,
}

impl Failure {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            setting: None,
            retry_after_ms: None,
        }
    }

    pub fn internal(what: &str, error: impl std::fmt::Display) -> Self {
        tracing::error!(%error, "native API: {what}");
        Self::new(ErrorCode::Internal, format!("The host failed to {what}."))
    }

    pub fn code(&self) -> ErrorCode {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    /// Name the setting the error is about.
    pub fn for_setting(mut self, id: &str) -> Self {
        self.setting = Some(id.to_owned());
        self
    }

    pub fn retry_after(mut self, milliseconds: u64) -> Self {
        self.retry_after_ms = Some(milliseconds);
        self
    }
}

impl From<crate::continue_here::Refused> for Failure {
    fn from(refused: crate::continue_here::Refused) -> Self {
        match refused {
            crate::continue_here::Refused::Session(refusal) => refusal.into(),
            crate::continue_here::Refused::GuestsWatching => Self::new(
                ErrorCode::HostInUse,
                "Guests are watching the other device's stream.",
            ),
            crate::continue_here::Refused::Launch(message) => {
                Self::new(ErrorCode::HostInUse, message)
            }
        }
    }
}

impl From<Refusal> for Failure {
    fn from(refusal: Refusal) -> Self {
        match refusal {
            Refusal::NoSession => Self::new(ErrorCode::NoSession, "There is no session."),
            Refusal::Busy => Self::new(
                ErrorCode::SessionBusy,
                "A launch or a stop is in progress. Try again in a moment.",
            )
            .retry_after(BUSY_RETRY_MS),
            Refusal::NotOwner => Self::new(
                ErrorCode::NotSessionOwner,
                "This session belongs to another device.",
            ),
        }
    }
}

pub fn permission_required(name: &str) -> Failure {
    Failure::new(
        ErrorCode::PermissionRequired,
        format!("Turn on {name} for this device in the Rubylight console."),
    )
}

pub fn require(client: &Client, bits: u32, name: &str) -> Result<(), Failure> {
    client
        .allows(bits)
        .then_some(())
        .ok_or_else(|| permission_required(name))
}

/// View streams or Launch applications: what seeing and leaving a session needs.
pub fn require_view_or_launch(client: &Client) -> Result<(), Failure> {
    require(client, VIEW, "View streams")
        .or_else(|_| require(client, LAUNCH, "Launch applications"))
}

pub fn require_host_control(client: &Client) -> Result<(), Failure> {
    native::host_control(client).then_some(()).ok_or_else(|| {
        Failure::new(
            ErrorCode::HostControlRequired,
            "Turn on Host control for this device in the Rubylight console.",
        )
    })
}

#[derive(Clone, Copy, Debug)]
pub enum Action {
    Disconnect,
    End,
    CloseGame,
}

/// Checks `client`'s permissions, then runs the action. The native API endpoints
/// and the couch menu's rows both run session actions through here.
pub fn session_action(h: &Shared, client: &Client, action: Action) -> Result<(), Failure> {
    match action {
        Action::Disconnect => require_view_or_launch(client)?,
        Action::End => require(client, LAUNCH, "Launch applications")?,
        Action::CloseGame => {
            require(client, LAUNCH, "Launch applications")?;
            require_host_control(client)?;
        }
    }
    let device = client.uuid.as_str();
    let disconnect = matches!(action, Action::Disconnect);
    let stopped_launch = match action {
        Action::Disconnect => false,
        Action::End => h.stop_launch(|launching| launching == device),
        Action::CloseGame => {
            h.stop_launch(|launching| crate::nvhttp::same_device(launching, device))
        }
    };
    if crate::nvhttp::quit_nucleus_stream(h, device, disconnect)? {
        return Ok(());
    }
    let done = match action {
        Action::Disconnect => h.disconnect_session(Some(device)),
        Action::End => h.end_session(Some(device)),
        Action::CloseGame => h.close_game(device),
    };
    match done {
        Err(Refusal::NoSession) if stopped_launch => Ok(()),
        Err(Refusal::NotOwner)
            if device.ends_with(butterpollo_session::state::SECOND_SCREEN_SUFFIX)
                && !matches!(action, Action::CloseGame) =>
        {
            end_second_screen(h, device, matches!(action, Action::End));
            Ok(())
        }
        done => done.map(drop).map_err(Failure::from),
    }
}

fn end_second_screen(h: &Shared, screen: &str, end: bool) {
    let canceled = h.sessions.lock().unwrap().request_stop(Some(screen));
    drop(canceled);
    crate::remote_display::disconnect(h, Some(screen));
    if end {
        let display = h.app_display.lock().unwrap().remove(screen);
        drop(display);
    }
}
