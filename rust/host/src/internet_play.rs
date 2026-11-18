//! Internet play on the host (`docs/internet-play.md`): what the port mapping did, the public
//! address STUN finds, the DHT rendezvous and hole-punched tunnels for paired devices, the
//! per-device secret that keys them, and `/internet-play/v1`, which hands a paired device on the
//! LAN what it needs to come back from outside.
//!
//! "Play over the internet" (`internet_play`) turns on STUN, the rendezvous and the tunnels.
//! Port mapping keeps following the `upnp` setting, and also asks PCP and NAT-PMP routers.

use crate::state::Shared;
use anyhow::{Context, Result};
use butterpollo_core::config::Config;
use rubylight_traverse::{
    self as traverse,
    host::{Device, HostConfig, HostService},
    record::{Nat, Ports},
};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::Digest;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{LazyLock, Mutex},
    time::Duration,
};

/// The paired-device record field holding the device's rendezvous secret (64 hex digits).
pub const SECRET_FIELD: &str = "internet_play_secret";
/// `RustHostInternetPlayVersion` in serverinfo, and the version of `/internet-play/v1`.
pub const VERSION: u32 = 1;
pub const SERVERINFO_FIELD: &str = "RustHostInternetPlayVersion";
pub const ROUTE: &str = "/internet-play/v1";

pub fn enabled(config: &Config) -> bool {
    config.boolean("internet_play", false)
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PortState {
    pub protocol: String,
    pub port: u16,
    pub external: Option<u16>,
    pub reason: Option<String>,
}

/// What the port mapping task last did, for the console.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MappingStatus {
    pub enabled: bool,
    /// UPnP, PCP or NAT-PMP: whichever the router answered.
    pub method: Option<String>,
    pub external_ipv4: Option<IpAddr>,
    pub mapped: Vec<PortState>,
    pub failed: Vec<PortState>,
    /// Why nothing could be mapped.
    pub error: Option<String>,
    pub ipv6_method: Option<String>,
    pub ipv6_pinholes: usize,
    pub ipv6_error: Option<String>,
    /// Whether the port mapping task runs: `upnp` is on, or the WebRTC server is, for browser
    /// guests' links made for the internet. Not on a host that listens on this PC only.
    pub runs: bool,
    /// The browser guests' ports (docs/browser-guests.md), mapped only while a link needs them.
    pub guest_mapped: Vec<PortState>,
    pub guest_failed: Vec<PortState>,
    /// This PC's IPv6 address whose firewall pinholes let every guest port in.
    pub guest_ipv6: Option<std::net::Ipv6Addr>,
    /// The last request for the guest ports the task has asked the router about
    /// (`Guests::internet_round`).
    pub guest_round: u64,
}

static MAPPING: LazyLock<Mutex<MappingStatus>> =
    LazyLock::new(|| Mutex::new(MappingStatus::default()));
static SERVICE: Mutex<Option<HostService>> = Mutex::new(None);

pub fn update_mapping(change: impl FnOnce(&mut MappingStatus)) {
    change(&mut MAPPING.lock().unwrap());
}

fn mapping() -> MappingStatus {
    MAPPING.lock().unwrap().clone()
}

fn service() -> Option<HostService> {
    SERVICE.lock().unwrap().clone()
}

/// Private, link-local and carrier-grade NAT (100.64/10) addresses: none of them is reachable
/// from the internet.
fn public_ipv4(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(ip) => {
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || (ip.octets()[0] == 100 && ip.octets()[1] & 0xc0 == 64))
        }
        IpAddr::V6(_) => false,
    }
}

/// What holds a router whose own internet address is not public, for the note that says why
/// the forwarded ports are out of reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InFront {
    /// The provider shares one address among its customers: carrier-grade NAT (100.64/10).
    CarrierGrade,
    /// Another router or modem forwards first (192.168/16 and 172.16/12).
    AnotherRouter,
    /// Either: 10/8 is used both inside homes and by providers behind another NAT.
    Either,
}

