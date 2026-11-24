//! Unit tests for sender slot claims, backpressure waits, keyframe skipping, and error/shutdown propagation.

use super::super::*;
use super::*;

#[test]
fn each_queued_frame_carries_its_pictures_claim_and_repeats_carry_none() {
    let (sender, frames, release) = blocked_sender();
    let start = Instant::now();
    let claim = |ms: u64| {
        let at = start + Duration::from_millis(ms);
        Claim {
            key: at,
            presented: at,
            acquired: at,
            published: at,
            claimed: at,
            submitted: at,
        }
    };
    let mut claims = Claims::default();
    claims.push(claim(1));
    claims.push(claim(2));
    let mut first = frame(2);
    first.presentation = Some(claim(1).key);
    // A static repeat: the encoder returns a presentation nobody claimed.
    let mut repeat = frame(3);
    repeat.presentation = Some(start + Duration::from_millis(9));
    let mut second = frame(4);
    second.presentation = Some(claim(2).key);
    thread::scope(|scope| {
        let sender = &sender;
        let producer = scope.spawn(move || {
            sender
                .submit(
                    vec![first, repeat, second],
                    peer(),
                    Duration::ZERO,
                    &mut claims,
                )
                .unwrap();
            claims
        });
        let mut seen = vec![];
        for _ in 0..3 {
            seen.push(frames.recv_timeout(WAIT).unwrap().claim);
            release.send(()).unwrap();
        }
        assert_eq!(seen, [Some(claim(1)), None, Some(claim(2))]);
        let mut claims = producer.join().unwrap();
        assert_eq!(claims.take(Some(claim(2).key)), None);
    });
}

#[test]
fn a_keyframe_waiting_for_the_sender_also_blocks_the_next_claim() {
    let now = Instant::now();
    let state = State {
        pending: Some(Frame {
            encoded: frame(1),
            peer: peer(),
            claimed: now,
            polled: now,
            latency: Duration::ZERO,
            processing_started: now,
            skipped: 0,
            claim: None,
            slice_blocks: 0,
        }),
        sending: false,
        sending_keyframe: false,
        ready: None,
        transmitting: false,
        transmitting_keyframe: false,
        transmitting_remaining: None,
        recovery_lead: None,
        recycled: Vec::new(),
        streaming: false,
        whole_open: false,
        next_wire_frame: 1,
        skipped: 0,
        error: None,
    };
    assert_eq!(state.backlog().unwrap(), 1);
    assert_eq!(state.claim_backlog(None).unwrap(), 2);
}

