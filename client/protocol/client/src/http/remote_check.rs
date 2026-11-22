//! The remote access check of Android's RemoteSetupActivity: five serverinfo probes over pinned
//! HTTPS only (no HTTP fallback), each verifying it reached the paired host, then the network
//! path when the host offers it. Latency, jitter and loss follow ConnectionTestStats.java.

use super::transport::{self, Request, Timeouts};
use super::{
    Error, HostApiError, HostClient, NetworkPath, ServerInfo, READ_TIMEOUT,
    SHORT_CONNECT_TIMEOUT,
};
use std::time::{Duration, Instant};

pub use nightjar_policy::http::{ConnectionTestStats, RemoteCheck, RemoteOutcome, REMOTE_CHECK_PROBES, verification_problem};

const PROBE_INTERVAL: Duration = Duration::from_millis(150);
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_PROBE_BYTES: usize = 64 * 1024;

/// The probe loop. `probe` answers serverinfo and its round trip in ms; a verification problem
/// ends the check at once. A cancel ends it with [`Error::Cancelled`]: a request cut off by
/// `cancelled` may fail with another error (the shut socket reads as an early EOF), so a
/// failure is checked against `cancelled` before it counts.
fn run(
    expected_unique_id: &str,
    mut probe: impl FnMut() -> Result<(ServerInfo, f64), Error>,
    mut pause: impl FnMut(),
    path: impl FnOnce(&ServerInfo) -> Result<NetworkPath, HostApiError>,
    cancelled: impl Fn() -> bool,
) -> Result<RemoteCheck, Error> {
    let mut stats = ConnectionTestStats::default();
    let mut verified = None;
    let mut last_error = None;
    for n in 0..REMOTE_CHECK_PROBES {
        if n > 0 {
            pause();
        }
        match probe() {
            Ok((info, milliseconds)) => {
                if let Some(outcome) = verification_problem(expected_unique_id, &info) {
                    stats.add_failure();
                    return Ok(RemoteCheck { outcome, stats });
                }
                stats.add(milliseconds);
                verified = Some(info);
            }
            Err(_) if cancelled() => return Err(Error::Cancelled),
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(e) => {
                stats.add_failure();
                last_error = Some(e);
            }
        }
    }
    let outcome = match verified {
        Some(server_info) => {
            if cancelled() {
                return Err(Error::Cancelled);
            }
            let path = path(&server_info);
            if cancelled() || matches!(path, Err(HostApiError::Host(Error::Cancelled))) {
                return Err(Error::Cancelled);
            }
            RemoteOutcome::Verified {
                path,
                server_info: Box::new(server_info),
            }
        }
        None => RemoteOutcome::Unreachable(
            last_error.expect("every probe failed, so the last one left its error"),
        ),
    };
    Ok(RemoteCheck { outcome, stats })
}

impl HostClient {
    /// One serverinfo over plain HTTP, answered within `timeout` with connecting included, as
    /// Android's `NvHTTP.probeServerLatency`: the round trip in milliseconds, parsing the answer
    /// included. The connection test's delay probe (`nightjar_policy::connection_test`): an
    /// unpaired PC answers it too, and a redirect is a failure, never followed.
    pub fn probe_server_latency(&self, timeout: Duration) -> Result<f64, Error> {
        let target = self.url_target("serverinfo", None);
        let started = Instant::now();
        let response = transport::probe(
            &Request {
                host: &self.host,
                port: self.http_port,
                target: &target,
                tls: None,
                timeouts: Timeouts {
                    connect: SHORT_CONNECT_TIMEOUT.min(timeout),
                    read: Some(READ_TIMEOUT),
                },
                body_limit: MAX_PROBE_BYTES,
            },
            &self.cancel,
            timeout,
            &mut |status, headers, _| {
                super::hardware_metrics::check_head("serverinfo", status, headers, false)
            },
        )?;
        let raw = String::from_utf8(response.body)
            .map_err(|_| Error::Malformed("host reply is not UTF-8".into()))?;
        ServerInfo::parse(&raw, self.http_port)?;
        Ok(started.elapsed().as_secs_f64() * 1000.0)
    }

    /// Checks that this address reaches the paired host whose serverinfo `uniqueid` is
    /// `expected_unique_id`: five pinned HTTPS probes 150 ms apart, then the network path.
    /// Blocks for up to about 20 s; [`HostClient::cancel_pending_requests`] ends it with
    /// [`Error::Cancelled`]. Needs the pinned certificate and the HTTPS port, and never
    /// discovers either.
    pub fn remote_check(&self, expected_unique_id: &str) -> Result<RemoteCheck, Error> {
        if expected_unique_id.is_empty() {
            return Err(Error::InvalidArgument(
                "The host's uniqueid is not known".into(),
            ));
        }
        self.pinned()?;
        run(
            expected_unique_id,
            || {
                let started = Instant::now();
                let response =
                    self.pinned_get("serverinfo", MAX_PROBE_BYTES, PROBE_TIMEOUT, false)?;
                let raw = String::from_utf8(response.body)
                    .map_err(|_| Error::Malformed("host reply is not UTF-8".into()))?;
                let info = ServerInfo::parse(&raw, self.http_port)?;
                Ok((info, started.elapsed().as_secs_f64() * 1000.0))
            },
            || std::thread::sleep(PROBE_INTERVAL),
            |info| self.network_path(&info.capabilities),
            || self.cancel.is_cancelled(),
        )
    }
}

#[cfg(test)]
mod tests;
