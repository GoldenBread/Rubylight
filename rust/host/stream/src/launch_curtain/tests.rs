//! The host's curtain: lock order, a replaced app, who may lift it and a resume that follows it.
use super::*;
use crate::process::RunningApp;
use crate::state::test_support::Fixture;
use butterpollo_session::state::App;
use rubylight_native_api::curtain::CurtainPhase;
use std::collections::BTreeMap;

fn launching_steam_app() -> RunningApp {
    let mut app = RunningApp::with_environment(&App::desktop(), BTreeMap::new()).unwrap();
    app.mark_launching();
    app.track_steam(r"C:\not-a-game");
    app
}

/// A curtain for a launching app, `generation` its app's.
fn curtained(f: &Fixture) -> String {
    let app = launching_steam_app();
    assert!(app.curtain_waits());
    let generation = app.generation.clone();
    *f.host.current_app.lock().unwrap() = Some(app);
    f.host
        .curtain
        .begin("launch-1", "device-a", generation.clone(), false);
    generation
}

/// One look that finds the game's window in front, asserting `current_app` is free throughout.
fn look_in_front(f: &Fixture) {
    look(
        &f.host,
        |_| {
            assert!(
                f.host.try_current_app().is_some(),
                "pid scan held current_app"
            );
            BTreeSet::from([30])
        },
        |pids| {
            assert!(
                f.host.try_current_app().is_some(),
                "window read held current_app"
            );
            assert_eq!(pids, &BTreeSet::from([30]));
            (vec![(30, false)], Some(30))
        },
        |_| {
            assert!(
                f.host.try_current_app().is_some(),
                "store lookup held current_app"
            );
            false
        },
        || {
            assert!(
                f.host.try_current_app().is_some(),
                "desktop read held current_app"
            );
            false
        },
    );
}

#[test]
fn launch_curtain_never_holds_current_app_during_os_calls() {
    let f = Fixture::new();
    curtained(&f);
    assert!(f.host.curtain.up());
    look_in_front(&f);
    assert!(f.host.curtain.up(), "one look in front is not enough");
    look_in_front(&f);
    assert!(!f.host.curtain.up());
    assert_eq!(f.host.curtain.state_for("launch-1").phase, Phase::Ready);
}

#[test]
fn launch_curtain_of_a_replaced_app_fails_instead_of_waiting() {
    let f = Fixture::new();
    curtained(&f);
    *f.host.current_app.lock().unwrap() = Some(launching_steam_app());
    let before = f.host.curtain.revision();
    look(
        &f.host,
        |_| panic!("a replaced app's processes are not read"),
        |_| panic!("a replaced app's windows are not read"),
        |_| false,
        || false,
    );
    let state = f.host.curtain.state_for("launch-1");
    assert_eq!(state.phase, Phase::Failed);
    assert!(!state.reason.is_empty());
    assert!(f.host.curtain.revision() > before);
    assert!(!f.host.curtain.up());
}

#[test]
fn launch_curtain_fails_when_the_app_ends() {
    let f = Fixture::new();
    curtained(&f);
    *f.host.current_app.lock().unwrap() = None;
    look(
        &f.host,
        |_| BTreeSet::new(),
        |_| (vec![], None),
        |_| false,
        || false,
    );
    assert_eq!(f.host.curtain.state_for("launch-1").phase, Phase::Failed);
}

#[test]
fn launch_curtain_secure_desktop_asks_for_the_player() {
    let f = Fixture::new();
    curtained(&f);
    look(
        &f.host,
        |_| BTreeSet::new(),
        |_| (vec![], None),
        |_| false,
        || true,
    );
    let snapshot = f.host.curtain.snapshot("device-a");
    assert_eq!(snapshot.phase, CurtainPhase::AttentionNeeded);
    assert!(snapshot.dismissible);
    assert_eq!(snapshot.prompt.unwrap().launcher, "windows");
}

#[test]
fn launch_curtain_is_lifted_only_by_the_device_that_launched() {
    let f = Fixture::new();
    curtained(&f);
    assert!(!f.host.curtain.snapshot("device-b").dismissible);
    assert_eq!(
        f.host.curtain.dismiss("device-b"),
        Err(DismissRefused::NotOwner)
    );
    assert!(f.host.curtain.up());
    let lifted = f.host.curtain.dismiss("device-a").unwrap();
    assert_eq!(lifted.phase, CurtainPhase::Dismissed);
    assert!(!lifted.active && !lifted.dismissible);
    assert_eq!(f.host.curtain.state_for("launch-1").phase, Phase::Dismissed);
    // A look after the lift changes nothing.
    look_in_front(&f);
    assert_eq!(f.host.curtain.state_for("launch-1").phase, Phase::Dismissed);
    let none = Tracker::default();
    assert_eq!(none.dismiss("device-a"), Err(DismissRefused::NoCurtain));
    assert!(!none.snapshot("device-a").active);
}

#[test]
fn launch_curtain_a_stream_without_one_reads_lifted() {
    let f = Fixture::new();
    curtained(&f);
    let other = f.host.curtain.state_for("launch-0");
    assert_eq!(other.phase, Phase::Ready);
    assert!(other.has(flags::DESKTOP_OR_LAUNCHER));
    f.host
        .curtain
        .begin("launch-2", "device-a", "g".into(), true);
    assert!(!f.host.curtain.up());
    assert_eq!(f.host.curtain.state_for("launch-2").phase, Phase::Ready);
}

#[test]
fn launch_curtain_follows_a_resume_of_the_same_game_by_its_owner() {
    let f = Fixture::new();
    let generation = curtained(&f);
    assert!(!f.host.curtain.rebind("launch-2", "device-b", &generation));
    assert!(
        !f.host
            .curtain
            .rebind("launch-2", "device-a", "another-game")
    );
    assert!(f.host.curtain.rebind("launch-2", "device-a", &generation));
    assert!(f.host.curtain.up());
    assert_eq!(
        f.host.curtain.state_for("launch-2").phase,
        Phase::StartingApp
    );
    assert_eq!(f.host.curtain.state_for("launch-1").phase, Phase::Ready);
}
