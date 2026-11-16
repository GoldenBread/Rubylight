//! The ended stream sessions the console lists, saved in the host's data directory
//! (`session-history.json`, the rules and caps are `butterpollo_core::session_history`).
//!
//! A stream's entry is written once, when its thread tears down or its start fails, and
//! its frame rate is sampled every second meanwhile (`Host::session_tick`). A journal
//! left by a host that stopped mid-stream becomes an `interrupted` entry at the next
//! start. Saving happens on the stream's own teardown thread; the console only reads.
use crate::state::{Host, Session};
use butterpollo_core::{
    log_analysis,
    session::{Role, Transport},
    session_history::{self as history, Caps, Entry, Store},
    session_state::{Journal, State},
    timing_record::codec_name,
};
use std::{
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError, atomic::Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// The history file in the profile directory.
pub const FILE: &str = "session-history.json";
/// How often a running stream's frame rate is sampled, for `p95_fps`.
const SAMPLE_EVERY: Duration = Duration::from_secs(1);
/// The most frame-rate samples kept per stream: a day of seconds.
const MAX_SAMPLES: usize = 24 * 60 * 60;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

/// One running stream's per-second frame rates.
#[derive(Default)]
struct Samples {
    /// The frame counter and time of the last sample.
    last: Option<(Instant, u64)>,
    /// One frame rate per sampled second.
    fps: VecDeque<f64>,
}
impl Samples {
    /// Records `frames` at `now`, at most once a second. The first call only
    /// starts the count: a stream's first second is not a measurement.
    fn sample(&mut self, frames: u64, now: Instant) {
        let Some((at, counted)) = self.last else {
            self.last = Some((now, frames));
            return;
        };
        let elapsed = now.saturating_duration_since(at);
        if elapsed < SAMPLE_EVERY {
            return;
        }
        let rate = frames.saturating_sub(counted) as f64 / elapsed.as_secs_f64();
        self.last = Some((now, frames));
        if self.fps.len() == MAX_SAMPLES {
            self.fps.pop_front();
        }
        self.fps.push_back(rate);
    }
    fn mean(&self) -> Option<f64> {
        (!self.fps.is_empty()).then(|| self.fps.iter().sum::<f64>() / self.fps.len() as f64)
    }
    /// The near-bottom of the sampled frame rates: 5% of the seconds ran at or
    /// below it, which is the 95th percentile of their frame times in fps.
    fn p95(&self) -> Option<f64> {
        if self.fps.is_empty() {
            return None;
        }
        let mut sorted: Vec<f64> = self.fps.iter().copied().collect();
        sorted.sort_by(f64::total_cmp);
        let rank = ((sorted.len() as f64 * 0.05).ceil() as usize).max(1);
        sorted.get(rank - 1).copied()
    }
}

/// The saved history and the running streams' samples.
pub struct History {
    directory: PathBuf,
    store: Mutex<Store>,
    samples: Mutex<BTreeMap<String, Samples>>,
}
impl History {
    /// Reads the history saved in `directory`. A damaged file starts from none
    /// and the next save replaces it; entries past the caps are dropped.
    pub fn load(directory: &Path) -> Self {
        let path = directory.join(FILE);
        let mut store = match std::fs::read(&path) {
            Ok(bytes) => Store::parse(&bytes).unwrap_or_else(|| {
                tracing::warn!(
                    path = %path.display(),
                    "the session history is damaged; starting from none"
                );
                Store::default()
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Store::default(),
            Err(error) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %error,
                    "the session history could not be read; starting from none"
                );
                Store::default()
            }
        };
        store.prune_with(now_ms(), Caps::default());
        Self {
            directory: directory.to_owned(),
            store: Mutex::new(store),
            samples: Mutex::new(BTreeMap::new()),
        }
    }
    /// Newest first.
    pub fn entries(&self) -> Vec<Entry> {
        self.store
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entries()
            .cloned()
            .collect()
    }
    /// Samples one running stream's frame rate, at most once a second.
    pub fn sample(&self, id: &str, frames: u64, now: Instant) {
        let mut samples = self.samples.lock().unwrap_or_else(PoisonError::into_inner);
        samples
            .entry(id.to_owned())
            .or_default()
            .sample(frames, now);
    }
    /// Adds an entry as the newest session and saves the history.
    fn push(&self, entry: Entry) {
        let mut store = self.store.lock().unwrap_or_else(PoisonError::into_inner);
        store.push(entry, now_ms());
        let path = self.directory.join(FILE);
        if let Err(error) = butterpollo_core::state::write_json(&path, &*store) {
            tracing::warn!(
                path = %path.display(),
                error = %format!("{error:#}"),
                "the session history could not be saved; this session stays until the host stops"
            );
        }
    }
}

impl Host {
    /// Writes the history entry of a stream that just ended; `reason` is one of
    /// the `session_history::REASON_*` values. Input-only and remote-monitor
    /// sessions are not history: only streams carry the statistics.
    pub fn record_session(&self, session: &Session, reason: &str) {
        if session.launch.role != Role::Stream {
            return;
        }
        let app = {
            let apps = self.apps.read().unwrap_or_else(PoisonError::into_inner);
            apps.iter()
                .find(|app| app.id() == session.launch.app_id)
                .map(|app| app.name.clone())
                .unwrap_or_default()
        };
        let samples = self
            .session_history
            .samples
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&session.launch.id);
        self.session_history
            .push(entry(session, reason, app, samples.as_ref()));
    }

    /// Writes the entry of a stream the previous host stopped during: its journal
    /// only knows the device, the app and when the session began.
    pub fn record_interrupted_session(&self, journal: &Journal) {
        if journal.state != State::Streaming {
            return;
        }
        let owner = journal
            .owner
            .as_deref()
            .map(butterpollo_core::state::device_uuid);
        let device_name = {
            let paired = self.paired.read().unwrap_or_else(PoisonError::into_inner);
            paired
                .clients
                .iter()
                .find(|client| Some(client.uuid.as_str()) == owner)
                .map(|client| client.name.clone())
                .unwrap_or_default()
        };
        let ended_unix_ms = now_ms();
        let app = journal.app.as_ref();
        self.session_history.push(Entry {
            device: owner.unwrap_or_default().to_owned(),
            device_name,
            app_id: app.map_or(0, |app| app.app_id),
            app: app.map_or_else(String::new, |app| app.title.clone()),
            role: "stream".into(),
            transport: String::new(),
            codec: String::new(),
            end_reason: history::REASON_INTERRUPTED.into(),
            end_detail: "The host stopped during the session.".into(),
            started_unix_ms: journal.since_unix_ms,
            ended_unix_ms,
            duration_ms: ended_unix_ms.saturating_sub(journal.since_unix_ms),
            ..Default::default()
        });
    }
}

