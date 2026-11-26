//! The focused text field for keyboard follows focus (0x553B, Experimental): a thread that
//! owns UI Automation's focus-changed handler and reports each text field the focus moves
//! to, or that it left them. With `max_text` above 0 it also follows the field's text and
//! selection and reads up to that much around the cursor, never in a password field.
//!
//! UI Automation calls the handlers on its own threads. They only wake the watcher, which
//! reads the newest focus itself, so a burst of changes costs one read and nothing here
//! runs on the video or input paths. A focus event that leaves the focus where it was (a
//! tooltip, the host's own window, an app too busy to answer) reports nothing, so the
//! keyboard does not lower and raise again for it.

use anyhow::{Context, Result};
use butterpollo_input_policy::text_input::{Focused, Watch};
use std::sync::{
    Arc, Condvar, Mutex, MutexGuard, PoisonError,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};
use windows::Win32::{
    Foundation::{LPARAM, WPARAM},
    System::{
        Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
        Threading::GetCurrentThreadId,
    },
    UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, MSG, PM_NOREMOVE, PM_REMOVE, PeekMessageW,
        PostThreadMessageW, WM_APP, WM_QUIT,
    },
};

mod element;
mod watcher;

use watcher::Watcher;

/// The focus moved.
const WAKE_FOCUS: u32 = WM_APP + 1;
/// The followed field's text or selection changed.
const WAKE_TEXT: u32 = WM_APP + 2;
/// The client asked for another watch ([`FocusWatch::set`]).
const WAKE_SET: u32 = WM_APP + 3;
/// How long a new watcher waits for a stopped one still stuck in UI Automation.
const LINGER_WAIT: Duration = if cfg!(test) {
    Duration::from_millis(300)
} else {
    Duration::from_secs(5)
};

/// Stopped watchers whose thread has not ended yet, so a hung app costs at most one
/// thread: the next watcher waits for it, then fails.
static LINGERING: Mutex<usize> = Mutex::new(0);
static ENDED: Condvar = Condvar::new();
#[cfg(test)]
static SPAWNED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Watches the focus until dropped, one thread for the watcher's whole life: the client's
/// toggles pause and resume it ([`FocusWatch::set`]). Neither starting, setting nor
/// dropping waits for the watcher's thread: the control loop drives it, and a UI
/// Automation call stuck in a hung app must not hold up input.
pub struct FocusWatch {
    control: Arc<Control>,
    worker: Option<std::thread::JoinHandle<()>>,
}

/// How the watcher is told what to do: its thread id once it has a message queue, the
/// newest watch and whether it stopped, under one lock that also covers each report.
#[derive(Default)]
struct Control {
    state: Mutex<State>,
    /// Whether the thread ended, and whether it was dropped before, under [`LINGERING`].
    ended: AtomicBool,
    detached: AtomicBool,
}

struct State {
    thread: u32,
    stop: bool,
    watch: Watch,
}

impl Default for State {
    fn default() -> Self {
        Self {
            thread: 0,
            stop: false,
            watch: Watch::Off,
        }
    }
}

impl Control {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl FocusWatch {
    /// Calls `report` on the watcher's thread with the focused text field, or `None` once the
    /// focus left text fields, first for the focus at start. `max_text` above 0 also reads up
    /// to that many bytes around the cursor of a field that is not a password, and reports
    /// again when its text or selection changes. `failed` gets why UI Automation did not
    /// start or follow the focus again, on the same thread; `Err` only when there is no
    /// thread.
    pub fn start(
        max_text: usize,
        report: impl Fn(Option<Focused>) + Send + 'static,
        failed: impl Fn(anyhow::Error) + Send + 'static,
    ) -> Result<Self> {
        let control = Arc::new(Control::default());
        control.state().watch = Watch::On { max_text };
        let shared = control.clone();
        let worker = std::thread::Builder::new()
            .name("text-focus".into())
            .spawn(move || {
                let _ended = Ended(&shared);
                if let Err(error) = wait_for_lingering(&shared).and_then(|()| {
                    // SAFETY: on this new thread, which owns the watcher's COM apartment.
                    unsafe { run(&report, &failed, &shared) }
                }) {
                    failed(error);
                }
            })
            .context("the focus watcher thread did not start")?;
        #[cfg(test)]
        SPAWNED.fetch_add(1, Ordering::SeqCst);
        Ok(Self {
            control,
            worker: Some(worker),
        })
    }

    /// Pauses the watcher for [`Watch::Off`], which reports nothing more once this returns,
    /// or resumes it, reading as much text as `On` says, and reports the focus again.
    pub fn set(&self, watch: Watch) {
        let mut state = self.control.state();
        state.watch = watch;
        // The thread reads the watch after it stores its id, under the same lock.
        if state.thread != 0 {
            post(state.thread, WAKE_SET);
        }
    }

