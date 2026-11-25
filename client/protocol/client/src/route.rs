//! Route reachability checks and retry waits before a recovery attempt.
//!
//! A Wi-Fi network often returns before it has an address toward the PC (DHCP after the
//! reconnect), so recovery waits for a route, checking every [`ROUTE_RETRY_MS`], instead of
//! failing at once and backing off.

use std::io::{self, ErrorKind};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use nightjar_policy::recovery::{ROUTE_PROBE_TIMEOUT_MS, ROUTE_RETRY_MS, RouteWait};

/// Outcome of waiting for a route to the PC before a recovery attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteWaitOutcome {
    Reachable,
    Exhausted,
    Cancelled,
}

/// A route check's immediate no-route error: no address toward the PC (yet). A check that
/// times out (a slow link, or a closed port, which Windows retries before it says so) or fails
/// otherwise is no reason to wait: the attempt itself decides, loudly.
pub fn route_missing(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::NetworkUnreachable | ErrorKind::HostUnreachable | ErrorKind::AddrNotAvailable
    )
}

/// Why there is no route toward the PC's HTTP port yet ([`route_missing`], or the name does not
/// resolve), or `None` to go on with the attempt: an answer, refused included, and also a check
/// that timed out or failed otherwise.
pub fn route_problem(address: &str, port: u16) -> Option<String> {
    let trimmed = address.trim();
    let host = trimmed
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(trimmed);
    let addrs = match (host, port).to_socket_addrs() {
        Ok(found) => found,
        Err(error) => return Some(error.to_string()),
    };
    let mut any = false;
    let mut last_missing = None;
    for remote in addrs {
        any = true;
        match TcpStream::connect_timeout(&remote, Duration::from_millis(ROUTE_PROBE_TIMEOUT_MS)) {
            Err(error) if route_missing(&error) => last_missing = Some(error.to_string()),
            _ => return None,
        }
    }
    if !any {
        Some(format!("{address} did not resolve"))
    } else {
        last_missing
    }
}

/// Waits until the PC is reachable, `deadline_ms` is reached, or `cancelled()` / `sleep_ms`
/// interrupts the wait.
pub fn wait_for_route(
    deadline_ms: u64,
    now_ms: &dyn Fn() -> u64,
    probe: &dyn Fn() -> Option<String>,
    cancelled: &dyn Fn() -> bool,
    sleep_ms: &dyn Fn(u64) -> bool,
    log_info: &dyn Fn(&str),
    log_warn: &dyn Fn(&str),
) -> RouteWaitOutcome {
    let started_ms = now_ms();
    let mut wait = RouteWait::new(deadline_ms);
    while !cancelled() {
        let checked_at = now_ms();
        let unreachable = probe();
        let after_probe = now_ms();
        match wait.next(after_probe, unreachable.is_none()) {
            Some(0) => {
                if wait.unreachable_checks() > 0 {
                    log_info(&format!(
                        "Recovery: the PC is reachable again after {} ms",
                        after_probe.saturating_sub(started_ms)
                    ));
                }
                return RouteWaitOutcome::Reachable;
            }
            None => {
                log_warn(&format!(
                    "Recovery: the PC stayed unreachable until the recovery window ended: {}",
                    unreachable.unwrap_or_default()
                ));
                return RouteWaitOutcome::Exhausted;
            }
            Some(next) => {
                if wait.unreachable_checks() == 1 {
                    log_info(&format!(
                        "Recovery: no route to the PC yet ({}), checking every {} ms",
                        unreachable.unwrap_or_default(),
                        ROUTE_RETRY_MS
                    ));
                }
                let delay = next.saturating_sub(after_probe.saturating_sub(checked_at));
                if !sleep_ms(delay) {
                    return RouteWaitOutcome::Cancelled;
                }
            }
        }
    }
    RouteWaitOutcome::Cancelled
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::net::{Ipv4Addr, TcpListener};

    #[test]
    fn only_an_immediate_no_route_error_waits_for_the_route() {
        for kind in [
            ErrorKind::NetworkUnreachable,
            ErrorKind::HostUnreachable,
            ErrorKind::AddrNotAvailable,
        ] {
            assert!(route_missing(&io::Error::from(kind)), "{kind:?}");
        }
        for kind in [
            ErrorKind::TimedOut,
            ErrorKind::ConnectionRefused,
            ErrorKind::ConnectionReset,
            ErrorKind::Other,
        ] {
            assert!(!route_missing(&io::Error::from(kind)), "{kind:?}");
        }
        #[cfg(windows)]
        let (unreachable, timed_out) = ([10051, 10065, 10049], 10060);
        #[cfg(not(windows))]
        let (unreachable, timed_out) = (
            [libc::ENETUNREACH, libc::EHOSTUNREACH, libc::EADDRNOTAVAIL],
            libc::ETIMEDOUT,
        );
        for code in unreachable {
            assert!(route_missing(&io::Error::from_raw_os_error(code)), "{code}");
        }
        assert!(!route_missing(&io::Error::from_raw_os_error(timed_out)));
    }

    #[test]
    fn route_problem_allows_listening_port_and_flags_bad_host() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert_eq!(route_problem("127.0.0.1", port), None);
        assert!(route_problem("invalid.invalid.", port).is_some());
        // Bracketed IPv6 literal must not fail DNS/parse due to brackets.
        if let Ok(v6) = std::net::TcpListener::bind("[::1]:0") {
            let v6_port = v6.local_addr().unwrap().port();
            assert_eq!(route_problem("[::1]", v6_port), None);
        }
    }

    #[test]
    fn wait_for_route_handles_reachable_exhausted_and_cancelled() {
        let now = Cell::new(100_u64);
        let probes = Cell::new(0_u32);
        let infos = RefCell::new(Vec::new());
        let warns = RefCell::new(Vec::new());
        let outcome = wait_for_route(
            2_000,
            &|| now.get(),
            &|| {
                let n = probes.get();
                probes.set(n + 1);
                if n < 2 {
                    Some("No route to host".into())
                } else {
                    None
                }
            },
            &|| false,
            &|ms| {
                now.set(now.get() + ms);
                true
            },
            &|msg| infos.borrow_mut().push(msg.to_owned()),
            &|msg| warns.borrow_mut().push(msg.to_owned()),
        );
        assert_eq!(outcome, RouteWaitOutcome::Reachable);
        assert_eq!(infos.borrow().len(), 2);
        assert!(warns.borrow().is_empty());

        now.set(1_000);
        let exhausted = wait_for_route(
            1_600,
            &|| now.get(),
            &|| Some("unreachable".into()),
            &|| false,
            &|ms| {
                now.set(now.get() + ms);
                true
            },
            &|_| {},
            &|_| {},
        );
        assert_eq!(exhausted, RouteWaitOutcome::Exhausted);

        let cancelled = wait_for_route(
            2_000,
            &|| 100,
            &|| Some("unreachable".into()),
            &|| false,
            &|_| false,
            &|_| {},
            &|_| {},
        );
        assert_eq!(cancelled, RouteWaitOutcome::Cancelled);
    }
}
