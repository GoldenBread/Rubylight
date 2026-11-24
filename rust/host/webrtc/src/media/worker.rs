//! Media transport receive/send threads, pacing loop and command dispatch.

use super::{
    Batch, EARLY_SEND, Engine, IdrReason, RECV_DATAGRAM_BYTES, SendQueue, TransportEvent,
    handle::{Command, Shared},
};
use anyhow::{Context, Result};
use butterpollo_win_graphics::timing::Timer;
use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Start the receive and media threads. If the media thread cannot start, the receive thread is
/// stopped and joined so the socket (and its port) is released before the error returns.
pub(super) fn start_threads(
    shared: &Shared,
    receive: impl FnOnce() -> std::io::Result<JoinHandle<()>>,
    media: impl FnOnce() -> std::io::Result<JoinHandle<()>>,
) -> Result<Vec<JoinHandle<()>>> {
    let receive = receive().context("starting the receive thread")?;
    match media() {
        Ok(media) => Ok(vec![receive, media]),
        Err(e) => {
            shared.stop.store(true, Ordering::Release);
            let _ = receive.join();
            Err(anyhow::Error::new(e).context("starting the media thread"))
        }
    }
}

/// Consecutive receive errors tolerated before the receive thread backs off: a lone error costs
/// one datagram and no latency, a persistent one must not spin the thread.
pub(super) const RECV_ERRORS_BEFORE_BACKOFF: u32 = 8;
pub(super) const RECV_ERROR_BACKOFF: Duration = Duration::from_millis(5);

/// Count one more consecutive receive error; the pause to take, if any.
pub(super) fn recv_error_backoff(consecutive: &mut u32) -> Option<Duration> {
    *consecutive = consecutive.saturating_add(1);
    (*consecutive >= RECV_ERRORS_BEFORE_BACKOFF).then_some(RECV_ERROR_BACKOFF)
}

