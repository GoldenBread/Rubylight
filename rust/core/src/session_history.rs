//! A bounded history of ended stream sessions (`docs/status.md`, fork gaps item 10).
//!
//! One [`Entry`] per stream: what it ran (device, app, codec, resolution, bitrate),
//! how it went (mean and 95th-percentile frame rate, reported loss) and why it
//! ended. The host writes them to `session-history.json` in its data directory
//! ([`crate::state::write_json`]); the console reads them back through
//! `GET /api/sessions/history`.
//!
//! The store only holds the newest sessions: [`MAX_ENTRIES`] entries, nothing
//! older than [`MAX_AGE_MS`] and at most [`MAX_BYTES`] of JSON. The caps are
//! fixed on purpose (there is no setting): the byte and count caps always keep
//! the newest entry, and the age cap drops anything older than the window, even
//! an entry pushed late.
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// How many sessions the history keeps.
pub const MAX_ENTRIES: usize = 200;
/// How long a session stays, counted from its end.
pub const MAX_AGE_MS: u64 = 90 * 24 * 60 * 60 * 1000;
/// The largest saved history. Entries are dropped oldest first to stay under it.
pub const MAX_BYTES: usize = 256 * 1024;
/// The version of the saved document; a file of another version is not read.
pub const VERSION: u32 = 1;

/// The stream ended and nothing failed it: the device left, or the host ended it.
pub const REASON_ENDED: &str = "ended";
/// The stream reported an error; `end_detail` and `end_category` say what.
pub const REASON_FAILED: &str = "failed";
/// The stream never started.
pub const REASON_START_FAILED: &str = "start_failed";
/// The host stopped during the stream; its journal was found at the next start.
pub const REASON_INTERRUPTED: &str = "interrupted";

/// One ended stream session.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Entry {
    /// The stream's session id (`Launch::id`).
    pub id: String,
    /// The device's paired uuid, and its name at the time.
    pub device: String,
    pub device_name: String,
    /// The app's library id and its title at the time.
    pub app_id: u32,
    pub app: String,
    /// `stream`, `remote_monitor` or `input_only`.
    pub role: String,
    /// `moonlight` or `webrtc`.
    pub transport: String,
    /// `h264`, `hevc`, `av1` or `pyrowave`.
    pub codec: String,
    /// The picture the stream ran at, after any mid-stream mode switch.
    pub width: u32,
    pub height: u32,
    /// The rate the host encoded at; `fps` is the client's request.
    pub fps: u32,
    pub hdr: bool,
    /// The encoder's bitrate when the stream ended, in kbps.
    pub bitrate_kbps: u32,
    /// The bitrate the stream was asked for (the negotiated rate), in kbps;
    /// `bitrate_kbps` is what the encoder ran at when it ended. A stream the
    /// network held below the request keeps the two apart. 0 for a stream that
    /// never started, and for an entry saved before this field existed.
    pub requested_bitrate_kbps: u32,
    /// What the video actually sent on average: its bytes over the stream's
    /// duration. Audio is not counted.
    pub mean_bitrate_kbps: f64,
    /// Frames sent over the whole stream: the mean of the per-second samples,
    /// or frames over duration until a second was sampled.
    pub mean_fps: f64,
    /// The near-bottom of the per-second frame rates: 5% of the sampled seconds
    /// ran at or below it, the 95th percentile of their frame times in fps.
    /// 0 until 20 seconds were sampled: below that the slowest second is not a
    /// percentile, and the console shows no figure.
    pub p95_fps: f64,
    /// The client's reported missing video packets, as a percentage of the
    /// packets the host sent, or `None` when the client reported no FEC blocks
    /// (WebRTC, older clients): "not reported" is not "no loss".
    pub loss_percent: Option<f64>,
    /// One of [`REASON_ENDED`], [`REASON_FAILED`], [`REASON_START_FAILED`] or
    /// [`REASON_INTERRUPTED`].
    pub end_reason: String,
    /// The host's words for a failure, when there are any.
    pub end_detail: String,
    /// `encoder`, `capture`, `network` or `driver` when
    /// [`crate::log_analysis`] knows the failure.
    pub end_category: Option<String>,
    pub started_unix_ms: u64,
    pub ended_unix_ms: u64,
    pub duration_ms: u64,
}

/// The limits [`Store::prune`] applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caps {
    pub entries: usize,
    pub age_ms: u64,
    pub bytes: usize,
}
impl Default for Caps {
    fn default() -> Self {
        Self {
            entries: MAX_ENTRIES,
            age_ms: MAX_AGE_MS,
            bytes: MAX_BYTES,
        }
    }
}

/// The saved sessions, newest first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    pub version: u32,
    pub entries: VecDeque<Entry>,
}
impl Default for Store {
    fn default() -> Self {
        Self {
            version: VERSION,
            entries: VecDeque::new(),
        }
    }
}

