//! The driver settings a panel shows and the five states a setting can be in. On Linux these are the same types as on Windows, with the rules between settings; nothing here reads a driver, because no vendor settings API is available on a Linux host yet.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A name read back from JSON, as a `&'static str`. The set of setting ids, groups and labels is
/// fixed by the code that builds them, so the pool stays small: each distinct name is leaked
/// once.
fn intern(text: &str) -> &'static str {
    use std::{collections::BTreeSet, sync::Mutex};
    static POOL: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());
    let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(known) = pool.get(text) {
        return known;
    }
    let leaked: &'static str = Box::leak(text.to_owned().into_boxed_str());
    pool.insert(leaked);
    leaked
}

/// The five states of plan decision 5, as named in `docs/native-api.md`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Configurable,
    ReadOnly,
    NeedsExternalSetup,
    Unsupported,
    TemporarilyUnavailable,
}
impl State {
    pub fn name(self) -> &'static str {
        match self {
            Self::Configurable => "configurable",
            Self::ReadOnly => "read_only",
            Self::NeedsExternalSetup => "needs_external_setup",
            Self::Unsupported => "unsupported",
            Self::TemporarilyUnavailable => "temporarily_unavailable",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Toggle,
    Integer,
    Choice,
    Action,
}

/// Whether the setting can be written, decided without writing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Writable {
    /// The driver reports the feature supported and, for integers, a usable range. The driver's
    /// acceptance of an actual write is not tested here.
    Yes,
    /// The driver itself says the value cannot change (unsupported, or an empty range).
    No,
    /// The driver has no query that tells; only a write would show.
    Unverified,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Choice(String),
}
impl Value {
    pub fn display(&self) -> String {
        match self {
            Self::Bool(true) => "on".into(),
            Self::Bool(false) => "off".into(),
            Self::Int(v) => v.to_string(),
            Self::Choice(v) => v.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub min: i64,
    pub max: i64,
    pub step: i64,
}
impl Range {
    /// A range a client can edit: non-empty, with a positive step.
    pub fn usable(self) -> bool {
        self.min < self.max && self.step > 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChoiceOption {
    pub value: String,
    pub raw: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    /// Cannot be on at the same time as the other.
    Excludes,
    /// Needs the other to be on.
    Requires,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Dependency {
    pub group: &'static str,
    pub relation: Relation,
    pub note: &'static str,
    /// Whether the other feature is on right now, when the driver could tell.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
}
/// `Dependency` as it comes back from JSON, with owned text.
impl<'de> Deserialize<'de> for Dependency {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        DependencyWire::deserialize(d).map(Self::from)
    }
}
#[derive(Deserialize)]
struct DependencyWire {
    group: String,
    relation: Relation,
    note: String,
    #[serde(default)]
    active: Option<bool>,
}
impl From<DependencyWire> for Dependency {
    fn from(wire: DependencyWire) -> Self {
        Self {
            group: intern(&wire.group),
            relation: wire.relation,
            note: intern(&wire.note),
            active: wire.active,
        }
    }
}

/// Which feature groups cannot be on together and which need another, as the driver documents
/// them. `gpu_scaling` is a display setting outside the 3D settings.
const DEPENDENCIES: &[(&str, Relation, &str, &str)] = &[
    ("anti_lag", Relation::Excludes, "chill", ""),
    ("chill", Relation::Excludes, "anti_lag", ""),
    ("chill", Relation::Excludes, "boost", ""),
    ("boost", Relation::Excludes, "chill", ""),
    ("boost", Relation::Excludes, "rsr", "on some GPUs"),
    (
        "image_sharpening",
        Relation::Excludes,
        "rsr",
        "on some GPUs",
    ),
    ("rsr", Relation::Excludes, "boost", "on some GPUs"),
    (
        "rsr",
        Relation::Excludes,
        "image_sharpening",
        "on some GPUs",
    ),
    (
        "rsr",
        Relation::Requires,
        "gpu_scaling",
        "enabling RSR turns GPU scaling on",
    ),
    ("enhanced_sync", Relation::Excludes, "vsync", ""),
    ("vsync", Relation::Excludes, "enhanced_sync", ""),
    (
        "image_sharpen_desktop",
        Relation::Requires,
        "image_sharpening",
        "",
    ),
];

/// Settings the driver only accepts while their feature is on. A request that changes one lists
/// `<group>.enabled` first.
const NEEDS_FEATURE_ON: &[&str] = &[
    "chill.min_fps",
    "chill.max_fps",
    "boost.min_resolution",
    "image_sharpening.sharpness",
    "frame_rate_target.fps",
    "afmf.search_mode",
    "afmf.performance_mode",
    "afmf.fast_motion_response",
];

/// The groups that cannot be on while `group` is (the driver may switch them off when `group`
/// goes on).
pub fn excluded_by(group: &str) -> impl Iterator<Item = &'static str> + '_ {
    DEPENDENCIES
        .iter()
        .filter(move |(g, relation, _, _)| *g == group && *relation == Relation::Excludes)
        .map(|(_, _, other, _)| *other)
}

/// The `<group>.enabled` toggle a setting needs on before the driver accepts a write of it.
pub fn feature_toggle(id: &str) -> Option<String> {
    let id = NEEDS_FEATURE_ON.iter().find(|needing| **needing == id)?;
    Some(format!("{}.enabled", id.split('.').next()?))
}

/// The setting a read-only one is derived from: it changes with that setting and needs no
/// journal entry of its own.
pub fn derived_from(id: &str) -> Option<&'static str> {
    (id == "vsync.enabled").then_some("vsync.mode")
}

/// One setting as a driver backend reports it.
#[derive(Clone, Debug, Serialize)]
pub struct Setting {
    pub id: &'static str,
    pub group: &'static str,
    pub label: &'static str,
    pub interface: &'static str,
    pub kind: Kind,
    pub state: State,
    /// `None` when the driver could not answer.
    pub supported: Option<bool>,
    /// `None` for actions.
    pub readable: Option<bool>,
    pub writable: Writable,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<ChoiceOption>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub depends: Vec<Dependency>,
    pub notes: Vec<String>,
    /// What the driver answered to each call, for the record.
    pub adlx: BTreeMap<&'static str, String>,
}
/// `Setting` as it comes back from JSON, with owned text. (A derived `Deserialize` on the
/// `&'static str` fields would demand `'de: 'static`.)
impl<'de> Deserialize<'de> for Setting {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        SettingWire::deserialize(d).map(Self::from)
    }
}
#[derive(Deserialize)]
struct SettingWire {
    id: String,
    group: String,
    label: String,
    interface: String,
    kind: Kind,
    state: State,
    supported: Option<bool>,
    readable: Option<bool>,
    writable: Writable,
    #[serde(default)]
    value: Option<Value>,
    #[serde(default)]
    unit: Option<String>,
    #[serde(default)]
    range: Option<Range>,
    #[serde(default)]
    options: Vec<ChoiceOption>,
    #[serde(default)]
    depends: Vec<DependencyWire>,
    notes: Vec<String>,
    adlx: BTreeMap<String, String>,
}
impl From<SettingWire> for Setting {
    fn from(wire: SettingWire) -> Self {
        Self {
            id: intern(&wire.id),
            group: intern(&wire.group),
            label: intern(&wire.label),
            interface: intern(&wire.interface),
            kind: wire.kind,
            state: wire.state,
            supported: wire.supported,
            readable: wire.readable,
            writable: wire.writable,
            value: wire.value,
            unit: wire.unit.as_deref().map(intern),
            range: wire.range,
            options: wire.options,
            depends: wire.depends.into_iter().map(Dependency::from).collect(),
            notes: wire.notes,
            adlx: wire
                .adlx
                .into_iter()
                .map(|(key, value)| (intern(&key), value))
                .collect(),
        }
    }
}
