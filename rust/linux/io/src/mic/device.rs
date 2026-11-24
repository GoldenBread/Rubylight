//! The virtual microphone's PipeWire side: one thread runs a loop with an output stream whose
//! `media.class` is `Audio/Source`, so the daemon lists it as a recording device named
//! "Rubylight Microphone". Apps record from it; the host's render thread fills a ring that the
//! stream's process callback drains, one graph quantum at a time.

use anyhow::{Context, Result, anyhow, bail};
use pipewire as pw;
use pw::{properties::properties, spa};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU32, AtomicUsize, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    thread::JoinHandle,
    time::Duration,
};

/// The node's `node.name`, which `endpoints()` reports as the device's id and the default
/// source metadata names.
pub const NODE_NAME: &str = "rubylight-microphone";
/// What apps list it as.
pub const DESCRIPTION: &str = "Rubylight Microphone";
/// Samples per second; the client's microphone is 48 kHz mono.
pub const RATE: u32 = 48_000;
/// How long opening may take before the daemon counts as unresponsive.
const OPEN_TIMEOUT: Duration = Duration::from_secs(5);
/// The most the ring holds, in frames (about 170 ms).
pub const RING_CAP: usize = 8192;
/// The quantum assumed until the graph runs the node, in frames (10 ms).
const FIRST_QUANTUM: usize = 480;

/// What the render thread and the stream's callbacks share.
pub struct Shared {
    /// Samples written by the render thread and not yet played.
    ring: Mutex<VecDeque<f32>>,
    /// Set by each process call; the render thread waits on it.
    woken: Mutex<bool>,
    wake: Condvar,
    /// Frames the graph asked for in the last process call.
    quantum: AtomicUsize,
    /// The node's volume and channel volume as apps last set them, as `f32` bits.
    volume: AtomicU32,
    channel_volume: AtomicU32,
    /// Why the node ended, once it did.
    error: Mutex<Option<String>>,
}
impl Shared {
    fn new() -> Self {
        Self {
            ring: Mutex::new(VecDeque::new()),
            woken: Mutex::new(false),
            wake: Condvar::new(),
            quantum: AtomicUsize::new(FIRST_QUANTUM),
            volume: AtomicU32::new(1f32.to_bits()),
            channel_volume: AtomicU32::new(1f32.to_bits()),
            error: Mutex::new(None),
        }
    }
    pub fn ring(&self) -> MutexGuard<'_, VecDeque<f32>> {
        self.ring.lock().unwrap_or_else(PoisonError::into_inner)
    }
    /// Frames the graph takes per cycle, as last seen.
    pub fn quantum(&self) -> usize {
        self.quantum.load(Ordering::Relaxed)
    }
    /// Waits up to `timeout` for the graph to take audio; a wake-up before the call counts.
    pub fn wait(&self, timeout: Duration) {
        let guard = self.woken.lock().unwrap_or_else(PoisonError::into_inner);
        let (mut guard, _) = self
            .wake
            .wait_timeout_while(guard, timeout, |woken| !*woken)
            .unwrap_or_else(PoisonError::into_inner);
        *guard = false;
    }
    fn notify(&self) {
        *self.woken.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.wake.notify_all();
    }
    /// Why the node ended, when it did.
    pub fn failure(&self) -> Option<String> {
        self.error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
    fn fail(&self, reason: String) {
        self.error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert(reason);
        self.notify();
    }
    /// The largest of the node's volume and channel volumes, linear.
    pub fn level(&self) -> f32 {
        let load = |bits: &AtomicU32| f32::from_bits(bits.load(Ordering::Relaxed));
        load(&self.volume).max(load(&self.channel_volume))
    }
}

/// Moves up to `want` frames from `ring` into `out` (little-endian `f32` bytes), padding with
/// silence; returns the frames of real audio.
pub fn drain(ring: &mut VecDeque<f32>, out: &mut [u8], want: usize) -> usize {
    let frames = want.min(out.len() / 4);
    let real = frames.min(ring.len());
    for (slot, sample) in out
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(ring.drain(..real))
    {
        slot.copy_from_slice(&sample.to_le_bytes());
    }
    out[real * 4..frames * 4].fill(0);
    real
}

