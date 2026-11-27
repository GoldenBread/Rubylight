use anyhow::{Context, Result, bail};
use std::{
    ffi::c_void,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::Path,
};
use windows::Win32::{Media::Audio::*, System::Com::*};
mod keepalive;
mod process_loopback;
pub use process_loopback::Activation as ProcessActivation;

struct MixFormat(*mut WAVEFORMATEX);
impl Drop for MixFormat {
    fn drop(&mut self) {
        // SAFETY: GetMixFormat allocated this pointer; this guard frees it exactly once.
        unsafe { CoTaskMemFree(Some(self.0.cast::<c_void>())) };
    }
}
pub struct Loopback {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    /// Set by Windows when captured audio is ready; None where event mode
    /// could not be opened, and the caller polls.
    ready: Option<OwnedHandle>,
    samples: CaptureSamples,
    /// Audio Windows holds for us; what arrives while it is full is lost.
    buffer: std::time::Duration,
    drained: Option<std::time::Instant>,
    rate: u32,
    /// Where the last packet started in the device's stream, and where the
    /// next should; a later start skipped what Windows dropped.
    position: Option<(u64, u64)>,
    /// Whether audio arrived since `take_delivered`, and the frames
    /// skipped before it.
    delivered: Option<u64>,
    /// Keeps a process identity alive for game-only capture; system capture
    /// owns no process and retains its existing device-routing behavior.
    process: Option<crate::process::AudioProcess>,
    /// The endpoint's own channel count and speaker mask.
    source: (usize, u32),
    /// Keeps an endpoint capture delivering through silence; None for game-only capture.
    _keepalive: Option<keepalive::KeepAlive>,
    /// Every packet `GetBuffer` returned since `drain_trace`, when `audio_cadence_trace` is on.
    trace: Option<Vec<butterpollo_media_policy::audio_cadence::Captured>>,
}
impl Loopback {
    pub fn new(output_channels: usize) -> Result<Self> {
        Self::new_sink(output_channels, "")
    }
    pub fn new_sink(output_channels: usize, sink: &str) -> Result<Self> {
        Self::new_sink_traced(output_channels, sink, false)
    }
    /// [`Self::new_sink`], recording every captured packet for `drain_trace` when `trace` is set.
    pub fn new_sink_traced(output_channels: usize, sink: &str, trace: bool) -> Result<Self> {
        // SAFETY: COM calls only fail if the thread lacks COM, `id` outlives GetDevice, and the mix
        // format is read within its header plus cbSize bytes and freed once.
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let device = if sink.is_empty() {
                enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?
            } else {
                let id: Vec<u16> = sink.encode_utf16().chain(Some(0)).collect();
                enumerator.GetDevice(windows::core::PCWSTR(id.as_ptr()))?
            };
            let mut client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            let format = MixFormat(client.GetMixFormat()?);
            if format.0.is_null() {
                bail!("empty WASAPI mix format");
            }
            let f = *format.0;
            let float = f.wFormatTag == 3
                || (f.wFormatTag == 65534
                    && f.cbSize >= 22
                    && (*(format.0 as *const WAVEFORMATEXTENSIBLE)).SubFormat.data1 == 3);
            let mask = if f.wFormatTag == 65534 && f.cbSize >= 22 {
                (*(format.0 as *const WAVEFORMATEXTENSIBLE)).dwChannelMask
            } else {
                0
            };
            let matrix = butterpollo_media_policy::audio::mix_matrix(
                f.nChannels as usize,
                mask,
                output_channels,
            );
            let resampler =
                butterpollo_media_policy::audio::Resampler::new(f.nSamplesPerSec, output_channels);
            // Event mode wakes the sender the moment audio is captured, as the
            // C++ host does; a 1 ms poll cost 0.6 ms on average. Where Windows
            // refuses it for loopback, a fresh client polls as before.
            let ready = windows::Win32::System::Threading::CreateEventW(
                None,
                false,
                false,
                windows::core::PCWSTR::null(),
            )
            .ok()
            .map(|handle| OwnedHandle::from_raw_handle(handle.0));
            let mut result = Err(windows::core::Error::empty());
            if let Some(event) = &ready {
                result = client
                    .Initialize(
                        AUDCLNT_SHAREMODE_SHARED,
                        AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                        100000,
                        0,
                        format.0,
                        None,
                    )
                    .and_then(|()| {
                        client.SetEventHandle(windows::Win32::Foundation::HANDLE(
                            event.as_raw_handle(),
                        ))
                    });
            }
            let ready = if result.is_ok() {
                ready
            } else {
                drop(ready);
                client = device.Activate(CLSCTX_ALL, None)?;
                result = client.Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_LOOPBACK,
                    100000,
                    0,
                    format.0,
                    None,
                );
                None
            };
            result?;
            let matrix = matrix?;
            let resampler = resampler?;
            if !matches!(f.wBitsPerSample, 16 | 24 | 32) || (float && f.wBitsPerSample != 32) {
                bail!("unsupported WASAPI sample format");
            }
            // Unknown when Windows will not say; loss is then not estimated.
            let rate = f.nSamplesPerSec.max(1);
            let buffer = client
                .GetBufferSize()
                .map_or(std::time::Duration::ZERO, |frames| {
                    std::time::Duration::from_secs_f64(f64::from(frames) / f64::from(rate))
                });
            let capture = client.GetService()?;
            client.Start()?;
            // Without it the capture still works and goes quiet through silence, as before.
            let keepalive = keepalive::KeepAlive::new(&device, format.0)
                .inspect_err(|error| {
                    tracing::warn!(
                        "audio keep-alive render stream failed, silence will start late: {error:#}"
                    )
                })
                .ok();
            if trace {
                let (mut default, mut minimum) = (0, 0);
                let _ = client.GetDevicePeriod(Some(&mut default), Some(&mut minimum));
                tracing::info!(
                    buffer_ms = buffer.as_secs_f64() * 1000.,
                    default_period_hns = default,
                    minimum_period_hns = minimum,
                    event_driven = ready.is_some(),
                    "audio cadence trace on"
                );
            }
            Ok(Self {
                client,
                capture,
                ready,
                samples: CaptureSamples {
                    channels: f.nChannels as usize,
                    bits: f.wBitsPerSample,
                    float,
                    resampler,
                    identity: butterpollo_media_policy::audio::mix_is_identity(&matrix),
                    matrix,
                    input: vec![0.; f.nChannels as usize],
                    output: vec![0.; output_channels],
                    trimmed: 0,
                },
                buffer,
                drained: None,
                rate,
                position: None,
                delivered: None,
                process: None,
                source: (f.nChannels as usize, mask),
                _keepalive: keepalive,
                trace: trace.then(Vec::new),
            })
        }
    }
    pub fn process_identity(&self) -> Option<butterpollo_session::audio_scope::Identity> {
        self.process.as_ref().map(|p| p.identity)
    }
    /// Captures the verified game process and its children, across playback
    /// devices. It never switches/mutes endpoints or captures other processes.
    fn from_process_client(
        output_channels: usize,
        process: crate::process::AudioProcess,
        client: IAudioClient,
    ) -> Result<Self> {
        anyhow::ensure!(
            process.alive(),
            "game process exited during audio activation"
        );
        // An explicit 48 kHz float stereo format works independently of an
        // endpoint mix format. Surround streams receive a documented stereo
        // upmix until process-loopback surround is independently validated.
        let format = WAVEFORMATEX {
            wFormatTag: 3,
            nChannels: 2,
            nSamplesPerSec: 48_000,
            nAvgBytesPerSec: 48_000 * 8,
            nBlockAlign: 8,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        // SAFETY: COM is initialized by the caller; the format lives through
        // Initialize, and the owned event outlives the client capture.
        unsafe {
            let event = windows::Win32::System::Threading::CreateEventW(
                None,
                false,
                false,
                windows::core::PCWSTR::null(),
            )?;
            let ready = OwnedHandle::from_raw_handle(event.0);
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK
                    | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                    | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
                0,
                0,
                &format,
                None,
            )?;
            client.SetEventHandle(event)?;
            let buffer = client
                .GetBufferSize()
                .map_or(std::time::Duration::ZERO, |n| {
                    std::time::Duration::from_secs_f64(f64::from(n) / 48_000.)
                });
            let capture = client.GetService()?;
            let matrix = butterpollo_media_policy::audio::mix_matrix(2, 3, output_channels)?;
            let resampler =
                butterpollo_media_policy::audio::Resampler::new(48_000, output_channels)?;
            client.Start()?;
            Ok(Self {
                client,
                capture,
                ready: Some(ready),
                samples: CaptureSamples {
                    channels: 2,
                    bits: 32,
                    float: true,
                    resampler,
                    identity: butterpollo_media_policy::audio::mix_is_identity(&matrix),
                    matrix,
                    input: vec![0.; 2],
                    output: vec![0.; output_channels],
                    trimmed: 0,
                },
                buffer,
                drained: None,
                rate: 48_000,
                position: None,
                delivered: None,
                process: Some(process),
                source: (2, 3),
                _keepalive: None,
                trace: None,
            })
        }
    }
    /// Wait up to `timeout` for captured audio, or the whole timeout without
    /// event mode. An idle endpoint sends no events; the timeout keeps the
    /// caller's silence on time.
    pub fn wait(&self, timeout: std::time::Duration) {
        match &self.ready {
            // SAFETY: `event` is the event this struct owns, closed only in Drop.
            Some(event) => unsafe {
                let _ = windows::Win32::System::Threading::WaitForSingleObject(
                    windows::Win32::Foundation::HANDLE(event.as_raw_handle()),
                    timeout.as_millis().clamp(1, u128::from(u32::MAX)) as u32,
                );
            },
            None => std::thread::sleep(timeout),
        }
    }
    pub fn event_driven(&self) -> bool {
        self.ready.is_some()
    }
    /// The endpoint's own channel count and speaker mask, before they are
    /// mixed to the output channels; a mask of 0 means the standard order.
    pub fn source(&self) -> (usize, u32) {
        self.source
    }
    /// How much audio Windows holds between reads; zero when unknown.
    pub fn buffer(&self) -> std::time::Duration {
        self.buffer
    }
    /// The time since the previous call, None on the first. Call it before
    /// reading every captured packet: Windows lost what arrived after the
    /// buffer filled in between.
    pub fn since_drained(&mut self) -> Option<std::time::Duration> {
        let now = std::time::Instant::now();
        self.drained
            .replace(now)
            .map(|at| now.saturating_duration_since(at))
    }
    /// None when no audio arrived since the last call, else how much
    /// Windows skipped before it because it had no room.
    pub fn take_delivered(&mut self) -> Option<std::time::Duration> {
        self.delivered
            .take()
            .map(|frames| std::time::Duration::from_secs_f64(frames as f64 / f64::from(self.rate)))
    }
    /// How much captured audio this host discarded since the last call (a backlog trimmed
    /// after a late wake, or the 100 ms cap), in 48 kHz samples. Windows delivered it, so
    /// `take_delivered` does not count it; it is a gap in the stream all the same.
    pub fn take_trimmed(&mut self) -> u64 {
        self.samples.take_trimmed(self.rate)
    }
    /// What this capture holds that will never be sent because it is being dropped: the
    /// audio it trimmed and the queue the resampler never got to read, in 48 kHz samples.
    pub fn take_discarded(&mut self) -> u64 {
        self.samples.take_trimmed(self.rate) + self.samples.queued(self.rate)
    }
    /// Moves the packets traced since the last call into `into`; nothing without
    /// `audio_cadence_trace`.
    pub fn drain_trace(&mut self, into: &mut butterpollo_media_policy::audio_cadence::Trace) {
        for entry in self.trace.iter_mut().flat_map(|trace| trace.drain(..)) {
            into.capture(entry);
        }
    }
    pub fn read(&mut self, frames: usize) -> Result<Option<Vec<f32>>> {
        let mut out = Vec::new();
        Ok(self.read_into(frames, &mut out)?.then_some(out))
    }
    /// As [`Self::read`], into `out` (emptied first), whose room the audio thread reuses for
    /// every packet: whether a whole packet was there.
    pub fn read_into(&mut self, frames: usize, out: &mut Vec<f32>) -> Result<bool> {
        anyhow::ensure!(
            self.process.as_ref().is_none_or(|process| process.alive()),
            "game audio process exited; waiting for a verified surviving game process"
        );
        // SAFETY: GetBuffer returns `count` frames of `channels` samples of `bits` bits each, read
        // only before ReleaseBuffer; each read is unaligned and inside that packet.
        unsafe {
            loop {
                let n = self.capture.GetNextPacketSize()?;
                if n == 0 {
                    break;
                }
                let mut data = std::ptr::null_mut();
                let mut count = 0;
                let mut flags = 0;
                let mut position = 0;
                let mut qpc = 0;
                self.capture.GetBuffer(
                    &mut data,
                    &mut count,
                    &mut flags,
                    Some(&mut position),
                    Some(&mut qpc),
                )?;
                if let Some(trace) = &mut self.trace {
                    trace.push(butterpollo_media_policy::audio_cadence::Captured {
                        read_ms: crate::os_helpers::qpc_ms(),
                        // GetBuffer reports QPC in 100 ns units.
                        device_qpc_ms: qpc as f64 / 10_000.,
                        position,
                        frames: count,
                        flags,
                    });
                }
                // A packet split at the same position continues the last one.
                let delivered = self.delivered.get_or_insert(0);
                let start = match self.position {
                    Some((last, next)) if last == position => next,
                    Some((_, next)) => {
                        *delivered += position.saturating_sub(next);
                        position
                    }
                    None => position,
                };
                self.position = Some((position, start + u64::from(count)));
                self.samples.push_packet(data, count as usize, flags);
                self.capture.ReleaseBuffer(count)?;
            }
            Ok(self.samples.read_into(frames, out))
        }
    }
}

