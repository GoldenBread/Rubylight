//! XInput and HID gamepad state, vibration rate, burst, and DualSense sections
//! for `pad_feedback_probe`.

use crate::harness::{
    LEVELS, Load, XInput, collect, controller, hid_paths, idle_host, new_hid, plug, rumble,
    spin_until, stats, thread_cpu_ms, unplug,
};
use anyhow::Result;
use butterpollo_win_graphics::capture::Priority;
use butterpollo_win_io::input::{Injector, PadReport};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use windows::Win32::{Foundation::*, Storage::FileSystem::*, System::IO::CancelIoEx};

pub(crate) fn state_section(pad: &str, args: &[String], loads: &[String]) -> Result<()> {
    println!("== {pad}: controller state to XInputGetState, 500 Hz ==");
    let xinput = XInput::load()?;
    let (mut injector, index) = plug(pad, args, &xinput)?;
    for load in loads {
        let _load = Load::start(load);
        let cpu = thread_cpu_ms("gamepads");
        let (mut latencies, mut missed) = (Vec::new(), 0);
        let mut refresh_at = Instant::now();
        let started = Instant::now();
        for i in 0..1500u32 {
            let buttons = if i % 2 == 0 { 0x1000 } else { 0 }; // A
            let t = Instant::now();
            injector.apply(&controller(buttons))?;
            let deadline = t + Duration::from_millis(500);
            loop {
                if xinput
                    .state(index)
                    .is_some_and(|(_, b)| u32::from(b) & 0x1000 == buttons)
                {
                    latencies.push(t.elapsed().as_secs_f64() * 1e6);
                    break;
                }
                if Instant::now() > deadline {
                    missed += 1;
                    break;
                }
                std::hint::spin_loop();
            }
            if Instant::now() >= refresh_at {
                refresh_at = Instant::now() + Duration::from_millis(8);
                let _ = injector.refresh();
            }
            let _ = injector.gamepad_reports().count();
            spin_until(t + Duration::from_millis(2));
        }
        stats(
            &format!("[{load}] state applied to seen by XInput"),
            latencies,
        );
        println!(
            "[{load}] missed (500 ms) {missed}; gamepads thread CPU {:.1} ms over {:.1} s",
            thread_cpu_ms("gamepads") - cpu,
            started.elapsed().as_secs_f64()
        );
    }
    injector.apply(&controller(0))?;
    unplug(injector, &xinput);
    Ok(())
}

/// The state section for a DualSense (`vhf_ds5`, or `usbip_ds5`: the
/// composite pad on USB/IP), seen in its HID input reports instead of XInput.
pub(crate) fn state_hid_section(profile: &str, args: &[String], loads: &[String]) -> Result<()> {
    use std::sync::atomic::AtomicU32;
    println!("== {profile}: controller state to the DualSense's HID input report, 500 Hz ==");
    idle_host()?;
    let switch = |flag: &str| args.iter().any(|a| a == flag);
    let config = butterpollo_config::config::Config::parse(&format!(
        "gamepad_driver_host_priority = {}\n",
        switch("--driver-host-priority")
    ))?;
    let before = hid_paths()?;
    let mut injector = Injector::new_options("", profile, &config)?;
    injector.apply(&controller(0))?;
    let hid = new_hid(&before, 0x054c, 0x0ce6)?;
    // Cross (Moonlight's A) in input report 0x01, and how many reports came.
    let cross = Arc::new(AtomicU32::new(0));
    let reader = {
        let (cross, hid) = (cross.clone(), hid.0 as usize);
        std::thread::spawn(move || {
            let _p = Priority::input();
            let mut buffer = [0u8; 64];
            loop {
                let mut read = 0;
                // SAFETY: the handle stays open until the device is gone and
                // this read fails; the buffer outlives the call.
                if unsafe {
                    ReadFile(
                        HANDLE(hid as *mut _),
                        Some(&mut buffer),
                        Some(&mut read),
                        None,
                    )
                }
                .is_err()
                {
                    break;
                }
                if buffer[0] == 0x01 && read >= 10 {
                    let reports = (cross.load(Ordering::Acquire) >> 1).wrapping_add(1);
                    cross.store(
                        reports << 1 | u32::from(buffer[8] & 0x20 != 0),
                        Ordering::Release,
                    );
                }
            }
        })
    };
    std::thread::sleep(Duration::from_secs(2));
    println!(
        "{profile}: driver host priority {}",
        switch("--driver-host-priority")
    );
    for load in loads {
        let _load = Load::start(load);
        let cpu = thread_cpu_ms("gamepads");
        let (mut latencies, mut missed) = (Vec::new(), 0);
        let mut refresh_at = Instant::now();
        let reports = cross.load(Ordering::Acquire) >> 1;
        for i in 0..1500u32 {
            let pressed = i % 2 == 0;
            let t = Instant::now();
            injector.apply(&controller(if pressed { 0x1000 } else { 0 }))?;
            let deadline = t + Duration::from_millis(500);
            loop {
                if (cross.load(Ordering::Acquire) & 1 != 0) == pressed {
                    latencies.push(t.elapsed().as_secs_f64() * 1e6);
                    break;
                }
                if Instant::now() > deadline {
                    missed += 1;
                    break;
                }
                std::hint::spin_loop();
            }
            if Instant::now() >= refresh_at {
                refresh_at = Instant::now() + Duration::from_millis(8);
                let _ = injector.refresh();
            }
            let _ = injector.gamepad_reports().count();
            spin_until(t + Duration::from_millis(2));
        }
        stats(
            &format!("[{load}] state applied to seen in the HID report"),
            latencies,
        );
        println!(
            "[{load}] missed (500 ms) {missed}; input reports {}; gamepads thread CPU {:.1} ms",
            (cross.load(Ordering::Acquire) >> 1).wrapping_sub(reports),
            thread_cpu_ms("gamepads") - cpu
        );
    }
    injector.apply(&controller(0))?;
    drop(injector);
    std::thread::sleep(Duration::from_millis(500));
    // SAFETY: the probe owns the handle; the reader's read has failed or
    // fails once it is closed.
    unsafe {
        let _ = CancelIoEx(hid, None);
        let _ = CloseHandle(hid);
    }
    let _ = reader.join();
    Ok(())
}

