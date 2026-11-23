//! Pure Wake-on-LAN target parsing, magic packet construction, burst planning,
//! poll cooldown reservation, and resend retry state machines.

use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6};
use std::time::{Duration, Instant};

use rubylight_native_api::wake::Wake;

/// Why a PC without a usable MAC cannot be woken.
pub const NO_MAC: &str = "The PC has no valid Wake-on-LAN MAC address.";
/// A host lists one target per physical adapter; more than this is not a PC's worth.
pub const MAX_MACS: usize = 8;
pub const MAX_BROADCASTS: usize = 16;
/// Minimum interval between background `/wake` + capability polls while a host stays online.
pub const WAKE_POLL_INTERVAL_MS: u64 = 30_000;

/// Constructs the 102-byte magic packet (`0xff` * 6 followed by `mac` * 16).
pub fn magic_packet(mac: &str) -> Result<[u8; 102], String> {
    let bytes = mac.as_bytes();
    if bytes.len() != 17 || !matches!(bytes[2], b':' | b'-') {
        return Err(NO_MAC.into());
    }
    let mut address = [0; 6];
    for (i, octet) in address.iter_mut().enumerate() {
        let offset = i * 3;
        if !bytes[offset..offset + 2].iter().all(u8::is_ascii_hexdigit)
            || (i < 5 && bytes[offset + 2] != bytes[2])
        {
            return Err(NO_MAC.into());
        }
        *octet = u8::from_str_radix(&mac[offset..offset + 2], 16).unwrap();
    }
    let mut packet = [0xff; 102];
    for chunk in packet[6..].as_chunks_mut::<6>().0 {
        chunk.copy_from_slice(&address);
    }
    Ok(packet)
}

/// Echo and discard, where magic packets classically go, the port Moonlight hosts listen on, and
/// the Moonlight ports that follow `http_port`.
pub fn ports(http_port: u16) -> Vec<u16> {
    let http_port = if http_port == 0 { 47989 } else { http_port };
    let mut ports = vec![7, 9, 47009];
    ports.extend(
        [9, 10, 11, 13, 21]
            .into_iter()
            .filter_map(|offset| http_port.checked_add(offset)),
    );
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// `mac` lowercase and colon-separated, or None when it is not a MAC or is the zero MAC a host
/// sends when it could not tell (it wakes nothing).
pub fn normal_mac(mac: &str) -> Option<String> {
    magic_packet(mac).ok()?;
    let normal = mac.replace('-', ":").to_ascii_lowercase();
    (normal != "00:00:00:00:00:00").then_some(normal)
}

/// The MACs typed into a PC's own wake addresses, separated by commas, spaces or lines, as
/// `00:11:22:33:44:55`, `00-11-22-33-44-55` or `001122334455`; each once. None when one of them is
/// not a MAC address, is all zeros, or there are more than [`MAX_MACS`].
pub fn parse_macs(text: &str) -> Option<Vec<String>> {
    let mut macs = Vec::new();
    for token in text
        .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
        .filter(|t| !t.is_empty())
    {
        let spaced;
        let token = if token.len() == 12 && token.bytes().all(|b| b.is_ascii_hexdigit()) {
            spaced = (0..6)
                .map(|i| &token[i * 2..i * 2 + 2])
                .collect::<Vec<_>>()
                .join(":");
            spaced.as_str()
        } else {
            token
        };
        let mac = normal_mac(token)?;
        if macs.contains(&mac) {
            continue;
        }
        if macs.len() >= MAX_MACS {
            return None;
        }
        macs.push(mac);
    }
    Some(macs)
}

/// Parses a strict 4-part dotted decimal IPv4 literal (`1..=3` ASCII digits `<= 255` per octet),
/// allowing leading zeros such as `"192.168.004.255"` -> `[192, 168, 4, 255]`, and rejecting
/// hostnames, whitespace, and non-ASCII text.
pub fn parse_ipv4_literal(literal: &str) -> Option<[u8; 4]> {
    let mut octets = [0u8; 4];
    let mut count = 0usize;
    for part in literal.split('.') {
        if count >= 4 || part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let value = part.parse::<u16>().ok()?;
        if value > 255 {
            return None;
        }
        octets[count] = value as u8;
        count += 1;
    }
    (count == 4).then_some(octets)
}

/// Extracts normalized, bounded `(macs, broadcasts)` from raw primary + target pairs.
pub fn extract_targets<'a>(
    primary: Option<&'a str>,
    target_list: impl IntoIterator<Item = (&'a str, &'a [String])>,
) -> (Vec<String>, Vec<String>) {
    let mut macs = Vec::new();
    let mut broadcasts = Vec::new();
    if let Some(mac) = primary.and_then(normal_mac) {
        macs.push(mac);
    }
    for (mac, target_broadcasts) in target_list {
        if let Some(normal) = normal_mac(mac)
            && macs.len() < MAX_MACS
            && !macs.contains(&normal)
        {
            macs.push(normal);
        }
        for broadcast in target_broadcasts {
            let Some(octets) = parse_ipv4_literal(broadcast) else {
                continue;
            };
            let normal = Ipv4Addr::from(octets).to_string();
            if broadcasts.len() < MAX_BROADCASTS && !broadcasts.contains(&normal) {
                broadcasts.push(normal);
            }
        }
    }
    (macs, broadcasts)
}

