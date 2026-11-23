//! Pure invite error classification, known-PC certificate conflict check, and punched-tunnel
//! address detection shared across Nightjar Android and Desktop.

use std::net::Ipv4Addr;

use serde::{Deserialize, Serialize};

/// Canonical engine error string for a link that is not a Nightjar invite.
pub const NOT_AN_INVITE: &str = "This is not a Nightjar invite link";
/// Canonical engine error string for an expired invite.
pub const EXPIRED: &str = "This invite has expired. Ask for a new one.";
/// Canonical engine error string for an invite refused by the host.
pub const REFUSED: &str = "The PC did not accept this invite: it was already used, taken back or has expired. Ask for a new one.";
/// Canonical engine error string when internet play is disabled on the client.
pub const INTERNET_OFF: &str =
    "Turn on Play over the internet in Nightjar's settings, then open the invite again.";
/// Canonical error string when an invite names a known PC paired with another certificate.
pub const KNOWN_PC: &str =
    "This invite is for a PC that is already on your list with another certificate.";

/// Typed invite error category mapped to localized strings by each client UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InviteErrorKind {
    NotAnInvite = 0,
    Expired = 1,
    Refused = 2,
    InternetOff = 3,
    KnownPc = 4,
    Other = 5,
}

/// Classifies an invite error message into a known [`InviteErrorKind`].
pub fn classify_invite_error(msg: &str) -> InviteErrorKind {
    match msg {
        NOT_AN_INVITE => InviteErrorKind::NotAnInvite,
        EXPIRED => InviteErrorKind::Expired,
        REFUSED => InviteErrorKind::Refused,
        INTERNET_OFF => InviteErrorKind::InternetOff,
        KNOWN_PC => InviteErrorKind::KnownPc,
        _ => InviteErrorKind::Other,
    }
}

/// Whether an invite names a known PC that is already paired with a certificate whose SHA-256
/// differs from `link_fp_sha256`.
pub fn invite_claims_known_pc(
    existing_cert_sha256: Option<[u8; 32]>,
    link_fp_sha256: [u8; 32],
) -> bool {
    existing_cert_sha256.is_some_and(|pinned| pinned != link_fp_sha256)
}

/// Whether `ip` is the loopback endpoint of an internet play tunnel (`127.78.x.y`).
pub fn is_tunnel_address(ip: &str) -> bool {
    ip.trim()
        .parse::<Ipv4Addr>()
        .is_ok_and(|addr| addr.octets()[..2] == [127, 78])
}
