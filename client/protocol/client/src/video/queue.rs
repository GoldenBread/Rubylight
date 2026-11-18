//! The RTP video queue: collects each FEC block of a frame, recovers missing data shards from
//! parity, and passes completed frames to the depacketizer in sequence order. Port of
//! `RtpVideoQueue.c` and Nightjar's `android_rtp_video.c` (PyroWave records keep the data
//! of incomplete blocks).

use super::depacketizer::Depacketizer;
use super::{
    is_before16, is_before32, le32, rtp_data_offset, FecBlock, FecStatus, Io, Packet, VideoConfig,
    VideoSignal, VideoSink, FLAG_CONTAINS_PIC_DATA, FLAG_EOF, FLAG_SOF, MAX_RTP_HEADER_SIZE,
    NV_VIDEO_PACKET_SIZE,
};
use crate::fec::ReedSolomon;

/// Control message type of [`FecStatus`] reports.
pub const FEC_STATUS_TYPE: u16 = 0x5502;

/// No speculative RFI for 5 minutes after out-of-order data or a wrong loss prediction.
const SPECULATIVE_RFI_COOLDOWN_PERIOD_US: u64 = 300_000_000;

/// Frames a host may skip while streaming before the frame number is a jump: 240 frames is 4 s at
/// 60 fps, longer than any loss or reordering the queue rides out.
const MAX_FRAME_JUMP: u32 = 240;
/// Packets a host may skip while streaming before the sequence number is a jump: more than the
/// 3,632 packets of the largest FEC block.
const MAX_SEQ_JUMP: u32 = 4096;
/// Packets the largest frame can carry: four blocks of at most 3,632 packets.
const MAX_FRAME_PACKETS: u32 = 4 * 3632;
/// Datagrams that must agree on a far-behind position before the queue follows it: a straggler
/// is one datagram, a replay repeats itself, a host whose counters restarted keeps sending from
/// there.
const REWIND_CONFIRM: u8 = 3;

/// How far `x` is ahead of `y` in the 16-bit wrapping order that hosts count frame and RTP
/// sequence numbers in: 0 when `x` is not ahead.
fn ahead16(x: u32, y: u32) -> u32 {
    let ahead = x.wrapping_sub(y) & 0xFFFF;
    if ahead < 0x8000 {
        ahead
    } else {
        0
    }
}

/// A position far behind the queue's that its datagrams kept coming from: a host whose frame
/// counters restarted (a new stream in place), or an old position played again. Followed only
/// once datagrams agree on it (`REWIND_CONFIRM`); until then the queue stays where it is and the
/// datagrams are dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rewind {
    frame: u32,
    seq: u16,
    /// Datagrams seen agreeing with it, this one included.
    count: u8,
}

pub(super) struct RtpQueue {
    pending: Vec<Packet>,
    completed: Vec<Packet>,

    buffer_first_recv_time_us: u64,
    buffer_lowest_seq: u16,
    buffer_highest_seq: u16,
    buffer_first_parity_seq: u16,
    buffer_data_packets: u32,
    buffer_parity_packets: u32,
    received_data_packets: u32,
    received_parity_packets: u32,
    received_highest_seq: u16,
    fec_percentage: u32,
    next_contiguous_seq: u16,
    missing_packets: u32,
    use_fast_queue_path: bool,
    /// One bit per `data + parity` offset of the block, so that a duplicate costs no scan:
    /// 57 words cover the 3,632 packets of the largest block.
    seen: [u64; 57],
    reported_lost_frame: bool,

    current_frame: u32,
    /// A block started: from here on the frame and sequence numbers have a position to jump from.
    started: bool,
    /// A far-behind position its datagrams kept coming from, while it is not followed yet.
    rewind: Option<Rewind>,

    multi_fec_capable: bool,
    current_block: u8,
    last_block: u8,

    last_oos_frame_pts: u64,
    received_oos_data: bool,

    packet_size: usize,
    pub(super) rfi: bool,
    fec_supported: bool,
    records: bool,
    /// Records go to the depacketizer as their packets arrive instead of when the frame
    /// completes ([`VideoConfig::early_chunks`]).
    early: bool,
    /// Data packets of the pending block already given to the depacketizer: the run from the
    /// block's first packet with no hole. They stay pending for FEC recovery.
    forwarded: u32,
    /// Blocks of a conventional frame go to the depacketizer as they complete instead of all
    /// at its last block ([`Self::queue_completed_ahead`]).
    process_ahead: bool,

    /// FEC recovery's code and scratch, kept so that recovering a block allocates nothing.
    fec: Option<ReedSolomon>,
    fec_missing: Vec<bool>,
    fec_shards: Vec<Box<[u8]>>,
}

