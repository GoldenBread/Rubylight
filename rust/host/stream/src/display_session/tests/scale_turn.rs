//! Tests for display scale resolution, physical/virtual rotation and turn-following,
//! HDR calibration carry-over, and configured window targets.
use super::*;

#[test]
fn the_configured_scale_reaches_every_stream_display_including_remote_monitors() {
    let mut seen = None;
    let display = r"\\.\DISPLAY7";
    scale_display(
        display,
        125,
        (2560, 1440),
        |output, scale, width, height| {
            seen = Some((output.to_owned(), scale, width, height));
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(seen, Some((display.to_owned(), 125, 2560, 1440)));
    // A display that cannot be scaled is a loud failure, not a silent skip.
    assert!(scale_display("x", 125, (1, 1), |_, _, _, _| anyhow::bail!("no")).is_err());
    // A physical display keeps the user's own scale; a retained remote monitor
    // and a virtual display take the configured one.
    assert_eq!(virtual_display_scale(false, 125), 0);
    assert_eq!(virtual_display_scale(true, 125), 125);
}

fn rotation_config(option: &str, manual: &str) -> Config {
    let mut config = Config::default();
    config
        .values
        .insert("dd_rotation_option".into(), option.into());
    config
        .values
        .insert("dd_manual_rotation".into(), manual.into());
    config
}

#[test]
fn the_clients_display_scale_comes_before_the_configured_one() {
    let mut config = Config::default();
    let options = |pairs: &[(&str, &str)]| -> std::collections::BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    };
    let unread = || -> Result<u32> { panic!("the scale range is read only for a request") };
    let setup = |config: &Config, pairs: &[(&str, &str)], virtual_layout, keeps| {
        scale_setup(config, &options(pairs), virtual_layout, keeps, || Ok(500)).unwrap()
    };
    let plain = |scale, learns| ScaleSetup {
        scale,
        requested: false,
        lowered: None,
        learns,
    };
    // Nothing configured or asked for: Windows' own scale, learned if the device keeps one.
    assert_eq!(setup(&config, &[], true, true), plain(0, true));
    assert_eq!(setup(&config, &[], true, false), plain(0, false));
    config
        .values
        .insert("dd_virtual_display_scale".into(), "125".into());
    assert_eq!(
        scale_setup(&config, &options(&[]), true, true, unread).unwrap(),
        plain(125, false),
        "a configured scale is applied and nothing is learned over it"
    );
    // The device's request wins on a virtual display and suppresses learning.
    assert_eq!(
        setup(&config, &[("rlDisplayScale", "200")], true, true),
        ScaleSetup {
            scale: 200,
            requested: true,
            lowered: None,
            learns: false,
        }
    );
    // The render scale (`scaleFactor`) never sets the Windows scale.
    assert_eq!(
        scale_setup(
            &config,
            &options(&[("scaleFactor", "200")]),
            true,
            true,
            unread
        )
        .unwrap(),
        plain(125, false)
    );
    assert!(
        scale_setup(
            &config,
            &options(&[("rlDisplayScale", "275")]),
            true,
            true,
            unread
        )
        .is_err()
    );
    // A physical display keeps the user's scale whatever the client asks, and the
    // request neither reads the range nor changes what is learned there.
    assert_eq!(
        scale_setup(
            &config,
            &options(&[("rlDisplayScale", "200")]),
            false,
            true,
            unread
        )
        .unwrap(),
        plain(0, false)
    );
    let unset = Config::default();
    assert_eq!(
        scale_setup(
            &unset,
            &options(&[("rlDisplayScale", "200")]),
            false,
            true,
            unread
        )
        .unwrap(),
        plain(0, true)
    );
    // Too large for the resolution: lowered, with the warning for the device.
    let lowered = scale_setup(
        &config,
        &options(&[("rlDisplayScale", "300")]),
        true,
        false,
        || Ok(175),
    )
    .unwrap();
    assert_eq!((lowered.scale, lowered.requested), (175, true));
    assert_eq!(lowered.warning(175), lowered.lowered);
    assert!(lowered.warning(175).unwrap().contains("at most 175%"));
}

#[test]
fn a_requested_scale_windows_refuses_is_a_warning_and_a_configured_one_fails() {
    let refuse = |_: &str, _: i64, _: u32, _: u32| -> Result<()> {
        anyhow::bail!("requested DPI scale is outside this display's range")
    };
    // The device's request: the stream goes on at the scale Windows shows, recovery puts
    // nothing back, and the device hears why.
    let (in_force, baseline) =
        set_up_scale("out", 200, true, None, (1920, 1080), false, refuse, |_| {
            panic!("nothing is learned under a request")
        })
        .unwrap();
    assert_eq!((in_force, baseline), (0, None));
    let setup = ScaleSetup {
        scale: 200,
        requested: true,
        lowered: None,
        learns: false,
    };
    assert!(
        setup
            .warning(in_force)
            .unwrap()
            .contains("did not take the 200%")
    );
    assert_eq!(setup.warning(200), None, "taken: no warning");
    // A request with an unreadable range is asked of Windows as chosen; a refusal is the
    // same warning, not a failed stream.
    assert_eq!(
        fit_client_scale(200, Err(anyhow::anyhow!("no"))),
        (200, None)
    );
    // The host's or the device's configured scale stays fatal.
    assert!(
        set_up_scale(
            "out",
            200,
            false,
            None,
            (1920, 1080),
            false,
            refuse,
            |_| Ok(100)
        )
        .is_err()
    );
    // A request that Windows takes is in force for recovery to put back.
    let (in_force, _) = set_up_scale(
        "out",
        175,
        true,
        Some(150),
        (1920, 1080),
        false,
        |_, percent, _, _| {
            assert_eq!(percent, 175, "the request wins over a kept scale");
            Ok(())
        },
        |_| panic!("not read"),
    )
    .unwrap();
    assert_eq!(in_force, 175);
}

#[test]
fn a_client_scale_too_large_for_the_resolution_is_lowered_with_a_warning() {
    let (scale, warning) = fit_client_scale(200, Ok(150));
    assert_eq!(scale, 150);
    assert!(warning.unwrap().contains("at most 150%"));
    assert_eq!(fit_client_scale(150, Ok(150)), (150, None));
    assert_eq!(fit_client_scale(125, Ok(300)), (125, None));
    // An unreadable range leaves the choice to Windows, which fails loudly if it must.
    assert_eq!(
        fit_client_scale(200, Err(anyhow::anyhow!("no"))),
        (200, None)
    );
}

#[test]
fn a_physical_display_takes_only_the_devices_own_scale() {
    let client = |overrides: serde_json::Value| -> butterpollo_session::state::Client {
        serde_json::from_value(serde_json::json!({
            "name": "phone", "cert": "", "uuid": "u", "config_overrides": overrides,
        }))
        .unwrap()
    };
    assert_eq!(physical_scale(&client(serde_json::json!({}))), 0);
    assert_eq!(
        physical_scale(&client(
            serde_json::json!({"dd_virtual_display_scale": 150})
        )),
        150
    );
    assert_eq!(
        physical_scale(&client(
            serde_json::json!({"dd_virtual_display_scale": "-1"})
        )),
        -1
    );
    assert_eq!(
        physical_scale(&client(
            serde_json::json!({"dd_virtual_display_scale": "big"})
        )),
        0
    );
    assert_eq!(
        physical_scale(&client(serde_json::json!({"dd_rotation_option": "auto"}))),
        0
    );
}

#[test]
fn a_virtual_display_is_made_in_the_portrait_streams_shape() {
    let phone = (1080, 2400);
    assert_eq!(
        virtual_display_size(&rotation_config("auto", "0"), (2560, 1440), phone),
        (1440, 2560)
    );
    assert_eq!(
        virtual_display_size(&rotation_config("manual", "90"), (1920, 1080), (1920, 1080)),
        (1080, 1920)
    );
    assert_eq!(
        virtual_display_size(&Config::default(), (2560, 1440), phone),
        (2560, 1440)
    );
}

#[test]
fn a_physical_display_turns_after_its_mode_is_set_in_its_shape_before_the_turn() -> Result<()> {
    let phone = (1080, 2400);
    let landscape = || Ok((0, (2560, 1440)));
    // Off: the display is not even read.
    let off = plan_physical_turn(&Config::default(), phone, Some(phone), || {
        panic!("not read")
    })?;
    assert_eq!(
        off,
        PhysicalTurn {
            degrees: None,
            before: Some(phone),
            after: Some(phone)
        }
    );
    // A portrait phone turns a landscape display: its mode goes on landscape.
    let auto = rotation_config("auto", "0");
    assert_eq!(
        plan_physical_turn(&auto, phone, Some(phone), landscape)?,
        PhysicalTurn {
            degrees: Some(90),
            before: Some((2400, 1080)),
            after: Some(phone)
        }
    );
    // Without a resolution of its own the display keeps its mode and turns.
    assert_eq!(
        plan_physical_turn(&auto, phone, None, landscape)?.degrees,
        Some(90)
    );
    // A display already in the stream's shape stays as it is.
    assert_eq!(
        plan_physical_turn(&auto, (1920, 1080), Some((1920, 1080)), landscape)?,
        PhysicalTurn {
            degrees: None,
            before: Some((1920, 1080)),
            after: Some((1920, 1080))
        }
    );
    // Upside down keeps the shape.
    assert_eq!(
        plan_physical_turn(
            &rotation_config("manual", "180"),
            (1920, 1080),
            Some((1920, 1080)),
            landscape
        )?,
        PhysicalTurn {
            degrees: Some(180),
            before: Some((1920, 1080)),
            after: Some((1920, 1080))
        }
    );
    // A display turned to 270° goes back upright for a landscape stream.
    assert_eq!(
        plan_physical_turn(&auto, (1920, 1080), Some((1920, 1080)), || Ok((
            270,
            (1440, 2560)
        )))?,
        PhysicalTurn {
            degrees: Some(0),
            before: Some((1080, 1920)),
            after: Some((1920, 1080))
        }
    );
    // A display that cannot be read fails the stream.
    assert!(plan_physical_turn(&auto, phone, None, || anyhow::bail!("gone")).is_err());
    Ok(())
}

#[derive(Default)]
struct FakeLease {
    calls: Vec<String>,
    refuse_turn: bool,
    /// The size its own virtual display shows; None for a shared one.
    size: Option<(u32, u32)>,
    reshaped: Cell<Option<(u32, u32)>>,
    refuse_shape: bool,
}
impl TurnLease for FakeLease {
    fn virtual_size(&self) -> Option<(u32, u32)> {
        self.size
    }
    fn reshape_virtual(&self, width: u32, height: u32) -> Result<()> {
        self.reshaped.set(Some((width, height)));
        if self.refuse_shape {
            anyhow::bail!("Windows did not show {width}x{height}");
        }
        Ok(())
    }
}
impl PhysicalAdjust for FakeLease {
    fn rotate(&mut self, degrees: u16) -> Result<()> {
        self.calls.push(format!("rotate {degrees}"));
        if self.refuse_turn {
            anyhow::bail!("Windows refused the rotation");
        }
        Ok(())
    }
    fn scale(&mut self, configured: i64) -> Result<()> {
        self.calls.push(format!("scale {configured}"));
        Ok(())
    }
}

#[test]
fn a_physical_display_turns_before_it_scales_and_a_refused_turn_fails_the_stream() {
    let mut lease = FakeLease::default();
    adjust_physical(&mut lease, Some(90), 150).unwrap();
    assert_eq!(lease.calls, ["rotate 90", "scale 150"]);
    // Nothing to turn and the device's scale left alone: the lease is untouched.
    let mut lease = FakeLease::default();
    adjust_physical(&mut lease, None, 0).unwrap();
    assert!(lease.calls.is_empty());
    let mut lease = FakeLease::default();
    adjust_physical(&mut lease, None, -1).unwrap();
    assert_eq!(lease.calls, ["scale -1"]);
    let mut lease = FakeLease {
        refuse_turn: true,
        ..Default::default()
    };
    assert!(adjust_physical(&mut lease, Some(90), 150).is_err());
    assert_eq!(lease.calls, ["rotate 90"]);
}

#[test]
fn a_virtual_display_changes_shape_when_its_stream_turns() -> Result<()> {
    let unread = || -> Result<(u16, (u32, u32))> { panic!("a virtual display is not read") };
    let mut lease = FakeLease {
        size: Some((2400, 1080)),
        ..Default::default()
    };
    // The phone turned to portrait: the display takes the portrait shape.
    assert_eq!(
        follow_turn(&mut lease, Follow::Virtual, (1080, 2400), unread)?,
        Some((1080, 2400))
    );
    assert_eq!(lease.reshaped.get(), Some((1080, 2400)));
    assert!(lease.calls.is_empty(), "a virtual display is never turned");
    // A rate change or a stream in the display's shape leaves it.
    let mut lease = FakeLease {
        size: Some((2400, 1080)),
        ..Default::default()
    };
    assert_eq!(
        follow_turn(&mut lease, Follow::Virtual, (1920, 1080), unread)?,
        None
    );
    assert_eq!(lease.reshaped.get(), None);
    // A shared display, or one Windows refused, keeps its shape and says so.
    let mut lease = FakeLease::default();
    assert!(follow_turn(&mut lease, Follow::Virtual, (1080, 2400), unread).is_err());
    let mut lease = FakeLease {
        size: Some((2400, 1080)),
        refuse_shape: true,
        ..Default::default()
    };
    assert!(follow_turn(&mut lease, Follow::Virtual, (1080, 2400), unread).is_err());
    Ok(())
}

#[test]
fn a_physical_display_turns_with_its_stream_and_back() -> Result<()> {
    // Upright landscape; the phone turned to portrait.
    let reads = std::cell::RefCell::new(vec![(90, (1440, 2560)), (0, (2560, 1440))]);
    let read = || Ok(reads.borrow_mut().pop().unwrap());
    let mut lease = FakeLease::default();
    assert_eq!(
        follow_turn(&mut lease, Follow::Physical, (1080, 2400), read)?,
        Some((1440, 2560))
    );
    assert_eq!(lease.calls, ["rotate 90"]);
    assert_eq!(
        lease.reshaped.get(),
        None,
        "a physical display is turned, not reshaped"
    );
    // Turned, and the phone went back to landscape: the display turns upright.
    let reads = std::cell::RefCell::new(vec![(0, (2560, 1440)), (90, (1440, 2560))]);
    let read = || Ok(reads.borrow_mut().pop().unwrap());
    let mut lease = FakeLease::default();
    assert_eq!(
        follow_turn(&mut lease, Follow::Physical, (2400, 1080), read)?,
        Some((2560, 1440))
    );
    assert_eq!(lease.calls, ["rotate 0"]);
    // Already in the stream's shape: read once and left.
    let mut lease = FakeLease::default();
    let reads = std::cell::RefCell::new(vec![(0, (2560, 1440))]);
    let read = || Ok(reads.borrow_mut().pop().unwrap());
    assert_eq!(
        follow_turn(&mut lease, Follow::Physical, (2400, 1080), read)?,
        None
    );
    assert!(lease.calls.is_empty());
    // Another stream keeps the display's rotation: nothing turned.
    let reads = std::cell::RefCell::new(vec![(0, (2560, 1440)), (0, (2560, 1440))]);
    let read = || Ok(reads.borrow_mut().pop().unwrap());
    let mut lease = FakeLease::default();
    assert_eq!(
        follow_turn(&mut lease, Follow::Physical, (1080, 2400), read)?,
        None
    );
    // A refused turn or an unreadable display fails.
    let mut lease = FakeLease {
        refuse_turn: true,
        ..Default::default()
    };
    assert!(
        follow_turn(&mut lease, Follow::Physical, (1080, 2400), || Ok((
            0,
            (2560, 1440)
        )))
        .is_err()
    );
    let mut lease = FakeLease::default();
    assert!(
        follow_turn(
            &mut lease,
            Follow::Physical,
            (1080, 2400),
            || anyhow::bail!("display unavailable")
        )
        .is_err()
    );
    assert!(lease.calls.is_empty());
    Ok(())
}

#[test]
fn only_an_auto_rotation_with_a_lease_follows_a_turned_stream() -> Result<()> {
    let fixture = crate::state::test_support::Fixture::new();
    let mut prepared = test_prepared(&fixture.host);
    let before = prepared.recovery_dimensions;
    // Not auto: a turned stream leaves the display alone.
    prepared.follow_shape((before.1, before.0))?;
    // Auto, but without a lease of its own (a remote monitor): nothing to turn.
    prepared.follow = Some(Follow::Physical);
    prepared.follow_shape((before.1, before.0))?;
    assert_eq!(prepared.recovery_dimensions, before);
    Ok(())
}

/// RL56's 3.4.0 log warned of the missing calibration at every stream.
#[test]
fn a_missing_calibration_is_warned_about_once() {
    let flag = AtomicBool::new(false);
    assert!(first_time(&flag));
    assert!(!first_time(&flag));
    assert!(!first_time(&flag));
    // A calibration found, then lost again (the profile deleted, the
    // display swapped): warned again, once.
    warn_again(&flag);
    assert!(first_time(&flag));
    assert!(!first_time(&flag));
}

#[test]
fn a_virtual_hdr_display_gets_the_physical_calibration_only_when_nothing_else_is_chosen() {
    let looked = Cell::new(0);
    let calibration = || {
        looked.set(looked.get() + 1);
        Some("calibrated.icm".to_string())
    };
    assert_eq!(
        stream_profile(true, None, true, calibration),
        Some("calibrated.icm".into())
    );
    assert_eq!(
        stream_profile(true, Some(""), true, calibration),
        Some("calibrated.icm".into())
    );
    assert_eq!(looked.get(), 2);
    // The device's own profile wins; SDR, a physical display and the
    // setting off never read the physical display's.
    assert_eq!(
        stream_profile(true, Some("device.icm"), true, calibration),
        Some("device.icm".into())
    );
    assert_eq!(stream_profile(false, None, true, calibration), None);
    assert_eq!(
        stream_profile(false, Some("device.icm"), true, calibration),
        None
    );
    assert_eq!(stream_profile(true, None, false, calibration), None);
    assert_eq!(looked.get(), 2);
    assert_eq!(stream_profile(true, None, true, || None), None);
}

#[test]
fn a_window_output_streams_that_window_and_a_display_name_does_not() {
    let window = Config::parse(r"output_name = window:0x2056c/812@\\.\DISPLAY1 ").unwrap();
    assert_eq!(
        configured_window(&window).unwrap(),
        Some(
            butterpollo_win_graphics::capture::WindowTarget::new(0x2056c, r"\\.\DISPLAY1")
                .owned_by(812)
        )
    );
    for display in ["", r"output_name=\\.\DISPLAY1", "output_name=Dell U2723QE"] {
        assert_eq!(
            configured_window(&Config::parse(display).unwrap()).unwrap(),
            None,
            "{display}"
        );
    }
    // A malformed target is refused instead of being streamed as a display.
    for broken in [
        "output_name=window:garbage",
        r"output_name=window:0@\\.\DISPLAY1",
        "output_name=window:12@",
        r"output_name=window:@\\.\DISPLAY1",
        // Without its process a reused handle could show another window.
        r"output_name=window:0x2056c@\\.\DISPLAY1",
    ] {
        let error = configured_window(&Config::parse(broken).unwrap()).unwrap_err();
        assert!(
            format!("{error:#}").contains("output_name"),
            "{broken}: {error:#}"
        );
    }
}

#[test]
fn a_closed_configured_window_refuses_the_stream_with_the_reason() {
    // No window has this handle; a minimised, reused or misplaced one is
    // refused the same way (capture::window's tests).
    let gone = butterpollo_win_graphics::capture::WindowTarget::new(0x7fff_fff0, r"\\.\DISPLAY1")
        .owned_by(9);
    let error = format!("{:#}", configured_window_open(&gone).unwrap_err());
    assert!(
        error.contains(r"output_name names the window window:2147483632/9@\\.\DISPLAY1"),
        "{error}"
    );
    assert!(error.contains("closed"), "{error}");
}

#[test]
fn a_window_stream_is_not_kept_for_a_reconnect_and_a_display_stream_is() {
    // Resuming a kept window stream would skip the window's check and
    // send its warnings to the first session; the launch prepares again.
    let fixture = crate::state::test_support::Fixture::new();
    let config = Config::default();
    let stream = Negotiated {
        width: 1920,
        height: 1080,
        fps: 60,
        ..Default::default()
    };
    let window = Ready::window(
        &fixture.host,
        &stream,
        &config,
        r"window:5/9@\\.\DISPLAY1".into(),
        Box::new(|| Ok(r"window:5/9@\\.\DISPLAY1".into())),
        Default::default(),
    )
    .unwrap();
    assert!(!window.display.kept_for_reconnect());
    let prepared = test_prepared(&fixture.host);
    let limiter =
        limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen).unwrap();
    let display = Ready::prepare((prepared, limiter)).unwrap();
    assert!(display.display.kept_for_reconnect());
}
