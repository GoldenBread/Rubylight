//! Input sink trait, native/co-op injector implementations, summary and delay histogram.

use anyhow::{Context, Result};
use butterpollo_input_policy::input::Input;
use butterpollo_stream::{
    controller_sessions,
    nucleus::Player,
    state::{Session, Shared},
    stream::{PadsWait, attach_pads, controllers_back, controllers_wait, effective_config},
};
use butterpollo_win_io::input::{Injector, PadReport};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

/// An event of the kind a gamepad message produces, to ask what permission it needs.
pub(super) fn controller_event() -> Input {
    Input::Controller {
        id: 0,
        active: 0,
        buttons: 0,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    }
}

/// Where input events go, which is the injector except in tests.
pub trait Sink {
    /// Inject in order; returns why events failed.
    fn apply(&mut self, events: &[Input]) -> Vec<anyhow::Error>;
    /// When key repeat or a delayed release is next due.
    fn next_due(&self) -> Option<Instant>;
    fn due(&mut self) -> Result<()>;
    fn refresh(&mut self) -> Result<()>;
    /// The stream's display and size, which absolute input follows.
    fn geometry(&mut self, output: &str, width: u32, height: u32);
    /// New rumble, lights and trigger effects the controllers reported: (slot, kind, data).
    fn feedback(&mut self) -> Vec<(u16, u16, Vec<u8>)>;
    fn feedback_allowed(&self, kind: u16) -> bool;
    /// Controllers the sink was made without, tried for again when it is time: the input
    /// they missed, injected once they are there, and why some failed.
    fn follow_controllers(&mut self, _session: &Session, _now: Instant) -> Vec<anyhow::Error> {
        Vec::new()
    }
}

/// The Windows injector, made the way the GameStream control loop makes its own: without the
/// controllers while they cannot be had, which it then tries for again ([`PadsWait`]).
pub(super) struct NativeSink {
    injector: Injector,
    h: Shared,
    pads: PadsWait,
}

impl NativeSink {
    pub(super) fn create(h: &Shared, s: &Session) -> Result<Box<dyn Sink>> {
        let warnings = &s.launch.warnings;
        let c = effective_config(h, &s.launch).inspect_err(|error| {
            tracing::warn!(error = %format!("{error:#}"), "input configuration unavailable");
        })?;
        let output = s.output.read().unwrap().clone();
        // Keys, clicks and moves need no controller.
        let pads = controller_sessions::acquire_for(h, &s.launch, &c)
            .inspect_err(|error| controllers_wait(s, error))
            .ok();
        let mut injector = Injector::new_options_reported(
            if output.is_empty() {
                c.get("output_name", "")
            } else {
                &output
            },
            c.get("gamepad", "auto"),
            &c,
            warnings.clone(),
        )
        .inspect_err(|error| {
            warnings.set(
                "input_initialization",
                format!("Input initialization failed ({error:#}); controls are unavailable. Check Input settings and the selected display, then reconnect."),
            );
        })?;
        let mut pads_wait = PadsWait::default();
        match pads {
            Some(pads) => {
                injector.use_gamepads(pads);
                warnings.clear("input_initialization");
            }
            None => pads_wait.failed(Instant::now()),
        }
        let mode = s.stream_mode();
        injector.set_stream_size(mode.width, mode.height);
        Ok(Box::new(Self {
            injector,
            h: h.clone(),
            pads: pads_wait,
        }))
    }
}