fn in_front(address: IpAddr) -> InFront {
    match address {
        IpAddr::V4(ip) if ip.octets()[0] == 100 && ip.octets()[1] & 0xc0 == 64 => {
            InFront::CarrierGrade
        }
        IpAddr::V4(ip)
            if (ip.octets()[0] == 192 && ip.octets()[1] == 168)
                || (ip.octets()[0] == 172 && ip.octets()[1] & 0xf0 == 16) =>
        {
            InFront::AnotherRouter
        }
        _ => InFront::Either,
    }
}

/// Why a router whose own address is not public can't be reached from the internet, in plain
/// words, and what to do about it.
fn not_public(address: IpAddr) -> String {
    match in_front(address) {
        InFront::CarrierGrade => "your internet provider shares one address among its customers (carrier-grade NAT), so nothing on the internet can open a port on it".to_owned(),
        InFront::AnotherRouter => "another router or modem is in front of this one, so nothing on the internet can open a port on it; forward the ports there too, or put it in bridge mode".to_owned(),
        InFront::Either => "your internet provider may share one address among its customers (carrier-grade NAT), or another router or modem may be in front of this one, so nothing on the internet can open a port on it; if it is another router or modem, forward the ports there too or put it in bridge mode".to_owned(),
    }
}

/// The public IPv4 address Moonlight stores as the PC's remote address (`ExternalIP`): the
/// router's own address when it is public, else the address STUN servers see.
pub fn external_ipv4() -> Option<IpAddr> {
    let mapped = mapping()
        .external_ipv4
        .filter(|address| public_ipv4(*address));
    mapped.or_else(|| {
        service()?
            .status()
            .public
            .iter()
            .map(SocketAddr::ip)
            .find(|address| public_ipv4(*address))
    })
}

fn nat_name(nat: Nat) -> &'static str {
    match nat {
        Nat::Unknown => "unknown",
        Nat::None => "none",
        Nat::Cone => "cone",
        Nat::Symmetric => "symmetric",
    }
}

/// Plain-English findings across the mapping and STUN: a router behind carrier-grade NAT, or a
/// second NAT in front of the router.
fn notes(mapping: &MappingStatus, stun: &[SocketAddr], nat: Nat) -> Vec<String> {
    let mut notes = Vec::new();
    if let Some(external) = mapping.external_ipv4 {
        if !public_ipv4(external) {
            notes.push(format!(
                "The router's own internet address ({external}) is not public: {}. Devices use a direct internet path instead.",
                not_public(external)
            ));
        } else if let Some(seen) = stun.iter().map(SocketAddr::ip).find(|ip| ip.is_ipv4())
            && seen != external
        {
            notes.push(format!(
                "STUN servers see this PC at {seen}, but the router reports {external}: another router or NAT is in front of it, and forwarded ports may not be reachable."
            ));
        }
    }
    if nat == Nat::Symmetric {
        notes.push(
            "This network gives each destination another port (symmetric NAT): a direct internet path opens only to devices on more open networks."
                .to_owned(),
        );
    }
    notes
}

/// `GET /api/internet-play`, the console's status card.
pub fn console_status(h: &Shared) -> Result<Value> {
    let config = h.config_snapshot();
    let mapping = mapping();
    let rendezvous = service().map(|service| service.status());
    let (public, nat) = rendezvous
        .as_ref()
        .map_or((Vec::new(), Nat::Unknown), |status| {
            (status.public.clone(), status.nat)
        });
    let notes = notes(&mapping, &public, nat);
    Ok(json!({
        "enabled": enabled(&config),
        "portMapping": config.boolean("upnp", false),
        "mapping": mapping,
        "externalIp": external_ipv4(),
        "rendezvous": rendezvous.map(|status| json!({
            "public": status.public,
            "nat": nat_name(status.nat),
            "stunError": status.stun_error,
            "devices": status.devices,
            "published": status.published,
            "dhtError": status.dht_error,
            "paths": status.paths.iter().map(|path| json!({
                "device": path.device,
                "peer": path.peer,
                "connected": path.connected,
            })).collect::<Vec<_>>(),
            "pathErrors": status.path_errors.iter().map(|(device, error)| json!({
                "device": device,
                "error": error,
            })).collect::<Vec<_>>(),
        })),
        "notes": notes,
    }))
}