impl RtpQueue {
    pub(super) fn new(config: &VideoConfig) -> Self {
        Self {
            pending: Vec::new(),
            completed: Vec::new(),
            buffer_first_recv_time_us: 0,
            buffer_lowest_seq: 0,
            buffer_highest_seq: 0,
            buffer_first_parity_seq: 0,
            buffer_data_packets: 0,
            buffer_parity_packets: 0,
            received_data_packets: 0,
            received_parity_packets: 0,
            received_highest_seq: 0,
            fec_percentage: 0,
            next_contiguous_seq: 0,
            missing_packets: 0,
            use_fast_queue_path: false,
            seen: [0; 57],
            reported_lost_frame: false,
            current_frame: 1,
            started: false,
            rewind: None,
            multi_fec_capable: config.at_least(7, 1, 431),
            current_block: 0,
            last_block: 0,
            last_oos_frame_pts: 0,
            received_oos_data: false,
            packet_size: config.packet_size,
            rfi: config.rfi,
            fec_supported: config.app_version[0] >= 5,
            records: config.pyrowave_records,
            early: config.pyrowave_records && config.early_chunks,
            forwarded: 0,
            process_ahead: !config.pyrowave_records,
            fec: None,
            fec_missing: Vec::new(),
            fec_shards: Vec::new(),
        }
    }

    pub(super) fn current_frame_number(&self) -> u32 {
        self.current_frame
    }

    /// Whether `frame` is more than `MAX_FRAME_JUMP` frames behind the queue's position: the
    /// counters went back, they did not slip. A straggler from the last seconds of a stream, or
    /// a packet replayed from them, is within a handful of frames; a host whose counters
    /// restarted starts a position far behind. The encrypted path checks it before decrypting,
    /// so an old frame it lets through is decided on after decryption.
    pub(super) fn is_rewind(&self, frame: u32) -> bool {
        self.started && !self.records && ahead16(self.current_frame, frame) > MAX_FRAME_JUMP
    }

    /// Whether this datagram continues the far-behind position being seen: it advances it by at
    /// most a frame and a frame's packets. A datagram replayed, repeated or gone back does not.
    fn continues_rewind(&self, frame: u32, seq: u16) -> bool {
        let Some(rewind) = self.rewind else { return false };
        let frames = ahead16(frame, rewind.frame);
        let packets = ahead16(seq.into(), rewind.seq.into());
        (frames > 0 || packets > 0) && frames <= 1 && packets <= MAX_SEQ_JUMP
    }

    /// Notes a datagram at a far-behind position and returns whether enough datagrams have agreed
    /// on it to follow it. A single straggler or a replay never lasts long enough.
    fn confirm_rewind(&mut self, frame: u32, seq: u16) -> bool {
        let count = if self.continues_rewind(frame, seq) {
            self.rewind.map_or(1, |rewind| rewind.count.saturating_add(1))
        } else {
            1
        };
        self.rewind = Some(Rewind { frame, seq, count });
        if count < REWIND_CONFIRM {
            return false;
        }
        self.rewind = None;
        true
    }

    /// Whether a forward jump is a counter no streaming host produces rather than loss: the frame
    /// and sequence numbers disagree about how far the stream went. A burst of loss advances both
    /// together, since each lost frame carries one to four blocks of at most 3,632 packets, so
    /// only a jump past both bounds is looked at. False for a frame or sequence number that went
    /// backwards (a straggler) and for PyroWave records, which keep their own recovery.
    fn forward_jump_disagrees(&self, frame: u32, seq: u16) -> bool {
        if !self.started || self.records || is_before16(frame, self.current_frame) {
            return false;
        }
        let frames = ahead16(frame, self.current_frame);
        let packets = ahead16(seq.into(), self.next_contiguous_seq.into());
        if frames <= MAX_FRAME_JUMP && packets <= MAX_SEQ_JUMP {
            return false;
        }
        packets > (frames + 1) * MAX_FRAME_PACKETS || frames > packets + 1
    }

