//! Live tests of private displays on this PC, all ignored: each changes the monitors of the
//! desktop it runs on, so run one only on a PC whose user agreed. They need a pool and its
//! broker (Rubylight's or Vibepollo's) and KDE Plasma.
use super::*;
use kscreen::Layout;
use std::time::Duration;

/// What KScreen shows of the enabled outputs, for comparing a layout before and after; modes
/// by name, as KScreen renumbers them.
fn shown(layout: &Layout) -> Vec<String> {
    let mut shown: Vec<String> = layout
        .outputs
        .iter()
        .filter(|o| o.enabled)
        .map(|o| {
            let rank = o.modes.iter().position(|m| m.id == o.current_mode_id);
            let mode = rank.map(|rank| &o.modes[rank]);
            format!(
                "{} mode #{} {} ({} Hz) at {},{} scale {} priority {}",
                o.name,
                rank.unwrap_or(usize::MAX),
                mode.map_or("?", |m| m.name.as_str()),
                mode.map_or(0.0, |m| m.refresh_rate),
                o.pos.x,
                o.pos.y,
                o.scale,
                o.priority
            )
        })
        .collect();
    shown.sort();
    shown
}

/// On this PC, with a pool and its broker: a display at `RUBYLIGHT_LIVE_MODE` (`WxH@mHz`,
/// default `1280x720@60000`) in the layout `RUBYLIGHT_LIVE_LAYOUT` names (default exclusive),
/// held `RUBYLIGHT_LIVE_HOLD` seconds (default 5), then dropped; the monitors must be back as
/// they were. It changes the
/// desktop: run it only on a PC whose user agreed.
#[test]
#[ignore = "changes this PC's monitors"]
fn live_a_private_display_comes_and_goes() {
    let layout = std::env::var("RUBYLIGHT_LIVE_LAYOUT").unwrap_or_else(|_| "exclusive".into());
    let hold: u64 = std::env::var("RUBYLIGHT_LIVE_HOLD")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    let mode = std::env::var("RUBYLIGHT_LIVE_MODE").unwrap_or_else(|_| "1280x720@60000".into());
    let (size, millihz) = mode.split_once('@').expect("WxH@mHz");
    let (width, height) = size.split_once('x').expect("WxH@mHz");
    let (width, height, millihz): (u32, u32, u32) = (
        width.parse().unwrap(),
        height.parse().unwrap(),
        millihz.parse().unwrap(),
    );
    eprintln!("status {}", virtual_display_status());
    assert!(virtual_display_available(), "{}", virtual_display_status());
    let before = kscreen::query().unwrap();
    let started = std::time::Instant::now();
    let display = VirtualDisplay::create_with(&PrivateRequest {
        stable_id: "rubylight-live-test".into(),
        width,
        height,
        refresh_millihz: millihz,
        arrangement: Arrangement::parse(&layout).unwrap(),
        scale_percent: None,
    })
    .unwrap();
    eprintln!("{} up in {:?}", display.name, started.elapsed());
    let during = kscreen::query().unwrap();
    eprintln!("during {:#?}", shown(&during));
    let output = during.output(&display.name).expect("the output is listed");
    assert!(output.enabled);
    assert_eq!(
        joinable_mode("rubylight-live-test"),
        Some((width, height, millihz))
    );
    std::thread::sleep(Duration::from_secs(hold));
    let name = display.name.clone();
    let started = std::time::Instant::now();
    drop(display);
    eprintln!("dropped in {:?}", started.elapsed());
    // KScreen takes a moment to list the outputs again.
    let mut after = kscreen::query().unwrap();
    for _ in 0..30 {
        if shown(&after) == shown(&before) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
        after = kscreen::query().unwrap();
    }
    eprintln!("after {:#?}", shown(&after));
    assert_eq!(shown(&after), shown(&before));
    assert!(after.output(&name).is_none_or(|o| !o.enabled));
}

fn live_request(stable_id: &str, width: u32, arrangement: Arrangement) -> PrivateRequest {
    PrivateRequest {
        stable_id: stable_id.into(),
        width,
        height: 720,
        refresh_millihz: 60_000,
        arrangement,
        scale_percent: None,
    }
}

