//! The saved display layout that streams share and restore, like the arrangement and
//! display settings leases.
use crate::state::Shared;
use anyhow::Result;
use butterpollo_config::config::Config;
use std::sync::{Arc, Mutex};

/// Streams share the saved layout's restoration, backed by the pure crash-restore
/// baseline journal in `butterpollo-media-policy`.
pub(super) type Baseline<T> = butterpollo_media_policy::display_session_machine::BaselineJournal<T>;

pub(super) type SavedLayout = (butterpollo_win_display::display::Snapshot, Vec<String>);

pub(super) static BASELINE: Mutex<Baseline<SavedLayout>> = Mutex::new(Baseline::new());

pub(super) struct GoldenLease {
    host: std::sync::Weak<crate::state::Host>,
}

impl GoldenLease {
    pub(super) fn new(
        h: &Shared,
        snapshot: butterpollo_win_display::display::Snapshot,
        config: &Config,
    ) -> Result<Self> {
        let excluded = crate::maintenance::display_exclusions(config)?;
        BASELINE.lock().unwrap().acquire(|| {
            tracing::info!("saved display baseline selected for restoration after streaming");
            butterpollo_win_display::display_recovery::baseline(Some((
                snapshot.clone(),
                excluded.clone(),
            )))?;
            Ok((snapshot, excluded))
        })?;
        Ok(Self {
            host: Arc::downgrade(h),
        })
    }
}

impl Drop for GoldenLease {
    fn drop(&mut self) {
        // Held through the restore, so a stream starting meanwhile records
        // its journal entry after this one is cleared, not before.
        BASELINE
            .lock()
            .unwrap()
            .release(|(snapshot, mut excluded)| {
                if let Some(host) = self.host.upgrade() {
                    let outputs = crate::remote_display::outputs(&host);
                    if let Ok(monitors) = butterpollo_win_display::display::monitors() {
                        excluded.extend(
                            monitors
                                .iter()
                                .filter(|m| outputs.contains(&m.display_name))
                                .map(|m| m.device_id.clone()),
                        );
                    }
                }
                // Displays still in use stay as they are: a stream without
                // this lease, a paused game display or a launch being prepared.
                excluded.extend(butterpollo_win_display::display::leased_displays());
                tracing::info!("restoring the saved display baseline");
                if let Err(error) = snapshot.restore_excluding(&excluded) {
                    tracing::warn!(%error,"saved display baseline restoration remains pending");
                } else {
                    let _ = butterpollo_win_display::display_recovery::baseline(None);
                }
            });
    }
}