    /// Stops the watcher and waits for its thread.
    #[cfg(test)]
    fn stop_and_wait(mut self) {
        let worker = self.worker.take();
        drop(self);
        worker.unwrap().join().unwrap();
    }
}

impl Drop for FocusWatch {
    fn drop(&mut self) {
        {
            // A report in progress finishes first; none comes after.
            let mut state = self.control.state();
            state.stop = true;
            if state.thread != 0 {
                post(state.thread, WM_QUIT);
            }
        }
        let mut lingering = LINGERING.lock().unwrap_or_else(PoisonError::into_inner);
        if !self.control.ended.load(Ordering::SeqCst) {
            self.control.detached.store(true, Ordering::SeqCst);
            *lingering += 1;
        }
        // Detached: it ends once UI Automation returns.
        drop(self.worker.take());
    }
}

/// Marks the watcher's thread ended, also when it panics.
struct Ended<'a>(&'a Control);

impl Drop for Ended<'_> {
    fn drop(&mut self) {
        self.0.state().thread = 0;
        let mut lingering = LINGERING.lock().unwrap_or_else(PoisonError::into_inner);
        self.0.ended.store(true, Ordering::SeqCst);
        if self.0.detached.load(Ordering::SeqCst) {
            *lingering -= 1;
            ENDED.notify_all();
        }
    }
}

/// Waits up to [`LINGER_WAIT`] for stopped watchers to end, so they never pile up behind a
/// hung app.
fn wait_for_lingering(control: &Control) -> Result<()> {
    let deadline = Instant::now() + LINGER_WAIT;
    let mut lingering = LINGERING.lock().unwrap_or_else(PoisonError::into_inner);
    while *lingering > 0 && !control.state().stop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            anyhow::bail!("a focus watcher before this one is still stuck in UI Automation");
        }
        // Woken when one ends; a stop of this one is seen within the step.
        lingering = ENDED
            .wait_timeout(lingering, left.min(Duration::from_millis(50)))
            .unwrap_or_else(PoisonError::into_inner)
            .0;
    }
    Ok(())
}

fn post(thread: u32, message: u32) {
    // SAFETY: PostThreadMessageW only takes a thread id and plain values; a stale id fails.
    unsafe {
        let _ = PostThreadMessageW(thread, message, WPARAM(0), LPARAM(0));
    }
}

unsafe fn run(
    report: &dyn Fn(Option<Focused>),
    failed: &dyn Fn(anyhow::Error),
    control: &Control,
) -> Result<()> {
    // SAFETY: COM is initialized and uninitialized on this thread, every UI Automation object
    // is created, used and released on it before CoUninitialize, and `message` is a live local
    // for every message call.
    unsafe {
        let mut message = MSG::default();
        // The thread needs a message queue before anything posts to it.
        let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
        {
            let mut state = control.state();
            state.thread = GetCurrentThreadId();
            if state.stop {
                return Ok(());
            }
        }
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .context("COM did not start for the focus watcher")?;
        // Checked under the lock Drop and set take: once they return, a watcher that was
        // stopped or paused while it read the focus reports nothing, so it never reports
        // after the one that replaced it or after the client turned it off.
        let report = |focused: Option<Focused>| {
            let state = control.state();
            if !state.stop && state.watch != Watch::Off {
                report(focused);
            }
        };
        let report: &dyn Fn(Option<Focused>) = &report;
        let result = Watcher::new(GetCurrentThreadId()).map(|mut watcher| {
            let watch = control.state().watch;
            if let Err(error) = watcher.apply(watch, report) {
                failed(error);
            }
            while GetMessageW(&mut message, None, 0, 0).0 > 0 {
                let (mut focus, mut text, mut set) = (false, false, false);
                loop {
                    match message.message {
                        WAKE_FOCUS => focus = true,
                        WAKE_TEXT => text = true,
                        WAKE_SET => set = true,
                        // COM's own window in this apartment.
                        _ => {
                            DispatchMessageW(&message);
                        }
                    }
                    if !PeekMessageW(&mut message, None, WAKE_FOCUS, WAKE_SET, PM_REMOVE).as_bool()
                    {
                        break;
                    }
                }
                if set {
                    let watch = control.state().watch;
                    if let Err(error) = watcher.apply(watch, report) {
                        failed(error);
                    }
                } else if focus && watcher.registered {
                    watcher.focus_moved(report);
                } else if text {
                    watcher.text_changed(report);
                }
            }
            watcher.stop();
        });
        CoUninitialize();
        result
    }
}

#[cfg(test)]
mod tests;