#[test]
fn capacity_wait_wakes_after_the_sender_releases_a_frame() {
    let (sender, frames, release) = blocked_sender();
    sender
        .submit(
            vec![frame(1)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    frames.recv_timeout(WAIT).unwrap();
    assert_eq!(sender.backlog().unwrap(), 1);
    assert_eq!(sender.claim_backlog(None).unwrap(), 2);
    thread::scope(|scope| {
        let (entered, waiting) = mpsc::channel();
        let (finished, done) = mpsc::channel();
        let sender = &sender;
        scope.spawn(move || {
            entered.send(()).unwrap();
            sender.wait_for_capacity(2, WAIT, None).unwrap();
            finished.send(()).unwrap();
        });
        waiting.recv_timeout(WAIT).unwrap();
        assert!(done.recv_timeout(Duration::from_millis(20)).is_err());
        release.send(()).unwrap();
        done.recv_timeout(WAIT).unwrap();
        sender
            .submit(
                vec![frame(2)],
                peer(),
                Duration::ZERO,
                &mut Claims::default(),
            )
            .unwrap();
        frames.recv_timeout(WAIT).unwrap();
        assert_eq!(sender.claim_backlog(None).unwrap(), 1);
        release.send(()).unwrap();
    });
}

#[test]
fn recovery_progress_opens_one_slot_without_changing_order_or_skipping_a_frame() {
    use butterpollo_media_policy::encoder_backlog::{Backpressure, backpressure};
    let (sender, frames, release) = blocked_sender();
    sender
        .submit(
            vec![frame(1)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    frames.recv_timeout(WAIT).unwrap();
    // Model the packetizer handing its keyframe to the transmitter. The
    // blocked test worker keeps ownership stable while admission is checked.
    {
        let mut state = sender.slot.state.lock().unwrap();
        state.sending = false;
        state.sending_keyframe = false;
        state.transmitting = true;
        state.transmitting_keyframe = true;
    }
    let lead = Some(Duration::from_millis(4));
    assert_eq!(sender.claim_backlog(lead).unwrap(), 2);
    sender
        .slot
        .recovery_progress(Some(Duration::from_millis(5)));
    assert_eq!(sender.claim_backlog(lead).unwrap(), 2);
    sender
        .slot
        .recovery_progress(Some(Duration::from_millis(4)));
    assert_eq!(sender.claim_backlog(lead).unwrap(), 1);
    assert_eq!(sender.claim_backlog(None).unwrap(), 2);
    assert_eq!(
        backpressure(0, sender.claim_backlog(lead).unwrap(), 1),
        Backpressure::None
    );
    assert_eq!(
        backpressure(1, sender.claim_backlog(lead).unwrap(), 2),
        Backpressure::Sender
    );
    // Progress never spends a wire number, packet sequence or encryption IV.
    {
        let state = sender.slot.state.lock().unwrap();
        assert_eq!(state.next_wire_frame, 1);
        assert_eq!(state.skipped, 0);
    }
    // A socket stall invalidates the estimate and restores the full barrier.
    sender.slot.recovery_progress(None);
    assert_eq!(sender.claim_backlog(lead).unwrap(), 2);
    sender
        .slot
        .recovery_progress(Some(Duration::from_millis(3)));
    // A newly queued recovery remains blocked, even near the old one's end.
    {
        let mut state = sender.slot.state.lock().unwrap();
        state.sending_keyframe = true;
    }
    assert_eq!(sender.claim_backlog(lead).unwrap(), 2);
    release.send(()).unwrap();
}

#[test]
fn recovery_progress_wakes_capacity_wait_before_the_keyframe_finishes() {
    let (sender, frames, release) = blocked_sender();
    sender
        .submit(
            vec![frame(1)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    frames.recv_timeout(WAIT).unwrap();
    {
        let mut state = sender.slot.state.lock().unwrap();
        state.sending = false;
        state.sending_keyframe = false;
        state.transmitting = true;
        state.transmitting_keyframe = true;
    }
    thread::scope(|scope| {
        let (finished, done) = mpsc::channel();
        let sender = &sender;
        scope.spawn(move || {
            sender
                .wait_for_capacity(2, WAIT, Some(Duration::from_millis(4)))
                .unwrap();
            finished.send(()).unwrap();
        });
        assert!(done.recv_timeout(Duration::from_millis(20)).is_err());
        sender
            .slot
            .recovery_progress(Some(Duration::from_millis(4)));
        done.recv_timeout(WAIT).unwrap();
        assert_eq!(sender.backlog().unwrap(), 1);
        release.send(()).unwrap();
    });
}

#[test]
fn ordered_output_waits_for_the_single_pending_slot_without_replacing_frames() {
    let (sender, frames, release) = blocked_sender();
    sender
        .submit(
            vec![frame(1)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    assert!(frames.recv_timeout(WAIT).unwrap().encoded.idr);
    assert_eq!(sender.backlog().unwrap(), 1);
    thread::scope(|scope| {
        let (submitted, done) = mpsc::channel();
        let sender = &sender;
        let producer = scope.spawn(move || {
            let mut third = frame(3);
            third.latency = Some(Duration::from_millis(3));
            sender
                .submit(
                    vec![frame(2), third],
                    peer(),
                    Duration::from_millis(7),
                    &mut Claims::default(),
                )
                .unwrap();
            submitted.send(()).unwrap();
        });
        // Wait for the producer to fill the slot, not for thread scheduling.
        {
            let state = sender.slot.state.lock().unwrap();
            let (state, timeout) = sender
                .slot
                .changed
                .wait_timeout_while(state, WAIT, |s| s.pending.is_none())
                .unwrap();
            assert!(!timeout.timed_out());
            assert_eq!(state.pending.as_ref().unwrap().encoded.bytes[0], 2);
            assert_eq!(state.backlog().unwrap(), 2);
        }
        assert!(matches!(done.try_recv(), Err(mpsc::TryRecvError::Empty)));
        let released = Instant::now();
        release.send(()).unwrap();
        let second = frames.recv_timeout(WAIT).unwrap();
        done.recv_timeout(WAIT).unwrap();
        assert_eq!(sender.backlog().unwrap(), 2);
        release.send(()).unwrap();
        let third = frames.recv_timeout(WAIT).unwrap();
        assert_eq!(second.encoded.bytes[0], 2);
        assert_eq!(third.encoded.bytes[0], 3);
        assert!(third.encoded.after_invalidation);
        assert_eq!(second.peer, peer());
        assert_eq!(second.polled, third.polled);
        assert!(third.polled <= released);
        assert_eq!(second.polled - second.claimed, Duration::from_millis(7));
        assert_eq!(third.polled - third.claimed, Duration::from_millis(3));
        release.send(()).unwrap();
        producer.join().unwrap();
    });
    assert_eq!(sender.next_wire_frame().unwrap(), 4);
    assert_eq!(sender.backlog().unwrap(), 0);
}

#[test]
fn recreation_waits_for_sending_and_pending_wire_indices() {
    let (sender, frames, release) = blocked_sender();
    sender
        .submit(
            vec![frame(1)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    frames.recv_timeout(WAIT).unwrap();
    sender
        .submit(
            vec![frame(2)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    thread::scope(|scope| {
        let (mapped, result) = mpsc::channel();
        let sender = &sender;
        scope.spawn(move || mapped.send(sender.next_wire_frame().unwrap()).unwrap());
        assert!(matches!(result.try_recv(), Err(mpsc::TryRecvError::Empty)));
        release.send(()).unwrap();
        frames.recv_timeout(WAIT).unwrap();
        assert!(matches!(result.try_recv(), Err(mpsc::TryRecvError::Empty)));
        release.send(()).unwrap();
        assert_eq!(result.recv_timeout(WAIT).unwrap(), 3);
    });
}

#[test]
fn a_keyframe_request_skips_the_waiting_frame_and_keeps_its_wire_number() {
    let (entered, frames) = mpsc::channel();
    let (release, resume) = mpsc::channel::<()>();
    let sender = Sender::spawn("skip".into(), 1, move |slot| {
        let mut wire = 1;
        slot.run(|frame| {
            wire += frame.skipped;
            entered
                .send((frame.encoded.bytes[0], wire, frame.skipped))
                .unwrap();
            wire += 1;
            resume.recv_timeout(WAIT)?;
            Ok(wire)
        })
    })
    .unwrap();
    sender
        .submit(
            vec![frame(1)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    assert_eq!(frames.recv_timeout(WAIT).unwrap(), (1, 1, 0));
    // A request while a frame waits: it is skipped, the one sending is not.
    sender
        .submit(
            vec![frame(2)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    assert_eq!(sender.backlog().unwrap(), 2);
    assert_eq!(sender.skip_pending().unwrap(), 1);
    assert_eq!(sender.skip_pending().unwrap(), 1);
    // A keyframe takes the place of a frame waiting before it.
    sender
        .submit(
            vec![frame(3)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    let mut keyframe = frame(4);
    keyframe.idr = true;
    thread::scope(|scope| {
        let (submitted, done) = mpsc::channel();
        let sender = &sender;
        scope.spawn(move || {
            sender
                .submit(
                    vec![keyframe],
                    peer(),
                    Duration::ZERO,
                    &mut Claims::default(),
                )
                .unwrap();
            submitted.send(()).unwrap();
        });
        done.recv_timeout(WAIT).unwrap();
    });
    assert_eq!(sender.backlog().unwrap(), 2);
    // A waiting keyframe is never skipped.
    assert_eq!(sender.skip_pending().unwrap(), 2);
    release.send(()).unwrap();
    assert_eq!(frames.recv_timeout(WAIT).unwrap(), (4, 4, 2));
    release.send(()).unwrap();
    assert_eq!(sender.next_wire_frame().unwrap(), 5);
    // A skipped frame with nothing after it still counts for a new encoder.
    sender
        .submit(
            vec![frame(5)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    assert_eq!(frames.recv_timeout(WAIT).unwrap(), (5, 5, 0));
    sender
        .submit(
            vec![frame(6)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    assert_eq!(sender.skip_pending().unwrap(), 1);
    release.send(()).unwrap();
    assert_eq!(sender.next_wire_frame().unwrap(), 7);
}

#[test]
fn sender_failure_wakes_submit_and_recreation_and_stays_in_the_error_slot() {
    let (entered, sending) = mpsc::channel();
    let (fail, failure) = mpsc::channel();
    let sender = Sender::spawn("error".into(), 1, move |slot| {
        slot.run(|_| {
            entered.send(()).unwrap();
            failure.recv_timeout(WAIT)?;
            bail!("injected socket failure");
        })
    })
    .unwrap();
    sender
        .submit(
            vec![frame(1)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    sending.recv_timeout(WAIT).unwrap();
    thread::scope(|scope| {
        let producer = scope.spawn(|| {
            sender.submit(
                vec![frame(2), frame(3)],
                peer(),
                Duration::ZERO,
                &mut Claims::default(),
            )
        });
        let recreation = scope.spawn(|| sender.next_wire_frame());
        let state = sender.slot.state.lock().unwrap();
        let (state, timeout) = sender
            .slot
            .changed
            .wait_timeout_while(state, WAIT, |s| s.pending.is_none())
            .unwrap();
        assert!(!timeout.timed_out());
        assert_eq!(state.backlog().unwrap(), 2);
        drop(state);
        fail.send(()).unwrap();
        for error in [
            producer.join().unwrap().unwrap_err(),
            recreation.join().unwrap().unwrap_err(),
            sender.backlog().unwrap_err(),
            sender
                .submit(vec![], peer(), Duration::ZERO, &mut Claims::default())
                .unwrap_err(),
        ] {
            assert!(error.to_string().contains("injected socket failure"));
        }
    });
    assert!(matches!(
        sending.try_recv(),
        Err(mpsc::TryRecvError::Disconnected)
    ));
}

#[test]
fn initialization_failure_reaches_the_error_slot_and_reconnect_starts_clean() {
    let mut sender =
        Sender::spawn("init".into(), 1, |_| bail!("timer initialization failed")).unwrap();
    sender.worker.take().unwrap().join().unwrap();
    assert!(
        sender
            .backlog()
            .unwrap_err()
            .to_string()
            .contains("timer initialization failed")
    );
    assert!(
        sender
            .submit(
                vec![frame(1)],
                peer(),
                Duration::ZERO,
                &mut Claims::default()
            )
            .is_err()
    );
    drop(sender);
    let (sender, frames, release) = blocked_sender();
    assert_eq!(sender.next_wire_frame().unwrap(), 1);
    sender
        .submit(
            vec![frame(1)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    assert!(frames.recv_timeout(WAIT).unwrap().encoded.idr);
    release.send(()).unwrap();
    assert_eq!(sender.next_wire_frame().unwrap(), 2);
}

#[test]
fn dropping_an_idle_sender_wakes_and_joins_it() {
    let finished = Arc::new(AtomicBool::new(false));
    let done = finished.clone();
    let sender = Sender::spawn("idle".into(), 1, move |slot| {
        let result = slot.run(|_| panic!("no frame was submitted"));
        done.store(true, Ordering::Release);
        result
    })
    .unwrap();
    drop(sender);
    assert!(finished.load(Ordering::Acquire));
}

#[test]
fn a_sender_started_mid_stream_continues_the_wire_frame_numbering() {
    let sender = Sender::spawn("handoff".into(), 4242, move |slot| {
        slot.run(|_| panic!("no frame was submitted"))
    })
    .unwrap();
    assert_eq!(sender.next_wire_frame().unwrap(), 4242);
}

#[test]
fn shutdown_interrupts_sending_and_releases_pending_output() {
    let (entered, sending) = mpsc::channel();
    let sender = Sender::spawn("stop".into(), 1, move |slot| {
        slot.run(|frame| {
            entered.send(frame.encoded.bytes[0]).unwrap();
            let state = slot.state.lock().unwrap();
            let (_state, timeout) = slot
                .changed
                .wait_timeout_while(state, WAIT, |_| !slot.stop.load(Ordering::Acquire))
                .unwrap();
            assert!(!timeout.timed_out());
            Ok(2)
        })
    })
    .unwrap();
    sender
        .submit(
            vec![frame(1)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    assert_eq!(sending.recv_timeout(WAIT).unwrap(), 1);
    sender
        .submit(
            vec![frame(2)],
            peer(),
            Duration::ZERO,
            &mut Claims::default(),
        )
        .unwrap();
    thread::scope(|scope| {
        let producer = scope.spawn(|| {
            sender.submit(
                vec![frame(3)],
                peer(),
                Duration::ZERO,
                &mut Claims::default(),
            )
        });
        sender.slot.stop();
        producer.join().unwrap().unwrap();
    });
    let slot = sender.slot.clone();
    drop(sender);
    assert!(matches!(
        sending.try_recv(),
        Err(mpsc::TryRecvError::Disconnected)
    ));
    assert_eq!(slot.state.lock().unwrap().backlog().unwrap(), 0);
}
