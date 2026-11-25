//! Unit tests for gamepad thread PnP recovery scheduling and dead-handle teardown (#138).

use super::*;
use std::sync::atomic::AtomicUsize;

struct RecoveringPads {
    plugged: bool,
    log: Arc<Mutex<Vec<String>>>,
}
static RECOVERING_FLAG: AtomicBool = AtomicBool::new(false);
static RECOVER_CALLS: AtomicUsize = AtomicUsize::new(0);
static DROP_BEFORE_RECOVER: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
static RECOVERY_TEST_LOCK: Mutex<()> = Mutex::new(());

impl Pads for RecoveringPads {
    fn apply(&mut self, event: &Event) -> Result<()> {
        if let Event::Arrival { id, .. } = event {
            self.plugged = true;
            self.log.lock().unwrap().push(format!("arrival {id}"));
        }
        Ok(())
    }
    fn refresh(&mut self) -> Result<()> {
        Ok(())
    }
    fn feedback(&mut self) -> Vec<(u16, u16, Vec<u8>)> {
        Vec::new()
    }
    fn motion_supported(&self, _id: u16) -> bool {
        false
    }
    fn plugged(&self) -> bool {
        self.plugged
    }
    fn release_all(&mut self) -> Result<()> {
        Ok(())
    }
    fn recovering_driver() -> bool {
        RECOVERING_FLAG.load(Ordering::SeqCst)
    }
}

struct LosingPads {
    lost: bool,
    present: bool,
}
impl Drop for LosingPads {
    fn drop(&mut self) {
        DROP_BEFORE_RECOVER.lock().unwrap().push("drop");
    }
}
impl Pads for LosingPads {
    fn apply(&mut self, _event: &Event) -> Result<()> {
        self.lost = true;
        anyhow::bail!("handle refused")
    }
    fn refresh(&mut self) -> Result<()> {
        Ok(())
    }
    fn feedback(&mut self) -> Vec<(u16, u16, Vec<u8>)> {
        Vec::new()
    }
    fn motion_supported(&self, _id: u16) -> bool {
        false
    }
    fn plugged(&self) -> bool {
        false
    }
    fn release_all(&mut self) -> Result<()> {
        Ok(())
    }
    fn lost(&self) -> bool {
        self.lost
    }
    fn driver_present(&self) -> bool {
        self.present
    }
    fn recover_driver() -> bool {
        DROP_BEFORE_RECOVER.lock().unwrap().push("recover");
        RECOVER_CALLS.fetch_add(1, Ordering::SeqCst);
        true
    }
    fn recovering_driver() -> bool {
        false
    }
}

#[test]
fn fresh_session_retries_automatically_while_pnp_recovery_is_in_progress() {
    let _guard = RECOVERY_TEST_LOCK.lock().unwrap();
    RECOVERING_FLAG.store(true, Ordering::SeqCst);
    let attempts = Arc::new(AtomicUsize::new(0));
    let log = Arc::new(Mutex::new(Vec::new()));
    let warnings = Arc::new(butterpollo_session::session::Warnings::default());
    let poll = Polling {
        every: Duration::from_millis(5),
        fast: false,
        backoff: BACKOFF,
        reopen: Duration::from_millis(15),
    };
    let thread = GamepadThread::spawn_polled(
        {
            let attempts = attempts.clone();
            let log = log.clone();
            move || {
                let n = attempts.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    anyhow::bail!("virtual gamepad unavailable during recovery (0x80070103)");
                }
                RECOVERING_FLAG.store(false, Ordering::SeqCst);
                Ok(RecoveringPads {
                    plugged: false,
                    log: log.clone(),
                })
            }
        },
        warnings.clone(),
        poll,
    )
    .unwrap();

    // Only one Arrival is sent; with recovery active the thread must retry at
    // `poll.reopen` instead of sleeping forever or waiting `poll.backoff`.
    thread.send(Event::Arrival {
        id: 0,
        kind: 2,
        capabilities: 0,
        buttons: 0,
    });
    assert!(
        thread.wait_ready(1, Duration::from_secs(2)),
        "fresh session did not reopen automatically after PnP recovery completed"
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    assert_eq!(*log.lock().unwrap(), vec!["arrival 0".to_owned()]);
    assert!(
        warnings
            .snapshot()
            .into_iter()
            .all(|warning| warning.code != "input_gamepad"),
        "input_gamepad warning should clear once recovery opens the driver"
    );
}

