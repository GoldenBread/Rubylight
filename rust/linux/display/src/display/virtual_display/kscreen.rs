//! KScreen as `kscreen-doctor` shows and changes it: the layout model, bounded runs of the
//! tool, and the argument lists that apply and restore a private display's layout.
//!
//! Ported from Vibepollo (Nonary/Vibepollo, GPL-3.0), src/platform/linux/private_display.cpp
//! (the apply arguments, `restore_arguments` and the `kscreen-doctor` failure handling).
//! Rubylight's rules are simpler: one apply for the virtual output, one restore from the
//! snapshot taken before the first change.
use anyhow::{Context, Result, anyhow, bail};
use butterpollo_media_policy::display_policy::Arrangement;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long `kscreen-doctor` may take before it is killed.
const TOOL_TIMEOUT: Duration = Duration::from_secs(10);
/// The gap, in pixels, an isolated display keeps from the others.
const ISOLATION_GAP: i32 = 1000;
/// `kscreen-doctor` can exit 0 on failure; these lines say it did.
const FAILURES: [&str; 3] = [
    "Unable to parse arguments:",
    "applying config failed!",
    "Invalid config.",
];

/// What `kscreen-doctor -j` reports: the outputs, as far as the host needs them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    pub outputs: Vec<Output>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Output {
    pub id: i64,
    pub name: String,
    pub enabled: bool,
    pub connected: bool,
    pub current_mode_id: String,
    pub modes: Vec<Mode>,
    pub pos: Pos,
    pub priority: i32,
    pub scale: f64,
    pub rotation: i32,
    pub size: Size,
}

impl Default for Output {
    fn default() -> Self {
        Self {
            id: 0,
            name: String::new(),
            enabled: false,
            connected: false,
            current_mode_id: String::new(),
            modes: Vec::new(),
            pos: Pos::default(),
            priority: 0,
            scale: 1.0,
            rotation: 1,
            size: Size::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Mode {
    pub id: String,
    pub name: String,
    pub refresh_rate: f64,
    pub size: Size,
}

impl Default for Mode {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            refresh_rate: 0.0,
            size: Size::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pos {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Size {
    pub width: i32,
    pub height: i32,
}

impl Layout {
    pub fn parse(json: &str) -> Result<Self> {
        serde_json::from_str(json).context("kscreen-doctor -j did not print a layout")
    }

    pub fn output(&self, name: &str) -> Option<&Output> {
        self.outputs.iter().find(|o| o.name == name)
    }
}

impl Output {
    /// The mode of exactly this size whose refresh is within 0.5 Hz of `millihz`.
    pub fn find_mode(&self, width: u32, height: u32, millihz: u32) -> Option<&Mode> {
        self.modes.iter().find(|m| {
            i64::from(m.size.width) == i64::from(width)
                && i64::from(m.size.height) == i64::from(height)
                && (m.refresh_rate * 1000.0 - f64::from(millihz)).abs() <= 500.0
        })
    }

    /// The size on the desktop, after scaling.
    fn logical(&self) -> (i32, i32) {
        let scale = self.scale.max(0.25);
        (
            (f64::from(self.size.width) / scale).ceil() as i32,
            (f64::from(self.size.height) / scale).ceil() as i32,
        )
    }
}

/// Run `command` and return its stdout and stderr together. Killed when it outlives `timeout`.
pub fn run_bounded(mut command: Command, timeout: Duration) -> Result<(bool, String)> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("could not start kscreen-doctor")?;
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut pipe) = pipe {
                let mut bytes = Vec::new();
                let _ = pipe.read_to_end(&mut bytes);
                text = String::from_utf8_lossy(&bytes).into_owned();
            }
            text
        })
    };
    let out = drain(child.stdout.take().map(|p| Box::new(p) as _));
    let err = drain(child.stderr.take().map(|p| Box::new(p) as _));
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    match status {
        Some(status) => Ok((status.success(), format!("{stdout}{stderr}"))),
        None => bail!("kscreen-doctor did not finish in {} s", timeout.as_secs()),
    }
}

/// Whether the output of an apply says it failed although the tool exited cleanly.
pub fn apply_failed(exited_ok: bool, text: &str) -> Option<String> {
    if !exited_ok || FAILURES.iter().any(|f| text.contains(f)) {
        let shown: String = text.trim().chars().take(300).collect();
        Some(shown)
    } else {
        None
    }
}

/// The current layout, from `kscreen-doctor -j`.
pub fn query() -> Result<Layout> {
    let mut command = Command::new("kscreen-doctor");
    command.arg("-j");
    query_with(command, TOOL_TIMEOUT)
}

pub fn query_with(command: Command, timeout: Duration) -> Result<Layout> {
    let (ok, text) = run_bounded(command, timeout)?;
    if !ok {
        bail!("kscreen-doctor -j failed: {}", text.trim());
    }
    Layout::parse(&text)
}

