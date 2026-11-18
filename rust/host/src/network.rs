use crate::state::Shared;
use anyhow::Result;
use axum::http::{HeaderMap, Uri, header, uri::Authority};
use butterpollo_core::config::{Config, Ports};
use igd_next::{
    PortMappingProtocol as Protocol,
    aio::{Gateway, Provider},
};
use mdns_sd::{IfKind, ServiceDaemon, ServiceInfo};
use socket2::{Domain, Socket, Type};
use std::{
    net::{IpAddr, SocketAddr, UdpSocket},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
#[cfg(test)]
mod fixtures;
mod ipv6;
mod portmap;
mod upnp;

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
            butterpollo_windows::net::host_name().as_deref(),
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
    let addresses = butterpollo_windows::net::lan_addresses().unwrap_or_else(|error| {
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
#[cfg(not(test))]
fn default_gateways() -> Result<Vec<IpAddr>> {
    butterpollo_windows::net::default_gateways()
}
/// The router of the test fixtures' 192.168.4.0/24, rather than this PC's.
#[cfg(test)]
pub const TEST_GATEWAY: &str = "192.168.4.1";
#[cfg(test)]
thread_local! {
    /// How often this thread read the gateways.
    static GATEWAY_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
#[cfg(test)]
fn default_gateways() -> Result<Vec<IpAddr>> {
    GATEWAY_READS.with(|reads| reads.set(reads.get() + 1));
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
    let name = butterpollo_windows::net::host_name()?;
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
            butterpollo_core::config::fallback("address_family", other, "ipv4");
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
pub struct Discovery(ServiceDaemon);
impl Discovery {
    pub fn start(config: &Config, bind: IpAddr) -> Result<Option<Self>> {
        if !config.boolean("enable_discovery", true) || bind.is_loopback() {
            return Ok(None);
        }
        let daemon = ServiceDaemon::new()?;
        let result = (|| -> Result<()> {
            daemon.set_ip_check_interval(30)?;
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
        Ok(Some(Self(daemon)))
    }
}
impl Drop for Discovery {
    fn drop(&mut self) {
        let _ = self.0.shutdown();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Mapping {
    protocol: Protocol,
    port: u16,
}
fn mappings(config: &Config, ports: Ports) -> Vec<Mapping> {
    let mut mappings = vec![
        Mapping {
            protocol: Protocol::TCP,
            port: ports.http,
        },
        Mapping {
            protocol: Protocol::TCP,
            port: ports.https,
        },
        Mapping {
            protocol: Protocol::TCP,
            port: ports.rtsp,
        },
        Mapping {
            protocol: Protocol::UDP,
            port: ports.video,
        },
        Mapping {
            protocol: Protocol::UDP,
            port: ports.control,
        },
        Mapping {
            protocol: Protocol::UDP,
            port: ports.audio,
        },
    ];
    if config.boolean("stream_mic", true) {
        mappings.push(Mapping {
            protocol: Protocol::UDP,
            port: ports.mic,
        });
    }
    if config.get("origin_web_ui_allowed", "lan") == "wan" {
        mappings.push(Mapping {
            protocol: Protocol::TCP,
            port: ports.web,
        });
    }
    mappings
}
/// Browser guests' ports (docs/browser-guests.md): the guest page on its own port, and one media
/// port per guest seat. Never the console's port, the WebRTC signalling port or the owner's own
/// media port.
fn guest_ports(config: &Config, ports: Ports) -> Vec<Mapping> {
    let media = config.webrtc_media_port();
    // A media port moved next to one of these (`webrtc_media_port = 7999` would reach UDP 8000
    // and 8001) leaves that seat unforwarded rather than open what must stay closed.
    let never = [
        ports.web,
        config.webrtc_port(),
        media,
        WEBRTC_DISCOVERY_PORT,
        WEBRTC_DISCOVERY_PORT + 1,
    ];
    std::iter::once(Mapping {
        protocol: Protocol::TCP,
        port: ports.guest,
    })
    .chain(
        (0..crate::webrtc::guests::MAX_GUESTS)
            .filter_map(|slot| media.checked_add(1 + slot))
            .map(|port| Mapping {
                protocol: Protocol::UDP,
                port,
            }),
    )
    .filter(|mapping| !never.contains(&mapping.port))
    .collect()
}
/// [`guest_ports`] while the WebRTC server that serves them is on. The cleanup at host start
/// uses [`guest_ports`] itself: a stop can leave them behind after the setting was turned off.
fn guest_mappings(config: &Config, ports: Ports) -> Vec<Mapping> {
    if !config.webrtc_server() {
        return Vec::new();
    }
    guest_ports(config, ports)
}
/// The WebRTC TV protocol's discovery port, UDP 8000 whatever the settings say; the owner's own
/// stream's media port defaults to the next one.
const WEBRTC_DISCOVERY_PORT: u16 = 8000;
/// The ports to map now: the streaming ports while `upnp` is on, and the browser guests' only
/// while a link needs them: a link made for the internet (whatever `upnp` says), or with `upnp`
/// on any link that works or guest who watches (`guests_open`).
fn wanted_now(
    config: &Config,
    ports: Ports,
    upnp: bool,
    guests_open: bool,
    guests_internet: bool,
) -> (Vec<Mapping>, Vec<Mapping>) {
    let stream = if upnp {
        mappings(config, ports)
    } else {
        Vec::new()
    };
    let guest = if guests_internet || (upnp && guests_open) {
        guest_mappings(config, ports)
    } else {
        Vec::new()
    };
    (stream, guest)
}
/// The router's mapping table, and whether it was read to the end. Routers
/// answer the index past the end with 713, but some hide their table (606),
/// answer 714, 402 or 501, or time out: that ends the list without stopping
/// the host from adding its own mappings.
async fn entries<P: Provider>(gateway: &Gateway<P>) -> (Vec<igd_next::PortMappingEntry>, bool) {
    let mut entries = Vec::new();
    for index in 0..4096 {
        match tokio::time::timeout(
            Duration::from_secs(3),
            gateway.get_generic_port_mapping_entry(index),
        )
        .await
        {
            Ok(Ok(entry)) => entries.push(entry),
            Ok(Err(igd_next::GetGenericPortMappingEntryError::SpecifiedArrayIndexInvalid)) => {
                return (entries, true);
            }
            Ok(Err(error)) => {
                tracing::debug!(%error, index, "UPnP router stopped listing its mappings");
                return (entries, false);
            }
            Err(_) => {
                tracing::debug!(index, "UPnP router timed out listing its mappings");
                return (entries, false);
            }
        }
    }
    tracing::debug!("UPnP router mapping table exceeds its limit");
    (entries, false)
}
fn owned(entry: &igd_next::PortMappingEntry, local: IpAddr, description: &str) -> bool {
    entry.internal_client.parse::<IpAddr>().ok() == Some(local)
        && entry.port_mapping_description == description
        && entry.internal_port == entry.external_port
}
/// Whether another mapping already forwards this port to the same port on
/// this PC, as one left by another streaming host does: clients reach the
/// host through it, so it is not a conflict.
fn forwards_here(entry: &igd_next::PortMappingEntry, local: IpAddr) -> bool {
    entry.internal_client.parse::<IpAddr>().ok() == Some(local)
        && entry.internal_port == entry.external_port
}
/// The router's entry for `mapping`, whichever application owns it.
fn entry<'a>(
    entries: &'a [igd_next::PortMappingEntry],
    mapping: &Mapping,
) -> Option<&'a igd_next::PortMappingEntry> {
    entries
        .iter()
        .find(|entry| entry.protocol == mapping.protocol && entry.external_port == mapping.port)
}
/// Whether a mapping is this host's to take down: one under its description, or one an
/// unreadable table cannot show, which stays this host's until the table shows another owner.
fn ours(
    entries: &[igd_next::PortMappingEntry],
    complete: bool,
    local: IpAddr,
    description: &str,
    mapping: &Mapping,
) -> bool {
    match entry(entries, mapping) {
        Some(entry) => owned(entry, local, description),
        None => !complete,
    }
}
/// This PC's address as the router sees it, for the mappings `gateway` holds.
fn local_for(gateway: &Gateway<upnp::Bounded>, bind: IpAddr) -> Result<IpAddr> {
    let socket = UdpSocket::bind((IpAddr::from([0, 0, 0, 0]), 0))?;
    socket.connect(gateway.addr)?;
    Ok(if bind.is_unspecified() || bind.is_ipv6() {
        socket.local_addr()?.ip()
    } else {
        bind
    })
}
/// The UPnP gateway on this network, and this PC's address towards it.
async fn upnp_gateway(bind: IpAddr) -> Result<(Gateway<upnp::Bounded>, IpAddr)> {
    let gateway = upnp::discover(igd_next::SearchOptions {
        timeout: Some(Duration::from_secs(3)),
        bind_addr: if bind.is_ipv4() {
            (bind, 0).into()
        } else {
            (IpAddr::from([0, 0, 0, 0]), 0).into()
        },
        ..Default::default()
    })
    .await?;
    let local = local_for(&gateway, bind)?;
    Ok((gateway, local))
}
/// `reported` holds the ports already warned about: the router is asked
/// again every minute, but a continuing conflict is only warned once.
/// `previous` is what the last round left mapped: a router that hides its table still gets the
/// ones no longer wanted removed.
/// Returns the mappings this host holds, and the ports another mapping already forwards to this
/// PC under the same number: reachable, but never this host's to take down.
async fn apply<P: Provider>(
    gateway: &Gateway<P>,
    local: IpAddr,
    wanted: &[Mapping],
    description: &str,
    reported: &mut Vec<Mapping>,
    previous: &[Mapping],
) -> Result<(Vec<Mapping>, Vec<Mapping>)> {
    let (entries, complete) = entries(gateway).await;
    // The stable host description lets a restarted host reclaim its own
    // permanent leases, including ports removed from a later configuration.
    let mut applied: Vec<_> = entries
        .iter()
        .filter(|entry| owned(entry, local, description))
        .map(|entry| Mapping {
            protocol: entry.protocol,
            port: entry.external_port,
        })
        .collect();
    let mut forwarded = Vec::new();
    // A table read to the end shows which of them are still this host's; an unreadable one
    // can't, so they stay this host's, as `remove` takes them at shutdown.
    if !complete {
        for mapping in previous {
            if !applied.contains(mapping) {
                applied.push(*mapping);
            }
        }
    }
    for mapping in wanted {
        if let Some(entry) = entries.iter().find(|entry| {
            entry.protocol == mapping.protocol
                && entry.external_port == mapping.port
                && !owned(entry, local, description)
        }) {
            if forwards_here(entry, local) {
                // Another mapping already forwards the port to this PC, as a manual forward
                // does: guests reach it at its own number, so it counts as mapped, and the
                // mapping is not this host's to renew or take down.
                tracing::debug!(port=mapping.port, protocol=?mapping.protocol, owner=%entry.port_mapping_description, "UPnP port already forwards to this PC under another mapping");
                reported.retain(|reported| reported != mapping);
                if !forwarded.contains(mapping) {
                    forwarded.push(*mapping);
                }
            } else if reported.contains(mapping) {
                tracing::debug!(port=mapping.port, protocol=?mapping.protocol, "UPnP port is still owned by another mapping");
            } else {
                reported.push(*mapping);
                tracing::warn!(port=mapping.port, protocol=?mapping.protocol, client=%entry.internal_client, owner=%entry.port_mapping_description, "UPnP port is already owned by another mapping; remove it on the router so clients outside your network reach this PC");
            }
            continue;
        }
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            gateway.add_port(
                mapping.protocol,
                mapping.port,
                (local, mapping.port).into(),
                120,
                description,
            ),
        )
        .await;
        let result = if matches!(
            result,
            Ok(Err(igd_next::AddPortError::OnlyPermanentLeasesSupported))
        ) {
            tokio::time::timeout(
                Duration::from_secs(3),
                gateway.add_port(
                    mapping.protocol,
                    mapping.port,
                    (local, mapping.port).into(),
                    0,
                    description,
                ),
            )
            .await
        } else {
            result
        };
        match result {
            Ok(Ok(())) => {
                reported.retain(|reported| reported != mapping);
                if !applied.contains(mapping) {
                    applied.push(*mapping);
                }
            }
            Ok(Err(igd_next::AddPortError::PortInUse)) => {
                if !reported.contains(mapping) {
                    reported.push(*mapping);
                    tracing::warn!(port=mapping.port, protocol=?mapping.protocol, "UPnP port is already owned by another mapping; remove it on the router so clients outside your network reach this PC");
                }
            }
            Ok(Err(error)) => tracing::warn!(%error,port=mapping.port,"UPnP mapping failed"),
            Err(error) => tracing::warn!(%error,port=mapping.port,"UPnP mapping timed out"),
        }
    }
    // This host's mappings that are no longer wanted (the guests' once their links are gone, or
    // a port a later configuration dropped) go now; one the router keeps is tried at shutdown.
    let unwanted: Vec<Mapping> = applied
        .iter()
        .filter(|mapping| !wanted.contains(mapping))
        .copied()
        .collect();
    for mapping in unwanted {
        // A mapping the router now shows under another owner is not this host's: it stays on
        // the router, and stops counting here now that nothing wants it.
        if !ours(&entries, complete, local, description, &mapping) {
            applied.retain(|held| *held != mapping);
            continue;
        }
        match tokio::time::timeout(
            Duration::from_secs(3),
            gateway.remove_port(mapping.protocol, mapping.port),
        )
        .await
        {
            Ok(Ok(())) => applied.retain(|held| *held != mapping),
            Ok(Err(error)) => {
                tracing::debug!(%error, port = mapping.port, "UPnP mapping no longer wanted could not be removed");
            }
            Err(_) => {
                tracing::debug!(
                    port = mapping.port,
                    "UPnP router timed out removing a mapping"
                );
                break;
            }
        }
    }
    Ok((applied, forwarded))
}
async fn remove<P: Provider>(
    gateway: &Gateway<P>,
    local: IpAddr,
    applied: &[Mapping],
    description: &str,
) -> Result<()> {
    let (entries, complete) = entries(gateway).await;
    // A mapping the router would not remove must not keep the later ones open:
    // all are tried, and the first failure is the one reported. A router that
    // stopped answering ends it: the next removal would wait as long, and the
    // host's shutdown gives this task ten seconds.
    let mut failed = None;
    for mapping in applied {
        // A mapping this host added that an unreadable table cannot show is
        // still removed; one the table shows under another owner is not.
        if ours(&entries, complete, local, description, mapping) {
            let removed = tokio::time::timeout(
                Duration::from_secs(3),
                gateway.remove_port(mapping.protocol, mapping.port),
            )
            .await;
            let error = match removed {
                Ok(Ok(())) => continue,
                Ok(Err(error)) => anyhow::Error::from(error),
                Err(error) => {
                    failed.get_or_insert(anyhow::Error::from(error));
                    break;
                }
            };
            if failed.is_none() {
                failed = Some(error);
            }
        }
    }
    failed.map_or(Ok(()), Err)
}

/// The router's entries for `guest` that carry this host's description: what an unclean stop
/// left forwarded.
fn guest_leftovers(
    entries: &[igd_next::PortMappingEntry],
    local: IpAddr,
    description: &str,
    guest: &[Mapping],
) -> Vec<Mapping> {
    guest
        .iter()
        .filter(|mapping| {
            entries.iter().any(|entry| {
                entry.protocol == mapping.protocol
                    && entry.external_port == mapping.port
                    && owned(entry, local, description)
            })
        })
        .copied()
        .collect()
}

/// Takes the guest ports an earlier run left on `gateway` down: only this host's own entries,
/// so the streaming ports and another application's forwards stay.
async fn remove_guest_leftovers<P: Provider>(
    gateway: &Gateway<P>,
    local: IpAddr,
    guest: &[Mapping],
    description: &str,
) {
    let (entries, _) = entries(gateway).await;
    for mapping in guest_leftovers(&entries, local, description, guest) {
        match tokio::time::timeout(
            Duration::from_secs(3),
            gateway.remove_port(mapping.protocol, mapping.port),
        )
        .await
        {
            Ok(Ok(())) => {
                tracing::debug!(
                    port = mapping.port,
                    "guest port left by an earlier run was removed"
                );
            }
            Ok(Err(error)) => {
                tracing::debug!(%error, port = mapping.port, "guest port left by an earlier run could not be removed");
            }
            Err(_) => {
                tracing::debug!(
                    port = mapping.port,
                    "UPnP router timed out removing a guest port left by an earlier run"
                );
                break;
            }
        }
    }
}

/// One round at host start: takes the guest ports an unclean stop left forwarded down. Links do
/// not survive a restart, so nothing wants them, and the first renewal asks for what it needs.
/// UPnP entries go only under this host's description; PCP and NAT-PMP mappings go with a
/// mapping request of lifetime 0, which PCP checks against this host's nonce.
async fn cleanup_guests(bind: IpAddr, config: &Config, description: &str, secret: &[u8]) {
    let Ok(ports) = config.ports() else {
        return;
    };
    let guest = guest_ports(config, ports);
    if guest.is_empty() {
        return;
    }
    match upnp_gateway(bind).await {
        Ok((gateway, local)) => {
            remove_guest_leftovers(&gateway, local, &guest, description).await;
        }
        Err(error) => {
            tracing::debug!(%error, "UPnP gateway unavailable for the guest port cleanup");
        }
    }
    let route = default_gateways()
        .ok()
        .and_then(|gateways| portmap::ipv4_route(&gateways, bind));
    if let Some((gateway, local)) = route
        && let Ok(mut mapper) = portmap::PortMapper::ipv4(gateway, local, secret).await
    {
        mapper.delete(&guest).await;
    }
}
/// The UPnP and IPv6 pinhole task, which sleeps until its next renewal or until
/// it is told that the host stops.
pub struct PortForward {
    task: tokio::task::JoinHandle<()>,
    stop: std::sync::Arc<tokio::sync::Notify>,
}
impl PortForward {
    /// The host stops: after `h.stop` is set, this wakes the task, which takes its
    /// mappings down and ends.
    pub fn stop(&self) {
        self.stop.notify_one();
    }
    /// Waits up to `wait` for the task to take its mappings down, then ends it.
    pub async fn finish(mut self, wait: Duration) {
        if tokio::time::timeout(wait, &mut self.task).await.is_err() {
            self.task.abort();
        }
    }
}
/// Waits until `renewal` or until `stop` is notified, whichever comes first.
async fn until_renewal_or_stop(stop: &tokio::sync::Notify, renewal: Instant) {
    let wait = renewal.saturating_duration_since(Instant::now());
    tokio::select! {
        () = stop.notified() => {}
        () = tokio::time::sleep(wait) => {}
    }
}
fn protocol_name(protocol: Protocol) -> String {
    if protocol == Protocol::TCP {
        "TCP"
    } else {
        "UDP"
    }
    .to_owned()
}
/// Every port the router has mapped for this host: the ones it holds, and the ones another
/// mapping already forwards here under the same number.
fn mapped_ports(applied: &[Mapping], forwarded: &[Mapping]) -> Vec<(Mapping, u16)> {
    applied
        .iter()
        .chain(forwarded)
        .map(|mapping| (*mapping, mapping.port))
        .collect()
}
/// The console's view of a mapping attempt: each wanted port mapped or refused, and why.
fn port_states(
    wanted: &[Mapping],
    mapped: &[(Mapping, u16)],
    refused: &[(Mapping, String)],
) -> (
    Vec<crate::internet_play::PortState>,
    Vec<crate::internet_play::PortState>,
) {
    let state = |mapping: &Mapping, external: Option<u16>, reason: Option<String>| {
        crate::internet_play::PortState {
            protocol: protocol_name(mapping.protocol),
            port: mapping.port,
            external,
            reason,
        }
    };
    let ok = wanted
        .iter()
        .filter_map(|mapping| {
            mapped
                .iter()
                .find(|(done, _)| done == mapping)
                .map(|(_, external)| state(mapping, Some(*external), None))
        })
        .collect();
    let failed = wanted
        .iter()
        .filter(|mapping| !mapped.iter().any(|(done, _)| done == *mapping))
        .map(|mapping| {
            let reason = refused
                .iter()
                .find(|(done, _)| done == mapping)
                .map_or_else(
                    || "the router refused the mapping".to_owned(),
                    |(_, reason)| reason.clone(),
                );
            state(mapping, None, Some(reason))
        })
        .collect();
    (ok, failed)
}
/// The router's IPv6 gateways, for PCP pinholes.
fn ipv6_gateways() -> Vec<std::net::Ipv6Addr> {
    default_gateways()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|gateway| match gateway {
            IpAddr::V6(ip) => Some(ip),
            IpAddr::V4(_) => None,
        })
        .collect()
}
/// What the port mapping task holds on the routers: the UPnP gateway and what it mapped there,
/// the PCP or NAT-PMP mapper, and the IPv6 pinholes.
#[derive(Default)]
struct Held {
    upnp: Option<(Gateway<upnp::Bounded>, IpAddr, Vec<Mapping>)>,
    mapper: Option<portmap::PortMapper>,
    firewall: Option<ipv6::Firewall>,
    pinholes: Vec<portmap::PortMapper>,
}
impl Held {
    fn any(&self) -> bool {
        self.upnp.is_some()
            || self.mapper.is_some()
            || self.firewall.is_some()
            || !self.pinholes.is_empty()
    }
    /// Takes every mapping and pinhole down: nothing is wanted any more, or the host stops.
    async fn release(&mut self, description: &str) {
        if let Some(mut firewall) = self.firewall.take()
            && let Err(error) = firewall.close().await
        {
            tracing::debug!(%error, "IPv6 pinholes will expire after their finite lease");
        }
        for mut mapper in self.pinholes.drain(..) {
            mapper.close().await;
        }
        if let Some(mut mapper) = self.mapper.take() {
            mapper.close().await;
        }
        if let Some((gateway, local, applied)) = self.upnp.take()
            && let Err(error) = remove(&gateway, local, &applied, description).await
        {
            tracing::warn!(%error, "UPnP cleanup failed; permanent mappings remain reclaimable by this host");
        }
    }
}
/// Writes one mapping attempt to the console's status: the streaming ports and the guests' apart,
/// so a guest port the router refuses never makes the streaming ports look unusable.
fn report(
    status: &mut crate::internet_play::MappingStatus,
    stream: &[Mapping],
    guest: &[Mapping],
    mapped: &[(Mapping, u16)],
    refused: &[(Mapping, String)],
) {
    (status.mapped, status.failed) = port_states(stream, mapped, refused);
    (status.guest_mapped, status.guest_failed) = port_states(guest, mapped, refused);
}
/// Maps the ports [`wanted_now`] names, renews them every minute and takes them down when they
/// are no longer wanted or the host stops. Runs with `upnp` on, and with the WebRTC server on for
/// browser guests' links made for the internet, which map only the guest ports.
pub fn port_forward(h: Shared, bind: IpAddr) -> Option<PortForward> {
    let config = h.config.read().unwrap().clone();
    let upnp = config.boolean("upnp", false) && !bind.is_loopback();
    let runs = !bind.is_loopback() && (upnp || config.webrtc_server());
    crate::internet_play::update_mapping(|status| {
        *status = crate::internet_play::MappingStatus {
            enabled: upnp,
            runs,
            ..Default::default()
        };
    });
    if !runs {
        return None;
    }
    let stop = std::sync::Arc::new(tokio::sync::Notify::new());
    let stopped = stop.clone();
    let task = tokio::spawn(async move {
        let ports = config.ports().unwrap();
        let description = format!("Butterpollo Rust {}", h.paired.read().unwrap().unique_id);
        // Makes the PCP mapping nonce, which must survive new mappers and restarts.
        let nonce_secret = h.identity.private_pem.clone();
        // A previous run that stopped uncleanly left the guest ports forwarded, and no link
        // survives a restart: take them down before the first renewal.
        cleanup_guests(bind, &config, &description, nonce_secret.as_bytes()).await;
        let mut next_search = Instant::now();
        let mut held = Held::default();
        let mut reported = Vec::new();
        while !h.stop.load(Ordering::Acquire) {
            if Instant::now() >= next_search {
                next_search = Instant::now() + Duration::from_secs(60);
                // Read before the guests are asked: a request made during this round is
                // served by the next one.
                let round = h.guests.internet_round();
                let (stream, guest) = wanted_now(
                    &config,
                    ports,
                    upnp,
                    h.guests.open(),
                    h.guests.internet_wanted(),
                );
                let wanted: Vec<Mapping> = stream.iter().chain(&guest).copied().collect();
                if wanted.is_empty() {
                    if held.any() {
                        held.release(&description).await;
                        reported.clear();
                    }
                    crate::internet_play::update_mapping(|status| {
                        *status = crate::internet_play::MappingStatus {
                            enabled: upnp,
                            runs,
                            guest_round: round,
                            ..Default::default()
                        };
                    });
                } else {
                    renew(
                        &mut held,
                        bind,
                        &stream,
                        &guest,
                        &wanted,
                        &description,
                        nonce_secret.as_bytes(),
                        &mut reported,
                    )
                    .await;
                    crate::internet_play::update_mapping(|status| status.guest_round = round);
                }
            }
            tokio::select! {
                () = until_renewal_or_stop(&stopped, next_search) => {}
                // A link made or revoked for the internet: map or unmap the guest ports now.
                () = h.guests.mapping_changed() => next_search = Instant::now(),
            }
        }
        held.release(&description).await;
    });
    Some(PortForward { task, stop })
}
/// One round of mapping `wanted` (the streaming ports `stream` and the guests' `guest`): UPnP
/// IGD, else PCP or NAT-PMP, for IPv4, and IPv6 pinholes on a host that listens on IPv6.
#[allow(clippy::too_many_arguments)]
async fn renew(
    held: &mut Held,
    bind: IpAddr,
    stream: &[Mapping],
    guest: &[Mapping],
    wanted: &[Mapping],
    description: &str,
    nonce_secret: &[u8],
    reported: &mut Vec<Mapping>,
) {
    let mut upnp_error = None;
    if held.upnp.is_none() {
        match upnp_gateway(bind).await {
            Ok((gateway, local)) => held.upnp = Some((gateway, local, Vec::new())),
            Err(error) => {
                tracing::debug!(%error, "UPnP gateway unavailable; retrying");
                upnp_error = Some(format!("{error:#}"));
            }
        }
    }
    if let Some((gateway, local, applied)) = held.upnp.as_mut() {
        let mut forwarded = Vec::new();
        match apply(gateway, *local, wanted, description, reported, applied).await {
            Ok((mappings, elsewhere)) => {
                *applied = mappings;
                forwarded = elsewhere;
            }
            Err(error) => tracing::warn!(%error, "UPnP mapping could not be renewed"),
        }
        let external = tokio::time::timeout(Duration::from_secs(3), gateway.get_external_ip())
            .await
            .ok()
            .and_then(|result| result.ok());
        let mapped = mapped_ports(applied, &forwarded);
        let refused: Vec<(Mapping, String)> = reported
            .iter()
            .map(|mapping| {
                (
                    *mapping,
                    "another mapping on the router already uses this port".to_owned(),
                )
            })
            .collect();
        crate::internet_play::update_mapping(|status| {
            status.method = Some("UPnP".into());
            status.external_ipv4 = external;
            report(status, stream, guest, &mapped, &refused);
            status.error = None;
        });
    } else {
        let route = default_gateways()
            .ok()
            .and_then(|gateways| portmap::ipv4_route(&gateways, bind));
        if let Some((gateway, local)) = route {
            // Another router or address: dropping the old mapper asks its router to delete the
            // leases, without waiting on a router that may be gone.
            if held
                .mapper
                .as_ref()
                .is_some_and(|mapper| !mapper.serves(gateway, local))
            {
                held.mapper = None;
            }
            if held.mapper.is_none() {
                held.mapper = portmap::PortMapper::ipv4(gateway, local, nonce_secret)
                    .await
                    .ok();
            }
        }
        let result = match held.mapper.as_mut() {
            Some(mapper) => mapper.renew(wanted).await,
            None => Err(anyhow::anyhow!("no default gateway")),
        };
        match result {
            Ok(outcome) => {
                for (mapping, reason) in &outcome.failed {
                    if !reported.contains(mapping) {
                        reported.push(*mapping);
                        tracing::warn!(port = mapping.port, protocol = ?mapping.protocol, %reason, "port mapping refused");
                    }
                }
                let method = held
                    .mapper
                    .as_ref()
                    .and_then(portmap::PortMapper::method)
                    .map(|method| method.name().to_owned());
                let external = held.mapper.as_ref().and_then(|mapper| mapper.external);
                crate::internet_play::update_mapping(|status| {
                    status.method = method;
                    status.external_ipv4 = external;
                    report(status, stream, guest, &outcome.mapped, &outcome.failed);
                    status.error = None;
                });
            }
            Err(error) => {
                // The mapper and its leases stay for next time: the router may answer then, and
                // the same nonce renews what it mapped.
                let message = format!(
                    "the router answers none of UPnP, PCP and NAT-PMP: turn UPnP on in the router, or forward the ports by hand ({})",
                    upnp_error.as_deref().unwrap_or("no UPnP router found")
                );
                tracing::debug!(error = %format!("{error:#}"), "PCP and NAT-PMP unavailable; retrying");
                crate::internet_play::update_mapping(|status| {
                    status.method = None;
                    report(status, stream, guest, &[], &[]);
                    status.error = Some(message);
                });
            }
        }
    }
    if bind.is_ipv6() {
        renew_pinholes(held, bind, guest, wanted, nonce_secret).await;
    }
}
/// IPv6 pinholes for `wanted` through the router's UPnP IGDv2 firewall, else PCP.
async fn renew_pinholes(
    held: &mut Held,
    bind: IpAddr,
    guest: &[Mapping],
    wanted: &[Mapping],
    nonce_secret: &[u8],
) {
    if held.firewall.is_none() {
        held.firewall = ipv6::discover(bind).await.ok();
    }
    if let Some(firewall) = held.firewall.as_mut() {
        let result = firewall.renew(wanted).await;
        if let Err(error) = &result {
            tracing::debug!(%error, "IPv6 firewall pinholes could not be renewed");
        }
        let pinholes = if result.is_ok() { wanted.len() } else { 0 };
        let guest_ipv6 = (result.is_ok() && !guest.is_empty()).then(|| firewall.local());
        let error = result.err().map(|error| format!("{error:#}"));
        crate::internet_play::update_mapping(|status| {
            status.ipv6_method = Some("UPnP".into());
            status.ipv6_pinholes = pinholes;
            status.ipv6_error = error;
            status.guest_ipv6 = guest_ipv6;
        });
        return;
    }
    if held.pinholes.is_empty() {
        let gateways = ipv6_gateways();
        for (local, index) in portmap::ipv6_locals(bind) {
            for gateway in &gateways {
                if let Ok(mapper) =
                    portmap::PortMapper::ipv6(*gateway, local, index, nonce_secret).await
                {
                    held.pinholes.push(mapper);
                }
            }
        }
    }
    let mut opened = 0;
    let mut last_error = None;
    let mut guest_ipv6 = None;
    for mapper in &mut held.pinholes {
        match mapper.renew(wanted).await {
            Ok(outcome) => {
                opened += outcome.mapped.len();
                let all_guests = guest
                    .iter()
                    .all(|port| outcome.mapped.iter().any(|(done, _)| done == port));
                if !guest.is_empty()
                    && all_guests
                    && let IpAddr::V6(local) = mapper.local()
                {
                    guest_ipv6.get_or_insert(local);
                }
            }
            Err(error) => last_error = Some(format!("{error:#}")),
        }
    }
    if opened == 0 {
        // Search again next time: the router may answer later.
        held.pinholes.clear();
    }
    crate::internet_play::update_mapping(|status| {
        status.ipv6_method = (opened > 0).then(|| "PCP".into());
        status.ipv6_pinholes = opened;
        status.ipv6_error = (opened == 0).then(|| {
            last_error.unwrap_or_else(|| {
                "the router opens no IPv6 pinholes (neither UPnP IGDv2 nor PCP)".to_owned()
            })
        });
        status.guest_ipv6 = guest_ipv6;
    });
}
#[cfg(test)]
mod port_state_tests {
    use super::*;
    #[test]
    fn a_port_another_mapping_forwards_here_is_reported_mapped_at_its_own_number() {
        let forwarded = [Mapping {
            protocol: Protocol::TCP,
            port: 47991,
        }];
        let (ok, failed) = port_states(&forwarded, &mapped_ports(&[], &forwarded), &[]);
        assert_eq!(ok.len(), 1);
        assert_eq!(ok[0].port, 47991);
        assert_eq!(ok[0].external, Some(47991));
        assert!(ok[0].reason.is_none());
        assert!(failed.is_empty());
    }
    #[test]
    fn the_console_sees_every_wanted_port_mapped_or_refused_with_a_reason() {
        let wanted = [
            Mapping {
                protocol: Protocol::TCP,
                port: 47984,
            },
            Mapping {
                protocol: Protocol::UDP,
                port: 47998,
            },
            Mapping {
                protocol: Protocol::UDP,
                port: 48000,
            },
        ];
        let (ok, failed) = port_states(
            &wanted,
            &[(wanted[0], 50000)],
            &[(wanted[1], "the router is out of mappings".into())],
        );
        assert_eq!(ok.len(), 1);
        assert_eq!(ok[0].external, Some(50000));
        assert_eq!(ok[0].protocol, "TCP");
        assert_eq!(failed.len(), 2);
        assert_eq!(
            failed[0].reason.as_deref(),
            Some("the router is out of mappings")
        );
        assert_eq!(
            failed[1].reason.as_deref(),
            Some("the router refused the mapping")
        );
    }
}
#[cfg(test)]
mod tests;
