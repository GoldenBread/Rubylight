//! PCP (RFC 6887) and NAT-PMP (RFC 6886) port mappings, for routers that don't speak UPnP IGD,
//! and PCP pinholes in an IPv6 router's firewall. The formats are `rubylight_traverse`'s; this
//! is the exchange with the router and the leases.
use super::{Mapping, Protocol};
use anyhow::{Context, Result, bail};
use rubylight_traverse::{natpmp, pcp};
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6},
    time::{Duration, Instant},
};
use tokio::net::UdpSocket;

/// The lifetime asked for every mapping, in seconds; leases renew at half of what the router
/// grants. The lease record keeps a guest port until this lifetime ends.
pub(super) const LIFETIME: u32 = 7200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Method {
    Pcp,
    NatPmp,
}
impl Method {
    pub(super) fn name(self) -> &'static str {
        match self {
            Method::Pcp => "PCP",
            Method::NatPmp => "NAT-PMP",
        }
    }
}

struct Lease {
    mapping: Mapping,
    external: u16,
    renew_at: Instant,
}

/// The mappings asked of one router at one of this PC's addresses.
pub(super) struct PortMapper {
    socket: UdpSocket,
    local: IpAddr,
    ipv6_target: Option<(Ipv6Addr, u32)>,
    method: Option<Method>,
    /// PCP's mapping nonce ([`nonce`]): the same for every mapper of this host at `local`.
    nonce: [u8; 12],
    leases: Vec<Lease>,
    epoch: Option<(u32, Instant)>,
    pub(super) external: Option<IpAddr>,
}

/// What a renewal did: each wanted port mapped (with its external port) or refused and why, the
/// mappings the router mapped afresh this round, the leases it confirmed gone, and the router's
/// silence if it ran out of answers.
#[derive(Debug, Default)]
pub(super) struct Outcome {
    pub(super) mapped: Vec<(Mapping, u16)>,
    /// The mappings the router answered with a fresh lease this round; a mapping whose lease
    /// was still valid is in `mapped` alone.
    pub(super) renewed: Vec<Mapping>,
    pub(super) failed: Vec<(Mapping, String)>,
    /// The mappings the router answered for and refused: not this host's to keep.
    pub(super) refused: Vec<Mapping>,
    /// The leases this round asked the router to delete, and that it answered for.
    pub(super) removed: Vec<Mapping>,
    /// The router stopped answering: why, for the console.
    pub(super) error: Option<anyhow::Error>,
}

/// The PCP mapping nonce of this host at `local`, from `secret` (the host's private key). A PCP
/// router refuses to renew or delete a mapping under another nonce than the one that made it
/// (RFC 6887 11.3, NOT_AUTHORIZED) until its lease runs out, so it stays the same across new
/// mappers and host restarts, and others cannot work it out.
fn nonce(secret: &[u8], local: IpAddr) -> [u8; 12] {
    let mut nonce = [0; 12];
    rubylight_traverse::crypto::derive(
        secret,
        b"pcp mapping nonce",
        local.to_string().as_bytes(),
        &mut nonce,
    );
    nonce
}

fn protocol_number(protocol: Protocol) -> u8 {
    if protocol == Protocol::TCP {
        pcp::TCP
    } else {
        pcp::UDP
    }
}

impl PortMapper {
    /// A mapper for the IPv4 router `gateway`, from this PC's `local` address; `secret` makes
    /// the nonce ([`nonce`]).
    pub(super) async fn ipv4(gateway: Ipv4Addr, local: Ipv4Addr, secret: &[u8]) -> Result<Self> {
        // Tests ask no router on this PC's network.
        #[cfg(test)]
        anyhow::ensure!(gateway.is_loopback(), "tests ask no router at {gateway}");
        let socket = UdpSocket::bind((local, 0)).await?;
        socket.connect((gateway, pcp::PORT)).await?;
        Ok(Self::new(socket, IpAddr::V4(local), secret))
    }

