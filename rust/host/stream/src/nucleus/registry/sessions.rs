//! Active Nucleus Co-op session membership, feedback, and reaping.

use super::super::*;
use super::{Registry, stop};

impl Registry {
    /// `seat`'s player plays the game on the PC too, or only on the client:
    /// their instance's device follows. What they should know, if anything.
    pub fn plays_on_pc(&self, seat: Seat, on_pc: bool) -> Option<&'static str> {
        let audio = self.lock().sessions.get(&seat.session)?.audio.clone()?;
        audio.on_pc(seat.slot, on_pc)
    }

    /// The session's instance while the registry has it.
    pub fn instance(&self, session: u32) -> Option<Arc<dyn Instance>> {
        self.lock()
            .sessions
            .get(&session)
            .map(|entry| entry.instance.clone())
    }

    /// The feedback for `seat`'s player since its last call. One read of
    /// the pads serves every slot; each slot keeps the last
    /// [`FEEDBACK_KEPT`] reports until its stream takes them.
    pub fn feedback(&self, seat: Seat) -> Vec<PadReport> {
        let Some(instance) = self.instance(seat.session) else {
            return Vec::new();
        };
        let reports = instance.feedback();
        let mut state = self.lock();
        let Some(entry) = state.sessions.get_mut(&seat.session) else {
            return Vec::new();
        };
        for (slot, report) in reports {
            if let Some(kept) = entry.feedback.get_mut(slot) {
                if kept.len() == FEEDBACK_KEPT {
                    kept.remove(0);
                }
                kept.push(report);
            }
        }
        entry
            .feedback
            .get_mut(seat.slot)
            .map(std::mem::take)
            .unwrap_or_default()
    }

    /// The stream of `launch` ended or its pending launch expired: the device
    /// keeps its slot for [`coop::GRACE`]. A launch that no longer holds the
    /// slot leaves nothing.
    pub fn leave(&self, seat: Seat, launch: &str) {
        {
            let mut state = self.lock();
            if let Some(entry) = state.sessions.get_mut(&seat.session)
                && let Some(holder) = entry.holders.get_mut(seat.slot)
                && holder.as_deref() == Some(launch)
            {
                *holder = None;
                let device = entry.instance.session(|s| {
                    s.slots
                        .get(seat.slot)
                        .and_then(|slot| slot.seat.as_ref())
                        .map(|seat| seat.device.clone())
                });
                if let Some(device) = device {
                    // Release held pad inputs before `s.leave` marks the slot away
                    // (once away, `session.route` drops controller events).
                    let _ = entry
                        .instance
                        .input(&device, super::stream::neutral_controller());
                    entry.instance.session(|s| {
                        s.leave(&device, Instant::now());
                    });
                }
                tracing::info!(
                    session = seat.session,
                    slot = seat.slot,
                    "Nucleus Co-op: a player's stream ended"
                );
            }
        }
        stop(self.reap());
    }

    /// Seat `client` in `session`, or return it to its own slot, from its
    /// session list. The slot waits [`coop::GRACE`] for the device to launch
    /// the entry, unless the device streams it now. Returns the slot and the
    /// entry's app id.
    pub fn join(&self, session: u32, client: &Client) -> Result<(usize, u32), JoinError> {
        stop(self.reap());
        let state = self.lock();
        let entry = state.sessions.get(&session).ok_or(JoinError::NoSession)?;
        // A background stream is private: its id is no session to join, and
        // its refusals would name the game.
        if entry.instance.session(|s| s.kind) != Kind::Coop {
            return Err(JoinError::NoSession);
        }
        let slot = entry
            .instance
            .session(|s| {
                let slot = s
                    .join(&client.uuid, &client.name, client.allows_coop_join())?
                    .slot();
                if entry.holders.get(slot).is_none_or(Option::is_none) {
                    s.leave(&client.uuid, Instant::now());
                }
                Ok(slot)
            })
            .map_err(JoinError::Refused)?;
        tracing::info!(session, slot, client = %client.name, "Nucleus Co-op: a device took a slot from its session list");
        Ok((slot, entry.app_id))
    }

    /// Remove the player of `slot`; they cannot rejoin this session.
    /// Returns the device.
    pub fn kick(&self, session: u32, slot: usize) -> Option<String> {
        let mut state = self.lock();
        let entry = state.sessions.get_mut(&session)?;
        let active_device = entry.instance.session(|s| {
            s.slots
                .get(slot)
                .and_then(|s| s.seat.as_ref())
                .map(|seat| seat.device.clone())
        });
        if let Some(device) = &active_device {
            let _ = entry
                .instance
                .input(device, super::stream::neutral_controller());
        }
        let device = entry.instance.session(|s| s.kick(slot))?;
        if let Some(holder) = entry.holders.get_mut(slot) {
            *holder = None;
        }
        tracing::info!(
            session,
            slot,
            device,
            "Nucleus Co-op: player removed in the web console"
        );
        Some(device)
    }

    /// The running background stream `device` plays in, whether its stream
    /// runs or it stepped away and keeps its slot: only that device ever
    /// returns to it.
    pub fn background_of(&self, device: &str) -> Option<u32> {
        stop(self.reap());
        self.lock().sessions.iter().find_map(|(&id, entry)| {
            entry
                .instance
                .session(|s| {
                    !s.ended() && s.kind == Kind::Background && s.slot_of(device).is_some()
                })
                .then_some(id)
        })
    }

    /// End `session`: Nucleus and every instance stop. False when there is
    /// no such session.
    pub fn end(&self, session: u32) -> bool {
        self.end_with(session, End::Ended)
    }

    /// [`Self::end`] with why, which its players' streams and the console
    /// show. A session that ended already keeps its own reason.
    pub fn end_with(&self, session: u32, why: End) -> bool {
        let instance = {
            let state = self.lock();
            let Some(entry) = state.sessions.get(&session) else {
                return false;
            };
            entry.instance.session(|s| s.end(why));
            entry.instance.clone()
        };
        let why = instance.session(|s| match &s.phase {
            coop::Phase::Ended(End::Ended) | coop::Phase::Ended(End::Empty) => None,
            coop::Phase::Ended(why) => Some(why.to_string()),
            _ => None,
        });
        match why {
            Some(why) => tracing::warn!(session, why, "Nucleus Co-op: session ended"),
            None => tracing::info!(session, "Nucleus Co-op: session ended"),
        }
        instance.stop();
        stop(self.reap());
        true
    }

    /// Every session the registry has, ended ones until they are reaped.
    pub fn sessions(&self) -> Vec<Listed> {
        stop(self.reap());
        self.lock()
            .sessions
            .values()
            .map(|entry| Listed {
                app_id: entry.app_id,
                session: entry.instance.session(|s| s.clone()),
                held: entry.holders.iter().map(Option::is_some).collect(),
            })
            .collect()
    }

    /// The instances of sessions newly seen ended, for the caller to stop
    /// outside the lock; drops sessions ended for [`LINGER`] that no stream
    /// holds.
    pub(super) fn reap(&self) -> Vec<Arc<dyn Instance>> {
        let now = Instant::now();
        let mut stopping = Vec::new();
        self.lock().sessions.retain(|_, entry| {
            if !entry.instance.session(|s| s.ended()) {
                return true;
            }
            let since = *entry.ended.get_or_insert_with(|| {
                stopping.push(entry.instance.clone());
                now
            });
            entry.holders.iter().any(Option::is_some) || now.duration_since(since) < LINGER
        });
        stopping
    }
}
