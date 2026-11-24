//! Costs on the host's input path: wake, decrypt, SendInput, display
//! lookups, synthetic pointer devices, VHF ioctls and thread priority.
//! Sections: priority wake sendinput display pointer crypto injector batch
//! (default); vhf, pad-contention, contention (explicit).
//! --load=none,normal,highest runs wake, sendinput, vhf and pad-contention
//! beside a CPU-bound child process; --no-streaming keeps the default timer
//! resolution and priority class; --trace prints the input module's logs.
//! Mouse input is zero-distance moves only. vhf and pad-contention plug a
//! neutral virtual pad and remove it: run them while no one streams.

#[path = "input_path_probe/harness.rs"]
mod harness;
#[path = "input_path_probe/input_sections.rs"]
mod input_sections;
#[path = "input_path_probe/system_sections.rs"]
mod system_sections;

use harness::{Load, Prio, priority_section, spin_child, stats};
use input_sections::{
    batch_section, crypto_section, injector_section, pad_contention_section, vhf_section,
};
use system_sections::{
    Wait, display_section, pointer_section, sendinput_section, wake_case, wake_section,
};
use windows::Win32::System::Threading::{GetCurrentProcess, HIGH_PRIORITY_CLASS, SetPriorityClass};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(kind) = args.iter().find_map(|a| a.strip_prefix("--spin=")) {
        spin_child(kind);
        return;
    }
    if args.iter().any(|a| a == "--trace") {
        // Shows input failures logged on other threads, such as the gamepad
        // thread's driver calls.
        tracing_subscriber::fmt()
            .with_env_filter("butterpollo_win_io::input=debug")
            .init();
    }
    let sections: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let all = sections.is_empty();
    let has = |s: &str| all || sections.iter().any(|a| *a == s);
    let streaming = !args.iter().any(|a| a == "--no-streaming");
    let loads: Vec<String> = args
        .iter()
        .find_map(|a| a.strip_prefix("--load="))
        .unwrap_or("none")
        .split(',')
        .map(str::to_owned)
        .collect();
    if streaming {
        // What StreamingScope does for the process, without its system-wide
        // side effects (DWM MMCSS, Wi-Fi mode, Mouse Keys).
        butterpollo_win_graphics::timing::disable_power_throttling();
        unsafe {
            let _ = windows::Win32::Media::timeBeginPeriod(1);
            let _ = SetPriorityClass(GetCurrentProcess(), HIGH_PRIORITY_CLASS);
        }
        println!("process: 1 ms timer resolution, HIGH priority class, no power throttling");
    } else {
        println!("process: default timer resolution and priority class");
    }
    let samples = args
        .iter()
        .find_map(|a| a.strip_prefix("--samples=").and_then(|n| n.parse().ok()))
        .unwrap_or(1500);
    if has("priority") {
        priority_section();
    }
    if has("crypto") {
        crypto_section();
    }
    if has("sendinput") {
        sendinput_section(&loads);
    }
    if has("display") {
        display_section();
    }
    if has("pointer") {
        pointer_section();
    }
    if sections.iter().any(|a| *a == "vhf") {
        vhf_section(&loads);
    }
    if sections.iter().any(|a| *a == "pad-contention") {
        pad_contention_section(&loads, "vhf");
    }
    if has("injector") {
        injector_section();
    }
    if has("batch") {
        batch_section();
    }
    if has("wake") {
        wake_section(samples, &loads);
    }
    if sections.iter().any(|a| *a == "contention") {
        // Spinners at TIME_CRITICAL (15) on every CPU: only the receiver's
        // priority differs between the two cases.
        println!("== receiver priority under TIME_CRITICAL spinners on all CPUs ==");
        let _load = Load::start("critical");
        for prio in [Prio::PriorityNew, Prio::PriorityInput, Prio::MmcssOnly] {
            let lat = wake_case(Wait::Poll1, prio, samples);
            stats(&format!("[critical] Poll1 {prio:?}"), lat);
        }
    }
    if streaming {
        unsafe {
            let _ = windows::Win32::Media::timeEndPeriod(1);
        }
    }
}
