//! Control-stream encryption, replay protection, and legacy input framing shared by host and client.
use crate::crypto;
use anyhow::{Result, bail};

const REPLAY_WINDOW: u32 = 4096;
/// Authenticate a message before recording its sequence. Moonlight numbers
/// control messages with one counter across ENet channels, which are only
/// ordered within themselves: a resent keyframe request can arrive after
/// hundreds of 200 Hz motion reports on other channels. A 64-entry window
/// rejected it as a replay (a stall of a second or two, or a stuck key);
/// accept unseen sequences within a wider window instead.
pub struct ReplayWindow {
    newest: Option<u32>,
    seen: Box<[u64; REPLAY_WINDOW as usize / 64]>,
}

impl Default for ReplayWindow {
    fn default() -> Self {
        Self {
            newest: None,
            seen: Box::new([0; REPLAY_WINDOW as usize / 64]),
        }
    }
}

impl ReplayWindow {
    fn slot(sequence: u32) -> (usize, u64) {
        let index = (sequence % REPLAY_WINDOW) as usize;
        (index / 64, 1 << (index % 64))
    }

    fn mark(&mut self, sequence: u32) {
        let (word, bit) = Self::slot(sequence);
        self.seen[word] |= bit;
    }

    pub fn accept(&mut self, sequence: u32) -> bool {
        let Some(newest) = self.newest else {
            self.newest = Some(sequence);
            self.mark(sequence);
            return true;
        };
        if sequence > newest {
            let distance = sequence - newest;
            if distance >= REPLAY_WINDOW {
                self.seen.fill(0);
            } else {
                // These slots held sequences one window older.
                for skipped in 1..=distance {
                    let (word, bit) = Self::slot(newest.wrapping_add(skipped));
                    self.seen[word] &= !bit;
                }
            }
            self.newest = Some(sequence);
            self.mark(sequence);
            true
        } else {
            let (word, bit) = Self::slot(sequence);
            if newest - sequence >= REPLAY_WINDOW || self.seen[word] & bit != 0 {
                return false;
            }
            self.seen[word] |= bit;
            true
        }
    }
}

pub fn control_header(b: &[u8]) -> Result<(u16, &[u8])> {
    if b.len() < 2 {
        bail!("short control packet");
    }
    Ok((u16::from_le_bytes(b[..2].try_into().unwrap()), &b[2..]))
}

/// The control stream's AES-GCM IV and its length. Version 2 marks the
/// direction (`H` from the host, `C` from the client) and carries the whole
/// sequence; version 1 only its low byte, as in moonlight-common-c.
fn control_iv(seq: u32, v2: bool, direction: u8) -> ([u8; 16], usize) {
    let mut iv = [0; 16];
    if v2 {
        iv[..4].copy_from_slice(&seq.to_le_bytes());
        iv[10] = direction;
        iv[11] = b'C';
        (iv, 12)
    } else {
        iv[0] = seq as u8;
        (iv, 16)
    }
}

pub fn encrypted_control(
    key: &[u8; 16],
    seq: u32,
    kind: u16,
    payload: &[u8],
    v2: bool,
) -> Result<Vec<u8>> {
    ControlCipher::new(key, v2).seal(seq, kind, payload)
}

pub fn decrypt_control(key: &[u8; 16], packet: &[u8], v2: bool) -> Result<(u32, u16, Vec<u8>)> {
    ControlCipher::new(key, v2).open(packet)
}

/// A control stream's AES-GCM key, expanded once rather than for every message
/// ([`encrypted_control`] and [`decrypt_control`] make one per call).
pub struct ControlCipher {
    key: [u8; 16],
    v2: bool,
    gcm: crypto::GcmKey,
}

impl ControlCipher {
    pub fn new(key: &[u8; 16], v2: bool) -> Self {
        Self {
            key: *key,
            v2,
            // Version 1's IV is 16 bytes, version 2's 12 ([`control_iv`]).
            gcm: crypto::GcmKey::sized(key, !v2),
        }
    }

