//! The [`Registry`]: the host's Nucleus Co-op install, its handlers and games,
//! the window the console opens, and the sessions launches are admitted to.
use super::*;

mod admit;
mod sessions;
#[cfg(test)]
mod tests;

struct Entry {
    instance: Arc<dyn Instance>,
    app_id: u32,
    /// The launch streaming each slot; a leave from any other launch is stale.
    holders: Vec<Option<String>>,
    /// Each slot's feedback its stream has not taken yet.
    feedback: Vec<Vec<PadReport>>,
    /// When the registry first saw the session ended.
    ended: Option<Instant>,
    /// Where its instances play ([`Registry::plays_on_pc`]).
    audio: Option<Arc<crate::stream::InstanceRoutes>>,
}

#[derive(Default)]
pub(super) struct State {
    sessions: BTreeMap<u32, Entry>,
    next: u32,
}

struct Cached {
    at: Instant,
    folder: PathBuf,
    catalog: Result<Arc<Catalog>, String>,
}

/// [`own_status`]'s answer for one .exe, as it was modified then.
pub struct OwnStatus {
    pub at: Instant,
    modified: Option<std::time::SystemTime>,
    status: Result<Option<Run>, Missing>,
}

/// The host's Nucleus Co-op sessions.
pub struct Registry {
    pc: RwLock<Arc<dyn Pc>>,
    /// One co-op launch is admitted at a time, so two first players of one
    /// game start one session.
    admit: Mutex<()>,
    state: Mutex<State>,
    catalog: Mutex<Option<Cached>>,
    /// The console's [`own_status`] answers by .exe path; only the console
    /// reads them.
    pub own_statuses: Mutex<BTreeMap<String, OwnStatus>>,
    /// Whether the signed-in account elevates, or why not, as the console
    /// last read it ([`Self::account`]), and when.
    pub account: Mutex<Option<(Instant, Account)>>,
    /// The Nucleus Co-op the console opened: the only one the host closes,
    /// for a session or the user.
    opened: Mutex<Option<Box<dyn Window>>>,
    /// Why the install was last refused for co-op entries, logged once until
    /// the reason changes ([`Self::newly_refused`]).
    refused: Mutex<Option<String>>,
}

