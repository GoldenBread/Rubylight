//! Long-lived retained virtual display lease and background heartbeat worker.

use super::super::*;
use super::Guard;

/// A monitor lease outlives a streaming session. The feeder owns only the guard,
/// so dropping the final lease stops it before removing VDD.
pub struct Retained {
    pub output: String,
    pub mode: (u32, u32, u32, bool),
    guard: std::sync::Arc<std::sync::Mutex<Option<Guard>>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    active: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Retained {
    pub fn stream_started(&self) {
        self.active
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        if let Some(worker) = &self.worker {
            worker.thread().unpark();
        }
    }
    pub fn stream_stopped(&self) {
        self.active
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
    pub fn capture_target(&self) -> (String, u64) {
        self.guard.lock().unwrap().as_ref().map_or_else(
            || (self.output.clone(), 0),
            |guard| (guard.output.clone(), guard.generation),
        )
    }
    pub fn current_output(&self) -> String {
        self.guard
            .lock()
            .unwrap()
            .as_ref()
            .map_or_else(|| self.output.clone(), |g| g.output.clone())
    }
    /// A lease with no display behind it, for the host's tests: it holds
    /// no guard or worker, so dropping it touches nothing.
    #[doc(hidden)]
    pub fn inert(output: &str) -> Self {
        Self {
            output: output.to_owned(),
            mode: (0, 0, 0, false),
            guard: Default::default(),
            stop: Default::default(),
            active: Default::default(),
            worker: None,
        }
    }
    pub fn create(id: &str, width: u32, height: u32, fps: u32, hdr: bool) -> Result<Self> {
        Self::create_rate(
            id,
            width,
            height,
            butterpollo_media_policy::framegen::Rate(
                fps.checked_mul(1000).context("refresh overflow")?,
            ),
            hdr,
        )
    }
    pub fn create_rate(
        id: &str,
        width: u32,
        height: u32,
        rate: butterpollo_media_policy::framegen::Rate,
        hdr: bool,
    ) -> Result<Self> {
        Self::create_options(
            id,
            width,
            height,
            rate,
            hdr,
            &VirtualOptions::default(),
            None,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn create_options(
        id: &str,
        width: u32,
        height: u32,
        rate: butterpollo_media_policy::framegen::Rate,
        hdr: bool,
        options: &VirtualOptions,
        on_recovery: Option<Box<dyn Fn() -> Result<()> + Send>>,
    ) -> Result<Self> {
        let guard = Guard::new_virtual_options(
            "",
            true,
            id,
            width,
            height,
            rate,
            Some(hdr),
            None,
            None,
            options,
        )?;
        let output = guard.output.clone();
        let guard = std::sync::Arc::new(std::sync::Mutex::new(Some(guard)));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let active = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let worker_guard = guard.clone();
        let worker_stop = stop.clone();
        let worker_active = active.clone();
        let worker = std::thread::Builder::new()
            .name("monitor-lease".into())
            .spawn(move || {
                let mut pending = false;
                let mut retry_at = Instant::now();
                let mut heartbeat =
                    butterpollo_media_policy::display_policy::HeartbeatThrottle::default();
                while !worker_stop.load(std::sync::atomic::Ordering::Acquire) {
                    // Windows refuses display configuration from the normal
                    // desktop while it is locked.
                    crate::desktop::keep_on_input_desktop();
                    let (result, restoring) = {
                        let mut guard = worker_guard.lock().unwrap();
                        let result = guard.as_mut().map(Guard::feed).transpose();
                        let restoring = guard.as_ref().is_some_and(|guard| guard.restore_pending);
                        (result, restoring)
                    };
                    let retry_feed = restoring || result.is_err();
                    match result {
                        Ok(Some(true)) => {
                            let _ = heartbeat.note_ok();
                            pending = true;
                            retry_at = Instant::now();
                        }
                        Err(e) => {
                            if heartbeat.note_error(&e.to_string()) {
                                tracing::warn!(error=%e,"retained monitor lease heartbeat failed");
                            }
                        }
                        _ => {
                            let _ = heartbeat.note_ok();
                        }
                    }
                    if pending && Instant::now() >= retry_at {
                        retry_at = Instant::now() + Duration::from_secs(1);
                        match on_recovery.as_ref().map(|callback| callback()).transpose() {
                            Ok(applied) => {
                                pending = false;
                                // Applying the layout can recall a saved mode.
                                if applied.is_some()
                                    && let Some(guard) = worker_guard.lock().unwrap().as_ref()
                                {
                                    guard.apply_virtual_mode("after layout recovery");
                                }
                            }
                            Err(error) => {
                                tracing::warn!(%error, "retained monitor layout recovery failed")
                            }
                        }
                    }
                    let base = Duration::from_millis(if pending || retry_feed {
                        25
                    } else if worker_active.load(std::sync::atomic::Ordering::Acquire) != 0 {
                        100
                    } else {
                        250
                    });
                    std::thread::park_timeout(heartbeat.next_tick(base, Duration::from_secs(1)));
                }
            })?;
        Ok(Self {
            output,
            mode: (width, height, rate.0, hdr),
            guard,
            stop,
            active,
            worker: Some(worker),
        })
    }
}
impl Drop for Retained {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            let deadline = Instant::now() + Duration::from_secs(5);
            while !worker.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if !worker.is_finished() {
                // The worker owns the guard until the driver returns. Taking
                // its lock here would turn a bounded join into another hang.
                tracing::error!(
                    "display teardown did not return within five seconds; retaining its lease in the driver worker"
                );
                return;
            }
            let _ = worker.join();
        }
        self.guard.lock().unwrap().take();
    }
}
