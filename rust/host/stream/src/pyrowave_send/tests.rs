//! Unit tests for PyroWave sender pacing stop, host timing rows, link-rate recording, and early blocks.

mod failures_and_pacing;

use super::*;
use std::net::UdpSocket;

#[test]
fn stopping_interrupts_a_paced_wait_without_sending_another_packet() -> Result<()> {
    let fixture = crate::state::test_support::Fixture::new();
    let launch = fixture.launch(
        fixture.client(0),
        butterpollo_session::session::Role::Stream,
    );
    let session = Session::new(
        launch,
        butterpollo_media_policy::rtsp::Negotiated {
            codec: 3,
            bitrate_kbps: 80_000,
            ..Default::default()
        },
    );
    let mut config = fixture.host.config.read().unwrap().clone();
    config.values.insert("packet_pacing".into(), "1".into());
    let client = UdpSocket::bind("127.0.0.1:0")?;
    client.set_read_timeout(Some(Duration::from_secs(2)))?;
    let socket = Arc::new(ReplySocket::bind("127.0.0.1:0")?);
    let sender = Sender::new(
        socket,
        session,
        config,
        fixture.host.clone(),
        Instant::now(),
        None,
    )?;
    sender.submit(
        vec![Encoded {
            framing: None,
            bytes: vec![42; 3000],
            unit: butterpollo_win_graphics::encoder::Unit::Whole,
            idr: true,
            after_invalidation: false,
            frame_cap_bytes: None,
            reencoded: false,
            frame_cap_target: None,
            latency: None,
            presentation: None,
        }],
        client.local_addr()?,
        Duration::ZERO,
        Instant::now(),
        &mut Claims::default(),
    )?;
    let mut packet = [0; 2048];
    client.recv_from(&mut packet)?;
    thread::sleep(Duration::from_millis(30));
    let (done, finished) = std::sync::mpsc::channel();
    thread::spawn(move || {
        drop(sender);
        let _ = done.send(());
    });
    assert!(
        finished.recv_timeout(Duration::from_millis(250)).is_ok(),
        "sender teardown remained blocked by packet pacing"
    );
    client.set_read_timeout(Some(Duration::from_millis(100)))?;
    let error = client
        .recv_from(&mut packet)
        .expect_err("a packet was sent after stopping");
    assert!(matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    ));
    Ok(())
}

