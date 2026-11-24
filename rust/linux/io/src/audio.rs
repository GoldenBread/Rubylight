//! Audio on Linux: the Opus encoder and the microphone's Opus decoder load libopus at run
//! time and work, and `Loopback` captures what the host plays through PipeWire. Capturing one
//! game's sound has no backend yet, so opening it fails.

use anyhow::{Context, Result, bail};
use std::{ffi::c_void, path::Path, time::Duration};

mod capture;
mod queue;
#[cfg(test)]
mod tests;

/// The library's name; the system's library path finds it.
const LIBRARY: &str = "libopus.so.0";
/// A folder holding the library, for tests that run without it installed.
const TEST_ROOT: &str = "BUTTERPOLLO_TEST_OPUS_ROOT";

/// libopus from `directory` when it holds one, else from the test folder, else from the
/// system.
fn load(directory: &Path) -> Result<libloading::Library> {
    let test_root = std::env::var_os(TEST_ROOT).map(std::path::PathBuf::from);
    let beside = [Some(directory.to_path_buf()), test_root]
        .into_iter()
        .flatten()
        .map(|folder| folder.join(LIBRARY))
        .find(|path| path.is_file());
    let target = beside.unwrap_or_else(|| LIBRARY.into());
    // SAFETY: libopus runs no initialisation code that depends on the loading thread.
    unsafe { libloading::Library::new(&target) }
        .with_context(|| format!("loading {}", target.display()))
}

/// Capture of what the host plays, through a PipeWire stream on a sink's monitor. PipeWire
/// converts the device to 48 kHz and `channels` channels in Moonlight's order, so the samples
/// are the packet's own.
pub struct Loopback {
    capture: capture::Capture,
    channels: usize,
    drained: Option<std::time::Instant>,
    /// Packets of audio `read_into` handed out, for the trace.
    trace: Option<Vec<butterpollo_media_policy::audio_cadence::Captured>>,
    started: std::time::Instant,
}