    /// Starts the block this datagram begins: the queue's position moves to `frame`, and the
    /// block's counters come from its header.
    fn start_block<S: VideoSink>(
        &mut self,
        frame: u32,
        block: u8,
        blocks: u8,
        fec_info: u32,
        seq: u16,
        io: &mut Io<S>,
    ) {
        self.current_frame = frame;
        self.started = true;
        io.sink.signal(VideoSignal::FrameSeen { frame });

        self.buffer_first_recv_time_us = io.now_us;
        self.buffer_lowest_seq = seq.wrapping_sub(((fec_info & 0x3F_F000) >> 12) as u16);
        self.next_contiguous_seq = self.buffer_lowest_seq;
        self.received_data_packets = 0;
        self.received_parity_packets = 0;
        self.forwarded = 0;
        self.received_highest_seq = 0;
        self.missing_packets = 0;
        self.use_fast_queue_path = true;
        self.seen = [0; 57];
        self.reported_lost_frame = false;
        self.buffer_data_packets = (fec_info & 0xFFC0_0000) >> 22;
        self.fec_percentage = (fec_info & 0xFF0) >> 4;
        self.buffer_parity_packets = (self.buffer_data_packets * self.fec_percentage).div_ceil(100);
        self.buffer_first_parity_seq = self
            .buffer_lowest_seq
            .wrapping_add(self.buffer_data_packets as u16);
        self.buffer_highest_seq = self
            .buffer_first_parity_seq
            .wrapping_add(self.buffer_parity_packets as u16)
            .wrapping_sub(1);
        self.current_block = block;
        self.last_block = (blocks >> 6) & 0x3;

        io.stats.packet_count_video = io
            .stats
            .packet_count_video
            .wrapping_add(self.buffer_data_packets);
        io.stats.packet_count_fec = io
            .stats
            .packet_count_fec
            .wrapping_add(self.buffer_parity_packets);
    }

    /// The previous behaviour, every block of a frame processed at its last, for comparison.
    #[cfg(test)]
    pub(super) fn set_process_ahead(&mut self, on: bool) {
        self.process_ahead = on && !self.records;
    }

    /// Tests: the position the queue has after the frames before `frame` were received, so a
    /// counter's wrap can be checked without the 65,536 frames that lead to it.
    #[cfg(test)]
    pub(super) fn seek(&mut self, frame: u32) {
        self.current_frame = frame;
        self.started = true;
    }

    fn purge_pending<S: VideoSink>(&mut self, io: &mut Io<S>) {
        for packet in self.pending.drain(..) {
            io.pools.recycle_packet(packet.buf);
        }
    }

    fn purge_completed<S: VideoSink>(
        &mut self,
        depacketizer: &mut Depacketizer,
        io: &mut Io<S>,
    ) {
        depacketizer.rewind();
        for packet in self.completed.drain(..) {
            io.pools.recycle_packet(packet.buf);
        }
    }

    /// `notifyOrdinaryFrameLost`: with PyroWave records a lost frame still decodes partially.
    fn notify_frame_lost<S: VideoSink>(
        &self,
        depacketizer: &mut Depacketizer,
        io: &mut Io<S>,
        frame: u32,
        speculative: bool,
    ) {
        if !self.records {
            depacketizer.notify_frame_lost(frame, speculative, io);
        }
    }

    fn report_fec_status<S: VideoSink>(&self, io: &mut Io<S>) {
        io.sink.signal(VideoSignal::FecStatus(FecStatus {
            frame_index: self.current_frame,
            highest_received_sequence: self.received_highest_seq,
            next_contiguous_sequence: self.next_contiguous_seq,
            missing_before_highest: self.missing_packets as u16,
            total_data_packets: self.buffer_data_packets as u16,
            total_parity_packets: self.buffer_parity_packets as u16,
            received_data_packets: self.received_data_packets as u16,
            received_parity_packets: self.received_parity_packets as u16,
            fec_percentage: self.fec_percentage as u8,
            block_index: self.current_block,
            block_count: self.last_block + 1,
        }));
    }

    /// Hands the pending block's counters to the FEC report as it finishes: `unrecovered` when
    /// the next block begins before this one completed (`FecTally::block` counts them).
    fn finish_fec_block<S: VideoSink>(&self, io: &mut Io<S>, unrecovered: bool) {
        io.sink.signal(VideoSignal::FecBlock(FecBlock {
            frame: self.current_frame,
            block_index: self.current_block,
            block_count: self.last_block + 1,
            data_shards: self.buffer_data_packets,
            parity_shards: self.buffer_parity_packets,
            arrived_shards: self.received_data_packets + self.received_parity_packets,
            missing_shards: self.missing_packets,
            unrecovered,
        }));
    }

