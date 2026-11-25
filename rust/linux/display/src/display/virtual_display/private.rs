//! The life of a private display: a pool connector connected through the broker, a mode
//! admitted, the KScreen layout applied around it, and everything put back afterwards.
//!
//! Ported from Vibepollo (Nonary/Vibepollo, GPL-3.0), src/platform/linux/private_display.cpp
//! and private_display_mode_client.cpp: connect, wait for KWin to list the output and the
//! mode, snapshot before the first change, apply once and verify `currentModeId`, restore the
//! snapshot and disconnect on teardown. The snapshot is also written to a state file so that
//! a crashed host restores the desktop at its next start.
//!
//! Rubylight's own pool (stock VKMS) lists no custom modes: the mode is added to the output as
//! a KWin custom mode and removed again when the display goes. The snapshot is taken before
//! the connector is plugged in, as KWin turns every connected monitor on and moves them when
//! it meets a set of monitors it has no layout saved for; the first display's apply puts them
//! back as they were.
use super::broker::{self, Broker};
use super::kscreen::{self, Layout};
use super::pool::{self, Connector, Kind};
use anyhow::{Context, Result, anyhow, bail};
use butterpollo_media_policy::display_policy::Arrangement;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// At most this many custom modes are removed from an output; KWin keeps one per mode added.
const CUSTOM_MODES: usize = 8;

/// How a private display is asked for.
#[derive(Clone, Debug, PartialEq)]
pub struct PrivateRequest {
    /// The identity that picks the same connector again for the same client.
    pub stable_id: String,
    pub width: u32,
    pub height: u32,
    /// The refresh in millihertz.
    pub refresh_millihz: u32,
    pub arrangement: Arrangement,
    /// The display scale in percent; None leaves KWin's default.
    pub scale_percent: Option<u32>,
}

/// What the lifecycle needs of KScreen.
pub trait Desktop: Send + Sync {
    fn query(&self) -> Result<Layout>;
    fn apply(&self, args: &[String]) -> Result<()>;
}

/// `kscreen-doctor` itself.
struct KScreen;

impl Desktop for KScreen {
    fn query(&self) -> Result<Layout> {
        kscreen::query()
    }
    fn apply(&self, args: &[String]) -> Result<()> {
        kscreen::apply(args)
    }
}

#[cfg(test)]
pub(super) use self::persisted_for_tests::read_saved;

/// The state file: what to restore after a crash.
#[derive(Debug, Serialize, Deserialize)]
struct Persisted {
    restore_pending: bool,
    connectors: Vec<String>,
    snapshot: Layout,
}

/// The state file's place: `${XDG_STATE_HOME:-$HOME/.local/state}/rubylight/private-display.json`.
pub fn state_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|h| PathBuf::from(h).join(".local/state"))
                .filter(|p| p.is_absolute())
        })?;
    Some(base.join("rubylight/private-display.json"))
}

struct Live {
    stable_id: String,
    connector: String,
    kind: Kind,
    mode: (u32, u32, u32),
    handles: usize,
}

#[derive(Default)]
struct State {
    live: Vec<Live>,
    /// The layout before the first live display changed anything.
    original: Option<Layout>,
}

/// The private displays of this process and the places they touch.
pub struct Manager {
    /// Each pool's broker, preferred first.
    pub(super) brokers: Vec<(Kind, Broker)>,
    pub(super) drm_root: PathBuf,
    desktop: Box<dyn Desktop>,
    state_file: Option<PathBuf>,
    poll: Duration,
    wait: Duration,
    ops: Mutex<()>,
    state: Mutex<State>,
}

