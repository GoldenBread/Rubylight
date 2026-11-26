//! Away wake relay, the PC's side: paired devices on this PC's network enroll as relays, and a
//! paired device that is away asks one of them over Traverse to send this PC Wake-on-LAN
//! (`docs/designs/15-away-wake-relay.md`). The relays never reach the PC while it sleeps; it
//! hands each one what it may wake and for whom while it is awake, and each paired device that
//! may launch games the keys it asks with.
//!
//! The relays and the key epoch live in `wake-relay.json` beside the paired devices
//! ([`butterpollo_session::wake::relay`]); the keys a device gets are derived from a relay's
//! secret ([`rubylight_traverse::wake_relay::device_secret`]) and never stored.
use crate::state::Host;
use anyhow::{Context, Result};
use butterpollo_config::config::Config;
use butterpollo_session::{
    state::{Client, LAUNCH_APPS},
    wake::relay::{self, Refusal, Relay, Relays},
};
use rubylight_native_api::wake_relay::{
    RelayAssignment, RelayCredential, RelayEnroll, RelayEnrolled, RelayPeer, WakeRelayBundle,
    WakeRelayInfo,
};
use rubylight_traverse::wake_relay as traverse;
use std::{
    net::IpAddr,
    path::PathBuf,
    sync::{Mutex, PoisonError},
};

/// Whether this PC offers the away wake relay: the setting is on, and so is Play over the
/// internet, whose rendezvous the relay and the device that is away meet through.
pub fn offered(config: &Config) -> bool {
    crate::internet_play::enabled(config) && config.away_wake_relay()
}

/// Why the PC refuses a relay request.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// The caller lacks Host control (enrolling) or both Launch and Host control (reading).
    Permission,
    /// The setting or Play over the internet is off.
    Off,
    Relay(Refusal),
    Failed(String),
}

impl From<anyhow::Error> for Refused {
    fn from(error: anyhow::Error) -> Self {
        Self::Failed(format!("{error:#}"))
    }
}

/// Orders the reads and writes of `wake-relay.json`.
static FILE: Mutex<()> = Mutex::new(());

fn path(h: &Host) -> PathBuf {
    h.directory.join(relay::FILE)
}

/// The enrolled relays; none when the file is missing. A damaged file is an error, never
/// silently a new epoch that would hand out keys again.
pub fn load(h: &Host) -> Result<Relays> {
    let path = path(h);
    match std::fs::read(&path) {
        // serde's message can quote the value it stopped at, which may be a secret.
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
            anyhow::anyhow!(
                "{} is damaged (line {}, column {})",
                path.display(),
                error.line(),
                error.column()
            )
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Relays::default()),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

fn save(h: &Host, relays: &Relays) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(relays)?;
    butterpollo_session::state::atomic_write(&path(h), &bytes)
}

fn new_secret() -> String {
    hex::encode(rand::random::<[u8; 32]>())
}

fn device_id(certificate: &str) -> String {
    hex::encode(traverse::device_id(certificate))
}

fn target_id(h: &Host) -> String {
    h.paired
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .unique_id
        .clone()
}

