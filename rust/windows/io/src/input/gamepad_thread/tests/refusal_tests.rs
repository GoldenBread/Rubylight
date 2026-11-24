//! Driver handle-refusal reopen, backoff, warning, and setup replay tests (#138).

use super::*;

/// Pads whose driver refuses their handle at a state while `refuse` is set,
/// as the VHF driver did about half a second after a plug following a boot
/// (#138). Each entry in the log starts with the pads' open number.
struct Refusing {
    log: Arc<Mutex<Vec<String>>>,
    refuse: Arc<AtomicBool>,
    opened: usize,
    lost: bool,
}
impl Pads for Refusing {
    fn apply(&mut self, event: &Event) -> Result<()> {
        assert!(!self.lost, "no request after the refusal");
        self.log
            .lock()
            .unwrap()
            .push(format!("{} {event:?}", self.opened));
        if matches!(event, Event::Controller { .. }) && self.refuse.load(Ordering::SeqCst) {
            self.lost = true;
            return Err(super::super::super::gamepad_backend::driver_error(
                windows::core::Error::from_hresult(windows::Win32::Foundation::E_HANDLE),
            ));
        }
        Ok(())
    }
    fn refresh(&mut self) -> Result<()> {
        assert!(!self.lost, "no refresh after the refusal");
        Ok(())
    }
    fn feedback(&mut self) -> Vec<(u16, u16, Vec<u8>)> {
        assert!(!self.lost, "no feedback poll after the refusal");
        Vec::new()
    }
    fn motion_supported(&self, _: u16) -> bool {
        false
    }
    fn plugged(&self) -> bool {
        true
    }
    fn release_all(&mut self) -> Result<()> {
        Ok(())
    }
    fn lost(&self) -> bool {
        self.lost
    }
    /// Each pads' memory names them, so the log shows whose the next took.
    fn memory(&self) -> PadMemory {
        PadMemory {
            steam_at_arrival: [(0, self.opened % 2 == 1)].into(),
            steam_fallbacks: [self.opened as u16].into(),
        }
    }
    fn remember(&mut self, memory: PadMemory) {
        self.log.lock().unwrap().push(format!(
            "{} remembers {:?} {:?}",
            self.opened, memory.steam_at_arrival, memory.steam_fallbacks
        ));
    }
}
impl Drop for Refusing {
    fn drop(&mut self) {
        self.log
            .lock()
            .unwrap()
            .push(format!("{} closed", self.opened));
    }
}
/// The thread over [`Refusing`] pads, its log, refusal switch and warnings.
type RefusingThread = (
    GamepadThread,
    Arc<Mutex<Vec<String>>>,
    Arc<AtomicBool>,
    Arc<butterpollo_session::session::Warnings>,
);
fn refusing(refuse: bool) -> RefusingThread {
    refusing_with(
        refuse,
        Polling {
            every: POLL,
            fast: false,
            retry: RETRY,
            reopen: REOPEN,
        },
    )
}
fn refusing_with(refuse: bool, poll: Polling) -> RefusingThread {
    let log = Arc::new(Mutex::new(Vec::new()));
    let refuse = Arc::new(AtomicBool::new(refuse));
    let warnings = Arc::new(butterpollo_session::session::Warnings::default());
    let mut opened = 0;
    let pads = GamepadThread::spawn_polled(
        {
            let (log, refuse) = (log.clone(), refuse.clone());
            move || {
                opened += 1;
                log.lock().unwrap().push(format!("{opened} opened"));
                Ok(Refusing {
                    log: log.clone(),
                    refuse: refuse.clone(),
                    opened,
                    lost: false,
                })
            }
        },
        warnings.clone(),
        poll,
    )
    .unwrap();
    (pads, log, refuse, warnings)
}
fn logged_line(log: &Mutex<Vec<String>>, line: &str) -> bool {
    log.lock().unwrap().iter().any(|logged| logged == line)
}
fn opens(log: &Mutex<Vec<String>>) -> usize {
    log.lock()
        .unwrap()
        .iter()
        .filter(|logged| logged.ends_with(" opened"))
        .count()
}
const DUALSENSE_ARRIVAL: Event = Event::Arrival {
    id: 0,
    kind: 2,
    capabilities: 0xfb,
    buttons: 0,
};