/// What a host's `GET /wake` adds to serverinfo's MAC: its adapters' MACs and their subnet
/// broadcasts, well formed and without repeats.
pub fn targets(wake: &Wake) -> (Vec<String>, Vec<String>) {
    extract_targets(
        wake.primary.as_deref(),
        wake.targets
            .iter()
            .map(|t| (t.mac.as_str(), t.broadcasts.as_slice())),
    )
}

/// This device's side of a wake: the subnet broadcast of each IPv4 adapter, and the interface
/// index of each IPv6 one, for `ff02::1` on its link.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Links {
    pub broadcasts: BTreeSet<Ipv4Addr>,
    pub ipv6: BTreeSet<u32>,
}

/// What one wake sends: each payload (one per MAC) to every destination, and to the best-effort ones.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub payloads: Vec<[u8; 102]>,
    pub destinations: BTreeSet<SocketAddr>,
    /// `ff02::1` on each IPv6 link; sends there may all fail without failing the wake.
    pub best_effort: BTreeSet<SocketAddr>,
}

/// Builds a pure Wake-on-LAN transmission plan from already-resolved endpoint IPs.
pub fn build_plan(
    macs: &[&str],
    resolved_endpoints: &[(IpAddr, u16)],
    broadcast_http_ports: &[u16],
    wake_broadcasts: &[&str],
    links: &Links,
) -> Result<Plan, String> {
    let mut normalized_macs: Vec<String> = Vec::new();
    for mac in macs.iter().filter_map(|m| normal_mac(m)) {
        if !normalized_macs.contains(&mac) {
            normalized_macs.push(mac);
        }
    }
    let payloads: Vec<_> = normalized_macs
        .iter()
        .filter_map(|mac| magic_packet(mac).ok())
        .collect();
    if payloads.is_empty() {
        return Err(NO_MAC.into());
    }

    let mut destinations = BTreeSet::new();
    for &(ip, http_port) in resolved_endpoints {
        for port in ports(http_port) {
            destinations.insert(SocketAddr::new(ip, port));
        }
    }

    let mut broadcast_ips = BTreeSet::new();
    broadcast_ips.insert(IpAddr::V4(Ipv4Addr::BROADCAST));
    broadcast_ips.extend(links.broadcasts.iter().copied().map(IpAddr::V4));
    broadcast_ips.extend(
        wake_broadcasts
            .iter()
            .filter_map(|b| parse_ipv4_literal(b).map(|octets| IpAddr::V4(Ipv4Addr::from(octets)))),
    );

    let effective_ports: Vec<u16> = if broadcast_http_ports.is_empty() {
        vec![47989]
    } else {
        broadcast_http_ports.to_vec()
    };
    let mut expanded_ports = BTreeSet::new();
    for http_port in effective_ports {
        expanded_ports.extend(ports(http_port));
    }

    for ip in broadcast_ips {
        for &port in &expanded_ports {
            destinations.insert(SocketAddr::new(ip, port));
        }
    }

    let best_effort = links
        .ipv6
        .iter()
        .flat_map(|scope| {
            expanded_ports.iter().map(move |&port| {
                SocketAddr::V6(SocketAddrV6::new(
                    Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 1),
                    port,
                    0,
                    *scope,
                ))
            })
        })
        .collect();

    Ok(Plan {
        payloads,
        destinations,
        best_effort,
    })
}

/// What [`WakeWait::next`] or [`WakeLaunch::next`] asks for on this tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakeStep {
    /// Nothing yet: the packet went out lately.
    Wait,
    /// Send a wake packet now.
    Send,
    /// The PC answers.
    Online,
    /// The PC did not wake in time.
    TimedOut,
}

/// Clock-agnostic millisecond wake retry state machine (mirrors [`WakeWait`] for JVM callers).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WakeLaunch {
    start_ms: u64,
    timeout_ms: u64,
    resend_ms: u64,
    last_sent_ms: Option<u64>,
}

impl WakeLaunch {
    pub const TIMEOUT_MS: u64 = 90_000;
    pub const RESEND_MS: u64 = 8_000;

    pub fn new(start_ms: u64) -> Self {
        Self::with(start_ms, Self::TIMEOUT_MS, Self::RESEND_MS)
    }

