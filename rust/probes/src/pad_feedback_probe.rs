//! Virtual gamepads and touch through the host's injector, as a stream drives
//! them, timed from outside: a stand-in game writes XInput vibration or HID
//! output, and the control loop's side collects the injector's pad reports.
//! Sections (explicit):
//!   state      controller states at 500 Hz until XInputGetState shows each
//!   rate       vibration changing at 500 Hz for 6 s, then a stop: forward
//!              latency, reports, stop latency and any rumble after the stop
//!   burst      two rumble changes within 8 ms ending in a stop, 40 times
//!   dualsense  a lightbar and trigger report, then a rumble-only report, in
//!              one poll on a VHF DualSense, 20 times
//!   pinch      two fingers moving in turn, and one finger alone, injected
//!              into a small window of this probe's: frames and time per pass
//!   release    a click and a long press in absolute mode into that window,
//!              with absolute_left_release_delay 0 and 10: the buttons it saw
//! --pad=vhf|vigem (state, rate, burst; state also vhf_ds5 and usbip_ds5,
//! read from the DualSense's HID input reports), --fast-poll and
//! --driver-host-priority set gamepad_feedback_fast_poll and
//! gamepad_driver_host_priority, --load=none,normal,highest runs state, rate
//! and pinch beside the CPU-bound child of input_path_probe, --trace prints
//! the input module's logs (its `gamepad timings` lines). The probe takes the
//! stream's process setup (HIGH class, 1 ms timer, input thread priority).
//! Pads are plugged on a free XInput slot and removed at the end: run it while
//! no one streams and no other XInput pad is connected. Its reports are read
//! by polling every 100 us, so latencies leave out the control loop's wake.

#[path = "pad_feedback_probe/gamepad_sections.rs"]
mod gamepad_sections;
#[path = "pad_feedback_probe/harness.rs"]
mod harness;
#[path = "pad_feedback_probe/window_sections.rs"]
mod window_sections;

use anyhow::Result;
use butterpollo_win_graphics::capture::Priority;
use gamepad_sections::{
    burst_section, dualsense_section, rate_section, state_hid_section, state_section,
};
use window_sections::{pinch_section, release_section};
use windows::Win32::System::Threading::{GetCurrentProcess, HIGH_PRIORITY_CLASS, SetPriorityClass};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--trace") {
        tracing_subscriber::fmt()
            .with_env_filter("butterpollo_win_io::input=info")
            .init();
    }
    let loads: Vec<String> = args
        .iter()
        .find_map(|a| a.strip_prefix("--load="))
        .unwrap_or("none")
        .split(',')
        .map(str::to_owned)
        .collect();
    let pad = args
        .iter()
        .find_map(|a| a.strip_prefix("--pad="))
        .unwrap_or("vhf")
        .to_owned();
    butterpollo_win_graphics::timing::disable_power_throttling();
    // SAFETY: process-wide settings the stream also makes; undone at exit.
    unsafe {
        let _ = windows::Win32::Media::timeBeginPeriod(1);
        let _ = SetPriorityClass(GetCurrentProcess(), HIGH_PRIORITY_CLASS);
    }
    println!("process: 1 ms timer resolution, HIGH priority class, no power throttling");
    let sections: Vec<String> = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .cloned()
        .collect();
    let run = std::thread::spawn(move || -> Result<()> {
        let _p = Priority::input();
        for section in &sections {
            match section.as_str() {
                "state" if pad.ends_with("_ds5") => state_hid_section(&pad, &args, &loads)?,
                "state" => state_section(&pad, &args, &loads)?,
                "rate" => rate_section(&pad, &args, &loads)?,
                "burst" => burst_section(&pad, &args)?,
                "dualsense" => dualsense_section()?,
                "pinch" => pinch_section(&loads)?,
                "release" => release_section()?,
                other => anyhow::bail!("unknown section {other}"),
            }
        }
        Ok(())
    });
    let result = run.join().unwrap();
    // SAFETY: pairs timeBeginPeriod above.
    unsafe {
        let _ = windows::Win32::Media::timeEndPeriod(1);
    }
    result
}
