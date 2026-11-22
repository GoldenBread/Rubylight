//! `GET /session/guests` and its actions: the devices that joined the running game as guests,
//! the joins waiting for an answer, and what the device playing (or one with Host control) does
//! about them, while the host lists [`crate::feature::SESSION_GUESTS`].
use serde::{Deserialize, Serialize};

/// The permissions a guest can have, as the wire spells them.
pub const PERMISSIONS: [&str; 3] = ["view", "controller", "controller-keyboard-mouse"];

/// `GET /session/guests`, and the answer of every action.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionGuests {
    /// The host's `guest_join`: `ask`, `allow` or `off`.
    pub policy: String,
    /// What a guest may do when nobody chose otherwise, one of [`PERMISSIONS`].
    pub default_permission: String,
    /// Whether the caller may answer joins and manage guests: the device playing, or one with
    /// Host control.
    pub manage: bool,
    /// The caller's own guest id, when it is a guest of the running game.
    pub you: Option<u32>,
    /// The name of the device playing, when a game runs.
    pub owner: Option<String>,
    pub max_guests: u32,
    /// Changes with every change to the list.
    pub version: u64,
    /// The guest holding the keyboard and mouse; none while the device playing holds them.
    pub keyboard_mouse: Option<u32>,
    #[serde(default)]
    pub guests: Vec<SessionGuest>,
    #[serde(default)]
    pub waiting: Vec<JoinRequest>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionGuest {
    pub id: u32,
    pub name: String,
    /// One of [`PERMISSIONS`].
    pub permission: String,
    /// Whether its stream runs now.
    pub streaming: bool,
    /// Unix seconds it was let in.
    pub since: u64,
    /// Unix seconds its time ends; none lasts until the game ends.
    pub expires: Option<u64>,
}

/// A device waiting for an answer to its join.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinRequest {
    pub id: u32,
    pub name: String,
    /// Until the join gives up.
    pub seconds_left: u64,
}

/// `POST /session/guests/approve`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuestApprove {
    pub id: u32,
    /// One of [`PERMISSIONS`]; the host's default without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission: Option<String>,
    /// How long the guest may stay, 300 to 604800 seconds; until the game ends without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<u64>,
}

/// `POST /session/guests/deny`, `/kick` and `/revoke`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuestId {
    pub id: u32,
}

/// `POST /session/guests/permission`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuestPermission {
    pub id: u32,
    pub permission: String,
}

/// `POST /session/guests/keyboard-mouse`: hands the keyboard and mouse to guest `id`, or back to
/// the device playing with `null`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuestKeyboardMouse {
    pub id: Option<u32>,
}

/// `POST /session/guests/invites`: an invite to play over the internet. The caller needs Approve
/// new devices or Host control; the invite never gives more input than the caller has.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InviteCreate {
    /// One of [`PERMISSIONS`]; the host's default without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission: Option<String>,
    /// How long the link, and the access it gives, work: 300 to 604800 seconds; a day without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<u64>,
}

/// The answer to [`InviteCreate`]: the link, which the host never shows again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InviteCreated {
    pub id: u32,
    /// `rubylight://invite#...`: a secret, to send to one person.
    pub link: String,
    /// Unix seconds the link, and the access it gives, end.
    pub expires: u64,
    /// One of [`PERMISSIONS`].
    pub permission: String,
}

/// `GET /session/guests/invites`, and the answer of `/session/guests/invites/revoke`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestInvites {
    /// Links nobody used yet.
    #[serde(default)]
    pub links: Vec<InviteLink>,
    /// Devices paired from an invite, until their time ends.
    #[serde(default)]
    pub friends: Vec<InvitedFriend>,
    pub max_links: u32,
    pub default_seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InviteLink {
    pub id: u32,
    pub permission: String,
    pub created: u64,
    pub expires: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvitedFriend {
    pub uuid: String,
    pub name: String,
    pub permission: String,
    /// Unix seconds its access ends and the host removes it.
    pub until: u64,
}

/// `POST /session/guests/invites/revoke`: `id` takes back a link nobody used yet, `uuid` removes a
/// device paired from an invite. Exactly one of them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InviteRevoke {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_reads_without_its_optional_parts() {
        let read: SessionGuests = serde_json::from_str(
            r#"{"policy":"ask","default_permission":"controller","manage":true,"you":null,
                "owner":"Living room","max_guests":4,"version":3,"keyboard_mouse":null,
                "guests":[{"id":2,"name":"Phone","permission":"view","streaming":true,
                           "since":1800000000,"expires":null}],
                "future":"ignored"}"#,
        )
        .unwrap();
        assert_eq!(read.guests[0].name, "Phone");
        assert!(read.waiting.is_empty());
    }

    #[test]
    fn actions_refuse_unknown_fields() {
        assert!(serde_json::from_str::<GuestId>(r#"{"id":1,"extra":true}"#).is_err());
        let approve: GuestApprove = serde_json::from_str(r#"{"id":4}"#).unwrap();
        assert_eq!(approve.permission, None);
        assert_eq!(serde_json::to_string(&approve).unwrap(), r#"{"id":4}"#);
        let give: GuestKeyboardMouse = serde_json::from_str(r#"{"id":null}"#).unwrap();
        assert_eq!(give.id, None);
    }

    #[test]
    fn invites_read_without_their_optional_parts_and_refuse_unknown_fields() {
        let create: InviteCreate = serde_json::from_str("{}").unwrap();
        assert_eq!(create, InviteCreate::default());
        assert_eq!(serde_json::to_string(&create).unwrap(), "{}");
        assert!(serde_json::from_str::<InviteCreate>(r#"{"seconds":300,"host":1}"#).is_err());
        assert!(serde_json::from_str::<InviteRevoke>(r#"{"id":1,"x":2}"#).is_err());
        let read: GuestInvites =
            serde_json::from_str(r#"{"max_links":8,"default_seconds":86400,"later":1}"#).unwrap();
        assert!(read.links.is_empty() && read.friends.is_empty());
    }
}
