//! Port-mapping sets, persisted guest port lease records, and console status formatting
//! for UPnP/PCP/NAT-PMP port forwarding.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Mapping {
    pub(super) protocol: Protocol,
    pub(super) port: u16,
}

pub(super) fn mappings(config: &Config, ports: Ports) -> Vec<Mapping> {
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

/// The WebRTC TV protocol's discovery port, UDP 8000 whatever the settings say; the owner's own
/// stream's media port defaults to the next one.
pub(super) const WEBRTC_DISCOVERY_PORT: u16 = 8000;

/// Browser guests' ports (docs/browser-guests.md): the guest page on its own port, and one media
/// port per guest seat. Never the console's port, the WebRTC signalling port or the owner's own
/// media port.
pub(super) fn guest_ports(config: &Config, ports: Ports) -> Vec<Mapping> {
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
        (0..crate::webrtc_bridge::MAX_GUESTS)
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
pub(super) fn guest_mappings(config: &Config, ports: Ports) -> Vec<Mapping> {
    if !config.webrtc_server() {
        return Vec::new();
    }
    guest_ports(config, ports)
}

/// The guest leases written to the profile ([`LeaseRecord`]).
pub(super) const GUEST_LEASES: &str = "guest-port-leases.json";

/// How a guest port was forwarded, which says how a start after an unclean stop takes it down.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum LeaseMethod {
    /// A UPnP IGD mapping under this host's description.
    Upnp,
    Pcp,
    NatPmp,
    /// A PCP pinhole in an IPv6 router's firewall. UPnP IGDv2 pinholes are not recorded: their
    /// 120-second lease ends by the next start.
    PcpV6,
}

/// A mapping's protocol, as the lease record writes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum LeaseProtocol {
    Tcp,
    Udp,
}
impl From<Protocol> for LeaseProtocol {
    fn from(protocol: Protocol) -> Self {
        match protocol {
            Protocol::TCP => Self::Tcp,
            Protocol::UDP => Self::Udp,
        }
    }
}
impl From<LeaseProtocol> for Protocol {
    fn from(protocol: LeaseProtocol) -> Self {
        match protocol {
            LeaseProtocol::Tcp => Self::TCP,
            LeaseProtocol::Udp => Self::UDP,
        }
    }
}

/// One guest port lease the router granted this host: how, the port, and when a PCP or NAT-PMP
/// lease ends. A UPnP mapping has no expiry: the router's table says when it is gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(super) struct GuestLease {
    pub(super) method: LeaseMethod,
    pub(super) protocol: LeaseProtocol,
    pub(super) port: u16,
    /// Unix seconds. `None` for UPnP, and for a record written before leases carried one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) expires: Option<u64>,
}
impl GuestLease {
    pub(super) fn new(method: LeaseMethod, mapping: Mapping) -> Self {
        let expires = match method {
            LeaseMethod::Upnp => None,
            _ => Some(lease_expiry()),
        };
        Self {
            method,
            protocol: mapping.protocol.into(),
            port: mapping.port,
            expires,
        }
    }
    pub(super) fn mapping(self) -> Mapping {
        Mapping {
            protocol: self.protocol.into(),
            port: self.port,
        }
    }
    /// The same lease, whatever its expiry.
    pub(super) fn same_mapping(&self, other: &Self) -> bool {
        self.method == other.method && self.protocol == other.protocol && self.port == other.port
    }
    /// Whether the lease's lifetime has run out: a PCP or NAT-PMP lease with no expiry came
    /// from a record written before leases carried one, so it is no longer trusted.
    pub(super) fn expired(&self, now: u64) -> bool {
        match self.method {
            LeaseMethod::Upnp => false,
            _ => self.expires.is_none_or(|at| at <= now),
        }
    }
}

/// The Unix time now, in whole seconds.
pub(super) fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The end of the PCP or NAT-PMP lease this host asks for.
pub(super) fn lease_expiry() -> u64 {
    now_seconds().saturating_add(u64::from(portmap::LIFETIME))
}

