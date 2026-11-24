//! The PipeWire side of loopback capture: one thread runs a PipeWire loop with a capture stream
//! on a sink's monitor and pushes what it receives into the shared queue.

use super::queue::{self, Queue, RATE};
use anyhow::{Context, Result, anyhow, bail};
use pipewire as pw;
use pw::{properties::properties, spa};
use std::{
    cell::Cell,
    rc::Rc,
    sync::{
        Arc, Condvar, Mutex, MutexGuard, PoisonError,
        mpsc::{self, RecvTimeoutError},
    },
    thread::JoinHandle,
    time::Duration,
};

/// How long opening may take before the daemon counts as unresponsive.
const OPEN_TIMEOUT: Duration = Duration::from_secs(5);

/// The queue and the condvar that wakes the host's audio thread.
pub(super) struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
}
impl Shared {
    pub(super) fn lock(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }
    /// Waits up to `timeout` for audio or a failure; what arrived before the call counts.
    pub(super) fn wait(&self, timeout: Duration) {
        let guard = self.lock();
        let (mut guard, _) = self
            .ready
            .wait_timeout_while(guard, timeout, |queue| {
                !queue.notify && queue.error.is_none()
            })
            .unwrap_or_else(PoisonError::into_inner);
        guard.notify = false;
    }
    /// Ends the capture with `reason`, waking the reader; the first reason stays.
    fn fail(&self, reason: String) {
        let mut queue = self.lock();
        queue.error.get_or_insert(reason);
        self.ready.notify_all();
    }
}

/// A running PipeWire capture; dropping it stops the thread.
pub(super) struct Capture {
    pub(super) shared: Arc<Shared>,
    quit: pw::channel::Sender<()>,
    thread: Option<JoinHandle<()>>,
}
impl Capture {
    /// Captures `sink`'s monitor, or the default sink's when `sink` is empty, as `channels`
    /// channels of 48 kHz `f32`. Fails when the daemon is not there or the sink is unknown.
    pub(super) fn open(channels: usize, sink: &str) -> Result<Self> {
        let Some(positions) = queue::positions(channels) else {
            bail!("no audio channel layout for {channels} channels");
        };
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue::new(channels)),
            ready: Condvar::new(),
        });
        let (quit, quit_rx) = pw::channel::channel::<()>();
        let (opened_tx, opened_rx) = mpsc::channel::<Result<(), String>>();
        let thread = {
            let shared = shared.clone();
            let sink = sink.to_owned();
            std::thread::Builder::new()
                .name("pipewire-capture".into())
                .spawn(move || {
                    let opened = opened_tx.clone();
                    if let Err(error) =
                        run(&shared, channels, positions, &sink, quit_rx, &opened_tx)
                    {
                        // Before the stream ran this is the open's error, else the reader's.
                        let text = format!("{error:#}");
                        let _ = opened.send(Err(text.clone()));
                        shared.fail(text);
                    }
                })
                .context("starting the PipeWire thread")?
        };
        let capture = Self {
            shared,
            quit,
            thread: Some(thread),
        };
        match opened_rx.recv_timeout(OPEN_TIMEOUT) {
            Ok(Ok(())) => Ok(capture),
            Ok(Err(error)) => Err(anyhow!(error)),
            Err(RecvTimeoutError::Timeout) => {
                bail!("PipeWire did not answer within {OPEN_TIMEOUT:?}")
            }
            Err(RecvTimeoutError::Disconnected) => bail!("the PipeWire thread ended early"),
        }
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.quit.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// What the stream's callbacks share.
struct StreamData {
    shared: Arc<Shared>,
    opened: mpsc::Sender<Result<(), String>>,
    /// Whether the stream reached the graph once.
    open: bool,
}

