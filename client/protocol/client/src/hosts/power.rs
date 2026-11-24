//! Unified Sleep PC, Restart/Shut down PC, and Host Update flows over [`HostClient`] and
//! [`nightjar_policy::hosts::power`].

use std::time::{Duration, Instant, SystemTime};

use nightjar_policy::hosts::{
    PcPowerStepAction, PowerFailureKind, SLEEP_TOKEN_REFRESH_MS, SleepStepAction, TokenStamp,
    UpdateStepAction, UpdateTargetError, UpdateTracker, classify_sleep_unavailable,
    update_target as policy_update_target,
};

use crate::http::{
    Error, HostClient, HostUpdate, LateRefusal, PcPower, PcPowerOffer, PowerError, PowerReply,
    host_words,
};

/// Single-use token timestamp backed by both monotonic [`Instant`] and wall [`SystemTime`].
///
/// Matches Desktop and Android: monotonic clocks pause during host sleep while wall clocks may jump
/// backward, so [`Self::age`] takes the larger elapsed duration of the two.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Handed {
    pub at: Instant,
    pub wall: SystemTime,
}

impl Handed {
    pub fn now() -> Self {
        Self {
            at: Instant::now(),
            wall: SystemTime::now(),
        }
    }

    pub fn from_instant(at: Instant) -> Self {
        let elapsed = at.elapsed();
        let wall = SystemTime::now()
            .checked_sub(elapsed)
            .unwrap_or_else(SystemTime::now);
        Self { at, wall }
    }

    pub fn age(&self) -> Duration {
        self.at.elapsed().max(self.wall.elapsed().unwrap_or_default())
    }

    pub fn is_stale(&self) -> bool {
        self.age() >= Duration::from_millis(SLEEP_TOKEN_REFRESH_MS)
    }

    /// Exports a pure policy [`TokenStamp`] relative to `mono_now_ms` / `wall_now_ms`.
    pub fn to_stamp(&self, mono_now_ms: u64, wall_now_ms: u64) -> TokenStamp {
        let mono_elapsed = self.at.elapsed().as_millis() as u64;
        let wall_elapsed = self.wall.elapsed().unwrap_or_default().as_millis() as u64;
        TokenStamp {
            mono_ms: mono_now_ms.saturating_sub(mono_elapsed),
            wall_ms: wall_now_ms.saturating_sub(wall_elapsed),
        }
    }
}

/// Abstract `/power-actions/v1` transport used by [`sleep_token`] and [`sleep_confirmed`].
pub trait PowerHost {
    fn power_capabilities(&self) -> Result<PowerReply, PowerError>;
    fn sleep(&self, token: &str) -> Result<PowerReply, PowerError>;
}

impl PowerHost for HostClient {
    fn power_capabilities(&self) -> Result<PowerReply, PowerError> {
        HostClient::power_capabilities(self)
    }

    fn sleep(&self, token: &str) -> Result<PowerReply, PowerError> {
        HostClient::sleep(self, token)
    }
}

/// Abstract `/power` + `/power/{restart,shutdown}` transport used by PC power flows.
pub trait PcPowerHost {
    fn pc_power(&self) -> Result<PcPowerOffer, Error>;
    fn power_pc(&self, action: PcPower, token: &str) -> Result<(), Error>;
}

impl PcPowerHost for HostClient {
    fn pc_power(&self) -> Result<PcPowerOffer, Error> {
        HostClient::pc_power(self)
    }

    fn power_pc(&self, action: PcPower, token: &str) -> Result<(), Error> {
        HostClient::power_pc(self, action, token)
    }
}

/// Abstract `/update` + `/serverinfo` version transport used by [`follow_update`].
pub trait UpdateHost {
    fn host_update(&self) -> Result<HostUpdate, Error>;
    fn running_version(&self) -> Option<String>;
}

impl UpdateHost for HostClient {
    fn host_update(&self) -> Result<HostUpdate, Error> {
        HostClient::host_update(self)
    }

    fn running_version(&self) -> Option<String> {
        self.server_info(false)
            .ok()
            .and_then(|info| info.capabilities.host_version().map(str::to_owned))
    }
}

