//! Streamed P-frame packetization: emitting FEC blocks as encoder slices arrive.

use anyhow::{Result, bail};
use butterpollo_crypto::crypto;
use std::sync::Arc;

use crate::pyrowave::PayloadCursor;

use super::{BlockFill, PacketBlock, VideoPacketizer, block_parity, conventional_fec};

/// A streamed frame's encoder output passed the cap its block plan relied on.
/// The frame cannot be finished in the block count already announced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SliceOverflow {
    pub cap: usize,
    pub bytes: usize,
}
impl std::fmt::Display for SliceOverflow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "streamed video frame reached {} bytes, past its {} byte cap",
            self.bytes, self.cap
        )
    }
}
impl std::error::Error for SliceOverflow {}
/// A FEC block of a streamed frame as it was built, for the sender's
/// accounting (`SendLoss::push_block`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamedBlock {
    pub index: usize,
    pub data: usize,
    pub parity: usize,
    /// The block count the frame's packets announce. A frame that turns out
    /// to be small is framed whole, so this can be fewer than planned.
    pub blocks: usize,
}
impl StreamedBlock {
    /// The frame's last block: nothing more follows it.
    pub fn is_last(&self) -> bool {
        self.index + 1 == self.blocks
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum StreamState {
    Open,
    Done,
    Abandoned,
}
impl VideoPacketizer {
    /// Use up the next frame index without sending a frame: a frame that was
    /// dropped before any packet left. The client sees the gap.
    pub fn skip_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }
    /// Whether a P-frame capped at `cap` payload bytes can be streamed in
    /// `blocks` FEC blocks: even all at 0% parity its shards must fit.
    pub fn streamable(&self, blocks: usize, cap: usize) -> bool {
        (256..=usize::from(rubylight_protocol::video_packet::MAX_VIDEO_PACKET))
            .contains(&self.packet_size)
            && self.fec_percent <= 100
            && (2..=4).contains(&blocks)
            && (cap + 8).div_ceil(self.packet_size + 16 - 32) <= blocks * 1023
    }
    /// Start a P-frame whose encoder output arrives in units (slices) and
    /// leaves in `blocks` FEC blocks as soon as each is complete. `slices`
    /// is how many units the encoder was asked for and `cap` the most payload
    /// bytes it can return. The frame takes its index when block 0 is built.
    pub fn streamed(
        &self,
        timestamp: u32,
        latency_us: u64,
        blocks: usize,
        slices: usize,
        cap: usize,
    ) -> Result<StreamedFrame> {
        self.streamed_in(Vec::new(), timestamp, latency_us, blocks, slices, cap)
    }
    /// [`streamed`](Self::streamed) collecting the payload in `buffer`, the
    /// one [`StreamedFrame::into_buffer`] gave back for the previous frame,
    /// so a stream allocates it once.
    pub fn streamed_in(
        &self,
        mut buffer: Vec<Vec<u8>>,
        timestamp: u32,
        latency_us: u64,
        blocks: usize,
        slices: usize,
        cap: usize,
    ) -> Result<StreamedFrame> {
        buffer.clear();
        if !self.streamable(blocks, cap) {
            bail!("video frame cannot be streamed in {blocks} blocks");
        }
        let slice = self.packet_size + 16 - 32;
        let mut header = [0u8; 8];
        header[0] = 1;
        header[1..3].copy_from_slice(&((latency_us / 100).min(65535) as u16).to_le_bytes());
        header[3] = 1;
        // The last payload length is not known yet. Receivers read it only
        // for codecs other than H.264 and HEVC; every shard is full.
        header[4..6].copy_from_slice(&(slice as u16).to_le_bytes());
        let (percentage, minimum) = (self.fec_percent, self.min_fec);
        // The most data shards a block holds at the frame's percentage.
        let max_data = if percentage == 0 {
            1023
        } else {
            (1..=255)
                .rev()
                .find(|&n| n + block_parity(n, percentage, minimum).0 <= 255)
                .unwrap_or(1)
        };
        Ok(StreamedFrame {
            sealer: self.sealer(),
            header,
            pieces: buffer,
            len: 0,
            piece_index: 0,
            piece_at: 0,
            timestamp,
            frame: self.frame,
            slice,
            percentage,
            minimum,
            max_data,
            blocks,
            slices: slices.max(1),
            cap,
            units: 0,
            next_block: 0,
            first_shard: 0,
            early: 0,
            fec_limited: false,
            state: StreamState::Open,
        })
    }
}
/// Data shards of each of `blocks` blocks holding `shards` in all, and the
/// largest percentage up to `percentage` at which every block fits
/// Moonlight's limits (down to 0% at 1023 shards).
fn tail_split(
    shards: usize,
    blocks: usize,
    percentage: usize,
    minimum: usize,
) -> Option<([usize; 4], usize)> {
    if blocks == 0 || blocks > 4 || shards < blocks || shards > blocks * 1023 {
        return None;
    }
    let mut data = [0; 4];
    for (i, data) in data.iter_mut().take(blocks).enumerate() {
        *data = shards / blocks + usize::from(i < shards % blocks);
    }
    let fits = |percentage| {
        percentage == 0
            || data[..blocks]
                .iter()
                .all(|&d| d + block_parity(d, percentage, minimum).0 <= 255)
    };
    let percentage = (0..=percentage).rev().find(|&p| fits(p))?;
    Some((data, percentage))
}
/// One P-frame sent block by block while the encoder is still producing it.
///
/// The frame's bytes are one stream (8-byte header, then the units in order)
/// cut into full shards, and an FEC block is a contiguous run of them. A
/// block is built once enough units have arrived, never ends on a short shard
/// (the unfilled tail waits for the next block) and the block count, which
/// every packet announces, is fixed by the plan before block 0 leaves. Only
/// the frame's last shard is short, as on a whole frame.
///
/// With `B` blocks and `S` slices, block `k < B - 1` leaves once unit
/// `ceil((k + 1) * S / B)` has arrived, minus a hold-back that keeps one
/// complete shard for every later block, and only while the cap still fits
/// the later blocks at 0% parity. The last unit splits what remains over the
/// remaining blocks like a whole frame. If the cap was too small, the frame
/// fails with [`SliceOverflow`], the caller's cue to ask for a recovery frame.
pub struct StreamedFrame {
    sealer: Option<Arc<crypto::PacketSealer>>,
    header: [u8; 8],
    /// The encoder's units, moved in as they arrive and read in order, so the
    /// packetizer copies each byte straight from its unit into a shard.
    pieces: Vec<Vec<u8>>,
    /// Payload bytes received, which `pieces` stop holding once the frame
    /// is over.
    len: usize,
    /// Saved monotonic cursor position across blocks of this frame.
    piece_index: usize,
    piece_at: usize,
    timestamp: u32,
    frame: u32,
    slice: usize,
    percentage: usize,
    minimum: usize,
    max_data: usize,
    blocks: usize,
    slices: usize,
    pub(super) cap: usize,
    units: usize,
    pub(super) next_block: usize,
    pub(super) first_shard: usize,
    early: usize,
    fec_limited: bool,
    state: StreamState,
}
impl StreamedFrame {
    /// Add the encoder's next unit and build every block that is ready, each
    /// passed to `emit` (to be sent before it returns) from the reused
    /// `shards`. After a failure the frame is abandoned: nothing more is
    /// built, and its index stays consumed, so the client sees the gap.
    pub fn push(
        &mut self,
        packetizer: &mut VideoPacketizer,
        unit: Vec<u8>,
        last: bool,
        shards: &mut PacketBlock,
        mut emit: impl FnMut(&mut [Vec<u8>], StreamedBlock) -> Result<()>,
    ) -> Result<()> {
        self.push_blocks(
            packetizer,
            unit,
            last,
            shards,
            |block_shards, filled, block| emit(&mut block_shards[..filled], block),
        )
    }
    /// [`Self::push`] handing the whole `PacketBlock` and its filled packet
    /// count to `emit`, so the caller can swap the whole block with a recycled
    /// one in O(1) without per-shard swapping.
    pub fn push_blocks(
        &mut self,
        packetizer: &mut VideoPacketizer,
        unit: Vec<u8>,
        last: bool,
        shards: &mut PacketBlock,
        mut emit: impl FnMut(&mut PacketBlock, usize, StreamedBlock) -> Result<()>,
    ) -> Result<()> {
        match self.state {
            StreamState::Open => {}
            StreamState::Done => bail!("streamed video frame is already finished"),
            StreamState::Abandoned => bail!("streamed video frame was abandoned"),
        }
        if packetizer.frame != self.frame.wrapping_add(u32::from(self.next_block > 0)) {
            self.state = StreamState::Abandoned;
            bail!("another video frame was packetized during a streamed frame");
        }
        let result = self.push_unit(packetizer, unit, last, shards, &mut emit);
        if result.is_err() {
            self.abandon(packetizer);
        }
        result
    }
    /// Give the frame up: nothing more is built. If no block was built, the
    /// frame index is consumed here, or the client would see no gap for the
    /// missing picture and decode the next one against a reference it never
    /// got. Safe to call on a finished or abandoned frame.
    pub fn abandon(&mut self, packetizer: &mut VideoPacketizer) {
        if self.state == StreamState::Open {
            self.state = StreamState::Abandoned;
        }
        if self.next_block == 0 && packetizer.frame == self.frame {
            packetizer.skip_frame();
        }
    }
    /// The unit buffers moved into this frame, returned intact so the caller
    /// can hand each unit buffer back to a pool (`frame_pool::give`).
    pub fn take_pieces(mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.pieces)
    }
    /// The unit buffers, emptied, for the next frame ([`VideoPacketizer::streamed_in`]).
    pub fn into_buffer(mut self) -> Vec<Vec<u8>> {
        self.pieces.clear();
        self.pieces
    }
    fn push_unit(
        &mut self,
        packetizer: &mut VideoPacketizer,
        unit: Vec<u8>,
        last: bool,
        shards: &mut PacketBlock,
        emit: &mut impl FnMut(&mut PacketBlock, usize, StreamedBlock) -> Result<()>,
    ) -> Result<()> {
        self.len += unit.len();
        self.pieces.push(unit);
        self.units += 1;
        if self.len > self.cap {
            return Err(self.overflow());
        }
        let stream_len = 8 + self.len;
        if !last {
            return self.send_ready(packetizer, stream_len, shards, emit);
        }
        if self.len == 0 {
            bail!("encoded frame is empty");
        }
        if self.next_block == 0 {
            // Nothing has left yet, so the block count is still free: this is
            // a whole frame, framed as one.
            let total_shards = stream_len.div_ceil(self.slice);
            let (blocks, percentage) =
                conventional_fec(total_shards, self.percentage, self.minimum);
            let last_shard = stream_len % self.slice;
            let length = if last_shard == 0 {
                self.slice
            } else {
                last_shard
            };
            self.header[4..6].copy_from_slice(&(length as u16).to_le_bytes());
            self.fec_limited = percentage < self.percentage;
            let aligned = total_shards.div_ceil(blocks);
            for block in 0..blocks {
                let data = total_shards.saturating_sub(block * aligned).min(aligned);
                if data == 0 {
                    bail!("invalid FEC block split");
                }
                self.emit_block(packetizer, data, percentage, blocks, shards, emit)?;
            }
        } else {
            let remaining = self.blocks - self.next_block;
            let rest = (stream_len - self.first_shard * self.slice).div_ceil(self.slice);
            let Some((data, percentage)) =
                tail_split(rest, remaining, self.percentage, self.minimum)
            else {
                // The cap rule should have kept room: the plan is broken.
                return Err(self.overflow());
            };
            self.fec_limited = percentage < self.percentage;
            for &data in &data[..remaining] {
                self.emit_block(packetizer, data, percentage, self.blocks, shards, emit)?;
            }
        }
        self.state = StreamState::Done;
        Ok(())
    }
    fn overflow(&self) -> anyhow::Error {
        SliceOverflow {
            cap: self.cap,
            bytes: self.len,
        }
        .into()
    }
    /// Build the blocks before the last whose unit has arrived and whose
    /// complete shards, hold-back and cap rule allow it.
    fn send_ready(
        &mut self,
        packetizer: &mut VideoPacketizer,
        stream_len: usize,
        shards: &mut PacketBlock,
        emit: &mut impl FnMut(&mut PacketBlock, usize, StreamedBlock) -> Result<()>,
    ) -> Result<()> {
        while self.next_block < self.blocks - 1
            && self.units >= ((self.next_block + 1) * self.slices).div_ceil(self.blocks)
        {
            let later = self.blocks - self.next_block - 1;
            let complete = stream_len / self.slice - self.first_shard;
            // Keep a complete shard for each later block.
            let data = complete.saturating_sub(later).min(self.max_data);
            if data == 0 {
                break;
            }
            // Whatever the encoder still adds, up to the cap, must fit the
            // later blocks without parity.
            let after = (self.first_shard + data) * self.slice;
            if (self.cap + 8).saturating_sub(after).div_ceil(self.slice) > later * 1023 {
                break;
            }
            self.emit_block(packetizer, data, self.percentage, self.blocks, shards, emit)?;
            self.early += 1;
        }
        Ok(())
    }
    fn emit_block(
        &mut self,
        packetizer: &mut VideoPacketizer,
        data: usize,
        percentage: usize,
        blocks: usize,
        shards: &mut PacketBlock,
        emit: &mut impl FnMut(&mut PacketBlock, usize, StreamedBlock) -> Result<()>,
    ) -> Result<()> {
        let mut cursor = PayloadCursor::from_pieces_position(
            &self.pieces,
            self.len,
            self.piece_index,
            self.piece_at,
        );
        let filled = packetizer.fill_block(
            &BlockFill {
                block: self.next_block,
                blocks,
                first_shard: self.first_shard,
                count: data,
                percentage,
                minimum: self.minimum,
                header: &self.header,
                layout: None,
                sealer: self.sealer.as_deref(),
                timestamp: self.timestamp,
                frame: self.frame,
            },
            &mut cursor,
            shards,
        )?;
        (self.piece_index, self.piece_at) = cursor.position();
        let block = StreamedBlock {
            index: self.next_block,
            data,
            parity: filled - data,
            blocks,
        };
        self.first_shard += data;
        self.next_block += 1;
        emit(shards, filled, block)
    }
    /// The frame was fully built.
    pub fn is_done(&self) -> bool {
        self.state == StreamState::Done
    }
    /// Blocks built so far.
    pub fn blocks_built(&self) -> usize {
        self.next_block
    }
    /// Blocks built before the last unit arrived.
    pub fn early_blocks(&self) -> usize {
        self.early
    }
    /// Units received so far.
    pub fn units(&self) -> usize {
        self.units
    }
    /// Payload bytes received so far.
    pub fn payload_len(&self) -> usize {
        self.len
    }
    /// Whether the last blocks carry less parity than the frame's percentage.
    pub fn fec_limited(&self) -> bool {
        self.fec_limited
    }
}