impl Registry {
    pub fn new(pc: Arc<dyn Pc>) -> Self {
        Self {
            pc: RwLock::new(pc),
            admit: Mutex::new(()),
            state: Mutex::default(),
            catalog: Mutex::new(None),
            own_statuses: Mutex::default(),
            account: Mutex::new(None),
            opened: Mutex::new(None),
            refused: Mutex::new(None),
        }
    }
    /// This PC; in tests a PC without Nucleus Co-op, so no test reads the
    /// real one.
    pub fn on_pc() -> Self {
        #[cfg(not(any(test, feature = "test-support")))]
        let pc: Arc<dyn Pc> = Arc::new(OnPc);
        #[cfg(any(test, feature = "test-support"))]
        let pc: Arc<dyn Pc> = Arc::new(fake::Pc::unavailable());
        Self::new(pc)
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn use_pc(&self, pc: Arc<dyn Pc>) {
        *self.pc.write().unwrap() = pc;
        *self.catalog.lock().unwrap() = None;
        self.own_statuses.lock().unwrap().clear();
        *self.account.lock().unwrap() = None;
    }
    fn pc(&self) -> Arc<dyn Pc> {
        self.pc
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
    pub(super) fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Why this host cannot run Nucleus Co-op sessions, or None.
    pub fn unavailable(&self) -> Option<String> {
        self.pc().unavailable()
    }

    /// Whether `why` the install is refused differs from the last refusal,
    /// remembered: the app list asks every few seconds, and the same refusal
    /// is logged once. None when it was accepted, so a refusal again logs.
    pub fn newly_refused(&self, why: Option<&str>) -> bool {
        let mut last = self.refused.lock().unwrap_or_else(PoisonError::into_inner);
        let new = why.is_some() && last.as_deref() != why;
        *last = why.map(str::to_string);
        new
    }

    /// The co-op entries of `apps`: none while Nucleus Co-op is unavailable
    /// or not installed. Each failure still shows: the console's panel names
    /// a broken install, and [`Self::catalog`] logs an unreadable one.
    pub fn entries(&self, apps: &[App], configured: &str) -> Vec<CoopApp> {
        let pc = self.pc();
        if pc.unavailable().is_some() {
            return Vec::new();
        }
        let folder = match pc.installed(configured) {
            Ok(Some(folder)) => {
                self.newly_refused(None);
                folder
            }
            Ok(None) => {
                self.newly_refused(None);
                return Vec::new();
            }
            Err(error) => {
                let error = format!("{error:#}");
                if self.newly_refused(Some(&error)) {
                    tracing::warn!(error, "Co-op helper: no co-op entries");
                }
                return Vec::new();
            }
        };
        match self.catalog(&folder, false) {
            Ok(catalog) => entries_of(&catalog, apps),
            Err(_) => Vec::new(),
        }
    }

    /// The co-op entry with id `id`.
    pub fn resolve(&self, apps: &[App], configured: &str, id: u32) -> Option<CoopApp> {
        self.entries(apps, configured)
            .into_iter()
            .find(|coop| coop.id() == id)
    }

    /// The handlers and games of `folder`, read again when `fresh` or once
    /// the last read is older than [`CATALOG_TTL`]. A failure is logged when
    /// it is new.
    fn catalog(&self, folder: &Path, fresh: bool) -> Result<Arc<Catalog>, String> {
        let mut cached = self.catalog.lock().unwrap_or_else(PoisonError::into_inner);
        if !fresh
            && let Some(cached) = cached
                .as_ref()
                .filter(|c| c.folder == folder && c.at.elapsed() < CATALOG_TTL)
        {
            return cached.catalog.clone();
        }
        let catalog = self
            .pc()
            .catalog(folder)
            .map(Arc::new)
            .map_err(|error| format!("{error:#}"));
        if let Err(error) = &catalog
            && cached
                .as_ref()
                .is_none_or(|c| c.catalog.as_ref().err() != Some(error))
        {
            tracing::warn!(folder = %folder.display(), %error, "Co-op helper: the handlers and games did not load");
        }
        *cached = Some(Cached {
            at: Instant::now(),
            folder: folder.into(),
            catalog: catalog.clone(),
        });
        catalog
    }

    /// Where Nucleus Co-op stands for `configured` (`nucleus_path`).
    pub fn install(&self, configured: &str) -> Install {
        Install::of(configured, self.pc().installed(configured))
    }

    /// What the console says while the copy an earlier Rubylight installed
    /// runs instead of the folder `configured` names ([`Pc::instead`]).
    pub fn notice(&self, configured: &str) -> Option<String> {
        self.pc().instead(configured).map(|(_, notice)| notice)
    }

    /// The Nucleus folder a launch runs: refused with the reason when none
    /// is set up, or the folder chosen is one Nucleus Co-op can't run from.
    /// Rubylight installs nothing itself.
    fn folder(&self, pc: &dyn Pc, configured: &str) -> Result<PathBuf, Refusal> {
        match pc.installed(configured) {
            Ok(Some(folder)) => Ok(folder),
            Ok(None) => Err(Refusal::unavailable(not_set_up())),
            Err(error) => Err(Refusal::unavailable(format!(
                "The co-op helper is not ready: {error:#}"
            ))),
        }
    }

    /// The console's Choose folder: `path` checked as a launch would check
    /// it, so a folder Nucleus Co-op can't run from is refused before it is
    /// saved, as is one only the copy an earlier Rubylight installed would
    /// stand in for ([`Pc::instead`]).
    pub fn check_folder(&self, path: &str) -> Result<Install, Refusal> {
        let pc = self.pc();
        if let Some(why) = pc.unavailable() {
            return Err(Refusal::unavailable(why));
        }
        let refused = |message: String| Refusal {
            status: 400,
            message,
        };
        match Install::of(path, pc.installed(path)) {
            Install::Failed { error, .. } => Err(refused(error)),
            install => match pc.instead(path) {
                Some((why, _)) => Err(refused(why.to_string())),
                None => Ok(install),
            },
        }
    }

    /// Keep `to` as `nucleus_path` in place of `from` with `save`. When the
    /// Nucleus Co-op `from` names is another than `to`'s, what its sessions
    /// left (Settings.ini's and the compatibility settings' journals) is put
    /// back first, or it would stay where no session looks again
    /// ([`Pc::put_back`]): refused while a session runs or starts, which
    /// would leave more. A folder the check refuses is not written to, and a
    /// host that runs no sessions has nothing to put back.
    pub fn change_folder(
        &self,
        from: &str,
        to: &str,
        save: impl FnOnce() -> Result<(), Refusal>,
    ) -> Result<(), Refusal> {
        let pc = self.pc();
        if pc.unavailable().is_some() {
            return save();
        }
        let old = match pc.installed(from) {
            Ok(Some(old)) => old,
            Ok(None) => return save(),
            Err(error) => {
                tracing::warn!(
                    error = format!("{error:#}"),
                    "Co-op helper: the folder left is refused, so what its sessions left stays there"
                );
                return save();
            }
        };
        let new = pc.installed(to).ok().flatten();
        if new.is_some_and(|new| new.as_os_str().eq_ignore_ascii_case(old.as_os_str())) {
            return save();
        }
        // A launch being admitted would leave its own.
        let _one = match self.admit.try_lock() {
            Ok(one) => one,
            Err(std::sync::TryLockError::Poisoned(one)) => one.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                return Err(Refusal::busy(
                    "A co-op or background stream is starting: choose the folder once it has",
                ));
            }
        };
        if let Some(title) = self.running_title() {
            return Err(Refusal::busy(format!(
                "The co-op helper is running {title}: end that session first, then choose the folder"
            )));
        }
        pc.put_back(&old)
            .map_err(|error| Refusal::busy(format!("{error:#}")))?;
        tracing::info!(folder = %old.display(), "Co-op helper: what its sessions left is put back before the folder changes");
        save()
    }

    /// The title of the session that runs or starts, read with no launch
    /// being admitted.
    fn running_title(&self) -> Option<String> {
        self.lock().sessions.values().find_map(|entry| {
            entry
                .instance
                .session(|s| (!s.ended()).then(|| s.title.clone()))
        })
    }

    /// Open Nucleus Co-op's window on the PC, where the user adds a handler
    /// or a game. Refused where it can't run, before it is set up, while a
    /// session runs or starts, which holds it, and while Nucleus Co-op
    /// already runs. The next session closes it first.
    pub fn open(&self, configured: &str) -> Result<(), Refusal> {
        let pc = self.pc();
        if let Some(why) = pc.unavailable() {
            return Err(Refusal::unavailable(why));
        }
        let folder = match pc.installed(configured) {
            Ok(Some(folder)) => folder,
            Ok(None) => return Err(Refusal::unavailable(not_set_up())),
            Err(error) => {
                return Err(Refusal::unavailable(format!(
                    "The co-op helper is not ready: {error:#}"
                )));
            }
        };
        // A launch being admitted may start Nucleus Co-op any moment.
        let _one = match self.admit.try_lock() {
            Ok(one) => one,
            Err(std::sync::TryLockError::Poisoned(one)) => one.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                return Err(Refusal::busy(
                    "A co-op or background stream is starting: open the co-op helper once it has",
                ));
            }
        };
        // Read with no launch being admitted, so a session can't slip in.
        if let Some(title) = self.running_title() {
            return Err(Refusal::busy(format!(
                "The co-op helper is running {title}: end that session first"
            )));
        }
        let mut opened = self.opened.lock().unwrap_or_else(PoisonError::into_inner);
        // Ours by its job, not its first process id: Nucleus Co-op may have
        // restarted itself.
        if let Some(window) = opened.as_ref() {
            let running = window.running().map_err(|error| {
                Refusal::unavailable(format!(
                    "The co-op helper's window could not be read: {error:#}"
                ))
            })?;
            if running {
                return Err(Refusal::busy(
                    "The co-op helper is already open on the PC's screen: use it there, or close it here",
                ));
            }
            *opened = None;
        }
        match pc.running() {
            Ok(Some(pid)) => {
                return Err(Refusal::busy(format!(
                    "The co-op helper is already running on the PC (process {pid}): close it there first"
                )));
            }
            Ok(None) => {}
            Err(error) => {
                return Err(Refusal::unavailable(format!(
                    "The co-op helper's processes could not be listed: {error:#}"
                )));
            }
        }
        let window = pc.open(&folder).map_err(|error| {
            Refusal::unavailable(format!("The co-op helper did not open: {error:#}"))
        })?;
        tracing::info!(pid = window.pid(), folder = %folder.display(), "Co-op helper: opened on the PC from the console");
        *opened = Some(window);
        Ok(())
    }