#[test]
fn fresh_session_stops_fast_polling_once_recovery_is_no_longer_active() {
    let _guard = RECOVERY_TEST_LOCK.lock().unwrap();
    RECOVERING_FLAG.store(false, Ordering::SeqCst);
    let attempts = Arc::new(AtomicUsize::new(0));
    let warnings = Arc::new(butterpollo_session::session::Warnings::default());
    let poll = Polling {
        every: Duration::from_millis(5),
        fast: false,
        backoff: BACKOFF,
        reopen: Duration::from_millis(10),
    };
    let thread = GamepadThread::spawn_polled(
        {
            let attempts = attempts.clone();
            move || -> Result<RecoveringPads> {
                attempts.fetch_add(1, Ordering::SeqCst);
                anyhow::bail!("driver missing or exhausted");
            }
        },
        warnings,
        poll,
    )
    .unwrap();

    thread.send(Event::Arrival {
        id: 0,
        kind: 1,
        capabilities: 0,
        buttons: 0,
    });
    let deadline = Instant::now() + Duration::from_secs(1);
    while attempts.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
    }
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        1,
        "must not spin when PnP recovery is not active"
    );
    drop(thread);
}

#[test]
fn forget_lost_drops_refused_handle_before_requesting_pnp_recovery() {
    let _guard = RECOVERY_TEST_LOCK.lock().unwrap();
    DROP_BEFORE_RECOVER.lock().unwrap().clear();
    RECOVER_CALLS.store(0, Ordering::SeqCst);
    let warnings = Arc::new(butterpollo_session::session::Warnings::default());
    let poll = Polling {
        every: Duration::from_millis(5),
        fast: false,
        backoff: BACKOFF,
        reopen: Duration::from_secs(1),
    };
    let opens = Arc::new(AtomicUsize::new(0));
    let thread = GamepadThread::spawn_polled(
        {
            let opens = opens.clone();
            move || {
                if opens.fetch_add(1, Ordering::SeqCst) == 0 {
                    Ok(LosingPads {
                        lost: false,
                        present: false,
                    })
                } else {
                    anyhow::bail!("still restarting")
                }
            }
        },
        warnings,
        poll,
    )
    .unwrap();

    thread.send(Event::Arrival {
        id: 0,
        kind: 2,
        capabilities: 0,
        buttons: 0,
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    while RECOVER_CALLS.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
    }
    drop(thread);
    assert_eq!(
        *DROP_BEFORE_RECOVER.lock().unwrap(),
        vec!["drop", "recover"],
        "the refused handle must close before PnP recovery starts"
    );
}

struct NoMoreItemsOnCreatePads {
    opened: usize,
    lost: bool,
    log: Arc<Mutex<Vec<String>>>,
}
impl Pads for NoMoreItemsOnCreatePads {
    fn apply(&mut self, event: &Event) -> Result<()> {
        if let Event::Arrival { id, .. } = event {
            if self.opened == 1 {
                let err = super::super::gamepad_backend::plug_error(
                    windows::Win32::Foundation::ERROR_NO_MORE_ITEMS.into(),
                );
                self.lost = super::super::gamepad_backend::driver_lost(&err);
                return Err(err);
            }
            self.log
                .lock()
                .unwrap()
                .push(format!("{} arrival {id}", self.opened));
        }
        Ok(())
    }
    fn refresh(&mut self) -> Result<()> {
        Ok(())
    }
    fn feedback(&mut self) -> Vec<(u16, u16, Vec<u8>)> {
        Vec::new()
    }
    fn motion_supported(&self, _id: u16) -> bool {
        false
    }
    fn plugged(&self) -> bool {
        !self.lost && self.opened > 1
    }
    fn release_all(&mut self) -> Result<()> {
        Ok(())
    }
    fn lost(&self) -> bool {
        self.lost
    }
}

#[test]
fn create_controller_0x80070103_reopens_handle_and_recovers_controller() {
    let _guard = RECOVERY_TEST_LOCK.lock().unwrap();
    let opens = Arc::new(AtomicUsize::new(0));
    let log = Arc::new(Mutex::new(Vec::new()));
    let warnings = Arc::new(butterpollo_session::session::Warnings::default());
    let poll = Polling {
        every: Duration::from_millis(5),
        fast: false,
        backoff: [
            Duration::from_millis(25),
            Duration::from_millis(50),
            Duration::from_millis(100),
            Duration::from_millis(200),
        ],
        reopen: Duration::from_millis(10),
    };
    let thread = GamepadThread::spawn_polled(
        {
            let opens = opens.clone();
            let log = log.clone();
            move || {
                let opened = opens.fetch_add(1, Ordering::SeqCst) + 1;
                Ok(NoMoreItemsOnCreatePads {
                    opened,
                    lost: false,
                    log: log.clone(),
                })
            }
        },
        warnings.clone(),
        poll,
    )
    .unwrap();

    thread.send(Event::Arrival {
        id: 0,
        kind: 2,
        capabilities: 0xfb,
        buttons: 0,
    });
    assert!(
        thread.wait_ready(1, Duration::from_secs(2)),
        "0x80070103 on create_controller must close handle, reopen, and replay arrival"
    );
    assert_eq!(opens.load(Ordering::SeqCst), 2);
    assert_eq!(*log.lock().unwrap(), vec!["2 arrival 0".to_owned()]);
    assert!(warnings.snapshot().is_empty());
}
