//! The focused text field for keyboard follows focus (0x553B, Experimental), read through
//! AT-SPI, the desktop's accessibility bus: a thread listens for the toolkits' focus events and
//! reports each text field the focus moves to, or that it left them. With `max_text` above 0 it
//! also follows the field's text and caret and reads up to that much around the cursor, never in
//! a password field.
//!
//! Toolkits publish their widgets only while accessibility is on (`org.a11y.Status.IsEnabled`),
//! so the watcher turns it on for its life and back off after if it was off; Qt and GTK
//! applications follow the switch while they run. An application that publishes nothing (a
//! game drawing its own text box) is never reported. A focus event that leaves the focus where
//! it was (a tooltip) reports nothing, so the keyboard does not lower and raise again for it.

use anyhow::{Context, Result};
use butterpollo_input_policy::text_input::{Focused, Watch};
use std::sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicBool, Ordering},
};
use zbus::blocking::{Connection, MessageIterator, Proxy};

mod atspi;

/// How long one call to an application may take: a hung application must not hold the
/// watcher up for long.
const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);
const EVENTS: &str = "org.a11y.atspi.Event.Object";

/// Watches the focus until dropped, one thread for the watcher's whole life: the client's
/// toggles pause and resume it ([`FocusWatch::set`]). Dropping it closes the watcher's
/// connection, which ends its thread without waiting for it.
pub struct FocusWatch {
    control: Arc<Control>,
}

struct Control {
    watch: Mutex<Watch>,
    stop: AtomicBool,
    /// The accessibility bus, once connected, so dropping the watch can close it.
    bus: Mutex<Option<Connection>>,
}

impl FocusWatch {
    /// Calls `report` on the watcher's thread with the focused text field, or `None` once the
    /// focus left text fields. `max_text` above 0 also reads up to that many bytes around the
    /// cursor of a field that is not a password, and reports again when its text or caret
    /// changes. `failed` gets why the accessibility bus could not be followed, on the same
    /// thread; `Err` only when there is no thread.
    pub fn start(
        max_text: usize,
        report: impl Fn(Option<Focused>) + Send + 'static,
        failed: impl Fn(anyhow::Error) + Send + 'static,
    ) -> Result<Self> {
        let control = Arc::new(Control {
            watch: Mutex::new(Watch::On { max_text }),
            stop: AtomicBool::new(false),
            bus: Mutex::new(None),
        });
        let shared = control.clone();
        std::thread::Builder::new()
            .name("text-focus".into())
            .spawn(move || {
                if let Err(error) = run(&shared, &report)
                    && !shared.stop.load(Ordering::Acquire)
                {
                    failed(error);
                }
            })
            .context("starting the text field watcher")?;
        Ok(Self { control })
    }

    /// Pauses the watcher for [`Watch::Off`] or resumes it, with the new `max_text`.
    pub fn set(&self, watch: Watch) {
        *self
            .control
            .watch
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = watch;
    }
}

impl Drop for FocusWatch {
    fn drop(&mut self) {
        self.control.stop.store(true, Ordering::Release);
        let bus = self
            .control
            .bus
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(bus) = bus {
            // Ends the watcher's wait for the next event.
            let _ = bus.close();
        }
    }
}

/// Turns accessibility on while it lives, and back off when dropped if it was off.
struct Accessibility {
    session: Connection,
    was_on: bool,
}

impl Accessibility {
    fn on(session: Connection) -> Result<Self> {
        let status = status(&session)?;
        let was_on = status.get_property::<bool>("IsEnabled").unwrap_or(false);
        if !was_on {
            status
                .set_property("IsEnabled", true)
                .context("turning accessibility on")?;
            tracing::info!("accessibility turned on to follow text fields");
        }
        Ok(Self { session, was_on })
    }
}

impl Drop for Accessibility {
    fn drop(&mut self) {
        if !self.was_on
            && let Ok(status) = status(&self.session)
        {
            let _ = status.set_property("IsEnabled", false);
            tracing::info!("accessibility turned back off");
        }
    }
}