    /// Whether Windows gives the account signed in on this PC an elevated
    /// token, which hub handlers, co-op and games that need administrator
    /// rights run with, or why not: for the console, read again once
    /// [`OWN_STATUS_TTL`] passed, so its poll asks Windows once a minute.
    /// None when Rubylight could not tell (nobody signed in yet), asked
    /// again at the next poll.
    pub fn account(&self) -> Option<Account> {
        let mut kept = self.account.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((at, account)) = *kept
            && at.elapsed() < OWN_STATUS_TTL
        {
            return Some(account);
        }
        match self.pc().account() {
            Ok(account) => {
                *kept = Some((Instant::now(), account));
                Some(account)
            }
            Err(error) => {
                *kept = None;
                tracing::debug!("Co-op helper: whether the signed-in account elevates: {error:#}");
                None
            }
        }
    }

    /// Whether the Nucleus Co-op the console opened still runs. One that
    /// can't be read counts as open, so the console offers Close, which then
    /// says why.
    pub fn opened(&self) -> bool {
        let opened = match self.opened.try_lock() {
            Ok(opened) => opened,
            Err(std::sync::TryLockError::Poisoned(opened)) => opened.into_inner(),
            // Being opened or closed right now.
            Err(std::sync::TryLockError::WouldBlock) => return true,
        };
        opened
            .as_ref()
            .is_some_and(|window| window.running().unwrap_or(true))
    }