#[test]
fn a_pyrowave_picture_yields_host_timing_stages_and_a_record_row() {
    let fixture = crate::state::test_support::Fixture::new();
    let launch = fixture.launch(
        fixture.client(0),
        butterpollo_session::session::Role::Stream,
    );
    let session = Session::new(
        launch,
        butterpollo_media_policy::rtsp::Negotiated {
            codec: 3,
            bitrate_kbps: 80_000,
            ..Default::default()
        },
    );
    let asked = butterpollo_media_policy::host_timing::HostTimingRequest { enabled: true }.encode();
    assert_eq!(session.host_timings.on_request(&asked), Some(true));
    let (sink, rows) = butterpollo_media_policy::timing_record::channel(8);
    *session.timing_record.write().unwrap() = Some(Arc::new(sink));
    let client = UdpSocket::bind("127.0.0.1:0").unwrap();
    let socket = Arc::new(ReplySocket::bind("127.0.0.1:0").unwrap());
    // The media clock started 500 ms ago; the picture was presented 6 ms ago.
    let now = Instant::now();
    let ago = |ms: u64| now - Duration::from_millis(ms);
    let sender = Sender::new(
        socket,
        session.clone(),
        fixture.host.config.read().unwrap().clone(),
        fixture.host.clone(),
        ago(500),
        None,
    )
    .unwrap();
    let mut claims = Claims::default();
    claims.push(Claim {
        key: ago(6),
        presented: ago(6),
        acquired: ago(5),
        published: ago(4),
        claimed: ago(3),
        submitted: ago(2),
    });
    let picture = Encoded {
        framing: None,
        bytes: vec![42; 3000],
        unit: butterpollo_win_graphics::encoder::Unit::Whole,
        idr: true,
        after_invalidation: false,
        frame_cap_bytes: None,
        reencoded: false,
        frame_cap_target: None,
        latency: Some(Duration::from_millis(3)),
        presentation: Some(ago(6)),
    };
    sender
        .submit(
            vec![picture],
            client.local_addr().unwrap(),
            Duration::ZERO,
            now,
            &mut claims,
        )
        .unwrap();
    let mut report = None;
    for _ in 0..400 {
        report = session.host_timings.take_report();
        if report.is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let report = report.expect("no 0x5536 record for the PyroWave picture");
    let records =
        butterpollo_media_policy::host_timing::HostTiming::decode_report_with(&report).unwrap();
    assert_eq!(records.len(), 1);
    let (record, extra) = records[0];
    assert_eq!(record.frame, 1);
    assert_eq!(record.queue_us, 4_000, "capture, hold, claim and convert");
    let extra = extra.unwrap();
    assert_eq!(
        (extra.presented_at_us, extra.capture_us, extra.convert_us),
        (494_000, 1_000, 1_000)
    );
    // Wall-clock stages (encode, send) vary; the named stages never exceed the total.
    assert!(record.queue_us + record.encode_us + record.send_us <= record.host_us);
    assert!(
        record.host_us >= 4_000,
        "present to submitted alone is 4 ms"
    );
    // The stage samples exist too, and the record has its row.
    let performance = &session.info()["performance"];
    assert_eq!(performance["stages"]["frames"], 1);
    let row = rows.try_recv().unwrap();
    assert_eq!(row.wire_frame, 1);
    assert!(row.rtp_us.abs_diff(494_000) <= 12, "{}", row.rtp_us);
    assert_eq!(row.points[0], 494_000);
    assert_eq!(row.points[4], 498_000);
    // The report sums nine stages, each cut to whole microseconds.
    assert!(
        (row.points[9] - row.points[0]).abs_diff(i64::from(record.host_us)) <= 9,
        "the row and the report end at the same last packet"
    );
}

/// The link-rate controller weighs the client's loss reports against what
/// the host sent, so the PyroWave sender records every picture it sends.
/// Without it a PyroWave stream that loses frames on Wi-Fi is never cut.
#[test]
fn a_sent_pyrowave_picture_is_recorded_for_the_link_rate_controller() {
    let fixture = crate::state::test_support::Fixture::new();
    let launch = fixture.launch(
        fixture.client(0),
        butterpollo_session::session::Role::Stream,
    );
    let session = Session::new(
        launch,
        butterpollo_media_policy::rtsp::Negotiated {
            codec: 3,
            bitrate_kbps: 80_000,
            ..Default::default()
        },
    );
    session.configure_link_rate(true, 0);
    assert!(
        session.link_sent.live(),
        "PyroWave has the link-rate controller"
    );
    let client = UdpSocket::bind("127.0.0.1:0").unwrap();
    let socket = Arc::new(ReplySocket::bind("127.0.0.1:0").unwrap());
    let sender = Sender::new(
        socket,
        session.clone(),
        fixture.host.config.read().unwrap().clone(),
        fixture.host.clone(),
        Instant::now(),
        None,
    )
    .unwrap();
    sender
        .submit(
            vec![Encoded {
                framing: None,
                bytes: vec![42; 3000],
                unit: butterpollo_win_graphics::encoder::Unit::Whole,
                idr: true,
                after_invalidation: false,
                frame_cap_bytes: None,
                reencoded: false,
                frame_cap_target: None,
                latency: None,
                presentation: None,
            }],
            client.local_addr().unwrap(),
            Duration::ZERO,
            Instant::now(),
            &mut Claims::default(),
        )
        .unwrap();
    for _ in 0..400 {
        if session.link_sent.head() > 0 {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let mut recorded = Vec::new();
    session.link_sent.drain(&mut 0, |frame, packets, bytes, _| {
        recorded.push((frame, packets, bytes))
    });
    let packets = session.stats.packets.load(Ordering::Relaxed);
    let payload = session.stats.bytes.load(Ordering::Relaxed);
    assert_eq!(recorded.len(), 1, "one picture, one record");
    let (frame, recorded_packets, wire_bytes) = recorded[0];
    assert_eq!(frame, 1);
    assert!(packets > 0);
    assert_eq!(u64::from(recorded_packets), packets);
    assert_eq!(
        u64::from(wire_bytes),
        payload + packets * butterpollo_media_policy::network_pacing::overhead(false) as u64,
        "the payload sent and each datagram's IP and Ethernet framing"
    );
}

#[test]
fn prepared_intra_frames_freeze_timing_and_keep_only_the_newest_waiter() {
    let wait = Duration::from_secs(4);
    for encrypted in [false, true] {
        let fixture = crate::state::test_support::Fixture::new();
        let launch = fixture.launch(
            fixture.client(0),
            butterpollo_session::session::Role::Stream,
        );
        let session = Session::new(
            launch,
            butterpollo_media_policy::rtsp::Negotiated {
                encryption: if encrypted { 3 } else { 1 },
                bitrate_kbps: 80_000,
                configured_bitrate_kbps: 100_000,
                ..Default::default()
            },
        );
        session.bitrate.store(40_000, Ordering::Relaxed);
        let config = fixture.host.config.read().unwrap().clone();
        let sender = Sender::packetizer(
            session.clone(),
            config,
            fixture.host.clone(),
            Instant::now(),
            None,
        )
        .unwrap();
        let snapshot = |wire_frame| {
            let ready = sender.slot.ready.lock().unwrap();
            let (ready, timeout) = sender
                .slot
                .ready_changed
                .wait_timeout_while(ready, wait, |r| {
                    r.open || r.frame.as_ref().is_none_or(|f| f.wire_frame != wire_frame)
                })
                .unwrap();
            let result = ready.frame.as_ref().map(|f| {
                (
                    f.processing,
                    f.latency,
                    f.kbps,
                    f.blocks[0].packets[0].clone(),
                    f.blocks.iter().map(|b| b.count).sum::<usize>(),
                )
            });
            drop(ready);
            assert!(
                !timeout.timed_out(),
                "packetizer did not prepare frame {wire_frame}"
            );
            result.unwrap()
        };
        let frame = || Encoded {
            framing: None,
            bytes: vec![42; 3000],
            unit: butterpollo_win_graphics::encoder::Unit::Whole,
            idr: true,
            after_invalidation: false,
            frame_cap_bytes: None,
            reencoded: false,
            frame_cap_target: None,
            latency: Some(Duration::from_millis(12)),
            presentation: None,
        };
        let tick = Instant::now() - Duration::from_millis(2);
        sender
            .submit(
                vec![frame()],
                "127.0.0.1:12345".parse().unwrap(),
                Duration::ZERO,
                tick,
                &mut Claims::default(),
            )
            .unwrap();
        let (processing, latency, kbps, packet, packets) = snapshot(1);
        assert_eq!(latency, 12_000);
        assert_eq!(kbps, 50_000);
        let plaintext = if encrypted {
            butterpollo_crypto::crypto::gcm_open(
                &session.launch.key,
                &packet[..12],
                &packet[16..32],
                &packet[32..],
            )
            .unwrap()
        } else {
            packet.clone()
        };
        assert_eq!(
            u16::from_le_bytes(plaintext[33..35].try_into().unwrap()),
            (processing / 100).min(65535) as u16
        );
        thread::sleep(Duration::from_millis(30));
        assert_eq!(snapshot(1).0, processing);
        assert_eq!(snapshot(1).3, packet);

        // A slow sender consumes the latest independent intra frame;
        // replacing prepared packets must never reuse their encryption IVs.
        sender
            .submit(
                vec![frame()],
                "127.0.0.1:12345".parse().unwrap(),
                Duration::ZERO,
                Instant::now(),
                &mut Claims::default(),
            )
            .unwrap();
        let (_, _, _, next, _) = snapshot(2);
        assert_eq!(session.stats.frames_replaced.load(Ordering::Relaxed), 1);
        assert_eq!(session.stats.frames.load(Ordering::Relaxed), 0);
        if encrypted {
            assert_eq!(
                u64::from_le_bytes(next[..8].try_into().unwrap()),
                packets as u64
            );
        }
        let (done, finished) = std::sync::mpsc::channel();
        thread::spawn(move || {
            drop(sender);
            done.send(()).unwrap();
        });
        finished.recv_timeout(wait).unwrap();
    }
}

/// Review 2026-11-07: the sender thread (031dc57e5) built every block of a
/// frame before the first left, losing 6772d4019's early blocks. An idle
/// sender gets block 0 with the frame's details and the rest as they are
/// built; a busy one gets the whole frame, which a newer one replaces.
#[test]
fn an_idle_sender_gets_each_block_as_it_is_built() {
    for early in [true, false] {
        let fixture = crate::state::test_support::Fixture::new();
        let launch = fixture.launch(
            fixture.client(0),
            butterpollo_session::session::Role::Stream,
        );
        let session = Session::new(
            launch,
            butterpollo_media_policy::rtsp::Negotiated {
                bitrate_kbps: 80_000,
                ..Default::default()
            },
        );
        let mut config = fixture.host.config.read().unwrap().clone();
        if !early {
            config
                .values
                .insert("video_send_early_blocks".into(), "false".into());
        }
        let sender = Sender::packetizer(
            session.clone(),
            config,
            fixture.host.clone(),
            Instant::now(),
            None,
        )
        .unwrap();
        let submit = || {
            sender
                .submit(
                    vec![Encoded {
                        framing: None,
                        bytes: vec![42; 400_000],
                        unit: butterpollo_win_graphics::encoder::Unit::Whole,
                        idr: true,
                        after_invalidation: false,
                        frame_cap_bytes: None,
                        reencoded: false,
                        frame_cap_target: None,
                        latency: None,
                        presentation: None,
                    }],
                    "127.0.0.1:12345".parse().unwrap(),
                    Duration::ZERO,
                    Instant::now(),
                    &mut Claims::default(),
                )
                .unwrap()
        };
        // Whether the frame was streamed, its blocks in hand and to follow,
        // and its packets.
        let settled = |wire_frame| {
            let ready = sender.slot.ready.lock().unwrap();
            let (ready, timeout) = sender
                .slot
                .ready_changed
                .wait_timeout_while(ready, Duration::from_secs(4), |r| {
                    r.open || r.frame.as_ref().is_none_or(|f| f.wire_frame != wire_frame)
                })
                .unwrap();
            assert!(!timeout.timed_out(), "frame {wire_frame} was not prepared");
            let frame = ready.frame.as_ref().unwrap();
            let packets = frame
                .blocks
                .iter()
                .chain(&ready.blocks)
                .map(|block| block.count)
                .sum::<usize>();
            (
                frame.streamed,
                frame.blocks.len(),
                ready.blocks.len(),
                packets,
            )
        };
        submit();
        let (streamed, first, later, packets) = settled(1);
        if early {
            assert_eq!((streamed, first), (true, 1));
            assert!(later > 0, "a 400 KB frame takes more than one block");
        } else {
            assert_eq!((streamed, later), (false, 0));
            assert!(first > 1);
        }
        // The sender is busy: the next frame comes whole, in place of the
        // one it never took, and that one's later blocks go with it.
        sender.slot.ready.lock().unwrap().sending = true;
        submit();
        let (streamed, first, later, next) = settled(2);
        assert_eq!(
            (streamed, first > 1, later, next),
            (false, true, 0, packets)
        );
        assert_eq!(session.stats.frames_replaced.load(Ordering::Relaxed), 1);
    }
}
