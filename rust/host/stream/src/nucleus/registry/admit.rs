//! Nucleus Co-op and background-stream session admission.

use super::super::*;
use super::{Entry, Registry, stop};

impl Registry {
    /// Admit a launch: start a session of its game, or seat the device in
    /// the running one. The launch then holds its slot until
    /// [`leave`](Self::leave). A background stream's refusal names the app
    /// and why it can't stream in the background; it never runs another way.
    pub fn admit(&self, request: Admission) -> Result<Seat, Refusal> {
        let game = request.game;
        // A device's second screen is the same player as its first: it takes no seat.
        if request
            .client
            .uuid
            .ends_with(butterpollo_session::state::SECOND_SCREEN_SUFFIX)
        {
            return Err(Refusal::busy(format!(
                "{} plays on this device's first screen: a second screen takes no co-op seat",
                game.app().name
            )));
        }
        if game.kind() == Kind::Coop {
            return self.admit_game(request);
        }
        log_background_mapping();
        // A value earlier builds wrote is Nucleus Co-op too, until the next save replaces it.
        let setting = request.config.get(mode::KEY, "");
        let admitted = match mode::parse(setting) {
            Err(reason) if !mode::retired_background(setting) => Err(Refusal::unavailable(reason)),
            _ => self.admit_game(request),
        };
        admitted.map_err(|refusal| Refusal {
            message: format!(
                "{} can't stream in the background: {}",
                game.app().name,
                mid_sentence(&refusal.message)
            ),
            ..refusal
        })
    }

    /// The game of the running background stream of app `app_id` that
    /// `device` plays in, which it returns to.
    fn returning(&self, app_id: u32, device: &str) -> Option<String> {
        self.lock()
            .sessions
            .values()
            .filter(|entry| entry.app_id == app_id)
            .find_map(|entry| {
                entry.instance.session(|s| {
                    (!s.ended() && s.kind == Kind::Background && s.slot_of(device).is_some())
                        .then(|| s.game.clone())
                })
            })
    }