    /// The console's Close Nucleus Co-op: closes the window [`Self::open`]
    /// opened, whether it was still open. A Nucleus Co-op the host did not
    /// start is refused, never ended.
    pub fn close(&self) -> Result<bool, Refusal> {
        let mut opened = self.opened.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(window) = opened.as_ref() {
            let running = window.running().map_err(|error| {
                Refusal::unavailable(format!(
                    "The co-op helper's window could not be read: {error:#}"
                ))
            })?;
            if running {
                window
                    .close(CLOSE_WAIT)
                    .map_err(|error| Refusal::busy(format!("{error:#}")))?;
                *opened = None;
                return Ok(true);
            }
            *opened = None;
        }
        match self.pc().running() {
            Ok(Some(pid)) => Err(Refusal::busy(format!(
                "The co-op helper (process {pid}) was not opened from this console: close it on the PC"
            ))),
            Ok(None) => Ok(false),
            Err(error) => Err(Refusal::unavailable(format!(
                "The co-op helper's processes could not be listed: {error:#}"
            ))),
        }
    }

    /// Before a session starts Nucleus Co-op: close the window the console
    /// opened. One that won't close refuses the session, loudly, as a
    /// Nucleus Co-op the host didn't start does.
    fn close_for_session(&self, title: &str) -> Result<(), Refusal> {
        let mut opened = self.opened.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(window) = opened.as_ref() else {
            return Ok(());
        };
        let pid = window.pid();
        let refuse = |error: anyhow::Error| {
            tracing::warn!(pid, error = %format!("{error:#}"), "Co-op helper: the console's window did not close for a session");
            Refusal::unavailable(format!(
                "The co-op helper did not start {title}: {}",
                admit::mid_sentence(&format!("{error:#}"))
            ))
        };
        let running = window.running().with_context(|| {
            format!("The co-op helper's window (process {pid}) could not be read")
        });
        if !running.map_err(refuse)? {
            *opened = None;
            return Ok(());
        }
        window.close(CLOSE_WAIT).map_err(refuse)?;
        *opened = None;
        Ok(())
    }