fn peer(relay: &Relay) -> RelayPeer {
    RelayPeer {
        id: relay.id.clone(),
        name: relay.name.clone(),
        enrolled_unix: relay.enrolled_unix,
        last_seen_unix: relay.last_seen_unix,
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// `GET /wake/relay`: the relays, and for a caller with Launch applications the keys it asks
/// them with.
pub fn info(h: &Host, client: &Client) -> Result<WakeRelayInfo, Refused> {
    let launch = client.allows(LAUNCH_APPS);
    if !launch && !client.allows_host_control() {
        return Err(Refused::Permission);
    }
    let target_id = target_id(h);
    if !offered(&h.config_snapshot()) {
        return Ok(WakeRelayInfo {
            enabled: false,
            target_id,
            epoch: 0,
            relays: Vec::new(),
            bundle: None,
        });
    }
    let relays = {
        let _file = FILE.lock().unwrap_or_else(PoisonError::into_inner);
        load(h)?
    };
    let epoch = relays.epoch();
    let device = traverse::device_id(&client.cert);
    let bundle = launch.then(|| WakeRelayBundle {
        target_id: target_id.clone(),
        epoch,
        device: hex::encode(device),
        relays: relays
            .relays
            .iter()
            .filter_map(|relay| {
                let secret = traverse::parse_hex::<32>(&relay.secret)?;
                let key = traverse::device_secret(&secret, &target_id, &device, epoch);
                Some(RelayCredential {
                    id: relay.id.clone(),
                    name: relay.name.clone(),
                    secret: hex::encode(key),
                })
            })
            .collect(),
    });
    Ok(WakeRelayInfo {
        enabled: true,
        target_id,
        epoch,
        relays: relays.relays.iter().map(peer).collect(),
        bundle,
    })
}

/// `POST /wake/relay/enroll` from `client` at `address`. Every action needs Host control;
/// enrolling and refreshing also a caller on one of this PC's own private subnets, as only
/// there its magic packets reach the PC.
pub fn enroll(
    h: &Host,
    client: &Client,
    address: IpAddr,
    action: RelayEnroll,
) -> Result<RelayEnrolled, Refused> {
    if !client.allows_host_control() {
        return Err(Refused::Permission);
    }
    let config = h.config_snapshot();
    let enabled = offered(&config);
    let id = device_id(&client.cert);
    let _file = FILE.lock().unwrap_or_else(PoisonError::into_inner);
    let mut relays = load(h)?;
    let mut assigned = None;
    match action {
        RelayEnroll::Enroll { .. } | RelayEnroll::Refresh if !enabled => {
            return Err(Refused::Off);
        }
        RelayEnroll::Enroll { name } => {
            let name = name.unwrap_or_else(|| client.name.clone());
            assigned = Some(assign(h, &config, &mut relays, &id, &name, address, true)?);
        }
        RelayEnroll::Refresh => {
            assigned = Some(assign(h, &config, &mut relays, &id, "", address, false)?);
        }
        RelayEnroll::Leave => relays.remove(&id).map_err(Refused::Relay)?,
        RelayEnroll::Revoke { relay } => relays.remove(&relay).map_err(Refused::Relay)?,
        RelayEnroll::RotateEpoch => relays.rotate(new_secret),
    }
    save(h, &relays)?;
    Ok(RelayEnrolled {
        assignment: assigned,
        relays: relays.relays.iter().map(peer).collect(),
    })
}

/// Enrolls (or, with `create` false, only refreshes) the relay `id` and builds what it needs.
fn assign(
    h: &Host,
    config: &Config,
    relays: &mut Relays,
    id: &str,
    name: &str,
    address: IpAddr,
    create: bool,
) -> Result<RelayAssignment, Refused> {
    let adapters = h
        .wake
        .power()
        .adapters()
        .context("reading the network adapters")?;
    let own = relay::lan_address(address, &adapters).ok_or(Refused::Relay(Refusal::NotOnLan))?;
    if !create && !relays.relays.iter().any(|relay| relay.id == id) {
        return Err(Refused::Relay(Refusal::UnknownRelay));
    }
    let secret = relays
        .enroll(id, name, now_unix(), new_secret)
        .map_err(Refused::Relay)?
        .secret
        .clone();
    let (macs, broadcasts) = relay::wake_targets(&adapters);
    let devices = {
        let paired = h.paired.read().unwrap_or_else(PoisonError::into_inner);
        relay::wakers(&paired.clients, device_id)
    };
    Ok(RelayAssignment {
        target_id: target_id(h),
        target_name: crate::network::host_name(config),
        epoch: relays.epoch(),
        secret,
        macs,
        broadcasts,
        address: Some(own.to_string()),
        http_port: config.ports()?.http,
        devices,
        refresh_seconds: relay::REFRESH_SECONDS,
    })
}

/// For the console: whether it is on, the epoch and the relays, without secrets.
pub fn console_status(h: &Host) -> Result<serde_json::Value> {
    let relays = {
        let _file = FILE.lock().unwrap_or_else(PoisonError::into_inner);
        load(h)?
    };
    Ok(serde_json::json!({
        "enabled": offered(&h.config_snapshot()),
        "epoch": relays.epoch(),
        "relays": relays.relays.iter().map(peer).collect::<Vec<_>>(),
    }))
}

/// The console's routes: `GET /api/wake-relay`, `POST /api/wake-relay/revoke {"id"}` and
/// `POST /api/wake-relay/rotate`, each answering the status after it.
pub fn console(
    h: &Host,
    method: &str,
    path: &str,
    data: &serde_json::Value,
) -> Result<serde_json::Value> {
    let changed = match (method, path) {
        ("POST", "/api/wake-relay/revoke") => {
            let id = data
                .get("id")
                .and_then(serde_json::Value::as_str)
                .context("id is missing")?;
            console_change(h, Some(id))
        }
        ("POST", "/api/wake-relay/rotate") => console_change(h, None),
        _ => Ok(()),
    };
    match changed {
        Err(Refused::Relay(Refusal::UnknownRelay)) => {
            anyhow::bail!("That wake relay is not enrolled on this PC.")
        }
        Err(Refused::Failed(error)) => anyhow::bail!(error),
        Err(other) => anyhow::bail!("{other:?}"),
        Ok(()) => console_status(h),
    }
}

/// From the console: remove one relay, or rotate every key.
pub fn console_change(h: &Host, revoke: Option<&str>) -> Result<(), Refused> {
    let _file = FILE.lock().unwrap_or_else(PoisonError::into_inner);
    let mut relays = load(h)?;
    match revoke {
        Some(id) => relays.remove(id).map_err(Refused::Relay)?,
        None => relays.rotate(new_secret),
    }
    save(h, &relays)?;
    Ok(())
}

#[cfg(test)]
mod tests;