    /// Whether this mapper asks `gateway` from `local`.
    pub(super) fn serves(&self, gateway: Ipv4Addr, local: Ipv4Addr) -> bool {
        self.local == IpAddr::V4(local)
            && self.socket.peer_addr().ok() == Some(SocketAddr::from((gateway, pcp::PORT)))
    }

    /// Whether this IPv6 pinhole mapper serves `gateway`, `local`, and `index`.
    pub(super) fn serves_ipv6(&self, gateway: Ipv6Addr, local: Ipv6Addr, index: u32) -> bool {
        self.local == IpAddr::V6(local) && self.ipv6_target == Some((gateway, index))
    }

    /// A PCP mapper for pinholes in the IPv6 router `gateway`'s firewall, for this PC's global
    /// address `local` on interface `index` (a link-local router needs it).
    pub(super) async fn ipv6(
        gateway: Ipv6Addr,
        local: Ipv6Addr,
        index: u32,
        secret: &[u8],
    ) -> Result<Self> {
        let socket = UdpSocket::bind(SocketAddrV6::new(local, 0, 0, index)).await?;
        let scope = if gateway.is_unicast_link_local() {
            index
        } else {
            0
        };
        socket
            .connect(SocketAddrV6::new(gateway, pcp::PORT, 0, scope))
            .await?;
        let mut mapper = Self::new(socket, IpAddr::V6(local), secret);
        mapper.ipv6_target = Some((gateway, index));
        // NAT-PMP knows no IPv6.
        mapper.method = Some(Method::Pcp);
        Ok(mapper)
    }

    pub(super) fn new(socket: UdpSocket, local: IpAddr, secret: &[u8]) -> Self {
        Self {
            socket,
            local,
            ipv6_target: None,
            method: None,
            nonce: nonce(secret, local),
            leases: Vec::new(),
            epoch: None,
            external: None,
        }
    }

    /// Updates router epoch tracking and drops stale leases if the epoch jumped back
    /// (RFC 6887 §8.5 / RFC 6886 §3.6 router reboot detection).
    fn observe_epoch(&mut self, epoch: u32, now: Instant) -> bool {
        let rebooted = self.epoch.is_some_and(|(prev, prev_at)| {
            if epoch < prev {
                return true;
            }
            let elapsed = now.saturating_duration_since(prev_at).as_secs();
            u64::from(epoch - prev).saturating_add(2) < elapsed.saturating_sub(elapsed / 16)
        });
        self.epoch = Some((epoch, now));
        if rebooted {
            self.leases.clear();
        }
        rebooted
    }

    pub(super) fn method(&self) -> Option<Method> {
        self.method
    }

    /// This PC's address the mappings are for.
    pub(super) fn local(&self) -> IpAddr {
        self.local
    }

    /// Sends `request` until a datagram `matches` comes back: RFC 6886's schedule from 250 ms,
    /// doubling, four tries.
    async fn exchange(&self, request: &[u8], matches: impl Fn(&[u8]) -> bool) -> Option<Vec<u8>> {
        let mut wait = Duration::from_millis(250);
        let mut buffer = [0u8; 1100];
        for _ in 0..4 {
            if self.socket.send(request).await.is_err() {
                return None;
            }
            let deadline = tokio::time::Instant::now() + wait;
            while let Ok(Ok(length)) =
                tokio::time::timeout_at(deadline, self.socket.recv(&mut buffer)).await
            {
                if matches(&buffer[..length]) {
                    return Some(buffer[..length].to_vec());
                }
            }
            wait *= 2;
        }
        None
    }

    fn pcp_request(&self, mapping: Mapping, lifetime: u32) -> [u8; pcp::REQUEST_BYTES] {
        let suggested = self.local.is_ipv6().then_some(self.local);
        pcp::map_request(
            self.local,
            lifetime,
            &self.nonce,
            protocol_number(mapping.protocol),
            mapping.port,
            mapping.port,
            suggested,
        )
    }

