//! Parallel host polling across direct address slots and internet-play fallback,
//! `is_site_local_host` resolution, Desktop address ordering, and `/internet-play/v1` refresh.

use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::{mpsc, Arc};

use nightjar_policy::hosts::{
    is_site_local_ip, is_tunnel_address, should_clear_cert_on_unpair, CandidateSlot, PollErrorKind,
    ProbeCandidate, SharedHostFields,
};

use crate::http::{
    Error, HostApiError, HostCapabilities, HostClient, HostClientCanceller, InfoPath, InternetPlay,
    ServerInfo,
};
use crate::internet::{self, Tried};

use super::client_pool::HostClientPool;

/// Successful single-address `/serverinfo` poll.
#[derive(Clone, Debug)]
pub struct PollIpSuccess {
    pub candidate: ProbeCandidate,
    pub info: ServerInfo,
    pub path: InfoPath,
    pub should_clear_cert: bool,
}

/// Failed single-address `/serverinfo` poll.
#[derive(Clone, Debug)]
pub struct PollIpError {
    pub candidate: ProbeCandidate,
    pub kind: PollErrorKind,
    pub message: Option<String>,
}

/// Aggregate result of polling direct candidates in precedence order.
#[derive(Clone, Debug, Default)]
pub struct DirectPollOutcome {
    pub winner: Option<PollIpSuccess>,
    pub another_certificate: bool,
    pub tried: Tried,
    pub last_error: Option<PollErrorKind>,
}

/// Resolves whether `host` points at a site-local / private / link-local address.
///
/// IP literals are classified directly without network I/O; hostnames resolve via DNS.
pub fn is_site_local_host(host: &str) -> bool {
    let trimmed = host.trim();
    let unbracketed = trimmed
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(trimmed);
    let without_scope = match unbracketed.split_once('%') {
        Some((addr, _)) if addr.contains(':') => addr,
        _ => unbracketed,
    };
    if let Ok(ip) = without_scope.parse::<IpAddr>() {
        return is_site_local_ip(ip);
    }
    let Ok(addrs) = (trimmed, 0).to_socket_addrs() else {
        return false;
    };
    addrs.into_iter().any(|sa| is_site_local_ip(sa.ip()))
}

/// Polls a single candidate using a borrowed [`HostClient`] from `pool`.
pub fn try_poll_ip(
    pool: &HostClientPool,
    candidate: &ProbeCandidate,
    expected_uuid: Option<&str>,
    cert_der: Option<&[u8]>,
    likely_online: bool,
) -> Result<PollIpSuccess, PollIpError> {
    let (client, pool_gen) = match pool.take(
        &candidate.address,
        candidate.port,
        candidate.https_port,
        cert_der,
    ) {
        Ok(taken) => taken,
        Err(err) => {
            return Err(PollIpError {
                candidate: candidate.clone(),
                kind: classify_http_error(&err),
                message: Some(err.to_string()),
            });
        }
    };
    let result = try_poll_ip_with_client(&client, candidate, expected_uuid, cert_der.is_some(), likely_online);
    pool.give_back(&candidate.address, candidate.port, cert_der, pool_gen, client);
    result
}

/// Polls a single candidate on a provided [`HostClient`].
pub fn try_poll_ip_with_client(
    client: &HostClient,
    candidate: &ProbeCandidate,
    expected_uuid: Option<&str>,
    has_pinned_cert: bool,
    likely_online: bool,
) -> Result<PollIpSuccess, PollIpError> {
    match client.server_info_checked(likely_online) {
        Ok((info, path)) => {
            if let Some(expected) = expected_uuid
                && !expected.is_empty()
                && info.unique_id != expected
            {
                return Err(PollIpError {
                    candidate: candidate.clone(),
                    kind: PollErrorKind::UuidMismatch {
                        expected: expected.to_owned(),
                        actual: info.unique_id,
                    },
                    message: Some("Host uniqueid mismatch".into()),
                });
            }
            let should_clear_cert = has_pinned_cert
                && should_clear_cert_on_unpair(
                    info.paired,
                    matches!(path, InfoPath::Plain),
                    !matches!(path, InfoPath::CertificateMismatch),
                );
            Ok(PollIpSuccess {
                candidate: candidate.clone(),
                info,
                path,
                should_clear_cert,
            })
        }
        Err(err) => Err(PollIpError {
            candidate: candidate.clone(),
            kind: classify_http_error(&err),
            message: Some(err.to_string()),
        }),
    }
}

