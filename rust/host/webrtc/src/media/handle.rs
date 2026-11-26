//! Running WebRTC transport handles, shared counters, audio/control senders and guest fan-out.

use super::{
    AudioPacket, Engine, FrameData, Hooks, IDR_REQUEST_INTERVAL, IdrReason, SendQueue, Session,
    Submit, TransportOptions, TransportStats, VideoFrame, WebRtcAudioSink,
    worker::{media_loop, receive_loop, start_threads},
};
use crate::sdp::VideoCodec;
use anyhow::{Context, Result};
use butterpollo_media_policy::audio::OpusLayout;
use butterpollo_win_graphics::timing::Signal;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct Counters {
    pub(super) video_frames: AtomicU64,
    pub(super) video_bytes: AtomicU64,
    pub(super) video_dropped: AtomicU64,
    pub(super) audio_packets: AtomicU64,
    pub(super) audio_dropped: AtomicU64,
    pub(super) datagrams_sent: AtomicU64,
    pub(super) bytes_sent: AtomicU64,
    pub(super) send_errors: AtomicU64,
    pub(super) idr_requests: AtomicU64,
    pub(super) datagrams_received: AtomicU64,
    /// Received but not queued: the media thread was behind.
    pub(super) datagrams_dropped: AtomicU64,
    /// Not the peer's, or refused by str0m.
    pub(super) datagrams_rejected: AtomicU64,
    pub(super) datagram_errors: AtomicU64,
    pub(super) recv_errors: AtomicU64,
    /// Paced packets thrown away by the backlog cap.
    pub(super) paced_dropped: AtomicU64,
    pub(super) channel_sent: AtomicU64,
    pub(super) channel_dropped: AtomicU64,
    /// Video datagrams the loss switch (`webrtc_loss_permille`) threw away instead of sending.
    pub(super) simulated_losses: AtomicU64,
}

pub(super) struct Shared {
    pub(super) stop: AtomicBool,
    /// Set by every other thread after it queues work, so the media thread's one wait covers
    /// the pacer, str0m's timer and the queues.
    pub(super) wake: Signal,
    pub(super) authenticated: Arc<AtomicBool>,
    pub(super) pending_video: AtomicUsize,
    pub(super) pending_audio: AtomicUsize,
    pub(super) pending_datagrams: AtomicUsize,
    /// No frame may go out but a keyframe: nothing has been sent yet, or one was dropped, so the
    /// decoder has no reference.
    pub(super) need_keyframe: AtomicBool,
    pub(super) last_idr_ms: AtomicU64,
    pub(super) epoch: Instant,
    pub(super) counters: Counters,
    pub(super) hooks: Hooks,
    pub(super) options: TransportOptions,
    /// Where the last datagram went: the remote end of the ICE pair str0m selected.
    pub(super) peer: std::sync::Mutex<Option<SocketAddr>>,
}

impl Shared {
    pub(super) fn request_idr(&self, reason: IdrReason) {
        self.counters.idr_requests.fetch_add(1, Ordering::Relaxed);
        (self.hooks.idr)(reason);
    }

    pub(super) fn notify(&self) {
        if let Err(e) = self.wake.set() {
            tracing::error!("webrtc: waking the media thread failed: {e:#}");
        }
    }

    pub(super) fn drop_video(&self, reason: IdrReason) -> Submit {
        self.counters.video_dropped.fetch_add(1, Ordering::Relaxed);
        self.need_keyframe.store(true, Ordering::Release);
        self.last_idr_ms
            .store(self.epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
        self.request_idr(reason);
        Submit::Dropped
    }
}

/// The queues of a running transport, for a session that sends it media it was given itself: a
/// guest's transport, fed by the owner's ([`Followers`]). Like [`AudioSender`] it does not own
/// the transport.
#[derive(Clone)]
pub struct Feed {
    tx: Sender<Command>,
    shared: Arc<Shared>,
}

impl Feed {
    /// As [`MediaHandle::submit_video`]: never blocks, and a frame this transport cannot take is
    /// dropped here alone, with its own keyframe request.
    pub fn submit_video(&self, frame: VideoFrame) -> Submit {
        queue_video(&self.shared, &self.tx, frame)
    }

