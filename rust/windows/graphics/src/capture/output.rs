//! Current colour information for one selected capture output. DXGI descriptions
//! belong to a factory snapshot: GetDesc1 on an old output can stay unchanged
//! after Windows changes HDR or display luminance.
//! https://learn.microsoft.com/windows/win32/api/dxgi1_6/nf-dxgi1_6-idxgioutput6-getdesc1

use super::*;
use std::sync::{Arc, Mutex, Weak};
use std::thread;

const CHECK_INTERVAL: Duration = Duration::from_secs(1);
/// Enumerations per poll while the display topology keeps changing, and the
/// wait before trying again when it is still changing.
const REFRESH_ATTEMPTS: usize = 3;
const REFRESH_RETRY: Duration = Duration::from_millis(50);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Identity {
    adapter: (u32, i32),
    name: String,
    monitor: usize,
}

fn bind_identity(expected: &str, identity: Identity, attached: bool) -> Result<Identity> {
    // Display selection and device creation enumerate separately. If their
    // ordinals changed in between, refuse the newly resolved wrong monitor.
    if !attached || identity.monitor == 0 || identity.name != expected {
        bail!("selected capture output changed or left the desktop: {expected}");
    }
    Ok(identity)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Description {
    pub color_space: DXGI_COLOR_SPACE_TYPE,
    pub metadata: butterpollo_media_policy::hdr::Metadata,
}

/// Whether an output's colour space is the HDR10 signal the HDR encode path carries.
pub(super) fn in_hdr(description: Option<Description>) -> Option<bool> {
    description.map(|d| d.color_space == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020)
}

pub(super) fn mode_changed(
    previous: Option<DXGI_COLOR_SPACE_TYPE>,
    current: Option<Description>,
) -> bool {
    current.is_some_and(|current| Some(current.color_space) != previous)
}

/// Never use an adapter/output ordinal, primary display or a name alone after
/// re-enumeration. A removed monitor's name can be reassigned to another one.
fn selected<T>(
    identity: &Identity,
    candidates: impl IntoIterator<Item = (Identity, T)>,
) -> Result<T> {
    let mut matching = candidates
        .into_iter()
        .filter_map(|(candidate, output)| (candidate == *identity).then_some(output));
    let output = matching
        .next()
        .with_context(|| format!("selected capture output is unavailable: {}", identity.name))?;
    if matching.next().is_some() {
        bail!("selected capture output is ambiguous: {}", identity.name);
    }
    Ok(output)
}

pub(super) trait Source {
    type Output;
    fn current(&self) -> bool;
    fn refresh(&mut self, identity: &Identity) -> Result<Self::Output>;
    fn describe(&self, output: &Self::Output) -> Option<Description>;
    /// The output's refresh rate in Hz, 0 when unknown.
    fn refresh_hz(&self, identity: &Identity) -> u32;
    /// Windows' "SDR content brightness" in nits, 100.0 when unavailable.
    fn sdr_white_nits(&self, identity: &Identity) -> f32;
}

struct Cache<S: Source> {
    source: S,
    identity: Identity,
    output: Option<S::Output>,
    next_check: Instant,
    snapshot: std::result::Result<Option<Description>, String>,
    metadata: butterpollo_media_policy::hdr::Metadata,
    /// The refresh rate in Hz at the last check, 0 when unknown.
    refresh: u32,
    /// Windows "SDR content brightness" in nits at the last check.
    sdr_white_nits: f32,
    /// Monotonic counter incremented on every poll/refresh so a slower watcher
    /// pass can never overwrite a fresher `refresh_now` snapshot.
    generation: u64,
}

impl<S: Source> Cache<S> {
    fn new(source: S, identity: Identity, output: S::Output, now: Instant) -> Self {
        Self {
            source,
            identity,
            output: Some(output),
            next_check: now,
            snapshot: Ok(None),
            metadata: Default::default(),
            refresh: 0,
            sdr_white_nits: 100.0,
            generation: 0,
        }
    }

    fn poll(&mut self, now: Instant) -> Result<Option<Description>> {
        if now >= self.next_check {
            self.generation += 1;
            self.next_check = now + CHECK_INTERVAL;
            // Read with the colour, at its interval only.
            self.refresh = self.source.refresh_hz(&self.identity);
            self.sdr_white_nits = self.source.sdr_white_nits(&self.identity);
            let mut attempts = 0;
            let result = loop {
                attempts += 1;
                let attempt = (|| {
                    if self.output.is_none() || !self.source.current() {
                        self.output = None;
                        self.output = Some(self.source.refresh(&self.identity)?);
                    }
                    let description = self.source.describe(self.output.as_ref().unwrap());
                    // The topology can change again during enumeration. Do not
                    // publish an already stale factory's description as current.
                    anyhow::Ok(self.source.current().then_some(description))
                })();
                match attempt {
                    Ok(None) if attempts < REFRESH_ATTEMPTS => {}
                    attempt => break attempt,
                }
            };
            self.snapshot = match result {
                Ok(Some(description)) => {
                    if let Some(description) = description {
                        self.metadata = description.metadata;
                    }
                    Ok(description)
                }
                // Displays are still changing (another display arriving, or
                // its mode and HDR being set). That is not a failed output:
                // capture must not restart or fail to open for it. Keep the
                // last published state and look again on the next poll.
                Ok(None) => {
                    self.output = None;
                    self.next_check = now + REFRESH_RETRY;
                    self.snapshot.clone()
                }
                Err(error) => {
                    // Retry even if the failed lookup left a current factory.
                    // Metadata retains the last known value for this output.
                    self.output = None;
                    Err(format!("{error:#}"))
                }
            };
        }
        self.snapshot.clone().map_err(anyhow::Error::msg)
    }

    /// Force a fresh factory enumeration and colour/white-level read right now,
    /// e.g. after `DuplicateOutput1` returns a surface format that disagrees
    /// with the pre-open reading during a mode transition.
    fn refresh_now(&mut self, now: Instant) -> Result<Option<Description>> {
        self.output = None;
        self.next_check = now;
        self.poll(now)
    }

    #[cfg(test)]
    fn metadata(&mut self, now: Instant) -> butterpollo_media_policy::hdr::Metadata {
        let _ = self.poll(now);
        self.metadata
    }
}

pub(super) struct Dxgi {
    factory: IDXGIFactory1,
}

impl Source for Dxgi {
    type Output = IDXGIOutput1;

    fn current(&self) -> bool {
        // SAFETY: The cache owns the live factory throughout this query.
        unsafe { self.factory.IsCurrent().as_bool() }
    }

    fn refresh(&mut self, identity: &Identity) -> Result<Self::Output> {
        // SAFETY: Enumeration returns owned COM references; descriptors are
        // owned values and no output ordinal survives this enumeration.
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let mut candidates = Vec::new();
            for index in 0..32 {
                let Ok(adapter) = factory.EnumAdapters1(index) else {
                    break;
                };
                let desc = adapter.GetDesc1()?;
                if (desc.AdapterLuid.LowPart, desc.AdapterLuid.HighPart) != identity.adapter {
                    continue;
                }
                for index in 0..32 {
                    let Ok(output) = adapter.EnumOutputs(index) else {
                        break;
                    };
                    let desc = output.GetDesc()?;
                    if !desc.AttachedToDesktop.as_bool() {
                        continue;
                    }
                    candidates.push((
                        Identity {
                            adapter: identity.adapter,
                            name: from_wide(&desc.DeviceName),
                            monitor: desc.Monitor.0 as usize,
                        },
                        output.cast::<IDXGIOutput1>()?,
                    ));
                }
            }
            let output = selected(identity, candidates)?;
            self.factory = factory;
            Ok(output)
        }
    }

    fn describe(&self, output: &Self::Output) -> Option<Description> {
        // Preserve the existing fallback when GetDesc1/IDXGIOutput6 is not
        // available: WGC uses the requested HDR state and metadata stays known
        // or at its default. Missing selected outputs fail in refresh instead.
        // SAFETY: `output` is a live owned COM reference; GetDesc1 returns a value.
        unsafe {
            output
                .cast::<IDXGIOutput6>()
                .and_then(|output| output.GetDesc1())
                .ok()
                .map(|desc| Description {
                    color_space: desc.ColorSpace,
                    metadata: butterpollo_media_policy::hdr::Metadata::display(
                        desc.MaxLuminance,
                        desc.MinLuminance,
                        desc.MaxFullFrameLuminance,
                    ),
                })
        }
    }

    fn refresh_hz(&self, identity: &Identity) -> u32 {
        current_refresh_hz(&identity.name)
    }

    fn sdr_white_nits(&self, identity: &Identity) -> f32 {
        sdr_white_nits_for_display(&identity.name).unwrap_or(100.0)
    }
}

