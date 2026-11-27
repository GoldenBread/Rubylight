//! Recording input sink used by tests and `test-support` reconnect harnesses.
use super::{Input, Sink};
use anyhow::Result;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Default)]
struct Log {
    events: Vec<Input>,
    feedback: Vec<(u16, u16, Vec<u8>)>,
    made: usize,
    alive: usize,
    fail_makes: usize,
    denied_kinds: Vec<u16>,
    panic_next: bool,
    apply_delay: Duration,
    suspended: bool,
}

/// An injector that records what it is handed and injects nothing: a test fixture's input
/// device, shared by every sink it makes.
#[derive(Default, Clone)]
pub struct Recorder(Arc<Mutex<Log>>);

impl Recorder {
    /// Returns the recorder installed on `fixture`, installing one on first use.
    pub fn for_fixture(fixture: &butterpollo_stream::stream::reconnect_harness::Fixture) -> Self {
        let mut slot = fixture.webrtc_input.lock().unwrap();
        if let Some(recorder) = slot.as_ref().and_then(|any| any.downcast_ref::<Recorder>()) {
            return recorder.clone();
        }
        let recorder = Self::default();
        *slot = Some(Arc::new(recorder.clone()));
        recorder
    }

    pub fn sink(&self) -> Box<dyn Sink> {
        let mut log = self.0.lock().unwrap();
        log.made += 1;
        log.alive += 1;
        Box::new(Recording(self.clone()))
    }

    /// Every event injected so far, in order.
    pub fn events(&self) -> Vec<Input> {
        self.0.lock().unwrap().events.clone()
    }

    pub fn clear(&self) {
        self.0.lock().unwrap().events.clear();
    }

    /// Sinks made so far, and how many are still alive.
    pub fn sinks(&self) -> (usize, usize) {
        let log = self.0.lock().unwrap();
        (log.made, log.alive)
    }

    /// Rumble or effects a controller's driver posted: (slot, kind, data).
    pub fn post_feedback(&self, slot: u16, kind: u16, data: &[u8]) {
        self.0
            .lock()
            .unwrap()
            .feedback
            .push((slot, kind, data.to_vec()));
    }

    /// The next `count` attempts to make a sink fail.
    pub fn fail_makes(&self, count: usize) {
        self.0.lock().unwrap().fail_makes = count;
    }

    /// The next `apply` panics, as a bug in the injector would.
    pub fn panic_once(&self) {
        self.0.lock().unwrap().panic_next = true;
    }

    /// Every `apply` takes this long, as `SendInput` does behind a slow low-level hook.
    pub fn slow_apply(&self, delay: Duration) {
        self.0.lock().unwrap().apply_delay = delay;
    }

    pub fn forbid_feedback(&self, kind: u16) {
        self.0.lock().unwrap().denied_kinds.push(kind);
    }

    pub fn set_suspended(&self, suspended: bool) {
        self.0.lock().unwrap().suspended = suspended;
    }

    /// Whether a sink could be made.
    pub fn make(&self) -> Result<Box<dyn Sink>> {
        let mut log = self.0.lock().unwrap();
        if log.fail_makes > 0 {
            log.fail_makes -= 1;
            anyhow::bail!("the test injector is unavailable");
        }
        drop(log);
        Ok(self.sink())
    }
}

struct Recording(Recorder);

impl Sink for Recording {
    fn apply(&mut self, events: &[Input]) -> Vec<anyhow::Error> {
        let mut log = (self.0).0.lock().unwrap();
        if std::mem::take(&mut log.panic_next) {
            drop(log);
            panic!("the test injector panicked");
        }
        let delay = log.apply_delay;
        log.events.extend_from_slice(events);
        drop(log);
        if !delay.is_zero() {
            std::thread::sleep(delay);
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
    fn geometry(&mut self, _: &str, _: u32, _: u32) {}
    fn feedback(&mut self) -> Vec<(u16, u16, Vec<u8>)> {
        std::mem::take(&mut (self.0).0.lock().unwrap().feedback)
    }
    fn feedback_allowed(&self, kind: u16) -> bool {
        !(self.0).0.lock().unwrap().denied_kinds.contains(&kind)
    }
    fn game_suspended(&self) -> bool {
        (self.0).0.lock().unwrap().suspended
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        (self.0).0.lock().unwrap().alive -= 1;
    }
}
