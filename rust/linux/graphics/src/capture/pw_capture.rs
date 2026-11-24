//! Capture of a PipeWire video node as DMA-BUFs: the picture of a gamescope session. A thread of
//! its own runs the PipeWire loop; for each frame it imports the producer's DMA-BUF into VA-API,
//! converts it into a surface of the stream's own pool by video processing and hands the buffer
//! straight back to the producer. The stream threads then take the newest converted frame from
//! [`PipeWireCapture::take`]. Ported from the spike's `pw_capture.rs`.
use crate::{
    timing::Signal,
    vaapi::{
        BGRA, BGRX, NV12, Plane, RGBA, RGBX, Va,
        ffi::{VA_RT_FORMAT_YUV420, VASurfaceID},
        pool::{Pool, Pooled},
        vpp::{Converter, Source},
    },
};
use anyhow::{Context, Result, anyhow, bail};
use pipewire as pw;
use pw::spa::{
    self,
    buffer::DataType,
    param::{
        ParamType,
        format::{FormatProperties, MediaSubtype, MediaType},
        video::{VideoFormat, VideoInfoRaw},
    },
    pod::{ChoiceValue, Object, Pod, Property, PropertyFlags, Value, serialize::PodSerializer},
    utils::{Choice, ChoiceEnum, ChoiceFlags, Id, SpaTypes},
};
use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// `DRM_FORMAT_MOD_LINEAR` and `DRM_FORMAT_MOD_INVALID` (the producer's own layout, undeclared).
const MOD_LINEAR: i64 = 0;
const MOD_INVALID: i64 = 0x00ff_ffff_ffff_ffff;
/// How long opening waits for the producer's first format.
const OPEN_TIMEOUT: Duration = Duration::from_secs(5);
/// Surfaces the stream's pool makes at most: one being converted, one held for the next
/// poll, and some for the encoder and the senders.
const POOL_LIMIT: usize = 8;

/// One converted frame, with the moments the host's timing reports read.
pub struct Frame {
    pub surface: Arc<Pooled>,
    pub captured: Instant,
    pub acquired: Instant,
    pub published: Instant,
}

/// What the PipeWire thread and its owner share.
struct Shared {
    latest: Mutex<Option<Frame>>,
    /// Why the capture ended, once it has.
    ended: Mutex<Option<String>>,
    signal: Signal,
    frames: AtomicU64,
    dropped: AtomicU64,
}

impl Shared {
    fn end(&self, why: impl Into<String>) {
        let mut ended = self.ended.lock().unwrap_or_else(PoisonError::into_inner);
        ended.get_or_insert_with(|| why.into());
        drop(ended);
        // Wake a waiter so it sees the end.
        let _ = self.signal.set();
    }
}

struct Quit;

/// A running capture of one node.
pub struct PipeWireCapture {
    shared: Arc<Shared>,
    quit: pw::channel::Sender<Quit>,
    thread: Option<JoinHandle<()>>,
    /// The picture's size, as the producer negotiated it.
    pub size: (u32, u32),
}

impl PipeWireCapture {
    /// Starts capturing `node` and waits until the producer has said what its picture is.
    pub fn open(node: u32, va: &Arc<Va>) -> Result<Self> {
        let shared = Arc::new(Shared {
            latest: Mutex::new(None),
            ended: Mutex::new(None),
            signal: Signal::new()?,
            frames: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
        });
        let (quit, quit_receiver) = pw::channel::channel::<Quit>();
        let (ready, ready_receiver) = mpsc::channel::<Result<(u32, u32), String>>();
        let thread = std::thread::Builder::new()
            .name("pipewire-capture".into())
            .spawn({
                let (shared, va) = (shared.clone(), va.clone());
                move || {
                    if let Err(error) = run(node, va, &shared, &ready, quit_receiver) {
                        let _ = ready.send(Err(format!("{error:#}")));
                        shared.end(format!("{error:#}"));
                    }
                }
            })
            .context("starting the capture thread")?;
        let mut capture = Self {
            shared,
            quit,
            thread: Some(thread),
            size: (0, 0),
        };
        capture.size = match ready_receiver.recv_timeout(OPEN_TIMEOUT) {
            Ok(Ok(size)) => size,
            Ok(Err(error)) => bail!("capturing PipeWire node {node}: {error}"),
            Err(_) => bail!(
                "PipeWire node {node} did not offer a picture within {OPEN_TIMEOUT:?}; is the game's display running?"
            ),
        };
        Ok(capture)
    }