/// Query Windows' "SDR content brightness" (`DISPLAYCONFIG_SDR_WHITE_LEVEL`) in nits for
/// `display_name` (when HDR is active on that display).
fn sdr_white_nits_for_display(display_name: &str) -> Option<f32> {
    use windows::Win32::Devices::Display::*;
    let mut path_count = 0u32;
    let mut mode_count = 0u32;
    // SAFETY: `path_count` and `mode_count` are live out-pointers.
    if unsafe {
        GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count)
    } != windows::Win32::Foundation::WIN32_ERROR(0)
    {
        return None;
    }
    let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
    let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
    // SAFETY: `paths` and `modes` have at least the reported counts.
    if unsafe {
        QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut path_count,
            paths.as_mut_ptr(),
            &mut mode_count,
            modes.as_mut_ptr(),
            None,
        )
    } != windows::Win32::Foundation::WIN32_ERROR(0)
    {
        return None;
    }
    for path in &paths[..path_count as usize] {
        let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                adapterId: path.sourceInfo.adapterId,
                id: path.sourceInfo.id,
            },
            ..Default::default()
        };
        // SAFETY: `source.header` names `GET_SOURCE_NAME` and the exact struct size.
        if unsafe { DisplayConfigGetDeviceInfo(&mut source.header) } != 0 {
            continue;
        }
        let name = crate::text::from_wide(&source.viewGdiDeviceName);
        if !display_name.is_empty() && !name.eq_ignore_ascii_case(display_name) {
            continue;
        }
        let mut level = DISPLAYCONFIG_SDR_WHITE_LEVEL {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL,
                size: size_of::<DISPLAYCONFIG_SDR_WHITE_LEVEL>() as u32,
                adapterId: path.targetInfo.adapterId,
                id: path.targetInfo.id,
            },
            SDRWhiteLevel: 0,
        };
        // SAFETY: `level.header` names `GET_SDR_WHITE_LEVEL` and the exact struct size.
        if unsafe { DisplayConfigGetDeviceInfo(&mut level.header) } == 0 && level.SDRWhiteLevel > 0
        {
            return Some(((level.SDRWhiteLevel as f32 / 1000.0) * 80.0).clamp(80.0, 1000.0));
        }
    }
    None
}