impl Store {
    /// The history saved in `bytes`, or `None` when they are damaged or of a
    /// version this host does not read, so the caller starts from none and the
    /// next save replaces the file.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let store: Self = serde_json::from_slice(bytes).ok()?;
        (store.version == VERSION).then_some(store)
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        // The shape `Host` saves (`state::write_json` is pretty): the byte cap
        // must measure the file it caps.
        serde_json::to_vec_pretty(self).unwrap_or_default()
    }
    /// Newest first.
    pub fn entries(&self) -> impl Iterator<Item = &Entry> + '_ {
        self.entries.iter()
    }
    /// Adds `entry` as the newest session and applies [`Caps::default`].
    pub fn push(&mut self, entry: Entry, now_ms: u64) {
        self.push_with(entry, now_ms, Caps::default());
    }
    /// Adds `entry` and applies `caps`: sessions that ended before
    /// `now_ms - caps.age_ms` go (the new one too, when it ended that long ago),
    /// then the oldest entries until the store holds at most `caps.entries` and
    /// `caps.bytes` of JSON. The byte and count caps always keep the newest entry.
    pub fn push_with(&mut self, entry: Entry, now_ms: u64, caps: Caps) {
        self.entries.push_front(entry);
        self.prune_with(now_ms, caps);
    }
    /// [`Store::push`]'s caps without adding an entry: for a history read at
    /// startup or waited on by the console.
    pub fn prune_with(&mut self, now_ms: u64, caps: Caps) {
        let oldest = now_ms.saturating_sub(caps.age_ms);
        self.entries.retain(|entry| entry.ended_unix_ms >= oldest);
        self.entries.truncate(caps.entries.max(1));
        while self.entries.len() > 1 && self.to_bytes().len() > caps.bytes {
            self.entries.pop_back();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, ended_unix_ms: u64) -> Entry {
        Entry {
            id: id.into(),
            device: "device".into(),
            device_name: "Living room TV".into(),
            app_id: 3,
            app: "Portal 2".into(),
            role: "stream".into(),
            transport: "moonlight".into(),
            codec: "hevc".into(),
            width: 2560,
            height: 1440,
            fps: 120,
            bitrate_kbps: 40_000,
            mean_fps: 118.4,
            p95_fps: 116.9,
            ended_unix_ms,
            started_unix_ms: ended_unix_ms.saturating_sub(3_600_000),
            duration_ms: 3_600_000,
            ..Default::default()
        }
    }

    #[test]
    fn the_newest_entry_is_first_and_the_count_cap_drops_the_oldest() {
        let caps = Caps {
            entries: 3,
            ..Default::default()
        };
        let mut store = Store::default();
        for id in ["one", "two", "three", "four"] {
            store.push_with(entry(id, 1_000), 1_000, caps);
        }
        let kept: Vec<_> = store.entries().map(|e| e.id.as_str()).collect();
        assert_eq!(kept, ["four", "three", "two"]);
    }

    #[test]
    fn sessions_older_than_the_age_cap_go_out() {
        let caps = Caps {
            age_ms: 60_000,
            ..Default::default()
        };
        let mut store = Store::default();
        store.push_with(entry("old", 1_000), 1_000, caps);
        store.push_with(entry("middle", 50_500), 100_000, caps);
        // 40 000 ms old: inside the minute; "old" ended 99 000 ms ago.
        store.push_with(entry("new", 90_000), 100_000, caps);
        let kept: Vec<_> = store.entries().map(|e| e.id.as_str()).collect();
        assert_eq!(kept, ["new", "middle"]);
    }

    #[test]
    fn an_entry_older_than_the_window_is_not_kept() {
        // A journal's interrupted entry can be late; one that ended before the
        // window is dropped even though it was just pushed.
        let caps = Caps {
            age_ms: 60_000,
            ..Default::default()
        };
        let mut store = Store::default();
        store.push_with(entry("stale", 1_000), 100_000, caps);
        assert_eq!(store.entries().count(), 0);
    }

    #[test]
    fn the_byte_cap_drops_old_entries_but_keeps_the_newest() {
        let caps = Caps {
            entries: 100,
            age_ms: MAX_AGE_MS,
            bytes: 900,
        };
        let mut store = Store::default();
        for id in ["one", "two", "three"] {
            store.push_with(entry(id, 1_000), 1_000, caps);
        }
        assert!(
            store.entries().count() < 3,
            "three entries fit in {} bytes",
            caps.bytes
        );
        assert_eq!(
            store.entries().next().map(|e| e.id.as_str()),
            Some("three"),
            "the newest entry is always kept"
        );
        assert!(store.to_bytes().len() <= caps.bytes);
        // A cap smaller than one entry still keeps that one.
        let mut tiny = Store::default();
        tiny.push_with(entry("only", 1_000), 1_000, Caps { bytes: 1, ..caps });
        assert_eq!(tiny.entries().count(), 1);
    }

    #[test]
    fn a_saved_history_reads_back_unchanged() {
        let mut store = Store::default();
        store.push(entry("one", 1_000), 1_000);
        let parsed = Store::parse(&store.to_bytes()).unwrap();
        assert_eq!(parsed, store);
        assert_eq!(parsed.entries().next().unwrap().codec, "hevc");
    }

    #[test]
    fn a_damaged_or_unknown_history_reads_as_none() {
        assert!(Store::parse(b"{\"entries\": [tru").is_none());
        assert!(Store::parse(b"").is_none());
        let future = Store {
            version: VERSION + 1,
            ..Default::default()
        };
        assert!(Store::parse(&future.to_bytes()).is_none());
    }

    #[test]
    fn an_entry_saved_before_a_field_existed_reads_with_its_default() {
        let saved = format!(
            "{{\"version\":{VERSION},\"entries\":[{{\"id\":\"one\",\"ended_unix_ms\":1000}}]}}"
        );
        let store = Store::parse(saved.as_bytes()).unwrap();
        let entry = store.entries().next().unwrap();
        assert_eq!(entry.id, "one");
        assert_eq!(entry.end_reason, "");
        assert_eq!(entry.loss_percent, None);
        assert_eq!(entry.p95_fps, 0.);
        assert_eq!(entry.requested_bitrate_kbps, 0);
    }
}