    /// Adds a packet to the pending block; hands it back if it duplicates a queued one.
    fn queue_packet(
        &mut self,
        mut packet: Packet,
        is_parity: bool,
        is_fec_recovery: bool,
    ) -> Result<(), Packet> {
        // The block's offsets are a contiguous run, so a bitmap covers them all.
        let offset = usize::from(packet.seq.wrapping_sub(self.buffer_lowest_seq));
        let word = offset / 64;
        let bit = 1u64 << (offset % 64);
        if self.seen.get(word).is_some_and(|w| w & bit != 0) {
            return Err(packet);
        }
        if let Some(w) = self.seen.get_mut(word) {
            *w |= bit;
        }
        let mut out_of_sequence = false;
        if self.use_fast_queue_path && packet.seq == self.next_contiguous_seq {
            self.next_contiguous_seq = packet.seq.wrapping_add(1);
        } else {
            out_of_sequence = !self.pending.is_empty()
                && is_before16(packet.seq.into(), self.received_highest_seq.into());
            self.use_fast_queue_path = false;
        }

        packet.is_parity = is_parity;
        packet.presentation_time_us = u64::from(packet.timestamp) * 1000 / 90;
        packet.rtp_timestamp = packet.timestamp;

        // Recovered packets are ours, so they say nothing about reordering on the network.
        if !is_fec_recovery {
            if out_of_sequence {
                self.last_oos_frame_pts = packet.presentation_time_us;
                self.received_oos_data = true;
            } else if self.received_oos_data
                && packet.presentation_time_us
                    > self.last_oos_frame_pts + SPECULATIVE_RFI_COOLDOWN_PERIOD_US
            {
                self.received_oos_data = false;
            }
        }

        self.pending.push(packet);
        Ok(())
    }

    /// Returns 0 once the block is complete, recovering missing data shards when enough
    /// packets arrived.
    fn reconstruct_frame<S: VideoSink>(
        &mut self,
        depacketizer: &mut Depacketizer,
        io: &mut Io<S>,
    ) -> i32 {
        let total = self.buffer_data_packets + self.buffer_parity_packets;
        let needed = self.buffer_data_packets;

        if (self.pending.len() as u32) < needed {
            // Without recent reordering from this host, a block missing more packets than its
            // parity can only be lost, so report it before the next frame arrives. A block
            // without parity can't tell a late packet from a lost one: one packet overtaken on
            // Wi-Fi would cost the frame and a keyframe, so its loss waits for the next block.
            if self.rfi
                && !self.reported_lost_frame
                && !self.received_oos_data
                && self.buffer_parity_packets > 0
                && self.missing_packets > total.wrapping_sub(needed)
            {
                self.notify_frame_lost(depacketizer, io, self.current_frame, true);
                self.reported_lost_frame = true;
            }
            return -1;
        }

        if self.reported_lost_frame && !self.received_oos_data {
            // The prediction was wrong; stop predicting for a while.
            self.received_oos_data = true;
            self.last_oos_frame_pts = self.pending[0].presentation_time_us;
        }

        if self.received_data_packets == self.buffer_data_packets {
            return 0;
        }

        if !self.fec_supported {
            return -1;
        }

        let (data, parity) = (
            self.buffer_data_packets as usize,
            self.buffer_parity_packets as usize,
        );
        let renewed = match &mut self.fec {
            Some(rs) => rs.renew(data, parity),
            fec @ None => {
                *fec = ReedSolomon::new(data, parity);
                fec.is_some()
            }
        };
        let Some(rs) = self.fec.as_ref().filter(|_| renewed) else {
            return -3;
        };

        let total = total as usize;
        let receive_size = self.packet_size + MAX_RTP_HEADER_SIZE;
        self.fec_missing.clear();
        self.fec_missing.resize(total, true);
        // Empty boxes hold no memory.
        self.fec_shards.clear();
        self.fec_shards.resize_with(total, Box::default);
        for packet in &mut self.pending {
            let index = packet.seq.wrapping_sub(self.buffer_lowest_seq) as usize;
            self.fec_missing[index] = false;
            packet.buf[packet.len..receive_size].fill(0);
            self.fec_shards[index] = std::mem::take(&mut packet.buf);
        }
        for (shard, &missing) in self.fec_shards.iter_mut().zip(&self.fec_missing) {
            if missing {
                *shard = io.pools.packet();
            }
        }

        let decoded = rs.decode(&mut self.fec_shards, &self.fec_missing, receive_size);
        let mut ret = if decoded.is_ok() { 0 } else { -1 };

        // Report the final FEC status of a block that needed recovery.
        self.report_fec_status(io);

        for packet in &mut self.pending {
            let index = packet.seq.wrapping_sub(self.buffer_lowest_seq) as usize;
            packet.buf = std::mem::take(&mut self.fec_shards[index]);
        }

        let head_header = self.pending[0].buf[0];
        let head_timestamp = self.pending[0].timestamp;
        let data_packets = self.buffer_data_packets as usize;
        for index in 0..total {
            if !self.fec_missing[index] {
                continue;
            }
            let buf = std::mem::take(&mut self.fec_shards[index]);
            if ret != 0 || index >= data_packets {
                io.pools.recycle_packet(buf);
                continue;
            }
            let mut recovered = Packet {
                buf,
                len: 0,
                seq: (index as u16).wrapping_add(self.buffer_lowest_seq),
                timestamp: head_timestamp,
                receive_time_us: 0,
                presentation_time_us: 0,
                rtp_timestamp: 0,
                is_parity: false,
            };
            recovered.buf[0] = head_header;
            let offset = recovered.data_offset();
            recovered.buf[offset + 4..offset + 8]
                .copy_from_slice(&self.current_frame.to_le_bytes());
            recovered.buf[offset + 11] = ((self.last_block << 2) | self.current_block) << 4;
            let flags = recovered.buf[offset + 8];
            // Reject recovered packets that cannot be part of this block.
            let corrupt = (index == 0 && flags & FLAG_SOF == 0)
                || (index == data_packets - 1 && flags & FLAG_EOF == 0)
                || (index > 0 && index < data_packets - 1 && flags & FLAG_CONTAINS_PIC_DATA == 0)
                || flags & !(FLAG_SOF | FLAG_EOF | FLAG_CONTAINS_PIC_DATA) != 0;
            if corrupt {
                ret = -1;
                io.pools.recycle_packet(recovered.buf);
                continue;
            }
            recovered.len = self.packet_size + offset;
            io.stats.packets_recovered += 1;
            if let Err(packet) = self.queue_packet(recovered, false, true) {
                io.pools.recycle_packet(packet.buf);
            }
        }
        ret
    }

