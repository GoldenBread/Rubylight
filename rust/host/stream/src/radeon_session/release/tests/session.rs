//! Holds of devices whose launch is still pending, through the host's real session machine.
use super::*;
use crate::state::test_support::Fixture;
use butterpollo_session::{session::Role, state::Client};

struct Streams {
    fixture: Fixture,
    profile: Profile,
    a: Client,
    b: Client,
}
impl Streams {
    /// Devices `a` and `b` are paired.
    fn new() -> Self {
        let fixture = Fixture::new();
        let profile = Profile::default();
        attach(&fixture.host.radeon, &profile, &fixture.host.directory);
        let a = fixture.client(u32::MAX);
        let mut b = a.clone();
        b.uuid = "second-device".into();
        Self {
            fixture,
            profile,
            a,
            b,
        }
    }
    fn hold(&self, client: &Client) {
        let (hold, result) = self.fixture.host.hold_vsync(&client.uuid, "", true);
        result.unwrap();
        hold.keep();
    }
    /// `b`'s launch waits for its client.
    fn queue_b(&self) -> String {
        let pending = self.fixture.launch(self.b.clone(), Role::Stream);
        let id = pending.id.clone();
        let mut sessions = self.fixture.host.sessions.lock().unwrap();
        sessions.pending.insert(id.clone(), pending);
        id
    }
    /// A launch of `b` onto the virtual display, accepted and waiting for its client.
    fn launch_b(&self) -> String {
        let mut session = self.fixture.host.session_begin(&self.b.uuid, None);
        let id = self.queue_b();
        self.hold(&self.b);
        session.succeed();
        id
    }
    fn assert_vsync(&self, value: u32) {
        assert!(self.fixture.host.radeon.flush(WAIT));
        assert_eq!(self.profile.vsync(), value);
    }
}

#[test]
fn a_launch_beside_a_stream_on_a_physical_display_keeps_its_force() {
    let streams = Streams::new();
    let host = &streams.fixture.host;
    let _a = streams.fixture.session(streams.a.clone(), Role::Stream);
    // The launch call returns and the machine settles on `a`'s stream before `b` connects.
    let id = streams.launch_b();
    host.session_tick();
    streams.assert_vsync(ON);
    {
        let mut sessions = host.sessions.lock().unwrap();
        let launch = sessions.pending[&id].clone();
        sessions.start(launch, Default::default()).unwrap();
    }
    host.session_tick();
    streams.assert_vsync(ON);
}

#[test]
fn the_first_stream_ending_keeps_the_force_of_a_pending_launch() {
    let streams = Streams::new();
    let host = &streams.fixture.host;
    streams.hold(&streams.a);
    let a = streams.fixture.session(streams.a.clone(), Role::Stream);
    let id = streams.launch_b();
    a.stop();
    host.session_tick();
    streams.assert_vsync(ON);
    // Nobody joins `b`'s launch: nothing holds the force any more.
    host.sessions.lock().unwrap().pending.remove(&id);
    host.session_tick();
    streams.assert_vsync(APPLICATION);
}

#[test]
fn an_earlier_launchs_profile_thread_keeps_the_force_of_a_pending_launch() {
    let streams = Streams::new();
    // `b` is applying its hold when `a`'s thread releases what `a`'s launch replaced.
    streams.queue_b();
    streams.hold(&streams.b);
    streams
        .fixture
        .host
        .release_replaced_settings(&streams.a.uuid, WAIT);
    streams.assert_vsync(ON);
}

/// `client`'s own Vertical sync value: Off.
fn choose_off(streams: &Streams, client: &Client) {
    let change = Change {
        id: VSYNC.into(),
        value: model::Value::Choice("off".into()),
    };
    let radeon = &streams.fixture.host.radeon;
    radeon.apply(&client.uuid, 0, vec![change]).unwrap();
}

fn assert_late(result: Result<(), String>) {
    let warning = result.unwrap_err();
    assert!(warning.contains("after a game restart"), "{warning}");
}

#[test]
fn a_takeover_says_the_replaced_value_delays_the_force() {
    let streams = Streams::new();
    let host = &streams.fixture.host;
    // `a` is suspended with its own value.
    choose_off(&streams, &streams.a);
    let mut session = host.session_begin(&streams.b.uuid, None);
    streams.queue_b();
    let (hold, result) = host.hold_vsync(&streams.b.uuid, "", true);
    assert_late(result);
    hold.keep();
    session.succeed();
    streams.assert_vsync(OFF);
    host.release_replaced_settings(&streams.b.uuid, WAIT);
    streams.assert_vsync(ON);
}

#[test]
fn another_live_streams_value_is_kept_without_a_warning() {
    let streams = Streams::new();
    let _a = streams.fixture.session(streams.a.clone(), Role::Stream);
    choose_off(&streams, &streams.a);
    streams.queue_b();
    streams.hold(&streams.b);
    streams.assert_vsync(OFF);
}

#[test]
fn only_the_devices_own_earlier_preset_delays_the_force() {
    let streams = Streams::new();
    let host = &streams.fixture.host;
    choose_off(&streams, &streams.a);
    // A choice in the stream menu stays without a warning.
    streams.hold(&streams.a);
    let mut app = App::desktop();
    app.extra
        .insert(radeon_profile::PROFILE.into(), "custom".into());
    app.extra.insert(
        radeon_profile::SETTINGS.into(),
        serde_json::json!({"nvidia_vsync.mode": "off"}),
    );
    let preset = radeon_profile::selected(&app.extra, Some(60))
        .unwrap()
        .unwrap()
        .for_driver(true);
    let status = rubylight_native_api::session::RadeonProfile {
        profile: "custom".into(),
        state: rubylight_native_api::session::ProfileState::Applied,
        message: None,
        settings: vec![VSYNC.into()],
    };
    host.notes
        .begin_profile(&streams.a.uuid, status, preset.changes);
    assert_late(host.hold_vsync(&streams.a.uuid, "", true).1);
}