    fn natpmp_request(mapping: Mapping, lifetime: u32) -> [u8; 12] {
        let protocol = if mapping.protocol == Protocol::TCP {
            natpmp::Protocol::Tcp
        } else {
            natpmp::Protocol::Udp
        };
        let external = if lifetime == 0 { 0 } else { mapping.port };
        natpmp::map_request(protocol, mapping.port, external, lifetime)
    }

    async fn pcp(
        &mut self,
        mapping: Mapping,
        lifetime: u32,
    ) -> Result<std::result::Result<pcp::MapResponse, pcp::Error>> {
        let protocol = protocol_number(mapping.protocol);
        let request = self.pcp_request(mapping, lifetime);
        let nonce = self.nonce;
        let reply = self
            .exchange(&request, |reply| match pcp::parse_map(reply) {
                Ok(response) if reply.len() < pcp::REQUEST_BYTES => response.result != 0,
                Ok(response) => {
                    response.nonce == nonce
                        && response.protocol == protocol
                        && response.internal_port == mapping.port
                }
                // A NAT-PMP router's answer to a PCP request.
                Err(pcp::Error::Version(0)) => true,
                Err(_) => false,
            })
            .await
            .context("the router does not answer PCP or NAT-PMP")?;
        Ok(pcp::parse_map(&reply))
    }

    async fn natpmp(
        &mut self,
        mapping: Mapping,
        lifetime: u32,
    ) -> Result<std::result::Result<natpmp::Response, natpmp::Error>> {
        let protocol = if mapping.protocol == Protocol::TCP {
            natpmp::Protocol::Tcp
        } else {
            natpmp::Protocol::Udp
        };
        let request = Self::natpmp_request(mapping, lifetime);
        let reply = self
            .exchange(&request, |reply| match natpmp::parse(reply) {
                Ok(natpmp::Response::Mapping {
                    protocol: answered,
                    internal,
                    ..
                }) => answered == protocol && internal == mapping.port,
                Err(natpmp::Error::Refused(_)) => reply.get(1) == Some(&(0x80 | request[1])),
                _ => false,
            })
            .await
            .context("the router does not answer NAT-PMP")?;
        Ok(natpmp::parse(&reply))
    }

    async fn natpmp_address(&mut self) -> bool {
        let reply = self
            .exchange(&natpmp::address_request(), |reply| {
                matches!(natpmp::parse(reply), Ok(natpmp::Response::Address { .. }))
            })
            .await;
        if let Some(Ok(natpmp::Response::Address { epoch, address })) =
            reply.as_deref().map(natpmp::parse)
        {
            self.external = Some(IpAddr::V4(address));
            return self.observe_epoch(epoch, Instant::now());
        }
        false
    }

