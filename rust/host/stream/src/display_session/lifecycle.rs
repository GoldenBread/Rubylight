//! Active display lease lifecycle: [`StreamPreparation`], [`Ready`], [`CaptureTarget`],
//! pause/resume transitions, settings updates, and virtual display [`retime`].
use super::Prepared;
use super::apply::{applied_mode, retry_refused_layout};
use super::mode_report::ModeReport;
use super::policy::{
    frame_policy, joined_report, profile_choice, resumed_profile, stream_mode, take_profile,
};
use super::scale::holds_layout_while_paused;
use crate::state::{Launch, Shared};
use anyhow::{Context, Result};
use butterpollo_config::config::Config;
use butterpollo_media_policy::{
    display_policy::HostDisplay,
    display_session_machine::{display_serves, display_shows_game, mode_resumes},
    framegen::{Policy, Rate},
    rtsp::Negotiated,
};
use butterpollo_session::session::{Preparation, Warnings};
use butterpollo_win_display::{
    display::{Guard, Retained},
    display_arrangement, hdr_profile,
};
use butterpollo_win_io::limiter;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

/// Only pending and connected streams own limiter changes. The retained game
/// display must not keep a global frame cap active after transport disconnects.
pub struct StreamPreparation {
    pub display: Arc<Ready>,
    _limiter: limiter::Lease,
}

impl Preparation<StreamPreparation> for StreamPreparation {
    fn prepared(&self) -> &Self {
        self
    }
    fn into_prepared(self: Box<Self>) -> Self {
        *self
    }
}

impl StreamPreparation {
    pub fn report_limiter(
        &self,
        warnings: &butterpollo_session::session::Warnings,
        config: &butterpollo_config::config::Config,
    ) {
        let explicit = config.boolean("frame_limiter_enable", false);
        let framegen = self.display.framegen.lock().unwrap().clone();
        if let Some(message) = self._limiter.warning(&framegen, explicit) {
            warnings.set("display_limiter", message);
        } else {
            warnings.clear("display_limiter");
        }
    }
}

impl Drop for StreamPreparation {
    fn drop(&mut self) {
        {
            // Capture the choice at the last transport's disconnect, before
            // a paused lease can observe unrelated local Win+P changes. The
            // state lock also serializes this boundary with resume's reapply.
            let mut prepared = self.display._state.lock().unwrap();
            let prev_active = self.display.active.fetch_sub(1, Ordering::AcqRel);
            let detach = prepared.machine.detach_stream();
            debug_assert_eq!(prev_active == 1, detach.capture_chosen_preferences);
            debug_assert_eq!(prev_active.saturating_sub(1), detach.phase.active_streams());
            if detach.capture_chosen_preferences {
                prepared.keep_chosen_layout();
                prepared.keep_chosen_resolution();
                prepared.keep_chosen_scale();
            }
        }
        if let Some(retained) = &self.display.retained {
            retained.stream_stopped();
        }
    }
}