    pub fn submit_audio(&self, packet: AudioPacket) -> Submit {
        queue_audio(&self.shared, &self.tx, packet)
    }

    /// Nothing this transport holds can be decoded without a keyframe.
    fn missed_frame(&self) {
        self.shared.drop_video(IdrReason::WriteFailed);
    }
}

/// The transports that are sent whatever a session's own transport is given, after it: the
/// guests watching the owner's stream (docs/browser-guests.md). Each has its own queues, pacer
/// and keyframe state, so a slow one drops frames alone. Empty for every other session, whose
/// media then takes exactly the path it took before guests existed.
#[derive(Default)]
pub struct Followers {
    feeds: std::sync::Mutex<Arc<[(u64, Feed)]>>,
    /// `feeds`' length, read without the lock: with nobody following, a frame takes no lock.
    count: AtomicUsize,
}

impl Followers {
    pub fn add(&self, id: u64, feed: Feed) {
        let mut feeds = self.feeds.lock().unwrap();
        let mut list: Vec<(u64, Feed)> = feeds.iter().filter(|(i, _)| *i != id).cloned().collect();
        list.push((id, feed));
        *feeds = list.into();
        self.count.store(feeds.len(), Ordering::Release);
    }

    /// False when `id` was not following.
    pub fn remove(&self, id: u64) -> bool {
        let mut feeds = self.feeds.lock().unwrap();
        let before = feeds.len();
        *feeds = feeds.iter().filter(|(i, _)| *i != id).cloned().collect();
        self.count.store(feeds.len(), Ordering::Release);
        feeds.len() != before
    }

    pub fn len(&self) -> usize {
        self.count.load(Ordering::Acquire)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The followers now; a list taken under the lock and read outside it. None, without the
    /// lock, while nobody follows.
    fn snapshot(&self) -> Option<Arc<[(u64, Feed)]>> {
        if self.is_empty() {
            return None;
        }
        Some(self.feeds.lock().unwrap().clone()).filter(|feeds| !feeds.is_empty())
    }
}

pub(super) enum Command {
    Datagram {
        at: Instant,
        from: SocketAddr,
        data: Vec<u8>,
    },
    Video(VideoFrame),
    Audio(AudioPacket),
    Control {
        data: Vec<u8>,
        binary: bool,
    },
    Gamepad {
        data: Vec<u8>,
        binary: bool,
    },
    Stop,
}

/// The running transport. Dropping it stops the threads.
pub struct MediaHandle {
    pub(super) tx: Sender<Command>,
    pub(super) shared: Arc<Shared>,
    pub(super) threads: Vec<JoinHandle<()>>,
    pub(super) local: SocketAddr,
    pub(super) audio_layout: Option<OpusLayout>,
    pub(super) codec: VideoCodec,
    pub(super) followers: Arc<Followers>,
}

/// The audio side of a [`MediaHandle`], for the thread that encodes the host's sound. It does not
/// own the transport: closing the handle stops the threads, and a packet sent after that is
/// dropped.
#[derive(Clone)]
pub struct AudioSender {
    tx: Sender<Command>,
    shared: Arc<Shared>,
    layout: OpusLayout,
    followers: Arc<Followers>,
}

impl AudioSender {
    /// The multistream layout the encoder must use: the one the client answered with.
    pub fn layout(&self) -> &OpusLayout {
        &self.layout
    }

    /// Never blocks; a full queue drops the packet (Opus conceals one).
    pub fn submit(&self, packet: AudioPacket) -> Submit {
        queue_audio_followed(&self.shared, &self.tx, &self.followers, packet)
    }
}

impl WebRtcAudioSink for AudioSender {
    fn layout(&self) -> &OpusLayout {
        AudioSender::layout(self)
    }