impl Sink for NativeSink {
    fn apply(&mut self, events: &[Input]) -> Vec<anyhow::Error> {
        if self.pads.retry_at().is_none() {
            return self.injector.apply_all(events);
        }
        let mut events = events.to_vec();
        self.pads.hold(&mut events);
        self.injector.apply_all(&events)
    }
    fn next_due(&self) -> Option<Instant> {
        [self.injector.next_due(), self.pads.retry_at()]
            .into_iter()
            .flatten()
            .min()
    }
    fn due(&mut self) -> Result<()> {
        self.injector.due()
    }
    fn follow_controllers(&mut self, session: &Session, now: Instant) -> Vec<anyhow::Error> {
        let Self { injector, h, pads } = self;
        let Some(held) = pads.retry(now, || {
            let acquired = effective_config(h, &session.launch)
                .and_then(|c| controller_sessions::acquire_for(h, &session.launch, &c))?;
            attach_pads(injector, acquired, &[0; 16]);
            Ok(())
        }) else {
            return Vec::new();
        };
        controllers_back(session);
        injector.apply_all(&held)
    }
    fn refresh(&mut self) -> Result<()> {
        self.injector.refresh()
    }
    fn geometry(&mut self, output: &str, width: u32, height: u32) {
        self.injector.set_output(output);
        self.injector.set_stream_size(width, height);
    }
    fn feedback(&mut self) -> Vec<(u16, u16, Vec<u8>)> {
        self.injector
            .gamepad_reports()
            .filter_map(|report| match report {
                PadReport::Feedback { id, kind, data } => Some((id, kind, data)),
                _ => None,
            })
            .collect()
    }
    fn feedback_allowed(&self, kind: u16) -> bool {
        self.injector.feedback_allowed(kind)
    }
}

impl Drop for NativeSink {
    fn drop(&mut self) {
        // Unplugs the virtual pads asynchronously; the injector's own drop releases pointers.
        self.injector.detach_gamepads();
    }
}

/// A co-op player's input, which drives its slot's pad and never the desktop.
pub(super) struct CoopSink {
    h: Shared,
    player: Player,
    warnings: Arc<butterpollo_session::session::Warnings>,
}

impl CoopSink {
    pub(super) fn create(h: &Shared, s: &Session) -> Result<Box<dyn Sink>> {
        let c = effective_config(h, &s.launch)?;
        let player = Player::of(h, &s.launch, &c).context("the co-op session ended")?;
        Ok(Box::new(Self {
            h: h.clone(),
            player,
            warnings: s.launch.warnings.clone(),
        }))
    }
}

impl Sink for CoopSink {
    fn apply(&mut self, events: &[Input]) -> Vec<anyhow::Error> {
        for event in events {
            self.player.input(event.clone(), &self.warnings);
        }
        Vec::new()
    }
    fn next_due(&self) -> Option<Instant> {
        None
    }
    fn due(&mut self) -> Result<()> {
        Ok(())
    }
    fn refresh(&mut self) -> Result<()> {
        Ok(())
    }
    fn geometry(&mut self, _output: &str, _width: u32, _height: u32) {}
    fn feedback(&mut self) -> Vec<(u16, u16, Vec<u8>)> {
        self.player
            .feedback(&self.h)
            .into_iter()
            .filter_map(|report| match report {
                PadReport::Feedback { id, kind, data } => Some((id, kind, data)),
                _ => None,
            })
            .collect()
    }
    fn feedback_allowed(&self, kind: u16) -> bool {
        self.player.feedback_allowed(kind)
    }
}

pub(super) type Make = Box<dyn FnMut(&Session) -> Result<Box<dyn Sink>> + Send>;
pub(super) type Reply = Box<dyn FnMut(Vec<u8>) -> bool + Send>;

