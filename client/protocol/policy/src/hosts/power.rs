//! Pure Sleep PC, Restart/Shut down PC, and Host Update state machines, timing constants,
//! and classification helpers shared across Nightjar Android and Desktop.

use serde::{Deserialize, Serialize};

use crate::http::{Availability, Feature, HostCapabilities, HostUpdate, Reason};

/// A sleep/power token older than this is refreshed before spending (hosts keep tokens for 30 s).
pub const SLEEP_TOKEN_REFRESH_MS: u64 = 25_000;
/// Interval between post-202 `GET /power` refusal polls.
pub const PC_POWER_REFUSAL_POLL_MS: u64 = 2_000;
/// Maximum number of post-202 `GET /power` refusal polls (~20 s total).
pub const PC_POWER_REFUSAL_READS: usize = 10;
/// Active wait budget for host update completion (excluding time waiting on another stream).
pub const UPDATE_WAIT_MS: u64 = 15 * 60_000;
/// Maximum total wait budget for host update completion (including waiting on another stream).
pub const UPDATE_MAX_WAIT_MS: u64 = 3 * 60 * 60_000;
/// Poll interval while following a host update.
pub const UPDATE_POLL_MS: u64 = 5_000;

/// Two-clock timestamp captured before requesting a single-use sleep/power token.
///
/// Using the maximum elapsed time across monotonic and wall clocks prevents stale tokens from
/// looking fresh across OS suspend/resume or backward wall-clock jumps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenStamp {
    pub mono_ms: u64,
    pub wall_ms: u64,
}

impl TokenStamp {
    pub fn age_ms(&self, now_mono_ms: u64, now_wall_ms: u64) -> u64 {
        let mono_elapsed = now_mono_ms.saturating_sub(self.mono_ms);
        let wall_elapsed = now_wall_ms.saturating_sub(self.wall_ms);
        mono_elapsed.max(wall_elapsed)
    }

    pub fn is_stale(&self, now_mono_ms: u64, now_wall_ms: u64) -> bool {
        self.age_ms(now_mono_ms, now_wall_ms) >= SLEEP_TOKEN_REFRESH_MS
    }
}

/// Why a host answered `/power-actions/v1` without a sleep token.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SleepUnavailableReason {
    Unsupported,
    NotAllowed,
    Busy,
}

/// Classifies an offer without a token according to `sleep_supported` and `sleep_allowed`.
pub fn classify_sleep_unavailable(
    sleep_supported: bool,
    sleep_allowed: bool,
) -> SleepUnavailableReason {
    if !sleep_supported {
        SleepUnavailableReason::Unsupported
    } else if !sleep_allowed {
        SleepUnavailableReason::NotAllowed
    } else {
        SleepUnavailableReason::Busy
    }
}

/// Whether Sleep PC should be offered in a host menu for an online paired host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SleepOfferState {
    Hidden,
    Offered,
    PermissionDenied,
}

/// Evaluates Sleep PC menu availability from host reachability, pairing state, and capabilities.
pub fn sleep_option(
    online: bool,
    paired: bool,
    capabilities: Option<&HostCapabilities>,
) -> SleepOfferState {
    if !online || !paired {
        return SleepOfferState::Hidden;
    }
    let Some(caps) = capabilities else {
        return SleepOfferState::Hidden;
    };
    match caps.availability(Feature::Sleep) {
        Availability::Supported => SleepOfferState::Offered,
        Availability::Unsupported(Some(Reason::SleepPermission)) => {
            SleepOfferState::PermissionDenied
        }
        _ => SleepOfferState::Hidden,
    }
}

/// Typed classification for sleep/power transport or API failures.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PowerFailureKind {
    NetworkOrTls,
    NotPaired,
    Refused { status: u16, message: String },
    Api { status: u16, message: String },
    Other(String),
}

/// Step outcomes for the Sleep PC state machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SleepStepAction {
    Offer { token: String, stamp: TokenStamp },
    Accepted,
    Unavailable(SleepUnavailableReason),
    Refused { status: u16, message: String },
    Failed(PowerFailureKind),
}

/// Step outcomes for the Restart / Shut down PC state machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PcPowerStepAction {
    Offer {
        token: String,
        stamp: TokenStamp,
        wake_from_off: bool,
    },
    Accepted,
    Unavailable(String),
    RefusedLater {
        message: String,
        unconfirmed: bool,
    },
    Failed(PowerFailureKind),
}