    fn submit(&self, packet: AudioPacket) -> Submit {
        AudioSender::submit(self, packet)
    }
}

/// The packet for this transport first, then a copy for each follower; an Opus packet is a few
/// hundred bytes.
fn queue_audio_followed(
    shared: &Shared,
    tx: &Sender<Command>,
    followers: &Followers,
    packet: AudioPacket,
) -> Submit {
    let Some(followers) = followers.snapshot() else {
        return queue_audio(shared, tx, packet);
    };
    let AudioPacket {
        data,
        capture,
        position,
    } = packet;
    let copy = |data: Vec<u8>| AudioPacket {
        data,
        capture,
        position,
    };
    for (_, feed) in followers.iter() {
        feed.submit_audio(copy(data.clone()));
    }
    queue_audio(shared, tx, copy(data))
}

/// Never blocks. A full queue drops the frame and requests an IDR, and until a keyframe goes
/// through nothing else is queued: it would reference the dropped frame.
fn queue_video(shared: &Shared, tx: &Sender<Command>, frame: VideoFrame) -> Submit {
    if shared.need_keyframe.load(Ordering::Acquire) && !frame.keyframe {
        shared
            .counters
            .video_dropped
            .fetch_add(1, Ordering::Relaxed);
        let now_ms = shared.epoch.elapsed().as_millis() as u64;
        let last = shared.last_idr_ms.load(Ordering::Relaxed);
        if now_ms.saturating_sub(last) >= IDR_REQUEST_INTERVAL.as_millis() as u64
            && shared
                .last_idr_ms
                .compare_exchange(last, now_ms, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            shared.request_idr(IdrReason::WaitingForKeyframe);
        }
        return Submit::Dropped;
    }
    if shared.pending_video.fetch_add(1, Ordering::AcqRel) >= shared.options.video_queue {
        shared.pending_video.fetch_sub(1, Ordering::AcqRel);
        return shared.drop_video(IdrReason::QueueFull);
    }
    // Cleared before the send: the media thread may drop this very keyframe at once and set
    // the flag again, and that must win over this store.
    if frame.keyframe {
        shared.need_keyframe.store(false, Ordering::Release);
    }
    if tx.send(Command::Video(frame)).is_err() {
        shared.pending_video.fetch_sub(1, Ordering::AcqRel);
        shared.need_keyframe.store(true, Ordering::Release);
        shared
            .counters
            .video_dropped
            .fetch_add(1, Ordering::Relaxed);
        return Submit::Dropped;
    }
    shared.notify();
    Submit::Queued
}

/// The `control` channel of a [`MediaHandle`], for the input worker. Like [`AudioSender`] it does
/// not own the transport: a message sent after the handle closed is counted in `channel_dropped`
/// while the transport's state is still around. It holds that state weakly: the state owns the
/// transport's hooks, and a guest's hooks own its seat in the input worker, which keeps this
/// sender, so a strong one would keep the guest (and its pads) plugged in after it left.
#[derive(Clone)]
pub struct ControlSender {
    tx: Sender<Command>,
    shared: Weak<Shared>,
}

impl ControlSender {
    /// Never blocks. False when the transport is gone.
    pub fn send(&self, data: Vec<u8>, binary: bool) -> bool {
        let shared = self.shared.upgrade();
        if self.tx.send(Command::Control { data, binary }).is_err() {
            if let Some(shared) = shared {
                shared
                    .counters
                    .channel_dropped
                    .fetch_add(1, Ordering::Relaxed);
            }
            return false;
        }
        if let Some(shared) = shared {
            shared.notify();
        }
        true
    }
}

fn queue_audio(shared: &Shared, tx: &Sender<Command>, packet: AudioPacket) -> Submit {
    if shared.pending_audio.fetch_add(1, Ordering::AcqRel) >= shared.options.audio_queue
        || tx.send(Command::Audio(packet)).is_err()
    {
        shared.pending_audio.fetch_sub(1, Ordering::AcqRel);
        shared
            .counters
            .audio_dropped
            .fetch_add(1, Ordering::Relaxed);
        return Submit::Dropped;
    }
    shared.notify();
    Submit::Queued
}

impl MediaHandle {
    /// Start the media thread for an accepted answer. `local` is the advertised host candidate.
    pub fn spawn(
        socket: UdpSocket,
        local: SocketAddr,
        session: Session<'_>,
        options: TransportOptions,
        hooks: Hooks,
    ) -> Result<Self> {
        let Session {
            rtc,
            ids,
            spec,
            authenticated,
        } = session;
        let epoch = Instant::now();
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            wake: Signal::new().context("creating the media thread's wake event")?,
            authenticated: authenticated.clone(),
            pending_video: AtomicUsize::new(0),
            pending_audio: AtomicUsize::new(0),
            pending_datagrams: AtomicUsize::new(0),
            need_keyframe: AtomicBool::new(true),
            last_idr_ms: AtomicU64::new(0),
            epoch,
            counters: Counters::default(),
            hooks,
            options,
            peer: std::sync::Mutex::new(None),
        });
        let (tx, rx) = mpsc::channel::<Command>();
        socket
            .set_read_timeout(Some(Duration::from_millis(100)))
            .context("setting the receive timeout")?;
        let recv_socket = socket.try_clone().context("cloning the media socket")?;
        let engine = Engine::new(rtc, ids, spec, local, epoch, authenticated);
        let queue = SendQueue::new(
            options.pacing,
            spec.video.bitrate_kbps,
            Duration::from_secs_f64(1. / f64::from(spec.video.fps.max(1))),
            local.is_ipv6(),
            Instant::now(),
        )
        .with_loss(options.loss_permille);
        let (shared_rx, tx_rx) = (shared.clone(), tx.clone());
        let shared_media = shared.clone();
        let threads = start_threads(
            &shared,
            || {
                std::thread::Builder::new()
                    .name("webrtc-recv".into())
                    .spawn(move || receive_loop(recv_socket, tx_rx, shared_rx))
            },
            || {
                std::thread::Builder::new()
                    .name("webrtc-media".into())
                    .spawn(move || media_loop(engine, queue, socket, rx, shared_media))
            },
        )?;
        Ok(Self {
            tx,
            shared,
            threads,
            local,
            audio_layout: None,
            codec: spec.video.codec,
            followers: Arc::default(),
        })
    }