    pub fn with(start_ms: u64, timeout_ms: u64, resend_ms: u64) -> Self {
        Self {
            start_ms,
            timeout_ms,
            resend_ms,
            last_sent_ms: None,
        }
    }

    /// An answering PC wins over everything, including the time that ran out in the same tick.
    pub fn next(&mut self, online: bool, now_ms: u64) -> WakeStep {
        if online {
            return WakeStep::Online;
        }
        if now_ms.saturating_sub(self.start_ms) >= self.timeout_ms {
            return WakeStep::TimedOut;
        }
        if self
            .last_sent_ms
            .is_none_or(|sent| now_ms.saturating_sub(sent) >= self.resend_ms)
        {
            self.last_sent_ms = Some(now_ms);
            return WakeStep::Send;
        }
        WakeStep::Wait
    }
}

/// Waking a PC as a clock-driven decision: send a wake packet at once, send it again every
/// [`WakeWait::RESEND`] while the PC stays away, and give up after [`WakeWait::TIMEOUT`].
pub struct WakeWait {
    start: Instant,
    timeout: Duration,
    resend: Duration,
    last_sent: Option<Instant>,
}

impl WakeWait {
    pub const TIMEOUT: Duration = Duration::from_secs(90);
    pub const RESEND: Duration = Duration::from_secs(8);

    pub fn new(start: Instant) -> Self {
        Self::with(start, Self::TIMEOUT, Self::RESEND)
    }

    pub fn with(start: Instant, timeout: Duration, resend: Duration) -> Self {
        Self {
            start,
            timeout,
            resend,
            last_sent: None,
        }
    }

    /// An answering PC wins over everything, including the time that ran out in the same tick.
    pub fn next(&mut self, online: bool, now: Instant) -> WakeStep {
        if online {
            return WakeStep::Online;
        }
        if now.saturating_duration_since(self.start) >= self.timeout {
            return WakeStep::TimedOut;
        }
        if self
            .last_sent
            .is_none_or(|sent| now.saturating_duration_since(sent) >= self.resend)
        {
            self.last_sent = Some(now);
            return WakeStep::Send;
        }
        WakeStep::Wait
    }
}

/// The PC being woken, if any.
#[derive(Default)]
pub struct Wakes {
    current: Option<Waking>,
    runs: u64,
}

pub struct Waking {
    pub host: u64,
    pub run: u64,
    pub wait: WakeWait,
}

impl Wakes {
    pub fn start(&mut self, host: u64, now: Instant) -> bool {
        if self.current.is_some() {
            return false;
        }
        self.runs += 1;
        self.current = Some(Waking {
            host,
            run: self.runs,
            wait: WakeWait::new(now),
        });
        true
    }

    pub fn current(&mut self) -> Option<&mut Waking> {
        self.current.as_mut()
    }

    pub fn host(&self) -> Option<u64> {
        self.current.as_ref().map(|w| w.host)
    }

    pub fn end(&mut self) {
        self.current = None;
    }

    pub fn failed(&mut self, host: u64, run: u64) -> bool {
        let ends = self
            .current
            .as_ref()
            .is_some_and(|w| w.host == host && w.run == run);
        if ends {
            self.current = None;
        }
        ends
    }
}

/// Pure reservation and cooldown tracker for periodic `/wake` + `/capabilities` polling.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WakeTargetsPollState {
    asks: BTreeMap<String, (u64, u64)>,
    next_token: u64,
}

impl WakeTargetsPollState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reserves an ask token when `online`, `has_native_api`, and the cooldown elapsed since
    /// the last reservation. Going offline clears the host's prior timestamp so returning
    /// online probes immediately.
    pub fn reserve(
        &mut self,
        uuid: &str,
        online: bool,
        has_native_api: bool,
        now_ms: u64,
    ) -> Option<u64> {
        if !online {
            self.asks.remove(uuid);
            return None;
        }
        if !has_native_api {
            return None;
        }
        if let Some(&(_, last_at)) = self.asks.get(uuid)
            && now_ms.saturating_sub(last_at) < WAKE_POLL_INTERVAL_MS
        {
            return None;
        }
        self.next_token = self.next_token.wrapping_add(1).max(1);
        let token = self.next_token;
        self.asks.insert(uuid.to_owned(), (token, now_ms));
        Some(token)
    }

    pub fn is_active(&self, uuid: &str, token: u64) -> bool {
        token != 0 && self.asks.get(uuid).is_some_and(|&(t, _)| t == token)
    }

    pub fn drop_ask(&mut self, uuid: &str, token: u64) {
        if self.is_active(uuid, token) {
            self.asks.remove(uuid);
        }
    }

    pub fn forget(&mut self, uuid: &str) {
        self.asks.remove(uuid);
    }

    pub fn shutdown(&mut self) {
        self.asks.clear();
    }

    pub fn pending_count(&self) -> usize {
        self.asks.len()
    }
}

#[cfg(test)]
mod tests;
