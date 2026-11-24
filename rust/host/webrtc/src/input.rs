//! Input from a WebRTC client (`docs/webrtc-design.md`): the `gamepad` and `input` DataChannels
//! reach the same injector the GameStream control path drives, and rumble and gamepad mouse mode
//! go back to the client on the `control` channel.
//!
//! One worker thread per session owns everything stateful: the core's gamepad state machine and
//! `input` channel parser, the injector, and the permission check. The transport's hooks run on
//! the media thread and must return quickly, so they only queue a [`Message`]. Nothing is
//! batched or coalesced: a message is injected as soon as the worker wakes for it.
//!
//! The worker sleeps on its queue while the session is idle and wakes on a deadline only for
//! work that has one: the 50 ms pointer tick while a controller is in mouse mode, the
//! injector's own key repeat and delayed release, and the 8 ms feedback poll while a controller
//! is plugged in. Whatever the client still holds when the `input` channel closes, the stream
//! stops or the session is replaced is released first, through core's `HeldInput` tracker.
//!
//! The injector sits behind [`Sink`] so the tests never reach `SendInput` or a virtual pad on
//! the desktop they run on.
//!
//! Guests of a console stream (docs/browser-guests.md) send through the owner's worker: their
//! pads take slots in the owner's table, under ids of their own, so the injector's one active
//! mask covers everyone's and the slot limit holds for all of them. Each guest's input is checked
//! against its own permission here, on top of the owner device's: a view-only guest's input never
//! reaches the injector, whatever its page sends.

use anyhow::{Context, Result};
use butterpollo_input_policy::input::{Input, required_permission};
use butterpollo_session::{
    session::EVENT_PERIOD,
    webrtc::{
        gamepad::{Action, Control, Gamepads},
        input::InputChannel,
    },
};
use butterpollo_stream::{
    state::{Session, Shared},
    stream::{RUMBLE_DATA, RUMBLE_TRIGGER_DATA, feedback_packets},
};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, atomic::Ordering, mpsc},
    time::{Duration, Instant},
};

mod guests;
mod queue;
mod sink;

use guests::Guest;
#[cfg(test)]
use queue::{
    GUEST_BACKLOG, GUEST_ID_BITS, GUEST_IDS, GUEST_QUEUE, GuestMessage, MAX_GUEST_TAG,
    guest_controller,
};
pub use queue::{GuestHandle, InputHandle};
use queue::{Message, guest_of};
pub(crate) use queue::{Queues, channel};
#[cfg(test)]
use sink::BUCKETS;
use sink::{CoopSink, Delays, Make, NativeSink, Reply, controller_event, event_kinds};
pub use sink::{Sink, Summary};

/// How often the injector's display lookup and pointer refresh run while input flows (the
/// GameStream control loop runs them every pass of its 8 ms poll).
const REFRESH: Duration = Duration::from_millis(8);
/// An injector call slower than this is counted, and logged with the kinds of event in it.
const SLOW_APPLY: Duration = Duration::from_millis(2);
/// How many slow calls a session logs; the summary's `slow_calls` counts them all.
const SLOW_LOGGED: u32 = 16;
/// How often the injector is polled for rumble while a controller is plugged in, as the
/// GameStream control loop does.
const FEEDBACK_POLL: Duration = Duration::from_millis(8);
/// How often a worker that holds something looks at whether the session is stopping, in case the
/// transport takes a moment to report it closed.
const STOP_POLL: Duration = Duration::from_millis(100);
/// A failed injector is made again no sooner than this.
const RETRY: Duration = Duration::from_millis(500);

/// The state of one session's input, driven by [`run`] and by the tests with a clock of their own.
struct Core {
    session: Arc<Session>,
    pads: Gamepads,
    channel: InputChannel,
    sink: Option<Box<dyn Sink>>,
    make: Make,
    control: Option<Reply>,
    /// Permissions the device lacked and was warned about.
    denied: u32,
    retry_at: Option<Instant>,
    refresh_at: Instant,
    feedback_at: Instant,
    summary: Summary,
    delays: Delays,
    waits: Delays,
    refreshes: Delays,
    applies: Delays,
    /// When the worker took the frame being handled off its queue.
    dequeued: Option<Instant>,
    slow_logged: u32,
    /// When the frame being handled reached the transport, for the delay it waited in the queue.
    received: Option<Instant>,
    warned_invalid: bool,
    /// The stream's guests by id, whose pads share `pads`.
    guests: BTreeMap<u64, Guest>,
}

