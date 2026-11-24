//! Virtual gamepad VHF, pad contention, injector, batching, and crypto
//! benchmark sections for `input_path_probe`.

use crate::harness::{Load, stats};
use butterpollo_win_graphics::capture::Priority;
use std::time::{Duration, Instant};

pub(crate) fn vhf_section(loads: &[String]) {
    use butterpollo_input_policy::input::Input;
    println!("== VHF virtual gamepad (neutral pad, removed at the end) ==");
    let worker = {
        let loads = loads.to_vec();
        std::thread::spawn(move || {
            let _p = Priority::new();
            let t = Instant::now();
            let opened = butterpollo_win_io::input::Gamepads::open(
                butterpollo_input_policy::input_policy::VHF_AUTO,
            );
            println!(
                "Gamepads::open (SetupDi + CreateFile + version ioctl): {:.1} us -> {}",
                t.elapsed().as_secs_f64() * 1e6,
                match &opened {
                    Ok(_) => "ok".to_owned(),
                    Err(e) => format!("{e:#}"),
                }
            );
            let Ok(mut pads) = opened else { return };
            let neutral = |buttons: u32| Input::Controller {
                id: 0,
                active: 1,
                buttons,
                left_trigger: 0,
                right_trigger: 0,
                sticks: [0; 4],
            };
            let t = Instant::now();
            let plugged = pads.apply(&neutral(0));
            println!(
                "first state (plug ioctl 0x801 + submit 0x803): {:.1} us -> {:?}",
                t.elapsed().as_secs_f64() * 1e6,
                plugged.as_ref().err()
            );
            if plugged.is_ok() {
                // Let PnP, HID class and any readers (Steam, GameInput) settle.
                std::thread::sleep(Duration::from_secs(3));
                for load in &loads {
                    let _load = Load::start(load);
                    for round in 0..2 {
                        let mut v = Vec::new();
                        let started = Instant::now();
                        for _ in 0..1000 {
                            if started.elapsed() > Duration::from_secs(8) {
                                break;
                            }
                            let t = Instant::now();
                            let _ = pads.apply(&neutral(0));
                            v.push(t.elapsed().as_secs_f64() * 1e6);
                            // ~1 kHz controller updates.
                            let until = t + Duration::from_millis(1);
                            while Instant::now() < until {
                                std::hint::spin_loop();
                            }
                        }
                        stats(
                            &format!("[{load}] round {round}: apply state (ioctl 0x803)"),
                            v,
                        );
                    }
                    let mut v = Vec::new();
                    for _ in 0..500 {
                        let t = Instant::now();
                        let _ = std::hint::black_box(pads.feedback());
                        v.push(t.elapsed().as_secs_f64() * 1e6);
                    }
                    stats(&format!("[{load}] feedback 1 pad (ioctl 0x804)"), v);
                }
            }
            let t = Instant::now();
            drop(pads);
            println!(
                "drop (unplug ioctl 0x802 + CloseHandle): {:.1} us",
                t.elapsed().as_secs_f64() * 1e6
            );
        })
    };
    worker.join().unwrap();
}