/// What a browser guest link made for the internet can use (docs/browser-guests.md), for the
/// console's invite dialog: what the router did with the guest ports, the public addresses, and
/// the address the link should carry.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GuestReach {
    /// The router has not been asked yet since the dialog asked for the ports.
    pub checking: bool,
    /// Whether the port mapping task runs at all.
    pub runs: bool,
    /// UPnP, PCP or NAT-PMP: whichever the router answered.
    pub method: Option<String>,
    /// Why nothing could be mapped.
    pub error: Option<String>,
    /// Each guest port, mapped (with its external port) or refused with the reason.
    pub ports: Vec<PortState>,
    /// The public IPv4 address: the router's own when public, else what STUN servers see.
    pub public_ipv4: Option<IpAddr>,
    /// The router's own internet address, public or not.
    pub router_ipv4: Option<IpAddr>,
    /// This PC's IPv6 address whose pinholes let every guest port in.
    pub ipv6: Option<std::net::Ipv6Addr>,
    /// The address and guest port the link carries; none when nothing on the internet reaches
    /// this PC's guest ports.
    pub address: Option<String>,
    /// Plain-English findings: carrier-grade NAT, another NAT in front, changed port numbers.
    pub notes: Vec<String>,
    /// Whether Play over the internet runs, which lets paired Nightjar devices in too.
    pub internet_play: bool,
    /// Whether the host listens on IPv6, so the router can open the guest ports to it.
    pub ipv6_listening: bool,
    /// When another device joins the running game: only `off` keeps a paired friend from
    /// joining as a guest (`ask` or `allow` lets one in).
    pub guest_join: String,
}

/// What [`guest_reach_of`] knows about this host besides the mapping.
struct GuestHost {
    /// The last request for the guest ports.
    requested: u64,
    /// The public IPv4 address shown (the router's own when public, else STUN's).
    public_ipv4: Option<IpAddr>,
    guest_port: u16,
    /// What STUN servers see, with Play over the internet on.
    stun: Vec<SocketAddr>,
    internet_play: bool,
    ipv6_listening: bool,
    guest_join: String,
}

