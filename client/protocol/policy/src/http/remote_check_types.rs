//! Remote access verification statistics and outcome types.

use super::network_path_types::NetworkPath;
use super::{Error, HostApiError, ServerInfo, PERMISSION_LAUNCH, PERMISSION_VIEW};

pub const REMOTE_CHECK_PROBES: usize = 5;

/// Round-trip samples of a connection test. A failed probe counts as loss and breaks the jitter
/// sequence; nothing measured is `None`, never zero.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConnectionTestStats {
    attempts: u32,
    received: u32,
    differences: u32,
    total_ms: f64,
    total_difference_ms: f64,
    previous_ms: Option<f64>,
}

impl ConnectionTestStats {
    /// One probe's round trip; a negative or non-finite time is a failed probe.
    pub fn add(&mut self, milliseconds: f64) {
        self.attempts += 1;
        if !milliseconds.is_finite() || milliseconds < 0.0 {
            self.previous_ms = None;
            return;
        }
        self.received += 1;
        self.total_ms += milliseconds;
        if let Some(previous) = self.previous_ms {
            self.total_difference_ms += (milliseconds - previous).abs();
            self.differences += 1;
        }
        self.previous_ms = Some(milliseconds);
    }

    pub fn add_failure(&mut self) {
        self.add(f64::NAN);
    }

    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    /// The mean round trip of the probes that answered.
    pub fn latency_ms(&self) -> Option<f64> {
        (self.received > 0).then(|| self.total_ms / f64::from(self.received))
    }

    /// The mean difference between consecutive answered probes.
    pub fn jitter_ms(&self) -> Option<f64> {
        (self.differences > 0).then(|| self.total_difference_ms / f64::from(self.differences))
    }

    pub fn loss_percent(&self) -> Option<f64> {
        (self.attempts > 0)
            .then(|| f64::from(self.attempts - self.received) * 100.0 / f64::from(self.attempts))
    }
}

/// What the check found.
#[derive(Debug)]
pub enum RemoteOutcome {
    /// The paired host answered with view and launch permission. `path` is the host's route
    /// category, or why there is none ([`HostApiError::Unavailable`] for a host without it).
    Verified {
        server_info: Box<ServerInfo>,
        path: Result<NetworkPath, HostApiError>,
    },
    /// A host answered with another uniqueid.
    WrongHost,
    /// The host no longer lists this client as paired.
    PairRevoked,
    /// Paired, but without view and launch permission.
    PermissionDenied,
    /// No probe reached the paired host; the last probe's error.
    Unreachable(Error),
}

#[derive(Debug)]
pub struct RemoteCheck {
    pub outcome: RemoteOutcome,
    pub stats: ConnectionTestStats,
}

/// RemoteSetupActivity.verificationProblem: identity first, then pairing, then permission.
pub fn verification_problem(expected_unique_id: &str, info: &ServerInfo) -> Option<RemoteOutcome> {
    if info.unique_id != expected_unique_id {
        Some(RemoteOutcome::WrongHost)
    } else if !info.paired {
        Some(RemoteOutcome::PairRevoked)
    } else if info
        .permission
        .is_some_and(|p| p & (PERMISSION_VIEW | PERMISSION_LAUNCH) == 0)
    {
        Some(RemoteOutcome::PermissionDenied)
    } else {
        None
    }
}