/// Apply `args` with one `kscreen-doctor` run. An empty list changes nothing.
pub fn apply(args: &[String]) -> Result<()> {
    let mut command = Command::new("kscreen-doctor");
    command.args(args);
    apply_with(command, args.is_empty())
}

pub fn apply_with(command: Command, nothing: bool) -> Result<()> {
    if nothing {
        return Ok(());
    }
    let (ok, text) = run_bounded(command, TOOL_TIMEOUT)?;
    match apply_failed(ok, &text) {
        Some(why) => Err(anyhow!("kscreen-doctor could not apply the layout: {why}")),
        None => Ok(()),
    }
}

fn token(output: &str, prop: &str, value: impl std::fmt::Display) -> String {
    format!("output.{output}.{prop}.{value}")
}

fn flag(output: &str, word: &str) -> String {
    format!("output.{output}.{word}")
}

fn rotation_name(rotation: i32) -> Option<&'static str> {
    Some(match rotation {
        1 => "none",
        2 => "left",
        4 => "inverted",
        8 => "right",
        16 => "flipped",
        32 => "flipped90",
        64 => "flipped180",
        128 => "flipped270",
        _ => return None,
    })
}

/// The arguments that make `virtual_name` show `mode_id` and arrange the desktop around it.
/// `current` is the layout now, which includes the virtual output once it is connected.
pub fn apply_args(
    current: &Layout,
    virtual_name: &str,
    mode_id: &str,
    scale: Option<f64>,
    arrangement: Arrangement,
) -> Vec<String> {
    let exclusive = arrangement == Arrangement::Exclusive;
    let primary = matches!(
        arrangement,
        Arrangement::Primary | Arrangement::PrimaryIsolated
    );
    let isolated = matches!(
        arrangement,
        Arrangement::Isolated | Arrangement::PrimaryIsolated
    );
    let mut args = vec![
        flag(virtual_name, "enable"),
        token(virtual_name, "mode", mode_id),
    ];
    if let Some(scale) = scale {
        args.push(token(virtual_name, "scale", scale));
    }
    let (mut right, mut last_priority) = (0, 0);
    for other in current
        .outputs
        .iter()
        .filter(|o| o.name != virtual_name && o.connected && o.enabled)
    {
        if exclusive {
            args.push(flag(&other.name, "disable"));
            continue;
        }
        right = right.max(other.pos.x + other.logical().0);
        last_priority = last_priority.max(other.priority);
        if primary && other.priority > 0 {
            args.push(token(&other.name, "priority", other.priority + 1));
        }
    }
    if exclusive {
        args.push(token(virtual_name, "position", "0,0"));
        args.push(token(virtual_name, "priority", 1));
    } else {
        let x = if isolated {
            right + ISOLATION_GAP
        } else {
            right
        };
        args.push(token(virtual_name, "position", format!("{x},0")));
        let priority = if primary {
            1
        } else {
            (last_priority + 1).max(1)
        };
        args.push(token(virtual_name, "priority", priority));
    }
    args
}

/// Points each saved output's mode at the id `present` gives that mode now. KScreen numbers
/// modes afresh when an output comes or goes (DP-1's 1920x1080@60 was mode 51, then 99 while a
/// virtual output was on), so a saved id can name another mode. Modes alike in name, size and
/// rate differ in timings KScreen does not show (DP-1 lists 1920x1080@60 at 60 Hz twice: its
/// preferred mode first, another later); KScreen keeps their order, so the one at the same rank
/// among them is taken. A mode `present` no longer lists is left out, and the output keeps the
/// mode it has.
pub fn renumber_modes(snapshot: &mut Layout, present: &Layout) {
    let alike = |a: &Mode, b: &Mode| {
        a.name == b.name
            && a.size.width == b.size.width
            && a.size.height == b.size.height
            && (a.refresh_rate - b.refresh_rate).abs() < 0.001
    };
    for saved in &mut snapshot.outputs {
        let Some(at) = saved
            .modes
            .iter()
            .position(|m| m.id == saved.current_mode_id)
        else {
            continue;
        };
        let mode = &saved.modes[at];
        let rank = saved.modes[..at].iter().filter(|m| alike(m, mode)).count();
        let now = present
            .output(&saved.name)
            .and_then(|o| o.modes.iter().filter(|m| alike(m, mode)).nth(rank));
        saved.current_mode_id = now.map(|m| m.id.clone()).unwrap_or_default();
    }
}