/// On this PC: two private displays at once. The second asks for exclusive but joins the first
/// beside it; dropping the first leaves the second, and dropping the second brings the
/// monitors back. It changes the desktop: run it only on a PC whose user agreed.
#[test]
#[ignore = "changes this PC's monitors"]
fn live_two_private_displays_at_once() {
    assert!(virtual_display_available(), "{}", virtual_display_status());
    let before = kscreen::query().unwrap();
    let first = VirtualDisplay::create_with(&live_request(
        "rubylight-live-one",
        1280,
        Arrangement::Extended,
    ))
    .unwrap();
    let second = VirtualDisplay::create_with(&live_request(
        "rubylight-live-two",
        1024,
        Arrangement::Exclusive,
    ))
    .unwrap();
    assert_ne!(first.name, second.name);
    let both = kscreen::query().unwrap();
    eprintln!("both {:#?}", shown(&both));
    for name in [&first.name, &second.name] {
        assert!(both.output(name).is_some_and(|o| o.enabled), "{name} is on");
    }
    // The physical monitor stays: the second display joined instead of taking the desktop.
    assert!(both.output("DP-1").is_some_and(|o| o.enabled));
    std::thread::sleep(Duration::from_secs(3));
    let (first_name, second_name) = (first.name.clone(), second.name.clone());
    drop(first);
    let one = kscreen::query().unwrap();
    eprintln!("one {:#?}", shown(&one));
    assert!(one.output(&first_name).is_none_or(|o| !o.enabled));
    assert!(one.output(&second_name).is_some_and(|o| o.enabled));
    assert_eq!(joinable_mode("rubylight-live-one"), None);
    std::thread::sleep(Duration::from_secs(2));
    drop(second);
    let mut after = kscreen::query().unwrap();
    for _ in 0..30 {
        if shown(&after) == shown(&before) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
        after = kscreen::query().unwrap();
    }
    eprintln!("after {:#?}", shown(&after));
    assert_eq!(shown(&after), shown(&before));
}

/// On this PC, half of a crash: an exclusive private display, held until the process is
/// killed (`kill -9`), so nothing restores the monitors. [`live_recovery_after_a_crash`] is
/// the other half. It changes the desktop: run it only on a PC whose user agreed.
#[test]
#[ignore = "changes this PC's monitors and waits to be killed"]
fn live_hold_until_killed() {
    assert!(virtual_display_available(), "{}", virtual_display_status());
    let display = VirtualDisplay::create_with(&live_request(
        "rubylight-live-crash",
        1280,
        Arrangement::Exclusive,
    ))
    .unwrap();
    eprintln!("HOLDING {}", display.name);
    std::thread::sleep(Duration::from_secs(120));
    // Not reached when killed; a run that is not killed restores as usual.
    drop(display);
}

/// On this PC, after [`live_hold_until_killed`] was killed: the host's start-up recovery
/// brings the monitors back, gives the connector back and clears the state file.
#[test]
#[ignore = "changes this PC's monitors"]
fn live_recovery_after_a_crash() {
    let file = private::state_path().expect("a state folder");
    assert!(file.exists(), "a killed hold leaves {}", file.display());
    let pending = kscreen::query().unwrap();
    eprintln!("left by the crash {:#?}", shown(&pending));
    recover_after_restart().unwrap();
    let mut after = kscreen::query().unwrap();
    for _ in 0..30 {
        if after.output("DP-1").is_some_and(|o| o.enabled) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
        after = kscreen::query().unwrap();
    }
    eprintln!("recovered {:#?}", shown(&after));
    assert!(after.output("DP-1").is_some_and(|o| o.enabled));
    assert!(
        after
            .outputs
            .iter()
            .all(|o| !broker::managed_name(&o.name) || !o.enabled)
    );
    assert!(!file.exists(), "the state file is cleared");
    for connector in pool::scan(Path::new(pool::DRM_ROOT)) {
        assert_eq!(connector.status, "disconnected", "{}", connector.name);
    }
}
