//! Pure host reachability polling policies, cadence constants, candidate slot precedence,
//! port reuse rules, internet punch triggers, and unpair certificate clearing decisions shared
//! across Nightjar Android and Desktop.

use serde::{Deserialize, Serialize};

use super::address::{EndpointSlot, HostSlots};

/// Interval between consecutive serverinfo polls on Android (`ComputerManagerService`).
pub const SERVERINFO_POLL_PERIOD_MS: u64 = 1_500;
/// Maximum age before previously polled reachability resets to [`HostReachState::Unknown`].
pub const POLL_DATA_TTL_MS: u64 = 30_000;
/// Number of consecutive failures before an online host transitions to [`HostReachState::Offline`].
pub const OFFLINE_POLL_TRIES: u32 = 3;
/// Number of consecutive failures before an initial/unknown host transitions to [`HostReachState::Offline`].
pub const INITIAL_POLL_TRIES: u32 = 2;
/// App list polling cadence once an app list has been received.
pub const APPLIST_POLL_PERIOD_MS: u64 = 30_000;
/// App list retry cadence before the first app list is received.
pub const APPLIST_RETRY_PERIOD_MS: u64 = 2_000;
/// Number of consecutive empty app list XML responses accepted as genuinely empty.
pub const EMPTY_APPLIST_THRESHOLD: u32 = 3;
/// Minimum interval between re-reading `/internet-play/v1` while on LAN.
pub const INTERNET_PLAY_READ_INTERVAL_MS: u64 = 600_000;

/// Host reachability state shared across Desktop and Android.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum HostReachState {
    #[default]
    Unknown,
    Online,
    Offline,
}

/// Per-host polling state machine (clock-free: callers pass `now_ms`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollTupleState {
    pub state: HostReachState,
    pub last_successful_poll_ms: u64,
    pub has_polled_successfully: bool,
    pub offline_count: u32,
    pub consecutive_empty_applist: u32,
    pub removed: bool,
}

impl PollTupleState {
    /// Whether the cached poll state has expired at `now_ms` and should reset to `Unknown`.
    pub fn is_ttl_expired(&self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.last_successful_poll_ms) > POLL_DATA_TTL_MS
    }

    /// Enforces [`POLL_DATA_TTL_MS`] on start of polling, resetting state to `Unknown` when stale.
    pub fn enforce_ttl(&mut self, now_ms: u64) {
        if self.is_ttl_expired(now_ms) {
            self.state = HostReachState::Unknown;
        }
    }

    /// Whether a poll is due at `now_ms`.
    pub fn is_poll_due(&self, now_ms: u64, force: bool) -> bool {
        if self.removed {
            return false;
        }
        force
            || !self.has_polled_successfully
            || now_ms.saturating_sub(self.last_successful_poll_ms) >= SERVERINFO_POLL_PERIOD_MS
    }

    /// Records a successful poll at `now_ms`, clearing `offline_count`.
    pub fn record_poll_success(&mut self, now_ms: u64) {
        self.state = HostReachState::Online;
        self.last_successful_poll_ms = now_ms;
        self.has_polled_successfully = true;
        self.offline_count = 0;
    }

    /// Records a failed poll. Returns `true` when the failure threshold has been reached and the
    /// host transitions to [`HostReachState::Offline`].
    ///
    /// Matches Android `ComputerManagerService::runPoll`: while `offline_count < tries`, returns
    /// `false` and increments `offline_count`; once `offline_count >= tries`, sets `Offline` and
    /// returns `true`.
    pub fn record_poll_failure(&mut self, initial: bool) -> bool {
        let tries = if initial {
            INITIAL_POLL_TRIES
        } else {
            OFFLINE_POLL_TRIES
        };
        if self.offline_count < tries {
            self.offline_count += 1;
            return false;
        }
        self.state = HostReachState::Offline;
        true
    }
}

/// Which address slot produced a [`ProbeCandidate`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CandidateSlot {
    Local,
    Manual,
    Ipv6,
    Remote,
    Pinned,
}

/// A concrete `(address, port)` probe candidate in precedence order with an optional cached HTTPS port.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeCandidate {
    pub slot: CandidateSlot,
    pub address: String,
    pub port: u16,
    pub https_port: u16,
}