/// The control loop's input pass with a virtual pad in use: one controller
/// state and one zero-distance mouse move per millisecond, plus the 8 ms
/// refresh and feedback poll. Reports how long the mouse move waited from the
/// start of its pass. A neutral pad is plugged and removed at the end.
pub(crate) fn pad_contention_section(loads: &[String], profile: &'static str) {
    use butterpollo_input_policy::input::Input;
    println!(
        "== {profile}: mouse move behind a controller state (Injector, neutral pad, zero moves) =="
    );
    let loads = loads.to_vec();
    std::thread::spawn(move || {
        let _p = Priority::input();
        let mut injector = butterpollo_win_io::input::Injector::new("", profile).unwrap();
        let state = Input::Controller {
            id: 0,
            active: 1,
            buttons: 0,
            left_trigger: 0,
            right_trigger: 0,
            sticks: [0; 4],
        };
        let mouse = Input::Relative { x: 0, y: 0 };
        // Plug the pad and let PnP and its readers settle.
        let t = Instant::now();
        let _ = injector.apply(&state);
        println!(
            "first state through the injector: {:.1} us",
            t.elapsed().as_secs_f64() * 1e6
        );
        std::thread::sleep(Duration::from_secs(3));
        for load in &loads {
            let _load = Load::start(load);
            let (mut pads, mut mice, mut waits, mut passes) =
                (Vec::new(), Vec::new(), Vec::new(), Vec::new());
            let started = Instant::now();
            let mut poll_at = Instant::now();
            while started.elapsed() < Duration::from_secs(6) {
                let t = Instant::now();
                let _ = injector.apply(&state);
                let m = Instant::now();
                let _ = injector.apply(&mouse);
                pads.push(m.duration_since(t).as_secs_f64() * 1e6);
                mice.push(m.elapsed().as_secs_f64() * 1e6);
                waits.push(t.elapsed().as_secs_f64() * 1e6);
                if Instant::now() >= poll_at {
                    poll_at = Instant::now() + Duration::from_millis(8);
                    let _ = injector.refresh();
                }
                // The gamepad thread polls feedback; the loop collects it.
                let _ = std::hint::black_box(injector.gamepad_reports().count());
                passes.push(t.elapsed().as_secs_f64() * 1e6);
                let until = t + Duration::from_millis(1);
                while Instant::now() < until {
                    std::hint::spin_loop();
                }
            }
            stats(&format!("[{load}] controller state apply"), pads);
            stats(&format!("[{load}] mouse move apply (SendInput)"), mice);
            stats(
                &format!("[{load}] state + mouse move: mouse done after"),
                waits,
            );
            stats(&format!("[{load}] whole pass incl. 8 ms poll"), passes);
        }
        let t = Instant::now();
        drop(injector);
        println!(
            "injector drop (pad unplugged): {:.1} us",
            t.elapsed().as_secs_f64() * 1e6
        );
    })
    .join()
    .unwrap();
}

/// One-off costs on the input thread: creating an injector, and following a
/// display that does not exist yet (looked up again every 500 ms).
pub(crate) fn injector_section() {
    println!("== injector creation and missing-display lookups on the calling thread ==");
    let first = butterpollo_win_graphics::capture::displays()
        .ok()
        .and_then(|d| d.first().map(|d| d.display_name.clone()))
        .unwrap_or_default();
    let config = butterpollo_config::config::Config::default();
    for (label, output) in [
        ("existing display", first.as_str()),
        ("missing display", r"\\.\DISPLAY99"),
    ] {
        let mut v = Vec::new();
        for _ in 0..20 {
            let t = Instant::now();
            let injector = butterpollo_win_io::input::Injector::new_options(output, "vhf", &config);
            v.push(t.elapsed().as_secs_f64() * 1e6);
            drop(injector);
        }
        stats(&format!("Injector::new_options ({label})"), v);
    }
    let mut injector =
        butterpollo_win_io::input::Injector::new_options(r"\\.\DISPLAY99", "vhf", &config).unwrap();
    let mut v = Vec::new();
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(3) {
        let t = Instant::now();
        injector.set_output(r"\\.\DISPLAY99");
        let _ = injector.refresh();
        v.push(t.elapsed().as_secs_f64() * 1e6);
        std::thread::sleep(Duration::from_millis(1));
    }
    let mut slow: Vec<f64> = v.iter().copied().filter(|us| *us > 200.).collect();
    slow.sort_by(f64::total_cmp);
    stats("set_output + refresh per pass, display missing", v);
    println!("  passes over 200 us: {slow:.0?}");
    let mut v = Vec::new();
    for _ in 0..200 {
        let t = Instant::now();
        std::thread::spawn(|| {}).join().unwrap();
        v.push(t.elapsed().as_secs_f64() * 1e6);
    }
    stats("thread spawn + join (empty)", v);
    let mut v = Vec::new();
    let mut handles = Vec::new();
    for _ in 0..200 {
        let t = Instant::now();
        handles.push(std::thread::spawn(|| {}));
        v.push(t.elapsed().as_secs_f64() * 1e6);
    }
    for h in handles {
        let _ = h.join();
    }
    stats("thread spawn alone (caller's cost)", v);
}

