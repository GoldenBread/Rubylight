//! Real-socket WebRTC media thread tests and ignored throughput benchmarks.

use super::super::*;
use super::*;
use std::sync::atomic::Ordering;
use std::sync::mpsc;

#[test]
fn the_threads_start_and_stop_on_a_real_socket() {
    let spec = spec(VideoCodec::H264, false);
    let socket = bind_socket("127.0.0.1:0".parse().unwrap()).unwrap();
    let local = socket.local_addr().unwrap();
    let (mut rtc, offer) = host_offer(&spec, local);
    let mut peer = client(&spec, CLIENT.parse().unwrap());
    let text = answer(&mut peer, &offer.sdp);
    let (ids, _) = accept_answer(&mut rtc, offer, &text, &spec).unwrap();
    let session = Session {
        rtc,
        ids,
        spec: &spec,
        authenticated: Arc::new(AtomicBool::new(false)),
    };
    let mut handle = MediaHandle::spawn(
        socket,
        local,
        session,
        TransportOptions::default(),
        Hooks::new(|_| {}, |_| {}),
    )
    .unwrap();
    // Not connected: the frame is accepted by the queue and then discarded by the thread.
    assert_eq!(handle.submit_video(frame(true)), Submit::Queued);
    let started = Instant::now();
    handle.close();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn a_media_thread_that_cannot_start_releases_the_port() {
    let (handle, _rx, _idr) = detached(TransportOptions::default());
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    let port = socket.local_addr().unwrap().port();
    let (tx, _keep) = mpsc::channel::<Command>();
    let shared = handle.shared.clone();
    let result = start_threads(
        &handle.shared,
        || std::thread::Builder::new().spawn(move || receive_loop(socket, tx, shared)),
        || Err(std::io::Error::other("no threads left")),
    );
    let err = format!("{:#}", result.expect_err("the media thread failed"));
    assert!(err.contains("starting the media thread"), "{err}");
    // The receive thread was stopped and joined, so its socket is closed.
    UdpSocket::bind(("127.0.0.1", port)).expect("the port is free again");
    assert!(handle.shared.stop.load(Ordering::Acquire));
}

#[test]
fn a_control_sender_kept_after_close_lets_the_hooks_go() {
    // A guest's hooks own its seat in the input worker, which keeps the control sender: the
    // hooks must go with the transport, or the guest's Left never comes and its pads stay in.
    let events = Arc::new(Mutex::new(Vec::new()));
    let (mut handle, _) = real_handle(events.clone());
    let control = handle.control_sender();
    assert!(control.send(b"{}".to_vec(), false));
    handle.close();
    drop(handle);
    assert_eq!(
        Arc::strong_count(&events),
        1,
        "the hooks outlived the transport"
    );
    assert!(!control.send(b"{}".to_vec(), false));
}

#[test]
fn big_and_garbage_datagrams_on_the_real_socket_do_not_end_the_session() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let (mut handle, local) = real_handle(events.clone());
    let stranger = UdpSocket::bind("127.0.0.1:0").unwrap();
    // 3000 bytes overflowed the old 2 KB buffer, and Windows then fails the receive.
    for size in [3000, 9000, 60_000, 1, 40] {
        stranger.send_to(&vec![0x5a; size], local).unwrap();
    }
    wait_for("the datagrams to be read and refused", || {
        let stats = handle.stats();
        stats.datagrams_received >= 5 && stats.datagrams_rejected >= 5
    });
    std::thread::sleep(Duration::from_millis(150));
    let stats = handle.stats();
    assert_eq!(stats.recv_errors, 0, "{stats:?}");
    assert!(handle.is_running(), "{stats:?}");
    assert!(
        events.lock().unwrap().is_empty(),
        "{:?}",
        events.lock().unwrap()
    );
    handle.close();
    assert!(!handle.is_running());
}

#[test]
fn a_frame_dropped_before_connecting_demands_a_keyframe_next() {
    let (mut handle, _) = real_handle(Arc::default());
    assert_eq!(handle.submit_video(frame(false)), Submit::Dropped);
    assert_eq!(handle.submit_video(frame(true)), Submit::Queued);
    // The thread discards it (no peer yet), so the next delta is refused again.
    wait_for("the keyframe to be discarded", || {
        handle.stats().video_dropped >= 2
    });
    assert_eq!(handle.submit_video(frame(false)), Submit::Dropped);
    handle.close();
}