    /// Whether this is the cipher for `key` and protocol version `v2`.
    pub fn is_for(&self, key: &[u8; 16], v2: bool) -> bool {
        self.v2 == v2 && crypto::equal(&self.key, key)
    }

    /// Message `seq` from the host, of `kind`, as the client receives it.
    pub fn seal(&self, seq: u32, kind: u16, payload: &[u8]) -> Result<Vec<u8>> {
        if payload.len() > u16::MAX as usize - 24 {
            bail!("control packet too large");
        }
        let (iv, iv_len) = control_iv(seq, self.v2, b'H');
        let mut p = Vec::with_capacity(28 + payload.len());
        p.extend_from_slice(&1u16.to_le_bytes());
        p.extend_from_slice(&((24 + payload.len()) as u16).to_le_bytes());
        p.extend_from_slice(&seq.to_le_bytes());
        p.extend_from_slice(&[0; 16]);
        p.extend_from_slice(&kind.to_le_bytes());
        p.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        p.extend_from_slice(payload);
        let tag = self.gcm.seal_in_place(&iv[..iv_len], &mut p[24..])?;
        p[8..24].copy_from_slice(&tag);
        Ok(p)
    }

    /// A client's encrypted control `packet`: its sequence, kind and payload.
    pub fn open(&self, packet: &[u8]) -> Result<(u32, u16, Vec<u8>)> {
        let mut scratch = Vec::with_capacity(packet.len().saturating_sub(24));
        let (seq, kind, _) = self.open_into(packet, &mut scratch)?;
        scratch.drain(..4);
        Ok((seq, kind, scratch))
    }

    /// [`Self::open`] decrypting into `scratch` and borrowing the payload
    /// (`&scratch[4..]`) with no per-packet allocation or header shift.
    pub fn open_into<'a>(
        &self,
        packet: &[u8],
        scratch: &'a mut Vec<u8>,
    ) -> Result<(u32, u16, &'a [u8])> {
        if packet.len() < 28 || u16::from_le_bytes(packet[..2].try_into().unwrap()) != 1 {
            bail!("short encrypted control packet");
        }
        let len = u16::from_le_bytes(packet[2..4].try_into().unwrap()) as usize;
        if len + 4 != packet.len() {
            bail!("control length mismatch");
        }
        let seq = u32::from_le_bytes(packet[4..8].try_into().unwrap());
        let (iv, iv_len) = control_iv(seq, self.v2, b'C');
        let (tag, sealed) = (&packet[8..24], &packet[24..]);
        let plain = self.gcm.open_into(&iv[..iv_len], tag, sealed, scratch)?;
        if plain.len() < 4 {
            bail!("short decrypted control header");
        }
        let kind = u16::from_le_bytes(plain[..2].try_into().unwrap());
        if u16::from_le_bytes(plain[2..4].try_into().unwrap()) as usize != plain.len() - 4 {
            bail!("decrypted control length mismatch");
        }
        Ok((seq, kind, &plain[4..]))
    }
}

/// A peer's [`ControlCipher`], made again only when its session's key or
/// protocol version is another than the one it was made for.
#[derive(Default)]
pub struct ControlCiphers {
    cipher: Option<ControlCipher>,
    /// How often a cipher was made, for the tests.
    #[cfg(test)]
    made: usize,
}

impl ControlCiphers {
    pub fn get(&mut self, key: &[u8; 16], v2: bool) -> &ControlCipher {
        let cipher = &mut self.cipher;
        if cipher.as_ref().is_some_and(|c| !c.is_for(key, v2)) {
            *cipher = None;
        }
        #[cfg(test)]
        if cipher.is_none() {
            self.made += 1;
        }
        cipher.get_or_insert_with(|| ControlCipher::new(key, v2))
    }
}

/// NVIDIA's separate input cipher chains the next IV from the last 16 bytes
/// of each authenticated message. Never advance it on a forged packet.
pub struct LegacyInput {
    pub iv: [u8; 16],
    cached: Option<([u8; 16], crypto::GcmKey)>,
}

impl LegacyInput {
    pub fn new(key_id: u32) -> Self {
        let mut iv = [0; 16];
        iv[..4].copy_from_slice(&key_id.to_be_bytes());
        Self { iv, cached: None }
    }

