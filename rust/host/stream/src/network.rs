//! Host network trust boundaries, address/socket helpers, mDNS discovery, and router port
//! forwarding orchestration.

use crate::state::Shared;
use anyhow::Result;
use axum::http::{HeaderMap, Uri, header, uri::Authority};
use butterpollo_config::config::{Config, Ports};
use igd_next::{
    PortMappingProtocol as Protocol,
    aio::{Gateway, Provider},
};
use mdns_sd::{IfKind, ServiceDaemon, ServiceInfo};
use serde::{Deserialize, Serialize};
use socket2::{Domain, Socket, Type};
use std::{
    net::{IpAddr, SocketAddr, UdpSocket},
    path::{Path, PathBuf},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

#[cfg(test)]
mod fixtures;
mod forward_loop;
mod ipv6;
mod leases;
mod pcp_pinholes;
mod peer_throttle;
mod portmap;
mod upnp;
mod upnp_forward;

#[cfg(test)]
use forward_loop::{Held, RENEWAL, renew, until_renewal_or_stop};
pub use forward_loop::{PortForward, port_forward};
use leases::Mapping;
#[cfg(test)]
use leases::{
    GUEST_LEASES, GuestLease, LeaseMethod, LeaseRecord, guest_mappings, mapped_ports, mappings,
    now_seconds, port_states, recorded_leases, wanted_now,
};
#[cfg(test)]
use pcp_pinholes::delete_recorded;
pub use peer_throttle::{Throttle, local_network, peer_group};
#[cfg(test)]
use upnp_forward::{
    Applied, apply_round, forwards_here, local_for, owned, pcp_route, remove,
    remove_guest_leftovers, search_order, upnp_gateway,
};

/// Whether a connection or datagram from `peer` came through an internet play tunnel. The tunnel
/// reaches the host's ports from this PC (a loopback address of the device's own, or the address
/// the host listens on), so its peers would otherwise look like programs on this PC. Every trust
/// decision that favours this PC or the local network asks this first: a tunnelled device gets
/// what a device on the internet gets.
pub fn tunnelled(peer: SocketAddr) -> bool {
    rubylight_traverse::peers::is_tunnelled(peer)
}

/// Whether a connection from `peer` is a program on this PC: over loopback, and not through an
/// internet play tunnel.
pub fn from_this_pc(peer: SocketAddr) -> bool {
    peer.ip().to_canonical().is_loopback() && !tunnelled(peer)
}

/// Whether a connection from `peer` to `local` comes from this PC: over loopback, or to one of
/// this PC's own addresses, which it then comes from too. Never through an internet play tunnel.
fn peer_is_this_pc(peer: SocketAddr, local: IpAddr) -> bool {
    if tunnelled(peer) {
        return false;
    }
    let ip = peer.ip().to_canonical();
    ip.is_loopback() || ip == local.to_canonical()
}

/// Whether the browser behind this request runs on this PC. The address can only say it might:
/// nginx, cloudflared, `tailscale serve`, `ssh -L` and `netsh portproxy` on this PC all connect
/// from here for a browser anywhere. So the request's headers can take it back. They only ever
/// restore the stricter behaviour, so a browser that sends them gains nothing.
///
/// What adds no header and shows the browser the same address and port still can't be told apart:
/// `ssh -L 47990:localhost:47990`, or a raw port proxy on the same port, opened at `localhost`.
pub fn browser_on_this_pc(peer: SocketAddr, local: SocketAddr, headers: &HeaderMap) -> bool {
    peer_is_this_pc(peer, local.ip())
        && !request_names_elsewhere(
            headers,
            local,
            butterpollo_win_io::net::host_name().as_deref(),
        )
}

/// Whether a request connected to `local` shows it came through something: a proxy's own header, a
/// `Host` at another port than the one it connected to, or a `Host` or `Origin` that isn't this PC.
/// A browser on this PC that opened the page at the PC's network address connects to that address,
/// so it names `local`. A missing header says nothing. A `Host` without a port means the scheme's
/// default, so it only fits a host listening on 80 or 443. The `Origin`'s port isn't compared: a
/// page may signal to another port than it came from.
fn request_names_elsewhere(headers: &HeaderMap, local: SocketAddr, pc_name: Option<&str>) -> bool {
    let forwarded = headers
        .keys()
        .any(|name| name == "forwarded" || name.as_str().starts_with("x-forwarded-"));
    let host_elsewhere = headers.get_all(header::HOST).iter().any(|host| {
        let authority = host.to_str().ok();
        let authority = authority.and_then(|host| host.parse::<Authority>().ok());
        !authority.is_some_and(|authority| {
            authority
                .port_u16()
                .map_or(matches!(local.port(), 80 | 443), |port| {
                    port == local.port()
                })
                && names_this_pc(authority.host(), local.ip(), pc_name)
        })
    });
    let origin_elsewhere = headers.get_all(header::ORIGIN).iter().any(|origin| {
        let origin = origin.to_str().ok();
        let origin = origin.and_then(|origin| origin.parse::<Uri>().ok());
        !origin
            .as_ref()
            .and_then(Uri::host)
            .is_some_and(|host| names_this_pc(host, local.ip(), pc_name))
    });
    forwarded || host_elsewhere || origin_elsewhere
}

/// Whether a request's host names this PC: loopback, the address the connection reached, or the
/// PC's own name. A longer name (`pc.tailnet.ts.net`, a domain's) may be a proxy's, so it doesn't
/// count, and such a stream keeps the configured layout.
fn names_this_pc(host: &str, local: IpAddr, pc_name: Option<&str>) -> bool {
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if let Ok(address) = host.parse::<IpAddr>() {
        let address = address.to_canonical();
        return address.is_loopback() || address == local.to_canonical();
    }
    host == "localhost"
        || host.ends_with(".localhost")
        || pc_name.is_some_and(|name| {
            let name = name.to_ascii_lowercase();
            host == name || host.strip_suffix(".local") == Some(name.as_str())
        })
}

/// The address a WebRTC stream's media socket binds and offers: the one the browser's signalling
/// reached, which is known to work from there. A browser on this PC that opened the page at
/// `localhost` signals over loopback, and not every browser sends media there: Firefox makes no
/// candidate pair with a loopback address, so its connection fails after 5 s without one check.
/// That browser gets `lan`, this PC's own address, which it reaches without leaving the PC. A host
/// bound to loopback stays there, and so does a PC with no network.
pub fn media_address(
    signalled: IpAddr,
    bind: IpAddr,
    lan: impl FnOnce() -> Option<IpAddr>,
) -> IpAddr {
    let signalled = signalled.to_canonical();
    if signalled.is_loopback() && bind.is_unspecified() {
        return lan().unwrap_or(signalled);
    }
    signalled
}

/// This PC's own address on the network, for [`media_address`]; None without a network.
pub fn this_pc_address() -> Option<IpAddr> {
    let addresses = butterpollo_win_io::net::lan_addresses().unwrap_or_else(|error| {
        tracing::warn!(error = %format!("{error:#}"), "the LAN addresses for a WebRTC stream could not be read");
        Vec::new()
    });
    addresses
        .iter()
        .find_map(|address| address.parse().ok())
        // `lan_addresses` lists IPv4 only. A UDP connect sends nothing; it only picks the route.
        .or_else(|| {
            let probe = UdpSocket::bind((IpAddr::from([0u16; 8]), 0)).ok()?;
            probe.connect(("2001:4860:4860::8888", 9)).ok()?;
            let address = probe.local_addr().ok()?.ip();
            let usable = !address.is_loopback()
                && !address.is_unspecified()
                && !matches!(address, IpAddr::V6(ip) if ip.is_unicast_link_local());
            usable.then_some(address)
        })
}

/// Where a peer is, by address, as Vibepollo classifies it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reach {
    /// This computer.
    Pc,
    /// Private, link-local and carrier-grade NAT (Tailscale) addresses.
    Lan,
    Wan,
}

