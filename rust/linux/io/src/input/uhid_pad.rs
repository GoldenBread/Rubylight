//! A virtual controller of any model on `/dev/uhid`. The Windows host presents each pad as a
//! device with its own HID descriptor and reports (the VHF driver's profiles in
//! `rubylight-vgamepad-proto`, the Steam Deck in `butterpollo_input_policy::steam_deck`); here a
//! [`PadModel`] wraps one of them and the kernel's HID layer is the host: the descriptor and
//! the answers to the kernel's feature-report requests come from the model, so the driver for
//! that vendor and product (hid-nintendo, hid-playstation, hid-steam) binds as it would to the
//! real controller, and the reports the driver writes back (rumble, lights, haptics) leave as
//! the client's feedback, as on Windows. One thread per pad reads the kernel's requests and
//! ticks a model that must report on its own, as a real controller does.

use super::{
    PadReport,
    feedback::Sink,
    hid_pad::Slot,
    uhid::{self, Description, Event, ReportType, Uhid},
};
use anyhow::{Context, Result, bail};
use butterpollo_input_policy::input::Input;
use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// How long the kernel's driver gets to finish binding to a new pad.
const BIND: Duration = Duration::from_secs(6);
/// How long a thread waits in one read of the uhid device.
const POLL: Duration = Duration::from_millis(50);

/// What a pad is to the kernel.
pub(super) struct Identity {
    pub(super) name: &'static str,
    pub(super) vendor: u16,
    pub(super) product: u16,
    pub(super) version: u16,
    /// The device's `phys` text up to the slot, which a test finds its nodes by.
    pub(super) phys: &'static str,
    /// The kernel module expected to bind it, for the warning when none does.
    pub(super) driver: &'static str,
}

/// A controller's device model: what it reports and how it answers the kernel.
pub(super) trait PadModel: Send + 'static {
    fn identity(&self) -> Identity;
    /// The HID report descriptor the kernel parses.
    fn descriptor(&mut self) -> Vec<u8>;
    /// The report a driver reads, `None` for one the controller does not serve. The reply
    /// starts with the report id, a 0 byte for a controller without report ids.
    fn get_report(&mut self, kind: ReportType, number: u8) -> Option<Vec<u8>>;
    /// A feature or output report a driver sets; `data` starts with the report id.
    fn set_report(&mut self, kind: ReportType, number: u8, data: &[u8]) -> bool;
    /// A report a driver writes to the output endpoint; `data` starts with the report id.
    fn output(&mut self, data: &[u8]);
    /// Applies a controller state, motion, touch or battery event.
    fn apply(&mut self, event: &Input);
    /// How often the model reports on its own, for a controller that streams its state.
    fn period(&self) -> Option<Duration> {
        None
    }
    fn tick(&mut self) {}
    /// The input reports the model has to send, oldest first.
    fn take_reports(&mut self) -> Vec<Vec<u8>>;
    /// The client feedback (kind, data) the driver's writes changed.
    fn take_feedback(&mut self) -> Vec<(u16, Vec<u8>)>;
    /// Whether the driver has finished its start-up exchange with the controller.
    fn bound(&self) -> bool;
}

struct Shared {
    id: u16,
    model: Mutex<Box<dyn PadModel>>,
    uhid: Uhid,
    sink: Arc<Sink>,
    stop: AtomicBool,
}

/// One virtual controller on `/dev/uhid`.
pub(super) struct UhidPad {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    /// The device's `phys` name, with which a test finds its nodes.
    pub(super) phys: String,
    _slot: Slot,
}

