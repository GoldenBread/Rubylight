//! One-time pairing PINs, pending PIN requests, and pairing tray notice formatting.

use anyhow::Result;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

/// A one-time PIN that lets a client pair with a passphrase, valid
/// [`ONE_TIME_PIN_LIFETIME`] and for [`OtpMisses`] wrong tries.
pub struct OneTimePin {
    pub pin: String,
    pub passphrase: String,
    pub device_name: String,
    pub created: Instant,
    pub misses: OtpMisses,
    /// The access the owner chose ([`butterpollo_session::state::pairing_access`]).
    pub permission: Option<u32>,
}

/// Wrong one-time PIN answers by device address. Pairing needs no client
/// certificate, so one global count let any LAN device use up the invitation
/// meant for another.
#[derive(Default)]
pub struct OtpMisses {
    by_peer: HashMap<std::net::IpAddr, u8>,
    total: u8,
}
impl OtpMisses {
    /// Wrong answers one address may give.
    pub const PER_PEER: u8 = 5;
    /// Wrong answers from all addresses before the PIN is spent.
    pub const TOTAL: u8 = 32;
    pub fn refuses(&self, peer: std::net::IpAddr) -> bool {
        self.by_peer
            .get(&peer)
            .is_some_and(|misses| *misses >= Self::PER_PEER)
    }
    /// Counts a wrong answer from `peer`; false once the PIN is spent.
    pub fn miss(&mut self, peer: std::net::IpAddr) -> bool {
        *self.by_peer.entry(peer).or_default() += 1;
        self.total += 1;
        self.total < Self::TOTAL
    }
    /// Wrong answers from `peer` so far.
    #[cfg(any(test, feature = "test-support"))]
    pub fn count(&self, peer: std::net::IpAddr) -> u8 {
        self.by_peer.get(&peer).copied().unwrap_or(0)
    }
}

pub const ONE_TIME_PIN_LIFETIME: Duration = Duration::from_secs(180);
/// Why a one-time PIN pairing found no live PIN.
pub const ONE_TIME_PIN_SPENT: &str =
    "no one-time pairing code is live: it was used or has expired; show a new one on the PC";

impl OneTimePin {
    pub fn live(&self) -> bool {
        self.created.elapsed() < ONE_TIME_PIN_LIFETIME
    }
}

/// The two one-time PINs: the console's QR code and the PIN typed in Artemis.
/// Making one never replaces the other, and pairing consumes only the one
/// that matched.
#[derive(Default)]
pub struct OneTimePins {
    pub qr: Option<OneTimePin>,
    pub typed: Option<OneTimePin>,
}
impl OneTimePins {
    /// The live slot whose PIN and passphrase hash with `salt` to `auth`
    /// (Artemis' `otpauth`), taken out of its slot. Slots that refuse `peer`
    /// ([`OtpMisses::refuses`]) are not tried. A miss counts a wrong answer
    /// from `peer` against every slot tried and closes a slot once it is
    /// spent. Fails when no slot is live or every live slot refuses `peer`.
    pub fn take_matching(
        &mut self,
        auth: &str,
        salt: &str,
        peer: std::net::IpAddr,
    ) -> Result<Option<OneTimePin>> {
        let auth = hex::decode(auth).ok();
        let (mut live, mut tried) = (false, false);
        for slot in [&mut self.qr, &mut self.typed] {
            if slot.as_ref().is_some_and(|otp| !otp.live()) {
                *slot = None;
            }
            let Some(otp) = slot.as_ref() else { continue };
            live = true;
            if otp.misses.refuses(peer) {
                continue;
            }
            tried = true;
            let expected = butterpollo_crypto::crypto::hash(
                format!("{}{salt}{}", otp.pin, otp.passphrase).as_bytes(),
            );
            if auth
                .as_deref()
                .is_some_and(|auth| butterpollo_crypto::crypto::equal(&expected, auth))
            {
                return Ok(slot.take());
            }
        }
        if !live {
            // Used, expired or never shown: `pair` answers 400, which clients
            // show as a refused code rather than an unreachable PC.
            anyhow::bail!("{ONE_TIME_PIN_SPENT}");
        }
        if !tried {
            anyhow::bail!("too many wrong one-time PIN answers from this address");
        }
        for slot in [&mut self.qr, &mut self.typed] {
            if let Some(otp) = slot.as_mut()
                && !otp.misses.refuses(peer)
                && !otp.misses.miss(peer)
            {
                *slot = None;
            }
        }
        Ok(None)
    }
}

/// What the console sends a waiting pairing request: the PIN, the device's
/// name, the access the owner chose (`None` keeps the default) and the uuid of
/// the paired device that typed the PIN (`None` from the console).
pub type PinEntry = (String, String, Option<u32>, Option<String>);

pub struct PendingPin {
    pub name: String,
    /// The requesting device's certificate: Moonlight's apps share one
    /// unique ID, so this tells another device apart.
    pub certificate: String,
    /// Where the request came from, shown with the name: the name is the device's to choose.
    pub address: std::net::IpAddr,
    pub created: Instant,
    pub sender: tokio::sync::oneshot::Sender<PinEntry>,
}

/// Where a pairing request came from, as the owner reads it: loopback is this PC, which for a
/// browser far away means a tunnel or proxy on it; an internet play tunnel's range is a device on
/// the internet.
pub fn pairing_source(address: std::net::IpAddr) -> String {
    let address = address.to_canonical();
    if rubylight_traverse::peers::tunnel_range(address) {
        "a device over the internet".into()
    } else if address.is_loopback() {
        "this PC (a tunnel or proxy)".into()
    } else {
        address.to_string()
    }
}

/// The longest part of a device's name a pairing notice shows.
const NOTICE_NAME_CHARS: usize = 40;

/// The tray notice for a pairing request, title and text. The address is the title, apart from
/// the name the device chose, which is cut short so it can neither push the address out of the
/// notice (Windows keeps 255 UTF-16 units) nor pass for it.
pub fn pairing_notice(name: &str, address: std::net::IpAddr) -> (String, String) {
    let text = if pairing_from_internet(address) {
        "wants to pair from the internet. Enter its PIN in the Rubylight console only if you expect it."
    } else {
        "wants to pair. Enter the PIN it shows in the Rubylight console."
    };
    (
        format!("Pairing request from {}", pairing_source(address)),
        format!("{} {text}", notice_name(name)),
    )
}

/// Whether a pairing request came from outside the local network: a PIN pairing there crosses
/// networks nobody vouches for, so the owner should be expecting it.
pub fn pairing_from_internet(address: std::net::IpAddr) -> bool {
    crate::network::reach(address) == crate::network::Reach::Wan
}

fn notice_name(name: &str) -> String {
    let name = butterpollo_crypto::pairing::without_direction_controls(name);
    let mut shown: String = name.chars().take(NOTICE_NAME_CHARS).collect();
    if shown.len() < name.len() {
        shown.push('…');
    }
    shown
}

/// The tray notice when a device paired because another one approved it.
pub fn pairing_approved_notice(approver: &str, name: &str) -> (String, String) {
    (
        "Device paired".into(),
        format!(
            "{} approved {}. It can now stream from this PC.",
            notice_name(approver),
            notice_name(name)
        ),
    )
}

/// The tray notice when wrong PINs ended an address's requests on other devices.
pub fn pairing_ended_notice(address: std::net::IpAddr) -> (String, String) {
    (
        "Pairing ended".into(),
        format!(
            "Three wrong PINs from {}. Other devices can't approve it for now; the Rubylight console still can.",
            pairing_source(address)
        ),
    )
}
