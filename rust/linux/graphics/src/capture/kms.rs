//! The target `kms` or `kms:<output>`: the picture a display controller shows, whichever
//! compositor owns it, the login manager's too, taken from Rubylight's KMS broker
//! (rust/linux/kms-broker), which holds the privilege of reading another program's
//! framebuffer. `kms` follows the first lit output.
//!
//! Each framebuffer comes as DMA-BUFs and is imported into VA-API in place (once per buffer of
//! the compositor's swapchain), converted like a PipeWire frame, and the cursor plane, never part
//! of the framebuffer, is blended over it at its place with video processing.
use super::pw_capture::{Frame, inode, pool};
use crate::{
    timing::Signal,
    vaapi::{
        AB30, AR30, BGRA, BGRX, Plane, RGBA, RGBX, Va, XB30, XR30,
        ffi::VASurfaceID,
        pool::Pool,
        vpp::{Colors, Converter, Rect, Source},
    },
};
use anyhow::{Context, Result, bail};
use butterpollo_media_policy::capture_policy::Picture;
use rubylight_kms_broker::{
    client::{Client, Frame as KmsFrame},
    protocol::{self, CursorInfo},
};
use std::{
    collections::HashMap,
    os::fd::{AsRawFd, OwnedFd},
    path::Path,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub const TARGET: &str = "kms";
/// How long opening waits for the first frame.
const OPEN_TIMEOUT: Duration = Duration::from_secs(5);
/// How long one request to the broker waits for a change; the thread looks at its stop flag
/// in between.
const WAIT: Duration = Duration::from_millis(200);
/// Surfaces kept for the compositor's buffers: a swapchain has two to four.
const IMPORTED_LIMIT: usize = 16;

/// `kms` → `Some(None)`, `kms:DP-1` → `Some(Some("DP-1"))`, anything else `None`.
pub fn parse(target: &str) -> Option<Option<&str>> {
    let rest = target.strip_prefix(TARGET)?;
    match rest.strip_prefix(':') {
        Some(name) if !name.is_empty() => Some(Some(name)),
        Some(_) => None,
        None => rest.is_empty().then_some(None),
    }
}

/// The VA-API pixel format of a DRM fourcc a compositor scans out.
fn va_pixel(fourcc: u32) -> Option<u32> {
    Some(match &fourcc.to_le_bytes() {
        b"XR24" => BGRX,
        b"AR24" => BGRA,
        b"XB24" => RGBX,
        b"AB24" => RGBA,
        b"AR30" => AR30,
        b"XR30" => XR30,
        b"AB30" => AB30,
        b"XB30" => XB30,
        _ => return None,
    })
}

/// The part of a `size` cursor image at `(x, y)` that falls on a `frame` sized output, as
/// the rectangle of the image and the rectangle of the frame; `None` when none does.
fn visible(x: i32, y: i32, size: (u32, u32), frame: (u32, u32)) -> Option<(Rect, Rect)> {
    let axis = |at: i32, length: u32, limit: u32| -> Option<(u32, u32, u32)> {
        let skip = (-at).max(0) as u32;
        let start = at.max(0) as u32;
        if skip >= length || start >= limit {
            return None;
        }
        Some((skip, start, (length - skip).min(limit - start)))
    };
    let (sx, dx, width) = axis(x, size.0, frame.0)?;
    let (sy, dy, height) = axis(y, size.1, frame.1)?;
    Some((
        Rect {
            x: sx,
            y: sy,
            width,
            height,
        },
        Rect {
            x: dx,
            y: dy,
            width,
            height,
        },
    ))
}

struct Shared {
    latest: Mutex<Option<Frame>>,
    ended: Mutex<Option<String>>,
    signal: Signal,
    frames: AtomicU64,
    dropped: AtomicU64,
    stop: AtomicBool,
}

impl Shared {
    fn end(&self, why: String) {
        self.ended
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert(why);
        let _ = self.signal.set();
    }
}

/// A running capture of one output through the broker.
pub struct KmsCapture {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    /// The output's name and the picture's size, as the first frame had them.
    pub output: String,
    pub size: (u32, u32),
    colors: Colors,
}

impl KmsCapture {
    /// Connects to the broker and waits for `output`'s first frame (the first lit output
    /// for `None`).
    pub fn open(output: Option<&str>, va: &Arc<Va>, picture: Picture) -> Result<Self> {
        let client = Client::connect(Path::new(protocol::SOCKET))?;
        let output = match output {
            Some(name) => name.to_owned(),
            None => {
                client
                    .outputs()?
                    .into_iter()
                    .next()
                    .context("no display is lit to capture")?
                    .name
            }
        };
        let shared = Arc::new(Shared {
            latest: Mutex::new(None),
            ended: Mutex::new(None),
            signal: Signal::new()?,
            frames: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            stop: AtomicBool::new(false),
        });
        let (ready, opened) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("kms-capture".into())
            .spawn({
                let (shared, va, name) = (shared.clone(), va.clone(), output.clone());
                move || {
                    let mut local = Local {
                        client,
                        output: name,
                        va,
                        picture,
                        shared: shared.clone(),
                        ready: Some(ready),
                        state: None,
                        imported: HashMap::new(),
                    };
                    if let Err(error) = local.run() {
                        let text = format!("{error:#}");
                        if let Some(ready) = local.ready.take() {
                            let _ = ready.send(Err(text.clone()));
                        }
                        shared.end(text);
                    }
                    local.release();
                }
            })
            .context("starting the KMS capture thread")?;
        let mut capture = Self {
            shared,
            thread: Some(thread),
            output,
            size: (0, 0),
            colors: picture.into(),
        };
        capture.size = match opened.recv_timeout(OPEN_TIMEOUT) {
            Ok(Ok(size)) => size,
            Ok(Err(error)) => bail!(
                "capturing {} through the KMS broker: {error}",
                capture.output
            ),
            Err(_) => bail!(
                "{} showed no picture within {OPEN_TIMEOUT:?}; is the display on?",
                capture.output
            ),
        };
        tracing::info!(output = %capture.output, size = ?capture.size, "KMS capture through the broker");
        Ok(capture)
    }

    pub fn signal(&self) -> &Signal {
        &self.shared.signal
    }

    pub fn colors(&self) -> Colors {
        self.colors
    }

    /// The newest converted frame, if one came since the last call. An error once the capture
    /// has ended and its last frame was taken.
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
            bail!("the display capture ended: {why}");
        }
        Ok(frame)
    }
}