/// #138: the driver refuses the pads' handle. The pads are closed at once, no
/// request follows on that handle, and after REOPEN the next input opens the
/// driver again, where the controller arrives first as the type it was.
#[test]
fn a_driver_that_refuses_the_handle_is_closed_and_opened_again_with_its_arrivals() {
    let (pads, log, refuse, warnings) = refusing(true);
    let gate = Gate::default();
    pads.send(DUALSENSE_ARRIVAL);
    pads.send(state(0, 1, 1, 0));
    gate.wait_until(|| logged_line(&log, "1 closed"));
    let refused = Instant::now();
    refuse.store(false, Ordering::SeqCst);
    // Input meanwhile waits for the reopen, not a request on a dead handle.
    pads.send(state(0, 1, 2, 0));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(opens(&log), 1, "not opened again before REOPEN");
    std::thread::sleep(REOPEN.saturating_sub(refused.elapsed()) + Duration::from_millis(50));
    pads.send(state(0, 1, 4, 0));
    let last = format!("2 {:?}", state(0, 1, 4, 0));
    gate.wait_until(|| logged_line(&log, &last));
    assert_eq!(
        *log.lock().unwrap(),
        [
            "1 opened".to_string(),
            format!("1 {DUALSENSE_ARRIVAL:?}"),
            format!("1 {:?}", state(0, 1, 1, 0)),
            "1 closed".into(),
            "2 opened".into(),
            // What the refused pads kept (Steam's answer at each arrival,
            // the controllers whose warning is up) goes to the next ones.
            "2 remembers {0: true} {1}".into(),
            format!("2 {DUALSENSE_ARRIVAL:?}"),
            last,
        ]
    );
    // Neither the profile hint nor the driver warning for one refusal.
    assert!(
        warnings.snapshot().is_empty(),
        "{:?}",
        warnings
            .snapshot()
            .iter()
            .map(|warning| &warning.message)
            .collect::<Vec<_>>()
    );
}

/// #138: a driver that keeps refusing the handle is opened again only every
/// RETRY after LOST_LIMIT refusals, and the console names it.
#[test]
fn a_driver_that_keeps_refusing_the_handle_backs_off_and_is_named() {
    let (pads, log, _refuse, warnings) = refusing(true);
    let gate = Gate::default();
    pads.send(DUALSENSE_ARRIVAL);
    for open in 1..=LOST_LIMIT {
        pads.send(state(0, 1, open as u32, 0));
        let closed = format!("{open} closed");
        gate.wait_until(|| logged_line(&log, &closed));
        if open < LOST_LIMIT {
            std::thread::sleep(REOPEN + Duration::from_millis(50));
        }
    }
    gate.wait_until(|| !warnings.snapshot().is_empty());
    let entries = warnings.snapshot();
    assert_eq!(entries.len(), 1, "only the driver warning");
    assert_eq!(entries[0].code, DRIVER_LOST);
    assert!(
        entries[0].message.contains("Rubylight Virtual Gamepad"),
        "{}",
        entries[0].message
    );
    assert!(
        !entries[0].message.contains("profile"),
        "{}",
        entries[0].message
    );
    assert!(
        entries[0]
            .message
            .contains("up to 2 more times, at least 10 seconds apart"),
        "{}",
        entries[0].message
    );
    // Backing off: past REOPEN, input does not open it again.
    std::thread::sleep(REOPEN + Duration::from_millis(100));
    pads.send(state(0, 1, 9, 0));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(opens(&log), LOST_LIMIT);
}

/// #138: the reopen does not wait for input. A controller that arrives
/// while the thread waits to reopen, with nothing after it, is plugged on the
/// pads opened again, and a launch waiting for it sees it.
#[test]
fn pads_the_driver_refused_open_again_without_input_with_every_arrival() {
    let (pads, log, refuse, _warnings) = refusing(true);
    let gate = Gate::default();
    pads.send(DUALSENSE_ARRIVAL);
    pads.send(state(0, 1, 1, 0));
    gate.wait_until(|| logged_line(&log, "1 closed"));
    refuse.store(false, Ordering::SeqCst);
    let second = Event::Arrival {
        id: 1,
        kind: 1,
        capabilities: 0,
        buttons: 0,
    };
    pads.send(second.clone());
    assert!(pads.wait_ready(0b11, REOPEN + Duration::from_secs(5)));
    assert_eq!(
        *log.lock().unwrap(),
        [
            "1 opened".to_string(),
            format!("1 {DUALSENSE_ARRIVAL:?}"),
            format!("1 {:?}", state(0, 1, 1, 0)),
            "1 closed".into(),
            "2 opened".into(),
            "2 remembers {0: true} {1}".into(),
            format!("2 {DUALSENSE_ARRIVAL:?}"),
            format!("2 {second:?}"),
        ]
    );
}

