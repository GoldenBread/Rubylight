//! The decoder a stream hands to the session that follows it.
//!
//! A resume or reconnect opens a new session, and with it a decoder: a new codec context, a new
//! hardware device where the backend makes one (VA-API, Vulkan, VideoToolbox), a new frames pool,
//! and PyroWave's SDK decoder with its textures and fences. When the stream that just ended will
//! be followed by a reconnect (`Shared::keep_decoder`) and its decoder never failed, `cleanup`
//! parks the decoder here instead of dropping it; the next session's `setup` takes it back when
//! the stream is the same, and flushes it so no picture of the last stream can be shown. A
//! parameter that changed, or a decoder that failed, is not reused: the stream opens its own.
//!
//! One decoder of each kind waits for the next stream; a new one takes its place, so nothing of
//! an older stream is kept beside it.

use std::sync::Mutex;

use super::pyrowave::Pyrowave;
use crate::media::{Codec, Hardware, Pictures, VideoDecoder};

/// What an FFmpeg decoder was opened for: the stream's parameters and the renderer device the
/// backend decodes on, so a stream that differs in any of them opens its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct MediaKey {
    pub codec: Codec,
    /// The backend as the stream asked for it, a renderer's device included: D3D11's, Vulkan
    /// video's, VA-API's node.
    pub requested: Hardware,
    /// The backend the decoder opened with (`VideoDecoder::hardware`), which is in the requested
    /// order and used to check the parked decoder against the next stream's offer. Ignored when
    /// the caller asks [`MediaKey::serves`].
    pub opened: Hardware,
    pub format: u32,
    pub width: i32,
    pub height: i32,
    pub chroma_packing: u8,
    pub pictures: Pictures,
    /// The slice threads a software decoder was opened with.
    pub threads: i32,
}

impl MediaKey {
    /// Whether a decoder opened for `self` serves a stream for `request`: the same codec, format,
    /// picture, parameters and threads. The backend that opened is checked with the stream's
    /// order instead ([`take_media`]).
    pub fn serves(&self, request: &MediaKey) -> bool {
        self.codec == request.codec
            && self.requested == request.requested
            && self.format == request.format
            && self.width == request.width
            && self.height == request.height
            && self.chroma_packing == request.chroma_packing
            && self.pictures == request.pictures
            && self.threads == request.threads
    }
}

/// What a PyroWave decoder was opened for: its picture, the device its GPU decoder renders on and
/// the plane sets its pictures wait in. Its parameters never change within a stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PyroWaveKey {
    pub width: usize,
    pub height: usize,
    pub chroma444: bool,
    pub ten_bit: bool,
    /// The GPU decoder on the renderer's device, rather than the processor's.
    pub gpu: bool,
    /// The renderer's device for a GPU decoder (0 where it makes its own).
    pub device: usize,
    pub slots: usize,
}

/// One parked decoder with what it was opened for.
struct Parked<K, D> {
    key: K,
    decoder: D,
    /// The name the overlay's decoder line showed for it, as a PyroWave decoder makes its own.
    name: Option<String>,
    /// When it was parked: a decoder waits for the reconnect that follows its own stream, and
    /// no longer.
    at: std::time::Instant,
}

/// How long a parked decoder waits for the stream that follows its own. A reconnect follows
/// within seconds; a stream that differs in its parameters leaves it (`Cache::take`), and one
/// that starts later than this drops it rather than keep its device for a reconnect never made.
const KEEP_FOR: std::time::Duration = std::time::Duration::from_secs(60);

/// A place for one decoder of a kind.
pub(super) struct Cache<K, D> {
    parked: Mutex<Option<Parked<K, D>>>,
}

impl<K, D> Cache<K, D> {
    pub const fn new() -> Self {
        Self { parked: Mutex::new(None) }
    }

    /// Keeps `decoder` for the next stream, replacing one parked before.
    pub fn park(&self, key: K, decoder: D) {
        self.park_named(key, decoder, None);
    }

    pub fn park_named(&self, key: K, decoder: D, name: Option<String>) {
        self.park_at(key, decoder, name, std::time::Instant::now());
    }

    fn park_at(&self, key: K, decoder: D, name: Option<String>, at: std::time::Instant) {
        let replaced = self.parked.lock().unwrap_or_else(|poison| poison.into_inner())
            .replace(Parked { key, decoder, name, at });
        drop(replaced);
    }

    /// A decoder that waited longer than [`KEEP_FOR`] before its stream, for the tests of that.
    #[cfg(test)]
    pub fn park_aged(&self, key: K, decoder: D, age: std::time::Duration) {
        let at = std::time::Instant::now().checked_sub(age).unwrap_or_else(std::time::Instant::now);
        self.park_at(key, decoder, None, at);
    }

