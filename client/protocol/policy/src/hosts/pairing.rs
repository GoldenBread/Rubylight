//! Pure pairing failure classification, unpair classification, BiDi control sanitization, and
//! pairing-request watch tracker shared across Nightjar Android and Desktop.

use serde::{Deserialize, Serialize};

/// Minimum interval between `GET /pairing/requests` polls on Home.
pub const PAIRING_WATCH_INTERVAL_MS: u64 = 4_000;

/// Strips BiDi directional controls (marks, embeddings, overrides, isolates, and Unicode line/
/// paragraph separators) while keeping zero-width joiners (`\u{200D}`) needed by emoji.
pub fn strip_bidi_controls(input: &str) -> String {
    input
        .chars()
        .filter(|&c| {
            !matches!(
                c,
                '\u{061C}'
                    | '\u{200E}'
                    | '\u{200F}'
                    | '\u{2028}'..='\u{202E}'
                    | '\u{2066}'..='\u{2069}'
            )
        })
        .collect()
}

/// Whether an HTTP status code represents a host refusal (4xx) rather than a server/network error.
pub fn pairing_refused_status(status: u16) -> bool {
    (400..500).contains(&status)
}

/// Whether a host is already paired and requires no new PIN handshake.
///
/// Pairing links re-run pairing when no certificate is pinned yet so the link's SHA-256 fingerprint
/// can be verified before pinning.
pub fn already_paired(is_paired: bool, has_link: bool, has_pinned_cert: bool) -> bool {
    is_paired && (!has_link || has_pinned_cert)
}

/// Input pairing state from the pairing engine (`PairingManager.PairState` / `PairResult`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PairFailureState {
    NotPaired,
    Paired,
    PinWrong,
    Failed,
    AlreadyInProgress,
    CertMismatch,
    ServerUnverified,
}

/// Typed pairing failure category mapped to localized strings by each client UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PairFailureKind {
    PinWrong,
    OneTimePinWrong,
    LinkPinWrong,
    AlreadyInProgress,
    ServerUnverified,
    InGame,
    Declined,
    LinkCertMismatch,
    Network,
    Refused,
    Failed,
}

/// Classifies a completed pairing handshake failure.
pub fn classify_pairing_failure(
    state: PairFailureState,
    from_link: bool,
    one_time: bool,
    in_game: bool,
) -> PairFailureKind {
    match state {
        PairFailureState::PinWrong => {
            if from_link {
                PairFailureKind::LinkPinWrong
            } else if one_time {
                PairFailureKind::OneTimePinWrong
            } else {
                PairFailureKind::PinWrong
            }
        }
        PairFailureState::AlreadyInProgress => PairFailureKind::AlreadyInProgress,
        PairFailureState::ServerUnverified => PairFailureKind::ServerUnverified,
        PairFailureState::CertMismatch => PairFailureKind::LinkCertMismatch,
        _ => {
            if in_game {
                PairFailureKind::InGame
            } else {
                PairFailureKind::Failed
            }
        }
    }
}

/// Classifies an HTTP 4xx refusal during pairing.
pub fn classify_pairing_http_refusal(from_link: bool, one_time: bool) -> PairFailureKind {
    if from_link {
        PairFailureKind::LinkPinWrong
    } else if one_time {
        PairFailureKind::OneTimePinWrong
    } else {
        PairFailureKind::Failed
    }
}

/// Typed unpair failure category.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnpairFailureKind {
    NotPaired,
    UnknownHost,
    NotFound,
    Failed,
}

/// Classifies an unpair error outcome.
pub fn classify_unpair(
    status: Option<u16>,
    is_not_paired: bool,
    is_unknown_host: bool,
    is_not_found: bool,
) -> UnpairFailureKind {
    if is_not_paired || status == Some(401) {
        UnpairFailureKind::NotPaired
    } else if is_unknown_host {
        UnpairFailureKind::UnknownHost
    } else if is_not_found || status == Some(404) {
        UnpairFailureKind::NotFound
    } else {
        UnpairFailureKind::Failed
    }
}

/// Decision returned by [`PairingWatchTracker::begin_poll`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairingWatchCommand {
    Skip,
    PublishEmpty,
    StartPoll { generation: u64 },
}

/// Decision returned by [`PairingWatchTracker::finish_poll`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairingWatchFinish {
    Stale = 0,
    Unchanged = 1,
    Changed = 2,
}

/// Clock-free watch state machine for polling waiting pairing approvals on Home.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairingWatchTracker {
    generation: u64,
    in_flight: bool,
    last_poll_ms: Option<u64>,
    stale: bool,
    previous_ids: Vec<String>,
}

impl Default for PairingWatchTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl PairingWatchTracker {
    pub fn new() -> Self {
        Self {
            generation: 0,
            in_flight: false,
            last_poll_ms: None,
            stale: true,
            previous_ids: Vec::new(),
        }
    }

    /// Requests an immediate poll on the next `begin_poll` call.
    pub fn refresh_soon(&mut self) {
        self.stale = true;
    }

    /// Leaves the screen: increments generation to drop in-flight replies and marks stale.
    pub fn pause(&mut self) {
        self.generation += 1;
        self.in_flight = false;
        self.stale = true;
    }

    /// Evaluates whether a poll should run at `now_ms`.
    pub fn begin_poll(&mut self, now_ms: u64, has_asked_hosts: bool) -> PairingWatchCommand {
        if !has_asked_hosts {
            self.generation += 1;
            self.in_flight = false;
            let changed = !self.previous_ids.is_empty();
            self.previous_ids.clear();
            return if changed {
                PairingWatchCommand::PublishEmpty
            } else {
                PairingWatchCommand::Skip
            };
        }
        if self.in_flight
            || (!self.stale
                && self
                    .last_poll_ms
                    .is_some_and(|last| now_ms.saturating_sub(last) < PAIRING_WATCH_INTERVAL_MS))
        {
            return PairingWatchCommand::Skip;
        }
        self.in_flight = true;
        self.stale = false;
        self.last_poll_ms = Some(now_ms);
        self.generation += 1;
        PairingWatchCommand::StartPoll {
            generation: self.generation,
        }
    }

    /// Commits a completed poll's request IDs if `generation` matches the current one.
    pub fn finish_poll(&mut self, generation: u64, ids: Vec<String>) -> PairingWatchFinish {
        if generation != self.generation {
            return PairingWatchFinish::Stale;
        }
        self.in_flight = false;
        if ids == self.previous_ids {
            PairingWatchFinish::Unchanged
        } else {
            self.previous_ids = ids;
            PairingWatchFinish::Changed
        }
    }
}
