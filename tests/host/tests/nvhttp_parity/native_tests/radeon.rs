//! The native API's Radeon endpoints (plan item A3, brief 4) against a fake driver. No test
//! here touches the real GPU: the worker is the real one, the backend under it is `Fake`.
use super::*;
use butterpollo_config::config::Config;
use butterpollo_stream::radeon_session::WorkerRef;
use butterpollo_win_graphics::radeon::{
    BackendError, Entry, GpuInfo, Journal, RadeonBackend,
    control::JOURNAL_FILE,
    model::{ChoiceOption, Kind, Range, Setting as Driven, State, Value as Raw, Writable},
    worker::{Phase, Timing, Worker},
};
use rubylight_native_api::radeon::{
    self as api, ControlState, Kind as ApiKind, Restore as ApiRestore, Value as Api,
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::Mutex,
    time::{Duration, Instant as Clock},
};

/// An ADLX unique id the way `/api/hardware-metrics` reports it, a signed number that can be
/// negative.
const GPU: i32 = 0xC000_0001_u32 as i32;

#[path = "radeon_driver.rs"]
mod radeon_driver;
use radeon_driver::Driver;
#[derive(Clone, Default)]
struct Fake(Arc<Mutex<Driver>>);
impl Fake {
    fn with(&self, change: impl FnOnce(&mut Driver)) {
        change(&mut self.0.lock().unwrap());
    }
    fn value(&self, id: &str) -> Raw {
        let d = self.0.lock().unwrap();
        d.settings
            .iter()
            .find(|s| s.id == id)
            .and_then(|s| s.value.clone())
            .unwrap()
    }
    fn set(&self, id: &str, value: Raw) {
        self.with(|d| {
            d.settings.iter_mut().find(|s| s.id == id).unwrap().value = Some(value);
        });
    }
    fn writes(&self) -> Vec<String> {
        self.0.lock().unwrap().writes.clone()
    }
    fn reads(&self) -> usize {
        self.0.lock().unwrap().reads
    }
}
impl RadeonBackend for Fake {
    fn read(&mut self, _gpu: i32) -> Result<Vec<Driven>, BackendError> {
        while self.0.lock().unwrap().hold {
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut d = self.0.lock().unwrap();
        d.reads += 1;
        if d.unreadable {
            return Err(BackendError("ADLX_FAIL".into()));
        }
        Ok(d.settings.clone())
    }
    fn write(&mut self, _gpu: i32, id: &str, value: &Raw) -> Result<(), BackendError> {
        let mut d = self.0.lock().unwrap();
        d.writes.push(format!("{id}={}", value.display()));
        if d.refuse_all || d.refuse == Some(id) {
            return Err(BackendError("ADLX_NOT_SUPPORTED".into()));
        }
        if d.ignore == Some(id) {
            return Ok(());
        }
        if let Some(setting) = d.settings.iter_mut().find(|s| s.id == id) {
            setting.value = Some(value.clone());
        }
        Ok(())
    }
    fn gpus(&mut self) -> Result<Vec<GpuInfo>, BackendError> {
        Ok(self.0.lock().unwrap().gpus.clone())
    }
}

fn driven(id: &'static str, kind: Kind, state: State, value: Raw) -> Driven {
    Driven {
        id,
        group: id.split('.').next().unwrap(),
        label: id,
        interface: "fake",
        kind,
        state,
        supported: Some(state != State::Unsupported),
        readable: Some(true),
        writable: Writable::Yes,
        value: Some(value),
        unit: None,
        range: None,
        options: vec![],
        depends: vec![],
        notes: vec![],
        adlx: BTreeMap::new(),
    }
}
fn toggle(id: &'static str, state: State, on: bool) -> Driven {
    driven(id, Kind::Toggle, state, Raw::Bool(on))
}
fn integer(id: &'static str, state: State, value: i64) -> Driven {
    let mut setting = driven(id, Kind::Integer, state, Raw::Int(value));
    setting.range = Some(Range {
        min: 30,
        max: 300,
        step: 10,
    });
    setting.unit = Some("fps");
    setting
}
fn choice(id: &'static str, value: &str) -> Driven {
    let mut setting = driven(
        id,
        Kind::Choice,
        State::Configurable,
        Raw::Choice(value.into()),
    );
    setting.options = ["a", "on_unless_app_specifies", "c"]
        .iter()
        .enumerate()
        .map(|(raw, name)| ChoiceOption {
            value: (*name).into(),
            raw: raw as i32,
        })
        .collect();
    setting
}
fn gpu() -> GpuInfo {
    GpuInfo {
        id: GPU,
        name: "AMD Radeon RX 7900 XT".into(),
        driver: "Adrenalin 26.2.1".into(),
    }
}
/// One setting of every state.
fn driver() -> Fake {
    let fake = Fake::default();
    fake.with(|d| {
        d.gpus = vec![gpu()];
        d.settings = vec![
            toggle("anti_lag.enabled", State::Configurable, false),
            integer("pacing.limit", State::Configurable, 140),
            choice("tessellation.mode", "a"),
            toggle("rsr.enabled", State::ReadOnly, false),
            toggle("sam.enabled", State::NeedsExternalSetup, false),
            integer("chill.min_fps", State::TemporarilyUnavailable, 60),
            toggle("boost.enabled", State::Unsupported, false),
        ];
    });
    fake
}
/// A panel of only `settings`.
fn only(settings: Vec<Driven>) -> Fake {
    let fake = driver();
    fake.with(|d| d.settings = settings);
    fake
}

/// The host with a Radeon worker over a fake driver.
struct Rig {
    s: Setup,
    fake: Fake,
}
impl std::ops::Deref for Rig {
    type Target = Setup;
    fn deref(&self) -> &Setup {
        &self.s
    }
}
impl Rig {
    fn new() -> Self {
        Self::with(driver())
    }
    fn with(fake: Fake) -> Self {
        let rig = Self::start(Setup::new(), fake);
        rig.ready();
        rig
    }
    /// A worker that looks again at unrestored settings every 100 ms instead of every minute.
    fn quick_retry() -> Self {
        let timing = Timing {
            retry_every: Duration::from_millis(100),
            ..Timing::default()
        };
        let rig = Self::start_timed(Setup::new(), driver(), timing);
        rig.ready();
        rig
    }
    /// The worker starts its recovery at once and this returns without waiting for it.
    fn start(s: Setup, fake: Fake) -> Self {
        Self::start_timed(s, fake, Timing::default())
    }
    fn start_timed(s: Setup, fake: Fake, timing: Timing) -> Self {
        let worker = Worker::start(fake.clone(), s.host().directory.join(JOURNAL_FILE), timing);
        assert!(s.host().radeon.attach(WorkerRef::Owned(Arc::new(worker))));
        Self { s, fake }
    }
    fn ready(&self) {
        let until = Clock::now() + Duration::from_secs(15);
        while self.host().radeon.recovering() {
            assert!(Clock::now() < until, "the worker never finished recovering");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            self.host().radeon.status().unwrap().phase,
            Phase::Ready,
            "the startup recovery failed"
        );
    }
    /// The journal as the worker reads it, between its jobs. A test that opened the file itself
    /// while the worker saved it made that save fail ("access denied": Windows does not replace a
    /// file that is open), and the restore then waited for the worker's next retry, a minute on.
    fn journal(&self) -> Vec<Entry> {
        let worker = self.host().radeon.worker().unwrap();
        worker.pending(Duration::from_secs(15)).unwrap()
    }
    /// Every restore the session machine handed on has finished.
    fn flush(&self) {
        assert!(self.host().radeon.flush(Duration::from_secs(15)));
    }
    async fn panel(&self) -> api::Radeon {
        radeon(&self.get("/radeon").await)
    }
    /// A write of `changes` to the GPU.
    async fn write(&self, changes: serde_json::Value) -> Reply {
        self.write_body(&json!({"gpu": GPU, "changes": changes}).to_string())
            .await
    }
    async fn write_body(&self, body: &str) -> Reply {
        post_json(&self.s.f, &self.s.connection, "/radeon/settings", body).await
    }
    /// Host control, and a game being streamed.
    fn owner(&self) -> Arc<Session> {
        self.host_control(true);
        self.streaming()
    }
    /// An owner who changed `pacing.limit` from 140 to 200.
    async fn changed(&self) -> Arc<Session> {
        let stream = self.owner();
        let panel = radeon(
            &self
                .write(json!([{"id": "pacing.limit", "value": 200}]))
                .await,
        );
        assert_eq!(
            setting(&panel, "pacing.limit").value,
            Some(Api::Integer(200))
        );
        assert_eq!(self.fake.value("pacing.limit"), Raw::Int(200));
        assert_eq!(self.journal().len(), 1);
        stream
    }
    /// The change was put back and the journal is empty.
    fn assert_restored(&self) {
        assert_eq!(self.fake.value("pacing.limit"), Raw::Int(140));
        assert_eq!(self.journal(), vec![]);
    }
    /// The change is still there and still journaled.
    fn assert_kept(&self) {
        assert_eq!(self.fake.value("pacing.limit"), Raw::Int(200));
        assert_eq!(self.journal().len(), 1);
    }
}

async fn post_json(f: &Fixture, connection: &Connection, path: &str, body: &str) -> Reply {
    send(
        f,
        true,
        connection,
        "POST",
        &format!("{PREFIX}{path}"),
        Some("application/json"),
        body,
    )
    .await
}
fn radeon(reply: &Reply) -> api::Radeon {
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.headers[header::CACHE_CONTROL], "no-store");
    serde_json::from_slice(&reply.body).unwrap()
}
fn setting<'a>(panel: &'a api::Radeon, id: &str) -> &'a api::Setting {
    panel.gpus[0]
        .settings
        .iter()
        .find(|setting| setting.id == id)
        .unwrap_or_else(|| panic!("the panel has no {id}"))
}
/// The error names this setting.
fn assert_names(reply: &Reply, id: &str) {
    assert_eq!(reply.json()["error"]["setting"], id, "{}", reply.text());
}

