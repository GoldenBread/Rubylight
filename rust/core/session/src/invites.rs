//! Invites to play over the internet (docs/multiclient.md, "Invite a friend over the internet"),
//! as pure state: no sockets, no clock, no global state; the caller passes the time in.
//!
//! An invite is a link with a 32-byte secret. Nightjar reaches the PC with a rendezvous secret
//! derived from it and pairs with a one-time PIN derived from it; the host keeps both, never the
//! link. Pairing spends the invite and pairs the device as an invited guest: it may join the game
//! being played when the player lets it in (`guest_join` asks for it whatever its value), with the
//! invite's guest permission, until the invite's time ends. [`Invites`] holds the links; the
//! paired device carries [`MARKER`] in its `extra`, read by [`invited_guest`].
use crate::state::{Client, LIST_APPS, VIEW_STREAMS};
pub use crate::webrtc::guests::Permission;
use serde_json::{Value, json};
use std::time::Duration;

/// The paired-device `extra` key of a device paired from an invite: `{"until": unix seconds,
/// "permission": "controller"}`.
pub const MARKER: &str = "invited_guest";
/// Invites waiting at once.
pub const MAX_LIVE: usize = 8;
/// How long an invite works when nobody chose: the link, and the friend's access after it.
pub const DEFAULT_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);
pub const MIN_LIFETIME: Duration = crate::multiclient::MIN_LIFETIME;
pub const MAX_LIFETIME: Duration = crate::multiclient::MAX_LIFETIME;
/// How long a spent invite's rendezvous goes on, in seconds: the friend's Nightjar fetches its own
/// secret over the invite's path right after pairing and then moves to its own.
pub const SPENT_GRACE: u64 = 10 * 60;
/// The passphrase Nightjar sends with an invite's PIN (`rubylight_traverse::invite::PASSPHRASE`).
pub const PASSPHRASE: &str = "invite";

/// Why an invite was not made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreateError {
    TooMany,
    Lifetime,
}

impl CreateError {
    pub fn message(self) -> &'static str {
        match self {
            Self::TooMany => "As many invites are waiting as this PC keeps: take one back first",
            Self::Lifetime => "An invite works from 5 minutes to 7 days",
        }
    }
}

/// The device that spent an invite.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Redeemed {
    pub at: u64,
    /// The paired device's uuid.
    pub device: String,
    pub name: String,
}

/// One invite.
#[derive(Clone, PartialEq, Eq)]
pub struct Invite {
    pub id: u32,
    /// The secret the invite's rendezvous runs on (derived from the link's secret).
    pub rendezvous: [u8; 32],
    /// The one-time pairing PIN (derived from the link's secret).
    pub pin: String,
    pub permission: Permission,
    /// Unix seconds.
    pub created: u64,
    /// Unix seconds the link, and the access it gives, end.
    pub expires: u64,
    /// The uuid of the paired device that made it; none from the console.
    pub creator: Option<String>,
    pub redeemed: Option<Redeemed>,
}

/// Leaves out the secrets, so no log can carry them.
impl std::fmt::Debug for Invite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Invite")
            .field("id", &self.id)
            .field("permission", &self.permission)
            .field("expires", &self.expires)
            .field("redeemed", &self.redeemed)
            .finish_non_exhaustive()
    }
}

impl Invite {
    /// Whether the link still pairs a device at `now`.
    pub fn live(&self, now: u64) -> bool {
        self.redeemed.is_none() && now < self.expires
    }
}

/// The host's invites.
#[derive(Debug, Default)]
pub struct Invites {
    list: Vec<Invite>,
    next: u32,
    version: u64,
}

impl Invites {
    /// Changes with every change to the list.
    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn list(&self) -> &[Invite] {
        &self.list
    }

    fn changed(&mut self) {
        self.version = self.version.wrapping_add(1);
    }

    /// Makes an invite that works for `lifetime` from `now`. Expired and spent ones are swept
    /// first, so only live invites count against [`MAX_LIVE`].
    pub fn create(
        &mut self,
        rendezvous: [u8; 32],
        pin: String,
        permission: Permission,
        lifetime: Duration,
        creator: Option<String>,
        now: u64,
    ) -> Result<&Invite, CreateError> {
        if lifetime < MIN_LIFETIME || lifetime > MAX_LIFETIME {
            return Err(CreateError::Lifetime);
        }
        self.expire(now);
        if self.list.iter().filter(|i| i.live(now)).count() >= MAX_LIVE {
            return Err(CreateError::TooMany);
        }
        self.next = self.next.wrapping_add(1).max(1);
        self.list.push(Invite {
            id: self.next,
            rendezvous,
            pin,
            permission,
            created: now,
            expires: now + lifetime.as_secs(),
            creator,
            redeemed: None,
        });
        self.changed();
        Ok(self.list.last().unwrap())
    }

