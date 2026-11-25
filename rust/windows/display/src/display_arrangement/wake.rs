//! Background wake watcher for monitors returning after an exclusive stream ends.

use super::{
    SETTLE_RETRY, active_ids, connected, forget_saved_layout, physical_displays, settled, state,
};
use crate::display::{Snapshot, Topology};
use anyhow::Result;
use butterpollo_media_policy::topology::Node;
use std::sync::{Arc, Condvar, Mutex};

/// How long after a user's display is switched on a drop off the list is put down to
/// its waking. Measured: 0.3 to 1.6 s.
pub(super) const WAKE_WATCH: std::time::Duration = std::time::Duration::from_secs(3);
/// How long a display that dropped gets to return. Measured: 3.1 to 6.0 s.
pub(super) const WAKE_RETURN: std::time::Duration = std::time::Duration::from_secs(10);
pub(super) const WAKE_POLL: std::time::Duration = std::time::Duration::from_millis(100);
/// What a display switched on after the stream did ([`watch_wake`]).
#[derive(Debug, PartialEq)]
pub(super) enum Wake {
    Stayed,
    CameBack,
    StayedAway,
    /// A launch or the host's stop ended the watch first: nothing more is known.
    Stopped,
}
/// Tells a watch to stop, and wakes it from its wait.
#[derive(Default)]
pub(super) struct WatchStop {
    stopped: Mutex<bool>,
    changed: Condvar,
}
impl WatchStop {
    pub(super) fn stop(&self) {
        *self.stopped.lock().unwrap() = true;
        self.changed.notify_all();
    }
    pub(super) fn is_stopped(&self) -> bool {
        *self.stopped.lock().unwrap()
    }
    /// Wait out `time`. True when stopped, as soon as that happens.
    pub(super) fn wait(&self, time: std::time::Duration) -> bool {
        let stopped = self.stopped.lock().unwrap();
        *self
            .changed
            .wait_timeout_while(stopped, time, |stopped| !*stopped)
            .unwrap()
            .0
    }
}
/// What a watch that did not finish leaves to the end of the holds ([`left_watch`]).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct LeftWatch {
    pub(super) woken: Vec<String>,
    /// Whether the user's layout goes back ([`WakeEnd::restore`]).
    pub(super) restore: bool,
}
pub(super) struct RunningWatch {
    stop: Arc<WatchStop>,
    /// Answers whether it was stopped before it finished.
    pub(super) thread: std::thread::JoinHandle<bool>,
    left: LeftWatch,
}
pub(super) struct Watches {
    /// Launches preparing a display: no watch starts, as what it would write can land
    /// on their layout.
    held_off: usize,
    /// The host is stopping: no watch starts again.
    closed: bool,
    pub(super) running: Option<RunningWatch>,
    left: Option<LeftWatch>,
}
impl Watches {
    /// Stop the watch and wait for its thread, which then has dropped the stream's
    /// display and writes nothing more. A thread that panicked did not finish either.
    fn stop_running(&mut self) {
        if let Some(RunningWatch { stop, thread, left }) = self.running.take() {
            stop.stop();
            if thread.join().unwrap_or(true) {
                self.left = Some(left);
            }
        }
    }
}
/// How [`WakeWatcher::start`] went.
#[derive(Debug, PartialEq)]
pub(super) enum WatchStart {
    Started,
    /// Not now: what it was to do is for the holds' end, or for the next start.
    Refused,
    /// No thread: nothing comes back for its journal entry.
    NoThread,
}
/// The thread a watch runs on.
type WatchThread = Box<dyn FnOnce() -> bool + Send>;
/// The one watch that can run after a stream's end, off the threads that wait for that
/// end. Its lock is held while it waits for the thread, so that whoever gets past a stop
/// finds the display gone; the thread never takes this lock.
pub(super) struct WakeWatcher(pub(super) Mutex<Watches>);
impl WakeWatcher {
    pub(super) const fn new() -> Self {
        Self(Mutex::new(Watches {
            held_off: 0,
            closed: false,
            running: None,
            left: None,
        }))
    }
    /// Run `job` on its own thread; when it may not, it is dropped unrun. It answers whether
    /// it was stopped before it finished. Never call this holding the arrangement's lock:
    /// a watch still running is waited for, and it may want that lock.
    pub(super) fn start(
        &self,
        left: LeftWatch,
        job: impl FnOnce(&WatchStop) -> bool + Send + 'static,
    ) -> WatchStart {
        self.start_on(left, job, |thread| {
            std::thread::Builder::new()
                .name("wake-watch".into())
                .spawn(thread)
        })
    }
    pub(super) fn start_on(
        &self,
        left: LeftWatch,
        job: impl FnOnce(&WatchStop) -> bool + Send + 'static,
        spawn: impl FnOnce(WatchThread) -> std::io::Result<std::thread::JoinHandle<bool>>,
    ) -> WatchStart {
        let mut watches = self.0.lock().unwrap();
        if watches.closed {
            return WatchStart::Refused;
        }
        if watches.held_off != 0 {
            // Here and not by the caller: `job` goes once this lock has, and its display
            // takes a while. The hold can end during that.
            watches.left = Some(left);
            return WatchStart::Refused;
        }
        watches.stop_running();
        watches.left = None;
        let stop = Arc::new(WatchStop::default());
        let stops = stop.clone();
        match spawn(Box::new(move || job(&stops))) {
            Ok(thread) => {
                watches.running = Some(RunningWatch { stop, thread, left });
                WatchStart::Started
            }
            Err(error) => {
                tracing::warn!(%error, "the watch over a woken display could not start");
                WatchStart::NoThread
            }
        }
    }
    /// No watch runs or starts until as many [`Self::release`]s.
    pub(super) fn hold_off(&self) {
        let mut watches = self.0.lock().unwrap();
        watches.held_off += 1;
        watches.stop_running();
    }
    /// For the last hold to end: what a watch held off or stopped left. No journal entry
    /// goes on that alone ([`left_watch`]).
    pub(super) fn release(&self) -> Option<LeftWatch> {
        let mut watches = self.0.lock().unwrap();
        watches.held_off -= 1;
        if watches.held_off != 0 || watches.closed {
            return None;
        }
        watches.left.take()
    }
    /// For good: the host is stopping. What a stopped watch left stays in the journal,
    /// for the next start to put back.
    pub(super) fn close(&self) {
        let mut watches = self.0.lock().unwrap();
        watches.closed = true;
        watches.stop_running();
    }
    /// Whether a watch is at work, or left work for the holds' end.
    pub(super) fn settling(&self) -> bool {
        let watches = self.0.lock().unwrap();
        watches.left.is_some()
            || watches
                .running
                .as_ref()
                .is_some_and(|watch| !watch.thread.is_finished())
    }
}
static WAKE_WATCHER: WakeWatcher = WakeWatcher::new();
/// Holds off the watch over a woken display while a launch prepares a display: a running
/// one drops the stream's display at once and writes nothing more, and none starts. Take
/// it before anything looks at the displays: a watched display still reads as live.
#[must_use]
pub struct WakeWatchOff(());
pub fn hold_off_wake_watch() -> WakeWatchOff {
    WAKE_WATCHER.hold_off();
    WakeWatchOff(())
}
impl Drop for WakeWatchOff {
    fn drop(&mut self) {
        if let Some(left) = WAKE_WATCHER.release() {
            start_left_watch(left);
        }
    }
}
/// The host is stopping: no watch outlasts that, or starts.
pub fn close_wake_watch() {
    WAKE_WATCHER.close();
}
/// Whether a stream's end is still at work on the displays, up to about 13 s after that
/// end, though no stream or teardown shows it. What is on can be anything until then:
/// a layout read now is nobody's, and one written now can be undone by it.
pub fn wake_watch_settling() -> bool {
    WAKE_WATCHER.settling()
}
/// The user's displays that are connected and off: switching them on wakes them. None
/// without the stream's own display, as no display is left to hold the layout then.
pub(super) fn displays_to_wake(
    before: &[Node],
    active: &[Node],
    connected: &[String],
    target: &str,
) -> Vec<String> {
    if !connected.iter().any(|id| id == target) {
        return Vec::new();
    }
    let on_now = |id: &String| active.iter().any(|a| a.active && a.device_id == *id);
    before
        .iter()
        .filter(|n| {
            n.active
                && n.device_id != target
                && connected.contains(&n.device_id)
                && !on_now(&n.device_id)
        })
        .map(|n| n.device_id.clone())
        .collect()
}
/// What Windows could switch on if the stream's display went while a woken display is
/// away: `others`, what is connected besides Rubylight's displays, that isn't in the
/// user's layout. A display whose path can't be read counts: being wrong costs a watch.
pub(super) fn displays_it_could_switch_on(before: &[Node], others: &[String]) -> Vec<String> {
    others
        .iter()
        .filter(|id| !before.iter().any(|n| n.active && n.device_id == **id))
        .cloned()
        .collect()
}
/// Watch `woken`, just switched on. A monitor that slept for want of a signal can show
/// a picture, drop off the list about a second later and return 3 to 6 s after that.
/// `away` runs at each look while one is gone. `read` failing means Windows is still
/// applying a change, which says nothing about them. `sleep` answers true when the watch
/// was stopped.
pub(super) fn watch_wake(
    woken: &[String],
    mut read: impl FnMut() -> Result<Vec<String>>,
    mut elapsed: impl FnMut() -> std::time::Duration,
    mut sleep: impl FnMut() -> bool,
    mut away: impl FnMut(),
) -> Wake {
    let mut looks = WakeLooks::default();
    loop {
        let now = elapsed();
        let present = read().ok().map(|c| all_listed(woken, &c));
        match looks.look(now, present) {
            WakeLook::Done(wake) => return wake,
            WakeLook::Away => away(),
            WakeLook::Watching => {}
        }
        if sleep() {
            return Wake::Stopped;
        }
    }
}
pub(super) fn all_listed(woken: &[String], connected: &[String]) -> bool {
    woken.iter().all(|id| connected.contains(id))
}
/// What one look at the displays says.
#[derive(Debug, PartialEq)]
pub(super) enum WakeLook {
    Watching,
    Away,
    Done(Wake),
}
/// What the looks at a woken display have seen.
#[derive(Debug, Default)]
pub(super) struct WakeLooks {
    pub(super) gone_since: Option<std::time::Duration>,
}
impl WakeLooks {
    /// `present`: whether they are all listed; None when that can't be read.
    pub(super) fn look(&mut self, now: std::time::Duration, present: Option<bool>) -> WakeLook {
        match (self.gone_since, present) {
            (None, Some(false)) => self.gone_since = Some(now),
            (Some(_), Some(true)) => return WakeLook::Done(Wake::CameBack),
            _ => {}
        }
        match self.gone_since {
            None if now >= WAKE_WATCH => WakeLook::Done(Wake::Stayed),
            Some(at) if now.saturating_sub(at) >= WAKE_RETURN => WakeLook::Done(Wake::StayedAway),
            Some(_) => WakeLook::Away,
            None => WakeLook::Watching,
        }
    }
}
/// What is on that the user's layout doesn't have, known physical (a stream's virtual
/// display is never one of them), and what stays on without it. `keep`: what someone
/// holds now, which a physical stream's display can be.
pub(super) fn strays(
    before: &[Node],
    active: &[String],
    physical: &[String],
    keep: &[String],
) -> (Vec<String>, Vec<String>) {
    active.iter().cloned().partition(|id| {
        physical.contains(id)
            && !keep.contains(id)
            && !before.iter().any(|n| n.active && n.device_id == *id)
    })
}
/// Whether the entry for what is connected may have only `on` on. Not with a display of
/// the user's connected and off: Windows would recall that as their layout.
pub(super) fn strays_off_saves(before: &[Node], on: &[String], connected: &[String]) -> bool {
    before
        .iter()
        .filter(|n| n.active && connected.contains(&n.device_id))
        .all(|n| on.contains(&n.device_id))
}
/// Windows recalled the entry for what is connected while a woken display is away, and
/// that can have a display on that was off before the stream, such as a TV. Off again,
/// and saved so that the entry no longer has it on, if it `saves`.
pub(super) fn switch_off_strays(before: &[Node], keep: &[String], saves: bool) -> Result<()> {
    let active: Vec<_> = Topology::query()?
        .monitors()
        .into_iter()
        .map(|m| m.device_id)
        .collect();
    let (off, on) = strays(before, &active, &physical_displays()?, keep);
    if off.is_empty() || on.is_empty() {
        return Ok(());
    }
    tracing::info!(displays = ?off, "Windows switched on a display the user's layout doesn't have while their monitor is away; switching it off");
    Topology::set_active(&on)?;
    if saves && strays_off_saves(before, &on, &connected()?) {
        Topology::save_current()?;
    }
    Ok(())
}
/// Whether what is on is the user's layout again: each of their displays that is connected
/// on at its mode, the primary theirs, nothing else physical on but what is held (`keep`).
pub(super) fn layout_is_back(
    before: &[Node],
    current: &[Node],
    connected: &[String],
    physical: &[String],
    keep: &[String],
) -> bool {
    let active: Vec<_> = active_ids(current).into_iter().collect();
    !butterpollo_media_policy::display_policy::recalled_over(before, current, connected, &[], true)
        && strays(before, &active, physical, keep).0.is_empty()
}
/// The user's layout, for what a stopped watch left. What is already right isn't touched,
/// as applying a layout again can retime a display; what is held now stays.
fn put_left_layout_back(before: &Snapshot) -> Result<()> {
    let leased = crate::display::leased_displays();
    let current = Snapshot::capture()?.nodes;
    let connected = connected()?;
    if layout_is_back(
        &before.nodes,
        &current,
        &connected,
        &physical_displays()?,
        &leased,
    ) {
        return Ok(());
    }
    tracing::info!("putting the user's layout back after a stream's end was cut short");
    present_layout(before, &connected).restore_excluding(&leased)
}
/// `before` for what is `connected`, for when a watch is done waiting. With the display at
/// the origin gone for good, the others would have nothing there, which Windows refuses
/// (invalid parameter) for as long as it is gone, and the journal entry would sit.
pub(super) fn present_layout(before: &Snapshot, connected: &[String]) -> Snapshot {
    let mut layout = before.clone();
    layout.nodes.retain(|n| connected.contains(&n.device_id));
    let on = || layout.nodes.iter().filter(|n| n.active);
    let at_origin = on().any(|n| (n.desired_position.x, n.desired_position.y) == (0, 0));
    let first = on()
        .map(|n| (n.desired_position.x, n.desired_position.y))
        .min();
    if let Some((x, y)) = first.filter(|_| !at_origin) {
        for node in &mut layout.nodes {
            node.desired_position.x = node.desired_position.x.saturating_sub(x);
            node.desired_position.y = node.desired_position.y.saturating_sub(y);
        }
    }
    layout
}
/// The watch's last part with no lease to take it up, the stream's display gone: a launch
/// that failed or took no lease (a configured window, a physical stream with no layout)
/// cut it short, or a stream's end during a launch never got one. With the monitor away
/// Windows can have lit what is left, so the layout goes back once it stayed or returned,
/// and only that clears the journal. If that fails the entry stays, for the next stream
/// or start. True when stopped before it finished: then the same goes for the next hold's
/// end. This runs beside what follows: nothing here delays a launch's answer.
pub(super) fn left_watch(
    left: LeftWatch,
    stop: &WatchStop,
    read: impl FnMut() -> Result<Vec<String>>,
    switch_off: impl FnMut() -> Result<()>,
    restore: impl FnMut() -> Result<()>,
    forget: impl FnOnce() -> Result<()>,
) -> bool {
    run_wake_watch(
        WakeEnd {
            woken: left.woken,
            restore: left.restore,
            // No entry to put back with the stream's display gone, and whether the layout
            // is right is read when it is time to look ([`put_left_layout_back`]).
            first: Ok(()),
            restored: Err(anyhow::anyhow!("a stopped watch left it")),
        },
        stop,
        read,
        switch_off,
        || Ok(()),
        restore,
        forget,
    )
}
/// Start [`left_watch`]. A lease that took the journal entry up has it from here on.
pub(super) fn start_left_watch(left: LeftWatch) {
    let job = left.clone();
    WAKE_WATCHER.start(left, move |stop| {
        crate::desktop::on_input_desktop(move || {
            let before = match watch_step(stop, crate::display_recovery::pending_arrangement) {
                None => return stop.is_stopped(),
                Some(Ok(Some(before))) => before,
                Some(Ok(None)) => return false,
                Some(Err(error)) => {
                    tracing::warn!(error = %format!("{error:#}"), "display arrangement restoration remains pending");
                    return false;
                }
            };
            left_watch(
                job,
                stop,
                connected,
                || switch_off_strays(&before.nodes, &crate::display::leased_displays(), true),
                || put_left_layout_back(&before),
                || crate::display_recovery::arrangement(None),
            )
        })
    });
}
/// The user's displays to watch at a stream's end: the ones it wakes, if Windows could
/// switch on something else with the stream's display gone. With nothing else to switch
/// on there is nothing to protect, whatever a woken display does.
pub(super) fn woken_to_watch(
    before: &[Node],
    active: impl FnOnce() -> Result<Vec<Node>>,
    connected: &[String],
    others: &[String],
    target: &str,
) -> Result<Vec<String>> {
    if displays_it_could_switch_on(before, others).is_empty() {
        return Ok(Vec::new());
    }
    Ok(displays_to_wake(before, &active()?, connected, target))
}
/// [`woken_to_watch`] for what is connected now.
pub(super) fn displays_to_watch(before: &Snapshot, target: &str) -> Vec<String> {
    settled(|| {
        let monitors = Topology::query_all()?.available_monitors();
        let others: Vec<String> = monitors
            .iter()
            .filter(|m| !m.is_rubylight())
            .map(|m| m.device_id.clone())
            .collect();
        let connected: Vec<String> = monitors.into_iter().map(|m| m.device_id).collect();
        woken_to_watch(
            &before.nodes,
            || Ok(Snapshot::capture()?.nodes),
            &connected,
            &others,
            target,
        )
    })
    .unwrap_or_else(|error| {
        tracing::warn!(error = %format!("{error:#}"), "reading the displays at a stream's end failed; not watching them wake");
        Vec::new()
    })
}
/// What a stream's end leaves to its watch: the stream's display stays until `woken` have
/// shown they stay. Gone with one of them away, what is left connected can be a TV that
/// was off, which Windows then switches on alone. All this runs beside whatever comes
/// next, and whatever comes next stops it first ([`WakeWatcher`]).
pub(super) struct WakeEnd {
    pub(super) woken: Vec<String>,
    /// Whether the user's layout goes back after the entry.
    pub(super) restore: bool,
    /// How the entry and that layout went at the stream's end.
    pub(super) first: Result<()>,
    pub(super) restored: Result<()>,
}
/// One write of a watch: under the arrangement's lock, and not once it was stopped or a
/// lease began. None then, with nothing written.
pub(super) fn watch_step<T>(
    stop: &WatchStop,
    work: impl FnOnce() -> Result<T>,
) -> Option<Result<T>> {
    let state = state().lock().unwrap();
    if stop.is_stopped() || state.users != 0 {
        return None;
    }
    Some(work())
}
/// [`settled`] for a watch's write ([`watch_step`]): a stop ends it at once.
pub(super) fn settled_step<T>(
    stop: &WatchStop,
    mut attempt: impl FnMut() -> Result<T>,
) -> Option<Result<T>> {
    for _ in 0..3 {
        match watch_step(stop, &mut attempt)? {
            Ok(value) => return Some(Ok(value)),
            Err(error) => {
                tracing::debug!(error = %format!("{error:#}"), "display layout change pending; retrying");
                if stop.wait(SETTLE_RETRY) {
                    return None;
                }
            }
        }
    }
    watch_step(stop, attempt)
}
/// The writes that follow the watch. One that dropped gets its entry and layout again
/// when it returns: the first try may have ended halfway, and an entry saved while it
/// was gone isn't its entry. What `put_back` and `restore` answer None to was stopped.
/// What came of the user's layout, None if stopped.
pub(super) fn after_wake(
    woken: &[String],
    wake: Wake,
    first: Result<()>,
    restored: Result<()>,
    mut put_back: impl FnMut() -> Option<Result<()>>,
    mut restore: impl FnMut() -> Option<Result<()>>,
) -> Option<Result<()>> {
    let put = match wake {
        Wake::Stopped => return None,
        Wake::Stayed if first.is_ok() && restored.is_ok() => return Some(Ok(())),
        Wake::Stayed if first.is_ok() => first,
        Wake::Stayed => put_back()?,
        Wake::CameBack => {
            tracing::info!(displays = ?woken, "a display dropped off the list when it woke and came back; putting its layout back");
            put_back()?
        }
        Wake::StayedAway => {
            tracing::warn!(displays = ?woken, "a display dropped off the list when it woke and did not come back in time");
            first
        }
    };
    if let Err(error) = put {
        tracing::warn!(error = %format!("{error:#}"), "putting back the saved display layout failed");
    }
    restore()
}
/// A watch's thread, its displays and journal behind the arguments. True when it was
/// stopped before it finished ([`left_watch`]). The journal entry stays until the end: a
/// host that dies during the watch has its next start put the layout back, and a stream
/// that starts takes it as its baseline, as what is on by then can be what Windows lit.
pub(super) fn run_wake_watch(
    end: WakeEnd,
    stop: &WatchStop,
    read: impl FnMut() -> Result<Vec<String>>,
    mut switch_off: impl FnMut() -> Result<()>,
    mut put_back: impl FnMut() -> Result<()>,
    mut restore: impl FnMut() -> Result<()>,
    forget: impl FnOnce() -> Result<()>,
) -> bool {
    let WakeEnd {
        woken,
        restore: restore_due,
        first,
        restored,
    } = end;
    let start = std::time::Instant::now();
    let mut failed_away = false;
    let wake = watch_wake(
        &woken,
        read,
        || start.elapsed(),
        || stop.wait(WAKE_POLL),
        || {
            if let Some(Err(error)) = watch_step(stop, &mut switch_off)
                && !std::mem::replace(&mut failed_away, true)
            {
                tracing::warn!(error = %format!("{error:#}"), "switching off a display Windows recalled while the user's monitor is away failed");
            }
        },
    );
    let layout = after_wake(
        &woken,
        wake,
        first,
        restored,
        || settled_step(stop, &mut put_back),
        || {
            if restore_due {
                settled_step(stop, &mut restore)
            } else {
                Some(Ok(()))
            }
        },
    );
    let cleared = layout.and_then(|result| match result {
        Ok(()) => watch_step(stop, forget),
        Err(error) => Some(Err(error)),
    });
    match cleared {
        Some(Ok(())) => false,
        Some(Err(error)) => {
            tracing::warn!(error = %format!("{error:#}"), "display arrangement restoration remains pending");
            false
        }
        None => {
            tracing::info!(displays = ?woken, "a launch or the host's stop ended the watch over a woken display");
            true
        }
    }
}
/// Start the watch, which owns what `remains` of the stream: its display above all.
pub(super) fn start_wake_watch(
    before: Snapshot,
    target: String,
    end: WakeEnd,
    remains: Option<Box<dyn Send>>,
    baseline: Option<crate::display::end_trace::Baseline>,
) {
    let layout_back = end.restored.is_ok();
    let left = LeftWatch {
        woken: end.woken.clone(),
        restore: end.restore,
    };
    let start = WAKE_WATCHER.start(left, move |stop| {
        tracing::info!(displays = ?end.woken, "keeping the stream's display in the background until the woken display has stayed or returned");
        crate::desktop::on_input_desktop(move || {
            let left = run_wake_watch(
                end,
                stop,
                connected,
                || switch_off_strays(&before.nodes, &[], true),
                || forget_saved_layout(&before, &target, true),
                || connected().and_then(|c| present_layout(&before, &c).restore()),
                || crate::display_recovery::arrangement(None),
            );
            drop(remains);
            // Issue #129: the stream's display is gone only now.
            crate::display::end_trace::settle_later(baseline);
            left
        })
    });
    if entry_goes_unwatched(&start, layout_back) {
        let state = state().lock().unwrap();
        if state.users == 0
            && let Err(error) = crate::display_recovery::arrangement(None)
        {
            tracing::warn!(error = %format!("{error:#}"), "display arrangement journal cleanup failed");
        }
    }
}
/// With no thread for its watch nothing comes back for a stream's journal entry, which
/// would turn up as the baseline of a stream much later. It goes if the layout went back,
/// as at an end with no watch; if not, it is what the next stream or start puts back.
pub(super) fn entry_goes_unwatched(start: &WatchStart, layout_back: bool) -> bool {
    *start == WatchStart::NoThread && layout_back
}