/// Returns `known_https_port` if the host is currently online at `active_http_port` matching
/// `candidate_port`, else `0` (forcing HTTPS port discovery via HTTP `serverinfo`).
pub fn port_matches_active_address(
    active_online: bool,
    active_http_port: Option<u16>,
    candidate_port: u16,
    known_https_port: u16,
) -> u16 {
    if active_online && active_http_port == Some(candidate_port) {
        known_https_port
    } else {
        0
    }
}

/// Builds the deduplicated list of direct probe candidates in precedence order.
///
/// If `slots.pinned` is set, returns only `[Pinned]`. Otherwise orders
/// `[Local, Manual, Ipv6, Remote]` and deduplicates by `(address, port)`.
pub fn probe_plan(
    slots: &HostSlots,
    active_online: bool,
    active_http_port: Option<u16>,
    known_https_port: u16,
) -> Vec<ProbeCandidate> {
    if let Some(pinned) = &slots.pinned {
        return vec![ProbeCandidate {
            slot: CandidateSlot::Pinned,
            address: pinned.address.clone(),
            port: pinned.port,
            https_port: port_matches_active_address(
                active_online,
                active_http_port,
                pinned.port,
                known_https_port,
            ),
        }];
    }
    let ordered = [
        (CandidateSlot::Local, &slots.local),
        (CandidateSlot::Manual, &slots.manual),
        (CandidateSlot::Ipv6, &slots.ipv6),
        (CandidateSlot::Remote, &slots.remote),
    ];
    let mut plan: Vec<ProbeCandidate> = Vec::with_capacity(4);
    for (slot, maybe) in ordered {
        let Some(ep) = maybe else { continue };
        if plan
            .iter()
            .any(|p| p.address == ep.address && p.port == ep.port)
        {
            continue;
        }
        plan.push(ProbeCandidate {
            slot,
            address: ep.address.clone(),
            port: ep.port,
            https_port: port_matches_active_address(
                active_online,
                active_http_port,
                ep.port,
                known_https_port,
            ),
        });
    }
    plan
}

/// Whether a poll whose direct paths failed tries the punched path now: once the PC reads as
/// offline/away, or while a punched tunnel address is already active.
pub fn punches_now(_has_local: bool, offline: bool, active_is_tunnel: bool) -> bool {
    offline || active_is_tunnel
}

/// Whether a host's pinned certificate should be cleared when `serverinfo` reports not paired.
///
/// Only the pinned host's own authoritative response (not plain HTTP fallback and not another
/// certificate) clears the stored pairing.
pub fn should_clear_cert_on_unpair(
    pair_state_paired: bool,
    info_path_plain: bool,
    same_cert: bool,
) -> bool {
    !pair_state_paired && !info_path_plain && same_cert
}

/// Whether `/internet-play/v1` should be refreshed from the host at `now_ms`.
pub fn is_internet_play_read_due(last_read_ms: Option<u64>, now_ms: u64) -> bool {
    match last_read_ms {
        None => true,
        Some(last) => now_ms.saturating_sub(last) >= INTERNET_PLAY_READ_INTERVAL_MS,
    }
}

/// Next sleep delay in milliseconds for app list polling.
pub fn next_applist_interval_ms(received_app_list: bool) -> u64 {
    if received_app_list {
        APPLIST_POLL_PERIOD_MS
    } else {
        APPLIST_RETRY_PERIOD_MS
    }
}

/// Evaluates whether an app list XML payload should be accepted and published.
///
/// Non-empty lists reset `empty_count` to `0` and return `true`. Empty parsed lists increment
/// `empty_count` and return `true` only once [`EMPTY_APPLIST_THRESHOLD`] is reached. Empty raw
/// XML always returns `false`.
pub fn accept_applist_response(
    non_empty_xml: bool,
    apps_empty: bool,
    empty_count: &mut u32,
) -> bool {
    if !non_empty_xml {
        return false;
    }
    if !apps_empty {
        *empty_count = 0;
        return true;
    }
    *empty_count += 1;
    *empty_count >= EMPTY_APPLIST_THRESHOLD
}

/// Structured failure classification for a single address poll attempt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PollErrorKind {
    CertificateMismatch,
    UuidMismatch { expected: String, actual: String },
    Unreachable,
    Malformed,
    Http(u16),
    Cancelled,
}

/// Structured result of polling a host across direct and internet paths.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollOutcome {
    pub reached: bool,
    pub active_slot: Option<CandidateSlot>,
    pub active_endpoint: Option<EndpointSlot>,
    pub another_certificate: bool,
    pub error: Option<PollErrorKind>,
}