/// The capture thread: connects, reports the open's result through `opened`, then runs until
/// asked to quit or until the daemon or the stream fails (reported to the reader).
fn run(
    shared: &Arc<Shared>,
    channels: usize,
    positions: &[u32],
    sink: &str,
    quit: pw::channel::Receiver<()>,
    opened: &mpsc::Sender<Result<(), String>>,
) -> Result<()> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).context("creating the PipeWire loop")?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context
        .connect_rc(None)
        .context("no PipeWire daemon to connect to")?;
    let _quit = quit.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        move |()| mainloop.quit()
    });
    let _core = core
        .add_listener_local()
        .error({
            let shared = shared.clone();
            let mainloop = mainloop.clone();
            move |id, _seq, res, message| {
                // Errors on the core itself (id 0) are the connection's: the daemon went away.
                if id == 0 {
                    shared.fail(format!("PipeWire connection lost: {message} ({res})"));
                    mainloop.quit();
                }
            }
        })
        .register();

    if !sink.is_empty() && !sink_exists(&mainloop, &core, sink)? {
        bail!("no audio sink named {sink:?}");
    }
    let mut props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Music",
        *pw::keys::NODE_NAME => "rubylight-capture",
        *pw::keys::NODE_DESCRIPTION => "Rubylight loopback capture",
        *pw::keys::STREAM_CAPTURE_SINK => "true",
        // About 5 ms per process call: the host's packets are 5 ms.
        *pw::keys::NODE_LATENCY => "256/48000",
    };
    if !sink.is_empty() {
        props.insert("target.object", sink);
        // An unknown sink must fail, not fall back to the default one.
        props.insert("node.dont-fallback", "true");
        props.insert("node.dont-reconnect", "true");
    }
    let stream = pw::stream::StreamBox::new(&core, "rubylight-capture", props)?;
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
                    Error(message) => {
                        stop(
                            data,
                            &mainloop,
                            format!("PipeWire stream failed: {message}"),
                        );
                    }
                    Unconnected if data.open => {
                        stop(data, &mainloop, "PipeWire closed the capture".into());
                    }
                    _ => {}
                }
            }
        })
        .process(|stream, data| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let Some(block) = buffer.datas_mut().first_mut() else {
                return;
            };
            let chunk = block.chunk();
            let (offset, size) = (chunk.offset() as usize, chunk.size() as usize);
            let Some(bytes) = block.data() else {
                return;
            };
            let end = (offset + size).min(bytes.len());
            let bytes = bytes.get(offset..end).unwrap_or_default();
            let mut queue = data.shared.lock();
            queue.push(
                bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| f32::from_le_bytes(*b)),
            );
            drop(queue);
            data.shared.ready.notify_all();
        })
        .register()?;

    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(RATE);
    info.set_channels(channels as u32);
    let mut position = [0; spa::sys::SPA_AUDIO_MAX_CHANNELS as usize];
    position[..positions.len()].copy_from_slice(positions);
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
    stream
        .connect(
            spa::utils::Direction::Input,
            None,
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::RT_PROCESS,
            &mut params,
        )
        .context("connecting the capture stream")?;
    mainloop.run();
    Ok(())
}

/// Ends the capture: the open's error before the stream was up, the reader's after.
fn stop(data: &mut StreamData, mainloop: &pw::main_loop::MainLoopRc, reason: String) {
    if !data.open {
        let _ = data.opened.send(Err(reason.clone()));
    }
    data.shared.fail(reason);
    mainloop.quit();
}

/// Whether the daemon has an output node whose name, object serial or id is `sink`.
fn sink_exists(
    mainloop: &pw::main_loop::MainLoopRc,
    core: &pw::core::CoreRc,
    sink: &str,
) -> Result<bool> {
    let registry = core.get_registry().context("reading PipeWire's registry")?;
    let found = Rc::new(Cell::new(false));
    let done = Rc::new(Cell::new(false));
    let pending = core.sync(0).context("syncing with PipeWire")?;
    let _core = core
        .add_listener_local()
        .done({
            let (done, mainloop) = (done.clone(), mainloop.clone());
            move |id, seq| {
                if id == pw::core::PW_ID_CORE && seq == pending {
                    done.set(true);
                    mainloop.quit();
                }
            }
        })
        .register();
    let _registry = registry
        .add_listener_local()
        .global({
            let found = found.clone();
            let sink = sink.to_owned();
            move |global| {
                let Some(props) = global.props else {
                    return;
                };
                let output = props
                    .get("media.class")
                    .is_some_and(|class| class.ends_with("/Sink"));
                let named = [props.get("node.name"), props.get("object.serial")]
                    .contains(&Some(sink.as_str()))
                    || global.id.to_string() == sink;
                if output && named {
                    found.set(true);
                }
            }
        })
        .register();
    mainloop.run();
    if !done.get() {
        bail!("PipeWire went away while listing sinks");
    }
    Ok(found.get())
}
