//! Tests of the private display's life and of availability, against a fake broker, a fake
//! KScreen and a temporary sysfs tree. Nothing here touches a real display, socket or sysfs,
//! except the ignored live test at the end, which changes this PC's monitors.
use super::broker::fake::{Fake, connector, serve, temp_dir};
use super::broker::{Broker, Trust};
use super::kscreen::{Layout, Mode, Output, Pos, Size};
use super::private::{Desktop, Manager, PrivateRequest, read_saved};
use super::*;
use std::sync::Mutex;
use std::time::Duration;

/// The desktop and the broker's view of it, shared by both fakes.
#[derive(Default)]
struct Sim {
    layout: Layout,
    broker_log: Vec<String>,
    applies: Vec<Vec<String>>,
    fail_apply: bool,
    refuse_mode: bool,
}

type Shared = Arc<Mutex<Sim>>;

struct FakeDesktop(Shared);

fn physical(name: &str, on: bool) -> Output {
    Output {
        name: name.into(),
        enabled: on,
        connected: true,
        current_mode_id: "3".into(),
        size: Size {
            width: if on { 1920 } else { 0 },
            height: if on { 1080 } else { 0 },
        },
        priority: 1,
        ..Output::default()
    }
}

impl Desktop for FakeDesktop {
    fn query(&self) -> Result<Layout> {
        Ok(self.0.lock().unwrap().layout.clone())
    }
    fn apply(&self, args: &[String]) -> Result<()> {
        let mut sim = self.0.lock().unwrap();
        sim.applies.push(args.to_vec());
        if sim.fail_apply {
            anyhow::bail!("Invalid config.");
        }
        for arg in args {
            let rest = arg.strip_prefix("output.").unwrap();
            for output in &mut sim.layout.outputs {
                let Some(prop) = rest
                    .strip_prefix(output.name.as_str())
                    .and_then(|r| r.strip_prefix('.'))
                else {
                    continue;
                };
                match prop.split_once('.') {
                    None if prop == "enable" => output.enabled = true,
                    None if prop == "disable" => output.enabled = false,
                    Some(("mode", id)) => output.current_mode_id = id.into(),
                    Some(("position", p)) => {
                        let (x, y) = p.split_once(',').unwrap();
                        output.pos = Pos {
                            x: x.parse().unwrap(),
                            y: y.parse().unwrap(),
                        };
                    }
                    Some(("priority", p)) => output.priority = p.parse().unwrap(),
                    _ => {}
                }
            }
        }
        for output in sim.layout.outputs.iter_mut().filter(|o| o.enabled) {
            if let Some(mode) = output.modes.iter().find(|m| m.id == output.current_mode_id) {
                output.size = mode.size;
            }
        }
        Ok(())
    }
}

