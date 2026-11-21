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
    /// The renderer generation it was made under (`generation`).
    generation: u64,
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

    /// Keeps `decoder` for the next stream, replacing one parked before (the tests' shorthand).
    #[cfg(test)]
    pub fn park(&self, key: K, decoder: D) {
        self.park_named(key, decoder, None, generation());
    }

    /// Keeps `decoder`, made under renderer generation `made_under`, unless the renderer it was
    /// made for is gone since: then it is dropped here (the caller's thread, as before the cache).
    /// The check is under the lock `clear` takes after it bumps the generation, so a decoder
    /// cannot be parked after the clear that should have taken it.
    pub fn park_named(&self, key: K, decoder: D, name: Option<String>, made_under: u64) {
        self.park_at(key, decoder, name, made_under, std::time::Instant::now());
    }

    fn park_at(&self, key: K, decoder: D, name: Option<String>, made_under: u64, at: std::time::Instant) {
        let mut parked = self.parked.lock().unwrap_or_else(|poison| poison.into_inner());
        // Every drop below runs under the lock, so a `clear` waits for a drop in progress. A
        // decoder's `Drop` never calls into the cache.
        if made_under != generation() {
            drop(decoder);
            return;
        }
        let replaced = parked.replace(Parked { key, decoder, name, generation: made_under, at });
        drop(replaced);
    }

    /// A decoder that waited longer than [`KEEP_FOR`] before its stream, for the tests of that.
    #[cfg(test)]
    pub fn park_aged(&self, key: K, decoder: D, age: std::time::Duration) {
        let at = std::time::Instant::now().checked_sub(age).unwrap_or_else(std::time::Instant::now);
        self.park_at(key, decoder, None, generation(), at);
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
                // Dropped under the lock: a `clear` (before the renderer's device goes) waits for it.
                let old = parked.take();
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
        if entry.at.elapsed() >= KEEP_FOR || entry.generation != generation() {
            let stale = parked.take();
            drop(stale);
            return None;
        }
        if !usable(&entry.key) {
            // One session runs at a time, so a decoder that does not serve this stream serves no
            // later one: its pool, textures and device go now, not beside the new decoder's.
            let unused = parked.take();
            drop(unused);
            return None;
        }
        let entry = parked.take()?;
        Some((entry.decoder, entry.name))
    }

    /// Drops the parked decoder, if any, on the calling thread. A drop another thread has in
    /// progress (the timer's, a `take`'s) holds the lock, so this returns only once it is done.
    pub fn clear(&self) {
        let mut parked = self.parked.lock().unwrap_or_else(|poison| poison.into_inner());
        let old = parked.take();
        drop(old);
    }
}