    /// Each app's co-op status, in the order of `apps`; None while Nucleus
    /// Co-op is not installed, and the error while its handlers and games
    /// don't load. The handlers are read as the library reads them, so a
    /// handler added in Nucleus Co-op shows within [`CATALOG_TTL`].
    pub fn games(&self, apps: &[App], config: &Config) -> Option<Result<Vec<GameStatus>, String>> {
        let configured = configured(config);
        let folder = self.pc().installed(&configured).ok().flatten()?;
        let catalog = match self.catalog(&folder, false) {
            Ok(catalog) => catalog,
            Err(error) => return Some(Err(error)),
        };
        // What a background launch without the hub's handler answers.
        let own = |app: &App| self.own_status(app);
        let ready = |handler: Handler| GameStatus::Ready {
            players: coop::players(
                handler.max_players,
                config.nucleus_coop_players(),
                coop::MAX_PLAYERS,
            )
            .unwrap_or(1),
            handler: handler.name,
        };
        // As the library lists them: a hidden entry is ready (its switch is
        // off), one whose id is taken is not.
        let statuses = listings(&catalog, apps)
            .into_iter()
            .zip(apps)
            .map(|(listing, app)| match listing {
                Listing::Listed(coop) => ready(coop.handler),
                Listing::Hidden(handler) => ready(handler),
                Listing::Taken => GameStatus::Taken {
                    entry: entry_name(&app.name),
                },
                // Shown as a background launch would answer it: one without
                // the user's added game streams with ours.
                Listing::Missing(missing @ Missing::NoHandler { .. }) => match own(app) {
                    Ok(Some(run)) => GameStatus::Own { missing, run },
                    Ok(None) => GameStatus::Missing(missing),
                    Err(refused) => GameStatus::Missing(refused),
                },
                // Adding the game makes it ready, for co-op and background
                // streams alike, through the hub handler: only what adding
                // can't fix (no .exe, one not on this PC, administrator
                // rights the account lacks) says otherwise.
                Listing::Missing(missing @ Missing::NotAdded { .. }) => {
                    match self.own_status(app) {
                        Err(refused @ (Missing::NoExecutable { .. } | Missing::Refused { .. })) => {
                            GameStatus::Missing(refused)
                        }
                        Ok(Some(run)) => GameStatus::Own { missing, run },
                        _ => GameStatus::Missing(missing),
                    }
                }
                Listing::Missing(missing) => GameStatus::Missing(missing),
            })
            .collect();
        Some(Ok(statuses))
    }

    /// [`own_status`] for the console, read again once [`OWN_STATUS_TTL`]
    /// passed or the .exe changed; one Rubylight could not tell is read
    /// again at once. The path is checked before anything opens it, and
    /// nothing else waits while the answer is worked out.
    fn own_status(&self, app: &App) -> Result<Option<Run>, Missing> {
        let Some(exe) = executable(app)
            .filter(|_| starts_exe(app))
            .filter(|exe| Path::new(exe).is_absolute())
        else {
            return own_status(self.pc().as_ref(), app).map_err(|no| no.missing);
        };
        refuse_path(exe)?;
        let pc = self.pc();
        let modified = apps::probe_as_user(pc.as_ref(), || {
            std::fs::metadata(exe)
                .and_then(|metadata| metadata.modified())
                .ok()
        })
        .ok()
        .flatten();
        let fresh = |status: &&OwnStatus| {
            status.modified == modified && status.at.elapsed() < OWN_STATUS_TTL
        };
        if let Some(status) = self
            .own_statuses
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(exe)
            .filter(fresh)
        {
            return status.status.clone();
        }
        let status = own_status(pc.as_ref(), app);
        let lasting = status.as_ref().err().is_none_or(|no| no.lasting);
        let status = status.map_err(|no| no.missing);
        let mut statuses = self
            .own_statuses
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        statuses.retain(|_, status| status.at.elapsed() < OWN_STATUS_TTL);
        if lasting {
            statuses.insert(
                exe.to_string(),
                OwnStatus {
                    at: Instant::now(),
                    modified,
                    status: status.clone(),
                },
            );
        } else {
            // Asked again at the next refresh, not a minute later.
            statuses.remove(exe);
        }
        status
    }
}

fn stop(instances: Vec<Arc<dyn Instance>>) {
    for instance in instances {
        instance.stop();
    }
}
