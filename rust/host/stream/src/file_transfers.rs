//! File transfer concurrency and pacer state stored on `Host` (`docs/native-api.md`, "Files").

use crate::state::Shared;
use butterpollo_session::file_transfer::Pacer;
use butterpollo_win_io::user_folder::{UserFolder, UserFolderError};
use std::{
    collections::HashMap,
    sync::{Mutex, PoisonError},
    time::{Duration, Instant},
};

/// How often a running transfer looks whether a stream started or ended,
/// whether the pace changed and whether the device may still transfer.
pub const PACE_CHECK: Duration = Duration::from_millis(250);
/// What a paced transfer may move at once after a pause.
pub const PACE_BURST: u64 = 256 * 1024;
/// Free space an upload leaves on its volume: 1/50 (2 %) of the volume, at
/// least 1 GiB and at most 4 GiB.
const MIN_RESERVE: u64 = 1 << 30;
const MAX_RESERVE: u64 = 4 << 30;
const RESERVE_SHARE: u64 = 50;

/// How long a transfer may stall, and how much of a refused upload is read.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub idle: Duration,
    pub min_rate: u64,
    pub grace: Duration,
    pub drain_bytes: u64,
    pub drain_time: Duration,
}

const LIMITS: Limits = Limits {
    idle: Duration::from_secs(30),
    min_rate: 64 * 1024,
    grace: Duration::from_secs(30),
    drain_bytes: 8 << 20,
    drain_time: Duration::from_secs(5),
};

/// The transfers running, all paced together.
#[derive(Debug)]
pub struct Transfers {
    pub uploads: Mutex<HashMap<String, u64>>,
    pub downloads: Mutex<HashMap<String, usize>>,
    pacing: Mutex<Pacing>,
    #[cfg(any(test, feature = "test-support"))]
    limits: Mutex<Limits>,
    #[cfg(any(test, feature = "test-support"))]
    space: Mutex<Option<(u64, u64)>>,
    #[cfg(any(test, feature = "test-support"))]
    refusal_hold: Mutex<Duration>,
}

impl Default for Transfers {
    fn default() -> Self {
        Self {
            uploads: Mutex::default(),
            downloads: Mutex::default(),
            pacing: Mutex::new(Pacing {
                pacer: Pacer::new(None, PACE_BURST, Instant::now()),
                checked: None,
            }),
            #[cfg(any(test, feature = "test-support"))]
            limits: Mutex::new(LIMITS),
            #[cfg(any(test, feature = "test-support"))]
            space: Mutex::default(),
            #[cfg(any(test, feature = "test-support"))]
            refusal_hold: Mutex::default(),
        }
    }
}

#[derive(Debug)]
struct Pacing {
    pacer: Pacer,
    checked: Option<Instant>,
}

impl Transfers {
    /// How long to wait before moving `bytes`: unpaced unless a stream runs.
    pub fn pace(&self, h: &Shared, bytes: u64) -> Duration {
        let now = Instant::now();
        let mut pacing = self.pacing.lock().unwrap_or_else(PoisonError::into_inner);
        if pacing
            .checked
            .is_none_or(|at| now.duration_since(at) >= PACE_CHECK)
        {
            pacing.checked = Some(now);
            let want = if crate::updater::streaming(h) {
                h.config
                    .read()
                    .unwrap_or_else(PoisonError::into_inner)
                    .file_transfer_stream_rate()
            } else {
                None
            };
            if pacing.pacer.rate() != want {
                pacing.pacer.set_rate(want, now);
            }
        }
        pacing.pacer.take(bytes, now)
    }

    #[cfg(not(any(test, feature = "test-support")))]
    pub fn limits(&self) -> Limits {
        LIMITS
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn limits(&self) -> Limits {
        *self.limits.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[cfg(not(any(test, feature = "test-support")))]
    pub fn space(&self, folder: &UserFolder) -> Result<(u64, u64), UserFolderError> {
        folder.volume_bytes()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn space(&self, folder: &UserFolder) -> Result<(u64, u64), UserFolderError> {
        match *self.space.lock().unwrap_or_else(PoisonError::into_inner) {
            Some(space) => Ok(space),
            None => folder.volume_bytes(),
        }
    }

    pub fn room(&self, folder: &UserFolder) -> Result<u64, UserFolderError> {
        let uploads = self.uploads.lock().unwrap_or_else(PoisonError::into_inner);
        let (free, total) = self.space(folder)?;
        Ok(room(&uploads, free, total))
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn refusal_hold_duration(&self) -> Duration {
        *self
            .refusal_hold
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Transfers {
    pub fn limit(&self, change: impl FnOnce(&mut Limits)) {
        change(&mut self.limits.lock().unwrap());
    }

    pub fn fake_space(&self, free: u64, total: u64) {
        *self.space.lock().unwrap() = Some((free, total));
    }

    pub fn hold_refusals(&self, hold: Duration) {
        *self.refusal_hold.lock().unwrap() = hold;
    }

    pub fn downloads_count(&self) -> usize {
        self.downloads.lock().unwrap().values().sum()
    }

    pub fn downloads(&self) -> usize {
        self.downloads_count()
    }

    pub fn clear_pacing_check_for_test(&self) {
        self.pacing.lock().unwrap().checked.take();
    }

    pub fn pacer_rate_for_test(&self) -> Option<u64> {
        self.pacing.lock().unwrap().pacer.rate()
    }
}

/// What is left of `free` bytes on a volume of `total` for another upload.
pub fn room(uploads: &HashMap<String, u64>, free: u64, total: u64) -> u64 {
    let reserve = (total / RESERVE_SHARE).clamp(MIN_RESERVE, MAX_RESERVE);
    let due = uploads
        .values()
        .fold(0u64, |due, left| due.saturating_add(*left));
    free.saturating_sub(reserve).saturating_sub(due)
}