/// The kinds of event in a call, each once, in the order they first appear.
pub(super) fn event_kinds(events: &[Input]) -> Vec<String> {
    let mut kinds: Vec<String> = Vec::new();
    for event in events {
        let kind = format!("{event:?}")
            .split([' ', '(', '{'])
            .next()
            .unwrap_or("")
            .to_string();
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    kinds
}

/// What a session's input did, for its end-of-stream log line.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub gamepad_messages: u64,
    /// Valid gamepad messages that changed nothing: stale snapshots and unknown controllers.
    pub gamepad_ignored: u64,
    pub input_messages: u64,
    /// Messages the parser refused.
    pub invalid: u64,
    /// Key codes the table has no virtual key for.
    pub unknown_codes: u64,
    /// Events the injector was handed.
    pub injected: u64,
    /// Events the device's permissions or a missing injector kept from the desktop.
    pub dropped: u64,
    pub rumble_sent: u64,
    /// Messages whose events reached the injector, and how long the host held them: from the
    /// transport receiving the frame to the worker calling the injector. The injector call itself,
    /// `SendInput`, is below: it can take milliseconds (on the 780M laptop the first left press
    /// after idle takes 20 to 38 ms, with or without a host), as it does for ENet, and that is not
    /// the host's queue.
    pub queued: u64,
    pub queue_p50_us: u64,
    pub queue_max_us: u64,
    pub queue_p99_us: u64,
    /// The parts of that delay: waiting in the channel for the worker, and the injector's
    /// periodic refresh that runs before the events; then the injector call itself.
    pub wait_p50_us: u64,
    pub wait_max_us: u64,
    pub wait_p99_us: u64,
    pub refresh_max_us: u64,
    pub refresh_p99_us: u64,
    pub apply_max_us: u64,
    pub apply_p99_us: u64,
    /// Injector calls that took [`super::SLOW_APPLY`] or more. Only the first 16 are logged; this is all.
    pub slow_calls: u64,
}

/// Sub-buckets per power of two in [`Delays`]: a percentile is at most 1/SUB too high.
const SUB: usize = 4;
/// Values below this are their own bucket.
const EXACT: u64 = 8;
pub(super) const BUCKETS: usize = EXACT as usize + SUB * 37;

/// A histogram of delays in microseconds, four buckets to the octave: no allocation per message,
/// exact below 8 us and within 25 % above.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Delays {
    pub(super) count: u64,
    pub(super) max_us: u64,
    buckets: [u64; BUCKETS],
}

impl Default for Delays {
    fn default() -> Self {
        Self {
            count: 0,
            max_us: 0,
            buckets: [0; BUCKETS],
        }
    }
}

impl Delays {
    pub(super) fn bucket(us: u64) -> usize {
        if us < EXACT {
            return us as usize;
        }
        let octave = (u64::BITS - 1 - us.leading_zeros()) as usize;
        let sub = ((us >> (octave - 2)) & (SUB as u64 - 1)) as usize;
        (EXACT as usize + (octave - 3) * SUB + sub).min(BUCKETS - 1)
    }

    /// The largest value that falls in `bucket`.
    pub(super) fn upper(bucket: usize) -> u64 {
        if bucket < EXACT as usize {
            return bucket as u64;
        }
        let octave = (bucket - EXACT as usize) / SUB + 3;
        let sub = (bucket - EXACT as usize) % SUB;
        (((SUB + sub + 1) as u64) << (octave - 2)) - 1
    }

    pub(super) fn record(&mut self, delay: Duration) {
        let us = u64::try_from(delay.as_micros()).unwrap_or(u64::MAX);
        self.buckets[Self::bucket(us)] += 1;
        self.count += 1;
        self.max_us = self.max_us.max(us);
    }

    /// The upper edge of the bucket the 99th percentile falls in, never above the maximum. It is
    /// a figure over messages: one that refreshed the pointer counts once, like any other.
    pub(super) fn p99_us(&self) -> u64 {
        self.percentile_us(99)
    }

    pub(super) fn median_us(&self) -> u64 {
        self.percentile_us(50)
    }

    fn percentile_us(&self, percent: u64) -> u64 {
        let rank = (self.count * percent).div_ceil(100);
        let mut seen = 0;
        for (bucket, count) in self.buckets.iter().enumerate() {
            seen += count;
            if self.count > 0 && seen >= rank {
                return Self::upper(bucket).min(self.max_us);
            }
        }
        0
    }
}