impl<K: Send + 'static, D: Send + 'static> Cache<K, D> {
    /// Drops the parked decoder, if any, on a helper thread, and says whether that finished by
    /// `deadline`: the wait for a drop in progress (the timer's, a `take`'s) and the drop itself
    /// both count. When it did not, the decoder may still hold the renderer's device: the caller
    /// leaves the device alone. The drop goes on in the helper.
    pub fn clear_until(&self, deadline: std::time::Instant) -> bool {
        let old = loop {
            match self.parked.try_lock() {
                Ok(mut parked) => break parked.take(),
                Err(std::sync::TryLockError::Poisoned(poison)) => break poison.into_inner().take(),
                Err(std::sync::TryLockError::WouldBlock) => {
                    if std::time::Instant::now() >= deadline {
                        eprintln!("nightjar: a parked decoder is still being dropped at the deadline");
                        return false;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            }
        };
        let Some(old) = old else { return true };
        let (done, finished) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("decoder-clear".into()).spawn(move || {
            drop(old);
            let _ = done.send(());
        });
        if spawned.is_err() {
            eprintln!("nightjar: no thread to drop the parked decoder on");
            return false;
        }
        let stopped = finished.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())).is_ok();
        if !stopped {
            eprintln!("nightjar: a parked decoder did not drop in time; its GPU device stays");
        }
        stopped
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
pub(super) fn park_media(key: MediaKey, decoder: VideoDecoder, made_under: u64) {
    with_media(|cache| cache.park_named(key, decoder, None, made_under));
    #[cfg(not(test))]
    expire_media_later();
}

/// A stream that decodes with FFmpeg is starting: a parked PyroWave decoder is of the other kind
/// of stream, which never takes it, so it goes now.
pub(super) fn only_media() {
    with_pyrowave(Cache::clear);
}

/// A PyroWave stream is starting: a parked FFmpeg decoder goes, as above.
pub(super) fn only_pyrowave() {
    with_media(Cache::clear);
}

/// The renderer generation: it counts the renderers whose GPU device is going. A decoder is made
/// under the generation the session started in and parked only while it is still the current
/// one, so a session that stops after its renderer was torn down (a launch's thread joining the
/// last session's teardown) cannot park a decoder that borrows or holds the old device.
#[cfg(not(test))]
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[cfg(test)]
std::thread_local! {
    static GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The current renderer generation.
pub(super) fn generation() -> u64 {
    #[cfg(not(test))]
    { GENERATION.load(std::sync::atomic::Ordering::SeqCst) }
    #[cfg(test)]
    { GENERATION.with(std::cell::Cell::get) }
}

/// The renderer's GPU device is going: no decoder made so far may be parked from now on.
pub fn retire() {
    #[cfg(not(test))]
    { GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst); }
    #[cfg(test)]
    { GENERATION.with(|generation| generation.set(generation.get() + 1)); }
}

/// Retires the generation and drops both parked decoders on a helper thread, within `deadline`.
/// The renderer's GPU device must outlive them (a Vulkan video or PyroWave decoder borrows it, a
/// Direct3D 11 one holds a reference to it), so every place that destroys the renderer calls
/// this first. False when a drop did not finish: the device must then stay, as for a teardown
/// that did not end.
#[must_use]
pub fn clear(deadline: std::time::Instant) -> bool {
    retire();
    let media = with_media(|cache| cache.clear_until(deadline));
    let pyrowave = with_pyrowave(|cache| cache.clear_until(deadline));
    media && pyrowave
}

/// Whether either kind of decoder is parked (the tests of `clear`).
#[cfg(test)]
pub(super) fn any_parked() -> bool {
    media_parked() || with_pyrowave(|cache| cache.is_parked())
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
pub(super) fn park_pyrowave(key: PyroWaveKey, decoder: Pyrowave, name: String, made_under: u64) {
    with_pyrowave(|cache| cache.park_named(key, decoder, Some(name), made_under));
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

    /// A decoder the timer is dropping is still alive for the device it holds: `clear`, which runs
    /// before the renderer is destroyed, waits for that drop instead of finding the cache empty.
    #[test]
    fn clear_waits_for_a_decoder_the_timer_is_dropping() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        struct Slow(Arc<AtomicBool>, Arc<AtomicBool>);
        impl Drop for Slow {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(300));
                self.1.store(true, Ordering::SeqCst);
            }
        }
        let (started, done) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let cache: &'static Cache<u8, Slow> = Box::leak(Box::new(Cache::new()));
        cache.park_aged(0, Slow(started.clone(), done.clone()), KEEP_FOR + std::time::Duration::from_secs(1));
        cache.expire_later();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !started.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(started.load(Ordering::SeqCst), "the timer starts dropping the decoder");
        assert!(!done.load(Ordering::SeqCst));
        assert!(cache.clear_until(std::time::Instant::now() + std::time::Duration::from_secs(5)));
        assert!(done.load(Ordering::SeqCst), "clear returns only after the drop in progress finished");
    }

    /// A drop that does not end by the deadline is reported, whether the timer or `clear` itself
    /// is in it, and the decoder is gone once it does end.
    #[test]
    fn clear_reports_a_drop_that_misses_the_deadline() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        struct Slow(Arc<AtomicBool>);
        impl Drop for Slow {
            fn drop(&mut self) {
                std::thread::sleep(std::time::Duration::from_millis(400));
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let short = || std::time::Instant::now() + std::time::Duration::from_millis(50);
        let done = Arc::new(AtomicBool::new(false));
        let cache: Cache<u8, Slow> = Cache::new();
        cache.park_aged(0, Slow(done.clone()), std::time::Duration::ZERO);
        assert!(!cache.clear_until(short()), "a slow drop is reported");
        assert!(!done.load(Ordering::SeqCst), "and not waited for");
        assert!(!cache.is_parked(), "the cache is empty meanwhile");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !done.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(done.load(Ordering::SeqCst), "the drop goes on in the helper");
        // A drop in progress under the lock is reported too.
        let cache: &'static Cache<u8, Slow> = Box::leak(Box::new(Cache::new()));
        let done = Arc::new(AtomicBool::new(false));
        cache.park_aged(0, Slow(done.clone()), KEEP_FOR + std::time::Duration::from_secs(1));
        cache.expire_later();
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(!cache.clear_until(short()), "waiting for the timer's drop is bounded");
        assert!(cache.clear_until(std::time::Instant::now() + std::time::Duration::from_secs(5)));
        assert!(done.load(Ordering::SeqCst));
    }