/// PCM queued across WASAPI packets, independent of the native capture client.
struct CaptureSamples {
    channels: usize,
    bits: u16,
    float: bool,
    resampler: butterpollo_media_policy::audio::Resampler,
    matrix: Vec<Vec<f32>>,
    /// Whether the matrix only copies each channel to its own output, decided
    /// once here rather than per sample: such a capture queues its converted
    /// samples without the mix, and they are clamped already.
    identity: bool,
    input: Vec<f32>,
    output: Vec<f32>,
    /// Frames, at the resampler's input rate, this host discarded from the queue since
    /// they were last taken: audio Windows delivered and nothing sent.
    trimmed: u64,
}
impl CaptureSamples {
    /// The audio discarded since the last call, as 48 kHz samples of a device running at `rate`.
    fn take_trimmed(&mut self, rate: u32) -> u64 {
        std::mem::take(&mut self.trimmed) * 48_000 / u64::from(rate)
    }
    /// The audio queued and not yet read, as 48 kHz samples of a device running at `rate`.
    fn queued(&self, rate: u32) -> u64 {
        self.resampler.queued() as u64 * 48_000 / u64::from(rate)
    }
    /// # Safety
    /// Unless silent or null, `data` holds `count` interleaved frames in this
    /// capture's validated sample format and remains readable throughout.
    unsafe fn push_packet(&mut self, data: *const u8, count: usize, flags: u32) {
        if count == 0 {
            return;
        }
        // DATA_DISCONTINUITY marks a stream-position discontinuity, not invalid
        // earlier samples. Clearing them here also erased whole packets drained
        // earlier in this read, or a partial Opus packet from the previous read.
        // Keep those samples and the fractional resampling phase; read() still
        // bounds the entire backlog.
        let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
        if silent || data.is_null() {
            self.trimmed += self.resampler.push_silence(count) as u64;
            return;
        }
        let total = count * self.channels;
        self.input.resize(total, 0.);
        // SAFETY: `data` holds `total` samples in `(self.float, self.bits)`.
        unsafe { convert_samples(data, self.float, self.bits, &mut self.input) };
        if self.identity {
            // Converted samples are already clamped and match output channels 1:1.
            self.trimmed += self.resampler.push_slice_clamped(&self.input) as u64;
        } else {
            let out_channels = self.matrix.len();
            self.output.resize(count * out_channels, 0.);
            for (src, dst) in self
                .input
                .chunks_exact(self.channels)
                .zip(self.output.chunks_exact_mut(out_channels))
            {
                for (dest, row) in dst.iter_mut().zip(&self.matrix) {
                    let mixed: f32 = src.iter().zip(row).map(|(v, gain)| v * gain).sum();
                    *dest = clamp_sample(mixed);
                }
            }
            self.trimmed += self.resampler.push_slice_clamped(&self.output) as u64;
        }
    }
    #[cfg(test)]
    fn read(&mut self, frames: usize) -> Option<Vec<f32>> {
        let mut out = Vec::new();
        self.read_into(frames, &mut out).then_some(out)
    }
    /// One packet of `frames` into `out` (emptied first), when that much is queued.
    fn read_into(&mut self, frames: usize, out: &mut Vec<f32>) -> bool {
        // Steady state queues under one WASAPI period plus a packet;
        // keep two packets once a stall left more than 30 ms.
        let packet_ms = (frames / 48).max(1) as u32;
        let dropped = self.resampler.bound((packet_ms * 3).max(30), packet_ms * 2);
        if dropped > 0 {
            self.trimmed += dropped as u64;
            tracing::debug!("audio capture backlog dropped");
        }
        self.resampler.read_into(frames, out)
    }
}
#[inline]
fn clamp_sample(v: f32) -> f32 {
    if v.is_finite() { v.clamp(-1., 1.) } else { 0. }
}