    /// Drops the parked decoder once it waited longer than [`KEEP_FOR`]: the reconnect it was kept
    /// for is over (a stream of the other kind may follow, which never takes it).
    pub fn drop_if_stale(&self) {
        let mut parked = self.parked.lock().unwrap_or_else(|poison| poison.into_inner());
        if parked.as_ref().is_some_and(|entry| entry.at.elapsed() >= KEEP_FOR) {
            let old = parked.take();
            drop(parked);
            drop(old);
        }
    }

    /// Drops the parked decoder after [`KEEP_FOR`] even if no stream starts: the timer of a parked
    /// one. A decoder parked later replaces this entry, and this timer leaves the newer one be.
    #[cfg_attr(test, allow(dead_code))] // Unit tests have no launcher idling; they call take or park_aged.
    pub fn expire_later(&'static self)
    where
        K: Send,
        D: Send,
    {
        let at = {
            let parked = self.parked.lock().unwrap_or_else(|poison| poison.into_inner());
            match parked.as_ref() {
                Some(entry) => entry.at,
                None => return,
            }
        };
        let spawned = std::thread::Builder::new().name("decoder cache expiry".into()).spawn(move || {
            std::thread::sleep(KEEP_FOR.saturating_sub(at.elapsed()));
            let mut parked = self.parked.lock().unwrap_or_else(|poison| poison.into_inner());
            if parked.as_ref().is_some_and(|entry| entry.at == at) {
                let old = parked.take();
                drop(parked);
                drop(old);
            }
        });
        if let Err(error) = spawned {
            eprintln!("nightjar: the decoder cache timer cannot start: {error}");
        }
    }

    /// Whether a decoder is parked (the tests of what is kept).
    #[cfg(test)]
    pub fn is_parked(&self) -> bool {
        self.parked.lock().unwrap_or_else(|poison| poison.into_inner()).is_some()
    }

    /// The parked decoder when `usable` accepts what it was opened for. One that is not is left
    /// for the stream it serves; one parked longer than [`KEEP_FOR`] is dropped, used by no stream.
    pub fn take(&self, usable: impl FnOnce(&K) -> bool) -> Option<(D, Option<String>)> {
        let mut parked = self.parked.lock().unwrap_or_else(|poison| poison.into_inner());
        let entry = parked.as_ref()?;
        if entry.at.elapsed() >= KEEP_FOR {
            let stale = parked.take();
            drop(parked);
            drop(stale);
            return None;
        }
        if !usable(&entry.key) {
            return None;
        }
        let entry = parked.take()?;
        Some((entry.decoder, entry.name))
    }
}

/// The FFmpeg decoder of the last stream that may be followed by a reconnect. Unit tests keep
/// their own: a test parses a stream the way the app does, and the process's cache would make the
/// tests depend on each other's order.
#[cfg(not(test))]
static MEDIA: Cache<MediaKey, VideoDecoder> = Cache::new();
#[cfg(not(test))]
static PYROWAVE: Cache<PyroWaveKey, Pyrowave> = Cache::new();
#[cfg(test)]
std::thread_local! {
    static MEDIA: Cache<MediaKey, VideoDecoder> = const { Cache::new() };
    static PYROWAVE: Cache<PyroWaveKey, Pyrowave> = const { Cache::new() };
}

/// Runs `f` on this thread's FFmpeg cache (the process's, outside tests).
fn with_media<R>(f: impl FnOnce(&Cache<MediaKey, VideoDecoder>) -> R) -> R {
    #[cfg(not(test))]
    {
        f(&MEDIA)
    }
    #[cfg(test)]
    {
        MEDIA.with(f)
    }
}

/// Runs `f` on this thread's PyroWave cache, as [`with_media`].
fn with_pyrowave<R>(f: impl FnOnce(&Cache<PyroWaveKey, Pyrowave>) -> R) -> R {
    #[cfg(not(test))]
    {
        f(&PYROWAVE)
    }
    #[cfg(test)]
    {
        PYROWAVE.with(f)
    }
}

/// How many streams took a decoder parked by the stream before
/// (`a_reconnect_takes_the_decoder_the_stream_before_kept`).
#[cfg(test)]
pub(super) static TAKEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// The same for PyroWave (`a_reconnect_takes_the_pyrowave_decoder_the_stream_before_kept`).
#[cfg(test)]
pub(super) static PYROWAVE_TAKEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Tests that park a decoder and count what the next stream takes run one at a time: the counters
/// are the process's, as in the app, while each test thread has its own cache (`cfg(test)` above).
#[cfg(test)]
pub(super) fn serialize() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Keeps an FFmpeg decoder for the stream that follows this one.
pub(super) fn park_media(key: MediaKey, decoder: VideoDecoder) {
    with_media(|cache| cache.park(key, decoder));
    #[cfg(not(test))]
    expire_media_later();
}

/// A stream that decodes with FFmpeg is starting: a PyroWave decoder parked longer than a
/// reconnect could take is dropped with it (one stream does not change codec across one).
pub(super) fn only_media() {
    with_pyrowave(|cache| cache.drop_if_stale());
}

/// A PyroWave stream is starting: an FFmpeg decoder parked longer than a reconnect could take is
/// dropped with it, as above.
pub(super) fn only_pyrowave() {
    with_media(|cache| cache.drop_if_stale());
}

/// The parked FFmpeg decoder when it serves `key` and one of the backends `order` opens now: a
/// setting that turned a backend off in between is not served by the decoder it parked.
pub(super) fn take_media(key: MediaKey, order: &[Hardware]) -> Option<VideoDecoder> {
    let (decoder, _) = with_media(|cache| {
        cache.take(|parked| {
            parked.serves(&key)
                && if key.requested == Hardware::None { parked.opened == Hardware::None } else { order.contains(&parked.opened) }
        })
    })?;
    #[cfg(test)]
    TAKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Some(decoder)
}

/// Keeps a PyroWave decoder, with the name the decoder line showed, for the stream that follows.
pub(super) fn park_pyrowave(key: PyroWaveKey, decoder: Pyrowave, name: String) {
    with_pyrowave(|cache| cache.park_named(key, decoder, Some(name)));
    #[cfg(not(test))]
    expire_pyrowave_later();
}

/// The parked PyroWave decoder when it was opened for exactly this stream.
pub(super) fn take_pyrowave(key: PyroWaveKey) -> Option<(Pyrowave, String)> {
    let (decoder, name) = with_pyrowave(|cache| cache.take(|parked| *parked == key))?;
    #[cfg(test)]
    PYROWAVE_TAKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Some((decoder, name.unwrap_or_else(|| "PyroWave".to_owned())))
}

/// Drops a decoder parked longer than [`KEEP_FOR`] even when no stream starts: the app's reconnect
/// follows within seconds, and after that the idle launcher must not keep a GPU device, a frames
/// pool or PyroWave's textures and fences for a stream that never comes.
#[cfg(not(test))]
fn expire_media_later() {
    MEDIA.expire_later();
}

/// [`expire_media_later`] for PyroWave.
#[cfg(not(test))]
fn expire_pyrowave_later() {
    PYROWAVE.expire_later();
}

/// Whether an FFmpeg decoder waits for the next stream (the tests of what is kept).
#[cfg(test)]
pub(super) fn media_parked() -> bool {
    with_media(|cache| cache.is_parked())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::VaapiNode;

    fn media(format: u32, width: i32, height: i32) -> MediaKey {
        MediaKey {
            codec: Codec::Hevc,
            requested: Hardware::None,
            opened: Hardware::None,
            format,
            width,
            height,
            chroma_packing: 0,
            pictures: Pictures { ten_bit: true, chroma444: false, packed: false, waiting: 8, vaapi_node: VaapiNode::Default },
            threads: 4,
        }
    }

    fn decoder() -> VideoDecoder {
        VideoDecoder::open(Codec::H264, Hardware::None, 1, false).expect("a software H.264 decoder")
    }

    #[test]
    fn the_same_stream_takes_the_parked_decoder_back_and_a_changed_one_does_not() {
        let cache: Cache<MediaKey, VideoDecoder> = Cache::new();
        let key = media(0x0100, 3840, 2160);
        cache.park(key, decoder());
        // Same codec, format, size, pictures and threads: reused.
        assert!(cache.take(|parked| parked.serves(&key)).is_some(), "the same stream reuses the decoder");
        // Nothing is parked any more, so the next stream opens its own.
        assert!(cache.take(|parked| parked.serves(&key)).is_none());
        // A stream that differs in one parameter is not served by it.
        for changed in [
            media(0x0100, 1920, 1080),
            media(0x0200, 3840, 2160),
            MediaKey { codec: Codec::H264, ..key },
            MediaKey { chroma_packing: 1, ..key },
            MediaKey { pictures: Pictures { ten_bit: false, ..key.pictures }, ..key },
            MediaKey { threads: 8, ..key },
            MediaKey { requested: Hardware::D3d11(0x3000 as *mut std::ffi::c_void), ..key },
        ] {
            assert!(!key.serves(&changed));
            cache.park(key, decoder());
            assert!(cache.take(|parked| parked.serves(&changed)).is_none(), "a changed stream does not reuse the decoder");
        }
    }

    #[test]
    fn a_backend_the_stream_no_longer_offers_is_not_reused() {
        let cache: Cache<MediaKey, VideoDecoder> = Cache::new();
        let vulkan = Hardware::Vulkan(0x1000 as *mut std::ffi::c_void);
        let key = MediaKey { requested: Hardware::Auto(0x2000 as *mut std::ffi::c_void, None), opened: vulkan, ..media(0x0100, 3840, 2160) };
        cache.park(key, decoder());
        // The stream still asks for the same order: Vulkan video serves it.
        assert!(cache.take(|parked| parked.serves(&key) && [vulkan].contains(&parked.opened)).is_some());
        // Vulkan video was turned off between the streams: the VA-API order does not contain it.
        cache.park(key, decoder());
        assert!(cache.take(|parked| parked.serves(&key) && [Hardware::Vaapi].contains(&parked.opened)).is_none());
    }

    #[test]
    fn a_decoder_that_waited_longer_than_the_reconnect_takes_is_dropped() {
        let cache: Cache<MediaKey, VideoDecoder> = Cache::new();
        let key = media(0x0100, 2560, 1440);
        cache.park(key, decoder());
        assert!(cache.take(|parked| parked.serves(&key)).is_some(), "a reconnect follows its stream at once");
        cache.park_aged(key, decoder(), KEEP_FOR + std::time::Duration::from_secs(1));
        assert!(cache.take(|parked| parked.serves(&key)).is_none(), "a decoder parked for longer is not reused");
        assert!(cache.take(|parked| parked.serves(&key)).is_none(), "and is gone");
    }

    /// No stream follows a reconnect that failed: the park starts a timer that releases the
    /// decoder, and with it a 4K frames pool or PyroWave's textures, without one.
    #[test]
    fn the_timer_drops_a_decoder_no_stream_takes() {
        let cache: &'static Cache<MediaKey, VideoDecoder> = Box::leak(Box::new(Cache::new()));
        let key = media(0x0100, 3200, 1800);
        cache.park_aged(key, decoder(), KEEP_FOR + std::time::Duration::from_secs(1));
        cache.expire_later();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while cache.is_parked() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!cache.is_parked(), "the timer drops the decoder the reconnect never took");
    }