    pub fn signal(&self) -> &Signal {
        &self.shared.signal
    }

    /// The newest converted frame, if one arrived since the last call. An error once the
    /// capture has ended and its last frame was taken.
    pub fn take(&self) -> Result<Option<Frame>> {
        let frame = self
            .shared
            .latest
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if frame.is_none()
            && let Some(why) = self
                .shared
                .ended
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        {
            bail!("the game's display closed: {why}");
        }
        Ok(frame)
    }

    /// Frames received and frames dropped for want of a free surface since the capture opened.
    pub fn counts(&self) -> (u64, u64) {
        (
            self.shared.frames.load(Ordering::Relaxed),
            self.shared.dropped.load(Ordering::Relaxed),
        )
    }
}

impl Drop for PipeWireCapture {
    fn drop(&mut self) {
        let (frames, dropped) = self.counts();
        tracing::debug!(frames, dropped, "PipeWire capture closed");
        let _ = self.quit.send(Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// What the producer's format says, and what is made from it.
struct Negotiated {
    pixel: u32,
    width: u32,
    height: u32,
    modifier: u64,
    pool: Arc<Pool>,
    converter: Converter,
}

/// The PipeWire thread's own state.
struct Local {
    va: Arc<Va>,
    shared: Arc<Shared>,
    ready: mpsc::Sender<Result<(u32, u32), String>>,
    quit: pw::main_loop::MainLoopRc,
    format: Option<Negotiated>,
    /// One surface per buffer of the producer's pool, by the buffer's inode.
    imported: HashMap<u64, VASurfaceID>,
}

impl Local {
    fn release(&mut self) {
        release(&self.va, &mut self.imported);
    }

    fn fail(&mut self, error: anyhow::Error) {
        let text = format!("{error:#}");
        let _ = self.ready.send(Err(text.clone()));
        self.shared.end(text);
        self.quit.quit();
    }

    fn format_changed(&mut self, stream: &pw::stream::Stream, param: &Pod) -> Result<()> {
        let mut info = VideoInfoRaw::new();
        info.parse(param)
            .map_err(|e| anyhow!("unreadable video format: {e:?}"))?;
        let pixel = match info.format() {
            VideoFormat::NV12 => NV12,
            VideoFormat::BGRx => BGRX,
            VideoFormat::BGRA => BGRA,
            VideoFormat::RGBx => RGBX,
            VideoFormat::RGBA => RGBA,
            other => bail!("the node chose {other:?}, which the capture does not take"),
        };
        let size = info.size();
        let (width, height) = (size.width, size.height);
        if let Some(old) = &self.format
            && (old.width, old.height) != (width, height)
        {
            bail!(
                "the picture changed size from {}x{} to {width}x{height}",
                old.width,
                old.height
            );
        }
        tracing::info!(
            format = ?info.format(),
            width,
            height,
            modifier = format_args!("0x{:x}", info.modifier()),
            "PipeWire capture format"
        );
        self.release();
        self.format = Some(Negotiated {
            pixel,
            width,
            height,
            modifier: info.modifier(),
            pool: Pool::new(
                &self.va,
                VA_RT_FORMAT_YUV420,
                NV12,
                width,
                height,
                3,
                POOL_LIMIT,
            )?,
            converter: Converter::new(&self.va, width, height)?,
        });
        // Ask for DMA-BUF buffers: a copy through system memory is not the capture this is.
        let buffers = serialize(Object {
            type_: SpaTypes::ObjectParamBuffers.as_raw(),
            id: ParamType::Buffers.as_raw(),
            properties: vec![property(
                spa::sys::SPA_PARAM_BUFFERS_dataType,
                Value::Int(1 << spa::sys::SPA_DATA_DmaBuf),
            )],
        })?;
        let mut params = [Pod::from_bytes(&buffers).context("buffers parameter")?];
        stream.update_params(&mut params)?;
        let _ = self.ready.send(Ok((width, height)));
        Ok(())
    }

    fn frame(&mut self, buffer: &mut pw::buffer::Buffer, arrived: Instant) -> Result<()> {
        let Self {
            va,
            shared,
            format,
            imported,
            ..
        } = self;
        let Some(format) = format else {
            bail!("a frame arrived before its format");
        };
        let mut planes = Vec::with_capacity(2);
        for data in buffer.datas_mut() {
            if data.type_() != DataType::DmaBuf {
                bail!(
                    "the node sent {:?} buffers, not DMA-BUFs: the picture would be copied",
                    data.type_()
                );
            }
            planes.push(Plane {
                fd: data.fd(),
                offset: data.chunk().offset(),
                pitch: data.chunk().stride() as u32,
            });
        }
        let Some(&luma) = planes.first() else {
            return Ok(());
        };
        if format.pixel == NV12 && planes.len() == 1 {
            // gamescope describes a two-plane picture by its first plane only; the chroma
            // plane follows the luma rows directly.
            planes.push(Plane {
                offset: luma.offset + luma.pitch * format.height,
                ..luma
            });
        }
        shared.frames.fetch_add(1, Ordering::Relaxed);
        let Some(output) = format.pool.take() else {
            // Every surface is held by the stream: this frame is dropped, the producer's next
            // one replaces it.
            shared.dropped.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        };
        let key = inode(luma.fd);
        let input = match imported.get(&key) {
            Some(&id) => id,
            None => {
                if imported.len() >= 16 {
                    // The producer made new buffers; the old ones are gone.
                    release(va, imported);
                }
                let id = va.import_dmabuf(
                    format.pixel,
                    format.modifier,
                    format.width,
                    format.height,
                    &planes,
                )?;
                imported.insert(key, id);
                id
            }
        };
        // The producer may still be drawing: wait for it to finish, bounded.
        let mut ready = libc::pollfd {
            fd: luma.fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one descriptor that stays open while the buffer is held.
        unsafe { libc::poll(&raw mut ready, 1, 50) };
        let source = if format.pixel == NV12 {
            Source::Video
        } else {
            Source::Rgb
        };
        let size = (format.width, format.height);
        format
            .converter
            .convert(input, source, size, output.id(), size)?;
        // The producer may draw into this buffer again as soon as it is back; wait for the
        // conversion to have read it.
        va.sync(output.id())?;
        let acquired = Instant::now();
        let frame = Frame {
            surface: Arc::new(output),
            captured: arrived,
            acquired,
            published: acquired,
        };
        *shared.latest.lock().unwrap_or_else(PoisonError::into_inner) = Some(frame);
        let _ = shared.signal.set();
        Ok(())
    }
}

/// Destroys the surfaces imported from the producer's buffers.
fn release(va: &Va, imported: &mut HashMap<u64, VASurfaceID>) {
    let mut surfaces: Vec<VASurfaceID> = imported.drain().map(|(_, id)| id).collect();
    va.destroy_surfaces(&mut surfaces);
}

/// The identity of the buffer a descriptor refers to: a descriptor's number is reused, its
/// DMA-BUF's inode is not.
fn inode(fd: i32) -> u64 {
    // SAFETY: an out structure of the right type and a descriptor the caller holds open.
    unsafe {
        let mut stat: libc::stat = std::mem::zeroed();
        if libc::fstat(fd, &raw mut stat) == 0 {
            stat.st_ino
        } else {
            fd as u64
        }
    }
}

fn property(key: u32, value: Value) -> Property {
    Property {
        key,
        flags: PropertyFlags::empty(),
        value,
    }
}

fn id_value(raw: u32) -> Value {
    Value::Id(Id(raw))
}

fn serialize(object: Object) -> Result<Vec<u8>> {
    let (cursor, _) =
        PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(object))
            .map_err(|e| anyhow!("serialize a PipeWire parameter: {e:?}"))?;
    Ok(cursor.into_inner())
}

/// One format the capture takes: DMA-BUF only (a modifier marks the offer as one, and it is
/// mandatory so that no memory-copy format can be chosen instead). A format the producer
/// converts itself is asked for in the stream's colours, BT.709 limited range.
fn offer(format: VideoFormat) -> Result<Vec<u8>> {
    let mut properties = vec![
        property(
            FormatProperties::MediaType.as_raw(),
            id_value(MediaType::Video.as_raw()),
        ),
        property(
            FormatProperties::MediaSubtype.as_raw(),
            id_value(MediaSubtype::Raw.as_raw()),
        ),
        property(
            FormatProperties::VideoFormat.as_raw(),
            id_value(format.as_raw()),
        ),
        Property {
            key: FormatProperties::VideoModifier.as_raw(),
            flags: PropertyFlags::MANDATORY,
            value: Value::Choice(ChoiceValue::Long(Choice(
                ChoiceFlags::empty(),
                ChoiceEnum::Enum {
                    default: MOD_LINEAR,
                    alternatives: vec![MOD_LINEAR, MOD_INVALID],
                },
            ))),
        },
    ];
    if format == VideoFormat::NV12 {
        properties.push(property(
            FormatProperties::VideoColorMatrix.as_raw(),
            id_value(spa::sys::SPA_VIDEO_COLOR_MATRIX_BT709),
        ));
        properties.push(property(
            FormatProperties::VideoColorRange.as_raw(),
            id_value(spa::sys::SPA_VIDEO_COLOR_RANGE_16_235),
        ));
    }
    serialize(Object {
        type_: SpaTypes::ObjectParamFormat.as_raw(),
        id: ParamType::EnumFormat.as_raw(),
        properties,
    })
}

fn run(
    node: u32,
    va: Arc<Va>,
    shared: &Arc<Shared>,
    ready: &mpsc::Sender<Result<(u32, u32), String>>,
    quit: pw::channel::Receiver<Quit>,
) -> Result<()> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_rc(None)?;
    let properties = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE => "Video",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Screen",
    };
    let stream = pw::stream::StreamBox::new(&core, "rubylight-capture", properties)?;
    let local = Rc::new(RefCell::new(Local {
        va,
        shared: shared.clone(),
        ready: ready.clone(),
        quit: mainloop.clone(),
        format: None,
        imported: HashMap::new(),
    }));
    let _quit = quit.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        move |Quit| mainloop.quit()
    });
    let listener = stream
        .add_local_listener_with_user_data(local.clone())
        .state_changed(|_, local, old, new| {
            tracing::debug!(?old, ?new, "PipeWire capture stream state");
            let mut local = local.borrow_mut();
            match new {
                pw::stream::StreamState::Error(text) => {
                    local.fail(anyhow!("stream error: {text}"));
                }
                pw::stream::StreamState::Unconnected
                    if matches!(
                        old,
                        pw::stream::StreamState::Streaming | pw::stream::StreamState::Paused
                    ) =>
                {
                    local.fail(anyhow!("the stream disconnected"));
                }
                _ => {}
            }
        })
        .param_changed(|stream, local, id, param| {
            let Some(param) = param else { return };
            if id != ParamType::Format.as_raw() {
                return;
            }
            let mut local = local.borrow_mut();
            if let Err(error) = local.format_changed(stream, param) {
                local.fail(error);
            }
        })
        .process(|stream, local| {
            let arrived = Instant::now();
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let mut local = local.borrow_mut();
            if let Err(error) = local.frame(&mut buffer, arrived) {
                local.fail(error);
            }
        })
        .register()?;
    // The node going away ends the capture: the stream itself may only go quiet.
    let registry = core.get_registry_rc()?;
    let _registry_listener = registry
        .add_listener_local()
        .global_remove({
            let local = local.clone();
            move |id| {
                if id == node {
                    local
                        .borrow_mut()
                        .fail(anyhow!("PipeWire node {node} was removed"));
                }
            }
        })
        .register();
    let offers = [
        offer(VideoFormat::NV12)?,
        offer(VideoFormat::BGRx)?,
        offer(VideoFormat::BGRA)?,
    ];
    let mut params = offers
        .iter()
        .map(|bytes| Pod::from_bytes(bytes).context("format offer"))
        .collect::<Result<Vec<_>>>()?;
    stream.connect(
        spa::utils::Direction::Input,
        Some(node),
        pw::stream::StreamFlags::AUTOCONNECT,
        &mut params,
    )?;
    mainloop.run();
    drop(listener);
    drop(stream);
    local.borrow_mut().release();
    // The pool and converter go before the display they belong to.
    local.borrow_mut().format = None;
    shared.end("the capture was closed");
    Ok(())
}
