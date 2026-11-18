//! The app's connection attempts: the one the connecting screen shows, and those the player
//! cancelled or that ran out of time. An ending attempt never shows again, but its worker may
//! still be inside a call that cannot be stopped (a decoder being made on a graphics device while
//! the driver resets), so it keeps the device hold it took until it ends. A stream it makes after
//! it left the screen is stopped where it is made, in the worker.

use super::*;

impl Connecting {
    /// Whether the worker has not answered by `now`, [`CONNECT_TIMEOUT`] after its last sign of
    /// progress ([`Connecting::progress`]).
    pub fn overdue(&self, now: Instant) -> bool {
        connect_overdue(self.progress, now)
    }

    /// Reaps an attempt that left the screen before its worker answered: true once the worker
    /// ended. A stream it made in the meantime is stopped here, as nobody waits for it.
    pub fn ended(&mut self) -> bool {
        match self.result.try_recv() {
            Ok(Ok((session, _, _))) => {
                eprintln!("nightjar: a cancelled start made a stream after all; it ends");
                lock(&self.shared.roam_watch).take();
                // Its threads may outlive the worker by a decoder call: the app joins them.
                lock(&self.shared.teardowns).push(super::reap_session(session));
                true
            }
            Ok(Err(_)) | Err(std::sync::mpsc::TryRecvError::Disconnected) => true,
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
        }
    }
}

/// The app's connection attempts. [`Attempts::cancel`] and [`Attempts::end`] take one off the
/// connecting screen at once, so Start and Cancel answer whatever its worker is doing; the worker
/// finishes on its own and is reaped, and waited for when the app leaves ([`end_all_before`]).
#[derive(Default)]
pub struct Attempts {
    current: Option<Connecting>,
    /// Cancelled or timed-out attempts whose workers have not ended yet.
    ending: Vec<Connecting>,
    /// Teardowns the ended attempts set aside, kept after their attempt is dropped so the app can
    /// join them before the renderer goes.
    teardowns: Vec<super::Teardown>,
}

impl Attempts {
    /// The attempt the connecting screen draws, if any.
    pub fn current(&self) -> Option<&Connecting> {
        self.current.as_ref()
    }

    /// The attempt the connecting screen polls, if any.
    pub fn current_mut(&mut self) -> Option<&mut Connecting> {
        self.current.as_mut()
    }

    /// Whether the connecting screen shows a start.
    pub fn is_connecting(&self) -> bool {
        self.current.is_some()
    }

    /// Whether a worker may still use the renderer's GPU device: one connecting, or one ending
    /// after its screen left. A renderer or graphics card switch waits for these.
    pub fn is_busy(&self) -> bool {
        self.current.is_some() || !self.ending.is_empty()
    }

    /// Shows `connecting` on the connecting screen. One start connects at a time: an attempt still
    /// on the screen is kept as ending, so its worker and its device hold are not dropped.
    pub fn begin(&mut self, connecting: Connecting) {
        if let Some(previous) = self.current.take() {
            eprintln!("nightjar: two starts connecting at once; the older one ends");
            self.end(previous);
        }
        self.current = Some(connecting);
    }

    /// Takes the attempt off the screen for its result, without cancelling it.
    pub fn take(&mut self) -> Option<Connecting> {
        self.current.take()
    }

    /// The player cancelled the start: the PC list is back now, and the worker, which may be
    /// inside a call that cannot be stopped, ends on its own. True when there was a start to end.
    pub fn cancel(&mut self) -> bool {
        let Some(connecting) = self.take() else { return false };
        self.end(connecting);
        true
    }

    /// Ends an attempt the app takes off the screen (a timeout, or a lost graphics device). Its
    /// worker is told to stop, and a stream it starts later is stopped too.
    pub fn end(&mut self, connecting: Connecting) {
        connecting.cancel();
        self.ending.push(connecting);
    }

    /// Whether the attempt on the screen has shown no progress for [`CONNECT_TIMEOUT`].
    pub fn overdue(&self, now: Instant) -> bool {
        self.current.as_ref().is_some_and(|connecting| connecting.overdue(now))
    }

    /// Drops the ending attempts whose workers ended, stopping any stream one made late and
    /// keeping the teardowns it set aside. True when at least one ended.
    pub fn reap(&mut self) -> bool {
        let ending = std::mem::take(&mut self.ending);
        let mut kept = Vec::with_capacity(ending.len());
        let mut ended_any = false;
        for mut attempt in ending {
            if attempt.ended() {
                ended_any = true;
                self.teardowns.extend(attempt.take_teardowns());
            } else {
                kept.push(attempt);
            }
        }
        self.ending = kept;
        ended_any
    }

    /// The device holds of every attempt, for the renderer's remake (`App::launch_devices`).
    #[cfg(windows)]
    pub fn devices(&self) -> impl Iterator<Item = &crate::media::DeviceRef> {
        self.current.iter().chain(self.ending.iter()).map(|attempt| &attempt.plan.device)
    }

    /// Sessions the attempts set aside to stop (a recovery that gave up, a previous session's
    /// stop that timed out, a cancelled start's late stream), for the app to keep and join
    /// before the renderer goes.
    pub fn drain_teardowns(&mut self) -> Vec<super::Teardown> {
        let mut ended: Vec<super::Teardown> = std::mem::take(&mut self.teardowns);
        if let Some(current) = &self.current {
            ended.append(&mut lock(&current.shared.teardowns));
        }
        for attempt in &self.ending {
            ended.append(&mut lock(&attempt.shared.teardowns));
        }
        ended
    }

    /// Every attempt, the one on the screen cancelled too, for [`end_all_before`].
    pub fn take_all(&mut self) -> Vec<Connecting> {
        let mut all = std::mem::take(&mut self.ending);
        all.extend(self.current.take());
        all
    }
}