impl Loopback {
    /// Capture the default device, mixed to `output_channels`.
    pub fn new(output_channels: usize) -> Result<Self> {
        Self::new_sink(output_channels, "")
    }
    /// Capture the device `sink`, or the default device when it is empty.
    pub fn new_sink(output_channels: usize, sink: &str) -> Result<Self> {
        Self::new_sink_traced(output_channels, sink, false)
    }
    /// [`Self::new_sink`], recording every captured packet for `drain_trace` when `trace` is
    /// set.
    pub fn new_sink_traced(output_channels: usize, sink: &str, trace: bool) -> Result<Self> {
        Ok(Self {
            capture: capture::Capture::open(output_channels, sink)
                .with_context(|| format!("opening audio capture ({})", sink_name(sink)))?,
            channels: output_channels,
            drained: None,
            trace: trace.then(Vec::new),
            started: std::time::Instant::now(),
        })
    }
    /// The identity of the game process this captures; none for a device capture.
    pub fn process_identity(&self) -> Option<butterpollo_session::audio_scope::Identity> {
        None
    }
    /// Wait up to `timeout` for captured audio. An idle sink delivers nothing; the timeout
    /// keeps the caller's silence on time.
    pub fn wait(&self, timeout: Duration) {
        self.capture.shared.wait(timeout);
    }
    /// Whether the capture wakes the caller when audio arrives.
    pub fn event_driven(&self) -> bool {
        true
    }
    /// The capture's channel count and speaker mask; PipeWire already mixed the device to the
    /// output channels in the standard order, so the mask is 0.
    pub fn source(&self) -> (usize, u32) {
        (self.channels, 0)
    }
    /// How much audio this capture holds between reads: its queue's bound.
    pub fn buffer(&self) -> Duration {
        Duration::from_millis(100)
    }
    /// The time since the previous call, None on the first.
    pub fn since_drained(&mut self) -> Option<Duration> {
        let now = std::time::Instant::now();
        self.drained
            .replace(now)
            .map(|at| now.saturating_duration_since(at))
    }
    /// None when no audio arrived since the last call, else how much the system skipped
    /// before it: PipeWire skips nothing.
    pub fn take_delivered(&mut self) -> Option<Duration> {
        self.capture
            .shared
            .lock()
            .take_arrived()
            .then_some(Duration::ZERO)
    }
    /// How much captured audio this host discarded since the last call, in 48 kHz samples.
    pub fn take_trimmed(&mut self) -> u64 {
        self.capture.shared.lock().take_trimmed()
    }
    /// What this capture holds that will never be sent, in 48 kHz samples.
    pub fn take_discarded(&mut self) -> u64 {
        let mut queue = self.capture.shared.lock();
        queue.take_trimmed() + queue.queued()
    }
    /// Moves the packets traced since the last call into `into`.
    pub fn drain_trace(&mut self, into: &mut butterpollo_media_policy::audio_cadence::Trace) {
        for entry in self.trace.iter_mut().flat_map(|trace| trace.drain(..)) {
            into.capture(entry);
        }
    }
    /// One packet of `frames`, when a whole one was there.
    pub fn read(&mut self, frames: usize) -> Result<Option<Vec<f32>>> {
        let mut out = Vec::new();
        Ok(self.read_into(frames, &mut out)?.then_some(out))
    }
    /// As [`Self::read`], into `out` (emptied first): whether a whole packet was there. Fails
    /// once PipeWire's stream or daemon is gone.
    pub fn read_into(&mut self, frames: usize, out: &mut Vec<f32>) -> Result<bool> {
        let mut queue = self.capture.shared.lock();
        let full = queue.read_into(frames, out);
        if !full && let Some(error) = &queue.error {
            bail!("audio capture failed: {error}");
        }
        if full && let Some(trace) = &mut self.trace {
            trace.push(butterpollo_media_policy::audio_cadence::Captured {
                read_ms: self.started.elapsed().as_secs_f64() * 1000.,
                device_qpc_ms: self.started.elapsed().as_secs_f64() * 1000.,
                position: 0,
                frames: frames as u32,
                flags: 0,
            });
        }
        Ok(full)
    }
}

fn sink_name(sink: &str) -> String {
    if sink.is_empty() {
        "default sink".into()
    } else {
        format!("sink {sink}")
    }
}

/// The start of capturing one game's sound. It cannot be made on Linux.
pub struct ProcessActivation {
    process: crate::process::AudioProcess,
}
impl ProcessActivation {
    /// Begin capturing the sound of `process` and its children. Fails on Linux.
    pub fn new(_process: crate::process::AudioProcess) -> Result<Self> {
        Err(crate::unsupported("Game audio capture"))
    }
    /// The process being captured.
    pub fn identity(&self) -> butterpollo_session::audio_scope::Identity {
        self.process.identity
    }
    /// Poll without waiting: the capture once it is ready.
    pub fn poll(&self, _output_channels: usize) -> Result<Option<Loopback>> {
        Err(crate::unsupported("Game audio capture"))
    }
}

/// The most an Opus packet may take: room for RTP, AES padding and parity headers is left
/// within the previous Moonlight receiver's fixed 1400-byte UDP buffer.
const OPUS_PACKET_ROOM: usize = 1360;
type Create = unsafe extern "C" fn(i32, i32, i32, i32, *const u8, i32, *mut i32) -> *mut c_void;
type Encode = unsafe extern "C" fn(*mut c_void, *const f32, i32, *mut u8, i32) -> i32;
type Destroy = unsafe extern "C" fn(*mut c_void);
type Ctl = unsafe extern "C" fn(*mut c_void, i32, ...) -> i32;