    /// Gives the depacketizer the pending block's data packets that arrived in order and are
    /// not in a hole, so it can pass them on while the rest of the block is still in flight.
    /// `arrived` is the data packet just queued: only it can extend the run, so a packet behind
    /// a hole costs nothing, and the pending list is searched only while packets that arrived
    /// out of order are waiting for the hole to fill.
    fn forward_ready<S: VideoSink>(
        &mut self,
        arrived: u16,
        depacketizer: &mut Depacketizer,
        io: &mut Io<S>,
    ) {
        if arrived != self.buffer_lowest_seq.wrapping_add(self.forwarded as u16) {
            return;
        }
        // In-order arrival is the last one queued.
        let mut index = self.pending.len() - 1;
        loop {
            let packet = &mut self.pending[index];
            packet.receive_time_us = self.buffer_first_recv_time_us;
            depacketizer.queue_early_packet(packet, io);
            self.forwarded += 1;
            let waiting = self.pending.len() - self.received_parity_packets as usize;
            if waiting <= self.forwarded as usize {
                return;
            }
            let seq = self.buffer_lowest_seq.wrapping_add(self.forwarded as u16);
            let Some(next) = self.pending.iter().rposition(|p| !p.is_parity && p.seq == seq) else {
                return;
            };
            index = next;
        }
    }

    /// Moves the block's data packets to the completed list in sequence order and drops the
    /// parity packets. With early records the packets not yet forwarded go straight on to the
    /// depacketizer, so the envelope stays in packet order across blocks.
    fn stage_complete_fec_block<S: VideoSink>(
        &mut self,
        depacketizer: &mut Depacketizer,
        io: &mut Io<S>,
    ) {
        let lowest = self.buffer_lowest_seq;
        let start = self.completed.len();
        for mut packet in self.pending.drain(..) {
            if packet.is_parity {
                io.pools.recycle_packet(packet.buf);
                continue;
            }
            // One receive time per block: the depacketizer only needs the first.
            packet.receive_time_us = self.buffer_first_recv_time_us;
            self.completed.push(packet);
        }
        self.completed[start..].sort_unstable_by_key(|p| p.seq.wrapping_sub(lowest));
        if self.early {
            let forwarded = std::mem::take(&mut self.forwarded) as usize;
            for (index, packet) in self.completed.drain(start..).enumerate() {
                if index < forwarded {
                    io.pools.recycle_packet(packet.buf);
                } else {
                    depacketizer.queue_rtp_packet(packet, io);
                }
            }
        }
    }

    fn submit_completed_frame<S: VideoSink>(
        &mut self,
        depacketizer: &mut Depacketizer,
        io: &mut Io<S>,
    ) {
        let ahead = depacketizer.take_ahead();
        // Room for the rest of the frame up front (records add 8 bytes per packet).
        depacketizer.reserve((self.completed.len() - ahead) * (self.packet_size + 8) + 4);
        for (index, packet) in self.completed.drain(..).enumerate() {
            if index < ahead {
                io.pools.recycle_packet(packet.buf);
            } else {
                depacketizer.queue_rtp_packet(packet, io);
            }
        }
    }

