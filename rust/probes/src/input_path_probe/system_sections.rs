//! UDP wake, SendInput, display lookup, and synthetic pointer benchmark
//! sections for `input_path_probe`.

use crate::harness::{Load, MmcssOnly, Prio, Rng, apply_prio, stats};
use butterpollo_win_graphics::capture::Priority;
use std::{
    net::UdpSocket,
    os::windows::io::AsRawSocket,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::HANDLE,
    Networking::WinSock::*,
    System::Threading::WaitForSingleObject,
    UI::{Controls::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Wait {
    Poll1,
    PollLong,
    Event,
    Blocking,
    Spin,
}

/// One-way delivery latency: sender stamps the datagram right before
/// send_to; the receiver measures when it has the bytes in hand.
pub(crate) fn wake_case(wait: Wait, prio: Prio, samples: usize) -> Vec<f64> {
    let base = Instant::now();
    let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
    let address = receiver.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let ready = Arc::new(AtomicBool::new(false));
    let (stop_r, ready_r) = (stop.clone(), ready.clone());
    let recv_thread = std::thread::spawn(move || {
        let _p = apply_prio(prio);
        let raw = receiver.as_raw_socket();
        let mut lat = Vec::with_capacity(samples);
        let mut buf = [0u8; 64];
        let event = unsafe { WSACreateEvent().unwrap() };
        match wait {
            Wait::Blocking => {
                receiver.set_nonblocking(false).unwrap();
                receiver
                    .set_read_timeout(Some(Duration::from_millis(50)))
                    .unwrap();
            }
            Wait::Event => unsafe {
                WSAEventSelect(SOCKET(raw as usize), Some(event), FD_READ as i32);
            },
            _ => receiver.set_nonblocking(true).unwrap(),
        }
        ready_r.store(true, Ordering::Release);
        while !stop_r.load(Ordering::Acquire) {
            // Drain everything there, as ENet's receive loop does.
            loop {
                match receiver.recv_from(&mut buf) {
                    Ok((n, _)) if n >= 8 => {
                        let now = base.elapsed().as_nanos() as u64;
                        let sent = u64::from_le_bytes(buf[..8].try_into().unwrap());
                        lat.push((now.saturating_sub(sent)) as f64 / 1000.);
                        if wait == Wait::Blocking {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            match wait {
                Wait::Poll1 => {
                    let _ = butterpollo_win_io::net::wait_readable(raw, 1);
                }
                Wait::PollLong => {
                    let _ = butterpollo_win_io::net::wait_readable(raw, 50);
                }
                Wait::Event => unsafe {
                    WaitForSingleObject(HANDLE(event.0 as *mut _), 50);
                    let mut ev = WSANETWORKEVENTS::default();
                    WSAEnumNetworkEvents(SOCKET(raw as usize), event, &mut ev);
                },
                Wait::Blocking | Wait::Spin => {}
            }
        }
        lat
    });
    while !ready.load(Ordering::Acquire) {
        std::hint::spin_loop();
    }
    // The sender stands in for the NIC: keep it out of the competition.
    let _sender_prio = MmcssOnly::new();
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let timer = butterpollo_win_graphics::timing::Timer::new().unwrap();
    let mut rng = Rng(0x9e3779b97f4a7c15);
    std::thread::sleep(Duration::from_millis(20));
    for _ in 0..samples {
        // Random phase relative to the receiver's 1 ms poll: 0.3-3.3 ms apart.
        let gap = Duration::from_micros(300 + rng.next() % 3000);
        timer.until(Instant::now() + gap);
        let stamp = base.elapsed().as_nanos() as u64;
        sender.send_to(&stamp.to_le_bytes(), address).unwrap();
    }
    std::thread::sleep(Duration::from_millis(60));
    stop.store(true, Ordering::Release);
    let _ = sender.send_to(&[0u8; 8], address);
    let mut lat = recv_thread.join().unwrap();
    lat.truncate(samples);
    lat
}

pub(crate) fn wake_section(samples: usize, loads: &[String]) {
    println!("== UDP loopback send -> bytes in receiver (wait method, receiver priority, load) ==");
    for load in loads {
        let _load = Load::start(load);
        for (wait, prio) in [
            (Wait::Poll1, Prio::PriorityNew),
            (Wait::Poll1, Prio::MmcssOnly),
            (Wait::Poll1, Prio::Plain),
            (Wait::PollLong, Prio::PriorityNew),
            (Wait::Event, Prio::PriorityNew),
            (Wait::Blocking, Prio::PriorityNew),
            (Wait::Spin, Prio::PriorityNew),
        ] {
            if load != "none" && matches!(wait, Wait::PollLong | Wait::Blocking) {
                continue;
            }
            let lat = wake_case(wait, prio, samples);
            stats(&format!("[{load}] {wait:?} {prio:?}"), lat);
        }
    }
    // Raw sendto cost (what ENet's ACK costs before the event is dispatched).
    let sink = UdpSocket::bind("127.0.0.1:0").unwrap();
    sink.set_nonblocking(true).unwrap();
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    sender.set_nonblocking(true).unwrap();
    let address = sink.local_addr().unwrap();
    let mut v = Vec::new();
    let mut b = [0u8; 64];
    for i in 0..4000 {
        let t = Instant::now();
        let _ = sender.send_to(&[0u8; 20], address);
        if i >= 100 {
            v.push(t.elapsed().as_secs_f64() * 1e6);
        }
        if i % 64 == 0 {
            while sink.recv_from(&mut b).is_ok() {}
        }
    }
    stats("sendto 20 B loopback (ACK-sized)", v);
    let mut v = Vec::new();
    for _ in 0..4000 {
        let t = Instant::now();
        let _ = sink.recv_from(&mut b);
        v.push(t.elapsed().as_secs_f64() * 1e6);
    }
    stats("recvfrom on empty nonblocking socket", v);
    let mut v = Vec::new();
    for _ in 0..4000 {
        let t = Instant::now();
        let _ = butterpollo_win_io::net::wait_readable(sink.as_raw_socket(), 0);
        v.push(t.elapsed().as_secs_f64() * 1e6);
    }
    stats("WSAPoll(timeout 0) on empty socket", v);
}

pub(crate) fn sendinput_section(loads: &[String]) {
    println!("== SendInput (zero relative mouse moves: no cursor motion) ==");
    let zero = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dwFlags: MOUSEEVENTF_MOVE,
                ..Default::default()
            },
        },
    };
    for load in loads {
        let _load = Load::start(load);
        let load = load.clone();
        let worker = std::thread::spawn(move || {
            let _p = Priority::new();
            for batch in [1usize, 2, 4, 8] {
                let inputs = vec![zero; batch];
                let mut v = Vec::new();
                let mut failed = 0;
                for i in 0..(1500 / batch).max(200) + 50 {
                    let t = Instant::now();
                    let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
                    let us = t.elapsed().as_secs_f64() * 1e6;
                    if sent as usize != batch {
                        failed += 1;
                    }
                    if i >= 50 {
                        v.push(us);
                    }
                    // Space calls like real input (~1 kHz mouse).
                    let until = Instant::now() + Duration::from_micros(500);
                    while Instant::now() < until {
                        std::hint::spin_loop();
                    }
                }
                stats(
                    &format!("[{load}] SendInput {batch}/call (failed {failed}) per call"),
                    v.clone(),
                );
                if batch > 1 {
                    let per: Vec<f64> = v.iter().map(|x| x / batch as f64).collect();
                    stats(&format!("[{load}]   ...per input at {batch}/call"), per);
                }
            }
        });
        worker.join().unwrap();
    }
    let mut v = Vec::new();
    for _ in 0..4000 {
        let t = Instant::now();
        unsafe {
            std::hint::black_box((
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            ));
        }
        v.push(t.elapsed().as_secs_f64() * 1e6);
    }
    stats("GetSystemMetrics x4 (absolute mouse)", v);
    let mut v = Vec::new();
    for i in 0..4000u32 {
        let t = Instant::now();
        unsafe {
            std::hint::black_box(MapVirtualKeyW(0x41 + (i % 26), MAPVK_VK_TO_VSC));
        }
        v.push(t.elapsed().as_secs_f64() * 1e6);
    }
    stats("MapVirtualKeyW (always_send_scancodes)", v);
}

pub(crate) fn display_section() {
    println!("== display lookups that run on the control thread ==");
    let displays = butterpollo_win_graphics::capture::displays().unwrap_or_default();
    let name = displays
        .first()
        .map(|d| d.display_name.clone())
        .unwrap_or_default();
    println!("first display {name:?}, {} attached", displays.len());
    let mut v = Vec::new();
    for _ in 0..200 {
        let t = Instant::now();
        let _ = std::hint::black_box(butterpollo_win_display::display::mode(&name));
        v.push(t.elapsed().as_secs_f64() * 1e6);
    }
    stats("display::mode (current_rect, every 500 ms)", v);
    let mut v = Vec::new();
    for _ in 0..30 {
        let t = Instant::now();
        let _ = std::hint::black_box(butterpollo_win_graphics::capture::displays());
        v.push(t.elapsed().as_secs_f64() * 1e6);
    }
    stats("capture::displays (DXGI factory+enum)", v);
    let mut v = Vec::new();
    for _ in 0..30 {
        let t = Instant::now();
        let _ = std::hint::black_box(butterpollo_win_display::display::monitors());
        v.push(t.elapsed().as_secs_f64() * 1e6);
    }
    stats("display::monitors (QDC + displays())", v);
    let mut v = Vec::new();
    for _ in 0..30 {
        let t = Instant::now();
        let _ = std::hint::black_box(butterpollo_win_display::display::monitors());
        let _ = std::hint::black_box(butterpollo_win_graphics::capture::displays());
        v.push(t.elapsed().as_secs_f64() * 1e6);
    }
    stats("display_rect equivalent (monitors+displays)", v);
}

pub(crate) fn pointer_section() {
    println!("== synthetic pointer devices (created and destroyed, nothing injected) ==");
    for (label, kind, count) in [("touch x32", PT_TOUCH, 32u32), ("pen x1", PT_PEN, 1)] {
        let mut create = Vec::new();
        let mut destroy = Vec::new();
        for _ in 0..5 {
            let t = Instant::now();
            let device =
                unsafe { CreateSyntheticPointerDevice(kind, count, POINTER_FEEDBACK_NONE) };
            create.push(t.elapsed().as_secs_f64() * 1e6);
            if let Ok(device) = device {
                let t = Instant::now();
                unsafe { DestroySyntheticPointerDevice(device) };
                destroy.push(t.elapsed().as_secs_f64() * 1e6);
            } else {
                println!("{label}: create failed {device:?}");
            }
        }
        stats(&format!("CreateSyntheticPointerDevice {label}"), create);
        stats(&format!("DestroySyntheticPointerDevice {label}"), destroy);
    }
    // What applications see while a synthetic touch device exists: making
    // one ahead of a client's first touch would change it for every stream.
    let touch = || unsafe {
        (
            GetSystemMetrics(SM_MAXIMUMTOUCHES),
            GetSystemMetrics(SM_DIGITIZER),
        )
    };
    let before = touch();
    let device = unsafe { CreateSyntheticPointerDevice(PT_TOUCH, 32, POINTER_FEEDBACK_NONE) };
    std::thread::sleep(Duration::from_millis(200));
    let during = touch();
    if let Ok(device) = device {
        unsafe { DestroySyntheticPointerDevice(device) };
    }
    std::thread::sleep(Duration::from_millis(200));
    println!(
        "SM_MAXIMUMTOUCHES, SM_DIGITIZER: before {before:?}, with a synthetic touch device {during:?}, after {:?}",
        touch()
    );
}
