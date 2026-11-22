//! Approve or deny another device's pairing request from a paired device (`docs/native-api.md`).
//! Holds the pending approval state and strike/cooldown bookkeeping shared between `nvhttp`,
//! `butterpollo-webrtc`, and `butterpollo-web`.

use crate::state::Host;
use butterpollo_session::{
    nucleus::coop,
    state::{
        Client, LATER_DEVICE_PERMISSION, LAUNCH_APPS, PAIRING_APPROVE_KEY, PLAY_DEVICE_PERMISSION,
        PairedState,
    },
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    net::IpAddr,
    sync::PoisonError,
    time::{Duration, Instant},
};
use tokio::sync::oneshot;

/// How long an approval waits for the new device to finish. Below the 15 s a
/// client waits for an action's answer.
#[cfg(not(any(test, feature = "test-support")))]
pub const HANDSHAKE_WAIT: Duration = Duration::from_secs(10);
#[cfg(any(test, feature = "test-support"))]
pub const HANDSHAKE_WAIT: Duration = Duration::from_secs(2);
/// As long as a waiting request and a started handshake live.
pub const REQUEST_LIFETIME: Duration = Duration::from_secs(300);
/// Wrong PINs that end an address's requests on this path.
pub const WRONG_PIN_LIMIT: u8 = 3;
/// How long those wrong PINs are remembered after the last one.
pub const WRONG_PIN_MEMORY: Duration = Duration::from_secs(300);
/// How long an address waits for its next approval after a wrong PIN.
pub const WRONG_PIN_COOLDOWN: Duration = Duration::from_secs(30);
/// A little past the handshake's own limit, so a late finish is still capped.
const APPROVAL_LIFETIME: Duration = Duration::from_secs(310);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Paired,
    WrongPin,
    /// The approver lost what this takes, named here. Not the new device's doing: no strike.
    ApproverRevoked(&'static str),
    /// The PIN was right but the host could not save the device: no strike either.
    NotSaved,
}

/// What an approver needs to hand on anything of Play.
pub const LIST_AND_VIEW: &str = "List applications and View streams";

/// Why [`capped`] refused: the device that approved lost `missing`.
#[derive(Debug)]
pub struct ApproverRevoked {
    pub missing: &'static str,
}

impl std::fmt::Display for ApproverRevoked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the device that approved this pairing no longer holds {}",
            self.missing
        )
    }
}

impl std::error::Error for ApproverRevoked {}

/// A handshake that was right, whose device could not be saved.
#[derive(Debug)]
pub struct NotSaved;

impl std::fmt::Display for NotSaved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the paired device could not be saved")
    }
}

impl std::error::Error for NotSaved {}

/// How an approval reports back, with what its log lines name.
pub struct Approval {
    pub approver_name: String,
    pub device: String,
    pub address: IpAddr,
    pub created: Instant,
    /// Taken when the wait gives up; a late finish still pairs under the cap.
    pub outcome: Option<oneshot::Sender<Outcome>>,
}

/// The device that approved a pairing, for the notice and the webhook.
pub struct Approver {
    pub uuid: String,
    pub name: String,
}

/// What the wrong PINs count against: the address, an IPv6 /64 as one.
fn counted_as(address: IpAddr) -> IpAddr {
    crate::network::peer_group(address.to_canonical())
}

/// The approvals whose new device has not finished yet, by request id, and the
/// wrong PINs per address.
#[derive(Default)]
pub struct Approvals {
    pub running: HashMap<String, Approval>,
    pub wrong_pins: HashMap<IpAddr, (u8, Instant)>,
}

impl Approvals {
    pub fn prune(&mut self) {
        self.running
            .retain(|_, approval| approval.created.elapsed() < APPROVAL_LIFETIME);
        self.wrong_pins
            .retain(|_, (_, last)| last.elapsed() < WRONG_PIN_MEMORY);
    }

    pub fn ended(&mut self, address: IpAddr) -> bool {
        self.prune();
        self.wrong_pins
            .get(&counted_as(address))
            .is_some_and(|(count, _)| *count >= WRONG_PIN_LIMIT)
    }

    pub fn cooling(&mut self, address: IpAddr) -> Option<u64> {
        self.prune();
        let (_, last) = self.wrong_pins.get(&counted_as(address))?;
        let left = WRONG_PIN_COOLDOWN.checked_sub(last.elapsed())?;
        Some(left.as_secs() + 1)
    }