/// Structured sleep failure returned before localized UI string formatting.
#[derive(Debug)]
pub enum SleepFailure {
    Unavailable(nightjar_policy::hosts::SleepUnavailableReason),
    Refused { status: u16, message: String },
    Transport(Error),
}

impl SleepFailure {
    pub fn into_action(self) -> SleepStepAction {
        match self {
            Self::Unavailable(r) => SleepStepAction::Unavailable(r),
            Self::Refused { status, message } => SleepStepAction::Refused { status, message },
            Self::Transport(err) => SleepStepAction::Failed(classify_http_power_error(&err)),
        }
    }
}

fn classify_power_error(err: PowerError) -> SleepFailure {
    match err {
        PowerError::Refused { reply, .. } => SleepFailure::Refused {
            status: reply.status,
            message: reply.message,
        },
        PowerError::Host(err) => SleepFailure::Transport(err),
    }
}

fn classify_http_power_error(err: &Error) -> PowerFailureKind {
    match err {
        Error::NotPaired => PowerFailureKind::NotPaired,
        Error::Api { status, error } => PowerFailureKind::Api {
            status: *status,
            message: error.message.clone(),
        },
        Error::Status { code, message } => PowerFailureKind::Api {
            status: *code as u16,
            message: message.clone(),
        },
        Error::Io(_) => PowerFailureKind::NetworkOrTls,
        other => PowerFailureKind::Other(other.to_string()),
    }
}

/// Requests a sleep token, capturing [`Handed::now`] before network I/O.
pub fn sleep_token(host: &impl PowerHost) -> Result<(String, Handed), SleepFailure> {
    let at = Handed::now();
    let reply = host.power_capabilities().map_err(classify_power_error)?;
    match reply.token {
        Some(token) => Ok((token, at)),
        None => Err(SleepFailure::Unavailable(classify_sleep_unavailable(
            reply.sleep_supported,
            reply.sleep_allowed,
        ))),
    }
}

/// Spends a confirmed sleep token, refreshing first when older than [`SLEEP_TOKEN_REFRESH_MS`]
/// using explicit monotonic and wall-clock timestamps.
pub fn sleep_confirmed_at(
    host: &impl PowerHost,
    token: String,
    stamp: TokenStamp,
    mono_now_ms: u64,
    wall_now_ms: u64,
) -> Result<(), SleepFailure> {
    let token = if stamp.is_stale(mono_now_ms, wall_now_ms) {
        sleep_token(host)?.0
    } else {
        token
    };
    host.sleep(&token).map_err(classify_power_error)?;
    Ok(())
}

/// Spends a confirmed sleep token, refreshing first when older than [`SLEEP_TOKEN_REFRESH_MS`].
pub fn sleep_confirmed(
    host: &impl PowerHost,
    token: String,
    at: Handed,
) -> Result<(), SleepFailure> {
    let token = if at.is_stale() {
        sleep_token(host)?.0
    } else {
        token
    };
    host.sleep(&token).map_err(classify_power_error)?;
    Ok(())
}

/// Structured PC power failure returned before localized UI string formatting.
#[derive(Debug)]
pub enum PcPowerFailure {
    Unavailable(String),
    Transport(Error),
}

impl PcPowerFailure {
    /// Extracts the host's own words when present (`Err(Error::Api)`), or `Ok(err)` for generic description.
    pub fn host_words(&self) -> Option<&str> {
        match self {
            Self::Unavailable(msg) if !msg.is_empty() => Some(msg.as_str()),
            _ => None,
        }
    }
}

/// Asks the PC for a `PcPower` (`Restart` / `ShutDown`) token, capturing [`Handed::now`] first.
pub fn pc_power_token(
    host: &impl PcPowerHost,
    action: PcPower,
) -> Result<(String, Handed, bool), PcPowerFailure> {
    let at = Handed::now();
    let offer = host.pc_power().map_err(|e| {
        host_words(&e)
            .map(PcPowerFailure::Unavailable)
            .unwrap_or(PcPowerFailure::Transport(e))
    })?;
    match offer.token_for(action) {
        Ok(token) => Ok((token.to_owned(), at, offer.wake_from_off)),
        Err(reason) => Err(PcPowerFailure::Unavailable(reason)),
    }
}

