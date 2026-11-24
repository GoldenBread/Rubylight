//! Tests for input worker timing, panic recovery, retry, histogram and co-op sink.

use super::*;

#[test]
fn the_queue_delay_is_measured_from_receipt_to_the_injector() {
    let mut r = rig(u32::MAX);
    let received = Instant::now() - Duration::from_millis(40);
    r.core.input(
        r#"{"v":1,"type":"key","code":"KeyA","down":true}"#,
        received,
    );
    // However long the machine held this thread, no delay exceeds this.
    let bound_us = received.elapsed().as_micros() as u64;
    r.core.input(
        r#"{"v":1,"type":"key","code":"KeyA","down":false}"#,
        Instant::now(),
    );
    let summary = r.core.summary();
    assert_eq!(summary.queued, 2);
    assert!(
        (40_000..=bound_us).contains(&summary.queue_max_us),
        "{summary:?}"
    );
    // Two samples: the 99th percentile is the slower one, to within a bucket and never above it.
    assert!(
        (40_000..=summary.queue_max_us).contains(&summary.queue_p99_us),
        "{summary:?}"
    );
    // A message that injected nothing is not a sample.
    r.core
        .input(r#"{"v":1,"type":"key","code":"Fn","down":true}"#, received);
    assert_eq!(r.core.summary().queued, 2);
}

#[test]
fn a_slow_injector_call_is_not_the_hosts_queue_delay() {
    // SendInput can take milliseconds for a click (Windows' own handling, not the host's). That is
    // reported as the injector's time, not as the queue's.
    let mut r = rig(u32::MAX);
    r.rec.slow_apply(Duration::from_millis(15));
    let mut took = Duration::ZERO;
    for i in 0..4 {
        took = took.max(r.deliver(&format!(
            r#"{{"v":1,"type":"mouse-button","button":0,"down":{}}}"#,
            i % 2 == 0
        )));
    }
    let summary = r.core.summary();
    assert_eq!(summary.queued, 4);
    assert!(summary.apply_max_us >= 15_000, "{summary:?}");
    // Each delivery took its queue delay plus a call of at least 15 ms: a queue delay that
    // counted the call would leave no room for it, however long the machine held this thread.
    let took_us = took.as_micros() as u64;
    assert!(
        summary.queue_max_us + 15_000 <= took_us,
        "{summary:?} {took_us}"
    );
    // The message was taken off the queue as it arrived.
    assert_eq!(summary.wait_max_us, 0, "{summary:?}");
    assert_eq!(summary.slow_calls, 4);
}

#[test]
fn slow_calls_are_all_counted_but_only_the_first_few_logged() {
    let mut r = rig(u32::MAX);
    r.rec.slow_apply(Duration::from_millis(3));
    for i in 0..20 {
        r.deliver(&format!(
            r#"{{"v":1,"type":"mouse-button","button":0,"down":{}}}"#,
            i % 2 == 0
        ));
    }
    assert_eq!(r.core.summary().slow_calls, 20);
    assert_eq!(r.core.slow_logged, SLOW_LOGGED);
}

#[test]
fn the_kinds_in_a_slow_call_are_listed_once_each() {
    let events = [key_down(1), key_up(1), key_down(2)];
    assert_eq!(event_kinds(&events), ["Keyboard"]);
    assert!(event_kinds(&[]).is_empty());
}

#[test]
fn a_burst_through_the_worker_reaches_the_injector_without_a_timer_in_the_way() {
    let f = Fixture::new();
    let rec = Recorder::default();
    let make = rec.clone();
    let (handle, rx) = channel();
    let worker = std::thread::spawn(move || run(rx, Box::new(move |_| make.make())));
    handle.bind(session(&f, u32::MAX));
    // The worker is up and has its injector before the messages, and idle.
    handle.input(
        br#"{"v":1,"type":"mouse-move","dx":1,"dy":0}"#.to_vec(),
        false,
    );
    wait_for("the first move", || rec.events().len() == 1);
    // A thread that only waits on a channel, woken beside the worker each time: how long this
    // machine takes to wake a thread right now, which a parallel build stretches to milliseconds.
    let (ping, pings) = mpsc::channel::<Instant>();
    let (woke, wakes) = mpsc::channel::<Duration>();
    let control = std::thread::spawn(move || {
        for sent in pings {
            let _ = woke.send(sent.elapsed());
        }
    });
    let mut baseline = Delays::default();
    std::thread::sleep(Duration::from_millis(50));
    // Single messages to an idle worker, spread so they meet every phase of a timer tick.
    for i in 0..20 {
        ping.send(Instant::now()).unwrap();
        baseline.record(wakes.recv().unwrap());
        handle.input(
            format!(r#"{{"v":1,"type":"mouse-move","dx":{},"dy":1}}"#, i % 5 + 1).into_bytes(),
            false,
        );
        wait_for("the message", || rec.events().len() == 2 + i);
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(ping);
    control.join().unwrap();
    handle.closed();
    let summary = worker.join().unwrap().expect("the worker ran");
    assert_eq!(summary.queued, 21);
    // The median, not a tail, and over the machine's own wake-up time: a loaded machine can hold
    // a thread for milliseconds now and then, but a worker that wakes on a timer tick instead of
    // on the channel holds most messages for 8 to 25 ms more than a thread blocked on a channel.
    let limit = baseline.median_us() + 2_000;
    assert!(
        summary.queue_p50_us < limit && summary.wait_p50_us < limit,
        "{summary:?} baseline_p50_us={}",
        baseline.median_us()
    );
}

#[test]
fn the_delay_percentile_follows_the_bulk_not_the_outlier() {
    let mut d = Delays::default();
    assert_eq!(d.p99_us(), 0);
    for _ in 0..990 {
        d.record(Duration::from_micros(100));
    }
    for _ in 0..10 {
        d.record(Duration::from_millis(20));
    }
    assert_eq!(d.max_us, 20_000);
    // 99 % of 1000 samples are the fast ones.
    assert!((100..=128).contains(&d.p99_us()), "{}", d.p99_us());
    d.record(Duration::from_millis(20));
    assert!(d.p99_us() > 1000, "{}", d.p99_us());
    // A delay beyond the last bucket lands in it rather than overflowing.
    d.record(Duration::MAX);
    assert_eq!(d.max_us, u64::MAX);
}

#[test]
fn a_worker_that_panics_still_releases_held_keys_and_buttons() {
    let f = Fixture::new();
    let rec = Recorder::default();
    let make = rec.clone();
    let (handle, rx) = channel();
    let s = session(&f, u32::MAX);
    let worker = std::thread::spawn(move || run(rx, Box::new(move |_| make.make())));
    handle.bind(s.clone());
    handle.input(
        br#"{"v":1,"type":"key","code":"KeyA","down":true}"#.to_vec(),
        false,
    );
    handle.input(
        br#"{"v":1,"type":"mouse-button","button":0,"down":true}"#.to_vec(),
        false,
    );
    wait_for("both held", || rec.events().len() == 2);
    // The injector fails on the next event, with the key and the button still down.
    rec.panic_once();
    handle.input(
        br#"{"v":1,"type":"key","code":"KeyB","down":true}"#.to_vec(),
        false,
    );
    // The thread survives its panic, releases, and ends; the handle is still open.
    worker.join().unwrap();
    let events = rec.events();
    assert!(events.contains(&button(1, false)), "{events:?}");
    assert!(events.contains(&key_up(0x41)), "{events:?}");
    assert!(events.contains(&key_up(0x42)), "{events:?}");
    assert_eq!(rec.sinks(), (1, 0));
    assert!(
        s.launch
            .warnings
            .snapshot()
            .iter()
            .any(|w| w.code == "input_worker")
    );
    drop(handle);
}

#[test]
fn an_injector_that_cannot_be_made_is_loud_and_tried_again_later() {
    let f = Fixture::new();
    let rec = Recorder::default();
    rec.fail_makes(2);
    let make = rec.clone();
    let t0 = Instant::now();
    let mut core = Core::new(session(&f, u32::MAX), Box::new(move |_| make.make()), t0);
    let ms = |n: u64| t0 + Duration::from_millis(n);
    // Made at the start and failed, so the first key finds no injector.
    core.input(r#"{"v":1,"type":"key","code":"KeyA","down":true}"#, ms(100));
    assert!(rec.events().is_empty());
    assert_eq!(core.summary().dropped, 1);
    // Not again before the retry delay; then one more failure, then it works.
    core.input(r#"{"v":1,"type":"key","code":"KeyB","down":true}"#, ms(600));
    assert!(rec.events().is_empty());
    core.input(
        r#"{"v":1,"type":"key","code":"KeyC","down":true}"#,
        ms(1200),
    );
    assert_eq!(rec.events(), vec![key_down(0x43)]);
    assert_eq!(rec.sinks(), (1, 1));
}

#[test]
fn the_worker_thread_buffers_early_frames_and_releases_when_the_transport_closes() {
    let f = Fixture::new();
    let rec = Recorder::default();
    let make = rec.clone();
    let (handle, rx) = channel();
    let worker = std::thread::spawn(move || run(rx, Box::new(move |_| make.make())));
    // A frame that arrives before the session is bound waits for it.
    handle.input(
        br#"{"v":1,"type":"key","code":"KeyA","down":true}"#.to_vec(),
        false,
    );
    handle.bind(session(&f, u32::MAX));
    handle.input(
        br#"{"v":1,"type":"key","code":"KeyB","down":true}"#.to_vec(),
        false,
    );
    // A binary frame is not part of the protocol.
    handle.input(vec![1, 2, 3], true);
    handle.closed();
    worker.join().unwrap();
    assert_eq!(
        rec.events(),
        vec![key_down(0x41), key_down(0x42), key_up(0x41), key_up(0x42)]
    );
    assert_eq!(rec.sinks(), (1, 0));
}

#[test]
fn the_input_channel_closing_releases_and_dropping_every_handle_ends_the_worker() {
    let f = Fixture::new();
    let rec = Recorder::default();
    let make = rec.clone();
    let (handle, rx) = channel();
    let worker = std::thread::spawn(move || run(rx, Box::new(move |_| make.make())));
    handle.bind(session(&f, u32::MAX));
    handle.input(
        br#"{"v":1,"type":"mouse-button","button":0,"down":true}"#.to_vec(),
        false,
    );
    handle.channel_closed("input");
    drop(handle);
    worker.join().unwrap();
    assert_eq!(rec.events(), vec![button(1, true), button(1, false)]);
}

#[test]
fn the_histogram_buckets_are_exact_below_eight_and_a_quarter_octave_above() {
    for (us, bucket) in [(0, 0), (7, 7), (8, 8), (9, 8), (10, 9), (15, 11), (16, 12)] {
        assert_eq!(Delays::bucket(us), bucket, "{us} us");
    }
    assert_eq!(Delays::bucket(u64::MAX), BUCKETS - 1);
    assert_eq!(BUCKETS - 1, 155);
    // Every value is within its bucket's upper edge, which is under 25 % above it.
    let mut x = 0u64;
    while x < 5_000_000 {
        let upper = Delays::upper(Delays::bucket(x));
        assert!(x <= upper, "{x} us above its bucket's edge {upper}");
        assert!(
            (upper as f64) < 1.25 * x as f64 + 1.0,
            "{x} us, edge {upper}"
        );
        x += 1 + x / 97;
    }
}

/// A co-op player's WebRTC input drives its slot's pad and nothing else: its
/// first pad becomes the slot's, a keyboard is dropped and said once, and the
/// slot's rumble reaches the device's pad. A launch whose session the host
/// no longer has gets no sink, never one that reaches the desktop.
#[test]
fn a_co_op_players_input_drives_only_its_slots_pad() {
    use butterpollo_config::config::Config;
    use butterpollo_session::state::App;
    use butterpollo_stream::nucleus::{Admission, Game, fake};
    use butterpollo_win_io::input::PadReport;
    let f = Fixture::new();
    let h = f.host.clone();
    let pc = Arc::new(fake::Pc::installed());
    h.nucleus.use_pc(pc.clone());
    let apps = [App {
        name: "Portal 2".into(),
        cmd: format!("\"{}\"", fake::EXE),
        working_dir: String::new(),
        prep: Vec::new(),
        computed_id: None,
        aliases: Vec::new(),
        extra: Default::default(),
    }];
    let entry = h.nucleus.entries(&apps, "").remove(0);
    let mut launch = f.launch(f.client(u32::MAX), Role::Stream);
    let seat = h
        .nucleus
        .admit(Admission {
            launch: &launch.id,
            client: &launch.client,
            game: Game::Coop(&entry),
            size: (1920, 1080),
            config: &Config::default(),
            warnings: Default::default(),
            release_parked: &|| Ok(()),
            instances: None,
        })
        .unwrap();
    launch.nucleus = Some(seat);
    let session = Session::new(launch, Negotiated::default());
    let t0 = Instant::now();
    let make = h.clone();
    let mut core = Core::new(
        session.clone(),
        Box::new(move |s| CoopSink::create(&make, s)),
        t0,
    );
    let sent = Arc::new(Mutex::new(Vec::<Value>::new()));
    let log = sent.clone();
    core.control = Some(Box::new(move |data| {
        log.lock()
            .unwrap()
            .push(serde_json::from_slice(&data).unwrap());
        true
    }));
    let at = |ms| t0 + Duration::from_millis(ms);
    core.gamepad(
        r#"{"v":2,"type":"gamepad-connected","controllerId":7,"id":"Pad","mapping":"standard","buttons":17,"axes":4,"dualRumble":true,"triggerRumble":true}"#,
        at(0),
    );
    core.gamepad(
        &format!(
            r#"{{"v":2,"type":"gamepad-state","controllerId":7,"seq":1,"buttons":{A},"leftTrigger":0,"rightTrigger":0,"leftStickX":0,"leftStickY":0,"rightStickX":0,"rightStickY":0}}"#
        ),
        at(5),
    );
    for _ in 0..2 {
        core.input(r#"{"v":1,"type":"key","code":"KeyA","down":true}"#, at(6));
    }
    let instance = pc.last();
    let inputs = instance.inputs.lock().unwrap().clone();
    assert_eq!(inputs.len(), 1, "{inputs:?}");
    assert!(
        matches!(inputs[0], (0, Input::Controller { id: 0, active: 0b11, buttons, .. }) if buttons != 0),
        "{inputs:?}"
    );
    let dropped: Vec<_> = session
        .launch
        .warnings
        .snapshot()
        .into_iter()
        .filter(|w| w.code.starts_with("coop_"))
        .map(|w| w.code)
        .collect();
    assert_eq!(dropped, ["coop_keyboard_mouse"]);
    // Slot 0's rumble, and slot 1's, which is not this player's.
    let rumble = |id| PadReport::Feedback {
        id,
        kind: RUMBLE_DATA,
        data: vec![0xff, 0xff, 0x00, 0x80],
    };
    instance
        .reports
        .lock()
        .unwrap()
        .extend([(1, rumble(0)), (0, rumble(0))]);
    assert!(core.poll(at(20)));
    let rumble: Vec<_> = sent
        .lock()
        .unwrap()
        .iter()
        .filter(|m| m["type"] == "rumble")
        .cloned()
        .collect();
    assert_eq!(rumble.len(), 1, "{rumble:?}");
    assert_eq!(rumble[0]["controllerId"], 7);
    assert_eq!(rumble[0]["strongMagnitude"], 1.0);
    // A session the registry no longer has: no sink at all.
    let mut gone = f.launch(f.client(u32::MAX), Role::Stream);
    gone.nucleus = Some(butterpollo_session::session::Seat {
        session: seat.session + 1,
        slot: 0,
    });
    let error = CoopSink::create(&h, &Session::new(gone, Negotiated::default()))
        .err()
        .unwrap();
    assert!(
        format!("{error:#}").contains("the co-op session ended"),
        "{error:#}"
    );
}