    pub fn strike(&mut self, address: IpAddr) -> bool {
        let entry = self
            .wrong_pins
            .entry(counted_as(address))
            .or_insert((0, Instant::now()));
        *entry = (entry.0.saturating_add(1), Instant::now());
        entry.0 == WRONG_PIN_LIMIT
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Approvals {
    pub fn is_idle(&self) -> bool {
        self.running.is_empty()
    }

    pub fn wrong_pins_at(&self, address: &str) -> u8 {
        self.wrong_pins
            .get(&counted_as(address.parse().unwrap()))
            .map_or(0, |(count, _)| *count)
    }

    pub fn strike_at(&mut self, address: IpAddr) {
        self.strike(address);
    }

    pub fn cool_down(&mut self) {
        for (_, last) in self.wrong_pins.values_mut() {
            *last -= WRONG_PIN_COOLDOWN;
        }
    }
}

pub fn approvals(h: &Host) -> std::sync::MutexGuard<'_, Approvals> {
    h.approvals.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn loud_strike(h: &Host, address: IpAddr, device: &str) {
    if !approvals(h).strike(address) {
        return;
    }
    tracing::warn!(client = %device, %address, "Three wrong PINs from other devices: this address can now only be paired in the console");
    let (title, text) = crate::state::pairing_ended_notice(address);
    butterpollo_win_io::tray::notify(&title, &text);
}

/// What a device gets for pairing, and who approved it.
pub struct Granted {
    pub perm: u32,
    pub extra: BTreeMap<String, Value>,
    pub approver: Option<Approver>,
}

/// What a device gets when `approver` typed its PIN: never more than the approver holds now,
/// and nothing if it may no longer approve.
pub fn capped(
    state: &PairedState,
    approver: Option<&str>,
    perm: u32,
    mut extra: BTreeMap<String, Value>,
) -> anyhow::Result<Granted> {
    let Some(uuid) = approver else {
        return Ok(Granted {
            perm,
            extra,
            approver: None,
        });
    };
    let approver = state
        .clients
        .iter()
        .find(|c| {
            c.uuid == butterpollo_session::state::device_uuid(uuid) && c.may_approve_pairing()
        })
        .ok_or(ApproverRevoked {
            missing: "Approve new devices",
        })?;
    let perm = perm & approver.perm;
    if perm == 0 {
        return Err(ApproverRevoked {
            missing: LIST_AND_VIEW,
        }
        .into());
    }
    extra.remove(PAIRING_APPROVE_KEY);
    if perm & LAUNCH_APPS != 0 && !approver.allows_coop_join() {
        extra.insert(coop::DEVICE_KEY.into(), false.into());
    }
    Ok(Granted {
        perm,
        extra,
        approver: Some(Approver {
            uuid: approver.uuid.clone(),
            name: approver.name.clone(),
        }),
    })
}

pub fn paired(h: &Host, id: &str, approver: &Approver, device: &str, address: IpAddr) {
    let address = address.to_canonical();
    let mut approvals = approvals(h);
    let running = approvals.running.remove(id);
    approvals.wrong_pins.remove(&counted_as(address));
    drop(approvals);
    if let Some(outcome) = running.and_then(|mut approval| approval.outcome.take()) {
        let _ = outcome.send(Outcome::Paired);
    }
    tracing::warn!(approver = %approver.name, client = %device, %address, "Device paired, approved from another device");
}

pub fn paired_by_owner(h: &Host, address: IpAddr) {
    approvals(h)
        .wrong_pins
        .remove(&counted_as(address.to_canonical()));
}

pub fn failed(h: &Host, id: &str) {
    let Some(mut approval) = approvals(h).running.remove(id) else {
        return;
    };
    let Some(outcome) = approval.outcome.take() else {
        return;
    };
    tracing::warn!(approver = %approval.approver_name, client = %approval.device, address = %approval.address, "Pairing approval failed: the device did not prove it knows the PIN");
    loud_strike(h, approval.address, &approval.device);
    let _ = outcome.send(Outcome::WrongPin);
}

pub fn approver_revoked(h: &Host, id: &str, missing: &'static str) {
    let Some(mut approval) = approvals(h).running.remove(id) else {
        tracing::warn!(
            missing,
            "Pairing refused: the device that approved it lost this"
        );
        return;
    };
    tracing::warn!(approver = %approval.approver_name, client = %approval.device, address = %approval.address, missing, "Pairing refused: the device that approved it lost this");
    if let Some(outcome) = approval.outcome.take() {
        let _ = outcome.send(Outcome::ApproverRevoked(missing));
    }
}

pub fn not_saved(h: &Host, id: &str) {
    let Some(mut approval) = approvals(h).running.remove(id) else {
        return;
    };
    tracing::warn!(approver = %approval.approver_name, client = %approval.device, address = %approval.address, "Pairing approval: the device could not be saved");
    if let Some(outcome) = approval.outcome.take() {
        let _ = outcome.send(Outcome::NotSaved);
    }
}

/// Permissions handed on when approving a pairing request: `None` for WebRTC, `Some(play)` for Moonlight.
pub fn approval_permission(client: &Client, is_webrtc: bool) -> Result<Option<u32>, &'static str> {
    if is_webrtc {
        if !client.allows(LATER_DEVICE_PERMISSION) {
            return Err(LIST_AND_VIEW);
        }
        Ok(None)
    } else {
        let play = PLAY_DEVICE_PERMISSION & client.perm;
        if play == 0 {
            return Err(LIST_AND_VIEW);
        }
        Ok(Some(play))
    }
}
