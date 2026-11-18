//! What an invite link's secret becomes, the same on the host and in Nightjar
//! (`rubylight_protocol::invite_uri`): the rendezvous secret that keys the friend's records,
//! punches and tunnel exactly as a paired device's secret keys them, and the one-time pairing PIN
//! the host matches (`otpauth`, as Artemis' one-time PIN pairing). Both are HKDF-SHA256 outputs
//! under this crate's domain, each with a label of its own, so neither reveals the other or the
//! link's secret.

use crate::crypto::{derive, derive_key, Key};

/// The passphrase every invite pairing sends with its PIN. The PIN carries the secret; the host
/// finds the invite by trying each live one.
pub const PASSPHRASE: &str = "invite";

/// The secret the invite's rendezvous runs on, in place of a paired device's secret.
pub fn rendezvous_secret(link_secret: &Key) -> Key {
    derive_key(link_secret, b"invite", b"rendezvous")
}

/// The one-time pairing PIN: 32 hex digits (128 bits). Moonlight's pairing keys its AES with
/// SHA-256 of the salt and the PIN, so a recorded pairing gives nobody a PIN to guess.
pub fn pairing_pin(link_secret: &Key) -> String {
    let mut pin = [0; 16];
    derive(link_secret, b"invite", b"pairing pin", &mut pin);
    pin.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_derived_values_are_stable_distinct_and_hide_the_secret() {
        let secret = [7u8; 32];
        let rendezvous = rendezvous_secret(&secret);
        let pin = pairing_pin(&secret);
        assert_eq!(rendezvous, rendezvous_secret(&secret));
        assert_eq!(pin, pairing_pin(&secret));
        assert_eq!(pin.len(), 32);
        assert!(pin.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(rendezvous, secret);
        assert!(!pin.contains("0707"));
        assert_ne!(rendezvous_secret(&[8; 32]), rendezvous);
        assert_ne!(pairing_pin(&[8; 32]), pin);
    }
}