#[test]
fn dropping_the_handle_inside_a_hook_does_not_join_itself() {
    let slot: Arc<Mutex<Option<MediaHandle>>> = Arc::default();
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let spec = spec(VideoCodec::H264, false);
    let socket = bind_socket("127.0.0.1:0".parse().unwrap()).unwrap();
    let local = socket.local_addr().unwrap();
    let (mut rtc, offer) = host_offer(&spec, local);
    let mut peer = client(&spec, CLIENT.parse().unwrap());
    let text = answer(&mut peer, &offer.sdp);
    let (ids, _) = accept_answer(&mut rtc, offer, &text, &spec).unwrap();
    let session = Session {
        rtc,
        ids,
        spec: &spec,
        authenticated: Arc::new(AtomicBool::new(false)),
    };
    let hook_slot = slot.clone();
    let hooks = Hooks::new(
        |_| {},
        move |event| {
            if event == TransportEvent::Closed {
                // The hook runs on the media thread, which must not wait for itself.
                let handle = hook_slot.lock().unwrap().take();
                drop(handle);
                let _ = done_tx.send(());
            }
        },
    );
    let handle =
        MediaHandle::spawn(socket, local, session, TransportOptions::default(), hooks).unwrap();
    let (tx, shared) = (handle.tx.clone(), handle.shared.clone());
    *slot.lock().unwrap() = Some(handle);
    // End the media thread from outside; its closing hook then drops the last handle.
    tx.send(Command::Stop).unwrap();
    shared.notify();
    done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the hook returned (no self-join)");
    assert!(slot.lock().unwrap().is_none());
}

/// `cargo test -p butterpollo --release -- --ignored --nocapture webrtc_bench`
#[test]
#[ignore = "measurement"]
fn webrtc_bench_keyframe_and_srtp_cost() {
    let mut link = Link::connected(spec(VideoCodec::H264, false));
    let keyframe = h264_frame(true, 1_000_000);
    let started = Instant::now();
    link.video(keyframe, 0, true);
    link.host.advance(link.now).unwrap();
    let out = link.host.take_outgoing();
    let elapsed = started.elapsed();
    let bytes: usize = out.iter().map(|o| o.data.len()).sum();
    println!(
        "webrtc_bench: 1 MB keyframe: {} datagrams, {} bytes, submit to last packet {:.2} ms (no pacing)",
        out.len(),
        bytes,
        elapsed.as_secs_f64() * 1000.
    );
    let rate = 800_000_000.0;
    println!(
        "webrtc_bench: at the 800 Mbps pacing ceiling the same frame leaves in {:.1} ms",
        bytes as f64 * 8. / rate * 1000.
    );

    let frames = 200;
    let size = 250_000;
    let mut total = 0usize;
    let mut spent = Duration::ZERO;
    for i in 0..frames {
        let data = h264_frame(false, size);
        let started = Instant::now();
        link.video(data, 10 * i, false);
        link.host.advance(link.now).unwrap();
        total += link
            .host
            .take_outgoing()
            .iter()
            .map(|o| o.data.len())
            .sum::<usize>();
        spent += started.elapsed();
    }
    let mbit = total as f64 * 8. / 1e6;
    println!(
        "webrtc_bench: packetize and SRTP (the built crypto provider): {:.1} us per KB, {:.2} s of one core per 1000 Mbit, so {:.1}% of a core at 100 Mbps",
        spent.as_secs_f64() * 1e6 / (total as f64 / 1000.),
        spent.as_secs_f64() / mbit * 1000.,
        spent.as_secs_f64() / mbit * 100. * 100.
    );
}

/// What guests cost the owner: from the encoder handing a 250 KB frame over to the owner's media
/// thread holding the buffer str0m packetizes, with 0, 1 and 4 guests following. With guests the
/// copy into the shared buffer (and AV1's delimiter strip) moves into the encoder's hand-off.
/// `cargo test -p butterpollo --release -- --ignored --nocapture webrtc_bench_guest_fan_out`
#[test]
#[ignore = "measurement"]
fn webrtc_bench_guest_fan_out() {
    let frames = 2000;
    for codec in [VideoCodec::H264, VideoCodec::Av1] {
        let make = if codec == VideoCodec::Av1 {
            av1_frame
        } else {
            h264_frame
        };
        for guests in [0, 1, 4] {
            let (mut owner, owner_rx, _) = detached(TransportOptions::default());
            owner.codec = codec;
            let followers: Vec<_> = (0..guests)
                .map(|_| detached(TransportOptions::default()))
                .collect();
            for (i, (guest, _, _)) in followers.iter().enumerate() {
                owner.followers().add(i as u64, guest.feed());
            }
            let mut ready = Vec::with_capacity(frames);
            let mut submit = Duration::ZERO;
            for i in 0..frames {
                let data = make(i == 0, 250_000);
                let started = Instant::now();
                owner.submit_video(frame_of(data, i == 0));
                submit += started.elapsed();
                let Ok(Command::Video(frame)) = owner_rx.try_recv() else {
                    panic!("the owner's frame was not queued");
                };
                owner.shared.pending_video.fetch_sub(1, Ordering::AcqRel);
                let data = frame.data.share(codec).unwrap();
                ready.push(started.elapsed());
                drop(data);
                for (guest, rx, _) in &followers {
                    while rx.try_recv().is_ok() {
                        guest.shared.pending_video.fetch_sub(1, Ordering::AcqRel);
                    }
                }
            }
            ready.sort();
            println!(
                "webrtc_bench: {codec:?}, {guests} guests: owner's frame ready to packetize after {:.1} us median, {:.1} us p99; the encoder's hand-off takes {:.1} us",
                ready[frames / 2].as_secs_f64() * 1e6,
                ready[frames * 99 / 100].as_secs_f64() * 1e6,
                submit.as_secs_f64() * 1e6 / frames as f64
            );
        }
    }
}