    fn gcm(&mut self, key: &[u8; 16]) -> &crypto::GcmKey {
        if self
            .cached
            .as_ref()
            .is_some_and(|(cached_key, _)| !crypto::equal(cached_key, key))
        {
            self.cached = None;
        }
        &self
            .cached
            .get_or_insert_with(|| (*key, crypto::GcmKey::sized(key, true)))
            .1
    }

    pub fn open(&mut self, key: &[u8; 16], payload: &[u8]) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        self.open_into(key, payload, &mut out)?;
        Ok(out)
    }

    /// [`Self::open`] decrypting into `scratch` with a cached AES-GCM key.
    pub fn open_into<'a>(
        &mut self,
        key: &[u8; 16],
        payload: &[u8],
        scratch: &'a mut Vec<u8>,
    ) -> Result<&'a [u8]> {
        if payload.len() < 20 {
            bail!("short legacy input message");
        }
        let len = u32::from_be_bytes(payload[..4].try_into().unwrap()) as usize;
        if len < 16 || len + 4 != payload.len() {
            bail!("legacy input length mismatch");
        }
        let tagged = &payload[4..];
        let iv = self.iv;
        self.gcm(key)
            .open_into(&iv, &tagged[..16], &tagged[16..], scratch)?;
        if len >= 32 {
            self.iv.copy_from_slice(&tagged[len - 16..]);
        }
        Ok(scratch.as_slice())
    }
}

#[cfg(test)]
mod control_cipher_tests;

#[cfg(test)]
mod open_into_tests {
    use super::*;

    #[test]
    fn control_and_legacy_open_into_match_allocating_open() {
        let key_a = [0x11u8; 16];
        let key_b = [0x22u8; 16];
        for v2 in [false, true] {
            let cipher = ControlCipher::new(&key_a, v2);
            let mut scratch = Vec::with_capacity(64);
            let ptr = scratch.as_ptr();
            for seq in 0..4u32 {
                let (iv, iv_len) = control_iv(seq, v2, b'C');
                let mut plain = 0x0206u16.to_le_bytes().to_vec();
                let body = b"mouse move";
                plain.extend_from_slice(&(body.len() as u16).to_le_bytes());
                plain.extend_from_slice(body);
                let (tag, sealed) = crypto::gcm_seal(&key_a, &iv[..iv_len], &plain).unwrap();
                let mut pkt = 1u16.to_le_bytes().to_vec();
                pkt.extend_from_slice(&((20 + plain.len()) as u16).to_le_bytes());
                pkt.extend_from_slice(&seq.to_le_bytes());
                pkt.extend_from_slice(&tag);
                pkt.extend_from_slice(&sealed);
                let (got_seq, got_kind, got_body) = cipher.open_into(&pkt, &mut scratch).unwrap();
                assert_eq!((got_seq, got_kind, got_body), (seq, 0x0206, &body[..]));
                assert_eq!(scratch.as_ptr(), ptr, "scratch must not reallocate");
            }
        }
        // LegacyInput caches GcmKey, rotates cleanly on key change, and keeps IV on forged packets.
        let mut legacy = LegacyInput::new(0x0102_0304);
        let mut scratch = Vec::with_capacity(64);
        for key in [key_a, key_a, key_b] {
            let iv_before = legacy.iv;
            let body = b"legacy input body 16+ bytes!!";
            let (tag, ct) = crypto::gcm_seal(&key, &iv_before, body).unwrap();
            let mut msg = ((16 + ct.len()) as u32).to_be_bytes().to_vec();
            msg.extend_from_slice(&tag);
            msg.extend_from_slice(&ct);
            let mut forged = msg.clone();
            forged[4] ^= 1;
            assert!(legacy.open_into(&key, &forged, &mut scratch).is_err());
            assert_eq!(legacy.iv, iv_before, "forged packet must not advance IV");
            assert_eq!(legacy.open_into(&key, &msg, &mut scratch).unwrap(), body);
            assert_eq!(legacy.iv, ct[ct.len() - 16..]);
        }
    }
}
