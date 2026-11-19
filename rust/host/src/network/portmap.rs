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
    method: Option<Method>,
    /// PCP's mapping nonce ([`nonce`]): the same for every mapper of this host at `local`.
    nonce: [u8; 12],
    leases: Vec<Lease>,
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
        // NAT-PMP knows no IPv6.
        mapper.method = Some(Method::Pcp);
        Ok(mapper)
    }

    pub(super) fn new(socket: UdpSocket, local: IpAddr, secret: &[u8]) -> Self {
        Self {
            socket,
            local,
            method: None,
            nonce: nonce(secret, local),
            leases: Vec::new(),
            external: None,
        }
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
                Ok(response) => {
                    response.result != 0
                        || (response.nonce == nonce
                            && response.protocol == protocol
                            && response.internal_port == mapping.port)
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

    async fn natpmp_address(&mut self) {
        let reply = self
            .exchange(&natpmp::address_request(), |reply| {
                matches!(natpmp::parse(reply), Ok(natpmp::Response::Address { .. }))
            })
            .await;
        if let Some(Ok(natpmp::Response::Address { address, .. })) =
            reply.as_deref().map(natpmp::parse)
        {
            self.external = Some(IpAddr::V4(address));
        }
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
        for &mapping in wanted {
            if let Some(lease) = self
                .leases
                .iter()
                .find(|lease| lease.mapping == mapping && lease.renew_at > now)
            {
                outcome.mapped.push((mapping, lease.external));
                continue;
            }
            match self.map(mapping).await {
                Ok((external, lifetime)) => {
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
        if self.method == Some(Method::NatPmp) && self.external.is_none() {
            self.natpmp_address().await;
        }
        outcome
    }

    /// One mapping: its external port and granted lifetime.
    async fn map(&mut self, mapping: Mapping) -> Result<(u16, u32)> {
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
                return Ok((response.external_port, response.lifetime));
            }
        }
        match self.natpmp(mapping, LIFETIME).await? {
            Ok(natpmp::Response::Mapping {
                external, lifetime, ..
            }) => {
                self.method = Some(Method::NatPmp);
                Ok((external, lifetime))
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

/// The IPv4 default gateway and this PC's address towards it: of [`lan_route`]'s choosing, with
/// `lan` this PC's Ethernet and Wi-Fi addresses.
pub(super) fn ipv4_route(
    gateways: &[IpAddr],
    bind: IpAddr,
    lan: &[IpAddr],
) -> Option<(Ipv4Addr, Ipv4Addr)> {
    let routes: Vec<(Ipv4Addr, Ipv4Addr)> = gateways
        .iter()
        .filter_map(|gateway| match gateway {
            IpAddr::V4(ip) => Some(*ip),
            IpAddr::V6(_) => None,
        })
        .filter_map(|gateway| {
            let local = match bind {
                IpAddr::V4(ip) if !ip.is_unspecified() => ip,
                _ => {
                    // A UDP connect sends nothing; it only picks the route.
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
        })
        .collect();
    lan_route(&routes, lan)
}
/// Of the default `routes` (gateway, this PC's address towards it), the one reached from the
/// first of the `lan` addresses (physical interfaces first), else the first: a VPN's default route
/// may be listed before the LAN router's, and its gateway is not the router that forwards ports
/// to this PC.
pub(super) fn lan_route(
    routes: &[(Ipv4Addr, Ipv4Addr)],
    lan: &[IpAddr],
) -> Option<(Ipv4Addr, Ipv4Addr)> {
    routes
        .iter()
        .filter_map(|route| {
            lan.iter()
                .position(|address| *address == IpAddr::V4(route.1))
                .map(|rank| (rank, *route))
        })
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, route)| route)
        .or_else(|| routes.first().copied())
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
mod tests {
    use super::*;

    /// A start after an unclean stop deletes the guest ports it may have left forwarded. The
    /// first request is PCP with no lifetime; the router's version 0 answer says it speaks
    /// NAT-PMP, whose delete for every guest port asks for lifetime 0 and no external port.
    #[tokio::test]
    async fn a_start_deletes_leftover_guest_leases_with_lifetime_zero() {
        use std::sync::{Arc, Mutex};
        let router = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = router.local_addr().unwrap();
        let seen: Arc<Mutex<Vec<Vec<u8>>>> = Arc::default();
        let log = seen.clone();
        tokio::spawn(async move {
            let mut buffer = [0u8; 128];
            loop {
                let Ok((length, from)) = router.recv_from(&mut buffer).await else {
                    return;
                };
                let request = buffer[..length].to_vec();
                // A NAT-PMP router answers PCP with version 0, and a NAT-PMP mapping request
                // or delete with the internal port asked and no external port.
                let reply = match request[0] {
                    2 => vec![0, 0x81, 0, 1, 0, 0, 0, 0],
                    _ => {
                        let mut reply = vec![0, 0x80 | request[1], 0, 0, 0, 0, 0, 1];
                        reply.extend_from_slice(&request[4..6]);
                        reply.extend_from_slice(&[0, 0]);
                        reply.extend_from_slice(&0u32.to_be_bytes());
                        reply
                    }
                };
                log.lock().unwrap().push(request);
                router.send_to(&reply, from).await.unwrap();
            }
        });
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        socket.connect(address).await.unwrap();
        let mut mapper = PortMapper::new(socket, "127.0.0.1".parse().unwrap(), b"key");
        let guest = [
            Mapping {
                protocol: Protocol::TCP,
                port: 47991,
            },
            Mapping {
                protocol: Protocol::UDP,
                port: 8002,
            },
        ];
        assert_eq!(mapper.delete(&guest).await, guest);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 3, "{seen:?}");
        // One PCP delete, under this host's nonce, with no lifetime and the guest port.
        assert_eq!(seen[0][0], 2);
        assert_eq!(u32::from_be_bytes(seen[0][4..8].try_into().unwrap()), 0);
        assert_eq!(u16::from_be_bytes([seen[0][40], seen[0][41]]), 47991);
        // Then a NAT-PMP delete per guest port: no external port, no lifetime.
        for (request, port, protocol) in [(&seen[1], 47991, 2), (&seen[2], 8002, 1)] {
            assert_eq!(request[0], 0);
            assert_eq!(request[1], protocol);
            assert_eq!(u16::from_be_bytes([request[4], request[5]]), port);
            assert_eq!(u16::from_be_bytes([request[6], request[7]]), 0);
            assert_eq!(u32::from_be_bytes(request[8..12].try_into().unwrap()), 0);
        }
    }

    /// A NAT-PMP-only router on loopback: it refuses PCP with version 0, maps with NAT-PMP and
    /// reports its address.
    #[tokio::test]
    async fn a_natpmp_router_is_used_after_its_pcp_refusal() {
        let router = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = router.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buffer = [0u8; 128];
            loop {
                let Ok((length, from)) = router.recv_from(&mut buffer).await else {
                    return;
                };
                let request = &buffer[..length];
                let reply: Vec<u8> = match request[0] {
                    2 => vec![0, 0x81, 0, 1, 0, 0, 0, 0],
                    _ if request[1] == 0 => vec![0, 128, 0, 0, 0, 0, 0, 1, 203, 0, 113, 9],
                    _ => {
                        let mut reply = vec![0, 0x80 | request[1], 0, 0, 0, 0, 0, 1];
                        reply.extend_from_slice(&request[4..6]);
                        reply.extend_from_slice(&request[4..6]);
                        reply.extend_from_slice(&3600u32.to_be_bytes());
                        reply
                    }
                };
                router.send_to(&reply, from).await.unwrap();
            }
        });
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        socket.connect(address).await.unwrap();
        let mut mapper = PortMapper::new(socket, "127.0.0.1".parse().unwrap(), b"key");
        let wanted = [
            Mapping {
                protocol: Protocol::TCP,
                port: 47984,
            },
            Mapping {
                protocol: Protocol::UDP,
                port: 47998,
            },
        ];
        let outcome = mapper.renew(&wanted).await;
        assert_eq!(mapper.method(), Some(Method::NatPmp));
        assert_eq!(outcome.mapped, vec![(wanted[0], 47984), (wanted[1], 47998)]);
        assert!(outcome.failed.is_empty());
        assert_eq!(mapper.external, Some("203.0.113.9".parse().unwrap()));
        // A second renewal within the lease asks nothing.
        let again = mapper.renew(&wanted).await;
        assert_eq!(again.mapped.len(), 2);
        mapper.close().await;
        assert!(mapper.leases.is_empty());
    }

    /// A PCP router that refuses one port: the other maps, the refusal says why.
    #[tokio::test]
    async fn a_pcp_refusal_names_its_reason() {
        let router = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = router.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buffer = [0u8; 128];
            loop {
                let Ok((length, from)) = router.recv_from(&mut buffer).await else {
                    return;
                };
                let request = buffer[..length].to_vec();
                let port = u16::from_be_bytes([request[40], request[41]]);
                let mut reply = vec![2, 0x81, 0, if port == 47998 { 8 } else { 0 }];
                reply.extend_from_slice(&7200u32.to_be_bytes());
                reply.extend_from_slice(&[0, 0, 0, 1]);
                reply.extend_from_slice(&[0; 12]);
                reply.extend_from_slice(&request[24..44]);
                reply.extend_from_slice(&Ipv4Addr::new(203, 0, 113, 9).to_ipv6_mapped().octets());
                router.send_to(&reply, from).await.unwrap();
            }
        });
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        socket.connect(address).await.unwrap();
        let mut mapper = PortMapper::new(socket, "127.0.0.1".parse().unwrap(), b"key");
        let wanted = [
            Mapping {
                protocol: Protocol::TCP,
                port: 47984,
            },
            Mapping {
                protocol: Protocol::UDP,
                port: 47998,
            },
        ];
        let outcome = mapper.renew(&wanted).await;
        assert_eq!(mapper.method(), Some(Method::Pcp));
        assert_eq!(outcome.mapped, vec![(wanted[0], 47984)]);
        assert_eq!(outcome.failed.len(), 1);
        assert!(
            outcome.failed[0].1.contains("out of mappings"),
            "{:?}",
            outcome.failed
        );
        assert_eq!(mapper.external, Some("203.0.113.9".parse().unwrap()));
    }

    /// A PCP router that holds each mapping for the nonce that made it, as RFC 6887 asks: a
    /// mapper made after the first one went (a lost renewal, a host restart) renews the same
    /// ports, and a dropped mapper deletes its leases.
    #[tokio::test]
    async fn a_new_mapper_renews_what_an_earlier_one_mapped() {
        use std::{
            collections::HashMap,
            sync::{Arc, Mutex},
        };
        let router = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = router.local_addr().unwrap();
        // Internal port and protocol to (nonce, lifetime).
        type Held = HashMap<(u16, u8), ([u8; 12], u32)>;
        let held: Arc<Mutex<Held>> = Arc::default();
        let table = held.clone();
        tokio::spawn(async move {
            let mut buffer = [0u8; 128];
            loop {
                let Ok((length, from)) = router.recv_from(&mut buffer).await else {
                    return;
                };
                let request = buffer[..length].to_vec();
                let lifetime = u32::from_be_bytes(request[4..8].try_into().unwrap());
                let nonce: [u8; 12] = request[24..36].try_into().unwrap();
                let key = (u16::from_be_bytes([request[40], request[41]]), request[36]);
                let result = {
                    let mut table = table.lock().unwrap();
                    match table.get(&key) {
                        Some((held, _)) if *held != nonce => 2,
                        _ if lifetime == 0 => {
                            table.remove(&key);
                            0
                        }
                        _ => {
                            table.insert(key, (nonce, lifetime));
                            0
                        }
                    }
                };
                let mut reply = vec![2, 0x81, 0, result];
                reply.extend_from_slice(&lifetime.to_be_bytes());
                reply.extend_from_slice(&[0, 0, 0, 1]);
                reply.extend_from_slice(&[0; 12]);
                reply.extend_from_slice(&request[24..44]);
                reply.extend_from_slice(&Ipv4Addr::new(203, 0, 113, 9).to_ipv6_mapped().octets());
                router.send_to(&reply, from).await.unwrap();
            }
        });
        let mapper = move || async move {
            let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            socket.connect(address).await.unwrap();
            PortMapper::new(socket, "127.0.0.1".parse().unwrap(), b"key")
        };
        let wanted = [Mapping {
            protocol: Protocol::UDP,
            port: 47998,
        }];
        let mut first = mapper().await;
        assert_eq!(first.renew(&wanted).await.mapped.len(), 1);
        // The first mapper is forgotten without a word (its renewal got no answer, or the host
        // crashed); the next one still renews the port.
        first.leases.clear();
        drop(first);
        let mut second = mapper().await;
        let outcome = second.renew(&wanted).await;
        assert_eq!(
            outcome.mapped,
            vec![(wanted[0], 47998)],
            "{:?}",
            outcome.failed
        );
        // Another host's nonce is refused, so the router does hold the rule.
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        socket.connect(address).await.unwrap();
        let mut other = PortMapper::new(socket, "127.0.0.1".parse().unwrap(), b"other");
        assert_eq!(other.renew(&wanted).await.failed.len(), 1);
        other.leases.clear();
        // Dropped with its lease, the second mapper deletes it.
        drop(second);
        for _ in 0..50 {
            if held.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(held.lock().unwrap().is_empty());
    }
}
