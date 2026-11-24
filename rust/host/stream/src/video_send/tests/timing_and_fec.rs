//! Unit tests for packetizer timing freezes, adaptive FEC reports, stage records, and inline parity.

use super::super::*;
use super::*;

#[test]
fn moonlight_timing_stops_at_packetization_before_a_blocked_sender() {
    for key in [None, Some([23; 16])] {
        let fixture = crate::state::test_support::Fixture::new();
        let session = fixture.session(
            fixture.client(0),
            butterpollo_session::session::Role::Stream,
        );
        let socket = Arc::new(ReplySocket::bind("127.0.0.1:0").unwrap());
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver.set_read_timeout(Some(WAIT)).unwrap();
        let start = Instant::now();
        let tick = start - Duration::from_millis(2);
        let mut sender =
            Sender::packetizer(session.clone(), start, handoff(packetizer(key)), false).unwrap();
        let mut picture = frame(2);
        picture.frame_cap_bytes = Some(112_815);
        picture.latency = Some(Duration::from_millis(12));
        picture.presentation = Some(tick - Duration::from_millis(15));
        let mut claims = Claims::default();
        claims.push(Claim {
            key: picture.presentation.unwrap(),
            presented: picture.presentation.unwrap(),
            acquired: tick - Duration::from_millis(14),
            published: tick - Duration::from_millis(13),
            claimed: tick - Duration::from_millis(10),
            submitted: tick - Duration::from_millis(9),
        });
        sender
            .submit_tick(
                vec![picture],
                receiver.local_addr().unwrap(),
                Duration::ZERO,
                &mut claims,
                tick,
            )
            .unwrap();
        let processing = {
            let state = wait_prepared(&sender);
            let ready = state.ready.as_ref().unwrap();
            assert_eq!(ready.frame_cap_bytes, Some(112_815));
            assert_eq!(
                ready.processing,
                micros(ready.packetized.duration_since(tick))
            );
            assert_eq!(ready.latency, 12_000);
            ready.processing
        };
        // A controlled downstream stall: the production packetizer finishes,
        // but the production paced sender has not started yet.
        thread::sleep(Duration::from_millis(30));
        assert_eq!(
            wait_prepared(&sender).ready.as_ref().unwrap().processing,
            processing
        );
        let shared = sender.slot.clone();
        let s = session.clone();
        let config = fixture.host.config.read().unwrap().clone();
        let host = fixture.host.clone();
        sender.transport = Some(thread::spawn(move || {
            let result = transport::run(
                &shared,
                socket,
                s,
                config,
                host,
                start,
                Instant::now(),
                Default::default(),
                None,
                Default::default(),
            );
            shared.finish(result);
        }));
        assert_eq!(sender.next_wire_frame().unwrap(), 2);
        let mut packet = [0; 2048];
        let len = receiver.recv(&mut packet).unwrap();
        let packet = match key {
            None => packet[..len].to_vec(),
            Some(key) => butterpollo_crypto::crypto::gcm_open(
                &key,
                &packet[..12],
                &packet[16..32],
                &packet[32..len],
            )
            .unwrap(),
        };
        assert_eq!(
            u16::from_le_bytes(packet[33..35].try_into().unwrap()),
            (processing / 100).min(65535) as u16
        );
        let performance = &session.info()["performance"];
        assert_eq!(performance["encode_mean_ms"], 12.);
        assert_eq!(
            performance["host_processing_mean_ms"],
            processing as f64 / 1000.
        );
        assert!(
            performance["stages"]["send_queue"]["mean_ms"]
                .as_f64()
                .unwrap()
                >= 30.
        );
    }
}