    fn admit_game(&self, request: Admission) -> Result<Seat, Refusal> {
        let pc = self.pc();
        if let Some(why) = pc.unavailable() {
            return Err(Refusal::unavailable(why));
        }
        let kind = request.game.kind();
        let _one = self.admit.lock().unwrap_or_else(PoisonError::into_inner);
        let configured = configured(request.config);
        let folder = self.folder(pc.as_ref(), &configured)?;
        let catalog = self.catalog(&folder, true).map_err(|error| {
            Refusal::unavailable(format!(
                "The co-op helper's handlers and games did not load: {error}"
            ))
        })?;
        let app = request.game.app();
        let client = request.client;
        stop(self.reap());
        let (found, returning) = if kind == Kind::Background {
            // A hub handler only for the game the user added at exactly this
            // path: a file-name match is another game's (Portal's hl2.exe is
            // not Half-Life 2). Any other one-player game streams in the
            // background with ours. Its owner returning to it finds the
            // session's game: no Steam read or folder scan again.
            match executable(app).and_then(|exe| catalog.added(exe)) {
                Some(found) => (Some(Chosen::Hub(found)), None),
                None => match self.returning(request.game.app_id(), &client.uuid) {
                    Some(game) => (None, Some(game)),
                    None => (Some(own_handler(pc.as_ref(), &catalog, app)?), None),
                },
            }
        } else {
            match catalog.find(&app.name, executable(app)) {
                Ok(found) => (Some(Chosen::Hub(found)), None),
                Err(missing) => return Err(Refusal::missing(&missing)),
            }
        };
        let game = returning
            .or_else(|| found.as_ref().map(Chosen::guid))
            .unwrap_or_default();
        let decision = {
            let state = self.lock();
            let current = state
                .sessions
                .values()
                .find_map(|entry| entry.instance.session(|s| (!s.ended()).then(|| s.clone())));
            coop::admit(current.as_ref(), &game, kind, &client.uuid)
        };
        match decision {
            Admit::Busy(text) => Err(Refusal::busy(text)),
            Admit::Join(id) => {
                let mut state = self.lock();
                let entry = state.sessions.get_mut(&id).ok_or_else(|| {
                    Refusal::busy(format!("The {} session ended: launch it again", app.name))
                })?;
                let slot = entry
                    .instance
                    .session(|s| s.join(&client.uuid, &client.name, client.allows_coop_join()))
                    .map_err(|refused| Refusal::busy(refused.to_string()))?
                    .slot();
                let holder = entry.holders.get_mut(slot).ok_or_else(|| {
                    Refusal::unavailable(format!("the session has no slot {slot}"))
                })?;
                *holder = Some(request.launch.into());
                tracing::info!(session = id, ?kind, slot, client = %client.name, title = %app.name, "Co-op helper: a player joined");
                Ok(Seat { session: id, slot })
            }
            Admit::Start => {
                // The session returned to ended meanwhile.
                let Some(found) = found else {
                    return Err(Refusal::busy(format!(
                        "The {} session ended: launch it again",
                        app.name
                    )));
                };
                // A stream that ended parks its controller in the first
                // XInput slot, and the game would read that one (#56).
                (request.release_parked)().map_err(|error| {
                    Refusal::unavailable(format!(
                        "The co-op helper could not unplug the controllers of streams that ended: {error:#}"
                    ))
                })?;
                let free = pc.free_pads().map_err(|error| {
                    Refusal::unavailable(format!(
                        "The co-op helper could not count the free controller slots: {error:#}"
                    ))
                })?;
                let players = match (kind, &found) {
                    (Kind::Coop, Chosen::Hub(found)) => coop::players(
                        found.handler.max_players,
                        request.config.nucleus_coop_players(),
                        free,
                    ),
                    _ => coop::players(1, 1, free),
                }
                .map_err(Refusal::unavailable)?;
                let policy = Policy::resolve(request.config)
                    .map_err(|error| Refusal::unavailable(format!("{error:#}")))?;
                let own = match &found {
                    Chosen::Own(setup) => Some(setup),
                    Chosen::Hub(_) => None,
                };
                let refused = |error: String| {
                    Refusal::unavailable(format!(
                        "The co-op helper did not start {}: {error}",
                        app.name
                    ))
                };
                // A hub handler always runs elevated; ours as the user unless
                // it or the game needs administrator rights.
                let run = match own {
                    Some(setup) => run_of(pc.as_ref(), setup, true).map_err(String::from),
                    None => pc
                        .account()
                        .map_err(|error| format!("{error:#}"))
                        .and_then(|account| elevation::choose(Script::Hub, Need::Nothing, account)),
                }
                .map_err(refused)?;
                // Ours that a hub handler replaces or no game uses, but the
                // one this session starts with.
                let current = own.map(Setup::guid);
                let stale = catalog.stale(current.as_deref());
                pc.ready(&folder, run, own, &stale)
                    .map_err(|error| refused(format!("{error:#}")))?;
                // Last, so a launch refused for another reason leaves the
                // user's window open.
                self.close_for_session(&app.name)?;
                // Copied from the host's once Nucleus Co-op is closed: none of
                // its files changes while they are read.
                let copy = match run {
                    Run::User => Some(pc.user_copy(&folder).map_err(|error| {
                        refused(format!(
                            "the signed-in user's own copy of the co-op helper was not made: {error:#}"
                        ))
                    })?),
                    Run::Elevated => None,
                };
                // They go while Nucleus is closed. A failure leaves a handler
                // that is never chosen and is tried again next time: the
                // stream goes on, and its player sees why.
                let mut kept = Vec::new();
                let mut why = String::new();
                for own in stale {
                    if let Err(error) = pc.remove_own(&folder, copy.as_deref(), own) {
                        tracing::error!(file = own.file, error = %format!("{error:#}"), "Co-op helper: Rubylight's replaced or unused handler was not fully removed");
                        kept.push(own.file.as_str());
                        why = format!("{error:#}");
                    }
                }
                match kept.as_slice() {
                    [] => {}
                    [file] => request.warnings.set(
                        HANDLER_NOT_REMOVED,
                        format!("Rubylight's co-op handler {file} was not fully removed: {why}"),
                    ),
                    files => request.warnings.set(
                        HANDLER_NOT_REMOVED,
                        format!(
                            "Rubylight's co-op handlers {} were not fully removed: {why}",
                            files.join(", ")
                        ),
                    ),
                }
                let exe_path = match &found {
                    Chosen::Hub(found) => found.exe_path.clone(),
                    Chosen::Own(setup) => setup.exe_path.clone(),
                };
                let (handler, steam_app) = match found {
                    Chosen::Hub(found) => (found.handler, None),
                    // The game renders at the stream's size where it can be told,
                    // and a DirectInput game's raw input is taken from it.
                    Chosen::Own(setup) => (
                        pc.reads_direct_input(&setup.exe_path)
                            .and_then(|direct_input| {
                                pc.write_own(
                                    &folder,
                                    copy.as_deref(),
                                    &Setup {
                                        size: Some(request.size),
                                        direct_input,
                                        ..setup.clone()
                                    },
                                )
                            })
                            .map_err(|error| {
                                Refusal::unavailable(format!(
                                    "Rubylight's co-op handler for {} was not written: {error:#}",
                                    app.name
                                ))
                            })?,
                        setup.steam_app,
                    ),
                };
                let id = {
                    let mut state = self.lock();
                    state.next += 1;
                    state.next
                };
                let start = Start {
                    id,
                    kind,
                    folder: &folder,
                    user_copy: copy.as_deref(),
                    handler: &handler,
                    exe_path: &exe_path,
                    steam_app,
                    title: &app.name,
                    players,
                    owner: (&client.uuid, &client.name),
                    size: request.size,
                    instances: request
                        .instances
                        .clone()
                        .map(|routes| routes as butterpollo_win_io::nucleus::session::Instances),
                };
                let instance: Arc<dyn Instance> = pc
                    .start(&start, policy, request.warnings)
                    .map_err(|error| refused(format!("{error:#}")))?
                    .into();
                let mut holders = vec![None; players];
                holders[0] = Some(request.launch.into());
                self.lock().sessions.insert(
                    id,
                    Entry {
                        instance,
                        app_id: request.game.app_id(),
                        holders,
                        feedback: (0..players).map(|_| Vec::new()).collect(),
                        ended: None,
                        audio: request.instances.clone(),
                    },
                );
                tracing::info!(session = id, ?kind, players, game = %handler.guid, own = handler.own, ?run, client = %client.name, title = %app.name, "Co-op helper: session started");
                Ok(Seat {
                    session: id,
                    slot: 0,
                })
            }
        }
    }
}

/// The first background stream says once that `background_stream` and
/// `background-stream` now run in Nucleus Co-op.
fn log_background_mapping() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        tracing::info!(
            "Co-op helper: background_stream (device) and background-stream (app) now run background streams in the co-op helper, which needs a handler for the game"
        );
    });
}

/// `message` as it reads after a colon: its neutral lead word lowercased, any
/// other start (a game's title) kept as it is.
pub(super) fn mid_sentence(message: &str) -> String {
    for (lead, lower) in [
        ("The co-op helper", "the co-op helper"),
        ("Co-op ", "co-op "),
    ] {
        if let Some(rest) = message.strip_prefix(lead) {
            return format!("{lower}{rest}");
        }
    }
    message.to_owned()
}
