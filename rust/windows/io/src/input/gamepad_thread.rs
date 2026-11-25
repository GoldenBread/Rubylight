//! Drive the virtual gamepads off the input thread. Blocking VHF driver
//! calls must not delay keyboard, mouse, touch or the control stream's
//! acknowledgements.
use super::gamepads::PadMemory;
use super::{Event, Gamepads};
use anyhow::Result;
use butterpollo_input_policy::input::Batch;
use butterpollo_input_policy::steam_feedback::{SteamHaptic, SteamPads};
use butterpollo_input_policy::{controller_audio, haptics};
use std::{
    collections::VecDeque,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicU16, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::{HANDLE, WAIT_FAILED},
    System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects},
};

#[path = "gamepad_thread/losses.rs"]
mod losses;
#[path = "gamepad_thread/state.rs"]
mod state;
#[path = "gamepad_thread/worker.rs"]
mod worker;

#[allow(unused_imports)]
use losses::{Losses, count_refusal, forget_lost, lost_warning, open_driver, reopen_at};
use state::{AudioQueue, Polling, Queue, Stats, enqueue};
#[cfg(test)]
use worker::keep_setup;
use worker::run;

/// How often feedback is polled and the back button emulated while a pad is
/// plugged in, as the control loop and the C++ host's feedback thread did.
const POLL: Duration = Duration::from_millis(8);
/// Graduated retry backoff for driver handle refusals and failed opens (#138):
/// 2 s -> 5 s -> 15 s -> 60 s instead of jumping straight to a 60 s lockout.
/// From the LOST_LIMIT'th refusal without pads that worked for LOST_SPAN in
/// between, the console warns; from the STOP_LIMIT'th, it opens only at input
/// (at most once per 60 s) until a controller freshly arrives or the next
/// stream or launch begins.
const BACKOFF: [Duration; 4] = [
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(15),
    Duration::from_secs(60),
];
/// Fast poll interval while bounded PnP recovery is actively restarting the driver.
const REOPEN: Duration = Duration::from_secs(1);
const LOST_LIMIT: usize = 3;
const STOP_LIMIT: usize = 4;
const LOST_SPAN: Duration = Duration::from_secs(60);
/// The warning for a driver that keeps refusing the pads' handle.
const DRIVER_LOST: &str = "input_gamepad_driver";
/// Events waiting for a stalled driver beyond this many are dropped.
const QUEUE_LIMIT: usize = 1024;
/// Moonlight's rumble packet, which synthesized Steam Controller rumble uses.
const RUMBLE_DATA: u16 = 0x010b;
/// With `gamepad_feedback_fast_poll`, feedback is polled this often for
/// FAST_SPAN after a driver had some.
const FAST_POLL: Duration = Duration::from_millis(2);
const FAST_SPAN: Duration = Duration::from_millis(250);
/// How often the `gamepad timings` line is logged while pads are plugged.
const STATS_EVERY: Duration = Duration::from_secs(5);

/// What the gamepad thread hands back to the control stream.
#[derive(Debug, PartialEq)]
pub enum PadReport {
    Haptics(haptics::Frame),
    Speaker(haptics::Frame),
    /// New rumble, lights or trigger effects for a controller.
    Feedback {
        id: u16,
        kind: u16,
        data: Vec<u8>,
    },
    /// A controller arrived on a profile with motion sensors; the client's
    /// arrival capabilities say which sensors to ask for.
    Motion {
        id: u8,
        capabilities: u16,
    },
}