/// A game may keep its display across transport disconnects. The heartbeat
/// owns the resources, not the Ready Arc, so final teardown cannot form a cycle.
pub struct Ready {
    // Retain the leases until after the worker has joined. Streaming readers
    // use the published target, never the lock held over native display I/O.
    pub(super) _state: Arc<Mutex<Prepared>>,
    target: CaptureTarget,
    pub(super) mode: (u32, u32, u32, bool, bool),
    client_display: Option<HostDisplay>,
    virtual_layout: Option<String>,
    pub(super) exclusive_layout: bool,
    /// The identity of the virtual display it streams, and whether that is a
    /// running game's it joined ([`super::record_game_display`]).
    pub(super) identity: Option<String>,
    pub(super) joined: bool,
    /// One game window's stream (`Ready::window`): never kept for a
    /// reconnect, whose launch prepares and checks the window again.
    game_window: bool,
    /// The launch's policy, or the stream request's once `retime` raised it.
    pub(super) framegen: Mutex<Policy>,
    mode_report: ModeReport,
    /// The size a stream turned to, for the heartbeat to give its display.
    turned: Arc<Mutex<Option<(u32, u32)>>>,
    stop: Arc<AtomicBool>,
    pub(super) active: Arc<AtomicUsize>,
    retained: Option<Arc<Retained>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Ready {
    pub fn prepare((prepared, limiter): (Prepared, limiter::Lease)) -> Result<StreamPreparation> {
        let target = CaptureTarget::new(prepared.capture_target());
        let mode = prepared.mode;
        let client_display = prepared.client_display;
        let virtual_layout = prepared.virtual_layout.clone();
        let exclusive_layout = prepared
            ._arrangement
            .as_ref()
            .is_some_and(display_arrangement::Lease::exclusive);
        let framegen = prepared.framegen.clone();
        let mode_report = prepared.mode_report.clone();
        let retained = prepared._retained.clone();
        let identity = prepared.identity.clone();
        let joined = prepared.joined;
        let state = Arc::new(Mutex::new(prepared));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_state = state.clone();
        let worker_target = target.clone();
        let worker_stop = stop.clone();
        let turned = Arc::new(Mutex::new(None));
        let worker_turned = turned.clone();
        let active = Arc::new(AtomicUsize::new(1));
        let worker_active = active.clone();
        let worker = std::thread::Builder::new()
            .name("game-display-lease".into())
            .spawn(move || {
                let mut heartbeat =
                    butterpollo_media_policy::display_policy::HeartbeatThrottle::default();
                while !worker_stop.load(Ordering::Acquire) {
                    butterpollo_win_io::input::keep_on_input_desktop();
                    let mut prepared = worker_state.lock().unwrap();
                    let active = worker_active.load(Ordering::Acquire) != 0;
                    let result = prepared.feed(active);
                    let taken = worker_turned.lock().unwrap().take();
                    if let Some(to) = taken {
                        let followed = prepared.follow_shape(to);
                        prepared.mode_report.followed(&followed);
                    }
                    let base_tick =
                        if active || result.is_err() || prepared.machine.recovery.pending {
                            Duration::from_millis(100)
                        } else {
                            Duration::from_millis(250)
                        };
                    let current = prepared.capture_target();
                    worker_target.publish(current);
                    drop(prepared);
                    match &result {
                        Ok(()) => {
                            if heartbeat.note_ok() {
                                tracing::info!("game display heartbeat recovered");
                            }
                        }
                        Err(error) => {
                            if heartbeat.note_error(&error.to_string()) {
                                tracing::warn!(%error, "game display heartbeat failed");
                            }
                        }
                    }
                    let tick = heartbeat.next_tick(base_tick, Duration::from_secs(1));
                    std::thread::park_timeout(tick);
                }
            })?;
        if let Some(retained) = &retained {
            retained.stream_started();
        }
        let display = Arc::new(Self {
            _state: state,
            target,
            mode,
            client_display,
            virtual_layout,
            exclusive_layout,
            identity,
            joined,
            game_window: false,
            framegen: Mutex::new(framegen),
            mode_report,
            turned,
            stop,
            active,
            retained,
            worker: Some(worker),
        });
        Ok(StreamPreparation {
            display,
            _limiter: limiter,
        })
    }

    /// A stream of one game window, a Nucleus Co-op player's: no display is
    /// prepared, changed or kept, and no frame limit is set.
    pub fn window(
        h: &Shared,
        stream: &Negotiated,
        config: &Config,
        first: String,
        mut follow: Box<dyn FnMut() -> Result<String> + Send>,
        warnings: Arc<Warnings>,
    ) -> Result<StreamPreparation> {
        let mut framegen = Policy::resolve(
            config,
            Rate(stream.fps_millihz()),
            false,
            "none",
            false,
            false,
        )?;
        framegen.capture = "wgc".into();
        framegen.enabled = false;
        let limiter = limiter::Lease::acquire(&h.directory, config, &framegen)?;
        let target = CaptureTarget::new((first.clone(), 0));
        let mode_report =
            ModeReport::new(warnings.clone(), (None, None, None), stream.fps_millihz());
        let prepared = Prepared::bare(
            h,
            first.clone(),
            framegen.clone(),
            stream_mode(stream),
            mode_report.clone(),
        );
        let stop = Arc::new(AtomicBool::new(false));
        let worker = std::thread::Builder::new()
            .name("game-window".into())
            .spawn({
                let (target, stop) = (target.clone(), stop.clone());
                move || {
                    let mut last = first;
                    while !stop.load(Ordering::Acquire) {
                        match follow() {
                            Ok(next) => {
                                if next != last {
                                    tracing::info!(from = %last, to = %next, "the stream's game window was replaced");
                                    target.publish((next.clone(), 0));
                                    last = next;
                                }
                                warnings.clear("capture_window");
                            }
                            Err(error) => warnings.set(
                                "capture_window",
                                format!("{error:#}; the stream waits for it to return"),
                            ),
                        }
                        std::thread::park_timeout(Duration::from_millis(100));
                    }
                }
            })?;
        let display = Arc::new(Self {
            _state: Arc::new(Mutex::new(prepared)),
            target,
            mode: stream_mode(stream),
            client_display: None,
            virtual_layout: None,
            exclusive_layout: false,
            identity: None,
            joined: false,
            game_window: true,
            framegen: Mutex::new(framegen),
            mode_report,
            turned: Arc::new(Mutex::new(None)),
            stop,
            active: Arc::new(AtomicUsize::new(1)),
            retained: None,
            worker: Some(worker),
        });
        Ok(StreamPreparation {
            display,
            _limiter: limiter,
        })
    }

    pub fn resume(
        self: Arc<Self>,
        directory: &std::path::Path,
        config: &Config,
        warnings: Arc<Warnings>,
    ) -> Result<StreamPreparation> {
        let framegen = self.framegen.lock().unwrap().clone();
        let limiter = limiter::Lease::acquire(directory, config, &framegen)?;
        retry_refused_layout(|| {
            butterpollo_win_io::input::on_input_desktop(|| -> Result<()> {
                let mut prepared = self._state.lock().unwrap();
                let fed = prepared.display.as_mut().map_or(Ok(false), Guard::resume);
                prepared.finish_feed(fed)?;
                if let Some(arrangement) = &prepared._arrangement {
                    prepared.lay_out_again(arrangement)?;
                    if let Some(display) = &prepared.display {
                        display.apply_virtual_mode("after resume layout");
                    }
                    self.mode_report.publish(applied_mode(&prepared.output));
                }
                prepared.put_scale_back()?;
                self.target.publish(prepared.capture_target());
                self.active.fetch_add(1, Ordering::AcqRel);
                prepared.machine.attach_stream();
                Ok(())
            })
        })?;
        self.mode_report.attach(warnings);
        if let Some(retained) = &self.retained {
            retained.stream_started();
        }
        if let Some(worker) = &self.worker {
            worker.thread().unpark();
        }
        Ok(StreamPreparation {
            display: self,
            _limiter: limiter,
        })
    }

    pub fn note_hdr_fallback(&self) {
        self.mode_report.hdr_fell_back();
    }

    pub fn clear_hdr_fallback(&self) {
        self.mode_report.hdr_restored();
    }

    pub fn follow_settings(
        &self,
        app: Option<&butterpollo_session::state::App>,
        launch: &Launch,
        stream: &Negotiated,
        config: &Config,
    ) -> Result<()> {
        let virtual_mode = self._state.lock().unwrap().virtual_mode;
        let (framegen, _) = frame_policy(app, launch, stream, config, virtual_mode)?;
        let choice = profile_choice(launch);
        let mut prepared = self._state.lock().unwrap();
        prepared.framegen = framegen.clone();
        *self.framegen.lock().unwrap() = framegen;
        prepared.machine.hold_layout_while_paused = holds_layout_while_paused(
            config,
            prepared
                ._arrangement
                .as_ref()
                .is_some_and(display_arrangement::Lease::exclusive),
        );
        let applies = prepared.mode.3 && !prepared.joined;
        let Some(profile) = resumed_profile(
            applies,
            prepared.profile_choice.as_deref(),
            choice.as_deref(),
        ) else {
            return Ok(());
        };
        tracing::info!(output = %prepared.output, profile = ?profile, "the device's HDR profile changed while its stream was paused; the resume applies it");
        prepared.recovery_profile = profile.clone();
        let state: &mut Prepared = &mut prepared;
        let applied = butterpollo_win_io::input::on_input_desktop(|| {
            take_profile(
                &mut state._profile,
                profile.as_deref(),
                |held, profile| match held {
                    Some(lease) => lease.switch(&state.output, profile).map(|()| None),
                    None => hdr_profile::Lease::acquire(&state.output, profile).map(Some),
                },
            )
        });
        applied.context("the HDR profile could not be applied on resume")?;
        prepared.profile_choice = choice;
        Ok(())
    }

    pub fn kept_for_reconnect(&self) -> bool {
        !self.game_window
    }

    pub fn matches(&self, stream: &Negotiated) -> bool {
        self.mode == stream_mode(stream)
    }

    pub fn resumes(&self, stream: &Negotiated) -> bool {
        mode_resumes(self.mode, stream_mode(stream))
    }

    pub fn serves(&self, requested: Option<HostDisplay>, layout: &str) -> bool {
        display_serves(
            self.client_display,
            self.virtual_layout.as_deref(),
            requested,
            layout,
        )
    }

    pub fn shows(&self, game: Option<&str>) -> bool {
        display_shows_game(self.identity.as_deref(), game)
    }

    pub fn output(&self) -> String {
        self.target.current().0
    }

    pub fn exclusive_layout(&self) -> bool {
        self.exclusive_layout
    }

    pub fn capture_target(&self) -> (String, u64) {
        self.target.current()
    }

    pub fn display_target(&self) -> (String, u64) {
        self.target.display.lock().unwrap().clone()
    }

    pub fn show_app_window(&self, window: Option<String>) -> bool {
        self.target.show_window(window)
    }

    pub fn capture(&self) -> String {
        self.framegen.lock().unwrap().capture.clone()
    }

    pub fn capture_method(&self) -> (String, bool) {
        let framegen = self.framegen.lock().unwrap();
        (framegen.capture.clone(), framegen.constant_rate)
    }

    pub fn follow_shape(&self, size: (u32, u32)) {
        *self.turned.lock().unwrap() = Some(size);
        if let Some(worker) = &self.worker {
            worker.thread().unpark();
        }
    }

    pub fn log_display_diagnostics(&self, stage: &str) {
        let mut prepared = self._state.lock().unwrap();
        prepared.log_display_diagnostics(stage);
    }

    pub fn display_swap_chain(&self) -> Option<bool> {
        let mut prepared = self._state.lock().unwrap();
        let monitor = prepared.display.as_mut()?.monitor_status()?;
        Some(monitor.swap_chain)
    }

    pub fn drain_display_log(&self) {
        let mut prepared = self._state.lock().unwrap();
        if let Some(display) = prepared.display.as_mut() {
            display.drain_display_log();
        }
    }
}

/// Only copies and publication hold this lock. Lease renewal, topology queries
/// and restoration must finish before publishing a new capture identity.
#[derive(Clone)]
pub(super) struct CaptureTarget {
    display: Arc<Mutex<(String, u64)>>,
    window: Arc<Mutex<Option<String>>>,
}

impl CaptureTarget {
    pub(super) fn new(target: (String, u64)) -> Self {
        Self {
            display: Arc::new(Mutex::new(target)),
            window: Arc::new(Mutex::new(None)),
        }
    }
    pub(super) fn current(&self) -> (String, u64) {
        match &*self.window.lock().unwrap() {
            Some(window) => (window.clone(), 0),
            None => self.display.lock().unwrap().clone(),
        }
    }
    pub(super) fn publish(&self, target: (String, u64)) {
        *self.display.lock().unwrap() = target;
    }
    pub(super) fn show_window(&self, window: Option<String>) -> bool {
        let mut shown = self.window.lock().unwrap();
        let changed = *shown != window;
        *shown = window;
        changed
    }
}

impl Drop for Ready {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            let _ = worker.join();
        }
        self.retained.take();
    }
}