impl Core {
    fn new(session: Arc<Session>, make: Make, now: Instant) -> Self {
        let mode = session.stream_mode();
        let mut core = Self {
            channel: InputChannel::new(mode.width, mode.height),
            session,
            pads: Gamepads::new(),
            sink: None,
            make,
            control: None,
            denied: 0,
            retry_at: None,
            refresh_at: now,
            feedback_at: now,
            summary: Summary::default(),
            delays: Delays::default(),
            waits: Delays::default(),
            refreshes: Delays::default(),
            applies: Delays::default(),
            dequeued: None,
            slow_logged: 0,
            received: None,
            warned_invalid: false,
            guests: BTreeMap::new(),
        };
        // Made before input comes, as the GameStream control loop does: making it with the
        // first input held that input up.
        core.ensure_sink(now);
        core
    }

    fn summary(&self) -> Summary {
        Summary {
            unknown_codes: self.channel.unknown_codes(),
            queued: self.delays.count,
            queue_p50_us: self.delays.median_us(),
            queue_max_us: self.delays.max_us,
            queue_p99_us: self.delays.p99_us(),
            wait_p50_us: self.waits.median_us(),
            wait_max_us: self.waits.max_us,
            wait_p99_us: self.waits.p99_us(),
            refresh_max_us: self.refreshes.max_us,
            refresh_p99_us: self.refreshes.p99_us(),
            apply_max_us: self.applies.max_us,
            apply_p99_us: self.applies.p99_us(),
            ..self.summary
        }
    }

    fn ensure_sink(&mut self, now: Instant) -> bool {
        if self.sink.is_some() {
            return true;
        }
        if self.retry_at.is_some_and(|at| now < at) {
            return false;
        }
        match (self.make)(&self.session) {
            Ok(sink) => {
                self.retry_at = None;
                self.sink = Some(sink);
                true
            }
            Err(error) => {
                tracing::warn!(error = %format!("{error:#}"), "WebRTC input: the injector could not be made");
                self.retry_at = Some(now + RETRY);
                false
            }
        }
    }

    /// One `gamepad` channel message received at `now`. A device without the controller
    /// permission gets no slot, no mouse mode and no timers: the state machine never sees it.
    fn gamepad(&mut self, text: &str, now: Instant) {
        self.summary.gamepad_messages += 1;
        if !self.allowed(&controller_event()) {
            self.summary.dropped += 1;
            return;
        }
        self.received = Some(now);
        match self.pads.handle_message(text, now) {
            Ok(outcome) => {
                if !outcome.accepted && outcome.actions.is_empty() {
                    self.summary.gamepad_ignored += 1;
                }
                self.actions(outcome.actions, now);
            }
            Err(error) => self.invalid("gamepad", &error),
        }
        self.received = None;
    }

    /// One `input` channel message received at `now`.
    fn input(&mut self, text: &str, now: Instant) {
        self.summary.input_messages += 1;
        self.received = Some(now);
        match butterpollo_session::webrtc::input::parse(text) {
            Ok(message) => {
                let events = self.channel.translate(message);
                self.dispatch(events, now);
            }
            Err(error) => self.invalid("input", &error),
        }
        self.received = None;
    }

    /// Whether the device may send `event`; says once per kind in the stream's warnings when not.
    fn allowed(&mut self, event: &Input) -> bool {
        let client = &self.session.launch.client;
        let permission = required_permission(event);
        if client.allows(permission) {
            return true;
        }
        if self.denied & permission == 0 {
            self.denied |= permission;
            let (code, message) =
                butterpollo_input_policy::input::denied_warning(event, &client.name);
            self.session.launch.warnings.set(&code, message);
        }
        false
    }

    fn invalid(&mut self, channel: &str, error: &dyn std::fmt::Display) {
        self.summary.invalid += 1;
        // The first is loud; a client that keeps sending them would flood the log.
        if !std::mem::replace(&mut self.warned_invalid, true) {
            tracing::warn!(channel, %error, "WebRTC input: invalid message dropped");
        } else {
            tracing::debug!(channel, %error, "WebRTC input: invalid message dropped");
        }
    }

    /// What the gamepad state machine decided: events for the injector and messages for the client.
    fn actions(&mut self, actions: Vec<Action>, now: Instant) {
        let mut events = Vec::new();
        for action in actions {
            match action {
                Action::Input(input) => events.push(input),
                Action::Control(control) => self.send(&control),
            }
        }
        self.dispatch(events, now);
    }