    #[test]
    fn a_new_decoder_replaces_the_one_parked_before() {
        let cache: Cache<MediaKey, VideoDecoder> = Cache::new();
        let first = media(0x0100, 1920, 1080);
        let second = media(0x0100, 3840, 2160);
        cache.park(first, decoder());
        cache.park(second, decoder());
        assert!(cache.take(|parked| parked.serves(&second)).is_some(), "the newer stream's decoder is the one kept");
        // Only the older one parked: it does not serve the newer stream, and one session runs at
        // a time, so it serves no later stream either and is dropped, not kept beside the new one.
        cache.park(first, decoder());
        assert!(cache.take(|parked| parked.serves(&second)).is_none(), "the older stream's decoder does not serve the newer stream");
        assert!(!cache.is_parked(), "and is released at once, with its pool and device");
        assert!(cache.take(|parked| parked.serves(&first)).is_none());
    }

    #[test]
    fn clear_empties_both_caches_and_a_stream_of_the_other_kind_drops_the_parked_one() {
        let _serial = serialize();
        park_media(media(0x0100, 640, 360), decoder(), generation());
        park_pyrowave(pyro(64, 64, false, 8), Pyrowave::cpu(64, 64, false).unwrap(), "PyroWave".into(), generation());
        assert!(media_parked() && any_parked());
        assert!(clear(std::time::Instant::now() + std::time::Duration::from_secs(10)));
        assert!(!any_parked(), "nothing outlives the renderer's device");
        assert!(!media_parked());
        park_media(media(0x0100, 640, 360), decoder(), generation());
        park_pyrowave(pyro(64, 64, false, 8), Pyrowave::cpu(64, 64, false).unwrap(), "PyroWave".into(), generation());
        only_media();
        assert!(media_parked() && !with_pyrowave(|cache| cache.is_parked()), "an FFmpeg stream drops the PyroWave decoder at once");
        park_pyrowave(pyro(64, 64, false, 8), Pyrowave::cpu(64, 64, false).unwrap(), "PyroWave".into(), generation());
        only_pyrowave();
        assert!(!media_parked() && with_pyrowave(|cache| cache.is_parked()), "a PyroWave stream drops the FFmpeg decoder at once");
        assert!(clear(std::time::Instant::now() + std::time::Duration::from_secs(10)));
    }

    /// A session that stops after its renderer was torn down parks nothing, and a decoder parked
    /// before it goes with the renderer: neither can outlive the GPU device it borrows or holds.
    #[test]
    fn a_decoder_made_for_a_renderer_that_is_gone_is_not_parked_or_taken() {
        let cache: Cache<MediaKey, VideoDecoder> = Cache::new();
        let key = media(0x0100, 800, 600);
        let made_under = generation();
        retire();
        cache.park_named(key, decoder(), None, made_under);
        assert!(!cache.is_parked(), "refused: the renderer it was made for is gone");
        cache.park_named(key, decoder(), None, generation());
        assert!(cache.is_parked(), "a decoder of the current renderer is kept");
        retire();
        assert!(cache.take(|parked| parked.serves(&key)).is_none(), "and not served once its renderer is gone");
        assert!(!cache.is_parked(), "but dropped at once");
    }

    fn pyro(width: usize, height: usize, ten_bit: bool, slots: usize) -> PyroWaveKey {
        PyroWaveKey { width, height, chroma444: false, ten_bit, gpu: false, device: 0, slots }
    }

    #[test]
    fn a_pyrowave_decoder_is_kept_only_for_the_same_picture_and_device() {
        let cache: Cache<PyroWaveKey, Pyrowave> = Cache::new();
        let key = pyro(3840, 2160, true, 8);
        cache.park_named(key, Pyrowave::cpu(64, 64, false).expect("a small CPU decoder"), Some("PyroWave (Rust, GPU wavelets)".into()), generation());
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
        cache.park_named(key, Pyrowave::cpu(64, 64, false).unwrap(), Some("named".into()), generation());
        let (_, name) = cache.take(|parked| *parked == key).expect("the same stream reuses the decoder");
        assert_eq!(name.as_deref(), Some("named"), "the decoder line keeps saying what it decoded with");
    }
}