impl Drop for KmsCapture {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        tracing::debug!(
            frames = self.shared.frames.load(Ordering::Relaxed),
            dropped = self.shared.dropped.load(Ordering::Relaxed),
            "KMS capture closed"
        );
    }
}

/// What the picture's size made: the surfaces frames go to and the converter.
struct Converting {
    size: (u32, u32),
    pool: Arc<Pool>,
    converter: Converter,
}

/// The capture thread's state.
struct Local {
    client: Client,
    output: String,
    va: Arc<Va>,
    picture: Picture,
    shared: Arc<Shared>,
    ready: Option<mpsc::Sender<Result<(u32, u32), String>>>,
    state: Option<Converting>,
    /// One surface per compositor buffer (and cursor image), by the DMA-BUF's inode, with
    /// the descriptors it was made from.
    imported: HashMap<u64, (VASurfaceID, Vec<OwnedFd>)>,
}

impl Local {
    fn run(&mut self) -> Result<()> {
        while !self.shared.stop.load(Ordering::Acquire) {
            let Some(frame) = self.client.frame(&self.output, true, WAIT)? else {
                continue;
            };
            let arrived = Instant::now();
            self.frame(frame, arrived)?;
        }
        Ok(())
    }

    fn release(&mut self) {
        for (_, (mut id, _)) in self.imported.drain() {
            self.va.destroy_surfaces(std::slice::from_mut(&mut id));
        }
    }

    /// The surface over `fds`, imported the first time this buffer is seen.
    fn import(
        &mut self,
        pixel: u32,
        modifier: u64,
        size: (u32, u32),
        fds: Vec<OwnedFd>,
        layout: &[(u32, u32)],
    ) -> Result<VASurfaceID> {
        let key = inode(fds[0].as_raw_fd());
        if let Some((id, _)) = self.imported.get(&key) {
            return Ok(*id);
        }
        if self.imported.len() >= IMPORTED_LIMIT {
            // The compositor made new buffers; the old ones are gone.
            self.release();
        }
        let planes: Vec<Plane> = fds
            .iter()
            .zip(layout)
            .map(|(fd, &(pitch, offset))| Plane {
                fd: fd.as_raw_fd(),
                offset,
                pitch,
            })
            .collect();
        let id = self
            .va
            .import_dmabuf(pixel, modifier, size.0, size.1, &planes)
            .with_context(|| format!("importing a framebuffer (modifier 0x{modifier:x})"))?;
        self.imported.insert(key, (id, fds));
        Ok(id)
    }