    fn send(&mut self, control: &Control) {
        let (Control::MouseMode { controller_id, .. } | Control::Rumble { controller_id, .. }) =
            control;
        if let Some((tag, own)) = guest_of(*controller_id) {
            // A guest's pad answers on the guest's channel, under the id its browser knows; a
            // guest that left gets nothing, and nothing goes to the owner instead.
            let Some(send) = self
                .guests
                .values_mut()
                .find(|g| g.tag == tag)
                .and_then(|g| g.control.as_mut())
            else {
                return;
            };
            let mut control = control.clone();
            let (Control::MouseMode { controller_id, .. } | Control::Rumble { controller_id, .. }) =
                &mut control;
            *controller_id = own;
            if matches!(control, Control::Rumble { .. }) {
                self.summary.rumble_sent += 1;
            }
            send(control.to_json().into_bytes());
            return;
        }
        let Some(send) = &mut self.control else {
            tracing::debug!("WebRTC input: no control channel to answer on");
            return;
        };
        if matches!(control, Control::Rumble { .. }) {
            self.summary.rumble_sent += 1;
        }
        send(control.to_json().into_bytes());
    }

    /// Check each event against the device's permissions, as the GameStream control loop does,
    /// remember what it holds, and hand the rest to the injector.
    fn dispatch(&mut self, events: Vec<Input>, now: Instant) {
        let mut allowed = Vec::with_capacity(events.len());
        for event in events {
            if !self.allowed(&event) {
                self.summary.dropped += 1;
                continue;
            }
            self.channel.observe(&event);
            allowed.push(event);
        }
        self.inject(allowed, now);
    }

    fn inject(&mut self, events: Vec<Input>, now: Instant) {
        if events.is_empty() {
            return;
        }
        if !self.ensure_sink(now) {
            self.summary.dropped += events.len() as u64;
            return;
        }
        let session = &self.session;
        let sink = self.sink.as_mut().expect("the injector was just made");
        // The stream's display can be created or renamed after input began.
        let mode = session.stream_mode();
        sink.geometry(&session.output.read().unwrap(), mode.width, mode.height);
        let mut refreshed = Duration::ZERO;
        if now >= self.refresh_at {
            self.refresh_at = now + REFRESH;
            let started = Instant::now();
            if let Err(error) = sink.refresh() {
                session.launch.warnings.event("input_pointer", format!("Pointer refresh failed ({error:#}); pointer updates may be missing. Unlock the desktop or reconnect."), EVENT_PERIOD);
            }
            refreshed = started.elapsed();
        }
        self.summary.injected += events.len() as u64;
        let started = Instant::now();
        for error in sink.apply(&events) {
            session.launch.warnings.event("input_injection", format!("Input injection failed ({error:#}); keyboard, mouse or controller actions may be missing. Unlock the desktop and check Windows input permissions."), EVENT_PERIOD);
        }
        let done = Instant::now();
        if done.saturating_duration_since(started) >= SLOW_APPLY {
            self.summary.slow_calls += 1;
            if self.slow_logged < SLOW_LOGGED {
                self.slow_logged += 1;
                tracing::info!(
                    took_us = done.saturating_duration_since(started).as_micros() as u64,
                    kinds = ?event_kinds(&events),
                    "WebRTC input: a slow injection"
                );
            }
        }
        if let Some(received) = self.received {
            self.delays
                .record(started.saturating_duration_since(received));
            self.refreshes.record(refreshed);
            self.applies.record(done.saturating_duration_since(started));
            if let Some(dequeued) = self.dequeued {
                self.waits
                    .record(dequeued.saturating_duration_since(received));
            }
        }
    }

    /// Time-driven work that is due: the pointer tick of mouse mode, the injector's key repeat
    /// and delayed release, and rumble. False when the session is stopping.
    fn poll(&mut self, now: Instant) -> bool {
        if self.session.stopping() {
            return false;
        }
        let actions = self.pads.tick(now);
        self.actions(actions, now);
        if let Some(sink) = &mut self.sink {
            for error in sink.follow_controllers(&self.session, now) {
                self.session.launch.warnings.event("input_injection", format!("Input injection failed ({error:#}); keyboard, mouse or controller actions may be missing. Unlock the desktop and check Windows input permissions."), EVENT_PERIOD);
            }
        }
        if let Some(sink) = &mut self.sink
            && sink.next_due().is_some_and(|due| now >= due)
            && let Err(error) = sink.due()
        {
            self.session.launch.warnings.event("input_repeat", format!("Input release or repeat failed ({error:#}); held controls may not update. Unlock the desktop or reconnect."), EVENT_PERIOD);
        }
        if self.pads.active_mask() != 0 && now >= self.feedback_at {
            self.feedback_at = now + FEEDBACK_POLL;
            self.feedback();
        }
        true
    }