/// The arguments that put the desktop back as `snapshot` saw it and switch off the virtual
/// outputs `virtual_names` (they were not in the snapshot, or were off). Outputs come on first
/// and go off last, so the desktop is never empty in between.
pub fn restore_args(snapshot: &Layout, virtual_names: &[String]) -> Vec<String> {
    let is_virtual = |name: &str| virtual_names.iter().any(|v| v == name);
    let mut on = Vec::new();
    let mut off = Vec::new();
    for saved in snapshot
        .outputs
        .iter()
        .filter(|o| !o.name.is_empty() && !is_virtual(&o.name))
    {
        if !saved.connected {
            continue;
        }
        let name = &saved.name;
        if !saved.enabled {
            off.push(flag(name, "disable"));
            continue;
        }
        on.push(flag(name, "enable"));
        if !saved.current_mode_id.is_empty() {
            on.push(token(name, "mode", &saved.current_mode_id));
        }
        if let Some(rotation) = rotation_name(saved.rotation) {
            on.push(token(name, "rotation", rotation));
        }
        on.push(token(name, "scale", saved.scale));
        on.push(token(
            name,
            "position",
            format!("{},{}", saved.pos.x, saved.pos.y),
        ));
        if saved.priority > 0 {
            on.push(token(name, "priority", saved.priority));
        }
    }
    for name in virtual_names {
        off.push(flag(name, "disable"));
    }
    on.extend(off);
    on
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two connected outputs, one on and one off, and the pool's `Virtual-1`.
    pub const FIXTURE: &str = r#"{
      "outputs": [
        {"id":1,"name":"DP-1","enabled":true,"connected":true,"currentModeId":"3",
         "modes":[{"id":"3","name":"1920x1080@60","refreshRate":60.0,"size":{"width":1920,"height":1080}}],
         "pos":{"x":0,"y":0},"priority":1,"scale":1.0,"rotation":1,"size":{"width":1920,"height":1080},"vrrPolicy":2},
        {"id":2,"name":"HDMI-A-1","enabled":false,"connected":true,"currentModeId":"7",
         "modes":[],"pos":{"x":0,"y":0},"priority":1,"scale":1.0,"rotation":1,"size":{"width":0,"height":0}},
        {"id":3,"name":"Virtual-1","enabled":false,"connected":true,"currentModeId":"",
         "modes":[{"id":"100","name":"1280x720@60","refreshRate":60.0,"size":{"width":1280,"height":720}},
                  {"id":"101","name":"1280x720@120","refreshRate":119.99,"size":{"width":1280,"height":720}}],
         "pos":{"x":0,"y":0},"priority":0,"scale":1.0,"rotation":1,"size":{"width":0,"height":0}}
      ],
      "screen": {"currentSize":{"width":1920,"height":1080}}
    }"#;

    fn layout() -> Layout {
        Layout::parse(FIXTURE).unwrap()
    }

    #[test]
    fn the_model_reads_kscreen_json() {
        let layout = layout();
        assert_eq!(layout.outputs.len(), 3);
        assert_eq!(layout.output("DP-1").unwrap().current_mode_id, "3");
        assert!(!layout.output("HDMI-A-1").unwrap().enabled);
        assert!(Layout::parse("not json").is_err());
    }

    #[test]
    fn modes_match_on_size_and_refresh_within_half_a_hertz() {
        let layout = layout();
        let v = layout.output("Virtual-1").unwrap();
        assert_eq!(v.find_mode(1280, 720, 60_000).unwrap().id, "100");
        assert_eq!(v.find_mode(1280, 720, 120_000).unwrap().id, "101");
        assert!(v.find_mode(1280, 720, 90_000).is_none());
        assert!(v.find_mode(1920, 1080, 60_000).is_none());
    }

    fn apply(arrangement: Arrangement, scale: Option<f64>) -> Vec<String> {
        apply_args(&layout(), "Virtual-1", "100", scale, arrangement)
    }

    #[test]
    fn exclusive_puts_the_virtual_output_alone_at_the_origin() {
        assert_eq!(
            apply(Arrangement::Exclusive, None),
            [
                "output.Virtual-1.enable",
                "output.Virtual-1.mode.100",
                "output.DP-1.disable",
                "output.Virtual-1.position.0,0",
                "output.Virtual-1.priority.1",
            ]
        );
    }

    #[test]
    fn extended_goes_right_of_the_desktop_with_the_next_priority() {
        let args = apply(Arrangement::Extended, Some(1.5));
        assert!(args.contains(&"output.Virtual-1.scale.1.5".into()));
        assert!(args.contains(&"output.Virtual-1.position.1920,0".into()));
        assert!(args.contains(&"output.Virtual-1.priority.2".into()));
        assert!(!args.iter().any(|a| a.contains("disable")));
    }

    #[test]
    fn primary_takes_priority_one_and_shifts_the_others() {
        let args = apply(Arrangement::Primary, None);
        assert!(args.contains(&"output.DP-1.priority.2".into()));
        assert!(args.contains(&"output.Virtual-1.priority.1".into()));
        assert!(args.contains(&"output.Virtual-1.position.1920,0".into()));
    }

    #[test]
    fn isolated_keeps_a_gap() {
        let args = apply(Arrangement::Isolated, None);
        assert!(args.contains(&"output.Virtual-1.position.2920,0".into()));
        assert!(args.contains(&"output.Virtual-1.priority.2".into()));
        let args = apply(Arrangement::PrimaryIsolated, None);
        assert!(args.contains(&"output.Virtual-1.position.2920,0".into()));
        assert!(args.contains(&"output.Virtual-1.priority.1".into()));
        assert!(args.contains(&"output.DP-1.priority.2".into()));
    }

    #[test]
    fn scaled_outputs_count_by_their_logical_width() {
        let mut layout = layout();
        layout.outputs[0].scale = 2.0;
        let args = apply_args(&layout, "Virtual-1", "100", None, Arrangement::Extended);
        assert!(args.contains(&"output.Virtual-1.position.960,0".into()));
    }

    #[test]
    fn a_saved_mode_is_named_by_its_id_now() {
        let mut saved = layout();
        let mut present = layout();
        let dp = present
            .outputs
            .iter_mut()
            .find(|o| o.name == "DP-1")
            .unwrap();
        let wanted = dp.modes.iter().find(|m| m.id == "3").unwrap().clone();
        for mode in &mut dp.modes {
            mode.id = format!("9{}", mode.id);
        }
        renumber_modes(&mut saved, &present);
        let dp = saved.output("DP-1").unwrap();
        assert_eq!(dp.current_mode_id, "93");
        let present_mode = present
            .output("DP-1")
            .unwrap()
            .modes
            .iter()
            .find(|m| m.id == "93");
        assert_eq!(present_mode.map(|m| m.name.clone()), Some(wanted.name));
        // Of two alike modes, the one at the same rank: the second stays the second.
        let mut twin = layout();
        let dp = twin.outputs.iter_mut().find(|o| o.name == "DP-1").unwrap();
        let mut copy = dp.modes.iter().find(|m| m.id == "3").unwrap().clone();
        copy.id = "7".into();
        dp.modes.push(copy);
        dp.current_mode_id = "7".into();
        let mut renumbered = twin.clone();
        for mode in &mut renumbered
            .outputs
            .iter_mut()
            .find(|o| o.name == "DP-1")
            .unwrap()
            .modes
        {
            mode.id = format!("9{}", mode.id);
        }
        renumber_modes(&mut twin, &renumbered);
        assert_eq!(twin.output("DP-1").unwrap().current_mode_id, "97");
        // A mode the output no longer lists is not set.
        let mut gone = layout();
        present.outputs.retain(|o| o.name != "DP-1");
        renumber_modes(&mut gone, &present);
        assert_eq!(gone.output("DP-1").unwrap().current_mode_id, "");
    }

    #[test]
    fn restore_brings_saved_outputs_back_and_switches_the_virtual_one_off() {
        assert_eq!(
            restore_args(&layout(), &["Virtual-1".to_string()]),
            [
                "output.DP-1.enable",
                "output.DP-1.mode.3",
                "output.DP-1.rotation.none",
                "output.DP-1.scale.1",
                "output.DP-1.position.0,0",
                "output.DP-1.priority.1",
                "output.HDMI-A-1.disable",
                "output.Virtual-1.disable",
            ]
        );
    }

    #[test]
    fn the_failure_strings_count_even_when_the_tool_exits_zero() {
        assert!(apply_failed(true, "Unable to parse arguments: x").is_some());
        assert!(apply_failed(true, "applying config failed!").is_some());
        assert!(apply_failed(true, "Invalid config.").is_some());
        assert!(apply_failed(false, "").is_some());
        assert!(apply_failed(true, "Output 1 enabled").is_none());
    }

    #[test]
    fn a_tool_that_hangs_is_killed_and_one_that_prints_is_read() {
        let mut slow = Command::new("sh");
        slow.args(["-c", "sleep 30"]);
        let started = Instant::now();
        let error = run_bounded(slow, Duration::from_millis(200)).unwrap_err();
        assert!(format!("{error:#}").contains("did not finish"));
        assert!(started.elapsed() < Duration::from_secs(5));

        let mut json = Command::new("sh");
        json.args(["-c", "printf '%s' \"$0\"", FIXTURE]);
        assert_eq!(
            query_with(json, Duration::from_secs(5))
                .unwrap()
                .outputs
                .len(),
            3
        );

        let mut bad = Command::new("sh");
        bad.args(["-c", "echo 'Invalid config.'"]);
        assert!(apply_with(bad, false).is_err());
        assert!(apply_with(Command::new("false"), true).is_ok());
    }
}