pub(super) fn receive_loop(socket: UdpSocket, tx: Sender<Command>, shared: Arc<Shared>) {
    let mut buf = vec![0u8; RECV_DATAGRAM_BYTES];
    let mut consecutive_errors = 0u32;
    while !shared.stop.load(Ordering::Acquire) {
        match socket.recv_from(&mut buf) {
            Ok((len, from)) => {
                consecutive_errors = 0;
                let counters = &shared.counters;
                counters.datagrams_received.fetch_add(1, Ordering::Relaxed);
                // Only the datagram that takes the queue from empty to non-empty wakes the media
                // thread: the rest of a burst is already behind it in the channel, which the media
                // thread drains before it waits, so one signal covers the whole burst.
                let pending = shared.pending_datagrams.fetch_add(1, Ordering::AcqRel);
                if pending >= shared.options.datagram_queue {
                    shared.pending_datagrams.fetch_sub(1, Ordering::AcqRel);
                    if counters
                        .datagrams_dropped
                        .fetch_add(1, Ordering::Relaxed)
                        .is_multiple_of(1000)
                    {
                        tracing::warn!(
                            "webrtc: the media thread is behind, dropping received datagrams"
                        );
                    }
                    continue;
                }
                let command = Command::Datagram {
                    at: Instant::now(),
                    from,
                    data: buf[..len].to_vec(),
                };
                if tx.send(command).is_err() {
                    return;
                }
                if pending == 0 {
                    shared.notify();
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                consecutive_errors = 0
            }
            // Windows reports an ICMP port-unreachable for an earlier send as a receive error.
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
            // Anything else costs the datagram, not the session: whoever can reach this port
            // must not be able to end it.
            Err(e) => {
                let n = shared.counters.recv_errors.fetch_add(1, Ordering::Relaxed);
                if n.is_multiple_of(100) {
                    tracing::warn!(
                        "webrtc: media socket receive failed ({} so far): {e}",
                        n + 1
                    );
                }
                if let Some(pause) = recv_error_backoff(&mut consecutive_errors) {
                    std::thread::sleep(pause);
                }
            }
        }
    }
}

pub(super) fn media_loop(
    mut engine: Engine,
    mut queue: SendQueue,
    socket: UdpSocket,
    rx: mpsc::Receiver<Command>,
    shared: Arc<Shared>,
) {
    let result = (|| -> Result<()> {
        // The high-resolution waitable timer the host's other senders pace with; it belongs to
        // this thread.
        let timer = Timer::new().context("creating the pacing timer")?;
        // The datagrams being sent, kept between batches.
        let mut batch = Batch::default();
        loop {
            if shared.stop.load(Ordering::Acquire) {
                return Ok(());
            }
            // Reset before draining: whatever is queued after the drain sets it again, so the
            // wait below returns at once instead of sleeping on it.
            shared.wake.reset()?;
            loop {
                match rx.try_recv() {
                    Ok(Command::Stop) | Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
                    Ok(command) => handle_command(&mut engine, &shared, command),
                    Err(mpsc::TryRecvError::Empty) => break,
                }
            }
            let timeout = engine.advance(Instant::now())?;
            // The frame's size first: it sets the pace its own packets are sent at.
            let frame = engine.take_frame_wire_bytes();
            if frame > 0 {
                queue.note_frame(frame);
            }
            for out in engine.drain_outgoing() {
                queue.push(out);
            }
            enforce_backlog(&mut queue, &shared);
            dispatch(&mut engine, &shared);
            if engine.is_closed() {
                return Ok(());
            }
            send_due(&socket, &mut queue, &mut batch, &shared);
            let (rejected, failed) = engine.dropped_datagrams();
            shared
                .counters
                .datagrams_rejected
                .store(rejected, Ordering::Relaxed);
            shared
                .counters
                .datagram_errors
                .store(failed, Ordering::Relaxed);
            // The receive thread wakes this thread only for the datagram that took the queue
            // from empty, and a datagram is counted before its bytes are copied into the
            // channel: one that arrives as the drain above ends is already in the queue, so
            // drain again rather than sleep until the timer.
            if shared.pending_datagrams.load(Ordering::Acquire) > 0 {
                continue;
            }
            match queue.due(Instant::now()) {
                Some(due) if due < timeout => {
                    timer.until_paced_or_signal(due, &shared.wake)?;
                }
                _ => {
                    timer.until_or_signal(timeout, &shared.wake)?;
                }
            }
        }
    })();
    if let Err(e) = result {
        tracing::error!("webrtc: media thread stopped: {e:#}");
    }
    if !engine.connected {
        // `received` tells a browser that never reached this socket from one that did.
        tracing::warn!(
            media = %engine.local,
            received = engine.received,
            "webrtc: the stream ended before its media connected"
        );
    }
    engine.disconnect();
    (shared.hooks.event)(TransportEvent::Closed);
}

/// The pacer fell further behind than it can ever catch up: what it holds is stale, so drop it
/// and start over from a keyframe instead of delivering late.
pub(super) fn enforce_backlog(queue: &mut SendQueue, shared: &Shared) {
    if !queue.backlog_exceeded() {
        return;
    }
    let dropped = queue.drop_paced() as u64;
    let total = shared
        .counters
        .paced_dropped
        .fetch_add(dropped, Ordering::Relaxed)
        + dropped;
    tracing::warn!(
        "webrtc: the pacer fell behind, dropped {dropped} packets ({total} so far), asking for a keyframe"
    );
    shared.need_keyframe.store(true, Ordering::Release);
    shared.request_idr(IdrReason::PacerBacklog);
}

pub(super) fn handle_command(engine: &mut Engine, shared: &Shared, command: Command) {
    let now = Instant::now();
    let counters = &shared.counters;
    match command {
        Command::Datagram { at, from, data } => {
            shared.pending_datagrams.fetch_sub(1, Ordering::AcqRel);
            // The outcome is counted from the engine's own tallies.
            engine.handle_datagram(at.min(now), from, &data);
        }
        Command::Video(frame) => {
            shared.pending_video.fetch_sub(1, Ordering::AcqRel);
            if shared.need_keyframe.load(Ordering::Acquire) && !frame.keyframe {
                // Queued before a drop: it references a frame that never left.
                counters.video_dropped.fetch_add(1, Ordering::Relaxed);
                return;
            }
            let (bytes, keyframe) = (frame.data.len() as u64, frame.keyframe);
            match engine.write_video(now, frame) {
                Ok(true) => {
                    counters.video_frames.fetch_add(1, Ordering::Relaxed);
                    counters.video_bytes.fetch_add(bytes, Ordering::Relaxed);
                    if keyframe {
                        shared.need_keyframe.store(false, Ordering::Release);
                    }
                }
                Ok(false) => {
                    // Not connected, or the path is down: this frame never left, so the next
                    // one needs a keyframe (`Connected` and `Reconnected` ask for it).
                    counters.video_dropped.fetch_add(1, Ordering::Relaxed);
                    shared.need_keyframe.store(true, Ordering::Release);
                }
                Err(e) => {
                    tracing::error!("webrtc: {e:#}");
                    counters.video_dropped.fetch_add(1, Ordering::Relaxed);
                    shared.need_keyframe.store(true, Ordering::Release);
                    shared.request_idr(IdrReason::WriteFailed);
                }
            }
        }
        Command::Audio(packet) => {
            shared.pending_audio.fetch_sub(1, Ordering::AcqRel);
            match engine.write_audio(now, packet) {
                Ok(true) => {
                    counters.audio_packets.fetch_add(1, Ordering::Relaxed);
                }
                Ok(false) => {
                    counters.audio_dropped.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => {
                    tracing::error!("webrtc: {e:#}");
                    counters.audio_dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        Command::Control { data, binary } => {
            let sent = engine.send_control(&data, binary);
            note_channel_send(shared, "control", sent);
        }
        Command::Gamepad { data, binary } => {
            let sent = engine.send_gamepad(&data, binary);
            note_channel_send(shared, "gamepad", sent);
        }
        Command::Stop => {}
    }
}

/// A message for the client that couldn't go: the channel isn't open, its buffer is full, or
/// the write failed. Counted, and logged for the first and every hundredth.
pub(super) fn note_channel_send(shared: &Shared, channel: &str, sent: Result<bool>) {
    let counters = &shared.counters;
    let why = match sent {
        Ok(true) => {
            counters.channel_sent.fetch_add(1, Ordering::Relaxed);
            return;
        }
        Ok(false) => "the channel is not open or its buffer is full".to_string(),
        Err(e) => format!("{e:#}"),
    };
    let n = counters.channel_dropped.fetch_add(1, Ordering::Relaxed);
    if n.is_multiple_of(100) {
        tracing::warn!(
            "webrtc: {channel} message not sent ({} so far): {why}",
            n + 1
        );
    }
}

fn dispatch(engine: &mut Engine, shared: &Shared) {
    for reason in engine.take_idr_requests() {
        shared.request_idr(reason);
    }
    for event in engine.take_events() {
        (shared.hooks.event)(event);
    }
}

pub(super) fn send_due(
    socket: &UdpSocket,
    queue: &mut SendQueue,
    batch: &mut Batch,
    shared: &Shared,
) {
    let mut now = Instant::now();
    // The pacing timer wakes up to its learned lead early; that is on schedule.
    while queue.due(now).is_some_and(|due| due <= now + EARLY_SEND) {
        queue.fill_batch(now + EARLY_SEND, batch);
        if batch.packets.is_empty() {
            break;
        }
        if let Some(last) = batch.packets.last() {
            let mut peer = shared.peer.lock().unwrap();
            if *peer != Some(last.destination) {
                *peer = Some(last.destination);
            }
        }
        // The simulated link loses them before the socket; the pacer still spent their time.
        let before = batch.packets.len();
        batch.packets.retain(|out| !queue.lost_in_the_air(out));
        shared
            .counters
            .simulated_losses
            .fetch_add((before - batch.packets.len()) as u64, Ordering::Relaxed);
        // In order: each peer's run of datagrams in as few writes as segmentation allows.
        let mut rest = &batch.packets[..];
        while let Some(first) = rest.first() {
            let to = first.destination;
            let (run, tail) =
                rest.split_at(rest.iter().take_while(|out| out.destination == to).count());
            rest = tail;
            let sent = batch.segments.send(socket, run, to);
            let counters = &shared.counters;
            counters
                .datagrams_sent
                .fetch_add(sent.datagrams as u64, Ordering::Relaxed);
            counters
                .bytes_sent
                .fetch_add(sent.bytes as u64, Ordering::Relaxed);
            if let Some(e) = sent.error {
                // Loud, but not fatal: the peer may be between networks. Logged for the first
                // and every hundredth.
                let failed = sent.failed as u64;
                let n = counters.send_errors.fetch_add(failed, Ordering::Relaxed);
                if n.next_multiple_of(100) < n + failed {
                    tracing::warn!("webrtc: sending to {to} failed: {e}");
                }
            }
        }
        now = Instant::now();
        queue.sent(now, batch);
    }
    // The sent datagrams' buffers go now, not with the next batch.
    batch.packets.clear();
}