    /// Rumble the controllers' drivers posted, as `rumble` messages.
    fn feedback(&mut self) {
        let Some(sink) = &mut self.sink else {
            return;
        };
        let mut controls = Vec::new();
        for (slot, kind, data) in sink.feedback() {
            for (kind, payload) in feedback_packets(slot, kind, &data) {
                if !sink.feedback_allowed(kind) {
                    continue;
                }
                let word = |at: usize| u16::from_le_bytes([payload[at], payload[at + 1]]);
                // The payloads are the GameStream ones: marker, slot, low, high; and slot, left,
                // right.
                let control = match (kind, payload.len()) {
                    (RUMBLE_DATA, 10) => self.pads.rumble(slot, word(6), word(8)),
                    (RUMBLE_TRIGGER_DATA, 6) => self.pads.trigger_rumble(slot, word(2), word(4)),
                    _ => None,
                };
                controls.extend(control);
            }
        }
        for control in controls {
            self.send(&control);
        }
    }

    /// How long the worker may sleep: None to wait for the next message.
    fn wait(&self, now: Instant) -> Option<Duration> {
        // An injector, a plugged pad or a held key is something to give up when the session
        // stops, even if the transport is slow to say so.
        let busy = self.sink.is_some()
            || self.pads.active_mask() != 0
            || !self.channel.held().is_empty()
            || self.guests.values().any(|g| !g.channel.held().is_empty());
        let deadlines = [
            self.pads.next_tick(),
            self.sink.as_ref().and_then(|sink| sink.next_due()),
            (self.pads.active_mask() != 0).then_some(self.feedback_at),
            busy.then_some(now + STOP_POLL),
        ];
        deadlines
            .into_iter()
            .flatten()
            .min()
            .map(|at| at.saturating_duration_since(now))
    }

    /// The `input` channel closed: let go of every key and button the client holds.
    fn release_held(&mut self, now: Instant) {
        let events = self.channel.release_all();
        if !events.is_empty() {
            tracing::info!(
                count = events.len(),
                "WebRTC input: releasing what the client held"
            );
            self.inject(events, now);
        }
    }

    /// The `gamepad` channel closed: unplug every controller, which also releases its mouse mode.
    /// The guests' stay.
    fn unplug(&mut self, now: Instant) {
        let ids = self.pads.controller_ids();
        if !ids.iter().any(|&id| guest_of(id).is_some()) {
            let actions = self.pads.close();
            self.actions(actions, now);
            return;
        }
        self.disconnect(ids.into_iter().filter(|&id| guest_of(id).is_none()), now);
    }

    fn disconnect(&mut self, ids: impl IntoIterator<Item = u32>, now: Instant) {
        let mut actions = Vec::new();
        for id in ids {
            actions.extend(self.pads.disconnect(id).actions);
        }
        self.actions(actions, now);
    }

    /// Release everything and report. Safe to call twice.
    fn finish(&mut self, now: Instant) {
        let actions = self.pads.close();
        self.actions(actions, now);
        let guests: Vec<u64> = self.guests.keys().copied().collect();
        for id in guests {
            self.guest_release(id, now);
        }
        self.release_held(now);
        // Drops the injector, which unplugs the virtual pads.
        self.sink = None;
        let summary = self.summary();
        tracing::info!(
            client = %self.session.launch.client.name,
            gamepad_messages = summary.gamepad_messages,
            gamepad_ignored = summary.gamepad_ignored,
            input_messages = summary.input_messages,
            invalid = summary.invalid,
            unknown_codes = summary.unknown_codes,
            injected = summary.injected,
            dropped = summary.dropped,
            rumble_sent = summary.rumble_sent,
            queued = summary.queued,
            queue_p50_us = summary.queue_p50_us,
            queue_max_us = summary.queue_max_us,
            queue_p99_us = summary.queue_p99_us,
            wait_p50_us = summary.wait_p50_us,
            wait_max_us = summary.wait_max_us,
            wait_p99_us = summary.wait_p99_us,
            refresh_max_us = summary.refresh_max_us,
            refresh_p99_us = summary.refresh_p99_us,
            apply_max_us = summary.apply_max_us,
            apply_p99_us = summary.apply_p99_us,
            slow_calls = summary.slow_calls,
            "WebRTC input summary"
        );
    }
}

