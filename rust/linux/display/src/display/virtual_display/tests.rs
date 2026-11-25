//! Tests of the private display's life and of availability, against a fake broker, a fake
//! KScreen and a temporary sysfs tree. Nothing here touches a real display, socket or sysfs;
//! the ignored live tests that do are in `live_tests.rs`.
use super::broker::fake::{Fake, connector, serve, temp_dir};
use super::broker::{Broker, Trust};
use super::kscreen::{Layout, Mode, Output, Pos, Size};
use super::pool::Kind;
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
    /// What KWin did here on meeting a new set of monitors: every connected one on, moved.
    reshuffle_on_connect: bool,
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
        let mut refused = false;
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
                    Some(("addCustomMode", m)) => {
                        let v: Vec<f64> =
                            m.split('.').take(3).map(|n| n.parse().unwrap()).collect();
                        let id = format!("{}", 200 + output.modes.len());
                        output.modes.push(Mode {
                            id,
                            name: String::new(),
                            refresh_rate: v[2] / 1000.0,
                            size: Size {
                                width: v[0] as i32,
                                height: v[1] as i32,
                            },
                        });
                    }
                    Some(("removeCustomMode", _)) => {
                        match output.modes.iter().position(|m| m.id.starts_with('2')) {
                            Some(at) => drop(output.modes.remove(at)),
                            None => refused = true,
                        }
                    }
                    _ => {}
                }
            }
        }
        for output in sim.layout.outputs.iter_mut().filter(|o| o.enabled) {
            if let Some(mode) = output.modes.iter().find(|m| m.id == output.current_mode_id) {
                output.size = mode.size;
            }
        }
        if refused {
            anyhow::bail!("applying config failed!");
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
    rig_on(pool, Kind::Vibepollo)
}

fn rig_on(pool: usize, kind: Kind) -> Rig {
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
            &format!("devices/faux/{}", kind.device()),
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
                if on && sim.reshuffle_on_connect {
                    for o in sim
                        .layout
                        .outputs
                        .iter_mut()
                        .filter(|o| o.name.contains('-') && !o.name.starts_with("Virtual"))
                    {
                        o.enabled = true;
                        o.pos.x += 1920;
                    }
                }
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
    let manager = Manager::with_brokers(
        vec![(kind, broker)],
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

/// Rubylight's pool lists no custom modes: the host adds the mode through KWin, never asks the
/// broker for it, and takes the mode away with the display.
#[test]
fn rubylight_s_pool_gets_its_mode_from_kwin() {
    let rig = rig_on(1, Kind::Rubylight);
    let display = rig
        .manager
        .create(&request("phone", Arrangement::Extended))
        .unwrap();
    {
        let sim = rig.sim.lock().unwrap();
        assert!(sim.broker_log.iter().all(|l| !l.starts_with("mode")));
        assert!(
            sim.applies
                .iter()
                .flatten()
                .any(|a| a == "output.Virtual-1.addCustomMode.1280.720.60000.full")
        );
        let v = sim.layout.output("Virtual-1").unwrap();
        assert!(v.enabled && v.current_mode_id.starts_with('2'));
    }
    drop(display);
    let sim = rig.sim.lock().unwrap();
    assert!(sim.broker_log.last().unwrap().starts_with("disconnect"));
    assert!(
        sim.applies
            .iter()
            .flatten()
            .any(|a| a == "output.Virtual-1.removeCustomMode.0")
    );
}

/// KWin turned HDMI-A-1 on and moved DP-1 when the connector was plugged in: the saved layout
/// is the one from before the plug, the first display's apply puts them back, and so does the
/// end.
#[test]
fn a_layout_kwin_changes_on_plug_is_put_back() {
    let rig = rig(1);
    let before = rig.sim.lock().unwrap().layout.clone();
    rig.sim.lock().unwrap().reshuffle_on_connect = true;
    let display = rig
        .manager
        .create(&request("phone", Arrangement::Extended))
        .unwrap();
    {
        let sim = rig.sim.lock().unwrap();
        let hdmi = sim.layout.output("HDMI-A-1").unwrap();
        assert!(!hdmi.enabled, "HDMI-A-1 stays off as it was");
        assert_eq!(sim.layout.output("DP-1").unwrap().pos.x, 0);
        assert_eq!(sim.layout.output("Virtual-1").unwrap().pos.x, 1920);
    }
    drop(display);
    let sim = rig.sim.lock().unwrap();
    for name in ["DP-1", "HDMI-A-1"] {
        let (now, then) = (
            sim.layout.output(name).unwrap(),
            before.output(name).unwrap(),
        );
        assert_eq!(now.enabled, then.enabled, "{name}");
        // An output that is off has no place on the desktop to put back.
        assert!(!then.enabled || now.pos.x == then.pos.x, "{name}");
    }
}