    /// The transports that get this one's media after it (the guests of the owner's stream).
    pub fn followers(&self) -> Arc<Followers> {
        self.followers.clone()
    }

    /// This transport's queues, for a session that follows another ([`Followers::add`]).
    pub fn feed(&self) -> Feed {
        Feed {
            tx: self.tx.clone(),
            shared: self.shared.clone(),
        }
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local
    }

    /// The client's address on the selected ICE pair, once media has gone out to it.
    pub fn peer(&self) -> Option<SocketAddr> {
        *self.shared.peer.lock().unwrap()
    }

    /// Whether both threads are still running. A session that ended (or was closed) is not.
    pub fn is_running(&self) -> bool {
        !self.threads.is_empty() && self.threads.iter().all(|t| !t.is_finished())
    }

    /// Hand a command to the media thread and wake it. False when the thread is gone.
    fn queue(&self, command: Command) -> bool {
        if self.tx.send(command).is_err() {
            return false;
        }
        self.shared.notify();
        true
    }

    /// Never blocks. A full queue drops the frame and requests an IDR, and until a keyframe goes
    /// through nothing else is queued: it would reference the dropped frame.
    ///
    /// With followers the frame is queued here first, then for each of them from the same
    /// buffer; what one of them cannot take is dropped for it alone.
    pub fn submit_video(&self, frame: VideoFrame) -> Submit {
        let Some(followers) = self.followers.snapshot() else {
            return queue_video(&self.shared, &self.tx, frame);
        };
        let VideoFrame {
            data,
            capture,
            keyframe,
        } = frame;
        let data = match data.share(self.codec) {
            Ok(data) => data,
            Err(e) => {
                // What the media thread does with a frame it cannot write, for every peer.
                tracing::error!("webrtc: {e:#}");
                for (_, feed) in followers.iter() {
                    feed.missed_frame();
                }
                return self.shared.drop_video(IdrReason::WriteFailed);
            }
        };
        let shared = |data: &Arc<[u8]>| VideoFrame {
            data: FrameData::Shared(data.clone()),
            capture,
            keyframe,
        };
        let submitted = queue_video(&self.shared, &self.tx, shared(&data));
        for (_, feed) in followers.iter() {
            feed.submit_video(shared(&data));
        }
        submitted
    }