pub(crate) fn rate_section(pad: &str, args: &[String], loads: &[String]) -> Result<()> {
    println!("== {pad}: vibration changing at 500 Hz for 6 s, then a stop ==");
    let xinput = Arc::new(XInput::load()?);
    let (mut injector, index) = plug(pad, args, &xinput)?;
    // The driver's resolution: VHF Xbox reports keep XInput's value as a
    // percentage, ViGEm's X360 a byte.
    let tolerance = 0x0300;
    for load in loads {
        let _load = Load::start(load);
        let mut reports = Vec::new();
        let mut refresh_at = Instant::now();
        collect(
            &mut injector,
            Instant::now() + Duration::from_millis(100),
            &mut refresh_at,
            &mut reports,
        );
        reports.clear();
        let cpu = thread_cpu_ms("gamepads");
        let done = Arc::new(AtomicBool::new(false));
        let game = {
            let (xinput, done) = (xinput.clone(), done.clone());
            std::thread::spawn(move || {
                let _p = Priority::new();
                let mut writes = Vec::new();
                let mut failed = std::collections::BTreeMap::new();
                let start = Instant::now();
                let mut i = 0usize;
                while start.elapsed() < Duration::from_secs(6) {
                    let at = start + Duration::from_micros(2000 * i as u64);
                    spin_until(at);
                    let (low, high) = (LEVELS[i % 15], LEVELS[(i + 7) % 15]);
                    let result = xinput.vibrate(index, low, high);
                    if result != 0 {
                        *failed.entry(result).or_insert(0u32) += 1;
                    }
                    writes.push((Instant::now(), low, high));
                    i += 1;
                }
                xinput.vibrate(index, 0, 0);
                writes.push((Instant::now(), 0, 0));
                done.store(true, Ordering::Release);
                (writes, failed)
            })
        };
        while !done.load(Ordering::Acquire) {
            collect(
                &mut injector,
                Instant::now() + Duration::from_millis(5),
                &mut refresh_at,
                &mut reports,
            );
        }
        collect(
            &mut injector,
            Instant::now() + Duration::from_millis(500),
            &mut refresh_at,
            &mut reports,
        );
        let cpu = thread_cpu_ms("gamepads") - cpu;
        let (writes, failed) = game.join().unwrap();
        let stop_at = writes.last().unwrap().0;
        let near = |a: u16, b: u16| a.abs_diff(b) <= tolerance;
        let (mut latencies, mut unmatched, mut stop) = (Vec::new(), 0, None);
        let mut after_stop = Vec::new();
        for (at, report) in &reports {
            let Some((low, high)) = rumble(report) else {
                continue;
            };
            if stop.is_some() {
                after_stop.push((low, high));
                continue;
            }
            if (low, high) == (0, 0) && *at >= stop_at {
                stop = Some(at.duration_since(stop_at));
                continue;
            }
            match writes
                .iter()
                .rev()
                .filter(|(w, ..)| w <= at)
                .find(|(_, l, h)| near(*l, low) && near(*h, high))
            {
                Some((w, ..)) => latencies.push(at.duration_since(*w).as_secs_f64() * 1e6),
                None => unmatched += 1,
            }
        }
        let rumbles = reports.iter().filter(|(_, r)| rumble(r).is_some()).count();
        stats(&format!("[{load}] XInputSetState to pad report"), latencies);
        println!(
            "[{load}] writes {} reports {rumbles} unmatched {unmatched}; stop after {}; reports after the stop {} {:?}; gamepads thread CPU {cpu:.1} ms",
            writes.len(),
            stop.map_or("never".to_owned(), |d| format!(
                "{:.2} ms",
                d.as_secs_f64() * 1e3
            )),
            after_stop.len(),
            after_stop.iter().take(4).collect::<Vec<_>>()
        );
        println!(
            "[{load}] XInputSetState failures {failed:?}; XInput pads now {:?}",
            xinput.connected()
        );
    }
    unplug(injector, &xinput);
    Ok(())
}

