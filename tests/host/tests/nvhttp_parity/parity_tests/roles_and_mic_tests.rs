//! NVHTTP microphone-only, input-only, and monitor role resolution tests.
use super::*;

#[test]
fn microphone_only_does_not_make_the_device_the_remote_input_owner() {
    let f = Fixture::new();
    let client = f.client(u32::MAX);
    let mut launch = f.launch(client.clone(), Role::InputOnly);
    launch
        .options
        .insert(butterpollo_session::mic::ONLY_OPTION.into(), "1".into());
    assert!(launch.mic_only());
    let session = butterpollo_stream::state::Session::new(
        launch,
        butterpollo_media_policy::rtsp::Negotiated::default(),
    );
    f.host
        .sessions
        .lock()
        .unwrap()
        .active
        .insert(session.launch.id.clone(), session.clone());
    assert!(remote_owner(&f.host, &client.uuid) == remote::Owner::None);
}

#[test]
fn a_device_that_may_only_view_streams_cannot_start_microphone_only() {
    let f = Fixture::new();
    let viewer = f.client(butterpollo_session::state::LATER_DEVICE_PERMISSION);
    let app_id = f.host.apps.read().unwrap()[0].id();
    let mut args = Args::new();
    args.insert("input_only".into(), "1".into());
    args.insert(butterpollo_session::mic::ONLY_OPTION.into(), "1".into());
    // /launch, /resume, and /launch naming the Resume tile or the running game.
    for (control, resume) in [
        (None, false),
        (None, true),
        (Some(Control::Resume), false),
        (Some(Control::RunningGame), false),
    ] {
        let resolved = resolve_launch_app(
            &f.host,
            &args,
            &viewer,
            app_id,
            control,
            remote::Owner::None,
            resume,
        );
        assert!(resolved.is_err(), "{control:?} resume={resume}");
    }
}

#[test]
fn a_device_that_may_only_view_streams_starts_no_remote_monitor_or_input_by_query() {
    let f = Fixture::new();
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert("enable_input_only_mode".into(), "true".into());
    let viewer = f.client(butterpollo_session::state::LATER_DEVICE_PERMISSION);
    let player = f.client(u32::MAX);
    let app_id = f.host.apps.read().unwrap()[0].id();
    for (option, role) in [
        ("remote_monitor", Role::RemoteMonitor),
        ("input_only", Role::InputOnly),
    ] {
        let mut args = Args::new();
        args.insert(option.into(), "1".into());
        // /resume, and /launch naming the Resume tile or the running game: all a viewer may call.
        for (control, resume) in [
            (None, true),
            (Some(Control::Resume), true),
            (Some(Control::RunningGame), true),
        ] {
            let refused = resolve_launch_app(
                &f.host,
                &args,
                &viewer,
                app_id,
                control,
                remote::Owner::None,
                resume,
            )
            .err()
            .map(|error| error.to_string());
            assert_eq!(
                refused.as_deref(),
                Some("a remote monitor or remote input session needs the launch permission"),
                "{option} {control:?}"
            );
        }
        let (resolved, ..) = resolve_launch_app(
            &f.host,
            &args,
            &player,
            app_id,
            None,
            remote::Owner::None,
            false,
        )
        .unwrap();
        assert_eq!(resolved, role, "{option}");
    }
}

#[test]
fn a_microphone_only_launch_needs_no_input_only_mode_and_keeps_no_input() {
    let f = Fixture::new();
    let client = f.client(u32::MAX);
    let app_id = f.host.apps.read().unwrap()[0].id();
    let mut args = Args::new();
    args.insert("input_only".into(), "1".into());
    let resolve = |args: &Args| {
        resolve_launch_app(
            &f.host,
            args,
            &client,
            app_id,
            None,
            remote::Owner::None,
            false,
        )
    };
    // Plain input-only stays the administrator's choice.
    assert!(resolve(&args).is_err());
    args.insert(butterpollo_session::mic::ONLY_OPTION.into(), "1".into());
    let (role, app, _, coop) = resolve(&args).unwrap();
    assert_eq!(role, Role::InputOnly);
    // Naming an app does not start it.
    assert!(app.is_none() && coop.is_none());
    let mut stream = args.clone();
    stream.remove("input_only");
    assert!(resolve(&stream).is_err());
    let launch = build_launch(
        "127.0.0.1".parse().unwrap(),
        &args,
        client.clone(),
        0,
        role,
        [0; 16],
        1,
    );
    assert!(launch.mic_only());
    assert!(
        !launch
            .client
            .allows(butterpollo_input_policy::input::KEYBOARD_PERMISSION)
    );
    assert_eq!(
        launch.client.perm & butterpollo_session::state::ALL_INPUT,
        0
    );
    assert!(
        launch
            .client
            .allows(butterpollo_session::state::LAUNCH_APPS)
    );
    // It is a launch: never a resume or a remote session tile.
    assert!(
        resolve_launch_app(
            &f.host,
            &args,
            &client,
            app_id,
            Some(Control::Resume),
            remote::Owner::None,
            false
        )
        .is_err()
    );
    assert!(
        resolve_launch_app(
            &f.host,
            &args,
            &client,
            app_id,
            None,
            remote::Owner::None,
            true
        )
        .is_err()
    );
    // Without the option an input-only launch keeps the device's input.
    args.remove(butterpollo_session::mic::ONLY_OPTION);
    let input = build_launch(
        "127.0.0.1".parse().unwrap(),
        &args,
        client,
        0,
        Role::InputOnly,
        [0; 16],
        1,
    );
    assert!(!input.mic_only());
    assert_eq!(
        input.client.perm & butterpollo_session::state::ALL_INPUT,
        butterpollo_session::state::ALL_INPUT
    );
}

#[test]
fn input_and_monitor_roles_resolve_without_starting_an_app_or_preparing_hardware() {
    let f = Fixture::new();
    let client = f.client(u32::MAX);
    let args = Args::new();
    let resolve = |control, owner, resume| {
        resolve_launch_app(&f.host, &args, &client, 0, Some(control), owner, resume)
    };
    assert!(resolve(Control::Input, remote::Owner::None, false).is_err());
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert("enable_input_only_mode".into(), "true".into());
    for (control, role) in [
        (Control::Input, Role::InputOnly),
        (Control::Monitor, Role::RemoteMonitor),
    ] {
        let (resolved, app, _, coop) = resolve(control, remote::Owner::None, false).unwrap();
        assert_eq!(resolved, role);
        assert!(app.is_none() && coop.is_none());
    }
    assert_eq!(
        resolve(Control::Resume, remote::Owner::Monitor, true)
            .unwrap()
            .0,
        Role::RemoteMonitor
    );
    assert!(resolve(Control::Resume, remote::Owner::None, true).is_err());
    let session = f.session(client.clone(), Role::InputOnly);
    assert!(remote_owner(&f.host, &client.uuid) == remote::Owner::Input);
    assert!(validate_launch_request(&f.host, &client, Some(Control::Monitor)).is_break());
    assert!(validate_launch_request(&f.host, &client, Some(Control::Input)).is_break());
    assert_eq!(session.launch.role, Role::InputOnly);
    assert!(f.host.current_app.lock().unwrap().is_none());
    assert!(session.launch.preparation.lock().unwrap().is_none());
}