/// The entry of a stream that just ended, at `Session::started`.
fn entry(session: &Session, reason: &str, app: String, samples: Option<&Samples>) -> Entry {
    let launch = &session.launch;
    let config = &session.config;
    let mode = session.stream_mode();
    let duration_ms = session.started.elapsed().as_millis() as u64;
    let seconds = duration_ms as f64 / 1000.;
    let frames = session.stats.frames.load(Ordering::Relaxed);
    let bytes = session.stats.bytes.load(Ordering::Relaxed);
    let sampled = samples.and_then(Samples::mean);
    let mean_fps = sampled.unwrap_or_else(|| {
        if seconds > 0. {
            frames as f64 / seconds
        } else {
            0.
        }
    });
    let p95_fps = samples.and_then(Samples::p95).unwrap_or(mean_fps);
    // The client reports the video packets it did not receive through its FEC
    // blocks; as a share of what the host sent, that is the loss it saw.
    let (missing, _, _) = session
        .stats
        .performance
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .fec_totals();
    let packets = session.stats.packets.load(Ordering::Relaxed);
    let detail = session.failure().unwrap_or_default();
    let failed = reason == history::REASON_FAILED || reason == history::REASON_START_FAILED;
    let category = failed
        .then(|| log_analysis::classify(&detail))
        .flatten()
        .map(category_name);
    let ended_unix_ms = now_ms();
    Entry {
        id: launch.id.clone(),
        device: launch.client.uuid.clone(),
        device_name: launch.client.name.clone(),
        app_id: launch.app_id,
        app,
        role: role_name(launch.role).into(),
        transport: transport_name(launch.transport).into(),
        codec: codec_name(config.codec).into(),
        width: mode.width,
        height: mode.height,
        fps: mode.fps_millihz.saturating_add(500) / 1000,
        hdr: config.hdr,
        bitrate_kbps: session.bitrate.load(Ordering::Relaxed),
        mean_bitrate_kbps: rounded(
            if seconds > 0. {
                bytes as f64 * 8. / duration_ms as f64
            } else {
                0.
            },
            10.,
        ),
        mean_fps: rounded(mean_fps, 10.),
        p95_fps: rounded(p95_fps, 10.),
        loss_percent: rounded(
            if packets > 0 {
                missing as f64 * 100. / packets as f64
            } else {
                0.
            },
            100.,
        ),
        end_reason: reason.into(),
        end_detail: detail,
        end_category: category.map(str::to_owned),
        started_unix_ms: ended_unix_ms.saturating_sub(duration_ms),
        ended_unix_ms,
        duration_ms,
    }
}

fn role_name(role: Role) -> &'static str {
    match role {
        Role::Stream => "stream",
        Role::RemoteMonitor => "remote_monitor",
        Role::InputOnly => "input_only",
    }
}
fn transport_name(transport: Transport) -> &'static str {
    match transport {
        Transport::GameStream => "moonlight",
        Transport::WebRtc => "webrtc",
    }
}
fn category_name(category: log_analysis::Category) -> &'static str {
    match category {
        log_analysis::Category::Encoder => "encoder",
        log_analysis::Category::Capture => "capture",
        log_analysis::Category::Network => "network",
        log_analysis::Category::Driver => "driver",
    }
}
/// `value` to `1 / scale` (10 for a tenth, 100 for a hundredth), with a value
/// that is not a number saved as 0 so the file stays readable.
fn rounded(value: f64, scale: f64) -> f64 {
    if value.is_finite() {
        (value * scale).round() / scale
    } else {
        0.
    }
}

#[cfg(test)]
mod tests;