#[test]
fn keyframe_skips_raw_and_prepared_waiters_without_spending_wire_numbers_twice() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(
        fixture.client(0),
        butterpollo_session::session::Role::Stream,
    );
    let sender = Sender::packetizer(
        session,
        Instant::now(),
        handoff(packetizer(Some([9; 16]))),
        false,
    )
    .unwrap();
    sender
        .submit(
            vec![frame(2)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    drop(wait_prepared(&sender));
    sender
        .submit(
            vec![frame(3)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    {
        let state = sender.slot.state.lock().unwrap();
        let (state, timeout) = sender
            .slot
            .changed
            .wait_timeout_while(state, WAIT, |s| !s.sending)
            .unwrap();
        assert!(!timeout.timed_out());
        assert!(state.ready.is_some());
    }
    // Raw frame 3 is being prepared; frame 4 waits behind it. Prepared wire
    // frame 1 and the raw waiter are discarded, but only the raw waiter still
    // needs a wire number spent. The in-progress frame becomes wire frame 2.
    sender
        .submit(
            vec![frame(4)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    sender.skip_pending().unwrap();
    {
        let state = wait_prepared(&sender);
        assert_eq!(state.ready.as_ref().unwrap().wire_frame, 2);
    }
    sender
        .submit(
            vec![frame(1)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    let (idr, wire_frame, iv_counter) = {
        let state = wait_prepared(&sender);
        let ready = state.ready.as_ref().unwrap();
        (ready.idr, ready.wire_frame, ready.packetizer.iv_counter)
    };
    assert!(idr);
    assert_eq!(wire_frame, 4);
    // Two prepared 3 KB frames spent five packet IVs apiece (data + FEC).
    assert_eq!(iv_counter, 10);
}

#[test]
fn stopping_wakes_a_packetizer_blocked_on_the_prepared_slot() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(
        fixture.client(0),
        butterpollo_session::session::Role::Stream,
    );
    let sender =
        Sender::packetizer(session, Instant::now(), handoff(packetizer(None)), false).unwrap();
    sender
        .submit(
            vec![frame(2)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    drop(wait_prepared(&sender));
    sender
        .submit(
            vec![frame(3)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        drop(sender);
        done.send(()).unwrap();
    });
    finished.recv_timeout(WAIT).unwrap();
}

#[test]
fn a_sender_taking_over_mid_stream_continues_the_inline_numbering() {
    for key in [None, Some([17; 16])] {
        let fixture = crate::state::test_support::Fixture::new();
        let session = fixture.session(
            fixture.client(0),
            butterpollo_session::session::Role::Stream,
        );
        let socket = Arc::new(ReplySocket::bind("127.0.0.1:0").unwrap());
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver.set_read_timeout(Some(WAIT)).unwrap();
        // Where the stream thread's own sends left off.
        let mut inline = packetizer(key);
        inline.frame = 300;
        inline.sequence = 7000;
        inline.iv_counter = 9000;
        let sender = Sender::new(
            socket,
            session.clone(),
            fixture.host.config.read().unwrap().clone(),
            fixture.host.clone(),
            Instant::now(),
            handoff(inline),
        )
        .unwrap();
        assert_eq!(sender.next_wire_frame().unwrap(), 300);
        sender
            .submit(
                vec![frame(1)],
                receiver.local_addr().unwrap(),
                Duration::ZERO,
                &mut Claims::default(),
            )
            .unwrap();
        assert_eq!(sender.next_wire_frame().unwrap(), 301);
        let mut packet = [0; 2048];
        let size = receiver.recv(&mut packet).unwrap();
        let packet = match key {
            None => packet[..size].to_vec(),
            Some(key) => {
                assert_eq!(u64::from_le_bytes(packet[..8].try_into().unwrap()), 9000);
                butterpollo_crypto::crypto::gcm_open(
                    &key,
                    &packet[..12],
                    &packet[16..32],
                    &packet[32..size],
                )
                .unwrap()
            }
        };
        assert!(size > 24);
        assert_eq!(u16::from_be_bytes([packet[2], packet[3]]), 7000);
        assert_eq!(u32::from_le_bytes(packet[20..24].try_into().unwrap()), 300);
    }
}

#[test]
fn the_sender_thread_accepts_client_reports_for_adaptive_fec() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(
        fixture.client(0),
        butterpollo_session::session::Role::Stream,
    );
    session.configure_fec(true, 20);
    let socket = Arc::new(ReplySocket::bind("127.0.0.1:0").unwrap());
    let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
    let sender = Sender::new(
        socket,
        session.clone(),
        fixture.host.config.read().unwrap().clone(),
        fixture.host.clone(),
        Instant::now(),
        handoff(packetizer(None)),
    )
    .unwrap();
    sender
        .submit(
            vec![frame(2)],
            receiver.local_addr().unwrap(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    assert_eq!(sender.next_wire_frame().unwrap(), 2);
    assert_eq!(session.stats.video_frame.load(Ordering::Acquire), 2);
    session.record_fec_status(&[
        0, 0, 0, 1, 0, 11, 0, 0, 0, 0, 0, 10, 0, 2, 0, 10, 0, 0, 20, 0, 1,
    ]);
    assert_eq!(session.info()["performance"]["fec_invalid_reports"], 0);
    session.tick_fec(Instant::now() + Duration::from_secs(60));
    assert!(session.fec_percent() < 20);
}

#[test]
fn the_sender_thread_records_where_a_new_pictures_time_went() {
    let fixture = crate::state::test_support::Fixture::new();
    let session = fixture.session(
        fixture.client(0),
        butterpollo_session::session::Role::Stream,
    );
    let socket = Arc::new(ReplySocket::bind("127.0.0.1:0").unwrap());
    let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
    // The media clock started 500 ms ago.
    let now = Instant::now();
    let ago = |ms: u64| now - Duration::from_millis(ms);
    let epoch = ago(500);
    let mut handoff = handoff(packetizer(None));
    handoff.last_stamp = ago(10);
    let sender = Sender::new(
        socket,
        session.clone(),
        fixture.host.config.read().unwrap().clone(),
        fixture.host.clone(),
        epoch,
        handoff,
    )
    .unwrap();
    // The host-side record is on.
    let (sink, rows) = butterpollo_media_policy::timing_record::channel(8);
    *session.timing_record.write().unwrap() = Some(Arc::new(sink));
    // A client that asked for host timing reports (0x5536).
    let asked = butterpollo_media_policy::host_timing::HostTimingRequest { enabled: true }.encode();
    assert_eq!(session.host_timings.on_request(&asked), Some(true));
    // A picture presented 6 ms ago, held 1 ms, claimed 3 ms ago and encoded
    // in 2 ms, beside a static repeat that has no present.
    let mut claims = Claims::default();
    claims.push(Claim {
        key: ago(6),
        presented: ago(6),
        acquired: ago(5),
        published: ago(4),
        claimed: ago(3),
        submitted: ago(2),
    });
    let mut picture = frame(2);
    picture.presentation = Some(ago(6));
    picture.latency = Some(Duration::from_millis(3));
    let mut repeat = frame(3);
    repeat.presentation = Some(now);
    sender
        .submit_tick(
            vec![picture, repeat],
            receiver.local_addr().unwrap(),
            Duration::ZERO,
            &mut claims,
            now,
        )
        .unwrap();
    assert_eq!(sender.next_wire_frame().unwrap(), 3);
    let performance = &session.info()["performance"];
    let stages = &performance["stages"];
    let ms = |stage: &str| stages[stage]["mean_ms"].as_f64().unwrap();
    assert_eq!(performance["sample_frames"], 2);
    assert_eq!(stages["frames"], 1);
    assert_eq!(
        (ms("capture"), ms("hold"), ms("claim"), ms("convert")),
        (1., 1., 1., 1.)
    );
    // Encoding ran from the submission to the output reaching the sender.
    assert!(ms("encode") >= 2.);
    let parts: f64 = butterpollo_media_policy::performance::STAGES
        .iter()
        .map(|stage| ms(stage))
        .sum();
    assert!((parts - ms("present_to_sent")).abs() < 0.01);
    assert!(ms("present_to_sent") >= 6.);
    // The tick is exactly 6 ms after presentation. Everything after entry
    // into packetization remains in the full path, outside Moonlight's field.
    let gap = ms("present_to_sent") - ms("moonlight_host");
    let excluded = 6. + ms("packetize") + ms("send_queue") + ms("send");
    assert!((gap - excluded).abs() < 0.01, "{gap} != {excluded}");
    // The client that asked hears the whole span for the new picture alone,
    // numbered as its packets are, with the same stages.
    let report = session.host_timings.take_report().unwrap();
    let records =
        butterpollo_media_policy::host_timing::HostTiming::decode_report(&report).unwrap();
    assert_eq!(records.len(), 1);
    let record = records[0];
    // The extension holds the present on the media clock and the split stages, and the
    // record's row has the stamp points themselves.
    let (_, extra) = butterpollo_media_policy::host_timing::HostTiming::decode_report_with(&report)
        .unwrap()
        .remove(0);
    let extra = extra.unwrap();
    assert_eq!(
        (extra.presented_at_us, extra.capture_us, extra.convert_us),
        (494_000, 1_000, 1_000)
    );
    let row = rows.try_recv().unwrap();
    assert!(rows.try_recv().is_err(), "the static repeat has no row");
    assert_eq!(row.wire_frame, 1);
    assert!(row.rtp_us.abs_diff(494_000) <= 12, "{}", row.rtp_us);
    let [
        present,
        acquired,
        published,
        claimed,
        submitted,
        encoded,
        packetized,
        prepared,
        first,
        last,
    ] = row.points;
    assert_eq!(
        [present, acquired, published, claimed, submitted],
        [494_000, 495_000, 496_000, 497_000, 498_000]
    );
    assert!(submitted <= encoded && encoded <= packetized && packetized <= prepared);
    assert!(prepared <= first && first <= last);
    // The report sums nine stages, each cut to whole microseconds.
    assert!(
        (last - present).abs_diff(i64::from(record.host_us)) <= 9,
        "the row and the report end at the same last packet"
    );
    assert_eq!(record.frame, 1);
    let us = |ms: f64| (ms * 1000.).round() as i64;
    assert!((i64::from(record.host_us) - us(ms("present_to_sent"))).abs() <= 1);
    assert_eq!(
        i64::from(record.queue_us),
        us(ms("capture") + ms("hold") + ms("claim") + ms("convert"))
    );
    assert!((i64::from(record.encode_us) - us(ms("encode"))).abs() <= 1);
    assert!((i64::from(record.send_us) - us(ms("send"))).abs() <= 1);
    assert_eq!(session.host_timings.take_report(), None);
}

#[test]
fn reconnect_keeps_packet_order_fec_recovery_and_iv_ownership_identical_to_inline() {
    for key in [None, Some([17; 16]), Some([29; 16])] {
        let (sent, packets) = mpsc::channel();
        let sender = Sender::spawn("reconnect".into(), 1, move |slot| {
            let mut packetizer = packetizer(key);
            slot.run(|frame| {
                let frame = frame.encoded;
                sent.send(packetizer.encode_recovery(
                    &frame.bytes,
                    frame.idr,
                    frame.after_invalidation,
                    9000,
                    1234,
                )?)
                .unwrap();
                Ok(packetizer.frame)
            })
        })
        .unwrap();
        assert_eq!(sender.next_wire_frame().unwrap(), 1);
        let mut inline = packetizer(key);
        for index in 1..=4 {
            let expected = frame(index);
            sender
                .submit(
                    vec![frame(index)],
                    peer(),
                    Duration::ZERO,
                    &mut Claims::default(),
                )
                .unwrap();
            assert_eq!(
                packets.recv_timeout(WAIT).unwrap(),
                inline
                    .encode_recovery(
                        &expected.bytes,
                        expected.idr,
                        expected.after_invalidation,
                        9000,
                        1234
                    )
                    .unwrap()
            );
        }
        assert_eq!(sender.next_wire_frame().unwrap(), u64::from(inline.frame));
        drop(sender);
        assert!(matches!(
            packets.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }
}