/// Internet play tunnels' loopback ranges (127.77.0.0/16, 127.78.0.0/16) are the internet: a
/// registered tunnel peer outside them needs its port as well, so callers that have it ask
/// [`tunnelled`] (or [`peer_reach`]) too.
pub fn reach(address: IpAddr) -> Reach {
    match address.to_canonical() {
        ip if rubylight_traverse::peers::tunnel_range(ip) => Reach::Wan,
        IpAddr::V4(ip) if ip.is_loopback() => Reach::Pc,
        IpAddr::V4(ip)
            if ip.is_private()
                || ip.is_link_local()
                || (ip.octets()[0] == 100 && ip.octets()[1] & 0xc0 == 64) =>
        {
            Reach::Lan
        }
        IpAddr::V6(ip) if ip.is_loopback() => Reach::Pc,
        IpAddr::V6(ip) if ip.is_unique_local() || ip.is_unicast_link_local() => Reach::Lan,
        _ => Reach::Wan,
    }
}

/// The farthest peers allowed to use the web console and API.
/// [`reach`] of a connection's or datagram's peer: the internet when it came through a tunnel.
pub fn peer_reach(peer: SocketAddr) -> Reach {
    if tunnelled(peer) {
        Reach::Wan
    } else {
        reach(peer.ip())
    }
}