/// The manager of this process, on the real broker, sysfs and KScreen.
pub fn system() -> &'static Arc<Manager> {
    static SYSTEM: std::sync::OnceLock<Arc<Manager>> = std::sync::OnceLock::new();
    SYSTEM.get_or_init(|| {
        Arc::new(Manager::with_brokers(
            Kind::ALL.into_iter().map(|k| (k, Broker::of(k))).collect(),
            PathBuf::from(pool::DRM_ROOT),
            Box::new(KScreen),
            state_path(),
        ))
    })
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Manager {
    /// A manager of Vibepollo's pool alone, on `broker`.
    #[cfg(test)]
    pub fn new(
        broker: Broker,
        drm_root: PathBuf,
        desktop: Box<dyn Desktop>,
        state_file: Option<PathBuf>,
    ) -> Self {
        Self::with_brokers(
            vec![(Kind::Vibepollo, broker)],
            drm_root,
            desktop,
            state_file,
        )
    }

    pub fn with_brokers(
        brokers: Vec<(Kind, Broker)>,
        drm_root: PathBuf,
        desktop: Box<dyn Desktop>,
        state_file: Option<PathBuf>,
    ) -> Self {
        Self {
            brokers,
            drm_root,
            desktop,
            state_file,
            poll: Duration::from_millis(50),
            wait: Duration::from_secs(3),
            ops: Mutex::new(()),
            state: Mutex::new(State::default()),
        }
    }

    /// Shorter waits, for a test whose desktop answers at once or never.
    #[cfg(test)]
    pub(super) fn with_timing(mut self, poll: Duration, wait: Duration) -> Self {
        self.poll = poll;
        self.wait = wait;
        self
    }

    fn broker(&self, kind: Kind) -> Result<&Broker> {
        self.brokers
            .iter()
            .find(|(k, _)| *k == kind)
            .map(|(_, b)| b)
            .ok_or_else(|| anyhow!("no broker for the {kind:?} pool"))
    }

    /// Whether a private display can be made now: a pool with connectors whose broker this
    /// process may use, or why not (the preferred pool's reason).
    pub fn availability(&self) -> Result<(), String> {
        let connectors = pool::scan(&self.drm_root);
        let mut reason = None;
        for (kind, broker) in &self.brokers {
            if !connectors.iter().any(|c| c.kind == *kind) {
                continue;
            }
            match super::broker_usable(*kind, broker) {
                Ok(()) => return Ok(()),
                Err(why) => {
                    reason.get_or_insert(why);
                }
            }
        }
        Err(reason.unwrap_or_else(|| {
            if self.brokers.iter().any(|(k, _)| *k == Kind::Rubylight) {
                "there is no virtual display pool: start Rubylight's (systemctl enable --now rubylight-vdisplay-pool.service)".into()
            } else {
                "Vibepollo's virtual display module is not loaded".into()
            }
        }))
    }

    /// The mode the live display `stable_id` runs at.
    pub fn joinable_mode(&self, stable_id: &str) -> Option<(u32, u32, u32)> {
        lock(&self.state)
            .live
            .iter()
            .find(|l| l.stable_id == stable_id)
            .map(|l| l.mode)
    }

    /// Make the private display `request` asks for.
    pub fn create(self: &Arc<Self>, request: &PrivateRequest) -> Result<super::VirtualDisplay> {
        if !broker::valid_mode(request.width, request.height, request.refresh_millihz) {
            bail!(
                "a private display of {}x{} at {} mHz is outside what the broker admits",
                request.width,
                request.height,
                request.refresh_millihz
            );
        }
        let _ops = lock(&self.ops);
        let (connector, kind, reused) = self.pick(&request.stable_id)?;
        let broker = self.broker(kind)?;
        self.save_original()?;
        tracing::info!(connector = %connector, pool = ?kind, stable_id = %request.stable_id, "connecting a private display");
        if let Err(error) = broker.connect(&connector) {
            self.forget_unused_original();
            return Err(error).with_context(|| format!("could not connect {connector}"));
        }
        match self.bring_up(&connector, kind, request) {
            Ok(()) => {
                let mut state = lock(&self.state);
                let mode = (request.width, request.height, request.refresh_millihz);
                if reused {
                    if let Some(live) = state.live.iter_mut().find(|l| l.connector == connector) {
                        live.handles += 1;
                        live.mode = mode;
                    }
                } else {
                    state.live.push(Live {
                        stable_id: request.stable_id.clone(),
                        connector: connector.clone(),
                        kind,
                        mode,
                        handles: 1,
                    });
                }
                drop(state);
                self.persist();
                tracing::info!(connector = %connector, "the private display is on");
                Ok(super::VirtualDisplay::live(
                    self.clone(),
                    request.stable_id.clone(),
                    connector,
                ))
            }
            Err(error) => {
                tracing::warn!(connector = %connector, error = %format!("{error:#}"), "the private display could not be set up; undoing it");
                self.roll_back(&connector, kind, reused);
                Err(error)
            }
        }
    }

    /// The layout before this process changes anything, taken while no display of its own is
    /// live and before the connector is plugged in; outputs of the pools are left out.
    fn save_original(&self) -> Result<()> {
        if lock(&self.state).original.is_some() {
            return Ok(());
        }
        let mut snapshot = self
            .desktop
            .query()
            .context("could not read the desktop layout before the private display")?;
        snapshot.outputs.retain(|o| !broker::managed_name(&o.name));
        tracing::info!(outputs = snapshot.outputs.len(), "saved the desktop layout");
        lock(&self.state).original = Some(snapshot);
        Ok(())
    }

    /// A display that never came: with none live, the saved layout is not needed.
    fn forget_unused_original(&self) {
        let mut state = lock(&self.state);
        if state.live.is_empty() {
            state.original = None;
        }
    }

    /// The connector for `stable_id`: the one it already holds, else the first free one of a
    /// pool this process may use, Rubylight's first.
    fn pick(&self, stable_id: &str) -> Result<(String, Kind, bool)> {
        let state = lock(&self.state);
        if let Some(live) = state.live.iter().find(|l| l.stable_id == stable_id) {
            return Ok((live.connector.clone(), live.kind, true));
        }
        let pool: Vec<Connector> = pool::scan(&self.drm_root)
            .into_iter()
            .filter(|c| {
                self.brokers
                    .iter()
                    .any(|(k, b)| *k == c.kind && super::broker_usable(*k, b).is_ok())
            })
            .collect();
        if pool.is_empty() {
            drop(state);
            bail!("{}", self.availability().err().unwrap_or_default());
        }
        let taken = |c: &Connector| state.live.iter().any(|l| l.connector == c.name);
        pool.iter()
            .find(|c| c.status == "disconnected" && !taken(c))
            .map(|c| (c.name.clone(), c.kind, false))
            .ok_or_else(|| {
                anyhow!(
                    "all {} virtual connectors are in use or held by another program",
                    pool.len()
                )
            })
    }

    fn wait_until<T>(&self, what: &str, mut ready: impl FnMut() -> Result<Option<T>>) -> Result<T> {
        let deadline = Instant::now() + self.wait;
        let mut last = None;
        loop {
            match ready() {
                Ok(Some(value)) => return Ok(value),
                Ok(None) => {}
                Err(error) => last = Some(format!("{error:#}")),
            }
            if Instant::now() >= deadline {
                bail!(
                    "KScreen did not show {what} within {} s{}",
                    self.wait.as_secs(),
                    last.map(|l| format!(" ({l})")).unwrap_or_default()
                );
            }
            std::thread::sleep(self.poll);
        }
    }

    /// Everything after the connector is connected; the caller rolls back on an error.
    fn bring_up(&self, connector: &str, kind: Kind, request: &PrivateRequest) -> Result<()> {
        self.wait_until(&format!("{connector} connected"), || {
            Ok(self
                .desktop
                .query()?
                .output(connector)
                .is_some_and(|o| o.connected)
                .then_some(()))
        })?;
        if kind.broker_sets_modes() {
            self.broker(kind)?
                .mode(
                    connector,
                    request.width,
                    request.height,
                    request.refresh_millihz,
                )
                .with_context(|| format!("{connector} did not admit the mode"))?;
        } else {
            self.add_custom_mode(connector, request)?;
        }
        let (layout, mode_id) = self.wait_until(
            &format!(
                "a {}x{} mode at {} mHz on {connector}",
                request.width, request.height, request.refresh_millihz
            ),
            || {
                let layout = self.desktop.query()?;
                let id = layout
                    .output(connector)
                    .filter(|o| o.connected)
                    .and_then(|o| {
                        o.find_mode(request.width, request.height, request.refresh_millihz)
                    })
                    .map(|m| m.id.clone());
                Ok(id.map(|id| (layout, id)))
            },
        )?;
        let others_live = !lock(&self.state)
            .live
            .iter()
            .all(|l| l.connector == connector);
        self.persist_with(&[connector.to_string()]);
        // A second private display cannot own the whole desktop; it joins the first.
        let arrangement = if others_live && request.arrangement == Arrangement::Exclusive {
            Arrangement::Extended
        } else {
            request.arrangement
        };
        // Without a scale KWin picks one from the connector's made-up size (1.7 for 1280x720).
        let scale = Some(request.scale_percent.map_or(1.0, |p| f64::from(p) / 100.0));
        let args = match lock(&self.state).original.clone() {
            // The first display: the other monitors as they were before the plug, then it.
            Some(mut original) if !others_live => {
                kscreen::renumber_modes(&mut original, &layout);
                original
                    .outputs
                    .retain(|o| layout.output(&o.name).is_some_and(|now| now.connected));
                let mut args = if arrangement == Arrangement::Exclusive {
                    Vec::new()
                } else {
                    kscreen::restore_args(&original, &[])
                };
                args.extend(kscreen::apply_args(
                    &original,
                    connector,
                    &mode_id,
                    scale,
                    arrangement,
                ));
                args
            }
            _ => kscreen::apply_args(&layout, connector, &mode_id, scale, arrangement),
        };
        self.desktop
            .apply(&args)
            .context("could not apply the layout")?;
        self.wait_until(&format!("{connector} on mode {mode_id}"), || {
            Ok(self
                .desktop
                .query()?
                .output(connector)
                .is_some_and(|o| o.enabled && o.current_mode_id == mode_id)
                .then_some(()))
        })
        .context("the layout was applied but the mode did not take")?;
        Ok(())
    }

    /// Undo a failed set-up: restore, then disconnect. `reused` keeps a display that already
    /// worked for another handle.
    fn roll_back(&self, connector: &str, kind: Kind, reused: bool) {
        if reused {
            return;
        }
        let alone = lock(&self.state).live.is_empty();
        if alone {
            self.restore_all(&[connector.to_string()]);
        } else {
            self.switch_off(connector);
        }
        self.give_back(connector, kind);
        if alone {
            self.clear_file();
        }
    }

    /// Put the saved layout back and forget it. Outputs the desktop no longer has are left out.
    fn restore_all(&self, virtual_names: &[String]) {
        let Some(original) = lock(&self.state).original.take() else {
            self.switch_off_all(virtual_names);
            return;
        };
        self.restore_layout(&original, virtual_names);
    }

    fn restore_layout(&self, snapshot: &Layout, virtual_names: &[String]) {
        let present = self.desktop.query().ok();
        let mut snapshot = snapshot.clone();
        let mut names = virtual_names.to_vec();
        if let Some(present) = &present {
            snapshot
                .outputs
                .retain(|o| present.output(&o.name).is_some());
            names.retain(|n| present.output(n).is_some());
            kscreen::renumber_modes(&mut snapshot, present);
        }
        let args = kscreen::restore_args(&snapshot, &names);
        match self.desktop.apply(&args) {
            Ok(()) => tracing::info!("restored the desktop layout"),
            Err(error) => {
                tracing::error!(error = %format!("{error:#}"), "could not restore the desktop layout")
            }
        }
    }

    fn switch_off(&self, connector: &str) {
        self.switch_off_all(&[connector.to_string()]);
    }

    fn switch_off_all(&self, names: &[String]) {
        let args: Vec<String> = names
            .iter()
            .map(|n| format!("output.{n}.disable"))
            .collect();
        if let Err(error) = self.desktop.apply(&args) {
            tracing::warn!(error = %format!("{error:#}"), "could not switch the private output off");
        }
    }

    /// A handle is dropped: the last one for a display ends it.
    pub(super) fn release(&self, stable_id: &str, connector: &str) {
        let _ops = lock(&self.ops);
        let (last, kind) = {
            let mut state = lock(&self.state);
            let Some(index) = state.live.iter().position(|l| l.stable_id == stable_id) else {
                return;
            };
            state.live[index].handles -= 1;
            if state.live[index].handles > 0 {
                return;
            }
            let gone = state.live.remove(index);
            (state.live.is_empty(), gone.kind)
        };
        tracing::info!(connector, "removing the private display");
        if last {
            let original = lock(&self.state).original.take();
            match original {
                Some(original) => self.restore_layout(&original, &[connector.to_string()]),
                None => self.switch_off(connector),
            }
        } else {
            self.switch_off(connector);
        }
        self.give_back(connector, kind);
        if last {
            self.clear_file();
        } else {
            self.persist();
        }
    }

    /// The connector goes back to its pool, without the custom modes added to its output.
    fn give_back(&self, connector: &str, kind: Kind) {
        if !kind.broker_sets_modes() {
            self.remove_custom_modes(connector);
        }
        let result = self.broker(kind).and_then(|b| b.disconnect(connector));
        if let Err(error) = result {
            tracing::warn!(connector, error = %format!("{error:#}"), "could not disconnect the connector");
        }
    }

    /// Adds the request's mode to the output when the pool does not list it.
    fn add_custom_mode(&self, connector: &str, request: &PrivateRequest) -> Result<()> {
        let listed = self.desktop.query()?.output(connector).is_some_and(|o| {
            o.find_mode(request.width, request.height, request.refresh_millihz)
                .is_some()
        });
        if listed {
            return Ok(());
        }
        let args = [format!(
            "output.{connector}.addCustomMode.{}.{}.{}.full",
            request.width, request.height, request.refresh_millihz
        )];
        self.desktop
            .apply(&args)
            .with_context(|| format!("KWin did not add the mode to {connector}"))
    }

    /// Removes every custom mode of the pool output `connector`: none is anyone else's.
    fn remove_custom_modes(&self, connector: &str) {
        for _ in 0..CUSTOM_MODES {
            let args = [format!("output.{connector}.removeCustomMode.0")];
            if self.desktop.apply(&args).is_err() {
                break;
            }
        }
    }

    fn persist(&self) {
        let names: Vec<String> = lock(&self.state)
            .live
            .iter()
            .map(|l| l.connector.clone())
            .collect();
        self.persist_with(&names);
    }

    fn persist_with(&self, extra: &[String]) {
        let (Some(path), Some(snapshot)) = (&self.state_file, lock(&self.state).original.clone())
        else {
            return;
        };
        let mut connectors: Vec<String> = lock(&self.state)
            .live
            .iter()
            .map(|l| l.connector.clone())
            .collect();
        for name in extra {
            if !connectors.contains(name) {
                connectors.push(name.clone());
            }
        }
        let saved = Persisted {
            restore_pending: true,
            connectors,
            snapshot,
        };
        if let Err(error) = write_file(path, &saved) {
            tracing::warn!(error = %format!("{error:#}"), "could not save the layout for a restore after a crash");
        }
    }

    fn clear_file(&self) {
        if let Some(path) = &self.state_file
            && let Err(error) = std::fs::remove_file(path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(error = %error, "could not remove the saved layout");
        }
    }

    /// After a crash: put the saved layout back and disconnect the connectors this user still
    /// holds, then forget the file.
    pub fn recover(&self) -> Result<()> {
        let _ops = lock(&self.ops);
        let Some(path) = &self.state_file else {
            return Ok(());
        };
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e).context("could not read the saved layout"),
        };
        let saved: Persisted =
            serde_json::from_str(&text).context("the saved layout is not readable")?;
        if !saved.restore_pending {
            return self.remove_saved(path);
        }
        tracing::warn!("a private display was left on by an earlier run; restoring the desktop");
        let held: Vec<Connector> = pool::scan(&self.drm_root)
            .into_iter()
            .filter(|c| {
                self.broker(c.kind)
                    .and_then(|b| b.status(&c.name))
                    .unwrap_or(false)
            })
            .collect();
        let mut names = saved.connectors.clone();
        for connector in &held {
            if !names.contains(&connector.name) {
                names.push(connector.name.clone());
            }
        }
        self.restore_layout(&saved.snapshot, &names);
        for connector in &held {
            if !connector.kind.broker_sets_modes() {
                self.remove_custom_modes(&connector.name);
            }
            let name = &connector.name;
            match self.broker(connector.kind).and_then(|b| b.disconnect(name)) {
                Ok(()) => tracing::info!(connector = %name, "disconnected a connector left behind"),
                Err(error) => {
                    tracing::warn!(connector = %name, error = %format!("{error:#}"), "could not disconnect a connector left behind")
                }
            }
        }
        self.remove_saved(path)
    }

    fn remove_saved(&self, path: &Path) -> Result<()> {
        std::fs::remove_file(path).context("could not remove the saved layout")
    }
}

fn write_file(path: &Path, saved: &Persisted) -> Result<()> {
    let dir = path.parent().context("the state file has no folder")?;
    std::fs::create_dir_all(dir)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(saved)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod persisted_for_tests {
    use super::*;

    /// (restore_pending, connectors) of a state file.
    pub fn read_saved(path: &Path) -> Option<(bool, Vec<String>)> {
        let saved: Persisted = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
        Some((saved.restore_pending, saved.connectors))
    }
}
