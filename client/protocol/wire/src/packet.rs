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
        let mut b = self.gcm.open(&iv[..iv_len], tag, sealed)?;
        if b.len() < 4 {
            bail!("short decrypted control header");
        }
        let kind = u16::from_le_bytes(b[..2].try_into().unwrap());
        if u16::from_le_bytes(b[2..4].try_into().unwrap()) as usize != b.len() - 4 {
            bail!("decrypted control length mismatch");
        }
        b.drain(..4);
        Ok((seq, kind, b))
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
}

impl LegacyInput {
    pub fn new(key_id: u32) -> Self {
        let mut iv = [0; 16];
        iv[..4].copy_from_slice(&key_id.to_be_bytes());
        Self { iv }
    }

    pub fn open(&mut self, key: &[u8; 16], payload: &[u8]) -> Result<Vec<u8>> {
        if payload.len() < 20 {
            bail!("short legacy input message");
        }
        let len = u32::from_be_bytes(payload[..4].try_into().unwrap()) as usize;
        if len < 16 || len + 4 != payload.len() {
            bail!("legacy input length mismatch");
        }
        let tagged = &payload[4..];
        let plain = crypto::gcm_open(key, &self.iv, &tagged[..16], &tagged[16..])?;
        if len >= 32 {
            self.iv.copy_from_slice(&tagged[len - 16..]);
        }
        Ok(plain)
    }
}

#[cfg(test)]
mod control_cipher_tests;