    /// Maps (or renews) each wanted port whose lease is due, and removes leases no longer
    /// wanted. Says which of those deletes the router answered for, and why an unanswered
    /// router ended the round.
    pub(super) async fn renew(&mut self, wanted: &[Mapping]) -> Outcome {
        let now = Instant::now();
        let mut outcome = Outcome::default();
        let gone: Vec<Mapping> = self
            .leases
            .iter()
            .map(|lease| lease.mapping)
            .filter(|mapping| !wanted.contains(mapping))
            .collect();
        for mapping in gone {
            if self.unmap(mapping).await {
                outcome.removed.push(mapping);
            }
        }
        let was_natpmp = self.method == Some(Method::NatPmp);
        if was_natpmp {
            // Refresh the external IPv4 on every round and notice NAT-PMP reboots
            // even when every cached lease is not due yet.
            let _ = self.natpmp_address().await;
        }
        loop {
            let mut rebooted = false;
            for &mapping in wanted {
                if outcome.renewed.contains(&mapping) {
                    continue;
                }
                if let Some(lease) = self
                    .leases
                    .iter()
                    .find(|lease| lease.mapping == mapping && lease.renew_at > now)
                {
                    outcome.mapped.push((mapping, lease.external));
                    continue;
                }
                match self.map(mapping).await {
                    Ok((external, lifetime, epoch_reset)) => {
                        rebooted |= epoch_reset;
                        self.leases.retain(|lease| lease.mapping != mapping);
                        self.leases.push(Lease {
                            mapping,
                            external,
                            renew_at: now + Duration::from_secs(u64::from(lifetime.max(120)) / 2),
                        });
                        outcome.mapped.push((mapping, external));
                        outcome.renewed.push(mapping);
                    }
                    Err(error) => {
                        // No answer at all is the router's, not this port's: stop asking. A
                        // mapping it did not answer for may still be held, so only a refusal
                        // means this host no longer has it.
                        let silent = error.to_string().contains("does not answer");
                        if self.method.is_none() || (outcome.mapped.is_empty() && silent) {
                            outcome.error = Some(error);
                            return outcome;
                        }
                        if !silent {
                            outcome.refused.push(mapping);
                        }
                        outcome.failed.push((mapping, format!("{error:#}")));
                    }
                }
            }
            if !rebooted {
                break;
            }
            // Drop any previously cached-valid entries from this round and remap them now
            // that `self.leases` was cleared by the router reboot.
            outcome
                .mapped
                .retain(|(mapping, _)| outcome.renewed.contains(mapping));
        }
        if !was_natpmp && self.method == Some(Method::NatPmp) {
            let _ = self.natpmp_address().await;
        }
        outcome
    }

    /// One mapping: its external port, granted lifetime, and whether the router rebooted.
    async fn map(&mut self, mapping: Mapping) -> Result<(u16, u32, bool)> {
        if self.method != Some(Method::NatPmp) {
            let response = self.pcp(mapping, LIFETIME).await?;
            if pcp::wants_natpmp(response.as_ref()) && self.local.is_ipv4() {
                self.method = Some(Method::NatPmp);
            } else {
                let response = response.map_err(|error| {
                    anyhow::anyhow!("the router's PCP answer is unreadable: {error:?}")
                })?;
                self.method = Some(Method::Pcp);
                if response.result != 0 {
                    bail!(
                        "{} (PCP {})",
                        pcp::describe(response.result),
                        response.result
                    );
                }
                if self.local.is_ipv4() {
                    self.external = Some(response.external_address);
                }
                let rebooted = self.observe_epoch(response.epoch, Instant::now());
                return Ok((response.external_port, response.lifetime, rebooted));
            }
        }
        match self.natpmp(mapping, LIFETIME).await? {
            Ok(natpmp::Response::Mapping {
                epoch,
                external,
                lifetime,
                ..
            }) => {
                self.method = Some(Method::NatPmp);
                let rebooted = self.observe_epoch(epoch, Instant::now());
                Ok((external, lifetime, rebooted))
            }
            Err(natpmp::Error::Refused(code)) => {
                self.method = Some(Method::NatPmp);
                bail!("{} (NAT-PMP {code})", natpmp::describe(code))
            }
            other => bail!("the router's NAT-PMP answer is unreadable: {other:?}"),
        }
    }

    /// Asks the router to delete `mapping`, and says whether it answered: one that did not
    /// keeps the mapping until its lease runs out.
    async fn unmap(&mut self, mapping: Mapping) -> bool {
        let removed = match self.method {
            Some(Method::Pcp) => self.pcp(mapping, 0).await.is_ok(),
            Some(Method::NatPmp) => self.natpmp(mapping, 0).await.is_ok(),
            None => true,
        };
        if !removed {
            tracing::debug!(
                port = mapping.port,
                "a PCP or NAT-PMP mapping will expire with its lease"
            );
        }
        self.leases.retain(|lease| lease.mapping != mapping);
        removed
    }