    /// Processes the completed blocks of a frame whose last block is still to come, so that
    /// the last block's packets are all that remain when it completes. They stay in the
    /// completed list until then: a lost frame or a recovery change rewinds the depacketizer
    /// ([`Depacketizer::rewind`]) and the frame is processed again or dropped as before.
    fn queue_completed_ahead<S: VideoSink>(
        &mut self,
        depacketizer: &mut Depacketizer,
        io: &mut Io<S>,
    ) {
        if !self.process_ahead {
            return;
        }
        let start = depacketizer.ahead();
        if start == 0 {
            // Room for the whole frame, guessed from the blocks so far.
            let blocks = usize::from(self.last_block) + 1;
            let done = usize::from(self.current_block);
            depacketizer.reserve(self.completed.len() * blocks / done * self.packet_size + 4);
        }
        for packet in &self.completed[start..] {
            if !depacketizer.queue_ahead(packet, io) {
                break;
            }
        }
    }

    /// `RtpvAddPacket` with Nightjar's PyroWave records wrapper. Hands the packet back when it
    /// is rejected.
    pub(super) fn add_packet<S: VideoSink>(
        &mut self,
        packet: Packet,
        depacketizer: &mut Depacketizer,
        io: &mut Io<S>,
    ) -> Result<(), Packet> {
        if !self.records {
            return self.add_ordinary(packet, depacketizer, io);
        }
        let offset = packet.data_offset();
        if packet.len < offset + NV_VIDEO_PACKET_SIZE {
            return Err(packet);
        }
        let frame = le32(&packet.buf, offset + 4);
        let block = (packet.buf[offset + 11] >> 4) & 3;
        if is_before32(frame, self.current_frame)
            || (frame == self.current_frame && block < self.current_block)
        {
            return Err(packet);
        }
        if frame != self.current_frame || block != self.current_block {
            if !self.pending.is_empty() {
                self.report_fec_status(io);
                self.finish_fec_block(io, true);
                // Keep every data packet of the incomplete block; the records survive holes.
                self.stage_complete_fec_block(depacketizer, io);
            }
            if frame != self.current_frame {
                self.submit_completed_frame(depacketizer, io);
                depacketizer.finish_pyrowave_frame(io);
            }
            self.current_frame = frame;
            self.current_block = block;
        }
        let result = self.add_ordinary(packet, depacketizer, io);
        if self.current_frame != frame {
            depacketizer.finish_pyrowave_frame(io);
        }
        result
    }

    /// Whether `add_packet` would reject this unencrypted datagram before reading anything else,
    /// so the caller can drop it without taking a packet buffer. Mirrors the rejections that
    /// `add_ordinary` makes first, with the same effective block (0 for a legacy host), and
    /// counts nothing. A PyroWave record stream is never rejected here.
    pub(super) fn rejects_datagram(&self, datagram: &[u8]) -> bool {
        if self.records {
            return false;
        }
        let seq = u16::from_be_bytes([datagram[2], datagram[3]]);
        let offset = rtp_data_offset(datagram[0]);
        if datagram.len() < offset + NV_VIDEO_PACKET_SIZE {
            return true;
        }
        let frame = le32(datagram, offset + 4);
        let block = if self.multi_fec_capable {
            (datagram[offset + 11] >> 4) & 3
        } else {
            0
        };
        // A far-behind position reaches `add_packet`, which counts it: a straggler or a replay is
        // dropped there unless its datagrams keep agreeing on the position.
        if self.is_rewind(frame) {
            return false;
        }
        // A new frame resets the block window from its own sequence number, so one behind the
        // last is no duplicate; the frame being assembled keeps the check.
        let fresh_frame = frame != self.current_frame && !is_before16(frame, self.current_frame);
        (!fresh_frame && is_before16(seq.into(), self.next_contiguous_seq.into()))
            || is_before16(frame, self.current_frame)
            || (frame == self.current_frame && block < self.current_block)
    }