/// A multistream Opus encoder.
pub struct Opus {
    state: *mut c_void,
    encode: Encode,
    destroy: Destroy,
    _library: libloading::Library,
    channels: usize,
    /// Each packet's room, made once.
    output: Vec<u8>,
}
impl Opus {
    pub fn new(directory: &Path, channels: usize, quality: bool) -> Result<Self> {
        Self::new_layout(
            directory,
            &butterpollo_media_policy::audio::OpusLayout::select(channels, quality, None)?,
        )
    }
    pub fn new_layout(
        directory: &Path,
        layout: &butterpollo_media_policy::audio::OpusLayout,
    ) -> Result<Self> {
        Self::new_layout_duration(directory, layout, 5)
    }
    pub fn new_layout_duration(
        directory: &Path,
        layout: &butterpollo_media_policy::audio::OpusLayout,
        packet_ms: u8,
    ) -> Result<Self> {
        if !matches!(packet_ms, 5 | 10 | 20) {
            bail!("unsupported Opus packet duration");
        }
        // High-quality surround at 10/20ms otherwise exceeds Moonlight's fixed UDP buffer.
        // Opus can silently starve the last coded stream when only its output buffer is
        // capped. Bound CBR before encoding, reserving space for each self-delimited
        // stream's length fields.
        let packet_bitrate = (1360 - layout.streams * 4) * 8000 / i32::from(packet_ms);
        let bitrate = layout.bitrate.min(packet_bitrate);
        let library = load(directory)?;
        // SAFETY: the symbol types match libopus, `layout.mapping` has `channels` entries, and
        // `library` is kept in `Self` with its function pointers.
        unsafe {
            let create: libloading::Symbol<Create> =
                library.get(b"opus_multistream_encoder_create\0")?;
            let encode = *library.get::<Encode>(b"opus_multistream_encode_float\0")?;
            let destroy = *library.get::<Destroy>(b"opus_multistream_encoder_destroy\0")?;
            let ctl = *library.get::<Ctl>(b"opus_multistream_encoder_ctl\0")?;
            let mut error = 0;
            let state = create(
                48000,
                layout.channels as i32,
                layout.streams,
                layout.coupled,
                layout.mapping.as_ptr(),
                2051,
                &mut error,
            );
            if error != 0 || state.is_null() {
                bail!("Opus initialization failed: {error}");
            }
            let encoder = Self {
                state,
                encode,
                destroy,
                _library: library,
                channels: layout.channels,
                output: vec![0; OPUS_PACKET_ROOM],
            };
            for (request, value) in [(4002, bitrate), (4006, 0i32)] {
                let result = ctl(state, request, value);
                if result != 0 {
                    bail!("Opus control {request} failed: {result}");
                }
            }
            Ok(encoder)
        }
    }
    pub fn encode(&mut self, samples: &[f32]) -> Result<Vec<u8>> {
        self.encode_frame(samples).map(<[u8]>::to_vec)
    }
    /// As [`Self::encode`], in the encoder's own room: the audio thread's way, which makes
    /// nothing per packet.
    pub fn encode_frame(&mut self, samples: &[f32]) -> Result<&[u8]> {
        if !samples.len().is_multiple_of(self.channels) {
            bail!("incomplete audio frame");
        }
        // SAFETY: `self.state` is a live encoder, `samples` holds whole frames for `channels`,
        // and `self.output` is writable for the length passed.
        let n = unsafe {
            (self.encode)(
                self.state,
                samples.as_ptr(),
                (samples.len() / self.channels) as i32,
                self.output.as_mut_ptr(),
                self.output.len() as i32,
            )
        };
        if n < 0 {
            bail!("Opus encoding error {n}");
        }
        // The room stays whole: the next packet is allowed the same length.
        Ok(&self.output[..n as usize])
    }
}
impl Drop for Opus {
    fn drop(&mut self) {
        // SAFETY: `self.state` came from opus_multistream_encoder_create and is destroyed
        // once, here.
        unsafe {
            (self.destroy)(self.state);
        }
    }
}