enum Raise {
    NotOwned,
    Refused(Rate),
    Raised(u32),
}

pub fn retime(
    preparation: StreamPreparation,
    h: &Shared,
    launch: &Launch,
    stream: &Negotiated,
    config: &Config,
) -> Result<std::result::Result<StreamPreparation, StreamPreparation>> {
    let display = preparation.display.clone();
    let launch_rate = display.mode.2;
    if !display.resumes(stream) {
        tracing::info!(
            client = %launch.client.name,
            "the stream asks for another size, HDR or VRR than its display has; it gets a new display"
        );
        return Ok(Err(preparation));
    }
    let (virtual_mode, joined) = {
        let prepared = display._state.lock().unwrap();
        (prepared.virtual_mode, prepared.joined)
    };
    let app = crate::stream::launch_app(h, launch);
    let (framegen, device_mode) =
        frame_policy(app.as_ref(), launch, stream, config, virtual_mode || joined)?;
    let raised = butterpollo_win_io::input::on_input_desktop(|| -> Result<Raise> {
        let mut prepared = display._state.lock().unwrap();
        if !prepared.joined
            && (!prepared.virtual_mode
                || prepared
                    .display
                    .as_ref()
                    .is_none_or(|guard| !guard.is_virtual()))
        {
            return Ok(Raise::NotOwned);
        }
        let (display_width, display_height) = prepared.recovery_dimensions;
        let request = butterpollo_media_policy::display_policy::display_request_rate(
            config,
            display_width,
            display_height,
            framegen.display_rate,
            stream.hdr,
            prepared.virtual_mode || prepared.joined,
        )?;
        let rate = if device_mode.is_some() {
            framegen.display_rate
        } else {
            Rate(request.refresh.unwrap_or(framegen.display_rate.0))
        };
        let (applied, requested) = if prepared.joined {
            let applied = applied_mode(&prepared.output).map_or(rate.0, |actual| actual.2);
            (applied, joined_report(rate, request.hdr))
        } else {
            let Some(guard) = &prepared.display else {
                return Ok(Raise::NotOwned);
            };
            let Some(applied) = guard.raise_virtual_rate(rate) else {
                return Ok(Raise::Refused(rate));
            };
            (
                applied,
                (
                    Some((display_width, display_height)),
                    Some(applied),
                    request.hdr,
                ),
            )
        };
        prepared.framegen = framegen.clone();
        prepared.mode_report.retime(requested, stream.fps_millihz());
        prepared.mode_report.publish(applied_mode(&prepared.output));
        Ok(Raise::Raised(applied))
    })?;
    let refresh = match raised {
        Raise::Raised(refresh) => refresh,
        Raise::NotOwned => {
            tracing::info!(
                client = %launch.client.name,
                launch_millihz = launch_rate,
                stream_millihz = stream.fps_millihz(),
                "the stream asks for another rate than its display, which is no virtual display of the stream's own; it is prepared again"
            );
            return Ok(Err(preparation));
        }
        Raise::Refused(rate) => {
            tracing::warn!(
                client = %launch.client.name,
                launch_millihz = launch_rate,
                stream_millihz = stream.fps_millihz(),
                wanted = %rate,
                "the virtual display could not be raised to the stream's rate; keeping it, so the stream runs at its refresh"
            );
            return Ok(Ok(preparation));
        }
    };
    tracing::info!(
        client = %launch.client.name,
        launch_millihz = launch_rate,
        stream_millihz = stream.fps_millihz(),
        display_millihz = refresh,
        limiter_rate = %framegen.rate,
        limiter_enabled = framegen.enabled,
        "the stream asks for another rate than its launch; the display and the frame limiter follow it"
    );
    *display.framegen.lock().unwrap() = framegen.clone();
    {
        let mut prepared = display._state.lock().unwrap();
        display.active.fetch_add(1, Ordering::AcqRel);
        prepared.machine.attach_stream();
    }
    if let Some(retained) = &display.retained {
        retained.stream_started();
    }
    drop(preparation);
    match limiter::Lease::acquire(&h.directory, config, &framegen) {
        Ok(limiter) => Ok(Ok(StreamPreparation {
            display,
            _limiter: limiter,
        })),
        Err(error) => {
            {
                let mut prepared = display._state.lock().unwrap();
                display.active.fetch_sub(1, Ordering::AcqRel);
                let _ = prepared.machine.detach_stream();
            }
            if let Some(retained) = &display.retained {
                retained.stream_stopped();
            }
            Err(error)
        }
    }
}