    #[test]
    fn a_new_decoder_replaces_the_one_parked_before() {
        let cache: Cache<MediaKey, VideoDecoder> = Cache::new();
        let first = media(0x0100, 1920, 1080);
        let second = media(0x0100, 3840, 2160);
        cache.park(first, decoder());
        cache.park(second, decoder());
        assert!(cache.take(|parked| parked.serves(&second)).is_some(), "the newer stream's decoder is the one kept");
        // Only the older one parked: it does not serve the newer stream, and stays for its own.
        cache.park(first, decoder());
        assert!(cache.take(|parked| parked.serves(&second)).is_none(), "the older stream's decoder does not serve the newer stream");
        assert!(cache.take(|parked| parked.serves(&first)).is_some(), "and is still there for the stream it was opened for");
    }

    fn pyro(width: usize, height: usize, ten_bit: bool, slots: usize) -> PyroWaveKey {
        PyroWaveKey { width, height, chroma444: false, ten_bit, gpu: false, device: 0, slots }
    }

    #[test]
    fn a_pyrowave_decoder_is_kept_only_for_the_same_picture_and_device() {
        let cache: Cache<PyroWaveKey, Pyrowave> = Cache::new();
        let key = pyro(3840, 2160, true, 8);
        cache.park_named(key, Pyrowave::cpu(64, 64, false).expect("a small CPU decoder"), Some("PyroWave (Rust, GPU wavelets)".into()));
        for changed in [
            pyro(1920, 1080, true, 8),
            pyro(3840, 2160, false, 8),
            pyro(3840, 2160, true, 6),
            PyroWaveKey { gpu: true, device: 0x1000, ..key },
            PyroWaveKey { chroma444: true, ..key },
        ] {
            assert_ne!(changed, key);
            cache.park(key, Pyrowave::cpu(64, 64, false).unwrap());
            assert!(cache.take(|parked| *parked == changed).is_none(), "a changed picture or device does not reuse the decoder");
        }
        cache.park_named(key, Pyrowave::cpu(64, 64, false).unwrap(), Some("named".into()));
        let (_, name) = cache.take(|parked| *parked == key).expect("the same stream reuses the decoder");
        assert_eq!(name.as_deref(), Some("named"), "the decoder line keeps saying what it decoded with");
    }
}
