//! Invites to play over the internet (docs/multiclient.md, "Invite a friend over the internet"):
//! the owner makes a link, a friend's Nightjar opens it, reaches this PC through the internet play
//! rendezvous keyed by the link, pairs with the PIN the link carries and joins the game being
//! played when the player lets it in. The state is `butterpollo_session::invites`; this module makes
//! the links, spends them at the end of a pairing, removes invited devices when their time ends,
//! and tells the rendezvous which invites and invited devices to look for, even while
//! "Play over the internet" itself is off.
use crate::state::Shared;
use anyhow::{Context, Result};
use butterpollo_session::{
    invites::{
        self as policy, CreateError, DEFAULT_LIFETIME, InvitedGuest, Invites, MARKER, Permission,
    },
    state::Client,
};
use rubylight_protocol::invite_uri::{InvitePorts, InviteUri};
use serde_json::{Value, json};
use std::{
    sync::{Mutex, MutexGuard, PoisonError},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// How often invites and invited devices are looked at for their end.
const SWEEP_EVERY: Duration = Duration::from_secs(30);

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The host's invites, on [`crate::state::Host::invites`]. In memory only: a restart voids the
/// links nobody used yet; devices already paired from one keep their place until their time ends.
#[derive(Default)]
pub struct HostInvites {
    state: Mutex<Invites>,
}

impl HostInvites {
    fn locked(&self) -> MutexGuard<'_, Invites> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Why an invite was not made.
#[derive(Debug)]
pub enum CreateFailure {
    PairingOff,
    /// The device asking may not approve new devices, or could not even watch.
    NotAllowed,
    Policy(CreateError),
    Internal(anyhow::Error),
}

impl CreateFailure {
    pub fn message(&self) -> String {
        match self {
            Self::PairingOff => {
                "Pairing new devices is off on this PC: turn it on to invite someone".into()
            }
            Self::NotAllowed => {
                "This device may not invite anyone: it needs Approve new devices or Host control"
                    .into()
            }
            Self::Policy(error) => error.message().into(),
            Self::Internal(error) => format!("{error:#}"),
        }
    }
}

/// A new invite, as its maker sees it once: the link is never kept or shown again.
#[derive(Debug)]
pub struct Created {
    pub id: u32,
    pub link: String,
    pub expires: u64,
    pub permission: Permission,
}

/// The paired device's invite, when it was paired from one.
pub(crate) fn invited(client: &Client) -> Option<InvitedGuest> {
    policy::invited_guest(client)
}

/// Whether `client` is a device paired from an invite whose time has not ended.
pub fn invited_live(client: &Client) -> bool {
    invited(client).is_some_and(|guest| !guest.ended(unix_now()))
}

/// The link of a new invite: its secret, this PC's identity and name, and what the friend's
/// first path can start from.
fn link(h: &Shared, secret: [u8; 32], expires: u64, permission: Permission) -> Result<String> {
    use sha2::Digest;
    let config = h.config_snapshot();
    let hints = crate::internet_play::hints(&config)?;
    let p = hints.ports;
    Ok(InviteUri {
        secret,
        unique_id: h.paired.read().unwrap().unique_id.clone(),
        cert_sha256: sha2::Sha256::digest(&h.identity.der).into(),
        name: crate::network::host_name(&config),
        expires,
        permission: permission.as_str().into(),
        ports: InvitePorts {
            http: p.http,
            https: p.https,
            rtsp: p.rtsp,
            video: p.video,
            audio: p.audio,
            control: p.control,
            mic: p.mic,
        },
        session: hints.session,
        endpoints: hints.endpoints,
        nat: hints.nat.into(),
        ipv6: hints.ipv6,
        external_ipv4: hints.external_ipv4,
        mapped_ports: hints.mapped_ports,
    }
    .to_uri())
}

/// Makes an invite: `permission` (the host's guest default without one) for `lifetime`
/// ([`DEFAULT_LIFETIME`] without one), by `creator` (a paired device; none from the console),
/// which must hold Approve new devices or Host control, and never gives more input than it has.
pub fn create(
    h: &Shared,
    permission: Option<Permission>,
    lifetime: Option<Duration>,
    creator: Option<&Client>,
) -> Result<Created, CreateFailure> {
    let config = h.config_snapshot();
    if !config.boolean("enable_pairing", true) {
        return Err(CreateFailure::PairingOff);
    }
    let wanted =
        permission.unwrap_or_else(|| butterpollo_session::multiclient::default_permission(&config));
    let permission = match creator {
        None => wanted,
        Some(client) if client.may_approve_pairing() => {
            policy::capped(wanted, client.perm).ok_or(CreateFailure::NotAllowed)?
        }
        Some(_) => return Err(CreateFailure::NotAllowed),
    };
    let secret: [u8; 32] = rubylight_traverse::crypto::random();
    let now = unix_now();
    let lifetime = lifetime.unwrap_or(DEFAULT_LIFETIME);
    let (id, expires) = {
        let mut invites = h.invites.locked();
        let invite = invites
            .create(
                rubylight_traverse::invite::rendezvous_secret(&secret),
                rubylight_traverse::invite::pairing_pin(&secret),
                permission,
                lifetime,
                creator.map(|c| butterpollo_session::state::device_uuid(&c.uuid).to_owned()),
                now,
            )
            .map_err(CreateFailure::Policy)?;
        (invite.id, invite.expires)
    };
    // The rendezvous starts now, so the record the link points at is there when the friend looks.
    crate::internet_play::ensure(h);
    crate::internet_play::refresh(h);
    let link = link(h, secret, expires, permission).and_then(|link| {
        // Without the rendezvous only the PC's IPv6 address or forwarded ports reach it; with
        // neither, a link would lead nowhere.
        let hints = crate::internet_play::hints(&h.config_snapshot())?;
        if crate::internet_play::bound()
            && !crate::internet_play::running()
            && hints.ipv6.is_empty()
            && !hints.mapped_ports
        {
            anyhow::bail!(
                "Play over the internet could not start on this PC, so a friend could not reach it. \
                 The host log says why."
            );
        }
        Ok(link)
    });
    let link = link.map_err(|error| {
        h.invites.locked().revoke(id);
        CreateFailure::Internal(error)
    })?;
    tracing::info!(
        invite = id,
        permission = permission.as_str(),
        by = creator.map_or("the console", |c| c.name.as_str()),
        "invite to play over the internet made"
    );
    Ok(Created {
        id,
        link,
        expires,
        permission,
    })
}

/// The invites waiting and the devices paired from one, for the console and the native API.
pub fn view(h: &Shared) -> Value {
    let now = unix_now();
    let links: Vec<Value> = {
        let mut invites = h.invites.locked();
        invites.expire(now);
        invites
            .list()
            .iter()
            .filter(|i| i.redeemed.is_none())
            .map(|i| {
                json!({
                    "id": i.id,
                    "permission": i.permission.as_str(),
                    "created": i.created,
                    "expires": i.expires,
                })
            })
            .collect()
    };
    let friends: Vec<Value> = h
        .paired
        .read()
        .unwrap()
        .clients
        .iter()
        .filter_map(|c| {
            invited(c).map(|guest| {
                json!({
                    "uuid": c.uuid,
                    "name": c.name,
                    "permission": guest.permission.as_str(),
                    "until": guest.until,
                })
            })
        })
        .collect();
    json!({
        "links": links,
        "friends": friends,
        "max_links": policy::MAX_LIVE,
        "default_seconds": DEFAULT_LIFETIME.as_secs(),
    })
}

/// Takes back an invite nobody used yet. False when there is none by that id.
pub fn revoke(h: &Shared, id: u32) -> bool {
    let gone = h.invites.locked().revoke(id).is_some();
    if gone {
        tracing::info!(invite = id, "invite taken back");
        crate::internet_play::refresh(h);
    }
    gone
}

/// Ends a device paired from an invite: its streams stop and its pairing and secret go. False
/// when `uuid` is no such device.
pub fn remove(h: &Shared, uuid: &str) -> Result<bool> {
    let known = h
        .paired
        .read()
        .unwrap()
        .clients
        .iter()
        .any(|c| c.uuid == uuid && invited(c).is_some());
    if !known {
        return Ok(false);
    }
    crate::nvhttp::end_device(h, uuid);
    h.unpair_with_network_history(Some(uuid))?;
    h.notify_webrtc_clients_changed();
    crate::nvhttp::release_device(h, uuid);
    crate::internet_play::refresh(h);
    Ok(true)
}

/// The live invite whose PIN `auth` (`otpauth`, hex) answers with `salt`: its id and PIN.
pub(crate) fn matching(h: &Shared, auth: &str, salt: &str) -> Option<(u32, String)> {
    let auth = hex::decode(auth).ok()?;
    let invites = h.invites.locked();
    invites
        .matching(&auth, salt, unix_now())
        .map(|i| (i.id, i.pin.clone()))
}

/// What a device paired from invite `id` gets: its permission bits and `extra`. Fails when the
/// invite is gone, spent or past its time, or when the device that made it may no longer approve
/// devices. The caller holds `paired` for writing until [`spend`], so no other pairing takes the
/// invite in between.
pub(crate) fn entitle(
    h: &Shared,
    state: &butterpollo_session::state::PairedState,
    id: u32,
) -> Result<(u32, InvitedGuest)> {
    let now = unix_now();
    let invites = h.invites.locked();
    let invite = invites
        .list()
        .iter()
        .find(|i| i.id == id && i.live(now))
        .cloned()
        .context("this invite was already used, taken back or has expired")?;
    let mut permission = invite.permission;
    if let Some(creator) = &invite.creator {
        let maker = state
            .clients
            .iter()
            .find(|c| &c.uuid == creator && c.may_approve_pairing())
            .context("the device that made this invite may no longer approve new devices")?;
        permission = policy::capped(permission, maker.perm)
            .context("the device that made this invite may no longer watch streams")?;
    }
    Ok((
        policy::device_permission(permission),
        InvitedGuest {
            until: invite.expires,
            permission,
        },
    ))
}

/// Spends invite `id` on device `uuid`, once that device is saved.
pub(crate) fn spend(h: &Shared, id: u32, uuid: &str, name: &str) -> Result<()> {
    h.invites
        .locked()
        .redeem(id, uuid, name, unix_now())
        .map(|_| ())
        .context("this invite was already used")
}

/// A device paired again from a new invite while it already was an invited guest: its bits and
/// time follow the new invite. A device paired on its own keeps its rights, as an invite never
/// takes any away: false then.
pub(crate) fn apply(client: &mut Client, perm: u32, guest: InvitedGuest) -> bool {
    if !client.extra.contains_key(MARKER) {
        return false;
    }
    client.perm = perm;
    client.extra.insert(MARKER.into(), guest.marker());
    true
}

/// The rendezvous devices invites need now: `(id, name, secret)` for each invite that works or
/// was spent in the last minutes.
pub(crate) fn rendezvous(h: &Shared) -> Vec<(String, String, [u8; 32])> {
    h.invites
        .locked()
        .rendezvous(unix_now())
        .into_iter()
        .map(|(id, secret)| (format!("invite-{id}"), format!("Invite {id}"), secret))
        .collect()
}

/// Whether invites or invited devices need the rendezvous while internet play itself is off.
pub fn need_rendezvous(h: &Shared) -> bool {
    h.invites.locked().any(unix_now()) || h.paired.read().unwrap().clients.iter().any(invited_live)
}

/// Forgets spent and expired invites and removes invited devices whose time ended.
pub fn sweep(h: &Shared) {
    let now = unix_now();
    let expired = h.invites.locked().expire(now);
    let ended: Vec<(String, String)> = h
        .paired
        .read()
        .unwrap()
        .clients
        .iter()
        .filter(|c| invited(c).is_some_and(|guest| guest.ended(now)))
        .map(|c| (c.uuid.clone(), c.name.clone()))
        .collect();
    for (uuid, name) in &ended {
        tracing::info!(client = %name, "an invited device's time ended; it is removed");
        if let Err(error) = remove(h, uuid) {
            tracing::warn!(client = %name, error = %format!("{error:#}"), "an invited device whose time ended could not be removed");
        }
    }
    if expired && ended.is_empty() {
        crate::internet_play::refresh(h);
    }
}

/// Runs [`sweep`] until the host stops.
pub fn start(h: Shared) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(SWEEP_EVERY).await;
            if h.stop.load(std::sync::atomic::Ordering::Acquire) {
                return;
            }
            let host = h.clone();
            let _ = tokio::task::spawn_blocking(move || sweep(&host)).await;
        }
    })
}
