//! Moonlight audio RTP packetization with cached AES-128-CBC and 4+2 Reed-Solomon FEC.
use anyhow::Result;
use butterpollo_crypto::crypto;
use rubylight_wire::crypto::CbcKey;

pub struct AudioPacketizer {
    pub sequence: u16,
    pub timestamp: u32,
    pub key: [u8; 16],
    pub key_id: u32,
    pub encrypted: bool,
    pub packet_ms: u32,
    /// Pre-expanded AES-128 key schedule, refreshed only if `self.key` changes.
    cbc: Option<([u8; 16], CbcKey)>,
    pending: Vec<Vec<u8>>,
    /// The RTP timestamp of the FEC block's first packet.
    block_timestamp: u32,
    /// The buffers earlier packets came in, reused for the next: a stream sends
    /// one packet after another, and none of them allocates its own buffer.
    spare: Vec<Vec<u8>>,
}

impl AudioPacketizer {
    pub fn new(key: [u8; 16], key_id: u32, encrypted: bool, packet_ms: u32) -> Self {
        Self {
            sequence: 0,
            timestamp: 0,
            key,
            key_id,
            encrypted,
            packet_ms,
            cbc: encrypted.then(|| (key, CbcKey::new(&key))),
            pending: vec![],
            block_timestamp: 0,
            spare: vec![],
        }
    }

    fn cbc_key(&mut self) -> &CbcKey {
        let key = self.key;
        if self
            .cbc
            .as_ref()
            .is_some_and(|(cached, _)| !crypto::equal(cached, &key))
        {
            self.cbc = None;
        }
        &self.cbc.get_or_insert_with(|| (key, CbcKey::new(&key))).1
    }

    /// One Opus or PCM packet, `packet_ms` after the last.
    pub fn encode(&mut self, opus: &[u8]) -> Result<Vec<Vec<u8>>> {
        self.encode_part(opus, true)
    }

    /// One piece of a frame. Every piece of a frame carries its timestamp, which moves on by
    /// `packet_ms` after the `last` piece (AC3 and E-AC3 frames take several packets).
    pub fn encode_part(&mut self, opus: &[u8], last: bool) -> Result<Vec<Vec<u8>>> {
        let mut output = Vec::new();
        self.encode_part_into(opus, last, &mut output)?;
        Ok(output)
    }

    /// [`Self::encode`] into `output`, whose buffers this packetizer reuses: pass the same
    /// `output` every call and each packet comes in the previous one's room. The caller
    /// sends between calls; this first takes the buffers of the packets already sent.
    pub fn encode_into(&mut self, opus: &[u8], output: &mut Vec<Vec<u8>>) -> Result<()> {
        self.encode_part_into(opus, true, output)
    }

    /// [`Self::encode_part`] into `output`, reused the same way as [`Self::encode_into`].
    pub fn encode_part_into(
        &mut self,
        opus: &[u8],
        last: bool,
        output: &mut Vec<Vec<u8>>,
    ) -> Result<()> {
        // The previous call's packets, handed back, become this one's buffers.
        self.spare.append(output);
        if self.pending.is_empty() {
            self.block_timestamp = self.timestamp;
        }
        let seq = self.sequence;
        let mut iv = [0; 16];
        iv[..4].copy_from_slice(&self.key_id.wrapping_add(u32::from(seq)).to_be_bytes());
        let mut p = self.room(12);
        p[0] = 0x80;
        p[1] = 97;
        p[2..4].copy_from_slice(&seq.to_be_bytes());
        p[4..8].copy_from_slice(&self.timestamp.to_be_bytes());
        if self.pending.len() == 3 {
            // 4th packet of the FEC block: write encrypted/plain bytes straight
            // into `p[12..]` and read them directly for the two FEC rows instead
            // of copying into a spare buffer that would drain right afterwards.
            if self.encrypted {
                self.cbc_key().seal_append(&iv, opus, &mut p);
            } else {
                p.extend_from_slice(opus);
            }
            self.sequence = self.sequence.wrapping_add(1);
            if last {
                self.timestamp = self.timestamp.wrapping_add(self.packet_ms);
            }
            let fourth = &p[12..];
            let len = self
                .pending
                .iter()
                .map(Vec::len)
                .fold(fourth.len(), usize::max);
            let matrix = [[0x77, 0x40, 0x38, 0x0e], [0xc7, 0xa7, 0x0d, 0x6c]];
            let mut fec = [Vec::new(), Vec::new()];
            for (index, (row, slot)) in matrix.iter().zip(fec.iter_mut()).enumerate() {
                let mut f = self.room(24 + len);
                f[0] = 0x80;
                f[1] = 127;
                f[2..4].copy_from_slice(&self.sequence.wrapping_add(index as u16).to_be_bytes());
                f[4..8].copy_from_slice(&self.timestamp.to_be_bytes());
                f[12] = index as u8;
                f[13] = 97;
                f[14..16].copy_from_slice(&seq.wrapping_sub(3).to_be_bytes());
                f[16..20].copy_from_slice(&self.block_timestamp.to_be_bytes());
                for (j, b) in self.pending.iter().enumerate() {
                    super::axpy(&mut f[24..24 + b.len()], b, row[j]);
                }
                super::axpy(&mut f[24..24 + fourth.len()], fourth, row[3]);
                *slot = f;
            }
            output.push(p);
            output.extend(fec);
            // The block's payloads become the next packets' buffers too.
            self.spare.append(&mut self.pending);
        } else {
            let mut data = self.spare.pop().unwrap_or_default();
            if self.encrypted {
                self.cbc_key().seal_into(&iv, opus, &mut data);
            } else {
                data.clear();
                data.extend_from_slice(opus);
            }
            p.extend_from_slice(&data);
            output.push(p);
            self.pending.push(data);
            self.sequence = self.sequence.wrapping_add(1);
            if last {
                self.timestamp = self.timestamp.wrapping_add(self.packet_ms);
            }
        }
        Ok(())
    }

    /// A packet buffer off the free list, zeroed to `len`.
    fn room(&mut self, len: usize) -> Vec<u8> {
        let mut room = self.spare.pop().unwrap_or_default();
        room.clear();
        room.resize(len, 0);
        room
    }
}