pub fn web_reach(config: &Config) -> Reach {
    match config.get("origin_web_ui_allowed", "lan") {
        "wan" => Reach::Wan,
        "lan" => Reach::Lan,
        _ => Reach::Pc,
    }
}

/// Carrier-grade NAT space, 100.64.0.0/10: Tailscale's addresses, and also the WAN side of a
/// router behind an ISP's NAT, whose other customers connect from it.
fn shared_address_space(address: IpAddr) -> bool {
    matches!(address.to_canonical(), IpAddr::V4(ip) if ip.octets()[0] == 100 && ip.octets()[1] & 0xc0 == 64)
}

/// Why `/play` refuses a browser at `peer` that reached this host on `local`, if it does, given
/// the PC's default gateways. Only the TCP peer counts. A forwarded guest port can make an
/// internet peer look local: a CGNAT neighbour's 100.64/10 address arrives on the LAN address
/// (real Tailscale traffic arrives on the Tailscale address), and a router that source-NATs its
/// forwards hands over its own address.
pub fn play_refusal(peer: IpAddr, local: IpAddr, gateways: &[IpAddr]) -> Option<&'static str> {
    let peer = peer.to_canonical();
    if reach(peer) > Reach::Lan {
        return Some("the browser is outside the local network");
    }
    if shared_address_space(peer) && !shared_address_space(local) {
        return Some("a 100.64/10 browser reached a non-Tailscale address of this PC");
    }
    if gateways
        .iter()
        .any(|gateway| gateway.to_canonical() == peer)
    {
        return Some("the browser connects from the default gateway, which forwards the internet");
    }
    None
}

/// [`play_refusal`] with this PC's default gateways, read only for a peer the address checks
/// alone let in. A routing table that cannot be read refuses: the gateway check would be skipped.
pub fn play_refused(peer: SocketAddr, local: SocketAddr) -> Option<String> {
    if tunnelled(peer) {
        return Some("the browser is outside the local network".to_owned());
    }
    if let Some(reason) = play_refusal(peer.ip(), local.ip(), &[]) {
        return Some(reason.to_owned());
    }
    match default_gateways() {
        Ok(gateways) => play_refusal(peer.ip(), local.ip(), &gateways).map(str::to_owned),
        Err(error) => Some(format!("the default gateway could not be read: {error:#}")),
    }
}

#[cfg(not(any(test, feature = "test-support")))]
fn default_gateways() -> Result<Vec<IpAddr>> {
    butterpollo_win_io::net::default_gateways()
}
#[cfg(all(not(test), feature = "test-support"))]
fn default_gateways() -> Result<Vec<IpAddr>> {
    Ok(vec![TEST_GATEWAY.parse()?])
}
/// The router of the test fixtures' 192.168.4.0/24, rather than this PC's.
#[cfg(any(test, feature = "test-support"))]
pub const TEST_GATEWAY: &str = "192.168.4.1";
#[cfg(test)]
thread_local! {
    /// How often this thread read the gateways.
    static GATEWAY_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// This thread's stand-in for this PC's interfaces and its network, for the port mapping
    /// task's tests: no test looks for a router on this PC's network.
    static TEST_NET: std::cell::RefCell<TestNet> = std::cell::RefCell::default();
}
/// What the port mapping task reads of this PC and its network, as a test sets it.
#[cfg(test)]
#[derive(Default)]
struct TestNet {
    /// The default gateways; None: [`TEST_GATEWAY`].
    gateways: Option<Vec<IpAddr>>,
    /// This PC's Ethernet and Wi-Fi addresses, as `lan_interfaces` lists them.
    lan: Vec<butterpollo_win_io::net::LanInterface>,
    /// Every address this PC has; None: every address asked about.
    addresses: Option<Vec<IpAddr>>,
    /// The UPnP routers a search from an address finds there, each found once.
    routers: Vec<(IpAddr, Gateway<upnp::Bounded>)>,
    /// The address each UPnP search went out from.
    searches: Vec<IpAddr>,
    /// The address Windows picks towards a gateway, by gateway.
    routes: Vec<(std::net::Ipv4Addr, IpAddr)>,
    /// The address Windows sends multicast from; None: unknown.
    multicast_from: Option<IpAddr>,
    /// The searches under way, and the most at once.
    in_flight: usize,
    overlap: usize,
    /// Extra turns a search from an address waits before it answers.
    delays: Vec<(IpAddr, usize)>,
    /// The addresses whose searches have answered, in that order.
    finished: Vec<IpAddr>,
}
#[cfg(test)]
fn default_gateways() -> Result<Vec<IpAddr>> {
    GATEWAY_READS.with(|reads| reads.set(reads.get() + 1));
    if let Some(gateways) = TEST_NET.with(|net| net.borrow().gateways.clone()) {
        return Ok(gateways);
    }
    Ok(vec![TEST_GATEWAY.parse()?])
}