    /// Never blocks; a full queue drops the packet (Opus conceals one).
    pub fn submit_audio(&self, packet: AudioPacket) -> Submit {
        queue_audio_followed(&self.shared, &self.tx, &self.followers, packet)
    }

    /// Record the Opus layout the answer agreed to, which [`AudioSender::layout`] hands the audio
    /// thread's encoder.
    pub fn with_audio_layout(mut self, layout: OpusLayout) -> Self {
        self.audio_layout = Some(layout);
        self
    }

    /// The audio end of the transport for the audio thread, which starts before the video worker
    /// takes this handle. `None` until the layout is recorded.
    pub fn audio_sender(&self) -> Option<AudioSender> {
        Some(AudioSender {
            tx: self.tx.clone(),
            shared: self.shared.clone(),
            layout: self.audio_layout.clone()?,
            followers: self.followers.clone(),
        })
    }

    pub fn send_control(&self, data: Vec<u8>, binary: bool) {
        self.send_channel(Command::Control { data, binary });
    }

    pub fn send_gamepad(&self, data: Vec<u8>, binary: bool) {
        self.send_channel(Command::Gamepad { data, binary });
    }

    fn send_channel(&self, command: Command) {
        if !self.queue(command) {
            self.shared
                .counters
                .channel_dropped
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The control-channel end of the transport for the input worker (rumble, mouse-mode), which
    /// does not own the handle.
    pub fn control_sender(&self) -> ControlSender {
        ControlSender {
            tx: self.tx.clone(),
            shared: Arc::downgrade(&self.shared),
        }
    }

    /// Whether the client may open the `input` channel. Set it once the session is authenticated.
    pub fn set_authenticated(&self, authenticated: bool) {
        self.shared
            .authenticated
            .store(authenticated, Ordering::Release);
    }

    pub fn stats(&self) -> TransportStats {
        let c = &self.shared.counters;
        let get = |a: &AtomicU64| a.load(Ordering::Relaxed);
        TransportStats {
            video_frames: get(&c.video_frames),
            video_bytes: get(&c.video_bytes),
            video_dropped: get(&c.video_dropped),
            audio_packets: get(&c.audio_packets),
            audio_dropped: get(&c.audio_dropped),
            datagrams_sent: get(&c.datagrams_sent),
            bytes_sent: get(&c.bytes_sent),
            send_errors: get(&c.send_errors),
            idr_requests: get(&c.idr_requests),
            datagrams_received: get(&c.datagrams_received),
            datagrams_dropped: get(&c.datagrams_dropped),
            datagrams_rejected: get(&c.datagrams_rejected),
            datagram_errors: get(&c.datagram_errors),
            recv_errors: get(&c.recv_errors),
            paced_dropped: get(&c.paced_dropped),
            channel_sent: get(&c.channel_sent),
            channel_dropped: get(&c.channel_dropped),
            simulated_losses: get(&c.simulated_losses),
        }
    }

    /// The video datagrams the loss switch has thrown away so far, for the `stream timings` line;
    /// None while the switch is off.
    pub fn simulated_losses(&self) -> Option<u64> {
        (self.shared.options.loss_permille > 0).then(|| {
            self.shared
                .counters
                .simulated_losses
                .load(Ordering::Relaxed)
        })
    }

    /// Stop and join the threads. Called from one of the transport's own hooks (the event hook
    /// runs on the media thread) it leaves that thread to finish by itself: a thread can't join
    /// itself.
    pub fn close(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        let _ = self.tx.send(Command::Stop);
        self.shared.notify();
        let current = std::thread::current().id();
        for thread in self.threads.drain(..) {
            if thread.thread().id() != current {
                let _ = thread.join();
            }
        }
    }
}

impl Drop for MediaHandle {
    fn drop(&mut self) {
        self.close();
    }
}