fn classify_http_error(err: &Error) -> PollErrorKind {
    match err {
        Error::CertificateMismatch => PollErrorKind::CertificateMismatch,
        Error::Cancelled => PollErrorKind::Cancelled,
        Error::Status { code, .. } => PollErrorKind::Http(*code as u16),
        Error::NotFound(_) => PollErrorKind::Http(404),
        Error::Malformed(_) => PollErrorKind::Malformed,
        _ => PollErrorKind::Unreachable,
    }
}

fn record_tried(tried: &mut Tried, candidate: &ProbeCandidate) {
    match candidate.slot {
        CandidateSlot::Local | CandidateSlot::Manual => tried.lan = true,
        CandidateSlot::Ipv6 => tried.ipv6 = true,
        CandidateSlot::Remote => {
            if is_tunnel_address(&candidate.address) {
                // Tunneled path is tracked as punched, not direct WAN.
            } else if candidate.address.contains(':') {
                tried.ipv6 = true;
            } else {
                tried.mapped = true;
            }
        }
        CandidateSlot::Pinned => {
            if candidate.address.contains(':') {
                tried.ipv6 = true;
            } else if is_site_local_host(&candidate.address) {
                tried.lan = true;
            } else {
                tried.mapped = true;
            }
        }
    }
}

/// Polls direct `candidates` concurrently while strictly preserving precedence order (`0..N`).
///
/// As soon as candidate `i` has succeeded and every higher-precedence candidate `< i` has
/// finished, all lower-precedence in-flight requests (`> i`) are cancelled via
/// [`HostClientCanceller::cancel`] and the winning response is returned immediately.
pub fn parallel_poll_direct(
    pool: &Arc<HostClientPool>,
    candidates: &[ProbeCandidate],
    expected_uuid: Option<&str>,
    cert_der: Option<&[u8]>,
    likely_online: bool,
) -> DirectPollOutcome {
    let mut outcome = DirectPollOutcome::default();
    if candidates.is_empty() {
        return outcome;
    }
    for cand in candidates {
        record_tried(&mut outcome.tried, cand);
    }
    if candidates.len() == 1 {
        let cand = &candidates[0];
        match try_poll_ip(pool, cand, expected_uuid, cert_der, likely_online) {
            Ok(win) => outcome.winner = Some(win),
            Err(err) => {
                outcome.another_certificate = matches!(err.kind, PollErrorKind::CertificateMismatch);
                outcome.last_error = Some(err.kind);
            }
        }
        return outcome;
    }

    let (tx, rx) = mpsc::channel::<(usize, Result<PollIpSuccess, PollIpError>)>();
    let n = candidates.len();
    let mut results: Vec<Option<Result<PollIpSuccess, PollIpError>>> = vec![None; n];
    let mut cancellers: Vec<Option<HostClientCanceller>> = Vec::with_capacity(n);

    for (idx, cand) in candidates.iter().enumerate() {
        let taken = pool.take(&cand.address, cand.port, cand.https_port, cert_der);
        match taken {
            Ok((client, pool_gen)) => {
                cancellers.push(Some(client.canceller()));
                let pool = Arc::clone(pool);
                let cand = cand.clone();
                let expected = expected_uuid.map(ToOwned::to_owned);
                let cert = cert_der.map(ToOwned::to_owned);
                let tx = tx.clone();
                let _ = std::thread::Builder::new()
                    .name(format!("nj-poll-{idx}"))
                    .spawn(move || {
                        let res = try_poll_ip_with_client(
                            &client,
                            &cand,
                            expected.as_deref(),
                            cert.is_some(),
                            likely_online,
                        );
                        pool.give_back(&cand.address, cand.port, cert.as_deref(), pool_gen, client);
                        let _ = tx.send((idx, res));
                    });
            }
            Err(err) => {
                cancellers.push(None);
                results[idx] = Some(Err(PollIpError {
                    candidate: cand.clone(),
                    kind: classify_http_error(&err),
                    message: Some(err.to_string()),
                }));
            }
        }
    }
    drop(tx);

    loop {
        let mut winner_idx = None;
        for (i, slot) in results.iter().enumerate() {
            match slot {
                None => break,
                Some(Ok(_)) => {
                    winner_idx = Some(i);
                    break;
                }
                Some(Err(err)) => {
                    if matches!(err.kind, PollErrorKind::CertificateMismatch) {
                        outcome.another_certificate = true;
                    }
                    outcome.last_error = Some(err.kind.clone());
                }
            }
        }
        if let Some(win_i) = winner_idx {
            for (j, cancel) in cancellers.iter().enumerate() {
                if j > win_i
                    && results[j].is_none()
                    && let Some(c) = cancel
                {
                    c.cancel();
                }
            }
            if let Some(Ok(win)) = results[win_i].take() {
                outcome.winner = Some(win);
            }
            return outcome;
        }
        if results.iter().all(Option::is_some) {
            return outcome;
        }
        let Ok((idx, res)) = rx.recv() else {
            return outcome;
        };
        results[idx] = Some(res);
    }
}

