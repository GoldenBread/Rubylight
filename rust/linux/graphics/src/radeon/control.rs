//! Writes, the journal and restore for driver settings (the rules are in `docs/native-api.md`, "Radeon state and writes"). On Linux the controller, the journal and the restore run as on Windows against any [`RadeonBackend`]; no backend for a real driver is available on a Linux host yet.
//!
//! A write request is validated whole, its originals are journaled and synced to disk, then it is
//! applied in order. If the driver refuses one change, everything the request did is undone. The
//! driver's read-back is what comes back to the caller. Restore puts a journaled setting back only
//! while it still holds the value the host wrote.
//!
//! `RadeonBackend` is the seam to the driver, so all of this runs without a GPU.
use super::{
    model::{self, Kind, Setting, State, Value},
    owner::{self, Owner},
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    path::{Path, PathBuf},
    time::Instant,
};

mod apply;
mod restore;
mod state;

/// The journal's file name inside the profile (config) directory.
pub const JOURNAL_FILE: &str = "radeon-journal.json";

/// What `Controller` needs from the driver.
pub trait RadeonBackend {
    /// All settings of one GPU, freshly read from the driver.
    fn read(&mut self, gpu: i32) -> Result<Vec<Setting>, BackendError>;
    /// The current value of one setting, freshly read.
    fn read_value(&mut self, gpu: i32, id: &str) -> Result<Value, BackendError> {
        let settings = self.read(gpu)?;
        find(&settings, id)
            .and_then(|s| s.value.clone())
            .ok_or_else(|| BackendError(format!("the driver does not report {id}")))
    }
    /// Write one value. The caller has validated it.
    fn write(&mut self, gpu: i32, id: &str, value: &Value) -> Result<(), BackendError>;
    /// The GPUs whose driver settings this backend controls. No vendor settings API lists any
    /// on a Linux host yet, so a backend that does not say otherwise lists none.
    fn gpus(&mut self) -> Result<Vec<GpuInfo>, BackendError> {
        Ok(Vec::new())
    }
    /// The driver's raw keys for `id` before a write, for a restore that puts them back as
    /// they were. As on Windows a backend that does not say otherwise keeps none.
    fn snapshot_keys(
        &mut self,
        _gpu: i32,
        _id: &str,
    ) -> Result<Option<RawSettingKeys>, BackendError> {
        Ok(None)
    }
    /// Puts `entry` back from its raw keys: `Ok(true)` when done and verified, `Ok(false)` to
    /// restore through `write`, which every Linux backend does.
    fn restore_raw(&mut self, _entry: &Entry) -> Result<bool, BackendError> {
        Ok(false)
    }
}

/// Raw driver `(key, optional value)` pairs captured before a setting is applied.
pub type RawSettingKeys = Vec<(u32, Option<u32>)>;

/// One GPU as its driver names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpuInfo {
    /// The driver's unique id for the GPU.
    pub id: i32,
    pub name: String,
    /// The driver software's release when the driver names it, else the driver version.
    pub driver: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendError(pub String);
impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The error codes of `POST /radeon/settings`, plus the two the host adds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// Unknown id, wrong type, outside the range or off the step.
    InvalidValue,
    /// The setting is not `configurable`.
    NotWritable,
    /// The driver refused a write or kept another value. Everything was undone.
    DriverRejected,
    /// The driver could not be read.
    Unavailable,
    /// The journal could not be read or written, so nothing was changed.
    Journal,
}
impl ErrorKind {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidValue => "invalid_value",
            Self::NotWritable => "not_writable",
            Self::DriverRejected => "driver_rejected",
            Self::Unavailable => "radeon_unavailable",
            Self::Journal => "journal_error",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteError {
    pub kind: ErrorKind,
    /// The setting the error names.
    pub id: Option<String>,
    pub message: String,
}
impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.id {
            Some(id) => write!(f, "{} ({id}): {}", self.kind.code(), self.message),
            None => write!(f, "{}: {}", self.kind.code(), self.message),
        }
    }
}
impl std::error::Error for WriteError {}
fn error(kind: ErrorKind, id: Option<&str>, message: impl Into<String>) -> WriteError {
    WriteError {
        kind,
        id: id.map(String::from),
        message: message.into(),
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Change {
    pub id: String,
    pub value: Value,
}

/// One setting the host changed, with the value to go back to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub gpu: i32,
    pub id: String,
    /// The session that last wrote it.
    pub session: String,
    /// The value before the first write of this journal's life, kept across later writes.
    pub original: Value,
    /// The value the host left the setting at; restore only acts while it still holds this.
    pub written: Value,
    /// The driver changed it as a consequence of another write (Chill turning Anti-Lag off).
    #[serde(default)]
    pub side_effect: bool,
    /// The id of the user whose settings these are (on Linux the numeric user id, as text).
    /// Driver settings are per user: another account's values are not this account's to read,
    /// compare or put back, so a restore leaves entries of other users alone. Empty matches
    /// nobody.
    #[serde(default)]
    pub user: String,
    /// The driver's raw keys before the first write ([`RadeonBackend::snapshot_keys`]); none
    /// on a Linux host yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<RawSettingKeys>,
}
impl Entry {
    /// The entry for this setting of this user's.
    fn is(&self, user: &str, gpu: i32, id: &str) -> bool {
        self.user == user && self.gpu == gpu && self.id == id
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Journal {
    #[serde(default)]
    pub version: u32,
    /// The host that last wrote the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<Owner>,
    #[serde(default)]
    pub entries: Vec<Entry>,
}
impl Journal {
    /// A missing file is an empty journal. An unreadable one is an error: the originals in it
    /// must not be silently forgotten.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => {
                let Some(json) = state::json_bytes(&bytes) else {
                    return Ok(Self::default());
                };
                serde_json::from_slice(json)
                    .with_context(|| format!("invalid Radeon journal {}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }
    /// Atomic, and synced before it returns. Stamps this process as the owner.
    pub fn save(&self, path: &Path) -> Result<()> {
        let journal = Self {
            version: 1,
            owner: Some(Owner::current()?),
            entries: self.entries.clone(),
        };
        state::write_json(path, &journal)
    }
    /// Keeps the first original per setting: a second write only moves `written`. If the setting
    /// no longer holds what the host wrote (`entry.original` is its value now), someone changed
    /// it on the PC, that change wins and becomes the value to go back to.
    fn record(&mut self, entry: Entry) {
        match self
            .entries
            .iter_mut()
            .find(|e| e.is(&entry.user, entry.gpu, &entry.id))
        {
            Some(existing) => {
                if existing.written != entry.original {
                    existing.original = entry.original;
                }
                existing.written = entry.written;
                existing.session = entry.session;
                existing.side_effect &= entry.side_effect;
            }
            None => self.entries.push(entry),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Scope<'a> {
    /// Every entry, for a host start after a crash.
    All,
    Session(&'a str),
    /// Only these settings of one session, and the switches the driver made as a consequence of
    /// them. The session's other entries (a client's own writes) stay journaled.
    Settings(&'a str, &'a [String]),
}

#[derive(Clone, Debug, PartialEq)]
pub enum RestoreResult {
    Restored,
    /// The setting already holds the original, for example because the crash came before the write.
    AlreadyOriginal,
    /// Someone changed it on the PC since: the local change wins and the entry is dropped.
    ChangedElsewhere {
        current: Value,
        written: Value,
    },
    /// Still journaled. The next restore tries again.
    Failed(String),
}
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub gpu: i32,
    pub id: String,
    pub result: RestoreResult,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RestoreReport {
    pub outcomes: Vec<Outcome>,
}
impl RestoreReport {
    pub fn failed(&self) -> impl Iterator<Item = &Outcome> {
        self.outcomes
            .iter()
            .filter(|o| matches!(o.result, RestoreResult::Failed(_)))
    }
    pub fn is_clean(&self) -> bool {
        self.failed().next().is_none()
    }
}

pub struct Controller<B> {
    backend: B,
    journal: PathBuf,
    /// Restore stops starting new work after this; what is left stays journaled.
    deadline: Option<Instant>,
    /// The id of the user whose settings the backend reads and writes: the signed-in user,
    /// `None` while nobody is.
    user: Box<dyn Fn() -> Option<String> + Send>,
}

fn find<'a>(settings: &'a [Setting], id: &str) -> Option<&'a Setting> {
    settings.iter().find(|s| s.id == id)
}

fn is_on(settings: &[Setting], id: &str) -> bool {
    matches!(
        find(settings, id).and_then(|s| s.value.as_ref()),
        Some(Value::Bool(true))
    )
}

/// Checks the whole request against the settings as they are now, before anything is written.
fn validate(settings: &[Setting], changes: &[Change]) -> Result<(), WriteError> {
    if changes.is_empty() {
        return Err(error(ErrorKind::InvalidValue, None, "no changes"));
    }
    let mut seen = HashSet::new();
    // Toggles as the request leaves them, in order.
    let mut toggles: HashMap<String, bool> = HashMap::new();
    for change in changes {
        let id = change.id.as_str();
        let invalid = |message: String| error(ErrorKind::InvalidValue, Some(id), message);
        if !seen.insert(id) {
            return Err(invalid("listed twice".into()));
        }
        let Some(setting) = find(settings, id) else {
            return Err(invalid("unknown setting".into()));
        };
        if setting.state != State::Configurable || setting.kind == Kind::Action {
            return Err(error(
                ErrorKind::NotWritable,
                Some(id),
                format!("the setting is {}", setting.state.name()),
            ));
        }
        match (setting.kind, &change.value) {
            (Kind::Toggle, Value::Bool(_)) => {}
            (Kind::Integer, Value::Int(value)) => {
                let range = setting
                    .range
                    .ok_or_else(|| invalid("the driver reports no range".into()))?;
                if *value < range.min || *value > range.max {
                    return Err(invalid(format!(
                        "{value} is outside {}..{}",
                        range.min, range.max
                    )));
                }
                if (value - range.min) % range.step != 0 {
                    return Err(invalid(format!(
                        "{value} is not on the step of {} from {}",
                        range.step, range.min
                    )));
                }
            }
            (Kind::Choice, Value::Choice(name)) => {
                if !setting.options.iter().any(|o| &o.value == name) {
                    return Err(invalid(format!("{name} is not one of the options")));
                }
            }
            (kind, _) => return Err(invalid(format!("expected a {kind:?} value").to_lowercase())),
        }
        // The driver only takes some values while their feature is on.
        if let Some(toggle) = model::feature_toggle(id) {
            let on = toggles
                .get(&toggle)
                .copied()
                .unwrap_or_else(|| is_on(settings, &toggle));
            if !on {
                return Err(invalid(format!(
                    "the driver only accepts it while {toggle} is on; turn that on first or earlier in the same request"
                )));
            }
        }
        if let (Some(group), Value::Bool(on)) = (id.strip_suffix(".enabled"), &change.value) {
            toggles.insert(id.to_string(), *on);
            // Turning a feature on can switch an excluded one off. If the host can neither write
            // nor restore that one, the request would leave the PC changed for good.
            for other in model::excluded_by(group).filter(|_| *on) {
                let partner = format!("{other}.enabled");
                let Some(partner) = find(settings, &partner) else {
                    continue;
                };
                let derived = model::derived_from(partner.id)
                    .and_then(|from| find(settings, from))
                    .is_some_and(|from| from.state == State::Configurable);
                if partner.state != State::Configurable && is_on(settings, partner.id) && !derived {
                    return Err(invalid(format!(
                        "turning it on can switch {} off, and the host cannot write or restore that setting ({})",
                        partner.id,
                        partner.state.name()
                    )));
                }
            }
            // Not measured yet: whether Enhanced Sync switches Wait for Vertical Refresh off (and
            // what that does to vsync.mode, which has a setter and is journaled, but whose value
            // after the flip is unknown). Refuse until a hardware run says what happens.
            if group == "enhanced_sync"
                && *on
                && !is_on(settings, id)
                && is_on(settings, "vsync.enabled")
            {
                return Err(invalid(
                    "vsync is on, and what Enhanced Sync does to it is not measured yet; set vsync.mode to always_off first".into(),
                ));
            }
        }
    }
    Ok(())
}

/// Whether turning on one of the `<group>.enabled` toggles in `ids` can make the driver switch
/// `partner` (another `<group>.enabled`) off.
fn switched_off_by(ids: &[String], partner: &str) -> bool {
    ids.iter()
        .filter_map(|id| id.strip_suffix(".enabled"))
        .any(|group| model::excluded_by(group).any(|other| format!("{other}.enabled") == partner))
}

/// The toggles a request is expected to make the driver switch off: each configurable
/// `<other>.enabled` that is on now and that a requested `<group>.enabled = true` excludes.
fn predicted_effects(before: &[Setting], changes: &[Change]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for change in changes {
        let (Some(group), Value::Bool(true)) = (change.id.strip_suffix(".enabled"), &change.value)
        else {
            continue;
        };
        if is_on(before, &change.id) {
            continue;
        }
        for other in model::excluded_by(group) {
            let partner = format!("{other}.enabled");
            let configurable =
                find(before, &partner).is_some_and(|s| s.state == State::Configurable);
            if configurable
                && is_on(before, &partner)
                && !changes.iter().any(|c| c.id == partner)
                && !out.contains(&partner)
            {
                out.push(partner);
            }
        }
    }
    out
}

impl<B: RadeonBackend> Controller<B> {
    pub fn new(backend: B, journal: PathBuf) -> Self {
        Self {
            backend,
            journal,
            deadline: None,
            user: Box::new(|| Some(owner::own_user())),
        }
    }
    /// Name the user another way (when the host runs as a service account, or in tests).
    pub fn with_user(mut self, user: impl Fn() -> Option<String> + Send + 'static) -> Self {
        self.user = Box::new(user);
        self
    }
    pub fn set_deadline(&mut self, deadline: Option<Instant>) {
        self.deadline = deadline;
    }
    pub fn backend(&mut self) -> &mut B {
        &mut self.backend
    }
    pub fn journal(&self) -> Result<Journal> {
        Journal::load(&self.journal)
    }
    /// What would still be put back for the signed-in user: the journal's entries of their
    /// settings. Another user's entries are not this user's to show or restore.
    pub fn pending(&self) -> Result<Vec<Entry>> {
        let user = (self.user)();
        Ok(self
            .journal()?
            .entries
            .into_iter()
            .filter(|e| user.as_deref() == Some(e.user.as_str()))
            .collect())
    }

    /// Take the journal's lock and load it, unless another live host owns settings in it. The
    /// lock stays held until the guard drops, so load, change and save are one step.
    fn open(&self) -> Result<(owner::JournalLock, Journal)> {
        let lock = owner::lock(&self.journal)?;
        let journal = Journal::load(&self.journal)?;
        if let Some(other) = journal.owner
            && !journal.entries.is_empty()
            && other.is_other_and_alive()
        {
            bail!(
                "another host (process {}) owns the Radeon journal and has settings changed",
                other.pid
            );
        }
        Ok((lock, journal))
    }
}