/// The current mode's refresh rate of the display named `name` (its GDI
/// device name), in Hz; 0 when unknown.
pub(crate) fn current_refresh_hz(name: &str) -> u32 {
    use windows::Win32::Graphics::Gdi::{DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW};
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let mut mode = DEVMODEW {
        dmSize: size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    // SAFETY: `name` is NUL-terminated and `mode` has dmSize set; both outlive the call.
    if unsafe { EnumDisplaySettingsW(PCWSTR(name.as_ptr()), ENUM_CURRENT_SETTINGS, &mut mode) }
        .as_bool()
        && mode.dmDisplayFrequency > 1
    {
        mode.dmDisplayFrequency
    } else {
        0
    }
}

/// A check's result: the description or why it failed, the last known
/// luminance, and the refresh rate in Hz (0 when unknown).
/// The latest colour reading for a captured output: DXGI description, HDR
/// metadata, refresh rate in Hz (0 when unknown), and SDR white level in nits.
pub(super) type Snapshot = (
    std::result::Result<Option<Description>, String>,
    butterpollo_media_policy::hdr::Metadata,
    u32,
    f32,
);

/// With no read for this long the watcher stops; the next read checks at once.
const IDLE: Duration = Duration::from_secs(5);

/// One output's colour, checked on a thread of its own. The check (IsCurrent,
/// GetDesc1 and, after a topology change, a new factory's enumeration) ran on
/// the encode and capture threads, under the lock both take; they now read the
/// last check.
pub(super) struct Watched<S: Source> {
    cache: Mutex<Cache<S>>,
    published: Mutex<Published>,
}

#[derive(Default)]
struct Published {
    /// Kept current by the watcher while it runs.
    snapshot: Option<Snapshot>,
    /// Monotonic `Cache::generation` of `snapshot`; older snapshots are dropped.
    generation: u64,
    last_read: Option<Instant>,
    watching: bool,
    /// The watcher thread could not start; told once.
    spawn_failed: bool,
}

impl Published {
    fn publish(&mut self, snapshot: Snapshot, generation: u64) {
        if self.watching && generation >= self.generation {
            self.snapshot = Some(snapshot);
            self.generation = generation;
        }
    }
}

pub(super) type Color = Watched<Dxgi>;

impl<S: Source + Send + 'static> Watched<S>
where
    S::Output: Send,
{
    fn new(cache: Cache<S>) -> Self {
        Self {
            cache: Mutex::new(cache),
            published: Mutex::default(),
        }
    }

    /// The last check, or a check now when no watcher keeps one.
    pub(super) fn read(self: &Arc<Self>) -> Snapshot {
        self.read_with(Instant::now(), |watched| {
            thread::Builder::new()
                .name("display-watch".into())
                .spawn(move || watch(watched))
                .map(drop)
        })
    }

    /// Whether the cached DXGI factory is still current.
    pub(super) fn is_current(&self) -> bool {
        self.cache.lock().unwrap().source.current()
    }

    /// Immediately re-enumerate DXGI and re-read colour state, publishing the
    /// result so subsequent reads do not stay stale until `CHECK_INTERVAL`.
    pub(super) fn refresh_now(self: &Arc<Self>) -> Snapshot {
        self.refresh_now_with(Instant::now())
    }

    fn refresh_now_with(&self, now: Instant) -> Snapshot {
        let (snapshot, generation) = {
            let mut cache = self.cache.lock().unwrap();
            let _ = cache.refresh_now(now);
            (
                (
                    cache.snapshot.clone(),
                    cache.metadata,
                    cache.refresh,
                    cache.sdr_white_nits,
                ),
                cache.generation,
            )
        };
        let mut published = self.published.lock().unwrap();
        published.last_read = Some(now);
        published.publish(snapshot.clone(), generation);
        snapshot
    }

    fn read_with(
        self: &Arc<Self>,
        now: Instant,
        spawn: impl FnOnce(Weak<Self>) -> std::io::Result<()>,
    ) -> Snapshot {
        {
            let mut published = self.published.lock().unwrap();
            published.last_read = Some(now);
            if let Some(snapshot) = &published.snapshot {
                return snapshot.clone();
            }
        }
        let (snapshot, generation) = self.check(now);
        let mut published = self.published.lock().unwrap();
        if !published.watching && !published.spawn_failed {
            match spawn(Arc::downgrade(self)) {
                Ok(()) => published.watching = true,
                Err(error) => {
                    // Without the thread, every read checks as before.
                    published.spawn_failed = true;
                    tracing::warn!(%error, "the display colour watcher did not start; capture and encode check the display themselves");
                }
            }
        }
        published.publish(snapshot.clone(), generation);
        snapshot
    }

    fn check(&self, now: Instant) -> (Snapshot, u64) {
        let mut cache = self.cache.lock().unwrap();
        let _ = cache.poll(now);
        (
            (
                cache.snapshot.clone(),
                cache.metadata,
                cache.refresh,
                cache.sdr_white_nits,
            ),
            cache.generation,
        )
    }

    fn next_check(&self) -> Instant {
        self.cache.lock().unwrap().next_check
    }

    /// One pass of the watcher; false when it stops, unread for [`IDLE`].
    fn tick(&self, now: Instant) -> bool {
        {
            let mut published = self.published.lock().unwrap();
            if published
                .last_read
                .is_none_or(|at| now.saturating_duration_since(at) >= IDLE)
            {
                published.snapshot = None;
                published.watching = false;
                return false;
            }
        }
        let (snapshot, generation) = self.check(now);
        let mut published = self.published.lock().unwrap();
        published.publish(snapshot, generation);
        true
    }
}

/// Holds the output only while it checks: the last device clone dropped ends it.
fn watch<S: Source + Send + 'static>(watched: Weak<Watched<S>>)
where
    S::Output: Send,
{
    loop {
        let Some(due) = watched.upgrade().map(|watched| watched.next_check()) else {
            return;
        };
        thread::sleep(due.saturating_duration_since(Instant::now()));
        let Some(watched) = watched.upgrade() else {
            return;
        };
        if !watched.tick(Instant::now()) {
            return;
        }
    }
}

/// A watched output's published refresh rate, for readers that know the
/// output only by name (the present-time stamper).
trait PublishedRefresh: Send + Sync {
    fn refresh_hz(self: Arc<Self>) -> u32;
}

impl<S: Source + Send + 'static> PublishedRefresh for Watched<S>
where
    S::Output: Send,
{
    fn refresh_hz(self: Arc<Self>) -> u32 {
        self.read().2
    }
}

/// The watched outputs by GDI device name; a newer device's watch replaces
/// an older one, and dropped ones are skipped.
static REFRESH: Mutex<Vec<(String, Weak<dyn PublishedRefresh>)>> = Mutex::new(Vec::new());

fn register<S: Source + Send + 'static>(name: &str, watched: &Arc<Watched<S>>)
where
    S::Output: Send,
{
    let watched: Arc<dyn PublishedRefresh> = watched.clone();
    let mut outputs = REFRESH.lock().unwrap();
    // Another device on the same output (a GPU probe's) keeps its own entry:
    // any live one answers, whichever stops first.
    outputs.retain(|(_, weak)| weak.strong_count() > 0);
    outputs.push((name.to_owned(), Arc::downgrade(&watched)));
}

/// The refresh rate the display watcher last published for the output named
/// `name`; `None` when no device watches it.
pub(crate) fn published_refresh_hz(name: &str) -> Option<u32> {
    let watched = REFRESH
        .lock()
        .unwrap()
        .iter()
        .rev()
        .filter(|(other, _)| other == name)
        .find_map(|(_, weak)| weak.upgrade())?;
    Some(watched.refresh_hz())
}

impl Color {
    /// Makes the colour readable by output name too, for the refresh rate.
    pub(super) fn publish(self: &Arc<Self>) {
        let name = self.cache.lock().unwrap().identity.name.clone();
        register(&name, self);
    }

    pub(super) fn open(output: &IDXGIOutput1, name: &str) -> Result<Self> {
        // Use the output's owner, which may differ from the encode device's
        // adapter when WGC captures a display on another GPU.
        // SAFETY: The output is live; GetParent returns owned COM references.
        unsafe {
            let adapter: IDXGIAdapter1 = output.GetParent()?;
            let adapter_desc = adapter.GetDesc1()?;
            let desc = output.GetDesc()?;
            let identity = bind_identity(
                name,
                Identity {
                    adapter: (
                        adapter_desc.AdapterLuid.LowPart,
                        adapter_desc.AdapterLuid.HighPart,
                    ),
                    name: from_wide(&desc.DeviceName),
                    monitor: desc.Monitor.0 as usize,
                },
                desc.AttachedToDesktop.as_bool(),
            )?;
            let factory = adapter.GetParent()?;
            Ok(Self::new(Cache::new(
                Dxgi { factory },
                identity,
                output.clone(),
                Instant::now(),
            )))
        }
    }
}

#[cfg(test)]
mod tests;
