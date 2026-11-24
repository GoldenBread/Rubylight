//! The temporary display layouts streams hold (exclusive, primary, isolated) and their
//! restoration, on Linux.
//!
//! Linux cannot arrange displays yet: a layout lease cannot be taken, so no layout is ever
//! in force, no display leaves one, and the watch over a woken monitor never runs.
use crate::display::Snapshot;
use anyhow::Result;
use butterpollo_media_policy::display_policy::Arrangement;

/// Load the layouts remembered per device from `directory`. Nothing is remembered on Linux,
/// where no layout is ever applied.
pub fn initialize_remembered(_directory: &std::path::Path) {}

/// Tell the layouts streams hold that a remote monitor's display was switched on (`on`) or is
/// about to be removed. No stream holds a layout on Linux, so there is nobody to tell.
pub fn retain(_output: &str, _on: bool) -> Result<()> {
    Ok(())
}

/// How many streams' displays have left a layout other streams still hold. Always 0 on Linux.
pub fn departures() -> u64 {
    0
}

/// How many times a monitor's return had displays put back. Always 0 on Linux.
pub fn wake_layouts() -> u64 {
    0
}

/// The windows that were minimised before a display left, while that departure is recent.
/// None on Linux, where no display leaves a layout.
pub fn minimized_before_departure() -> Option<std::collections::BTreeSet<isize>> {
    None
}

/// Undo what the system changed on the other displays when a remote monitor's display
/// `output` arrived, `before` being the layout from just before it. Not available on Linux.
pub fn undo_recall_for_remote_monitor(_before: &Snapshot, _output: &str) -> Result<()> {
    Err(crate::unsupported("Restoring a display layout"))
}

/// Holds off the watch over a woken display while a launch prepares a display. No such watch
/// runs on Linux, so it holds nothing.
#[must_use]
pub struct WakeWatchOff(());

/// Hold off the watch over a woken display until the returned value is dropped.
pub fn hold_off_wake_watch() -> WakeWatchOff {
    WakeWatchOff(())
}

/// The host is stopping: no watch outlasts that. None runs on Linux.
pub fn close_wake_watch() {}

/// Whether a stream's end is still at work on the displays. Never on Linux, where a stream's
/// end changes no display.
pub fn wake_watch_settling() -> bool {
    false
}

/// A stream's share of the display layout in force. None can be taken on Linux yet.
pub struct Lease {
    arrangement: Arrangement,
}

impl Lease {
    /// Whether the layout in force shows the stream's display only.
    pub fn exclusive(&self) -> bool {
        self.arrangement == Arrangement::Exclusive
    }
    /// The layout in force for this stream.
    pub fn arrangement(&self) -> Arrangement {
        self.arrangement
    }
    /// Whether the layout in force makes this stream's display the primary one. Never on
    /// Linux, where no layout is applied.
    pub fn makes_primary(&self) -> bool {
        false
    }
    /// Why what the system changed when this stream's display arrived could not be put back.
    pub fn repair_failure(&self) -> Option<&str> {
        None
    }
    /// Lay the displays out as `arrangement` says around the stream's display `output`,
    /// sharing the layout with the streams that already hold it. Fails on Linux, which cannot
    /// arrange displays yet.
    pub fn acquire(
        _output: &str,
        _arrangement: Arrangement,
        _retained: &[String],
        _virtual_target: bool,
        _original: Option<Snapshot>,
        _device: &str,
    ) -> Result<Self> {
        Err(crate::unsupported("Arranging displays for a stream"))
    }
    /// End the lease, and with it what `remains` of the stream. No layout has to go back
    /// first on Linux, so `remains` is dropped at once.
    pub fn end(self, remains: impl Send + 'static) {
        drop(remains);
    }
    /// One look at a display a stopped watch was waiting for. False: nothing waits on Linux.
    pub fn follow_wake(&self) -> Result<bool> {
        Ok(false)
    }
    /// Whether this stream took up what a stopped watch left. Never on Linux.
    pub fn after_cut_watch(&self) -> bool {
        false
    }
    /// Displays this stream's layout switched off that the system has switched back on. None
    /// on Linux, where no layout switches a display off.
    pub fn switched_back_on(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
    /// Apply the stream's layout again. Not available on Linux.
    pub fn reapply(&self, _output: &str, _retained: &[String]) -> Result<()> {
        Err(crate::unsupported("Arranging displays for a stream"))
    }
    /// [`Self::reapply`] once another stream's display has left. Not available on Linux.
    pub fn reapply_after_departure(
        &self,
        output: &str,
        retained: &[String],
    ) -> Result<Option<String>> {
        self.reapply(output, retained).map(|()| None)
    }
}

/// A physical display switched on for a stream before capture or mode changes. None can be
/// switched on from Linux yet.
pub struct Activation(());

impl Activation {
    /// Switch the display `output` on for a stream. An empty `output` names no display and
    /// needs nothing; any other fails on Linux.
    pub fn acquire(output: &str) -> Result<Option<Self>> {
        if output.is_empty() {
            return Ok(None);
        }
        Err(crate::unsupported("Switching a display on for a stream"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_layout_is_ever_in_force() {
        assert!(retain("DP-1", true).is_ok() && retain("DP-1", false).is_ok());
        assert_eq!((departures(), wake_layouts()), (0, 0));
        assert!(minimized_before_departure().is_none());
        assert!(!wake_watch_settling());
        let held = hold_off_wake_watch();
        close_wake_watch();
        drop(held);
        assert!(Lease::acquire("DP-1", Arrangement::Exclusive, &[], false, None, "").is_err());
        assert!(matches!(Activation::acquire(""), Ok(None)));
        assert!(Activation::acquire("DP-1").is_err());
    }
}