    /// Deletes `mappings` this host left on the router, as one start after an unclean stop does:
    /// a mapping request with lifetime 0 for each, which PCP checks against this host's nonce.
    /// The first request also tells which protocol the router speaks; one that answers neither
    /// ends the round, as asking for every port would only wait as long. Returns the mappings
    /// the router answered for: those deletes are confirmed, the rest may still be held.
    pub(super) async fn delete(&mut self, mappings: &[Mapping]) -> Vec<Mapping> {
        let mut answered = Vec::new();
        for &mapping in mappings {
            if self.method.is_none() {
                match self.pcp(mapping, 0).await {
                    Ok(response) => {
                        if pcp::wants_natpmp(response.as_ref()) && self.local.is_ipv4() {
                            self.method = Some(Method::NatPmp);
                        } else {
                            self.method = Some(Method::Pcp);
                            answered.push(mapping);
                            continue;
                        }
                    }
                    Err(_) => return answered,
                }
            }
            let deleted = match self.method {
                Some(Method::Pcp) => self.pcp(mapping, 0).await.is_ok(),
                Some(Method::NatPmp) => self.natpmp(mapping, 0).await.is_ok(),
                None => return answered,
            };
            if !deleted {
                return answered;
            }
            answered.push(mapping);
        }
        answered
    }

    /// Removes every mapping this mapper made: the host stops. Returns the mappings whose delete
    /// the router answered for.
    pub(super) async fn close(&mut self) -> Vec<Mapping> {
        let mappings: Vec<Mapping> = self.leases.iter().map(|lease| lease.mapping).collect();
        let mut confirmed = Vec::new();
        for mapping in mappings {
            if self.unmap(mapping).await {
                confirmed.push(mapping);
            }
        }
        confirmed
    }
}

impl Drop for PortMapper {
    /// A mapper dropped without [`PortMapper::close`] (its task ended early, or its pinholes
    /// were searched again) still asks the router to delete its leases: once each, without
    /// waiting for an answer.
    fn drop(&mut self) {
        let socket = socket2::SockRef::from(&self.socket);
        for lease in &self.leases {
            let _ = match self.method {
                Some(Method::Pcp) => socket.send(&self.pcp_request(lease.mapping, 0)),
                Some(Method::NatPmp) => socket.send(&Self::natpmp_request(lease.mapping, 0)),
                None => continue,
            };
        }
    }
}

/// The IPv4 default gateway and this PC's address towards it.
pub(super) fn ipv4_route(gateways: &[IpAddr], bind: IpAddr) -> Option<(Ipv4Addr, Ipv4Addr)> {
    let gateway = gateways.iter().find_map(|gateway| match gateway {
        IpAddr::V4(ip) => Some(*ip),
        IpAddr::V6(_) => None,
    })?;
    let local = match bind {
        IpAddr::V4(ip) if !ip.is_unspecified() => ip,
        _ => {
            let socket = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
            socket
                .connect(SocketAddr::new(IpAddr::V4(gateway), pcp::PORT))
                .ok()?;
            match socket.local_addr().ok()?.ip() {
                IpAddr::V4(ip) => ip,
                IpAddr::V6(_) => return None,
            }
        }
    };
    Some((gateway, local))
}

/// This PC's global IPv6 addresses with their interface index, for pinholes.
pub(super) fn ipv6_locals(bind: IpAddr) -> Vec<(Ipv6Addr, u32)> {
    let Ok(interfaces) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    interfaces
        .into_iter()
        .filter(|interface| interface.oper_status == if_addrs::IfOperStatus::Up)
        .filter_map(|interface| match (interface.ip(), interface.index) {
            (IpAddr::V6(ip), Some(index)) if ip.segments()[0] & 0xe000 == 0x2000 => {
                Some((ip, index))
            }
            _ => None,
        })
        .filter(|(ip, _)| bind.is_unspecified() || bind == IpAddr::V6(*ip))
        .take(4)
        .collect()
}

#[cfg(test)]
mod tests;