/// The pads the thread drives: VHF, or a stand-in in tests.
pub(super) trait Pads {
    fn warnings(&mut self, _warnings: Arc<butterpollo_session::session::Warnings>) {}
    fn apply(&mut self, event: &Event) -> Result<()>;
    fn refresh(&mut self) -> Result<()>;
    /// Controllers whose client should be asked for motion again, with their
    /// arrival capabilities.
    fn motion_requests(&mut self) -> Vec<(u8, u16)> {
        Vec::new()
    }
    fn feedback(&mut self) -> Vec<(u16, u16, Vec<u8>)>;
    /// Feedback reports read from the driver since the last call, coalesced
    /// or not.
    fn feedback_drained(&mut self) -> usize {
        0
    }
    /// Events a driver signals when feedback is waiting (ViGEmBus's
    /// notification requests): the thread waits on them beside its queue
    /// and polls at once instead of at the next tick.
    fn wait_handles(&self, _handles: &mut Vec<HANDLE>) {}
    /// Whether one of [`Pads::wait_handles`]' requests has completed and
    /// waits for the next feedback poll.
    fn feedback_ready(&self) -> bool {
        false
    }
    fn haptics(&mut self) -> Vec<haptics::Frame> {
        vec![]
    }
    fn speaker(&mut self) -> Vec<haptics::Frame> {
        vec![]
    }
    /// Haptic reports Steam wrote to a virtual Steam Controller whose client
    /// has no real pads, for synthesized rumble.
    fn steam_haptics(&mut self) -> Vec<SteamHaptic> {
        vec![]
    }
    fn audio(&mut self, _id: u16, _kind: u16, _payload: &[u8]) {}
    /// Controllers with an attached USB microphone endpoint, excluding VHF fallback.
    fn audio_ready(&self) -> u16 {
        0
    }
    /// Controllers whose pad is a Steam Controller, a bit each: its back grips are its own.
    fn steam_pads(&self) -> u16 {
        0
    }
    fn motion_supported(&self, id: u16) -> bool;
    /// Whether any pad is plugged in, and so feedback needs polling.
    fn plugged(&self) -> bool;
    /// Registers a wake callback invoked when background pad attachment finishes.
    fn set_wake(&mut self, _wake: Arc<dyn Fn() + Send + Sync>) {}
    /// Whether any completed background pad attachment is ready to finish.
    fn attach_ready(&self) -> bool {
        false
    }
    fn release_all(&mut self) -> Result<()>;
    /// Whether the driver refused these pads' handle: the thread closes them
    /// and opens the driver again (#138).
    fn lost(&self) -> bool {
        false
    }
    /// Whether the driver's device is still present, for the log line when
    /// it refused the handle.
    fn driver_present(&self) -> bool {
        true
    }
    /// Starts bounded PnP recovery once a refused handle is closed (#138).
    fn recover_driver() -> bool {
        false
    }
    /// Whether bounded PnP recovery is running or settling.
    fn recovering_driver() -> bool {
        false
    }
    /// What the pads opened after these must keep.
    fn memory(&self) -> PadMemory {
        PadMemory::default()
    }
    /// Takes over what the pads before these kept.
    fn remember(&mut self, _memory: PadMemory) {}
}
impl Pads for Gamepads {
    fn warnings(&mut self, warnings: Arc<butterpollo_session::session::Warnings>) {
        self.set_warnings(warnings);
    }
    fn apply(&mut self, event: &Event) -> Result<()> {
        let applied = Gamepads::apply(self, event);
        self.follow_driver_host();
        self.note(applied)
    }
    fn refresh(&mut self) -> Result<()> {
        self.check_driver_host();
        let refreshed = Gamepads::refresh(self);
        self.note(refreshed)
    }
    fn motion_requests(&mut self) -> Vec<(u8, u16)> {
        Gamepads::motion_requests(self)
    }
    fn feedback(&mut self) -> Vec<(u16, u16, Vec<u8>)> {
        Gamepads::feedback(self)
    }
    fn feedback_drained(&mut self) -> usize {
        self.take_feedback_drained()
    }
    fn wait_handles(&self, handles: &mut Vec<HANDLE>) {
        Gamepads::wait_handles(self, handles);
    }
    fn feedback_ready(&self) -> bool {
        Gamepads::feedback_ready(self)
    }
    fn haptics(&mut self) -> Vec<haptics::Frame> {
        Gamepads::haptics(self)
    }
    fn speaker(&mut self) -> Vec<haptics::Frame> {
        Gamepads::speaker(self)
    }
    fn steam_haptics(&mut self) -> Vec<SteamHaptic> {
        self.take_steam_haptics()
    }
    fn audio(&mut self, id: u16, kind: u16, payload: &[u8]) {
        Gamepads::audio(self, id, kind, payload);
    }
    fn audio_ready(&self) -> u16 {
        Gamepads::audio_ready(self)
    }
    fn steam_pads(&self) -> u16 {
        Gamepads::steam_pads(self)
    }
    fn motion_supported(&self, id: u16) -> bool {
        Gamepads::motion_supported(self, id)
    }
    fn plugged(&self) -> bool {
        !self.active.is_empty()
            || !self.decks.is_empty()
            || !self.dualsenses.is_empty()
            || !self.switch2s.is_empty()
    }
    fn set_wake(&mut self, wake: Arc<dyn Fn() + Send + Sync>) {
        Gamepads::set_wake(self, wake);
    }
    fn attach_ready(&self) -> bool {
        Gamepads::attach_ready(self)
    }
    fn release_all(&mut self) -> Result<()> {
        let released = Gamepads::release_all(self);
        self.note(released)
    }
    fn lost(&self) -> bool {
        Gamepads::lost(self)
    }
    fn driver_present(&self) -> bool {
        Gamepads::driver_present(self)
    }
    fn recover_driver() -> bool {
        super::gamepad_backend::recover_driver()
    }
    fn recovering_driver() -> bool {
        super::gamepad_backend::recovery_in_progress()
    }
    fn memory(&self) -> PadMemory {
        Gamepads::memory(self)
    }
    fn remember(&mut self, memory: PadMemory) {
        Gamepads::remember(self, memory);
    }
}