/// Failure details from [`poll_over_the_internet`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InternetPollFailure {
    pub reason: String,
    pub another_certificate: bool,
}

/// Probes WAN candidates from `internet_play_json` (IPv6 first, then UPnP/mapped IPv4) before
/// optionally gating only [`internet::punch`] on `allow_punch`, computing `unreachable` on failure.
#[allow(clippy::too_many_arguments)]
pub fn poll_over_the_internet(
    pool: &HostClientPool,
    uuid: &str,
    internet_play_json: &str,
    expected_uuid: Option<&str>,
    cert_der: Option<&[u8]>,
    direct_candidates: &[ProbeCandidate],
    mut tried: Tried,
    likely_online: bool,
    allow_punch: bool,
) -> Result<(SocketAddr, PollIpSuccess), InternetPollFailure> {
    let reply = InternetPlay::parse(internet_play_json).map_err(|e| InternetPollFailure {
        reason: e.to_string(),
        another_certificate: false,
    })?;
    let mut another_certificate = false;

    for ip in &reply.ipv6 {
        let addr_str = ip.to_string();
        tried.ipv6 = true;
        if direct_candidates
            .iter()
            .any(|d| d.address == addr_str && d.port == reply.ports.http)
        {
            continue;
        }
        let candidate = ProbeCandidate {
            slot: CandidateSlot::Ipv6,
            address: addr_str,
            port: reply.ports.http,
            https_port: reply.ports.https,
        };
        match try_poll_ip(pool, &candidate, expected_uuid, cert_der, likely_online) {
            Ok(win) => return Ok((SocketAddr::new(IpAddr::V6(*ip), candidate.port), win)),
            Err(err) => {
                if matches!(err.kind, PollErrorKind::CertificateMismatch) {
                    another_certificate = true;
                }
            }
        }
    }

    if reply.mapped_ports
        && let Some(ip) = reply.external_ipv4
    {
        let addr_str = ip.to_string();
        tried.mapped = true;
        if !direct_candidates
            .iter()
            .any(|d| d.address == addr_str && d.port == reply.ports.http)
        {
            let candidate = ProbeCandidate {
                slot: CandidateSlot::Remote,
                address: addr_str,
                port: reply.ports.http,
                https_port: reply.ports.https,
            };
            match try_poll_ip(pool, &candidate, expected_uuid, cert_der, likely_online) {
                Ok(win) => return Ok((SocketAddr::new(IpAddr::V4(ip), candidate.port), win)),
                Err(err) => {
                    if matches!(err.kind, PollErrorKind::CertificateMismatch) {
                        another_certificate = true;
                    }
                }
            }
        }
    }

    if !allow_punch {
        return Err(InternetPollFailure {
            reason: internet::unreachable(&reply, tried, None),
            another_certificate,
        });
    }

    let addr = internet::punch(uuid, &reply, tried).map_err(|reason| InternetPollFailure {
        reason,
        another_certificate,
    })?;
    let candidate = ProbeCandidate {
        slot: CandidateSlot::Remote,
        address: addr.ip().to_string(),
        port: addr.port(),
        https_port: reply.ports.https,
    };
    match try_poll_ip(pool, &candidate, expected_uuid, cert_der, likely_online) {
        Ok(win) => Ok((addr, win)),
        Err(err) => {
            if matches!(err.kind, PollErrorKind::CertificateMismatch) {
                another_certificate = true;
            }
            let msg = err
                .message
                .unwrap_or_else(|| "internet poll failed".into());
            Err(InternetPollFailure {
                reason: internet::unreachable(&reply, tried, Some(msg)),
                another_certificate,
            })
        }
    }
}