/// #138: from the STOP_LIMIT'th refusal without working pads in between,
/// the thread opens the driver only at input, SLOW_RETRY RETRYs apart, and
/// says so, until the next stream or launch begins.
#[test]
fn a_driver_that_refuses_every_reopen_is_left_alone_until_the_next_stream() {
    let poll = Polling {
        every: POLL,
        fast: false,
        retry: Duration::from_millis(60),
        reopen: Duration::from_millis(20),
    };
    let (pads, log, refuse, warnings) = refusing_with(true, poll);
    let gate = Gate::default();
    pads.send(DUALSENSE_ARRIVAL);
    for open in 1..=STOP_LIMIT {
        // Each reopen comes by itself, with the arrival; a state is refused.
        let opened = format!("{open} opened");
        gate.wait_until(|| logged_line(&log, &opened));
        pads.send(state(0, 1, open as u32, 0));
        let closed = format!("{open} closed");
        gate.wait_until(|| logged_line(&log, &closed));
    }
    gate.wait_until(|| {
        warnings
            .snapshot()
            .iter()
            .any(|warning| warning.message.contains("only when a controller is used"))
    });
    let stopped = Instant::now();
    // Not by itself, nor at input before the slow retry.
    std::thread::sleep(poll.retry * 2);
    pads.send(state(0, 1, 9, 0));
    std::thread::sleep(poll.retry * 2);
    assert_eq!(opens(&log), STOP_LIMIT);
    let entries = warnings.snapshot();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].code, DRIVER_LOST);
    assert!(
        entries[0]
            .message
            .contains("until the next stream or game launch starts"),
        "{}",
        entries[0].message
    );
    // Past the slow retry, input opens it again within the stream: a device
    // restarted meanwhile would play. This one refuses again.
    std::thread::sleep(
        (poll.retry * SLOW_RETRY).saturating_sub(stopped.elapsed()) + Duration::from_millis(30),
    );
    pads.send(state(0, 1, 10, 0));
    let closed = format!("{} closed", STOP_LIMIT + 1);
    gate.wait_until(|| logged_line(&log, &closed));
    // A new stream: the driver opens at its first input, and the warning goes.
    refuse.store(false, Ordering::SeqCst);
    pads.activate();
    pads.send(state(0, 1, 11, 0));
    let last = format!("{} {:?}", STOP_LIMIT + 2, state(0, 1, 11, 0));
    gate.wait_until(|| logged_line(&log, &last));
    assert!(logged_line(
        &log,
        &format!("{} {DUALSENSE_ARRIVAL:?}", STOP_LIMIT + 2)
    ));
    assert!(warnings.snapshot().is_empty());
}

/// #138: a driver whose device is gone after a refusal (0x80070103 at open,
/// as in the report) is not a refusal: it is tried every RETRY, by itself,
/// for as long as it takes, and opens once the device is back.
#[test]
fn a_driver_that_will_not_open_after_a_refusal_is_tried_every_retry_without_stopping() {
    let poll = Polling {
        every: POLL,
        fast: false,
        retry: Duration::from_millis(30),
        reopen: Duration::from_millis(10),
    };
    let log = Arc::new(Mutex::new(Vec::new()));
    let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let device_back = Arc::new(AtomicBool::new(false));
    let warnings = Arc::new(butterpollo_session::session::Warnings::default());
    let pads = GamepadThread::spawn_polled(
        {
            let (log, attempts, device_back) = (log.clone(), attempts.clone(), device_back.clone());
            move || {
                let opened = attempts.fetch_add(1, Ordering::SeqCst) + 1;
                if opened > 1 && !device_back.load(Ordering::SeqCst) {
                    anyhow::bail!("no device interface (0x80070103)");
                }
                log.lock().unwrap().push(format!("{opened} opened"));
                Ok(Refusing {
                    log: log.clone(),
                    refuse: Arc::new(AtomicBool::new(opened == 1)),
                    opened,
                    lost: false,
                })
            }
        },
        warnings.clone(),
        poll,
    )
    .unwrap();
    let gate = Gate::default();
    pads.send(DUALSENSE_ARRIVAL);
    pads.send(state(0, 1, 1, 0));
    gate.wait_until(|| logged_line(&log, "1 closed"));
    gate.wait_until(|| attempts.load(Ordering::SeqCst) > STOP_LIMIT + 3);
    let entries = warnings.snapshot();
    assert!(
        entries
            .iter()
            .any(|warning| warning.code == "input_gamepad"),
        "the open failure is shown"
    );
    assert!(
        !entries.iter().any(|warning| warning.code == DRIVER_LOST),
        "failed opens are no refusals"
    );
    device_back.store(true, Ordering::SeqCst);
    gate.wait_until(|| {
        log.lock().unwrap().iter().any(|line| {
            !line.starts_with("1 ") && line.ends_with(&format!("{DUALSENSE_ARRIVAL:?}"))
        })
    });
}

