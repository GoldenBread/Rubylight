//! Tests for the UI Automation focus watcher lifecycle and element IDs.
use super::{element::runtime_id, watcher::passing, *};
use butterpollo_input_policy::text_input;
use std::sync::Mutex;
use windows::{
    Win32::{
        System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
        UI::Accessibility::{CUIAutomation, IUIAutomation},
    },
    core::HRESULT,
};

/// The tests count threads and stuck watchers, which are process-wide.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

fn failed(error: anyhow::Error) {
    panic!("UI Automation did not start: {error:#}");
}

fn wait_until(done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(done(), "timed out");
}

#[test]
fn the_watcher_starts_reports_the_focus_and_stops() {
    let _serial = serial();
    let reports = Arc::new(Mutex::new(0));
    let counted = reports.clone();
    let watch = FocusWatch::start(64, move |_| *counted.lock().unwrap() += 1, failed)
        .expect("the watcher starts");
    // The focus at start is reported right after the thread is up.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while *reports.lock().unwrap() == 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(*reports.lock().unwrap() >= 1);
    // A watcher that did not stop would hang the test.
    watch.stop_and_wait();
}

#[test]
fn a_watcher_stopped_right_after_start_ends() {
    let _serial = serial();
    // Stopped before, while and after the thread gets its message queue.
    for _ in 0..20 {
        let watch = FocusWatch::start(0, |_| {}, failed).expect("the watcher starts");
        let started = std::time::Instant::now();
        watch.stop_and_wait();
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }
    assert_eq!(*LINGERING.lock().unwrap(), 0);
}

/// The client's toggles pause and resume one thread; a paused watcher reports nothing.
#[test]
fn toggling_the_watch_keeps_one_thread() {
    let _serial = serial();
    let reports = Arc::new(Mutex::new(0));
    let counted = reports.clone();
    let spawned = SPAWNED.load(Ordering::SeqCst);
    let watch = FocusWatch::start(0, move |_| *counted.lock().unwrap() += 1, failed)
        .expect("the watcher starts");
    wait_until(|| *reports.lock().unwrap() >= 1);
    let thread = watch.control.state().thread;
    for round in 0..20 {
        watch.set(Watch::Off);
        watch.set(Watch::On {
            max_text: round % 2 * 64,
        });
    }
    watch.set(Watch::Off);
    let paused = *reports.lock().unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        *reports.lock().unwrap(),
        paused,
        "a paused watcher reported"
    );
    watch.set(Watch::On { max_text: 64 });
    wait_until(|| *reports.lock().unwrap() > paused);
    assert_eq!(watch.control.state().thread, thread);
    assert_eq!(SPAWNED.load(Ordering::SeqCst), spawned + 1);
    watch.stop_and_wait();
}

/// The id the client gets is the same for one element read twice, and another for another.
#[test]
fn element_ids_come_from_the_runtime_id() {
    std::thread::spawn(|| {
        // SAFETY: COM is initialized on this thread and every object is dropped before
        // CoUninitialize.
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
            {
                let automation: IUIAutomation =
                    CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).unwrap();
                let root = automation.GetRootElement().unwrap();
                let again = automation.GetRootElement().unwrap();
                let id = runtime_id(&root).expect("the desktop has a runtime id");
                assert!(!id.is_empty());
                assert_eq!(runtime_id(&again), Some(id.clone()));
                let child = automation
                    .CreateTreeWalker(&automation.CreateTrueCondition().unwrap())
                    .unwrap()
                    .GetFirstChildElement(&root);
                if let Ok(child) = child
                    && let Some(child) = runtime_id(&child)
                {
                    assert_ne!(text_input::element_id(&child), text_input::element_id(&id));
                }
            }
            CoUninitialize();
        }
    })
    .join()
    .unwrap();
}

/// A busy app or an element that went away keeps the last report; access denied and
/// anything else is a focus UI Automation cannot read, no text field.
#[test]
fn only_passing_errors_keep_the_last_report() {
    for code in [0x8004_0201_u32, 0x8013_1505, 0x8001_0001, 0x8001_010A] {
        assert!(passing(HRESULT(code as i32)), "{code:#x}");
    }
    for code in [0x8007_0005_u32, 0x8000_4005, 0x8000_4002] {
        assert!(!passing(HRESULT(code as i32)), "{code:#x}");
    }
}

/// A watcher stuck in UI Automation holds up the next one, which fails instead of
/// adding a thread.
#[test]
fn a_watcher_after_a_stuck_one_fails() {
    let _serial = serial();
    *LINGERING.lock().unwrap() += 1;
    let error = Arc::new(Mutex::new(None));
    let noted = error.clone();
    let watch = FocusWatch::start(
        0,
        |_| panic!("reported behind a stuck watcher"),
        move |error| *noted.lock().unwrap() = Some(format!("{error:#}")),
    )
    .expect("the thread starts");
    wait_until(|| error.lock().unwrap().is_some());
    *LINGERING.lock().unwrap() -= 1;
    assert!(
        error
            .lock()
            .unwrap()
            .as_deref()
            .unwrap()
            .contains("still stuck"),
        "{:?}",
        error.lock().unwrap()
    );
    watch.stop_and_wait();
    assert_eq!(*LINGERING.lock().unwrap(), 0);
}