/// Computes `"lan"`, `"ipv6"`, `"mapped"`, or `"punched"` for an active endpoint given
/// local/manual LAN endpoints and stored `/internet-play/v1` reply.
pub fn compute_internet_path(
    active: Option<&nightjar_policy::hosts::EndpointSlot>,
    local: Option<&nightjar_policy::hosts::EndpointSlot>,
    manual: Option<&nightjar_policy::hosts::EndpointSlot>,
    reply: Option<&InternetPlay>,
) -> Option<String> {
    let active_addr = active?.address.parse::<IpAddr>().ok()?;
    let lan: Vec<IpAddr> = local
        .into_iter()
        .chain(manual)
        .filter_map(|ep| ep.address.parse::<IpAddr>().ok())
        .collect();
    Some(internet::path_of(active_addr, &lan, reply).key().to_owned())
}

/// Fetches a fresh `/internet-play/v1` reply from the host over HTTPS.
pub fn refresh_internet_play(
    pool: &HostClientPool,
    address: &str,
    http_port: u16,
    https_port: u16,
    cert_der: &[u8],
    capabilities: &HostCapabilities,
) -> Result<String, HostApiError> {
    let (client, pool_gen) = pool.take(address, http_port, https_port, Some(cert_der))?;
    let res = client.internet_play(capabilities).map(|(raw, _)| raw);
    pool.give_back(address, http_port, Some(cert_der), pool_gen, client);
    res
}

/// Parses stored `internet_play` JSON cleanly.
pub fn parse_internet_reply(raw: Option<&str>) -> Option<InternetPlay> {
    raw.and_then(|text| InternetPlay::parse(text).ok())
}

/// Builds the ordered poll candidate addresses for a Desktop host entry.
///
/// Pinned addresses poll alone; otherwise active + other non-remote addresses run before
/// IPv6, stored remote, and mapped IPv4 internet candidates.
pub fn desktop_poll_addresses(
    shared: &SharedHostFields,
    reply: Option<&InternetPlay>,
) -> Vec<String> {
    if let Some(pinned) = &shared.pinned_address {
        return vec![pinned.clone()];
    }
    let remote = shared.remote_address.as_ref();
    let mut addresses: Vec<String> = std::iter::once(&shared.address)
        .chain(&shared.other_addresses)
        .filter(|a| !a.is_empty() && Some(*a) != remote)
        .cloned()
        .collect();

    let mut away: Vec<String> = Vec::new();
    if let Some(reply) = reply {
        away.extend(reply.ipv6.iter().map(ToString::to_string));
    }
    away.extend(remote.cloned());
    if let Some(mapped) = reply
        .filter(|r| r.mapped_ports)
        .and_then(|r| r.external_ipv4)
    {
        away.push(mapped.to_string());
    }
    for address in away {
        if !address.is_empty() && !addresses.contains(&address) {
            addresses.push(address);
        }
    }
    addresses
}

