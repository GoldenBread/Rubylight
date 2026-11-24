//! Shared slot, backlog accounting, and block-pool recycling between the encode loop,
//! video packetizer thread, and paced transport thread.

use super::*;

pub(super) struct Frame {
    pub(super) encoded: Encoded,
    pub(super) peer: SocketAddr,
    pub(super) claimed: Instant,
    pub(super) polled: Instant,
    pub(super) latency: Duration,
    /// Capture/encode-loop tick that produced this output (Punktfunk's `cap_at`).
    pub(super) processing_started: Instant,
    /// Frames skipped just before this one; their wire numbers stay used.
    pub(super) skipped: u32,
    /// A new picture's claim, for the stage timings.
    pub(super) claim: Option<Claim>,
    /// The encoder's slice delivery when the frame was submitted.
    pub(super) slice_blocks: usize,
}

/// Fully prepared packets. Their Moonlight timing is immutable before the send queue.
pub(super) struct Prepared {
    pub(super) blocks: InlineBlocks<PacketBlock>,
    pub(super) counts: InlineBlocks<usize>,
    pub(super) layout: InlineBlocks<(usize, usize)>,
    pub(super) packetizer: VideoPacketizer,
    pub(super) wire_frame: u32,
    pub(super) idr: bool,
    pub(super) after_invalidation: bool,
    pub(super) frame_cap_bytes: Option<u64>,
    /// The cap the host asked the encoder for (AMF only).
    pub(super) cap_target_bytes: Option<u64>,
    /// AMF encoded this keyframe again in place of an oversize scene cut.
    pub(super) reencoded: bool,
    /// Too large for full FEC within Moonlight's four blocks.
    pub(super) fec_limited: bool,
    pub(super) payload_bytes: usize,
    pub(super) peer: SocketAddr,
    pub(super) claimed: Instant,
    /// The encoder's first unit of the frame was in hand. A whole frame has one.
    pub(super) polled: Instant,
    /// The frame's last unit was in hand: the same as `polled` for a whole frame, and for
    /// a streamed one the part that completes it.
    pub(super) last_polled: Instant,
    /// Blocks of a streamed frame that were built before its last unit arrived.
    pub(super) early_blocks: usize,
    pub(super) packetized: Instant,
    pub(super) prepared: Instant,
    pub(super) latency: u64,
    pub(super) processing: u64,
    pub(super) age: u64,
    pub(super) claim: Option<Claim>,
    /// The RTP timestamp on the wire, for the timing record.
    pub(super) timestamp: u32,
    /// Set when this is one FEC block of a streamed frame, not a whole frame.
    pub(super) stream: Option<Part>,
    /// The encoder returned the frame whole, even when it goes out block by
    /// block; false for a frame the encoder delivered in units.
    pub(super) whole: bool,
    /// On the first part of a whole frame sent block by block: the frame's
    /// wire bytes, which the keyframe recovery estimate counts from the start.
    pub(super) frame_wire_bytes: Option<usize>,
}

impl Prepared {
    /// A later block of a whole frame sent block by block: the frame is
    /// already counted, and its keyframe already timed, from its first.
    pub(super) fn continues_whole(&self) -> bool {
        self.whole && self.stream.is_some_and(|part| !part.first)
    }
}

/// Where a block sits in a streamed frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Part {
    pub(super) first: bool,
    pub(super) last: bool,
}

/// Blocks kept for reuse. A frame's layout takes at most four blocks, and
/// up to four frames hold blocks at once: the one being packetized, the
/// ready one, and the two the paced sender is sending or has in flight. Four
/// times four is 16; blocks past that are freed outside the lock.
pub(super) const RECYCLED_BLOCKS: usize = 16;

pub(super) struct State {
    pub(super) pending: Option<Frame>,
    pub(super) sending: bool,
    pub(super) sending_keyframe: bool,
    pub(super) ready: Option<Prepared>,
    pub(super) transmitting: bool,
    pub(super) transmitting_keyframe: bool,
    /// Updated after actual send progress, never a wall-clock-only countdown.
    pub(super) transmitting_remaining: Option<Duration>,
    /// The stream thread's last recovery lead: send progress wakes it only
    /// when the remaining time crosses it.
    pub(super) recovery_lead: Option<Duration>,
    /// Blocks the packetizer fills again instead of allocating their shard
    /// buffers. Kept per block, so a whole frame and a block-by-block frame
    /// draw on the same pool.
    pub(super) recycled: Vec<PacketBlock>,
    /// A streamed frame is open on the packetizer thread: some of its blocks
    /// may be out and the encoder still produces the rest. It counts toward
    /// the claim backlog only, so a new picture is not claimed (and aged)
    /// behind it, while recreation still waits just for what is queued.
    pub(super) streaming: bool,
    /// A whole frame goes to the paced sender block by block: it counts once,
    /// though it is being packetized and sent at the same time.
    pub(super) whole_open: bool,
    pub(super) next_wire_frame: u32,
    pub(super) skipped: u32,
    pub(super) error: Option<String>,
}

impl State {
    /// A client waiting for a keyframe throws away the frames before it.
    /// The skipped frame keeps its wire number, so the encoder's frame
    /// numbers (reference frame invalidation) still match the wire.
    /// A streamed frame's units and blocks are not skipped: the frame is
    /// already half on the wire, and dropping a piece of it would leave a
    /// frame the client cannot finish and an index nobody consumed.
    /// Blocks past the pool's cap go to `spill`, which the caller drops after
    /// releasing the state lock.
    pub(super) fn skip_pending(&mut self, spill: &mut Vec<PacketBlock>) -> bool {
        let raw = self
            .pending
            .as_ref()
            .is_some_and(|frame| !frame.encoded.idr && frame.encoded.unit == Unit::Whole);
        if raw {
            self.pending = None;
            self.skipped = self.skipped.wrapping_add(1);
        }
        let prepared = self
            .ready
            .as_ref()
            .is_some_and(|frame| !frame.idr && frame.stream.is_none());
        if prepared {
            // Packetization has already spent this frame's numbers and IVs.
            let frame = self.ready.take().unwrap();
            self.recycle(frame.blocks, spill);
        }
        raw || prepared
    }