/// Converts `out.len()` interleaved WASAPI samples in format `(float, bits)`
/// into clamped `f32` values with the format branch hoisted outside the loop.
///
/// # Safety
/// `data` must point to at least `out.len() * (bits / 8)` readable bytes.
unsafe fn convert_samples(data: *const u8, float: bool, bits: u16, out: &mut [f32]) {
    // SAFETY: each unaligned read stays within the caller's `out.len()` samples.
    unsafe {
        match (float, bits) {
            (true, _) => {
                let src = data.cast::<f32>();
                for (i, d) in out.iter_mut().enumerate() {
                    *d = clamp_sample(std::ptr::read_unaligned(src.add(i)));
                }
            }
            (false, 16) => {
                let src = data.cast::<i16>();
                for (i, d) in out.iter_mut().enumerate() {
                    *d = (std::ptr::read_unaligned(src.add(i)) as f32 / 32768.).clamp(-1., 1.);
                }
            }
            (false, 24) => {
                for (i, d) in out.iter_mut().enumerate() {
                    let p = data.add(i * 3);
                    let v =
                        ((*p as i32) | ((*p.add(1) as i32) << 8) | ((*p.add(2) as i32) << 16)) << 8;
                    *d = (v as f32 / 2147483648.).clamp(-1., 1.);
                }
            }
            (false, _) => {
                let src = data.cast::<i32>();
                for (i, d) in out.iter_mut().enumerate() {
                    *d = (std::ptr::read_unaligned(src.add(i)) as f32 / 2147483648.).clamp(-1., 1.);
                }
            }
        }
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        // SAFETY: `self.client` is live. The owned event closes after capture stops.
        unsafe {
            let _ = self.client.Stop();
        }
    }
}
/// The most an Opus packet may take: room for RTP, AES padding and parity headers is left
/// within the previous Moonlight receiver's fixed 1400-byte UDP buffer.
const OPUS_PACKET_ROOM: usize = 1360;
type Create = unsafe extern "C" fn(i32, i32, i32, i32, *const u8, i32, *mut i32) -> *mut c_void;
type Encode = unsafe extern "C" fn(*mut c_void, *const f32, i32, *mut u8, i32) -> i32;
type Destroy = unsafe extern "C" fn(*mut c_void);
type Ctl = unsafe extern "C" fn(*mut c_void, i32, ...) -> i32;
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
        // High-quality surround at 10/20ms otherwise exceeds Moonlight's
        // fixed UDP buffer. Opus can silently starve the last coded stream
        // when only its output buffer is capped. Bound CBR before encoding,
        // reserving space for each self-delimited stream's length fields.
        let packet_bitrate = (1360 - layout.streams * 4) * 8000 / i32::from(packet_ms);
        let bitrate = layout.bitrate.min(packet_bitrate);
        // SAFETY: the symbol types match libopus, `layout.mapping` has `channels` entries, and
        // `library` is kept in `Self` with its function pointers.
        unsafe {
            let path = directory.join("libopus-0.dll");
            let library = libloading::Library::new(&path)
                .with_context(|| format!("loading {}", path.display()))?;
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
        // SAFETY: `self.state` is a live encoder, `samples` holds whole frames for `channels`, and
        // `self.output` is writable for the length passed.
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
        // SAFETY: `self.state` came from opus_multistream_encoder_create and is destroyed once,
        // here.
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
        // SAFETY: the symbol types match libopus, and `library` is kept in `Self` with its
        // function pointers.
        unsafe {
            let path = directory.join("libopus-0.dll");
            let library = libloading::Library::new(&path)
                .with_context(|| format!("loading {}", path.display()))?;
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
    /// Appends a frame to `out` and returns its length: `packet` decoded,
    /// the frame before it from its in-band FEC when `fec` is set, or a
    /// concealed frame when there is no packet.
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
        // SAFETY: `self.state` is a live decoder, `data` is null or readable for `length` bytes,
        // and `out` has room for `frames` mono samples after `start`.
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

#[cfg(test)]
mod tests;