type DecoderCreate = unsafe extern "C" fn(i32, i32, *mut i32) -> *mut c_void;
type Decode = unsafe extern "C" fn(*mut c_void, *const u8, i32, *mut f32, i32, i32) -> i32;
type PacketSamples = unsafe extern "C" fn(*const c_void, *const u8, i32) -> i32;
/// Longest Opus frame, 120 ms at 48 kHz.
const MAX_OPUS_FRAMES: usize = 5760;

/// A mono 48 kHz Opus decoder, for a client's microphone.
pub struct OpusDecoder {
    state: *mut c_void,
    decode: Decode,
    samples: PacketSamples,
    ctl: Ctl,
    destroy: Destroy,
    _library: libloading::Library,
    /// The last frame's length, for concealing the next one.
    frames: usize,
}
// SAFETY: the decoder state is used only through `&mut self`.
unsafe impl Send for OpusDecoder {}
impl OpusDecoder {
    pub fn new(directory: &Path) -> Result<Self> {
        let library = load(directory)?;
        // SAFETY: the symbol types match libopus, and `library` is kept in `Self` with its
        // function pointers.
        unsafe {
            let create = *library.get::<DecoderCreate>(b"opus_decoder_create\0")?;
            let decode = *library.get::<Decode>(b"opus_decode_float\0")?;
            let samples = *library.get::<PacketSamples>(b"opus_decoder_get_nb_samples\0")?;
            let ctl = *library.get::<Ctl>(b"opus_decoder_ctl\0")?;
            let destroy = *library.get::<Destroy>(b"opus_decoder_destroy\0")?;
            let mut error = 0;
            let state = create(48000, 1, &mut error);
            if error != 0 || state.is_null() {
                bail!("Opus decoder initialization failed: {error}");
            }
            Ok(Self {
                state,
                decode,
                samples,
                ctl,
                destroy,
                _library: library,
                frames: 960,
            })
        }
    }
    /// Forgets the previous speech, for a stream that starts again.
    pub fn reset(&mut self) {
        // SAFETY: `self.state` is a live decoder; OPUS_RESET_STATE takes no argument.
        unsafe {
            (self.ctl)(self.state, 4028);
        }
    }
    /// Appends a frame to `out` and returns its length: `packet` decoded, the frame before it
    /// from its in-band FEC when `fec` is set, or a concealed frame when there is no packet.
    pub fn decode(
        &mut self,
        packet: Option<&[u8]>,
        fec: bool,
        out: &mut Vec<f32>,
    ) -> Result<usize> {
        let (data, length) = packet.map_or((std::ptr::null(), 0), |p| (p.as_ptr(), p.len() as i32));
        let frames = match packet {
            // SAFETY: `self.state` is a live decoder and `data` is readable for `length` bytes.
            Some(_) => match unsafe { (self.samples)(self.state, data, length) } {
                n if n > 0 && (n as usize) <= MAX_OPUS_FRAMES => n as usize,
                n => bail!("invalid Opus packet ({n})"),
            },
            None => self.frames,
        };
        let start = out.len();
        out.resize(start + frames, 0.);
        // SAFETY: `self.state` is a live decoder, `data` is null or readable for `length`
        // bytes, and `out` has room for `frames` mono samples after `start`.
        let n = unsafe {
            (self.decode)(
                self.state,
                data,
                length,
                out[start..].as_mut_ptr(),
                frames as i32,
                i32::from(fec),
            )
        };
        if n < 0 {
            out.truncate(start);
            bail!("Opus decoding error {n}");
        }
        out.truncate(start + n as usize);
        self.frames = n as usize;
        Ok(n as usize)
    }
}
impl Drop for OpusDecoder {
    fn drop(&mut self) {
        // SAFETY: `self.state` came from opus_decoder_create and is destroyed once, here.
        unsafe {
            (self.destroy)(self.state);
        }
    }
}