/// The guest leases on disk: written before each map request, dropped once a delete is
/// confirmed (or the router shows the mapping gone), and read at start so that only those
/// ports, never another application's mapping on the same numbers, are deleted.
pub(super) struct LeaseRecord {
    path: PathBuf,
    leases: Vec<GuestLease>,
}
impl LeaseRecord {
    /// The record in `directory`; a missing or unreadable one lists nothing, so nothing is
    /// deleted from it.
    pub(super) fn load(directory: &Path) -> Self {
        let path = directory.join(GUEST_LEASES);
        let mut leases: Vec<GuestLease> =
            butterpollo_session::state::load_json(&path, serde_json::Value::Array(Vec::new()))
                .and_then(|value| {
                    serde_json::from_value::<Vec<GuestLease>>(value).map_err(anyhow::Error::from)
                })
                .unwrap_or_else(|error| {
                    if path.exists() {
                        tracing::warn!(%error, path = %path.display(), "guest lease record could not be read; no ports are cleaned from it");
                    }
                    Vec::new()
                });
        let now = now_seconds();
        leases.retain(|lease| !lease.expired(now));
        leases.sort_unstable();
        leases.dedup();
        Self { path, leases }
    }
    pub(super) fn leases(&self) -> &[GuestLease] {
        &self.leases
    }
    /// Adds leases before their map requests go out: a stop between a request and its answer
    /// leaves them listed, to be taken down at the next start. A lease already listed keeps the
    /// expiry it was granted: a router that never answers must not extend it.
    pub(super) fn add_all(&mut self, leases: impl IntoIterator<Item = GuestLease>) {
        let mut next = self.leases.clone();
        for lease in leases {
            if !next.iter().any(|held| held.same_mapping(&lease)) {
                next.push(lease);
            }
        }
        next.sort_unstable();
        next.dedup();
        self.write(next);
    }
    /// The router answered with a fresh lease for these guest ports: they now end a lifetime
    /// from now. Only a grant extends a lease, so a silent router's entries run out.
    pub(super) fn granted(&mut self, method: LeaseMethod, mappings: &[Mapping]) {
        if mappings.is_empty() {
            return;
        }
        let expires = lease_expiry();
        let mut next = self.leases.clone();
        for lease in &mut next {
            if lease.method == method && mappings.contains(&lease.mapping()) {
                lease.expires = Some(expires);
            }
        }
        self.write(next);
    }
    /// Drops the leases of `methods` for `mappings`, once the router confirmed those gone: a
    /// delete it answered, or a mapping it shows under another owner.
    pub(super) fn remove_confirmed(
        &mut self,
        methods: impl Fn(LeaseMethod) -> bool,
        mappings: &[Mapping],
    ) {
        let next = self
            .leases
            .iter()
            .copied()
            .filter(|lease| !(methods(lease.method) && mappings.contains(&lease.mapping())))
            .collect();
        self.write(next);
    }
    /// Writes the record, or removes its file once it lists nothing.
    fn write(&mut self, mut leases: Vec<GuestLease>) {
        leases.sort_unstable();
        leases.dedup();
        if leases == self.leases {
            return;
        }
        let result = if leases.is_empty() {
            match std::fs::remove_file(&self.path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
                _ => Ok(()),
            }
        } else {
            butterpollo_session::state::write_json(&self.path, &leases)
        };
        match result {
            Ok(()) => self.leases = leases,
            Err(error) => tracing::warn!(
                %error,
                path = %self.path.display(),
                "guest lease record could not be written"
            ),
        }
    }
}

/// The ports to map now: the streaming ports while `upnp` is on, and the browser guests' only
/// while a link needs them: a link made for the internet (whatever `upnp` says), or with `upnp`
/// on any link that works or guest who watches (`guests_open`).
pub(super) fn wanted_now(
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

/// The recorded leases whose method `wanted` accepts.
pub(super) fn recorded_leases(
    leases: &[GuestLease],
    wanted: impl Fn(LeaseMethod) -> bool,
) -> Vec<GuestLease> {
    leases
        .iter()
        .copied()
        .filter(|lease| wanted(lease.method))
        .collect()
}

pub(super) fn protocol_name(protocol: Protocol) -> String {
    if protocol == Protocol::TCP {
        "TCP"
    } else {
        "UDP"
    }
    .to_owned()
}

/// Every port the router has mapped for this host: the ones it holds, and the ones another
/// mapping already forwards here under the same number.
pub(super) fn mapped_ports(applied: &[Mapping], forwarded: &[Mapping]) -> Vec<(Mapping, u16)> {
    applied
        .iter()
        .chain(forwarded)
        .map(|mapping| (*mapping, mapping.port))
        .collect()
}

/// The console's view of a mapping attempt: each wanted port mapped or refused, and why.
pub(super) fn port_states(
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

/// Writes one mapping attempt to the console's status: the streaming ports and the guests' apart,
/// so a guest port the router refuses never makes the streaming ports look unusable.
pub(super) fn report(
    status: &mut crate::internet_play::MappingStatus,
    stream: &[Mapping],
    guest: &[Mapping],
    mapped: &[(Mapping, u16)],
    refused: &[(Mapping, String)],
) {
    (status.mapped, status.failed) = port_states(stream, mapped, refused);
    (status.guest_mapped, status.guest_failed) = port_states(guest, mapped, refused);
}
