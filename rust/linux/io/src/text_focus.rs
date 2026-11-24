//! The focused text field for keyboard follows focus (0x553B, Experimental). Linux has no
//! accessibility backend for it yet, so starting a watcher fails and no focus is reported.

use anyhow::Result;
use butterpollo_input_policy::text_input::{Focused, Watch};

/// Watches the focus until dropped. None exists on Linux yet: [`FocusWatch::start`] always
/// fails.
pub struct FocusWatch {
    _watching: (),
}

impl FocusWatch {
    /// Calls `report` with the focused text field, or `None` once the focus left text fields,
    /// and `failed` with why the focus could not be followed. Fails on Linux, where the
    /// focused text field cannot be read yet; neither callback is ever called.
    pub fn start(
        max_text: usize,
        report: impl Fn(Option<Focused>) + Send + 'static,
        failed: impl Fn(anyhow::Error) + Send + 'static,
    ) -> Result<Self> {
        let _ = (max_text, report, failed);
        Err(crate::unsupported("Following the focused text field"))
    }

    /// Pauses the watcher for [`Watch::Off`] or resumes it. Does nothing on Linux, where no
    /// watcher runs.
    pub fn set(&self, watch: Watch) {
        let _ = watch;
    }
}