/// [`GuestReach`] from the mapping status and the host. A link takes the IPv4 address only when
/// the router that forwarded the ports holds it: its own address is public, and STUN servers,
/// when asked, see the PC there too. Behind carrier-grade NAT or another router the forwarded
/// ports are out of reach whatever STUN sees.
fn guest_reach_of(mapping: &MappingStatus, host: &GuestHost) -> GuestReach {
    let GuestHost {
        requested,
        public_ipv4: shown_ipv4,
        guest_port,
        ref stun,
        internet_play,
        ipv6_listening,
        ref guest_join,
    } = *host;
    let mut notes = Vec::new();
    // A guest's browser opens the link's port and offers media to the guest ports by number: a
    // router that maps them to other numbers cannot be used.
    let renumbered = mapping
        .guest_mapped
        .iter()
        .any(|port| port.external != Some(port.port));
    let ipv4_mapped = !mapping.guest_mapped.is_empty() && mapping.guest_failed.is_empty();
    if renumbered {
        notes.push("The router mapped the guest ports to other port numbers, which guests' browsers can't use. Forward them by hand under their own numbers.".to_owned());
    }
    let mut holder = None;
    if let Some(router) = mapping.external_ipv4 {
        if !public_ipv4(router) {
            notes.push(format!(
                "The router's own internet address ({router}) is not public: {}.",
                not_public(router)
            ));
        } else if let Some(seen) = stun.iter().map(SocketAddr::ip).find(IpAddr::is_ipv4)
            && seen != router
        {
            notes.push(format!("STUN servers see this PC at {seen}, but the router reports {router}: another router or NAT is in front of it, so the forwarded ports are out of reach."));
        } else {
            holder = Some(router);
        }
    } else if ipv4_mapped {
        notes.push("The router forwarded the ports but did not say its internet address, so Rubylight can't tell whether guests reach it. Type your public address if you know it works.".to_owned());
    }
    let address = match (holder, mapping.guest_ipv6) {
        (Some(ip), _) if ipv4_mapped && !renumbered => Some(format!("{ip}:{guest_port}")),
        (_, Some(ip)) => Some(format!("[{ip}]:{guest_port}")),
        _ => None,
    };
    let mut ports = mapping.guest_mapped.clone();
    ports.extend(mapping.guest_failed.iter().cloned());
    let guest_join = match guest_join.as_str() {
        "ask" | "allow" => guest_join.clone(),
        _ => "off".to_owned(),
    };
    GuestReach {
        checking: mapping.runs && mapping.guest_round < requested,
        runs: mapping.runs,
        method: mapping.method.clone(),
        error: mapping.error.clone(),
        ports,
        public_ipv4: shown_ipv4,
        router_ipv4: mapping.external_ipv4,
        ipv6: mapping.guest_ipv6,
        address,
        notes,
        internet_play,
        ipv6_listening,
        guest_join,
    }
}

/// `GET /api/guests/internet`: [`GuestReach`] now.
pub fn guest_reach(h: &Shared) -> Result<GuestReach> {
    let config = h.config_snapshot();
    let guest_port = config.ports()?.guest;
    let stun = service()
        .map(|service| service.status().public)
        .unwrap_or_default();
    let ipv6_listening = crate::network::bind_address(&config, None)
        .is_ok_and(|bind| bind.is_ipv6() && !bind.is_loopback());
    Ok(guest_reach_of(
        &mapping(),
        &GuestHost {
            requested: h.guests.internet_round(),
            public_ipv4: external_ipv4(),
            guest_port,
            stun,
            internet_play: enabled(&config) && service().is_some(),
            ipv6_listening,
            guest_join: config.get("guest_join", "off").to_owned(),
        },
    ))
}

fn ports(config: &Config) -> Result<Ports> {
    let ports = config.ports()?;
    Ok(Ports {
        http: ports.http,
        https: ports.https,
        rtsp: ports.rtsp,
        video: ports.video,
        audio: ports.audio,
        control: ports.control,
        mic: if config.boolean("stream_mic", true) {
            ports.mic
        } else {
            0
        },
    })
}

/// A stable id for a paired device: the SHA-256 of its certificate.
fn device_id(certificate: &str) -> String {
    hex::encode(sha2::Sha256::digest(certificate.trim().as_bytes()))
}

fn parse_secret(value: &Value) -> Option<[u8; 32]> {
    let bytes = hex::decode(value.as_str()?).ok()?;
    bytes.try_into().ok()
}

/// The devices that have a secret, for the rendezvous.
fn devices(h: &Shared) -> Vec<Device> {
    let paired = h.paired.read().unwrap();
    let mut devices: Vec<Device> = Vec::new();
    for client in paired.clients.iter().filter(|client| client.enabled) {
        let Some(secret) = client.extra.get(SECRET_FIELD).and_then(parse_secret) else {
            continue;
        };
        let id = device_id(&client.cert);
        if !devices.iter().any(|device| device.id == id) {
            devices.push(Device {
                id,
                name: client.name.clone(),
                secret,
            });
        }
    }
    devices
}