/// The encryption a client at `peer` needs: `lan_encryption_mode` on this PC and the local
/// network, `wan_encryption_mode` elsewhere, including through an internet play tunnel.
pub fn encryption_mode(config: &Config, peer: SocketAddr) -> u32 {
    let lan = peer_reach(peer) <= Reach::Lan;
    config
        .integer(
            if lan {
                "lan_encryption_mode"
            } else {
                "wan_encryption_mode"
            },
            if lan { 0 } else { 1 },
        )
        .clamp(0, 2) as u32
}

/// The warning for a stream to `peer` whose video and audio are not both encrypted
/// (`encryption`, the negotiated flags: 2 video, 4 audio) under `mode`, the encryption the peer's
/// network gets. Only When the device supports it (1) warns: Off is the user's choice and
/// Required refuses such a stream. The local network, Tailscale included, is not the internet,
/// and a path through Play over the internet is sealed with the device's own keys.
pub fn unencrypted_internet_warning(
    mode: u32,
    peer: SocketAddr,
    encryption: u32,
) -> Option<&'static str> {
    let ip = peer.ip().to_canonical();
    (mode == 1
        && encryption & 6 != 6
        && !tunnelled(peer)
        && !rubylight_traverse::peers::tunnel_range(ip)
        && reach(ip) == Reach::Wan)
        .then_some(
            "This device streams over the internet unencrypted: it cannot encrypt video and audio. Update its Moonlight app, or set Internet encryption to Required to turn such devices away.",
        )
}

pub fn ping_timeout(config: &Config) -> Duration {
    Duration::from_millis(config.integer("ping_timeout", 10000).clamp(1000, 300000) as u64)
}

/// The name clients show for this host: `sunshine_name`, else the PC's name.
pub fn host_name(config: &Config) -> String {
    let configured = config.get("sunshine_name", "").trim();
    if !configured.is_empty() {
        return configured.to_owned();
    }
    pc_name().unwrap_or_else(|| "Rubylight".into())
}

/// The PC's DNS host name, read from Windows once: a new name takes a restart of
/// Windows. Every `/serverinfo` asked for it. A failed read is tried again.
fn pc_name() -> Option<String> {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    if let Some(name) = NAME.get() {
        return Some(name.clone());
    }
    let name = butterpollo_win_io::net::host_name()?;
    Some(NAME.get_or_init(|| name).clone())
}

pub fn bind_address(config: &Config, override_address: Option<IpAddr>) -> Result<IpAddr> {
    if let Some(address) = override_address {
        return Ok(address);
    }
    let configured = config.get("bind_address", "");
    if !configured.is_empty() {
        // A value that is no address at all (an interface name) would keep
        // the host, and the console to fix it, down: this PC only until it is
        // corrected. A valid address that is not up yet, as at boot, is kept:
        // the service retries until the network has it.
        return Ok(
            match configured.trim().trim_matches('"').parse::<IpAddr>() {
                Ok(address) => address,
                Err(_) => {
                    let fallback = IpAddr::from([127, 0, 0, 1]);
                    tracing::warn!(
                        bind_address = configured,
                        %fallback,
                        "bind_address is not an IP address; listening on this PC only"
                    );
                    fallback
                }
            },
        );
    }
    match config.get("address_family", "ipv4") {
        "both" => Ok(IpAddr::from([0u16; 8])),
        other => {
            butterpollo_config::config::fallback("address_family", other, "ipv4");
            Ok(IpAddr::from([0, 0, 0, 0]))
        }
    }
}

fn socket(address: SocketAddr, kind: Type) -> Result<Socket> {
    let socket = Socket::new(Domain::for_address(address), kind, None)?;
    if address.is_ipv6() {
        socket.set_only_v6(false)?;
    }
    socket.bind(&address.into())?;
    socket.set_nonblocking(true)?;
    Ok(socket)
}

pub fn udp(address: SocketAddr) -> Result<UdpSocket> {
    Ok(socket(address, Type::DGRAM)?.into())
}

