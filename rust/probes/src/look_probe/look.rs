//! Configuration, input history, and camera model for `look_probe`.

use anyhow::{Context, Result, bail};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

pub const USAGE: &str = r"usage: look_probe \\.\DISPLAYn OUT.json [--seconds N] [--fps current|HZ] [--gain PX_PER_COUNT] [--lag-ms MS] [--stick-gain PX_PER_SECOND] [--stick-deadzone F] [--stick-exponent 1|1.5|2|2.5|3] [--hud on|off] [--barcode on|off] [--pad 0-3] [--injected-only] [--fov DEGREES]";
/// As `rubylight_protocol::frame_warp`'s: the exponents a model may carry.
pub const STICK_EXPONENTS: [f32; 5] = [1.0, 1.5, 2.0, 2.5, 3.0];
/// As `rubylight_protocol::frame_warp`'s: the largest dead zone a model may carry.
pub const MAX_STICK_DEADZONE: f32 = 0.5;

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub display: String,
    pub out: PathBuf,
    pub seconds: u64,
    /// None: present at the display's refresh.
    pub fps: Option<u32>,
    /// Pixels per mouse count, x and y.
    pub gain: f64,
    pub lag_us: i64,
    /// Pixels per second of full deflection after the curve, x and y.
    pub stick_gain: f64,
    pub stick_deadzone: f32,
    pub stick_exponent: f32,
    pub hud: bool,
    /// motion_probe's barcode at the top left.
    pub barcode: bool,
    /// XInput user index.
    pub pad: u32,
    /// Only SendInput's relative moves turn the camera.
    pub injected_only: bool,
    /// The horizontal field of view of the perspective camera, in degrees; None slides a
    /// flat world.
    pub fov: Option<f64>,
}

/// The focal length, in pixels, of a camera that sees `fov` degrees across `width` pixels.
pub fn focal(width: u32, fov: f64) -> f64 {
    f64::from(width) * 0.5 / (fov.to_radians() * 0.5).tan()
}

impl Config {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut args = args.into_iter();
        let (Some(display), Some(out)) = (args.next(), args.next()) else {
            bail!("{USAGE}");
        };
        if !display.to_ascii_uppercase().starts_with(r"\\.\DISPLAY") {
            bail!(r"{display} is not a display name like \\.\DISPLAY2; {USAGE}");
        }
        if out.is_empty() || out.starts_with("--") {
            bail!("the second argument is the output JSON path; {USAGE}");
        }
        let mut config = Self {
            display,
            out: PathBuf::from(out),
            seconds: 60,
            fps: None,
            gain: 1.0,
            lag_us: 50_000,
            stick_gain: 1200.0,
            stick_deadzone: 0.2,
            stick_exponent: 1.5,
            hud: true,
            barcode: true,
            pad: 0,
            injected_only: false,
            fov: None,
        };
        while let Some(flag) = args.next() {
            // The one switch without a value.
            if flag == "--injected-only" {
                config.injected_only = true;
                continue;
            }
            let Some(value) = args.next() else {
                bail!("{flag} needs a value; {USAGE}");
            };
            match flag.as_str() {
                "--seconds" => config.seconds = whole(&flag, &value, 1..=3600)?,
                "--fps" if value == "current" => config.fps = None,
                "--fps" => config.fps = Some(whole(&flag, &value, 1..=1000)? as u32),
                "--gain" => config.gain = number(&flag, &value, -10_000.0..=10_000.0)?,
                "--lag-ms" => {
                    config.lag_us = (number(&flag, &value, 0.0..=1000.0)? * 1000.0).round() as i64
                }
                "--stick-gain" => {
                    config.stick_gain = number(&flag, &value, -100_000.0..=100_000.0)?
                }
                "--stick-deadzone" => {
                    let range = 0.0..=f64::from(MAX_STICK_DEADZONE);
                    config.stick_deadzone = number(&flag, &value, range)? as f32;
                }
                "--stick-exponent" => {
                    let exponent = number(&flag, &value, 1.0..=3.0)? as f32;
                    if !STICK_EXPONENTS.contains(&exponent) {
                        bail!("--stick-exponent {value}: must be 1, 1.5, 2, 2.5 or 3");
                    }
                    config.stick_exponent = exponent;
                }
                "--hud" => config.hud = on_off(&flag, &value)?,
                "--barcode" => config.barcode = on_off(&flag, &value)?,
                "--pad" => config.pad = whole(&flag, &value, 0..=3)? as u32,
                "--fov" => config.fov = Some(number(&flag, &value, 20.0..=140.0)?),
                _ => bail!("unknown option {flag}; {USAGE}"),
            }
        }
        Ok(config)
    }
}