/// The device's secret, made and stored with its pairing the first time it asks.
fn secret(h: &Shared, certificate: &str) -> Result<[u8; 32]> {
    let mut paired = h.paired.write().unwrap();
    let matching =
        |client: &&mut butterpollo_core::state::Client| client.cert.trim() == certificate.trim();
    if let Some(secret) = paired
        .clients
        .iter_mut()
        .filter(matching)
        .find_map(|client| client.extra.get(SECRET_FIELD).and_then(parse_secret))
    {
        return Ok(secret);
    }
    let secret: [u8; 32] = traverse::crypto::random();
    let mut stored = false;
    for client in paired.clients.iter_mut().filter(matching) {
        client
            .extra
            .insert(SECRET_FIELD.to_owned(), Value::String(hex::encode(secret)));
        stored = true;
    }
    anyhow::ensure!(stored, "the device is not paired");
    paired
        .save(&h.paired_path)
        .context("the internet play secret could not be saved")?;
    drop(paired);
    if let Some(service) = service() {
        service.set_devices(devices(h));
    }
    Ok(secret)
}

/// This PC's global IPv6 addresses, when the host listens on IPv6.
fn ipv6_addresses(config: &Config) -> Vec<String> {
    if config.get("address_family", "ipv4") != "both" {
        return Vec::new();
    }
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|interface| interface.oper_status == if_addrs::IfOperStatus::Up)
        .filter_map(|interface| match interface.ip() {
            IpAddr::V6(ip) if ip.segments()[0] & 0xe000 == 0x2000 => Some(ip.to_string()),
            _ => None,
        })
        .take(4)
        .collect()
}

/// `GET /internet-play/v1` for a paired device (`certificate`, over HTTPS): the secret, the
/// host session, the public endpoints and ports. The device keeps it per host.
pub fn describe(h: &Shared, certificate: &str) -> Result<Value> {
    let config = h.config_snapshot();
    let ports = ports(&config)?;
    let mapping = mapping();
    let mut answer = json!({
        "version": VERSION,
        "enabled": enabled(&config),
        "portMapping": config.boolean("upnp", false),
        "externalIpv4": external_ipv4(),
        // A client derives every port from the base port: a mapping is usable only when the
        // router kept the port numbers.
        "mappedPorts": mapping.failed.is_empty()
            && !mapping.mapped.is_empty()
            && mapping.mapped.iter().all(|port| port.external == Some(port.port)),
        "ipv6": ipv6_addresses(&config),
        "ports": {
            "http": ports.http,
            "https": ports.https,
            "rtsp": ports.rtsp,
            "video": ports.video,
            "audio": ports.audio,
            "control": ports.control,
            "mic": ports.mic,
        },
    });
    if enabled(&config) {
        answer["secret"] = Value::String(hex::encode(secret(h, certificate)?));
        if let Some(service) = service() {
            let status = service.status();
            answer["session"] = Value::String(hex::encode(service.session()));
            answer["endpoints"] = json!(status.public);
            answer["nat"] = Value::String(nat_name(status.nat).to_owned());
        }
    }
    Ok(answer)
}

/// The running rendezvous; stops it when dropped through [`InternetPlay::stop`].
pub struct InternetPlay {
    refresh: tokio::task::JoinHandle<()>,
}

impl InternetPlay {
    pub fn stop(self) {
        self.refresh.abort();
        if let Some(service) = SERVICE.lock().unwrap().take() {
            service.stop();
        }
    }
}

