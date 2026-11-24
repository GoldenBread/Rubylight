//! Shared fixtures and test modules for the video sender worker, packetizer handoff, and block pool.

mod benchmark_and_mtu;
mod early_blocks_and_pool;
mod slot_and_capacity;
mod streamed_delivery;
mod timing_and_fec;

use super::*;
use std::net::UdpSocket;
use std::sync::mpsc::{self, Receiver, Sender as Channel};

pub(super) const WAIT: Duration = Duration::from_secs(5);

pub(super) fn peer() -> SocketAddr {
    "127.0.0.1:12345".parse().unwrap()
}

pub(super) fn frame(index: u8) -> Encoded {
    Encoded {
        framing: None,
        bytes: vec![index; if index == 1 { 300_000 } else { 3000 }],
        unit: butterpollo_win_graphics::encoder::Unit::Whole,
        idr: index == 1,
        after_invalidation: index == 3,
        frame_cap_bytes: None,
        reencoded: false,
        frame_cap_target: None,
        latency: None,
        presentation: None,
    }
}

pub(super) fn blocked_sender() -> (Sender, Receiver<Frame>, Channel<()>) {
    let (entered, frames) = mpsc::channel();
    let (release, resume) = mpsc::channel();
    let sender = Sender::spawn("test".into(), 1, move |slot| {
        slot.run(|frame| {
            let next = u32::from(frame.encoded.bytes[0]) + 1;
            entered.send(frame).unwrap();
            resume.recv_timeout(WAIT)?;
            Ok(next)
        })
    })
    .unwrap();
    (sender, frames, release)
}
pub(super) fn packetizer(key: Option<[u8; 16]>) -> VideoPacketizer {
    VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: 1024,
        fec_percent: 20,
        min_fec: 2,
        key,
    }
}

pub(super) fn handoff(packetizer: VideoPacketizer) -> Handoff {
    Handoff {
        packetizer,
        present_stamper: None,
        last_stamp: Instant::now(),
        pacing_due: Instant::now(),
        send_loss: Default::default(),
        slice_blocks: 0,
        slices: 0,
        send_jitter: None,
        link: Default::default(),
    }
}

pub(super) fn wait_prepared(sender: &Sender) -> std::sync::MutexGuard<'_, State> {
    let state = sender.slot.state.lock().unwrap();
    let (state, timeout) = sender
        .slot
        .ready_changed
        .wait_timeout_while(state, WAIT, |s| s.ready.is_none())
        .unwrap();
    assert!(!timeout.timed_out(), "packetizer did not prepare a frame");
    state
}
/// One unit of a streamed frame.
pub(super) fn unit(
    unit: butterpollo_win_graphics::encoder::Unit,
    shards: usize,
    fill: u8,
) -> Encoded {
    Encoded {
        framing: None,
        bytes: vec![fill; shards * 1008],
        unit,
        idr: false,
        after_invalidation: false,
        frame_cap_bytes: Some(100 * 1008),
        reencoded: false,
        frame_cap_target: None,
        latency: None,
        presentation: None,
    }
}

pub(super) fn slice(index: u32, shards: usize, fill: u8) -> Encoded {
    unit(
        butterpollo_win_graphics::encoder::Unit::Slice { index },
        shards,
        fill,
    )
}

pub(super) fn last(index: u32, shards: usize, fill: u8) -> Encoded {
    unit(
        butterpollo_win_graphics::encoder::Unit::Last { index },
        shards,
        fill,
    )
}

/// A packetizer thread whose prepared slot the test drains by hand, as the
/// paced sender would.
pub(super) fn streaming_sender(session: Arc<Session>, blocks: usize, slices: usize) -> Sender {
    let mut handoff = handoff(packetizer(None));
    handoff.slice_blocks = blocks;
    handoff.slices = slices;
    Sender::packetizer(session, Instant::now(), handoff, false).unwrap()
}

pub(super) fn take_prepared(sender: &Sender) -> Prepared {
    let mut state = wait_prepared(sender);
    let prepared = state.ready.take().unwrap();
    drop(state);
    sender.slot.ready_changed.notify_all();
    prepared
}

/// The next whole frame, after any parts of a streamed one.
pub(super) fn take_whole(sender: &Sender) -> Prepared {
    loop {
        let prepared = take_prepared(sender);
        if prepared.stream.is_none() {
            return prepared;
        }
    }
}

pub(super) fn submit_units(sender: &Sender, units: Vec<Encoded>) {
    sender
        .submit(units, peer(), Duration::ZERO, &mut Claims::default())
        .unwrap();
}
/// A packetizer that sends whole frames block by block to an idle sender,
/// drained by hand.
pub(super) fn early_sender(session: Arc<Session>) -> Sender {
    Sender::packetizer_with(
        session,
        Instant::now(),
        handoff(packetizer(None)),
        false,
        true,
    )
    .unwrap()
}

pub(super) fn keyframe(shards: usize) -> Encoded {
    // Encoded returns its buffer to a pool on drop, so no struct update.
    let mut frame = unit(butterpollo_win_graphics::encoder::Unit::Whole, shards, 7);
    frame.idr = true;
    frame.frame_cap_bytes = None;
    frame
}
/// A state with nothing in flight, for the block pool alone.
pub(super) fn empty_state() -> State {
    State {
        pending: None,
        sending: false,
        sending_keyframe: false,
        ready: None,
        transmitting: false,
        transmitting_keyframe: false,
        transmitting_remaining: None,
        recovery_lead: None,
        recycled: Vec::new(),
        streaming: false,
        whole_open: false,
        next_wire_frame: 1,
        skipped: 0,
        error: None,
    }
}

/// Takes a block from the pool and fills it as the packetizer does. A take the
/// pool cannot serve is a miss: the block is empty and allocates its shards.
pub(super) fn build_block(
    state: &mut State,
    misses: &mut usize,
) -> butterpollo_media_policy::packet::PacketBlock {
    const SHARDS: usize = 8;
    const SHARD_BYTES: usize = 1_400;
    let mut block = state.take_block();
    if block.capacity() == 0 {
        *misses += 1;
    } else {
        assert!(
            block.iter().all(|shard| shard.capacity() >= SHARD_BYTES),
            "a pooled block must keep its shard buffers"
        );
    }
    block.truncate(SHARDS);
    block.resize_with(SHARDS, Vec::new);
    for shard in &mut block {
        shard.clear();
        shard.resize(SHARD_BYTES, 0);
    }
    block
}