    fn frame(&mut self, frame: KmsFrame, arrived: Instant) -> Result<()> {
        let KmsFrame {
            info,
            planes,
            cursor,
        } = frame;
        let size = (info.width, info.height);
        match &self.state {
            Some(state) if state.size != size => bail!(
                "the picture changed size from {}x{} to {}x{}",
                state.size.0,
                state.size.1,
                size.0,
                size.1
            ),
            Some(_) => {}
            None => {
                self.state = Some(Converting {
                    size,
                    pool: pool(&self.va, size.0, size.1, self.picture.full_chroma)?,
                    converter: Converter::new(&self.va, size.0, size.1)?
                        .writing(self.picture.into()),
                });
            }
        }
        let pixel = va_pixel(info.fourcc).with_context(|| {
            format!(
                "the display scans out {:?}, which the capture does not take",
                String::from_utf8_lossy(&info.fourcc.to_le_bytes())
            )
        })?;
        // The compositor may still be drawing: wait for its fence, bounded.
        let mut ready = libc::pollfd {
            fd: planes[0].as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one open descriptor.
        unsafe { libc::poll(&raw mut ready, 1, 50) };
        let layout: Vec<_> = info.planes.iter().map(|p| (p.pitch, p.offset)).collect();
        let input = self.import(pixel, info.modifier, size, planes, &layout)?;
        let pointer = match (info.cursor, cursor) {
            (Some(at), Some(fd)) => self.pointer(&at, fd)?,
            _ => None,
        };
        self.shared.frames.fetch_add(1, Ordering::Relaxed);
        let state = self.state.as_ref().context("no converter")?;
        let Some(output) = state.pool.take() else {
            // Every surface is held by the stream: the next change replaces this frame.
            self.shared.dropped.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        };
        let full_chroma = self.picture.full_chroma;
        if full_chroma {
            state.converter.scale_rgb(input, size, output.id(), size)?;
        } else {
            state
                .converter
                .convert(input, Source::Rgb, size, output.id(), size)?;
        }
        if let Some((image, at)) = pointer
            && let Some((from, to)) = visible(at.x, at.y, (at.width, at.height), size)
        {
            state
                .converter
                .blend(image, from, output.id(), to, full_chroma)?;
        }
        // The compositor draws into this buffer again once it has flipped away: the copy must
        // have read it by then.
        self.va.sync(output.id())?;
        let acquired = Instant::now();
        *self
            .shared
            .latest
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Frame {
            surface: Arc::new(output),
            captured: arrived,
            acquired,
            published: acquired,
        });
        let _ = self.shared.signal.set();
        if let Some(ready) = self.ready.take() {
            let _ = ready.send(Ok(size));
        }
        Ok(())
    }

    /// The cursor image's surface, when its format is one the blend takes.
    fn pointer(
        &mut self,
        at: &CursorInfo,
        fd: OwnedFd,
    ) -> Result<Option<(VASurfaceID, CursorInfo)>> {
        let pixel = match &at.fourcc.to_le_bytes() {
            b"AR24" => BGRA,
            b"AB24" => RGBA,
            _ => return Ok(None),
        };
        let id = self.import(
            pixel,
            at.modifier,
            (at.width, at.height),
            vec![fd],
            &[(at.pitch, at.offset)],
        )?;
        Ok(Some((id, at.clone())))
    }
}

/// The KMS capture as the stream holds it: the capture and the device its frames are on.
pub struct Kms {
    capture: KmsCapture,
    device: super::Device,
    staging: Option<super::D3d11Texture>,
}

impl Kms {
    /// Opens `output` (the first lit one for `None`) on the configured GPU, as a PipeWire
    /// capture does.
    pub(super) fn open(
        output: Option<&str>,
        hdr: bool,
        config: &butterpollo_config::config::Config,
    ) -> Result<Self> {
        if hdr {
            return Err(crate::unsupported("HDR capture"));
        }
        let name = output.map_or_else(|| TARGET.to_owned(), |o| format!("{TARGET}:{o}"));
        let probe = super::Device::new_adapter(
            &name,
            config.get("adapter_name", ""),
            config.get("adapter_pnp_id", ""),
        )?;
        let capture = KmsCapture::open(output, probe.va(), Picture::read(config))?;
        let found = crate::vaapi::node::nodes()
            .iter()
            .find(|candidate| candidate.minor as usize == probe.key())
            .context("the GPU's render node is gone")?;
        let device = super::Device::on(probe.va().clone(), found, &name, capture.size);
        Ok(Self {
            capture,
            device,
            staging: None,
        })
    }

    pub(super) fn signal(&self) -> &Signal {
        self.capture.signal()
    }

    pub(super) fn next_gpu(&mut self, origin: u64) -> Result<Option<Arc<super::GpuImage>>> {
        let Some(frame) = self.capture.take()? else {
            return Ok(None);
        };
        let (width, height) = self.capture.size;
        Ok(Some(Arc::new(super::GpuImage {
            width,
            height,
            pixel: super::Pixel::Bgra8,
            captured: frame.captured,
            wgc_stamp: None,
            acquired: frame.acquired,
            published: frame.published,
            gpu: self.device.clone(),
            focus: None,
            repeat: false,
            origin,
            surface: Some(frame.surface),
            colors: self.capture.colors(),
        })))
    }

    pub(super) fn next_frame(&mut self) -> Result<Option<super::Image>> {
        match self.next_gpu(0)? {
            Some(image) => image.readback(&mut self.staging).map(Some),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
#[path = "kms_tests.rs"]
mod tests;