struct Shared {
    warnings: Mutex<Arc<butterpollo_session::session::Warnings>>,
    queue: Mutex<Queue>,
    /// Signalled with `wake` for the gamepad thread, which waits on it and
    /// the drivers' feedback events together; `wake` stays for
    /// [`GamepadThread::wait_ready`].
    wake_event: OwnedHandle,
    feedback_waker: Mutex<Option<Arc<std::net::UdpSocket>>>,
    haptics: Mutex<haptics::Queue>,
    speaker: Mutex<haptics::Queue>,
    audio_input: Mutex<AudioQueue>,
    wake: Condvar,
    stop: AtomicBool,
    parked: AtomicBool,
    /// The client's Haptics setting, which Steam Controller rumble follows.
    haptics_on: AtomicBool,
    ready: AtomicU16,
    audio_ready: AtomicU16,
    /// [`Pads::steam_pads`] as the thread last saw it, for the back grip hint.
    steam_pads: AtomicU16,
    /// A stream or launch began: the driver's refusals count from none again,
    /// and one opened only seldom is opened at the next input (#138).
    fresh: AtomicBool,
}
impl Shared {
    /// Wake the gamepad thread and anyone waiting for its pads.
    fn wake(&self) {
        self.wake.notify_all();
        // SAFETY: `wake_event` is an event handle Shared owns.
        if let Err(error) = unsafe { SetEvent(HANDLE(self.wake_event.as_raw_handle())) } {
            tracing::error!(%error, "could not wake the virtual controllers' thread");
        }
    }
}