/// Whether `address` reaches a Desktop host from outside its home network.
pub fn away_address(
    shared: &SharedHostFields,
    reply: Option<&InternetPlay>,
    address: &str,
) -> bool {
    if shared.remote_address.as_deref() == Some(address) {
        return true;
    }
    let Ok(ip) = address.parse::<IpAddr>() else {
        return false;
    };
    if internet::is_tunnel(ip) {
        return true;
    }
    reply.is_some_and(|r| match ip {
        IpAddr::V6(ip6) => r.ipv6.contains(&ip6),
        IpAddr::V4(ip4) => r.external_ipv4 == Some(ip4),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::identity::tests::fixture;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    #[test]
    fn site_local_host_detects_private_literals_without_dns() {
        assert!(is_site_local_host("192.168.1.50"));
        assert!(is_site_local_host("10.0.0.5"));
        assert!(is_site_local_host("172.16.4.1"));
        assert!(is_site_local_host("[fd00::1]"));
        assert!(!is_site_local_host("203.0.113.10"));
        assert!(!is_site_local_host("definitely-invalid.invalid"));
    }

    #[test]
    fn parallel_poll_direct_prefers_higher_precedence_winner_and_cancels_loser() {
        let local_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let local_port = local_listener.local_addr().unwrap().port();
        // Second listener accepts and intentionally stalls until cancelled.
        let manual_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let manual_port = manual_listener.local_addr().unwrap().port();

        let local_srv = std::thread::spawn(move || {
            let (mut sock, _) = local_listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let _ = sock.read(&mut buf);
            let body = r#"<root status_code="200"><hostname>LocalPC</hostname><uniqueid>pc-1</uniqueid><state>MJ_SERVER_FREE</state><PairStatus>0</PairStatus><HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort></root>"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            sock.write_all(reply.as_bytes()).unwrap();
        });

        let (accepted_tx, accepted_rx) = mpsc::channel();
        let manual_srv = std::thread::spawn(move || {
            let (mut sock, _) = manual_listener.accept().unwrap();
            let _ = accepted_tx.send(());
            let mut buf = [0u8; 64];
            // Blocks until client closes socket via cancel_pending_requests.
            let _ = sock.read(&mut buf);
        });

        let pool = Arc::new(HostClientPool::new(Arc::new(fixture())));
        let candidates = vec![
            ProbeCandidate {
                slot: CandidateSlot::Local,
                address: "127.0.0.1".into(),
                port: local_port,
                https_port: 0,
            },
            ProbeCandidate {
                slot: CandidateSlot::Manual,
                address: "127.0.0.1".into(),
                port: manual_port,
                https_port: 0,
            },
        ];
        let outcome = parallel_poll_direct(&pool, &candidates, Some("pc-1"), None, true);
        let winner = outcome.winner.expect("expected local winner");
        assert_eq!(winner.candidate.slot, CandidateSlot::Local);
        assert_eq!(winner.info.hostname, "LocalPC");
        assert!(accepted_rx.recv_timeout(Duration::from_secs(2)).is_ok());

        local_srv.join().unwrap();
        manual_srv.join().unwrap();
    }

    #[test]
    fn desktop_poll_addresses_and_away_address_preserve_order_and_internet_paths() {
        let shared = SharedHostFields {
            address: "192.168.1.5".into(),
            remote_address: Some("203.0.113.10".into()),
            other_addresses: vec!["203.0.113.10".into(), "10.0.0.5".into()],
            ..Default::default()
        };
        let reply = InternetPlay::parse(
            r#"{"version":1,"enabled":true,"mappedPorts":true,"externalIpv4":"198.51.100.7","ipv6":["2001:db8::9"],"ports":{"http":47989,"https":47984,"rtsp":48010,"video":47998,"audio":48000,"control":47999,"mic":48002},"endpoints":[],"nat":"unknown"}"#,
        )
        .unwrap();
        assert_eq!(
            desktop_poll_addresses(&shared, Some(&reply)),
            vec!["192.168.1.5", "10.0.0.5", "2001:db8::9", "203.0.113.10", "198.51.100.7"]
        );
        assert!(away_address(&shared, Some(&reply), "203.0.113.10"));
        assert!(away_address(&shared, Some(&reply), "2001:db8::9"));
        assert!(away_address(&shared, Some(&reply), "127.78.0.5"));
        assert!(!away_address(&shared, Some(&reply), "192.168.1.5"));
    }
}
