//! The end of the user's session, heard by the host from systemd-logind: the PC shutting down
//! or restarting (`PrepareForShutdown(true)`) and the user's own session being removed
//! (`SessionRemoved`, a sign-out). Sleep is no end. The host's wait for it is the one
//! `Ending` is passed to; logind gives no word of critical endings, so `critical` and
//! `close_app` stay false.

use anyhow::{Context, Result};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use crate::power::logind;

/// How the session ends, for the log.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ending {
    /// The user signs out; otherwise the PC shuts down or restarts.
    pub logoff: bool,
    /// The session ends whatever the programs answer.
    pub critical: bool,
    /// A program is asked to close as well.
    pub close_app: bool,
}

/// Listens for the end of the session until dropped.
pub struct SessionEndWatch {
    stopped: Arc<AtomicBool>,
}

/// Ends what the value holds, at the point the host drops it: the listeners stop at their next
/// signal.
impl Drop for SessionEndWatch {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
    }
}

type OnEnd = Arc<Mutex<Option<Box<dyn FnOnce(Ending) + Send>>>>;

/// Calls the handler the first time only.
fn fire(on_end: &OnEnd, ending: Ending) {
    let handler = on_end.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(handler) = handler {
        handler(ending);
    }
}

impl SessionEndWatch {
    /// Start listening. `on_end` is called once with how the session ends, from a listener
    /// thread, before logind goes on to end it (the host may hold a delay inhibitor for the
    /// time it needs).
    pub fn start(on_end: impl FnOnce(Ending) + Send + 'static) -> Result<Self> {
        let manager = logind::manager()?;
        let shutdown = manager
            .receive_signal("PrepareForShutdown")
            .context("subscribing to PrepareForShutdown")?;
        let removed = manager
            .receive_signal("SessionRemoved")
            .context("subscribing to SessionRemoved")?;
        let own = std::env::var("XDG_SESSION_ID")
            .ok()
            .filter(|id| !id.is_empty());
        let stopped = Arc::new(AtomicBool::new(false));
        let on_end: OnEnd = Arc::new(Mutex::new(Some(Box::new(on_end))));
        {
            let (stopped, on_end) = (stopped.clone(), on_end.clone());
            std::thread::spawn(move || {
                for message in shutdown {
                    if stopped.load(Ordering::Relaxed) {
                        break;
                    }
                    if logind::flag(&message) == Some(true) {
                        fire(&on_end, Ending::default());
                    }
                }
            });
        }
        {
            let stopped = stopped.clone();
            std::thread::spawn(move || {
                for message in removed {
                    if stopped.load(Ordering::Relaxed) {
                        break;
                    }
                    let id = message
                        .body()
                        .deserialize::<(String, zbus::zvariant::OwnedObjectPath)>()
                        .ok()
                        .map(|(id, _)| id);
                    if id.is_some() && id == own {
                        fire(
                            &on_end,
                            Ending {
                                logoff: true,
                                ..Ending::default()
                            },
                        );
                    }
                }
            });
        }
        Ok(Self { stopped })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_handler_runs_once() {
        let count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counted = count.clone();
        let on_end: OnEnd = Arc::new(Mutex::new(Some(Box::new(move |_| {
            counted.fetch_add(1, Ordering::Relaxed);
        }))));
        fire(&on_end, Ending::default());
        fire(
            &on_end,
            Ending {
                logoff: true,
                ..Ending::default()
            },
        );
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }
}