fn status(session: &Connection) -> Result<Proxy<'static>> {
    Ok(zbus::blocking::proxy::Builder::new(session)
        .destination("org.a11y.Bus")?
        .path("/org/a11y/bus")?
        .interface("org.a11y.Status")?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()?)
}

/// The field the watcher reported last, to follow its text and see it lose the focus.
struct Followed {
    sender: String,
    path: String,
    sensitive: bool,
    reported: Option<Focused>,
}

fn run(control: &Control, report: &dyn Fn(Option<Focused>)) -> Result<()> {
    let session = crate::power::logind::session_bus()?;
    let address: String = Proxy::new(&session, "org.a11y.Bus", "/org/a11y/bus", "org.a11y.Bus")?
        .call("GetAddress", &())
        .context("finding the accessibility bus (is at-spi2-core installed?)")?;
    let _accessibility = Accessibility::on(session)?;
    let bus = zbus::blocking::connection::Builder::address(address.as_str())?
        .method_timeout(CALL_TIMEOUT)
        .build()
        .context("connecting to the accessibility bus")?;
    let registry = Proxy::new(
        &bus,
        "org.a11y.atspi.Registry",
        "/org/a11y/atspi/registry",
        "org.a11y.atspi.Registry",
    )?;
    for event in [
        "object:state-changed:focused",
        "object:state-changed:active",
        "object:text-changed",
        "object:text-caret-moved",
    ] {
        registry
            .call_method("RegisterEvent", &(event, Vec::<String>::new(), ""))
            .with_context(|| format!("asking for {event} events"))?;
    }
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(EVENTS)?
        .build();
    let events = MessageIterator::for_match_rule(rule, &bus, Some(256))?;
    *control.bus.lock().unwrap_or_else(PoisonError::into_inner) = Some(bus.clone());
    if control.stop.load(Ordering::Acquire) {
        return Ok(());
    }
    tracing::info!("following the focused text field through AT-SPI");
    let mut followed: Option<Followed> = None;
    let mut paused = false;
    for message in events {
        if control.stop.load(Ordering::Acquire) {
            break;
        }
        let Ok(message) = message else {
            break;
        };
        let watch = *control.watch.lock().unwrap_or_else(PoisonError::into_inner);
        let Watch::On { max_text } = watch else {
            paused = true;
            continue;
        };
        if paused {
            // A paused watcher heard nothing: what it last reported may be stale.
            paused = false;
            followed = None;
        }
        let header = message.header();
        let (Some(sender), Some(path), Some(member)) =
            (header.sender(), header.path(), header.member())
        else {
            continue;
        };
        let (sender, path) = (sender.to_string(), path.to_string());
        let Some(event) = atspi::Event::parse(member.as_str(), &message) else {
            continue;
        };
        let same = followed
            .as_ref()
            .is_some_and(|f| f.sender == sender && f.path == path);
        match event {
            atspi::Event::Focused(true) | atspi::Event::Activated => {
                let element = if event == atspi::Event::Activated {
                    atspi::focused_within(&bus, &sender, &path)
                } else {
                    atspi::read(&bus, &sender, &path)
                };
                let Some(element) = element else {
                    continue;
                };
                if element.passes_over() {
                    continue;
                }
                let focused = element.focused(&bus, max_text);
                followed = focused.as_ref().map(|focused| Followed {
                    sender: element.sender.clone(),
                    path: element.path.clone(),
                    sensitive: focused.field.sensitive,
                    reported: Some(focused.clone()),
                });
                report(focused);
            }
            // Only the field the watcher followed losing the focus lowers the keyboard: the
            // element gaining it reports itself.
            atspi::Event::Focused(false) if same => {
                followed = None;
                report(None);
            }
            atspi::Event::Text if same && max_text > 0 => {
                let Some(f) = followed.as_mut().filter(|f| !f.sensitive) else {
                    continue;
                };
                let Some(previous) = f.reported.clone() else {
                    continue;
                };
                let surrounding = atspi::surrounding(&bus, &f.sender, &f.path, max_text);
                if surrounding != previous.surrounding {
                    let focused = Focused {
                        surrounding,
                        ..previous
                    };
                    f.reported = Some(focused.clone());
                    report(Some(focused));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "text_focus/tests.rs"]
mod tests;