pub(crate) fn burst_section(pad: &str, args: &[String]) -> Result<()> {
    println!("== {pad}: two rumble changes within 8 ms, then a stop, 40 times ==");
    let xinput = XInput::load()?;
    let (mut injector, index) = plug(pad, args, &xinput)?;
    let mut refresh_at = Instant::now();
    let (mut stops, mut stuck, mut changes_seen) = (Vec::new(), 0, Vec::new());
    for round in 0..40u32 {
        let mut reports = Vec::new();
        let offset = Duration::from_micros(u64::from(round % 8) * 1000);
        collect(
            &mut injector,
            Instant::now() + Duration::from_millis(30) + offset,
            &mut refresh_at,
            &mut reports,
        );
        reports.clear();
        let t = Instant::now();
        xinput.vibrate(index, 0xc000, 0x4000);
        spin_until(t + Duration::from_millis(3));
        xinput.vibrate(index, 0x4000, 0xc000);
        spin_until(t + Duration::from_millis(6));
        xinput.vibrate(index, 0, 0);
        let stop_at = Instant::now();
        collect(
            &mut injector,
            stop_at + Duration::from_millis(100),
            &mut refresh_at,
            &mut reports,
        );
        let rumbles: Vec<_> = reports
            .iter()
            .filter_map(|(at, r)| rumble(r).map(|v| (*at, v)))
            .collect();
        changes_seen.push(rumbles.iter().filter(|(_, v)| *v != (0, 0)).count() as f64);
        match rumbles.last() {
            Some((at, (0, 0))) => {
                stops.push(at.saturating_duration_since(stop_at).as_secs_f64() * 1e6)
            }
            other => {
                stuck += 1;
                println!("round {round}: motors left at {other:?}");
            }
        }
    }
    stats("stop write to stop report", stops);
    stats("non-zero rumble reports per burst (count)", changes_seen);
    println!("bursts ending with the motors running: {stuck}");
    unplug(injector, &xinput);
    Ok(())
}

pub(crate) fn dualsense_section() -> Result<()> {
    println!("== VHF DualSense: lightbar and trigger report, then rumble-only, in one poll ==");
    idle_host()?;
    let before = hid_paths()?;
    let config = butterpollo_config::config::Config::default();
    let mut injector = Injector::new_options("", "vhf_ds5", &config)?;
    injector.apply(&controller(0))?;
    let hid = new_hid(&before, 0x054c, 0x0ce6)?;
    std::thread::sleep(Duration::from_secs(2));
    let mut refresh_at = Instant::now();
    let mut reports = Vec::new();
    collect(
        &mut injector,
        Instant::now() + Duration::from_millis(100),
        &mut refresh_at,
        &mut reports,
    );
    let (mut both, mut counts) = (0, Vec::new());
    for round in 0..20u8 {
        reports.clear();
        let write = |bytes: &[u8]| {
            let mut written = 0;
            // SAFETY: the handle is open for synchronous writes; the buffer
            // outlives the call.
            unsafe { WriteFile(hid, Some(bytes), Some(&mut written), None) }
        };
        // Report 0x02 (USB): flags 0x0c right and left trigger effects, flag
        // 0x04 in the second byte the lightbar; rumble-only: flags 0x03.
        let mut lights = [0u8; 48];
        lights[0] = 0x02;
        lights[1] = 0x0c;
        lights[2] = 0x04;
        lights[11] = 0x01; // right trigger: continuous resistance
        lights[12] = round;
        lights[13] = 0xff;
        lights[22] = 0x01;
        lights[23] = round;
        lights[24] = 0xff;
        lights[45] = 0x60 + round;
        lights[46] = 0x40;
        lights[47] = 0x80;
        let mut motors = [0u8; 48];
        motors[0] = 0x02;
        motors[1] = 0x03;
        motors[3] = 0x20 + round;
        motors[4] = 0x40 + round;
        write(&lights)?;
        write(&motors)?;
        collect(
            &mut injector,
            Instant::now() + Duration::from_millis(60),
            &mut refresh_at,
            &mut reports,
        );
        let feedback: Vec<_> = reports
            .iter()
            .filter_map(|(_, r)| match r {
                PadReport::Feedback { kind, data, .. } => Some((*kind, data.clone())),
                _ => None,
            })
            .collect();
        if round == 0 {
            for (kind, data) in &feedback {
                println!("round 0 report kind {kind:#06x} {data:02x?}");
            }
        }
        counts.push(feedback.len() as f64);
        let lights_seen = feedback.iter().any(|(_, d)| d.contains(&(0x60 + round)));
        let motors_seen = feedback
            .iter()
            .any(|(_, d)| d.contains(&(0x20 + round)) && d.contains(&(0x40 + round)));
        if lights_seen && motors_seen {
            both += 1;
        }
    }
    stats("feedback reports per pair (count)", counts);
    println!("pairs with the lights and the rumble both forwarded: {both}/20");
    // SAFETY: the probe owns the handle.
    unsafe {
        let _ = CloseHandle(hid);
    }
    injector.apply(&controller(0))?;
    drop(injector);
    std::thread::sleep(Duration::from_millis(500));
    Ok(())
}