/// Starts STUN, the rendezvous and the tunnels when Play over the internet is on.
pub fn start(h: Shared, bind: IpAddr) -> Option<InternetPlay> {
    let config = h.config_snapshot();
    if !enabled(&config) || bind.is_loopback() {
        return None;
    }
    let ports = match ports(&config) {
        Ok(ports) => ports,
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), "Play over the internet could not read the ports");
            return None;
        }
    };
    // Tunnels connect to the host's own ports on loopback, from an address per device; a host
    // bound to one address is reached there instead.
    let (service_address, per_device_source) = if bind.is_unspecified() {
        (IpAddr::V4(Ipv4Addr::LOCALHOST), true)
    } else {
        (bind, false)
    };
    let node_bind = match bind {
        IpAddr::V4(ip) if !ip.is_unspecified() => SocketAddr::new(bind, 0),
        _ => traverse::node::any_ipv4(),
    };
    let host = match HostService::start(HostConfig {
        ports,
        service: service_address,
        per_device_source,
        bind: node_bind,
        stun_servers: traverse::server_list(
            config.get("internet_play_stun_servers", ""),
            traverse::STUN_SERVERS,
        ),
        bootstrap: traverse::DHT_BOOTSTRAP
            .iter()
            .map(|entry| (*entry).to_owned())
            .collect(),
    }) {
        Ok(host) => host,
        Err(error) => {
            tracing::warn!(%error, "Play over the internet could not open its UDP socket");
            return None;
        }
    };
    host.set_devices(devices(&h));
    tracing::info!(local = ?host.local_addr().ok(), "Play over the internet started");
    *SERVICE.lock().unwrap() = Some(host);
    let refresh = tokio::spawn(async move {
        let mut reported: Option<String> = None;
        let mut reported_paths: Vec<(String, String)> = Vec::new();
        loop {
            tokio::time::sleep(Duration::from_secs(15)).await;
            let Some(service) = service() else { return };
            service.set_devices(devices(&h));
            // Log a changed state once, not every 15 s.
            let status = service.status();
            let summary = format!(
                "{:?} {} {:?} {:?}",
                status.public,
                nat_name(status.nat),
                status.stun_error,
                status.dht_error
            );
            if reported.as_ref() != Some(&summary) {
                if let Some(error) = status.stun_error.as_ref().or(status.dht_error.as_ref()) {
                    tracing::warn!(%error, "Play over the internet: devices outside this network may not reach this PC");
                } else {
                    tracing::info!(public = ?status.public, nat = nat_name(status.nat), published = status.published, "Play over the internet is reachable");
                }
                reported = Some(summary);
            }
            // A device's own path problem is no sign that others cannot reach this PC.
            for (device, error) in &status.path_errors {
                if !reported_paths
                    .iter()
                    .any(|(d, e)| d == device && e == error)
                {
                    tracing::info!(%device, %error, "Play over the internet: a device's path did not start");
                }
            }
            reported_paths = status.path_errors;
        }
    });
    Some(InternetPlay { refresh })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_public_ipv4_counts_as_external() {
        assert!(public_ipv4("203.0.113.5".parse().unwrap()));
        assert!(!public_ipv4("192.168.1.1".parse().unwrap()));
        assert!(!public_ipv4("100.72.0.1".parse().unwrap()));
        assert!(!public_ipv4("2001:db8::1".parse().unwrap()));
    }

    #[test]
    fn notes_name_carrier_grade_and_double_nat() {
        let cgnat = MappingStatus {
            external_ipv4: Some("100.72.0.1".parse().unwrap()),
            ..Default::default()
        };
        let found = notes(&cgnat, &[], Nat::Cone);
        assert_eq!(found.len(), 1);
        assert!(found[0].contains("carrier-grade NAT"));

        let double = MappingStatus {
            external_ipv4: Some("203.0.113.5".parse().unwrap()),
            ..Default::default()
        };
        let found = notes(
            &double,
            &["198.51.100.7:40000".parse().unwrap()],
            Nat::Symmetric,
        );
        assert_eq!(found.len(), 2);
        assert!(found[0].contains("another router or NAT"));
        assert!(found[1].contains("symmetric NAT"));

        assert!(notes(&double, &["203.0.113.5:40000".parse().unwrap()], Nat::Cone).is_empty());
    }

    #[test]
    fn a_private_router_address_names_what_is_in_front_of_it() {
        let router_note = |address: &str| {
            let mapping = MappingStatus {
                external_ipv4: Some(address.parse().unwrap()),
                ..Default::default()
            };
            notes(&mapping, &[], Nat::Cone)
        };
        for address in ["192.168.1.1", "172.20.0.1"] {
            let found = router_note(address);
            assert_eq!(found.len(), 1, "{address}: {found:?}");
            assert!(
                found[0].contains("another router or modem is in front of this one"),
                "{address}: {found:?}"
            );
            assert!(
                found[0].contains("forward the ports there too, or put it in bridge mode"),
                "{address}: {found:?}"
            );
            assert!(
                !found[0].contains("carrier-grade NAT"),
                "{address}: {found:?}"
            );
        }
        // 10/8 is used at home and by providers behind another NAT: the note names either.
        let found = router_note("10.1.2.3");
        assert!(found[0].contains("carrier-grade NAT"), "{found:?}");
        assert!(found[0].contains("another router or modem"), "{found:?}");
        // 100.64/10 is the provider's shared address, never another router.
        let found = router_note("100.72.0.1");
        assert!(found[0].contains("carrier-grade NAT"), "{found:?}");
        assert!(!found[0].contains("another router"), "{found:?}");

        // The guest link's note says the same, and stays off the address.
        let behind = MappingStatus {
            runs: true,
            method: Some("UPnP".into()),
            external_ipv4: Some("192.168.1.1".parse().unwrap()),
            guest_mapped: guest_ports(|port| port),
            ..Default::default()
        };
        let reach = guest_reach_of(&behind, &host(0, None, &[]));
        assert_eq!(reach.address, None);
        assert!(
            reach.notes[0].contains("forward the ports there too, or put it in bridge mode"),
            "{:?}",
            reach.notes
        );
    }

    fn guest_ports(external: impl Fn(u16) -> u16) -> Vec<PortState> {
        [
            ("TCP", 47991),
            ("UDP", 8002),
            ("UDP", 8003),
            ("UDP", 8004),
            ("UDP", 8005),
        ]
        .into_iter()
        .map(|(protocol, port)| PortState {
            protocol: protocol.into(),
            port,
            external: Some(external(port)),
            reason: None,
        })
        .collect()
    }

    fn host(requested: u64, public: Option<&str>, stun: &[&str]) -> GuestHost {
        GuestHost {
            requested,
            public_ipv4: public.map(|ip| ip.parse().unwrap()),
            guest_port: 47991,
            stun: stun.iter().map(|ip| ip.parse().unwrap()).collect(),
            internet_play: !stun.is_empty(),
            ipv6_listening: false,
            guest_join: "off".to_owned(),
        }
    }

    #[test]
    fn a_guest_link_takes_the_public_address_only_when_every_guest_port_is_mapped() {
        let public: IpAddr = "203.0.113.5".parse().unwrap();
        let mapped = MappingStatus {
            runs: true,
            method: Some("UPnP".into()),
            external_ipv4: Some(public),
            guest_mapped: guest_ports(|port| port),
            guest_round: 3,
            ..Default::default()
        };
        let reach = guest_reach_of(&mapped, &host(3, Some("203.0.113.5"), &[]));
        assert_eq!(reach.address.as_deref(), Some("203.0.113.5:47991"));
        assert!(!reach.checking);
        assert_eq!(reach.ports.len(), 5);
        assert!(reach.notes.is_empty(), "{:?}", reach.notes);
        // A paired friend joins as a guest unless When another device joins the running game
        // is off; another value reads as off.
        let mut letting = host(3, Some("203.0.113.5"), &[]);
        letting.guest_join = "allow".to_owned();
        assert_eq!(guest_reach_of(&mapped, &letting).guest_join, "allow");
        letting.guest_join = "sometimes".to_owned();
        assert_eq!(guest_reach_of(&mapped, &letting).guest_join, "off");
        // STUN agreeing with the router changes nothing.
        let agreed = guest_reach_of(
            &mapped,
            &host(3, Some("203.0.113.5"), &["203.0.113.5:40000"]),
        );
        assert_eq!(agreed.address, reach.address);
        assert!(agreed.internet_play);
        // Until the task has asked the router for the latest request, the dialog waits.
        assert!(guest_reach_of(&mapped, &host(4, Some("203.0.113.5"), &[])).checking);
        // A task that does not run never answers: no waiting for it.
        let idle = MappingStatus {
            runs: false,
            ..Default::default()
        };
        assert!(!guest_reach_of(&idle, &host(4, None, &[])).checking);

        // One media port refused: no link at the public address.
        let mut partly = mapped.clone();
        let refused = partly.guest_mapped.pop().unwrap();
        partly.guest_failed.push(PortState {
            external: None,
            reason: Some("the router is out of mappings".into()),
            ..refused
        });
        let reach = guest_reach_of(&partly, &host(3, Some("203.0.113.5"), &[]));
        assert_eq!(reach.address, None);
        assert_eq!(reach.ports.len(), 5);

        // Other port numbers can't be used, and say so.
        let renumbered = MappingStatus {
            guest_mapped: guest_ports(|port| port + 10_000),
            ..mapped.clone()
        };
        let reach = guest_reach_of(&renumbered, &host(3, Some("203.0.113.5"), &[]));
        assert_eq!(reach.address, None);
        assert!(reach.notes[0].contains("other port numbers"));

        // A router that does not say its address: no guess from STUN.
        let silent = MappingStatus {
            external_ipv4: None,
            ..mapped
        };
        let reach = guest_reach_of(&silent, &host(3, Some("198.51.100.7"), &["198.51.100.7:1"]));
        assert_eq!(reach.address, None);
        assert!(reach.notes[0].contains("did not say its internet address"));
    }

    #[test]
    fn a_guest_link_behind_carrier_grade_nat_falls_back_to_ipv6_or_nothing() {
        let cgnat = MappingStatus {
            runs: true,
            method: Some("UPnP".into()),
            external_ipv4: Some("100.72.0.1".parse().unwrap()),
            guest_mapped: guest_ports(|port| port),
            ..Default::default()
        };
        // With Play over the internet on, STUN sees the provider's public address: the
        // forwarded ports on the home router are still out of reach.
        let reach = guest_reach_of(
            &cgnat,
            &host(0, Some("198.51.100.7"), &["198.51.100.7:40000"]),
        );
        assert_eq!(reach.address, None);
        assert!(
            reach.notes[0].contains("carrier-grade NAT"),
            "{:?}",
            reach.notes
        );
        assert_eq!(reach.public_ipv4, Some("198.51.100.7".parse().unwrap()));

        let with_ipv6 = MappingStatus {
            guest_ipv6: Some("2001:db8::7".parse().unwrap()),
            ..cgnat
        };
        let reach = guest_reach_of(&with_ipv6, &host(0, None, &[]));
        assert_eq!(reach.address.as_deref(), Some("[2001:db8::7]:47991"));

        // Another NAT in front of a router with a public address: nothing at IPv4.
        let double = MappingStatus {
            runs: true,
            external_ipv4: Some("203.0.113.5".parse().unwrap()),
            guest_mapped: guest_ports(|port| port),
            ..Default::default()
        };
        let reach = guest_reach_of(
            &double,
            &host(0, Some("203.0.113.5"), &["198.51.100.7:40000"]),
        );
        assert_eq!(reach.address, None);
        assert!(reach.notes[0].contains("another router or NAT"));
        assert!(reach.internet_play);
    }

    #[test]
    fn device_ids_ignore_surrounding_whitespace() {
        assert_eq!(device_id("cert\n"), device_id("cert"));
        assert_ne!(device_id("a"), device_id("b"));
        assert_eq!(parse_secret(&Value::String("00".repeat(32))), Some([0; 32]));
        assert_eq!(parse_secret(&Value::String("00".repeat(31))), None);
    }
}