/// Several events in one pass: zero-distance mouse moves, so nothing moves.
/// Each event applied on its own (one SendInput call each, the control loop
/// before October 7) against the whole pass at once (apply_all).
pub(crate) fn batch_section() {
    use butterpollo_input_policy::input::Input;
    println!("== events applied in one pass (zero mouse moves) ==");
    std::thread::spawn(|| {
        let _p = Priority::input();
        let mut injector = butterpollo_win_io::input::Injector::new("", "vhf").unwrap();
        for count in [1usize, 2, 4, 8] {
            let events = vec![Input::Relative { x: 0, y: 0 }; count];
            let (mut each, mut all) = (Vec::new(), Vec::new());
            for i in 0..1300 {
                let t = Instant::now();
                if i % 2 == 0 {
                    for event in &events {
                        let _ = injector.apply(event);
                    }
                } else {
                    let _ = injector.apply_all(&events);
                }
                if i >= 100 {
                    let us = t.elapsed().as_secs_f64() * 1e6;
                    if i % 2 == 0 {
                        each.push(us)
                    } else {
                        all.push(us)
                    }
                }
                let until = Instant::now() + Duration::from_micros(500);
                while Instant::now() < until {
                    std::hint::spin_loop();
                }
            }
            stats(&format!("{count} events per pass, apply each"), each);
            stats(&format!("{count} events per pass, apply_all"), all);
        }
    })
    .join()
    .unwrap();
}

pub(crate) fn crypto_section() {
    println!("== control packet decrypt + decode (per input datagram) ==");
    let key = [7u8; 16];
    // A client mouse-move input (magic 7) inside an encrypted 0x0206 message.
    let mut input = 8u32.to_be_bytes().to_vec();
    input.extend_from_slice(&7u32.to_le_bytes());
    input.extend_from_slice(&5i16.to_be_bytes());
    input.extend_from_slice(&(-3i16).to_be_bytes());
    let mut plain = 0x0206u16.to_le_bytes().to_vec();
    plain.extend_from_slice(&(input.len() as u16).to_le_bytes());
    plain.extend_from_slice(&input);
    for v2 in [true, false] {
        let mut v = Vec::new();
        for seq in 0..20000u32 {
            let mut iv = vec![0u8; if v2 { 12 } else { 16 }];
            if v2 {
                iv[..4].copy_from_slice(&seq.to_le_bytes());
                iv[10] = b'C';
                iv[11] = b'C';
            } else {
                iv[0] = seq as u8;
            }
            let (tag, ct) = butterpollo_crypto::crypto::gcm_seal(&key, &iv, &plain).unwrap();
            let mut p = 1u16.to_le_bytes().to_vec();
            p.extend_from_slice(&((4 + 16 + ct.len()) as u16).to_le_bytes());
            p.extend_from_slice(&seq.to_le_bytes());
            p.extend_from_slice(&tag);
            p.extend_from_slice(&ct);
            let t = Instant::now();
            let (_, kind, payload) =
                butterpollo_media_policy::packet::decrypt_control(&key, &p, v2).unwrap();
            let event = butterpollo_input_policy::input::decode(&payload).unwrap();
            std::hint::black_box((kind, event));
            v.push(t.elapsed().as_secs_f64() * 1e6);
        }
        stats(&format!("decrypt_control(v2={v2}) + input::decode"), v);
    }
}