    fn add_ordinary<S: VideoSink>(
        &mut self,
        mut packet: Packet,
        depacketizer: &mut Depacketizer,
        io: &mut Io<S>,
    ) -> Result<(), Packet> {
        let offset = packet.data_offset();
        if packet.len < offset + NV_VIDEO_PACKET_SIZE {
            return Err(packet);
        }
        let frame = le32(&packet.buf, offset + 4);
        // Legacy hosts look like single-block multi-FEC hosts.
        let block = if self.multi_fec_capable {
            (packet.buf[offset + 11] >> 4) & 0x3
        } else {
            0
        };
        if !self.multi_fec_capable {
            packet.buf[offset + 10] = 0x10;
            packet.buf[offset + 11] = 0;
        }
        let fec_info = le32(&packet.buf, offset + 12);
        let blocks = packet.buf[offset + 11];

        if self.is_rewind(frame) {
            // The counters went back instead of slipping: a straggler or a replay is dropped
            // unless datagrams keep coming from the position, as after a host reset they do.
            if !self.confirm_rewind(frame, packet.seq) {
                return Err(packet);
            }
            self.purge_pending(io);
            self.purge_completed(depacketizer, io);
            depacketizer.resync(frame, io);
            self.start_block(frame, block, blocks, fec_info, packet.seq, io);
        } else {
            self.rewind = None;
            let starts_block =
                self.pending.is_empty() || self.current_frame != frame || self.current_block != block;
            // A jump past both bounds that only a burst of loss explains is loss, counted and
            // invalidated by the block start below; one the counters disagree on is a
            // discontinuity. Resyncing on it stops every real packet after it looking "before".
            let discontinuity = starts_block && self.forward_jump_disagrees(frame, packet.seq);
            // A new frame resets the block window from its own sequence number, so one behind the
            // last is no duplicate of the frame being assembled.
            let fresh_frame =
                frame != self.current_frame && !is_before16(frame, self.current_frame);
            if is_before16(packet.seq.into(), self.next_contiguous_seq.into())
                && !discontinuity
                && !fresh_frame
            {
                return Err(packet);
            }
            if is_before16(frame, self.current_frame) && !discontinuity {
                return Err(packet);
            }
            if frame == self.current_frame && block < self.current_block && !discontinuity {
                return Err(packet);
            }

            // Start a new block when the queue is empty after a delivery or when the next frame or
            // block begins before this one completed.
            if discontinuity {
                // Drop what was in flight: the counters say it belongs to another stream or
                // another stretch of frames and cannot be joined to this datagram. Nothing is
                // counted lost, and the key frame asked for recovers the picture.
                self.purge_pending(io);
                self.purge_completed(depacketizer, io);
                depacketizer.resync(frame, io);
                self.start_block(frame, block, blocks, fec_info, packet.seq, io);
            } else if starts_block {
                if !self.pending.is_empty() {
                    self.report_fec_status(io);
                    self.finish_fec_block(io, true);
                    // Losing one block loses the rest of its frame.
                    if self.last_block != 0 && self.current_frame == frame {
                        self.purge_pending(io);
                        self.purge_completed(depacketizer, io);
                        if !self.reported_lost_frame {
                            self.notify_frame_lost(depacketizer, io, self.current_frame, false);
                        }
                        self.current_frame = self.current_frame.wrapping_add(1);
                        self.current_block = 0;
                        self.reported_lost_frame = false;
                        return Err(packet);
                    }
                }

                // A block must continue the current frame or start the next frame at block 0.
                let expected = if self.current_frame == frame {
                    self.current_block
                } else {
                    0
                };
                if block != expected {
                    self.report_fec_status(io);
                    self.purge_pending(io);
                    self.purge_completed(depacketizer, io);
                    if !self.reported_lost_frame {
                        self.notify_frame_lost(depacketizer, io, frame, false);
                    }
                    self.current_frame = frame.wrapping_add(1);
                    self.current_block = 0;
                    self.reported_lost_frame = false;
                    return Err(packet);
                }

                self.purge_pending(io);
                if self.current_frame != frame {
                    self.purge_completed(depacketizer, io);
                    // A whole frame went missing. The frame before this one may already have been
                    // reported by the speculative RFI logic.
                    if self.current_frame.wrapping_add(1) != frame || !self.reported_lost_frame {
                        self.notify_frame_lost(depacketizer, io, frame.wrapping_sub(1), false);
                    }
                }

                self.start_block(frame, block, blocks, fec_info, packet.seq, io);
            }
        }

        if is_before16(self.buffer_highest_seq.into(), packet.seq.into()) {
            return Err(packet);
        }

        let seq = packet.seq;
        let is_parity = !is_before16(seq.into(), self.buffer_first_parity_seq.into());
        self.queue_packet(packet, is_parity, false)?;

        if self.pending.len() == 1 {
            self.missing_packets += u32::from(seq.wrapping_sub(self.buffer_lowest_seq));
            self.received_highest_seq = seq;
        } else if is_before16(self.received_highest_seq.into(), seq.into()) {
            self.missing_packets +=
                u32::from(seq.wrapping_sub(self.received_highest_seq).wrapping_sub(1));
            self.received_highest_seq = seq;
        } else {
            // An accepted packet behind the highest one fills a hole.
            self.missing_packets = self.missing_packets.wrapping_sub(1);
        }

        if is_parity {
            self.received_parity_packets += 1;
        } else {
            self.received_data_packets += 1;
        }

        if self.early && !is_parity {
            self.forward_ready(seq, depacketizer, io);
        }

        if self.reconstruct_frame(depacketizer, io) == 0 {
            self.finish_fec_block(io, false);
            self.stage_complete_fec_block(depacketizer, io);
            if self.current_block < self.last_block {
                self.current_block += 1;
                self.queue_completed_ahead(depacketizer, io);
            } else {
                self.submit_completed_frame(depacketizer, io);
                self.current_frame = self.current_frame.wrapping_add(1);
                self.current_block = 0;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::tests::config;

    /// More packets than a test block uses, so one word covers it whichever way it wraps.
    const BLOCK_PACKETS: u16 = 33;

    fn packet(seq: u16) -> Packet {
        Packet {
            buf: vec![0u8; 64].into_boxed_slice(),
            len: 32,
            seq,
            timestamp: 0,
            receive_time_us: 0,
            presentation_time_us: 0,
            rtp_timestamp: 0,
            is_parity: false,
        }
    }

    /// A queue whose block already started at `lowest`.
    fn queue_at(lowest: u16) -> RtpQueue {
        let mut queue = RtpQueue::new(&config(0x1000));
        queue.buffer_lowest_seq = lowest;
        queue.buffer_highest_seq = lowest.wrapping_add(BLOCK_PACKETS - 1);
        queue.next_contiguous_seq = lowest;
        queue.received_highest_seq = 0;
        queue.use_fast_queue_path = true;
        queue.seen = [0; 57];
        queue
    }

    /// A block past the signed-16 midpoint must not read its first packet as out of order.
    #[test]
    fn a_block_starting_at_or_above_0x8000_takes_its_first_packet_in_order() {
        for lowest in [0x8000u16, 0x8001, 0xFFFF, 0x0000] {
            let mut queue = queue_at(lowest);
            assert!(
                queue.queue_packet(packet(lowest), false, false).is_ok(),
                "lowest {lowest:#06x}"
            );
            assert!(!queue.received_oos_data, "lowest {lowest:#06x}");
            assert_eq!(queue.next_contiguous_seq, lowest.wrapping_add(1));
        }
    }

    /// The 16-bit order hosts count in: a wrapped counter is adjacent, and only the half-range
    /// ahead of a position counts as ahead of it.
    #[test]
    fn a_counter_is_ahead_only_within_half_the_16_bit_order() {
        assert_eq!(ahead16(2, 1), 1, "one frame on");
        assert_eq!(ahead16(1, 2), 0, "behind is not ahead");
        assert_eq!(ahead16(0x1_0000, 0xFFFF), 1, "a host's counter wraps");
        assert_eq!(ahead16(3, 0xFFFF), 4, "across the wrap");
        assert_eq!(ahead16(0x7FFF, 0), 0x7FFF, "just within half the range");
        assert_eq!(ahead16(0x8000, 0), 0, "half the range away is not ahead");
    }

    /// A burst of loss advances the frame and sequence counters together; only a jump that one
    /// counter cannot explain is a discontinuity.
    #[test]
    fn a_jump_agrees_only_as_far_as_frames_can_carry_packets() {
        let mut queue = queue_at(0);
        queue.seek(100);
        queue.next_contiguous_seq = 1_000;
        // Six frames and 6,250 packets: a 100 ms burst at 500 Mbps, which those frames carry.
        assert!(!queue.forward_jump_disagrees(106, 7_250));
        // A frame counter that jumped with no packets behind it is no burst.
        assert!(queue.forward_jump_disagrees(10_000, 1_001));
        // Nor is a sequence counter that outran what the frames can carry.
        assert!(queue.forward_jump_disagrees(101, 31_000));
        // Below both bounds it is ordinary loss.
        assert!(!queue.forward_jump_disagrees(101, 2_000));
        // A frame that went backwards is a straggler, not a forward jump.
        assert!(!queue.forward_jump_disagrees(99, 5_000));
    }

    /// A duplicate after a reordered packet is rejected, inside the contiguous run or behind it.
    #[test]
    fn a_duplicate_is_rejected_under_reordering() {
        let mut queue = queue_at(0x8000);
        assert!(queue.queue_packet(packet(0x8002), false, false).is_ok());
        assert!(!queue.use_fast_queue_path);
        assert_eq!(queue.next_contiguous_seq, 0x8000);
        assert!(queue.queue_packet(packet(0x8000), false, false).is_ok());
        assert_eq!(queue.next_contiguous_seq, 0x8000);
        assert!(queue.queue_packet(packet(0x8000), false, false).is_err());
        assert!(queue.queue_packet(packet(0x8002), false, false).is_err());
        assert_eq!(queue.pending.len(), 2);
    }
}