#[test]
fn refusals_back_off_then_stop_unless_pads_work_a_minute_in_between() {
    let poll = Polling {
        every: POLL,
        fast: false,
        retry: RETRY,
        reopen: REOPEN,
    };
    let mut at = Instant::now();
    let mut losses = Losses::default();
    for n in 1..=STOP_LIMIT {
        losses.opened(at);
        at += Duration::from_secs(5);
        let expected = if n >= STOP_LIMIT {
            RETRY * SLOW_RETRY
        } else if n >= LOST_LIMIT {
            RETRY
        } else {
            REOPEN
        };
        assert_eq!(losses.record(at, &poll), expected, "refusal {n}");
    }
    assert!(losses.stopped() && losses.warned);
    assert_eq!(RETRY * SLOW_RETRY, Duration::from_secs(60));
    // Pads that worked a minute in between start the count again.
    let mut losses = Losses::default();
    for _ in 0..10 {
        losses.opened(at);
        at += LOST_SPAN;
        assert_eq!(losses.record(at, &poll), REOPEN);
    }
    assert!(!losses.warned);
    // The warning goes, once, when reopened pads have worked a minute.
    let mut losses = Losses::default();
    for _ in 0..LOST_LIMIT {
        losses.opened(at);
        at += Duration::from_secs(1);
        losses.record(at, &poll);
    }
    losses.opened(at);
    assert!(!losses.recovered(at + LOST_SPAN - Duration::from_millis(1)));
    assert!(losses.recovered(at + LOST_SPAN));
    assert!(!losses.recovered(at + LOST_SPAN), "once");
    assert!(!losses.stopped());
    // The warning names the driver, says whether its device is there and
    // how many reopens are left.
    let backing_off = lost_warning(true, 2, RETRY);
    assert!(backing_off.contains("Rubylight Virtual Gamepad"));
    assert!(!backing_off.contains("device is gone"));
    assert!(backing_off.contains("up to 2 more times, at least 10 seconds apart"));
    assert!(lost_warning(true, 1, RETRY).contains("up to 1 more time,"));
    assert!(
        backing_off.contains("then only when a controller is used, at most once every 60 seconds")
    );
    let stopped = lost_warning(false, 0, RETRY);
    assert!(stopped.contains("device is gone"));
    assert!(stopped.contains(
        "only when a controller is used, at most once every 60 seconds, until the next stream or game launch starts"
    ));
}

#[test]
fn the_setup_replayed_on_reopened_pads_keeps_only_connected_controllers() {
    let arrival = |id, kind| Event::Arrival {
        id,
        kind,
        capabilities: 0,
        buttons: 0,
    };
    let mut setup = Vec::new();
    keep_setup(&mut setup, &arrival(0, 1));
    keep_setup(&mut setup, &arrival(1, 2));
    keep_setup(&mut setup, &motion(0, 1, 1.));
    keep_setup(&mut setup, &arrival(0, 2));
    assert_eq!(
        setup,
        [arrival(0, 2), arrival(1, 2)],
        "the latest arrival, where the first stood: player numbers stay"
    );
    keep_setup(&mut setup, &state(0, 1, 0, 0));
    assert_eq!(setup, [arrival(0, 2)], "controller 1 left");
    keep_setup(&mut setup, &arrival(20, 1));
    keep_setup(&mut setup, &state(0, 1, 0, 0));
    assert_eq!(setup, [arrival(0, 2)], "no controller beyond 16");
}