enum Command {
    Quit,
    /// Bring volumes above unity back to it.
    Unity,
}

/// The running source node; dropping it removes the node.
pub struct Device {
    pub shared: Arc<Shared>,
    commands: pw::channel::Sender<Command>,
    thread: Option<JoinHandle<()>>,
}
impl Device {
    /// Creates the node and waits until the daemon has it. Fails when no daemon runs.
    pub fn open() -> Result<Self> {
        let shared = Arc::new(Shared::new());
        let (commands, commands_rx) = pw::channel::channel::<Command>();
        let (opened_tx, opened_rx) = mpsc::channel::<Result<(), String>>();
        let thread = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("pipewire-microphone".into())
                .spawn(move || {
                    if let Err(error) = run(&shared, commands_rx, &opened_tx) {
                        let text = format!("{error:#}");
                        let _ = opened_tx.send(Err(text.clone()));
                        shared.fail(text);
                    }
                })
                .context("starting the PipeWire thread")?
        };
        let device = Self {
            shared,
            commands,
            thread: Some(thread),
        };
        match opened_rx.recv_timeout(OPEN_TIMEOUT) {
            Ok(Ok(())) => Ok(device),
            Ok(Err(error)) => Err(anyhow!(error)),
            Err(RecvTimeoutError::Timeout) => {
                bail!("PipeWire did not answer within {OPEN_TIMEOUT:?}")
            }
            Err(RecvTimeoutError::Disconnected) => bail!("the PipeWire thread ended early"),
        }
    }
    /// Whether the node still runs.
    pub fn alive(&self) -> bool {
        self.shared.failure().is_none()
    }
    /// Asks the stream to bring its volumes back to unity where they are above it.
    pub fn limit_volume(&self) -> Result<()> {
        self.commands
            .send(Command::Unity)
            .map_err(|_| anyhow!("the microphone node is gone"))
    }
}
impl Drop for Device {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct StreamData {
    shared: Arc<Shared>,
    opened: mpsc::Sender<Result<(), String>>,
    open: bool,
}

fn run(
    shared: &Arc<Shared>,
    commands: pw::channel::Receiver<Command>,
    opened: &mpsc::Sender<Result<(), String>>,
) -> Result<()> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).context("creating the PipeWire loop")?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context
        .connect_rc(None)
        .context("no PipeWire daemon to connect to")?;
    let _core = core
        .add_listener_local()
        .error({
            let shared = shared.clone();
            let mainloop = mainloop.clone();
            move |id, _seq, res, message| {
                if id == 0 {
                    shared.fail(format!("PipeWire connection lost: {message} ({res})"));
                    mainloop.quit();
                }
            }
        })
        .register();
    let props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CLASS => "Audio/Source",
        *pw::keys::NODE_NAME => NODE_NAME,
        *pw::keys::NODE_NICK => DESCRIPTION,
        *pw::keys::NODE_DESCRIPTION => DESCRIPTION,
        *pw::keys::NODE_VIRTUAL => "true",
        *pw::keys::AUDIO_CHANNELS => "1",
        "audio.position" => "MONO",
        // A 10 ms quantum if the graph lets it; apps that record ask for their own.
        *pw::keys::NODE_LATENCY => "480/48000",
    };
    let stream = pw::stream::StreamRc::new(core.clone(), NODE_NAME, props)?;
    let data = StreamData {
        shared: shared.clone(),
        opened: opened.clone(),
        open: false,
    };
    let _listener = stream
        .add_local_listener_with_user_data(data)
        .state_changed({
            let mainloop = mainloop.clone();
            move |_, data, _old, new| {
                use pw::stream::StreamState::*;
                match new {
                    Paused | Streaming if !data.open => {
                        data.open = true;
                        let _ = data.opened.send(Ok(()));
                    }
                    Error(message) => stop(
                        data,
                        &mainloop,
                        format!("PipeWire stream failed: {message}"),
                    ),
                    Unconnected if data.open => {
                        stop(data, &mainloop, "PipeWire closed the microphone".into())
                    }
                    _ => {}
                }
            }
        })
        .control_info(|_, data, id, control| {
            // SAFETY: PipeWire passes a live control (or null) for the length of the callback.
            let Some(control) = (unsafe { control.as_ref() }) else {
                return;
            };
            if control.values.is_null() || control.n_values == 0 {
                return;
            }
            // SAFETY: `values` points at `n_values` floats.
            let values =
                unsafe { std::slice::from_raw_parts(control.values, control.n_values as usize) };
            let top = values.iter().copied().fold(0f32, f32::max);
            if id == spa::sys::SPA_PROP_volume {
                data.shared.volume.store(top.to_bits(), Ordering::Relaxed);
            } else if id == spa::sys::SPA_PROP_channelVolumes {
                data.shared
                    .channel_volume
                    .store(top.to_bits(), Ordering::Relaxed);
            }
        })
        .process(|stream, data| {
            // The raw buffer, for `requested`, the frames this cycle needs: the crate's safe
            // `Buffer::requested` is behind a feature this crate does not turn on.
            // SAFETY: the buffer is queued back before the callback returns, and only its
            // first data block, which MAP_BUFFERS maps, is touched.
            unsafe {
                let raw = stream.dequeue_raw_buffer();
                if raw.is_null() {
                    return;
                }
                let spa = (*raw).buffer;
                if !spa.is_null() && (*spa).n_datas > 0 {
                    let block = &mut *(*spa).datas;
                    if !block.data.is_null() {
                        let capacity = block.maxsize as usize / 4;
                        let requested = (*raw).requested as usize;
                        let frames = if requested > 0 {
                            requested.min(capacity)
                        } else {
                            capacity
                        };
                        let bytes =
                            std::slice::from_raw_parts_mut(block.data.cast::<u8>(), frames * 4);
                        match data.shared.ring.try_lock() {
                            Ok(mut ring) => {
                                drain(&mut ring, bytes, frames);
                            }
                            Err(_) => bytes.fill(0),
                        }
                        data.shared.quantum.store(frames, Ordering::Relaxed);
                        let chunk = &mut *block.chunk;
                        chunk.offset = 0;
                        chunk.stride = 4;
                        chunk.size = (frames * 4) as u32;
                    }
                }
                stream.queue_raw_buffer(raw);
            }
            data.shared.notify();
        })
        .register()?;
    let _commands = commands.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        let stream = stream.clone();
        let shared = shared.clone();
        move |command| match command {
            Command::Quit => mainloop.quit(),
            Command::Unity => {
                let above = |bits: &AtomicU32| f32::from_bits(bits.load(Ordering::Relaxed)) > 1.;
                if above(&shared.volume) {
                    let _ = stream.set_control(spa::sys::SPA_PROP_volume, &[1.]);
                }
                if above(&shared.channel_volume) {
                    let _ = stream.set_control(spa::sys::SPA_PROP_channelVolumes, &[1.]);
                }
            }
        }
    });

    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(RATE);
    info.set_channels(1);
    let mut position = [0; spa::sys::SPA_AUDIO_MAX_CHANNELS as usize];
    position[0] = spa::sys::SPA_AUDIO_CHANNEL_MONO;
    info.set_position(position);
    let object = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    };
    let bytes = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    )
    .map_err(|error| anyhow!("building the audio format: {error:?}"))?
    .0
    .into_inner();
    let mut params = [spa::pod::Pod::from_bytes(&bytes).context("the audio format pod")?];
    // No AUTOCONNECT: this is a source for apps to link to, not a stream to place.
    stream
        .connect(
            spa::utils::Direction::Output,
            None,
            pw::stream::StreamFlags::MAP_BUFFERS | pw::stream::StreamFlags::RT_PROCESS,
            &mut params,
        )
        .context("connecting the microphone stream")?;
    mainloop.run();
    Ok(())
}

fn stop(data: &mut StreamData, mainloop: &pw::main_loop::MainLoopRc, reason: String) {
    if !data.open {
        let _ = data.opened.send(Err(reason.clone()));
    }
    data.shared.fail(reason);
    mainloop.quit();
}