fn on_off(flag: &str, value: &str) -> Result<bool> {
    match value {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => bail!("{flag} {value}: must be on or off"),
    }
}

fn number(flag: &str, value: &str, range: RangeInclusive<f64>) -> Result<f64> {
    let parsed: f64 = value
        .parse()
        .with_context(|| format!("{flag} {value}: not a number"))?;
    // NaN and infinities fail the range too.
    if !range.contains(&parsed) {
        bail!(
            "{flag} {value}: must be {} to {}",
            range.start(),
            range.end()
        );
    }
    Ok(parsed)
}

fn whole(flag: &str, value: &str, range: RangeInclusive<u64>) -> Result<u64> {
    let parsed: u64 = value
        .parse()
        .with_context(|| format!("{flag} {value}: not a whole number"))?;
    if !range.contains(&parsed) {
        bail!(
            "{flag} {value}: must be {} to {}",
            range.start(),
            range.end()
        );
    }
    Ok(parsed)
}

/// `OUT.json`'s sibling `OUT.<kind>.csv`.
pub fn sibling(out: &Path, kind: &str) -> PathBuf {
    out.with_extension(format!("{kind}.csv"))
}

pub fn qpc_us(ticks: i64, frequency: i64) -> i64 {
    (i128::from(ticks) * 1_000_000 / i128::from(frequency.max(1))) as i64
}

/// The curve the host fits, from the protocol crate through butterpollo-core.
pub use butterpollo_media_policy::frame_warp::stick_curve;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseMove {
    pub at_us: i64,
    pub dx: i32,
    pub dy: i32,
    /// The running sums the camera uses, with this move if it counts.
    pub sum: [i64; 2],
    /// From SendInput (no device), as the host's are.
    pub injected: bool,
}

/// Every relative move with the running sums, oldest first.
#[derive(Debug, Default)]
pub struct MouseHistory {
    pub moves: Vec<MouseMove>,
    /// Only injected moves add to the sums.
    injected_only: bool,
}

impl MouseHistory {
    pub fn new(injected_only: bool) -> Self {
        Self {
            moves: Vec::new(),
            injected_only,
        }
    }

    pub fn push(&mut self, at_us: i64, dx: i32, dy: i32, injected: bool) {
        let (at_us, sum) = self.moves.last().map_or((at_us, [0, 0]), |last| {
            // Times never go back, so lookups can bisect.
            (at_us.max(last.at_us), last.sum)
        });
        let (add_x, add_y) = if injected || !self.injected_only {
            (i64::from(dx), i64::from(dy))
        } else {
            (0, 0)
        };
        self.moves.push(MouseMove {
            at_us,
            dx,
            dy,
            sum: [sum[0] + add_x, sum[1] + add_y],
            injected,
        });
    }

