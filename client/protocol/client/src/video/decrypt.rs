//! `SS_ENC_VIDEO`: each datagram is a 32-byte `ENC_VIDEO_HEADER` (12-byte IV, frame number,
//! 16-byte tag) followed by the AES-128-GCM encrypted RTP packet, without associated data.

use ring::aead::{AES_128_GCM, Aad, LessSafeKey, Nonce, Tag, UnboundKey};

use super::ENC_VIDEO_HEADER_SIZE;

pub(super) struct Decryptor {
    cipher: LessSafeKey,
}

impl Decryptor {
    pub(super) fn new(key: &[u8; 16]) -> Self {
        let unbound = UnboundKey::new(&AES_128_GCM, key).expect("16-byte AES-128 key");
        Self {
            cipher: LessSafeKey::new(unbound),
        }
    }

    /// Decrypts `datagram` into `out` and returns the plaintext length, or `None` if it does
    /// not authenticate or does not fit.
    pub(super) fn open(&self, datagram: &[u8], out: &mut [u8]) -> Option<usize> {
        let ciphertext = &datagram[ENC_VIDEO_HEADER_SIZE..];
        let plain = out.get_mut(..ciphertext.len())?;
        plain.copy_from_slice(ciphertext);
        let iv: [u8; 12] = datagram[..12].try_into().ok()?;
        let tag: [u8; 16] = datagram[16..32].try_into().ok()?;
        self.cipher
            .open_in_place_separate_tag(
                Nonce::assume_unique_for_key(iv),
                Aad::empty(),
                Tag::from(tag),
                plain,
                0..,
            )
            .ok()?;
        Some(ciphertext.len())
    }
}