pub fn tcp(address: SocketAddr) -> Result<tokio::net::TcpListener> {
    let socket = socket(address, Type::STREAM)?;
    socket.listen(128)?;
    Ok(tokio::net::TcpListener::from_std(socket.into())?)
}

/// Accepts the next connection. A failed accept belongs to one connection
/// (a client resetting mid-handshake) or is transient (out of handles), so
/// it never ends the listener, which would shut the host and its game down.
pub async fn accept(listener: &tokio::net::TcpListener) -> (tokio::net::TcpStream, SocketAddr) {
    loop {
        match listener.accept().await {
            Ok((socket, peer)) => {
                return (
                    socket,
                    SocketAddr::new(peer.ip().to_canonical(), peer.port()),
                );
            }
            Err(error) if per_connection(&error) => {
                tracing::debug!(%error, "connection closed before it was accepted");
            }
            Err(error) => {
                tracing::warn!(%error, "accepting a connection failed; retrying");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}

fn per_connection(error: &std::io::Error) -> bool {
    use std::io::ErrorKind::*;
    matches!(
        error.kind(),
        ConnectionReset | ConnectionAborted | ConnectionRefused | Interrupted | WouldBlock
    )
}

/// How often mdns-sd re-reads this PC's interfaces on its own: a fallback for the interface and
/// address change notifications below, which have them re-read within a second of a change
/// instead, and covers a notification that could not be registered or missed one.
const DISCOVERY_FALLBACK: u32 = 30;
/// How soon after a change the notification has mdns-sd re-read the interfaces.
const DISCOVERY_SOON: u32 = 1;

pub struct Discovery {
    /// Windows' interface change notification: `None` when Windows refused it, and the daemon's
    /// own interval is the fallback. Dropped before the daemon, so it wakes nothing shut down.
    watch: Option<butterpollo_win_io::net::InterfaceChanges>,
    daemon: ServiceDaemon,
}
impl Discovery {
    pub fn start(config: &Config, bind: IpAddr) -> Result<Option<Self>> {
        if !config.boolean("enable_discovery", true) || bind.is_loopback() {
            return Ok(None);
        }
        let daemon = ServiceDaemon::new()?;
        let result = (|| -> Result<()> {
            daemon.set_ip_check_interval(DISCOVERY_FALLBACK)?;
            daemon.disable_interface(IfKind::LoopbackV4)?;
            daemon.disable_interface(IfKind::LoopbackV6)?;
            if bind.is_ipv4() {
                daemon.disable_interface(IfKind::IPv6)?;
            }
            if !bind.is_unspecified() {
                daemon.disable_interface(IfKind::All)?;
                daemon.enable_interface(IfKind::Addr(bind))?;
            }
            let hostname = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "butterpollo".into());
            let hostname = format!(
                "{}.local.",
                hostname.trim_end_matches('.').to_ascii_lowercase()
            );
            let instance = host_name(config);
            let instance = instance.as_str();
            let addresses = if bind.is_unspecified() {
                String::new()
            } else {
                bind.to_string()
            };
            let service = ServiceInfo::new(
                "_nvstream._tcp.local.",
                instance,
                &hostname,
                addresses.as_str(),
                config.ports()?.http,
                None,
            )?;
            let service = if bind.is_unspecified() {
                service.enable_addr_auto()
            } else {
                service
            };
            daemon.register(service)?;
            tracing::debug!(%instance, "Moonlight discovery registered");
            Ok(())
        })();
        if let Err(error) = result {
            let _ = daemon.shutdown();
            return Err(error);
        }
        // Windows tells the daemon when an interface or address changed, and it re-reads them
        // within a second of that; the interval set above stays only the fallback. A refused
        // registration leaves the fallback, thirty seconds, as before.
        let watch = butterpollo_win_io::net::InterfaceChanges::register(
            {
                let daemon = daemon.clone();
                move || {
                    let _ = daemon.set_ip_check_interval(DISCOVERY_SOON);
                }
            },
            {
                let daemon = daemon.clone();
                move || {
                    let _ = daemon.set_ip_check_interval(DISCOVERY_FALLBACK);
                }
            },
        );
        if watch.is_none() {
            tracing::warn!(
                "interface change notifications are unavailable; Moonlight discovery re-reads the interfaces on its own timer instead"
            );
        }
        Ok(Some(Self { watch, daemon }))
    }
}
impl Drop for Discovery {
    fn drop(&mut self) {
        // The notification first, so its watcher sends a shut-down daemon nothing.
        self.watch = None;
        let _ = self.daemon.shutdown();
    }
}

#[cfg(test)]
mod tests;