    /// The live invite whose PIN hashes with `salt` (the pairing's salt, as hex) to `auth`
    /// (`otpauth`), compared in constant time against every live invite.
    pub fn matching(&self, auth: &[u8], salt: &str, now: u64) -> Option<&Invite> {
        let mut found = None;
        for invite in self.list.iter().filter(|i| i.live(now)) {
            let expected = butterpollo_crypto::crypto::hash(
                format!("{}{salt}{PASSPHRASE}", invite.pin).as_bytes(),
            );
            if butterpollo_crypto::crypto::equal(&expected, auth) && found.is_none() {
                found = Some(invite);
            }
        }
        found
    }

    /// Spends invite `id` on paired device `device` at the end of its pairing. Only a live
    /// invite can be spent, once: of two pairings with one link, the second finds it spent.
    pub fn redeem(&mut self, id: u32, device: &str, name: &str, now: u64) -> Option<Invite> {
        let invite = self.list.iter_mut().find(|i| i.id == id && i.live(now))?;
        invite.redeemed = Some(Redeemed {
            at: now,
            device: device.to_owned(),
            name: name.to_owned(),
        });
        let spent = invite.clone();
        self.changed();
        Some(spent)
    }

    /// Takes back invite `id`, used or not.
    pub fn revoke(&mut self, id: u32) -> Option<Invite> {
        let at = self.list.iter().position(|i| i.id == id)?;
        self.changed();
        Some(self.list.remove(at))
    }

    /// Forgets invites past their time, and spent ones once their rendezvous grace is over.
    /// True when anything went.
    pub fn expire(&mut self, now: u64) -> bool {
        let before = self.list.len();
        self.list.retain(|i| match &i.redeemed {
            None => now < i.expires,
            Some(spent) => now < spent.at + SPENT_GRACE,
        });
        let gone = self.list.len() != before;
        if gone {
            self.changed();
        }
        gone
    }

    /// The rendezvous secrets to look for at `now`, by invite id: live invites, and spent ones
    /// for [`SPENT_GRACE`].
    pub fn rendezvous(&self, now: u64) -> Vec<(u32, [u8; 32])> {
        self.list
            .iter()
            .filter(|i| match &i.redeemed {
                None => now < i.expires,
                Some(spent) => now < spent.at + SPENT_GRACE,
            })
            .map(|i| (i.id, i.rendezvous))
            .collect()
    }

    /// Whether any invite still needs the rendezvous.
    pub fn any(&self, now: u64) -> bool {
        !self.rendezvous(now).is_empty()
    }
}

/// What a device paired from an invite may do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvitedGuest {
    /// Unix seconds its access ends.
    pub until: u64,
    pub permission: Permission,
}

impl InvitedGuest {
    pub fn ended(&self, now: u64) -> bool {
        now >= self.until
    }

    pub fn marker(&self) -> Value {
        json!({"until": self.until, "permission": self.permission.as_str()})
    }
}

/// The device's invite, when it was paired from one. A marker that does not read is still an
/// invited guest with no time left and no input: the safe reading.
pub fn invited_guest(client: &Client) -> Option<InvitedGuest> {
    let marker = client.extra.get(MARKER)?;
    Some(InvitedGuest {
        until: marker.get("until").and_then(Value::as_u64).unwrap_or(0),
        permission: marker
            .get("permission")
            .and_then(Value::as_str)
            .and_then(Permission::parse)
            .unwrap_or(Permission::View),
    })
}

/// The permission bits of a device paired from an invite: list and watch, and the input its
/// guest permission takes. Never launch, clipboard, files, server commands or host control.
pub fn device_permission(permission: Permission) -> u32 {
    LIST_APPS | VIEW_STREAMS | permission.input_mask()
}

/// The permission an invite made by a device with permission bits `creator` may give: never
/// more input than the creator has. None when the creator could not even watch.
pub fn capped(permission: Permission, creator: u32) -> Option<Permission> {
    if creator & (LIST_APPS | VIEW_STREAMS) != LIST_APPS | VIEW_STREAMS {
        return None;
    }
    [
        Permission::ControllerKeyboardMouse,
        Permission::Controller,
        Permission::View,
    ]
    .into_iter()
    .filter(|p| p.input_mask() & !permission.input_mask() == 0)
    .find(|p| p.input_mask() & creator == p.input_mask())
}

#[cfg(test)]
mod tests;