/// Spends a confirmed PC power token, refreshing first when older than [`SLEEP_TOKEN_REFRESH_MS`]
/// using explicit monotonic and wall-clock timestamps.
pub fn pc_power_confirmed_at(
    host: &impl PcPowerHost,
    action: PcPower,
    token: String,
    stamp: TokenStamp,
    mono_now_ms: u64,
    wall_now_ms: u64,
) -> Result<PcPowerStepAction, PcPowerFailure> {
    let token = if stamp.is_stale(mono_now_ms, wall_now_ms) {
        pc_power_token(host, action)?.0
    } else {
        token
    };
    host.power_pc(action, &token).map_err(|e| {
        host_words(&e)
            .map(PcPowerFailure::Unavailable)
            .unwrap_or(PcPowerFailure::Transport(e))
    })?;
    Ok(PcPowerStepAction::Accepted)
}

/// Spends a confirmed PC power token, refreshing first when older than [`SLEEP_TOKEN_REFRESH_MS`].
pub fn pc_power_confirmed(
    host: &impl PcPowerHost,
    action: PcPower,
    token: String,
    at: Handed,
) -> Result<PcPowerStepAction, PcPowerFailure> {
    let token = if at.is_stale() {
        pc_power_token(host, action)?.0
    } else {
        token
    };
    host.power_pc(action, &token).map_err(|e| {
        host_words(&e)
            .map(PcPowerFailure::Unavailable)
            .unwrap_or(PcPowerFailure::Transport(e))
    })?;
    Ok(PcPowerStepAction::Accepted)
}

/// Polls `GET /power` up to `reads` times after HTTP 202 for a delayed refusal or unconfirmed note.
pub fn pc_power_refusal(
    host: &impl PcPowerHost,
    reads: u32,
    pause: impl Fn(),
) -> Option<LateRefusal> {
    for _ in 0..reads {
        pause();
        match host.pc_power() {
            Ok(offer) => {
                if let Some(refusal) = offer.late_refusal() {
                    return Some(refusal);
                }
            }
            Err(_) => return None,
        }
    }
    None
}

/// Extracts the offered target version for a host update.
pub fn update_target(update: &HostUpdate) -> Result<String, UpdateTargetError> {
    policy_update_target(update)
}

/// Follows an accepted host update install using [`UpdateTracker`].
pub fn follow_update(
    host: &impl UpdateHost,
    target: &str,
    polls_limit: u32,
    max_polls: u32,
    pause: impl Fn(),
    mut on_waiting: impl FnMut(String),
) -> UpdateStepAction {
    let mut tracker = UpdateTracker::with_limits(target, polls_limit, max_polls);
    while tracker.can_poll() {
        pause();
        let running = host.running_version();
        let update = if running
            .as_deref()
            .is_some_and(|v| nightjar_policy::http::update_arrived(v, target))
        {
            None
        } else {
            host.host_update().ok()
        };
        match tracker.step(running.as_deref(), update.as_ref()) {
            UpdateStepAction::Continue => {}
            UpdateStepAction::Waiting { reason } => on_waiting(reason),
            terminal => return terminal,
        }
    }
    UpdateStepAction::TimedOut {
        target: target.to_owned(),
    }
}

/// Follows an accepted host update install using [`UpdateTracker`], stopping early if
/// `pause()` returns `false` or `cancelled()` returns `true`.
pub fn follow_update_cancellable(
    host: &impl UpdateHost,
    target: &str,
    polls_limit: u32,
    max_polls: u32,
    pause: impl Fn() -> bool,
    cancelled: impl Fn() -> bool,
    mut on_waiting: impl FnMut(String),
) -> Option<UpdateStepAction> {
    let mut tracker = UpdateTracker::with_limits(target, polls_limit, max_polls);
    while tracker.can_poll() {
        if cancelled() || !pause() || cancelled() {
            return None;
        }
        let running = host.running_version();
        let update = if running
            .as_deref()
            .is_some_and(|v| nightjar_policy::http::update_arrived(v, target))
        {
            None
        } else {
            host.host_update().ok()
        };
        match tracker.step(running.as_deref(), update.as_ref()) {
            UpdateStepAction::Continue => {}
            UpdateStepAction::Waiting { reason } => on_waiting(reason),
            terminal => return Some(terminal),
        }
    }
    Some(UpdateStepAction::TimedOut {
        target: target.to_owned(),
    })
}