impl Shared {
    fn model(&self) -> std::sync::MutexGuard<'_, Box<dyn PadModel>> {
        self.model.lock().unwrap_or_else(PoisonError::into_inner)
    }
    /// Sends what the model has to report and hands the client its feedback.
    fn flush(&self, model: &mut dyn PadModel) {
        for report in model.take_reports() {
            if let Err(error) = self.uhid.send(&uhid::encode_input(&report)) {
                tracing::debug!(%error, "uhid report failed");
            }
        }
        for (kind, data) in model.take_feedback() {
            self.sink.push(PadReport::Feedback {
                id: self.id,
                kind,
                data,
            });
        }
    }
    fn handle(&self, event: Event) {
        let mut model = self.model();
        match event {
            Event::GetReport { id, number, kind } => {
                let report = model.get_report(kind, number);
                if let Err(error) = self
                    .uhid
                    .send(&uhid::encode_get_report_reply(id, report.as_deref()))
                {
                    tracing::debug!(%error, "uhid report reply failed");
                }
            }
            Event::SetReport {
                id,
                number,
                kind,
                data,
            } => {
                let accepted = model.set_report(kind, number, &data);
                if let Err(error) = self.uhid.send(&uhid::encode_set_report_reply(id, accepted)) {
                    tracing::debug!(%error, "uhid report reply failed");
                }
            }
            Event::Output(data) => model.output(&data),
            _ => {}
        }
        self.flush(model.as_mut());
    }
    fn tick(&self) {
        let mut model = self.model();
        model.tick();
        self.flush(model.as_mut());
    }
}

impl UhidPad {
    /// Creates pad `id` in `slot` for the kernel to bind its driver to. Returns once the
    /// driver has finished starting, or fails naming the driver when none did.
    pub(super) fn create_in(
        id: u16,
        slot: Slot,
        mut model: Box<dyn PadModel>,
        sink: Arc<Sink>,
    ) -> Result<Self> {
        let identity = model.identity();
        let descriptor = model.descriptor();
        let period = model.period();
        let shared = Arc::new(Shared {
            id,
            model: Mutex::new(model),
            uhid: Uhid::open()?,
            sink,
            stop: AtomicBool::new(false),
        });
        let phys = format!(
            "{}-{}/{}{}",
            "rubylight",
            std::process::id(),
            identity.phys,
            slot.0
        );
        let reader = shared.clone();
        let thread = std::thread::Builder::new()
            .name(format!("pad {id} uhid"))
            .spawn(move || {
                let mut next = period.map(|p| Instant::now() + p);
                while !reader.stop.load(Ordering::Acquire) {
                    let wait = next.map_or(POLL, |at| {
                        at.saturating_duration_since(Instant::now()).min(POLL)
                    });
                    match reader.uhid.read(wait) {
                        Ok(Some(event)) => reader.handle(event),
                        Ok(None) => {}
                        Err(error) => {
                            tracing::debug!(%error, "uhid device ended");
                            break;
                        }
                    }
                    if let (Some(at), Some(period)) = (next, period) {
                        let now = Instant::now();
                        if now >= at {
                            reader.tick();
                            // A pause (the host was busy) is not made up for in a burst.
                            next = Some((at + period).max(now));
                        }
                    }
                }
            })
            .context("the uhid thread did not start")?;
        let pad = Self {
            shared,
            thread: Some(thread),
            phys,
            _slot: slot,
        };
        let description = Description {
            name: identity.name,
            phys: &pad.phys,
            uniq: "",
            bus: uhid::BUS_USB,
            vendor: u32::from(identity.vendor),
            product: u32::from(identity.product),
            version: u32::from(identity.version),
            descriptor: &descriptor,
        };
        // On failure the pad's drop stops the thread.
        pad.shared
            .uhid
            .create(&description)
            .with_context(|| format!("cannot create the virtual {} on /dev/uhid", identity.name))?;
        let deadline = Instant::now() + BIND;
        while !pad.shared.model().bound() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        if !pad.shared.model().bound() {
            bail!(
                "the kernel's {} driver did not bind the virtual {} (vendor {:04x}, product {:04x}); is that module available?",
                identity.driver,
                identity.name,
                identity.vendor,
                identity.product
            );
        }
        Ok(pad)
    }
    /// Applies a controller, motion, touch or battery event and sends what the pad reports.
    pub(super) fn apply(&self, event: &Input) {
        let mut model = self.shared.model();
        model.apply(event);
        self.shared.flush(model.as_mut());
    }
}

impl Drop for UhidPad {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.shared.uhid.destroy();
        if let Some(thread) = self.thread.take() {
            drop(thread.join());
        }
    }
}