    /// The running sums as of `at_us`, moves at exactly `at_us` included.
    pub fn at(&self, at_us: i64) -> [i64; 2] {
        let after = self.moves.partition_point(|m| m.at_us <= at_us);
        after.checked_sub(1).map_or([0, 0], |i| self.moves[i].sum)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StickSample {
    pub at_us: i64,
    /// XInput's packet number; None while the pad is missing (read as centred).
    pub packet: Option<u32>,
    pub stick: (i16, i16),
    pub curve: [f32; 2],
    /// The integral of the curve from the start to `at_us`.
    pub integral: [f64; 2],
}

/// The right stick as polled, each value held until the next, with its integral.
#[derive(Debug)]
pub struct StickHistory {
    pub samples: Vec<StickSample>,
    deadzone: f32,
    exponent: f32,
}

impl StickHistory {
    /// Starts centred at `start_us`, where the integral starts.
    pub fn new(start_us: i64, deadzone: f32, exponent: f32) -> Self {
        let start = StickSample {
            at_us: start_us,
            packet: None,
            stick: (0, 0),
            curve: [0.0; 2],
            integral: [0.0; 2],
        };
        Self {
            samples: vec![start],
            deadzone,
            exponent,
        }
    }

    pub fn push(&mut self, at_us: i64, packet: Option<u32>, stick: (i16, i16)) {
        let last = self.samples[self.samples.len() - 1];
        if last.packet.is_some() == packet.is_some() && last.stick == stick {
            return;
        }
        let at_us = at_us.max(last.at_us);
        let curve = match packet {
            Some(_) => stick_curve(stick.0, stick.1, self.deadzone, self.exponent),
            None => [0.0; 2],
        };
        self.samples.push(StickSample {
            at_us,
            packet,
            stick,
            curve,
            integral: held(&last, at_us),
        });
    }

    /// The integral of the curve from the start to `at_us`, in seconds of full deflection;
    /// zero before the start.
    pub fn integral(&self, at_us: i64) -> [f64; 2] {
        let after = self.samples.partition_point(|s| s.at_us <= at_us);
        after
            .checked_sub(1)
            .map_or([0.0; 2], |i| held(&self.samples[i], at_us))
    }
}

/// `sample`'s integral plus its curve held from it to `at_us`.
fn held(sample: &StickSample, at_us: i64) -> [f64; 2] {
    let seconds = (at_us - sample.at_us).max(0) as f64 / 1e6;
    [
        sample.integral[0] + f64::from(sample.curve[0]) * seconds,
        sample.integral[1] + f64::from(sample.curve[1]) * seconds,
    ]
}

/// The camera in pixels: x unwrapped, y clamped.
#[derive(Debug)]
pub struct Camera {
    gain: f64,
    stick_gain: f64,
    y_limit: f64,
    raw_y: f64,
    pub x: f64,
    pub y: f64,
}

impl Camera {
    pub fn new(gain: f64, stick_gain: f64, y_limit: f64) -> Self {
        Self {
            gain,
            stick_gain,
            y_limit,
            raw_y: 0.0,
            x: 0.0,
            y: 0.0,
        }
    }

    /// Turns to the input as of the frame's input time: the mouse sums and the stick
    /// integral.
    pub fn update(&mut self, mouse: [i64; 2], stick: [f64; 2]) {
        self.x = self.gain * mouse[0] as f64 + self.stick_gain * stick[0];
        let raw_y = self.gain * mouse[1] as f64 + self.stick_gain * stick[1];
        // Pitch stops at the limit and drops the turn past it, so turning back moves at once.
        self.y = (self.y + raw_y - self.raw_y).clamp(-self.y_limit, self.y_limit);
        self.raw_y = raw_y;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Config> {
        Config::parse(text.split_whitespace().map(str::to_owned))
    }

    #[test]
    fn arguments_have_defaults_and_bounds() {
        let config = parse(r"\\.\DISPLAY3 out/run.json").unwrap();
        assert_eq!(config.display, r"\\.\DISPLAY3");
        assert_eq!(config.out, PathBuf::from("out/run.json"));
        assert_eq!(
            (config.seconds, config.fps, config.lag_us),
            (60, None, 50_000)
        );
        assert_eq!((config.gain, config.stick_gain), (1.0, 1200.0));
        assert_eq!((config.stick_deadzone, config.stick_exponent), (0.2, 1.5));
        assert!(config.hud && config.barcode);
        assert_eq!(config.pad, 0);
        assert!(!config.injected_only);
        assert_eq!(config.fov, None);
        let lens = parse(r"\\.\DISPLAY1 o --fov 90").unwrap();
        assert_eq!(lens.fov, Some(90.0));
        assert!((focal(2560, 90.0) - 1280.0).abs() < 1e-9);
        let config = parse(
            r"\\.\display3 o.json --seconds 30 --fps 90 --gain -2.5 --lag-ms 33.3 --stick-gain 800 --stick-deadzone 0 --stick-exponent 2 --hud off --injected-only --barcode off --pad 1",
        )
        .unwrap();
        assert_eq!(
            (config.seconds, config.fps, config.lag_us),
            (30, Some(90), 33_300)
        );
        assert_eq!((config.gain, config.stick_gain), (-2.5, 800.0));
        assert_eq!((config.stick_deadzone, config.stick_exponent), (0.0, 2.0));
        assert!(!config.hud && !config.barcode);
        assert_eq!(config.pad, 1);
        assert!(config.injected_only);
        let only = parse(r"\\.\DISPLAY1 o --injected-only").unwrap();
        assert!(only.injected_only);
        assert_eq!(parse(r"\\.\DISPLAY1 o --fps current").unwrap().fps, None);
        for bad in [
            "",
            r"\\.\DISPLAY1",
            "DISPLAY1 o.json",
            r"\\.\DISPLAY1 --seconds",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        for bad in [
            "--seconds",
            "--seconds 0",
            "--seconds 3601",
            "--fps 0",
            "--fps fast",
            "--gain nan",
            "--gain inf",
            "--lag-ms -1",
            "--stick-deadzone 0.6",
            "--stick-exponent 1.2",
            "--hud maybe",
            "--barcode maybe",
            "--barcode",
            "--injected-only on",
            "--pad 4",
            "--fov 10",
            "--fov 141",
            "--fov wide",
            "--speed 2",
        ] {
            assert!(
                parse(&format!(r"\\.\DISPLAY1 o.json {bad}")).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn outputs_sit_beside_the_report() {
        let frames = sibling(Path::new("out/run.json"), "frames");
        assert_eq!(frames, PathBuf::from("out/run.frames.csv"));
        assert_eq!(
            sibling(Path::new("run"), "mouse"),
            PathBuf::from("run.mouse.csv")
        );
        assert_eq!(qpc_us(30_000_000, 10_000_000), 3_000_000);
        assert_eq!(qpc_us(1, 3), 333_333);
    }

    // The protocol crate's own expectations for the original.
    #[test]
    fn stick_curve_matches_the_protocol() {
        assert_eq!(stick_curve(0, 0, 0.1, 1.0), [0.0, 0.0]);
        assert_eq!(stick_curve(3000, 0, 0.1, 1.0), [0.0, 0.0]);
        let full = stick_curve(32767, 0, 0.1, 2.0);
        assert!((full[0] - 1.0).abs() < 1e-5 && full[1] == 0.0);
        let left = stick_curve(-32768, 0, 0.0, 1.0);
        assert!((left[0] + 1.0).abs() < 1e-5);
        let half = stick_curve(16384, 0, 0.0, 1.5);
        assert!((half[0] - 0.353_6).abs() < 1e-3, "{half:?}");
        let diagonal = stick_curve(20000, 20000, 0.0, 1.0);
        assert!((diagonal[0] - diagonal[1]).abs() < 1e-6);
        assert!(diagonal[0] > 0.6 && diagonal[0] <= 0.7072);
        let corner = stick_curve(32767, 32767, 0.0, 1.0);
        let length = (corner[0] * corner[0] + corner[1] * corner[1]).sqrt();
        assert!((length - 1.0).abs() < 1e-5);
    }

    #[test]
    fn mouse_sums_are_looked_up_by_time() {
        let mut mouse = MouseHistory::default();
        assert_eq!(mouse.at(0), [0, 0]);
        mouse.push(100, 10, -2, true);
        mouse.push(200, 5, 1, false);
        // A late timestamp is held at the last one.
        mouse.push(150, 1, 1, true);
        assert_eq!(mouse.moves[2].at_us, 200);
        assert_eq!(mouse.at(99), [0, 0]);
        assert_eq!(mouse.at(100), [10, -2]);
        assert_eq!(mouse.at(199), [10, -2]);
        assert_eq!(mouse.at(200), [16, 0]);
        assert_eq!(mouse.at(i64::MAX), [16, 0]);
    }

    #[test]
    fn injected_only_sums_leave_device_moves_out_but_keep_them() {
        let mut mouse = MouseHistory::new(true);
        mouse.push(100, 10, -2, true);
        mouse.push(200, 5, 1, false);
        mouse.push(300, 1, 1, true);
        assert_eq!(mouse.moves.len(), 3);
        assert_eq!(mouse.moves[1].sum, [10, -2]);
        assert!(!mouse.moves[1].injected);
        assert_eq!(mouse.at(250), [10, -2]);
        assert_eq!(mouse.at(300), [11, -1]);
    }

    #[test]
    fn the_stick_integral_holds_each_poll_until_the_next() {
        let mut stick = StickHistory::new(1_000_000, 0.0, 1.0);
        assert_eq!(stick.integral(0), [0.0, 0.0]);
        // Full right from 1.5 s; the same stick again adds no sample.
        stick.push(1_500_000, Some(1), (32767, 0));
        stick.push(1_600_000, Some(2), (32767, 0));
        assert_eq!(stick.samples.len(), 2);
        // Full down from 2 s, then the pad is gone and reads as centred.
        stick.push(2_000_000, Some(3), (0, -32768));
        stick.push(2_250_000, None, (0, 0));
        let close =
            |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4;
        assert!(close(stick.integral(1_500_000), [0.0, 0.0]));
        assert!(close(stick.integral(1_750_000), [0.25, 0.0]));
        assert!(close(stick.integral(2_000_000), [0.5, 0.0]));
        assert!(close(stick.integral(2_100_000), [0.5, -0.1]));
        assert!(close(stick.integral(3_000_000), [0.5, -0.25]));
    }

    #[test]
    fn the_camera_follows_both_inputs_and_pitch_stops_at_its_limit() {
        let mut camera = Camera::new(2.0, 100.0, 50.0);
        camera.update([10, 5], [0.5, 0.25]);
        assert_eq!((camera.x, camera.y), (70.0, 35.0));
        camera.update([10, 100], [0.5, 0.25]);
        assert_eq!(camera.y, 50.0);
        // Turning back moves at once: 225 to 205.
        camera.update([10, 90], [0.5, 0.25]);
        assert_eq!(camera.y, 30.0);
        camera.update([-10, -100], [0.0, 0.0]);
        assert_eq!((camera.x, camera.y), (-20.0, -50.0));
    }
}