/// Error returned when `POST /update/install` offers no target version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UpdateTargetError;

/// Extracts the target version from a `HostUpdate` reply, or [`UpdateTargetError`] if none is offered.
pub fn update_target(update: &HostUpdate) -> Result<String, UpdateTargetError> {
    update
        .offered()
        .map(str::to_owned)
        .ok_or(UpdateTargetError)
}

/// Step result produced by [`UpdateTracker::step`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum UpdateStepAction {
    Continue,
    Waiting { reason: String },
    Back { version: String },
    UpdateFailed { error: Option<String> },
    CameBackOn { current_version: String, error: Option<String> },
    TimedOut { target: String },
}

/// Pure step-by-step state machine for following a host update install.
///
/// Mirrors both Desktop `follow_update` and Android `HostUpdateFlow.follow`:
/// - `polls_limit`: active non-waiting polls (`WAIT_MS / POLL_MS = 180`)
/// - `max_polls`: total polls including waiting on another device (`MAX_WAIT_MS / POLL_MS = 2160`)
/// - `gone`: set once the host stops answering while installing; prevents false `CameBackOn`
///   before the host actually restarted or finished.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateTracker {
    pub target: String,
    pub polls_limit: u32,
    pub max_polls: u32,
    pub total_polls: u32,
    pub counted: u32,
    pub gone: bool,
    pub waiting: Option<String>,
}

impl UpdateTracker {
    pub fn new(target: impl Into<String>) -> Self {
        Self::with_limits(
            target,
            (UPDATE_WAIT_MS / UPDATE_POLL_MS) as u32,
            (UPDATE_MAX_WAIT_MS / UPDATE_POLL_MS) as u32,
        )
    }

    pub fn with_limits(target: impl Into<String>, polls_limit: u32, max_polls: u32) -> Self {
        Self {
            target: target.into(),
            polls_limit,
            max_polls,
            total_polls: 0,
            counted: 0,
            gone: false,
            waiting: None,
        }
    }

    /// Whether another poll iteration may run before timing out.
    pub fn can_poll(&self) -> bool {
        self.counted < self.polls_limit && self.total_polls < self.max_polls
    }

    /// Advances the tracker for one poll attempt.
    ///
    /// Pass `running_version` (from `/serverinfo`, if available) and `update` (`Some` when
    /// `GET /update` succeeded, `None` when the host was unreachable).
    pub fn step(
        &mut self,
        running_version: Option<&str>,
        update: Option<&HostUpdate>,
    ) -> UpdateStepAction {
        if !self.can_poll() {
            return UpdateStepAction::TimedOut {
                target: self.target.clone(),
            };
        }
        self.total_polls += 1;

        if running_version
            .is_some_and(|v| crate::http::update_arrived(v, &self.target))
        {
            return UpdateStepAction::Back {
                version: self.target.clone(),
            };
        }

        let Some(update) = update else {
            self.gone = true;
            self.counted += 1;
            return if self.can_poll() {
                UpdateStepAction::Continue
            } else {
                UpdateStepAction::TimedOut {
                    target: self.target.clone(),
                }
            };
        };

        if crate::http::update_arrived(&update.current_version, &self.target) {
            return UpdateStepAction::Back {
                version: self.target.clone(),
            };
        }

        if let Some(failure) = update.failure() {
            return UpdateStepAction::UpdateFailed {
                error: failure.map(str::to_owned),
            };
        }

        if self.gone && update.gave_up(&self.target) {
            return UpdateStepAction::CameBackOn {
                current_version: update.current_version.clone(),
                error: update.error.clone(),
            };
        }

        let mut newly_waiting = None;
        if update.waiting_for != self.waiting {
            self.waiting = update.waiting_for.clone();
            if let Some(reason) = &self.waiting {
                newly_waiting = Some(reason.clone());
            }
        }
        if self.waiting.is_none() {
            self.counted += 1;
        }
        if let Some(reason) = newly_waiting {
            return UpdateStepAction::Waiting { reason };
        }
        if !self.can_poll() {
            return UpdateStepAction::TimedOut {
                target: self.target.clone(),
            };
        }
        UpdateStepAction::Continue
    }
}