// A game's Radeon profile (plan item B5): applied at launch through the journaled path, shown to
// the owner in the session, and restored with the session.

use rubylight_native_api::session::{ProfileState, RadeonProfile};

impl Rig {
    /// The first app carries `extra` (a Radeon profile) from now on.
    fn app_with(&self, extra: serde_json::Value) -> butterpollo_session::state::App {
        let mut apps = self.host().apps.write().unwrap();
        for (key, value) in extra.as_object().unwrap() {
            apps[0].extra.insert(key.clone(), value.clone());
        }
        apps[0].clone()
    }
    fn launched(&self, app: &butterpollo_session::state::App, fps: Option<u32>) {
        butterpollo_stream::radeon_profile::launched(
            self.host(),
            &self.client.uuid,
            Some(app),
            fps,
        );
    }
    /// The profile status once it left Applying.
    fn settled(&self) -> RadeonProfile {
        self.settled_for(&self.client.uuid)
    }
    fn settled_for(&self, device: &str) -> RadeonProfile {
        let until = Clock::now() + Duration::from_secs(15);
        loop {
            let held = self.host().notes.profile(device);
            if let Some(held) = held
                && held.status.state != ProfileState::Applying
            {
                return held.status;
            }
            assert!(Clock::now() < until, "the Radeon profile never settled");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    /// A launch of `app` from `device` (not necessarily the test's own).
    fn launched_by(&self, device: &str, app: &butterpollo_session::state::App, fps: Option<u32>) {
        butterpollo_stream::radeon_profile::launched(self.host(), device, Some(app), fps);
    }
    /// The game that runs now belongs to `owner`.
    fn game_owned_by(&self, owner: &str) {
        let app = self.host().apps.read().unwrap()[0].clone();
        let mut running =
            butterpollo_stream::process::RunningApp::with_environment(&app, Default::default())
                .unwrap();
        running.owner = owner.into();
        *self.host().current_app.lock().unwrap() = Some(running);
    }
    /// `device`'s Radeon settings go back in the end, wherever the session stands.
    fn put_back(&self, device: &str) {
        let worker = self.host().radeon.worker().unwrap();
        assert!(worker.restore_session(device).unwrap().is_clean());
    }
}

/// Wait for `done`: the profile and its restore run off the launch path.
fn until(what: &str, mut done: impl FnMut() -> bool) {
    let end = Clock::now() + Duration::from_secs(15);
    while !done() {
        assert!(Clock::now() < end, "{what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[path = "radeon/panel_tests.rs"]
mod panel_tests;
#[path = "radeon/profile_tests.rs"]
mod profile_tests;
#[path = "radeon/restore_tests.rs"]
mod restore_tests;