/// Start the input worker of a session. It idles until [`InputHandle::bind`] gives it the
/// session; anything the client sends before that waits in its queue.
pub fn spawn(h: Shared) -> Result<InputHandle> {
    let (handle, queues) = channel();
    let make: Make = Box::new(move |session| {
        if session.launch.nucleus.is_some() {
            return CoopSink::create(&h, session);
        }
        #[cfg(any(test, feature = "test-support"))]
        if let Some(fixture) = &h.reconnect_fixture {
            let recorder = Recorder::for_fixture(fixture);
            return recorder.make();
        }
        NativeSink::create(&h, session)
    });
    std::thread::Builder::new()
        .name("webrtc-input".into())
        .spawn(move || {
            // The control thread's priority: it injects every input. Test
            // workers must not boost their priority or register a Windows task.
            #[cfg(not(any(test, feature = "test-support")))]
            let _priority = butterpollo_win_graphics::capture::Priority::input();
            run(queues, make)
        })
        .context("starting the WebRTC input worker")?;
    Ok(handle)
}

/// The summary is for the tests; the spawned worker logs it and drops it.
fn run(queues: Queues, make: Make) -> Option<Summary> {
    let mut early = VecDeque::new();
    let mut control = None;
    let session = loop {
        match queues.rx.recv() {
            Ok(Message::Bind(session)) => break session,
            Ok(Message::Control(sender)) => control = Some(sender),
            Ok(Message::Closed) | Err(_) => return None,
            Ok(message) => early.push_back(message),
        }
    };
    let mut core = Core::new(session, make, Instant::now());
    if let Some(sender) = control {
        core.control = Some(Box::new(move |data| sender.send(data, false)));
    }
    // A panic must not leave a key or a button down on the desktop: whatever happened, release
    // what the client held and unplug its pads, and say so loudly.
    let served = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        serve(&mut core, &queues, early)
    }));
    if let Err(panic) = served {
        let what = panic_text(panic.as_ref());
        tracing::error!(panic = %what, "WebRTC input worker panicked; releasing held keys, buttons and controllers");
        core.session.launch.warnings.set(
            "input_worker",
            format!("The input worker failed ({what}); keyboard, mouse and controller input stopped. Reconnect to restore it."),
        );
    }
    let finished =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| core.finish(Instant::now())));
    if finished.is_err() {
        tracing::error!("WebRTC input worker failed again while releasing; dropping the injector");
        core.sink = None;
    }
    Some(core.summary())
}

pub(crate) fn panic_text(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into())
}

/// The worker's loop, until the transport closes, every handle is gone or the session stops.
fn serve(core: &mut Core, queues: &Queues, mut early: VecDeque<Message>) {
    let rx = &queues.rx;
    loop {
        // The owner's messages first; a guest's is taken only while the owner's queue is empty.
        let mut guest = None;
        let message = if let Some(message) = early.pop_front() {
            Some(message)
        } else {
            match rx.try_recv() {
                Ok(message) => Some(message),
                Err(mpsc::TryRecvError::Disconnected) => break,
                Err(mpsc::TryRecvError::Empty) => {
                    guest = queues.guests.pop();
                    if guest.is_some() {
                        None
                    } else {
                        match core.wait(Instant::now()) {
                            None => match rx.recv() {
                                Ok(message) => Some(message),
                                Err(_) => break,
                            },
                            Some(timeout) => match rx.recv_timeout(timeout) {
                                Ok(message) => Some(message),
                                Err(mpsc::RecvTimeoutError::Timeout) => None,
                                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                            },
                        }
                    }
                }
            }
        };
        let now = Instant::now();
        core.dequeued = Some(now);
        if let Some(guest) = guest {
            core.guest(guest, now);
        }
        match message {
            Some(Message::Gamepad(text, received)) => core.gamepad(&text, received),
            Some(Message::Input(text, received)) => core.input(&text, received),
            Some(Message::GamepadClosed) => core.unplug(now),
            Some(Message::InputClosed) => core.release_held(now),
            Some(Message::Control(sender)) => {
                core.control = Some(Box::new(move |data| sender.send(data, false)));
            }
            Some(Message::Closed) => break,
            // Cleared before the queue is read: a guest's message queued after this wakes the
            // worker again.
            Some(Message::Guests) => queues.guests.woken.store(false, Ordering::Release),
            Some(Message::Bind(_)) | None => {}
        }
        if !core.poll(now) {
            break;
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
mod recording;
#[cfg(any(test, feature = "test-support"))]
pub use recording::Recorder;

#[cfg(test)]
mod tests;