/// One client's virtual gamepads, driven on their own thread. The input
/// thread only queues events and collects reports. Dropping it waits for
/// the thread, which unplugs the pads and frees their slots first, so a
/// later session never sees a stale pad; one [`GamepadThread::stop_within`]
/// gave up on is left running instead.
pub struct GamepadThread {
    shared: Arc<Shared>,
    reports: Mutex<mpsc::Receiver<PadReport>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    /// [`GamepadThread::stop_within`] timed out: the thread is stuck in the
    /// driver and nothing waits for it again.
    abandoned: AtomicBool,
    /// The back grips no setting maps; pressing one the controller's pad lacks shows a hint.
    unmapped_grips: u32,
    /// The `gamepad` profile and the controllers' identities, for the grips each pad has
    /// ([`butterpollo_input_policy::input_policy::pad_back_grips`]).
    profile: u16,
    identities: Mutex<std::collections::BTreeMap<u16, (u16, u16)>>,
}
impl GamepadThread {
    /// The driver is opened with the first controller event, on the thread.
    pub fn new(
        profile: u16,
        policy: butterpollo_input_policy::input_policy::Policy,
        warnings: Arc<butterpollo_session::session::Warnings>,
    ) -> std::io::Result<Self> {
        let unmapped_grips = policy.unmapped_back_grips();
        let pad_warnings = warnings.clone();
        let polling = Polling {
            every: POLL,
            fast: policy.feedback_fast_poll,
            backoff: BACKOFF,
            reopen: REOPEN,
        };
        let mut thread = Self::spawn_polled(
            move || Gamepads::open_options(profile, policy.clone(), pad_warnings.clone()),
            warnings,
            polling,
        )?;
        thread.unmapped_grips = unmapped_grips;
        thread.profile = profile;
        Ok(thread)
    }
    /// Test pads, polled as the VHF pads are by default.
    #[cfg(test)]
    fn spawn_reported<P: Pads + 'static>(
        open: impl FnMut() -> Result<P> + Send + 'static,
        warnings: Arc<butterpollo_session::session::Warnings>,
    ) -> std::io::Result<Self> {
        Self::spawn_polled(
            open,
            warnings,
            Polling {
                every: POLL,
                fast: false,
                backoff: BACKOFF,
                reopen: REOPEN,
            },
        )
    }
    /// The thread with feedback polled as `poll` says; tests stretch it.
    fn spawn_polled<P: Pads + 'static>(
        open: impl FnMut() -> Result<P> + Send + 'static,
        warnings: Arc<butterpollo_session::session::Warnings>,
        poll: Polling,
    ) -> std::io::Result<Self> {
        // SAFETY: CreateEventW returned a new auto-reset event, which only
        // this OwnedHandle owns.
        let wake_event = unsafe {
            OwnedHandle::from_raw_handle(
                CreateEventW(None, false, false, None)
                    .map_err(std::io::Error::other)?
                    .0,
            )
        };
        let shared = Arc::new(Shared {
            warnings: Mutex::new(warnings),
            queue: Mutex::default(),
            wake_event,
            feedback_waker: Mutex::default(),
            haptics: Mutex::default(),
            speaker: Mutex::default(),
            audio_input: Mutex::default(),
            wake: Condvar::new(),
            stop: AtomicBool::new(false),
            parked: AtomicBool::new(false),
            haptics_on: AtomicBool::new(true),
            ready: AtomicU16::new(0),
            audio_ready: AtomicU16::new(0),
            steam_pads: AtomicU16::new(0),
            fresh: AtomicBool::new(false),
        });
        let (sender, reports) = mpsc::channel();
        let thread = std::thread::Builder::new().name("gamepads".into()).spawn({
            let shared = shared.clone();
            move || run(&shared, open, &sender, poll)
        })?;
        Ok(Self {
            shared,
            reports: Mutex::new(reports),
            thread: Mutex::new(Some(thread)),
            abandoned: AtomicBool::new(false),
            unmapped_grips: 0,
            profile: 0,
            identities: Mutex::default(),
        })
    }
    /// The back grips controller `id`'s pad has itself: every one on a Steam Controller,
    /// whether `gamepad` or Automatic chose it.
    fn pad_back_grips(&self, id: u16) -> u32 {
        use butterpollo_input_policy::input_policy::{VHF_STEAM_CONTROLLER, pad_back_grips};
        let steam = id < 16 && self.shared.steam_pads.load(Ordering::Acquire) & (1 << id) != 0;
        pad_back_grips(
            if steam {
                VHF_STEAM_CONTROLLER
            } else {
                self.profile
            },
            self.identities.lock().unwrap().get(&id).copied(),
        )
    }
    /// Queue an event for the pads; never waits for the driver.
    pub fn set_warnings(&self, warnings: Arc<butterpollo_session::session::Warnings>) {
        *self.shared.warnings.lock().unwrap() = warnings;
    }
    pub fn send(&self, event: Event) {
        if let Event::Arrival { id, .. } = &event
            && *id < 16
        {
            self.shared.ready.fetch_and(!(1 << *id), Ordering::Release);
        }
        if let Event::ControllerIdentity(identity) = &event {
            self.identities.lock().unwrap().insert(
                u16::from(identity.controller),
                (identity.vendor, identity.product),
            );
        }
        if let Event::Controller { id, buttons, .. } = &event
            && buttons & self.unmapped_grips != 0
            && buttons & self.unmapped_grips & !self.pad_back_grips(*id) != 0
        {
            self.shared.warnings.lock().unwrap().event(
                "input_back_grips",
                butterpollo_input_policy::input_policy::BACK_GRIP_HINT,
                butterpollo_session::session::EVENT_PERIOD,
            );
        }
        let mut queue = self.shared.queue.lock().unwrap();
        if queue.oldest.is_none() {
            queue.oldest = Some(Instant::now());
        }
        if !enqueue(&mut queue.events, event) && !std::mem::replace(&mut queue.full, true) {
            self.shared.warnings.lock().unwrap().event("input_gamepad_queue", "Virtual gamepad driver is not keeping up; controller input was dropped. Check the virtual gamepad driver or choose another supported profile.", butterpollo_session::session::EVENT_PERIOD);
        }
        drop(queue);
        self.shared.wake();
    }
    /// Feedback and motion requests the thread has produced since the last call.
    pub fn reports(&self) -> impl Iterator<Item = PadReport> + '_ {
        let pcm: Vec<_> = self
            .shared
            .haptics
            .lock()
            .unwrap()
            .drain(Instant::now())
            .collect();
        let reports: Vec<_> = self.reports.lock().unwrap().try_iter().collect();
        let speaker: Vec<_> = self
            .shared
            .speaker
            .lock()
            .unwrap()
            .drain(Instant::now())
            .collect();
        reports
            .into_iter()
            .chain(pcm.into_iter().map(PadReport::Haptics))
            .chain(speaker.into_iter().map(PadReport::Speaker))
    }
    pub(super) fn set_feedback_waker(&self, waker: Arc<std::net::UdpSocket>) {
        *self.shared.feedback_waker.lock().unwrap() = Some(waker);
    }
    pub(super) fn release_all(&self) {
        let mut queue = self.shared.queue.lock().unwrap();
        queue.events.clear();
        queue.oldest = None;
        queue.release = true;
        self.shared.haptics.lock().unwrap().retain_controllers(0);
        self.shared.speaker.lock().unwrap().retain_controllers(0);
        self.shared.audio_input.lock().unwrap().release_pcm();
        drop(queue);
        self.shared.wake();
    }
    /// Follow the client's Haptics setting: turning it off stops Steam
    /// Controller rumble and drops its pending pulses at the next poll.
    pub(super) fn set_haptics(&self, enabled: bool) {
        self.shared.haptics_on.store(enabled, Ordering::Release);
        drop(self.shared.queue.lock());
        self.shared.wake();
    }
    /// Release controls immediately while keeping the device identity for reconnect.
    pub fn audio(&self, id: u16, kind: u16, payload: &[u8]) {
        let mut queue = self.shared.audio_input.lock().unwrap();
        if !self.shared.parked.load(Ordering::Acquire) {
            queue.push(id, kind, payload, Instant::now());
        }
    }
    pub fn controller_audio_ready(&self, id: u16) -> bool {
        id < 16
            && !self.shared.parked.load(Ordering::Acquire)
            && !self.shared.stop.load(Ordering::Acquire)
            && self.shared.audio_ready.load(Ordering::Acquire) & (1 << id) != 0
    }
    pub fn park(&self) {
        self.shared.parked.store(true, Ordering::Release);
        self.shared.audio_input.lock().unwrap().clear();
        *self.shared.feedback_waker.lock().unwrap() = None;
        self.release_all();
        self.reports.lock().unwrap().try_iter().for_each(drop);
    }
    pub fn activate(&self) {
        self.reports.lock().unwrap().try_iter().for_each(drop);
        self.shared.haptics.lock().unwrap().retain_controllers(0);
        self.shared.speaker.lock().unwrap().retain_controllers(0);
        self.shared.audio_input.lock().unwrap().clear();
        // A new client starts with haptics on, as its injector does.
        self.shared.haptics_on.store(true, Ordering::Release);
        self.shared.fresh.store(true, Ordering::Release);
        self.shared.parked.store(false, Ordering::Release);
    }
    /// Only the blocking HTTPS launch worker waits here, never the control loop.
    pub fn wait_ready(&self, mask: u16, timeout: Duration) -> bool {
        let end = Instant::now() + timeout;
        let mut queue = self.shared.queue.lock().unwrap();
        while self.shared.ready.load(Ordering::Acquire) & mask != mask {
            let now = Instant::now();
            if now >= end || self.shared.stop.load(Ordering::Acquire) {
                return false;
            }
            queue = self.shared.wake.wait_timeout(queue, end - now).unwrap().0;
        }
        true
    }
    /// How many of its pads are plugged in.
    pub fn plugged_pads(&self) -> u32 {
        self.shared.ready.load(Ordering::Acquire).count_ones()
    }
    /// Tell the thread to end, without waiting: several stopped at once
    /// unplug at once.
    pub fn request_stop(&self) {
        self.shared.stop.store(true, Ordering::Release);
        // Taking the lock orders the flag before a wait that checked it.
        drop(self.shared.queue.lock());
        self.shared.wake();
    }
    pub fn stop(&self) -> Option<JoinHandle<()>> {
        self.request_stop();
        self.thread.lock().unwrap().take()
    }
    /// Stop the thread and wait up to `timeout` for it to end, which unplugs
    /// its pads. A thread stuck in the driver is an error; once it timed out
    /// it is never waited for again: a later call fails at once while it
    /// still runs, and is Ok once it ended. Dropping it then doesn't wait.
    pub fn stop_within(&self, timeout: Duration) -> Result<()> {
        self.request_stop();
        let deadline = match self.abandoned.load(Ordering::Acquire) {
            true => Instant::now(),
            false => Instant::now() + timeout,
        };
        loop {
            // The handle stays put while it runs, so callers at the same
            // time all wait for it and all see how it ends.
            let mut slot = self.thread.lock().unwrap();
            let Some(thread) = slot.as_ref() else {
                return Ok(());
            };
            if thread.is_finished() {
                let thread = slot.take().expect("checked above");
                drop(slot);
                if thread.join().is_err() {
                    // Unwinding dropped the pads too.
                    tracing::warn!("the virtual controllers' thread panicked");
                }
                return Ok(());
            }
            if Instant::now() >= deadline {
                self.abandoned.store(true, Ordering::Release);
                anyhow::bail!(
                    "the virtual controllers did not unplug: the virtual gamepad driver isn't responding, so restart Rubylight and start again"
                );
            }
            drop(slot);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for GamepadThread {
    fn drop(&mut self) {
        if let Some(thread) = self.stop() {
            if self.abandoned.load(Ordering::Acquire) && !thread.is_finished() {
                tracing::warn!(
                    "the virtual controllers' thread is stuck in the driver; left running"
                );
                return;
            }
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
#[path = "gamepad_thread/recovery_tests.rs"]
mod recovery_tests;
#[cfg(test)]
#[path = "gamepad_thread/tests.rs"]
pub(crate) mod tests;