struct Rig {
    manager: Arc<Manager>,
    sim: Shared,
    state: PathBuf,
    base: PathBuf,
    _fake: Fake,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

use std::path::PathBuf;

fn rig(pool: usize) -> Rig {
    let sim: Shared = Arc::default();
    {
        let mut s = sim.lock().unwrap();
        let mut dp = physical("DP-1", true);
        dp.current_mode_id = "3".into();
        s.layout.outputs = vec![dp, physical("HDMI-A-1", false)];
        for n in 1..=pool {
            s.layout.outputs.push(Output {
                name: format!("Virtual-{n}"),
                ..Output::default()
            });
        }
    }
    let base = temp_dir("rig");
    for n in 1..=pool {
        connector(
            &base,
            &format!("card1-Virtual-{n}"),
            "devices/faux/vibeshine",
            "disconnected",
        );
    }
    let for_broker = sim.clone();
    let (fake, broker) = serve(move |line| {
        let mut sim = for_broker.lock().unwrap();
        sim.broker_log.push(line.to_string());
        let parts: Vec<&str> = line.split(' ').collect();
        let reply = match parts[0] {
            "connect" | "disconnect" => {
                let on = parts[0] == "connect";
                if let Some(o) = sim.layout.outputs.iter_mut().find(|o| o.name == parts[1]) {
                    o.connected = on;
                    if !on {
                        o.enabled = false;
                        o.modes.clear();
                    }
                }
                format!("OK {}ed {}", parts[0], parts[1])
            }
            "status" => {
                let on = sim
                    .layout
                    .outputs
                    .iter()
                    .any(|o| o.name == parts[1] && o.connected);
                format!(
                    "STATUS {} {}",
                    if on { "connected" } else { "disconnected" },
                    parts[1]
                )
            }
            "mode" if sim.refuse_mode => "ERROR refused".into(),
            "mode" => {
                let (w, h, mhz): (i32, i32, f64) = (
                    parts[2].parse().unwrap(),
                    parts[3].parse().unwrap(),
                    parts[4].parse().unwrap(),
                );
                let o = sim
                    .layout
                    .outputs
                    .iter_mut()
                    .find(|o| o.name == parts[1])
                    .unwrap();
                o.modes.push(Mode {
                    id: format!("{}", 100 + o.modes.len()),
                    name: String::new(),
                    refresh_rate: mhz / 1000.0,
                    size: Size {
                        width: w,
                        height: h,
                    },
                });
                format!("OK {line}")
            }
            _ => "ERROR what".into(),
        };
        format!("{reply}\n").into_bytes()
    });
    let state = base.join("state/rubylight/private-display.json");
    let manager = Manager::new(
        broker,
        base.join("drm"),
        Box::new(FakeDesktop(sim.clone())),
        Some(state.clone()),
    )
    .with_timing(Duration::from_millis(5), Duration::from_millis(300));
    Rig {
        manager: Arc::new(manager),
        sim,
        state,
        base,
        _fake: fake,
    }
}

fn request(id: &str, arrangement: Arrangement) -> PrivateRequest {
    PrivateRequest {
        stable_id: id.into(),
        width: 1280,
        height: 720,
        refresh_millihz: 60_000,
        arrangement,
        scale_percent: None,
    }
}

fn outputs(rig: &Rig) -> Vec<Output> {
    rig.sim.lock().unwrap().layout.outputs.clone()
}

#[test]
fn a_private_display_takes_the_desktop_and_gives_it_back() {
    let rig = rig(2);
    let before: Vec<Output> = outputs(&rig)
        .into_iter()
        .filter(|o| o.name != "Virtual-1" && o.name != "Virtual-2")
        .collect();
    let display = rig
        .manager
        .create(&request("a", Arrangement::Exclusive))
        .unwrap();
    assert_eq!(display.name, "Virtual-1");
    assert!(display.switched_on);
    assert_eq!(rig.manager.joinable_mode("a"), Some((1280, 720, 60_000)));
    let now = outputs(&rig);
    assert!(now.iter().find(|o| o.name == "Virtual-1").unwrap().enabled);
    assert!(!now.iter().find(|o| o.name == "DP-1").unwrap().enabled);
    assert_eq!(
        read_saved(&rig.state),
        Some((true, vec!["Virtual-1".to_string()]))
    );
    drop(display);
    let after = outputs(&rig);
    for saved in &before {
        let o = after.iter().find(|o| o.name == saved.name).unwrap();
        assert_eq!(
            (o.enabled, o.pos, o.priority),
            (saved.enabled, saved.pos, saved.priority)
        );
        assert_eq!(o.current_mode_id, saved.current_mode_id);
    }
    let v = after.iter().find(|o| o.name == "Virtual-1").unwrap();
    assert!(!v.enabled && !v.connected);
    assert!(!rig.state.exists());
    assert!(rig.manager.joinable_mode("a").is_none());
    let log = rig.sim.lock().unwrap().broker_log.clone();
    assert_eq!(log.first().map(String::as_str), Some("connect Virtual-1"));
    assert_eq!(log.last().map(String::as_str), Some("disconnect Virtual-1"));
}

#[test]
fn a_second_display_joins_and_only_the_last_restores_the_desktop() {
    let rig = rig(2);
    let first = rig
        .manager
        .create(&request("a", Arrangement::Exclusive))
        .unwrap();
    let second = rig
        .manager
        .create(&request("b", Arrangement::Exclusive))
        .unwrap();
    assert_eq!(second.name, "Virtual-2");
    let on: Vec<String> = outputs(&rig)
        .into_iter()
        .filter(|o| o.enabled)
        .map(|o| o.name)
        .collect();
    assert_eq!(on, ["Virtual-1", "Virtual-2"]);
    drop(first);
    // The desktop is still the private one while "b" lives.
    assert!(
        !outputs(&rig)
            .iter()
            .find(|o| o.name == "DP-1")
            .unwrap()
            .enabled
    );
    assert!(
        outputs(&rig)
            .iter()
            .find(|o| o.name == "Virtual-2")
            .unwrap()
            .enabled
    );
    assert_eq!(
        read_saved(&rig.state),
        Some((true, vec!["Virtual-2".to_string()]))
    );
    drop(second);
    assert!(
        outputs(&rig)
            .iter()
            .find(|o| o.name == "DP-1")
            .unwrap()
            .enabled
    );
    assert!(!rig.state.exists());
}

#[test]
fn the_same_stable_id_keeps_its_connector() {
    let rig = rig(2);
    let one = rig
        .manager
        .create(&request("a", Arrangement::Exclusive))
        .unwrap();
    let two = rig
        .manager
        .create(&request("a", Arrangement::Exclusive))
        .unwrap();
    assert_eq!(one.name, two.name);
    drop(one);
    assert!(rig.manager.joinable_mode("a").is_some());
    drop(two);
    assert!(rig.manager.joinable_mode("a").is_none());
    assert!(
        outputs(&rig)
            .iter()
            .find(|o| o.name == "DP-1")
            .unwrap()
            .enabled
    );
}

#[test]
fn a_failed_apply_restores_and_disconnects_before_the_error() {
    let rig = rig(1);
    rig.sim.lock().unwrap().fail_apply = true;
    let error = rig
        .manager
        .create(&request("a", Arrangement::Exclusive))
        .err()
        .unwrap();
    assert!(format!("{error:#}").contains("Invalid config."));
    rig.sim.lock().unwrap().fail_apply = false;
    assert!(
        outputs(&rig)
            .iter()
            .find(|o| o.name == "DP-1")
            .unwrap()
            .enabled
    );
    let virtual_one = outputs(&rig)
        .into_iter()
        .find(|o| o.name == "Virtual-1")
        .unwrap();
    assert!(!virtual_one.connected);
    assert!(!rig.state.exists());
    assert!(rig.manager.joinable_mode("a").is_none());
}

#[test]
fn a_refused_mode_disconnects_the_connector() {
    let rig = rig(1);
    rig.sim.lock().unwrap().refuse_mode = true;
    let error = rig
        .manager
        .create(&request("a", Arrangement::Extended))
        .err()
        .unwrap();
    assert!(format!("{error:#}").contains("ERROR refused"));
    let log = rig.sim.lock().unwrap().broker_log.clone();
    assert_eq!(log.last().map(String::as_str), Some("disconnect Virtual-1"));
    assert!(
        rig.sim
            .lock()
            .unwrap()
            .applies
            .iter()
            .all(|a| a.len() < 2 || a[0].contains("DP-1"))
            || rig.sim.lock().unwrap().applies.is_empty()
    );
}

#[test]
fn an_empty_pool_and_a_full_pool_say_so() {
    let none = rig(0);
    let error = none
        .manager
        .create(&request("a", Arrangement::Exclusive))
        .err()
        .unwrap();
    assert!(format!("{error:#}").contains("module is not loaded"));
    let one = rig(1);
    let _held = one
        .manager
        .create(&request("a", Arrangement::Exclusive))
        .unwrap();
    let error = one
        .manager
        .create(&request("b", Arrangement::Exclusive))
        .err()
        .unwrap();
    assert!(format!("{error:#}").contains("in use"));
    let bad = PrivateRequest {
        width: 10,
        ..request("c", Arrangement::Exclusive)
    };
    assert!(one.manager.create(&bad).is_err());
}

#[test]
fn recovery_restores_the_saved_layout_and_disconnects_what_is_held() {
    let rig = rig(1);
    let display = rig
        .manager
        .create(&request("a", Arrangement::Exclusive))
        .unwrap();
    // Simulate a crash: keep the file and the changed desktop, forget the process.
    let saved = std::fs::read(&rig.state).unwrap();
    std::mem::forget(display);
    let fresh = Manager::new(
        Broker::unchecked(&rig._fake.socket),
        rig.base.join("drm"),
        Box::new(FakeDesktop(rig.sim.clone())),
        Some(rig.state.clone()),
    );
    assert_eq!(std::fs::read(&rig.state).unwrap(), saved);
    fresh.recover().unwrap();
    assert!(
        outputs(&rig)
            .iter()
            .find(|o| o.name == "DP-1")
            .unwrap()
            .enabled
    );
    assert!(
        !outputs(&rig)
            .iter()
            .find(|o| o.name == "Virtual-1")
            .unwrap()
            .connected
    );
    assert!(!rig.state.exists());
    fresh.recover().unwrap();
}

#[test]
fn availability_needs_a_pool_and_a_socket_this_user_may_use() {
    let base = temp_dir("avail");
    let drm = base.join("drm");
    std::fs::create_dir_all(&drm).unwrap();
    let missing = Broker::unchecked(base.join("none.sock"));
    let reason = availability(&drm, &missing).unwrap_err();
    assert_eq!(reason, "Vibepollo's virtual display module is not loaded");
    connector(
        &base,
        "card1-Virtual-1",
        "devices/faux/vibeshine",
        "disconnected",
    );
    assert!(
        availability(&drm, &missing)
            .unwrap_err()
            .contains("missing")
    );
    let (fake, broker) = serve(|_| Vec::new());
    assert!(availability(&drm, &broker).is_ok());
    let production = Broker {
        trust: Trust::Root,
        ..broker.clone()
    };
    assert!(
        availability(&drm, &production)
            .unwrap_err()
            .contains("root-owned")
    );
    // A socket nobody may open.
    std::fs::set_permissions(
        &fake.socket,
        std::os::unix::fs::PermissionsExt::from_mode(0o000),
    )
    .unwrap();
    // As root access(2) passes anyway; otherwise the reason names the group.
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } != 0 {
        assert_eq!(
            availability(&drm, &broker).unwrap_err(),
            "this user is not in the vibeshine-vkms group"
        );
    }
    let status = status_for(Err("because".into()));
    assert_eq!(status["capable"], false);
    assert_eq!(status["ready"], false);
    assert_eq!(status["reason"], "because");
    assert_eq!(status_for(Ok(()))["ready"], true);
    drop(fake);
    std::fs::remove_dir_all(&base).unwrap();
}

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

/// On this PC, with Vibepollo's pool and broker: a 1280x720 display at 60 Hz in the layout
/// `RUBYLIGHT_LIVE_LAYOUT` names (default exclusive), held `RUBYLIGHT_LIVE_HOLD` seconds
/// (default 5), then dropped; the monitors must be back as they were. It changes the
/// desktop: run it only on a PC whose user agreed.
#[test]
#[ignore = "changes this PC's monitors"]
fn live_a_private_display_comes_and_goes() {
    let layout = std::env::var("RUBYLIGHT_LIVE_LAYOUT").unwrap_or_else(|_| "exclusive".into());
    let hold: u64 = std::env::var("RUBYLIGHT_LIVE_HOLD")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    eprintln!("status {}", virtual_display_status());
    assert!(virtual_display_available(), "{}", virtual_display_status());
    let before = kscreen::query().unwrap();
    let started = std::time::Instant::now();
    let display = VirtualDisplay::create_with(&PrivateRequest {
        stable_id: "rubylight-live-test".into(),
        width: 1280,
        height: 720,
        refresh_millihz: 60_000,
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
        Some((1280, 720, 60_000))
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