    /// Takes a block from the pool, or an empty one that allocates on use.
    pub(super) fn take_block(&mut self) -> PacketBlock {
        self.recycled.pop().unwrap_or_default()
    }

    /// Returns blocks to the pool up to [`RECYCLED_BLOCKS`]; the rest go to
    /// `spill`, so that nothing is freed while the state lock is held.
    pub(super) fn recycle(
        &mut self,
        blocks: impl IntoIterator<Item = PacketBlock>,
        spill: &mut Vec<PacketBlock>,
    ) {
        for block in blocks {
            if self.recycled.len() < RECYCLED_BLOCKS {
                self.recycled.push(block);
            } else {
                spill.push(block);
            }
        }
    }

    pub(super) fn backlog(&self) -> Result<usize> {
        if let Some(error) = &self.error {
            bail!("video sender stopped: {error}");
        }
        let frame = if self.whole_open {
            1
        } else {
            usize::from(self.sending)
                + usize::from(self.transmitting)
                + usize::from(
                    self.ready
                        .as_ref()
                        .is_some_and(|f| !(self.transmitting && f.continues_whole())),
                )
        };
        Ok(frame + usize::from(self.pending.is_some()))
    }

    pub(super) fn claim_backlog(&self, recovery_lead: Option<Duration>) -> Result<usize> {
        Ok(self.backlog()?
            + usize::from(
                self.streaming
                    || self.sending_keyframe
                    || (self.transmitting_keyframe
                        && !self
                            .transmitting_remaining
                            .zip(recovery_lead)
                            .is_some_and(|(remaining, lead)| remaining <= lead))
                    || self.pending.as_ref().is_some_and(|f| f.encoded.idr)
                    || self
                        .ready
                        .as_ref()
                        .is_some_and(|f| f.idr && !f.continues_whole()),
            ))
    }
}

pub(super) struct Slot {
    pub(super) state: Mutex<State>,
    /// The pending slot and the backlog: the packetizer waits here for a
    /// frame, the stream thread for room. The paced sender never does.
    pub(super) changed: Condvar,
    /// The prepared slot: the paced sender waits here for a frame, the
    /// packetizer for room.
    pub(super) ready_changed: Condvar,
    pub(super) stop: AtomicBool,
    pub(super) stream_id: String,
}

impl Slot {
    pub(super) fn finish(&self, result: Result<()>) {
        let mut state = self.state.lock().unwrap();
        if let Err(error) = result {
            state.error.get_or_insert_with(|| format!("{error:#}"));
        }
        self.stop.store(true, Ordering::Release);
        state.pending = None;
        state.ready = None;
        state.sending = false;
        state.sending_keyframe = false;
        state.streaming = false;
        state.whole_open = false;
        state.transmitting = false;
        state.transmitting_keyframe = false;
        state.transmitting_remaining = None;
        self.changed.notify_all();
        self.ready_changed.notify_all();
    }

    pub(super) fn enqueue(&self, frame: Prepared) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        while state.ready.is_some() && !self.stop.load(Ordering::Acquire) {
            state = self.ready_changed.wait(state).unwrap();
        }
        state.backlog()?;
        if !self.stop.load(Ordering::Acquire) {
            // The backlog does not shrink: the frame was counted as being sent.
            state.ready = Some(frame);
            self.ready_changed.notify_one();
        }
        Ok(())
    }

    pub(super) fn run(&self, mut send: impl FnMut(Frame) -> Result<u32>) -> Result<()> {
        loop {
            let frame = {
                let mut state = self.state.lock().unwrap();
                while state.pending.is_none() && !self.stop.load(Ordering::Acquire) {
                    state = self.changed.wait(state).unwrap();
                }
                if self.stop.load(Ordering::Acquire) {
                    return Ok(());
                }
                state.sending = true;
                let mut frame = state.pending.take().unwrap();
                state.sending_keyframe = frame.encoded.idr;
                frame.skipped = std::mem::take(&mut state.skipped);
                self.changed.notify_all();
                frame
            };
            let next_wire_frame = send(frame)?;
            let mut state = self.state.lock().unwrap();
            state.next_wire_frame = next_wire_frame;
            state.sending = false;
            state.sending_keyframe = false;
            // With `sending`: the frame's blocks are all queued or out, and it
            // still counts once.
            state.whole_open = false;
            self.changed.notify_all();
        }
    }

    /// The packetizer gave up a whole frame part way through its blocks. A
    /// frame given up while the sender waits for its next block is no longer
    /// a keyframe in flight; one whose block is being sent ends with it.
    pub(super) fn abandon_whole(&self) {
        let mut state = self.state.lock().unwrap();
        state.whole_open = false;
        if !state.transmitting && state.ready.is_none() {
            state.transmitting_keyframe = false;
            state.transmitting_remaining = None;
        }
        self.changed.notify_all();
    }

    pub(super) fn stop(&self) {
        let _state = self.state.lock().unwrap();
        self.stop.store(true, Ordering::Release);
        self.changed.notify_all();
        self.ready_changed.notify_all();
    }

    /// A frame was skipped from the pending or the prepared slot: room for
    /// the stream thread, and for a packetizer waiting to hand over.
    pub(super) fn skipped(&self) {
        self.changed.notify_all();
        self.ready_changed.notify_one();
    }
}
