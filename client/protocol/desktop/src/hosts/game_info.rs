//! Artwork and facts from SteamGridDB and IGDB for the PCs' games ([`crate::game_info`]): the
//! worker that looks facts up for the library's filters and game pages, and what the pages read.

use nightjar_policy::game_info::cache::{Facts, Record};
use nightjar_policy::game_info::job::{Game, Keys};

use super::*;

impl Hosts {
    /// Starts looking games up with `info`: a worker for facts, and SteamGridDB covers in the
    /// cover workers.
    pub fn set_game_info(&mut self, info: crate::game_info::Shared) {
        let (sender, games) = channel::<Game>();
        let events = self.sender.clone();
        let waker = self.waker.clone();
        let worker = info.clone();
        let started = std::thread::Builder::new().name("nightjar-game-info".into()).spawn(move || {
            for game in games {
                if let Some(record) = worker.record(&game, false) {
                    if events.send(Event::GameInfo(Box::new(record))).is_err() {
                        return;
                    }
                    waker();
                }
            }
        });
        if started.is_ok() {
            self.facts_worker = Some(sender);
        }
        self.game_info = Some(info);
    }

    /// The keys in use, empty without any.
    pub fn game_info_keys(&self) -> Keys {
        self.game_info.as_ref().map(|i| i.keys()).unwrap_or_default()
    }

    /// Takes new keys: covers that found none are asked for again, and so are facts.
    pub fn set_game_info_keys(&mut self, keys: Keys) {
        let Some(info) = &self.game_info else { return };
        if info.keys() == keys {
            return;
        }
        info.set_keys(keys);
        self.facts_asked.clear();
        for host in &mut self.list {
            host.art.retain(|_, art| !matches!(art, Art::Missing));
        }
    }

    /// Whether facts can be looked up: IGDB has its keys.
    pub fn facts_offered(&self) -> bool {
        self.game_info.as_ref().is_some_and(|i| i.keys().info())
    }

    /// The facts of the game under `key` ([`crate::game_info::entry_key`]), when known.
    pub fn facts(&self, key: &str) -> Option<&Facts> {
        self.facts.get(key)
    }

    /// Whether the facts of `key` were asked for since the keys last changed.
    pub fn facts_asked(&self, key: &str) -> bool {
        self.facts_asked.contains(key)
    }

    /// Asks for the facts of `games` that were not asked for yet: from disk when kept, else from
    /// IGDB, one at a time on the worker.
    pub fn want_facts(&mut self, games: Vec<Game>) {
        if !self.facts_offered() {
            return;
        }
        let Some(worker) = &self.facts_worker else { return };
        for game in games {
            let key = nightjar_policy::game_info::names::key(&game.name, game.steam);
            if self.facts_asked.insert(key) && worker.send(game).is_err() {
                return;
            }
        }
    }

    /// Takes in a record a worker finished.
    pub(super) fn game_info_landed(&mut self, record: Record) {
        match record.facts.filter(|f| !f.is_empty()) {
            Some(facts) => {
                self.facts.insert(record.key, facts);
            }
            None => {
                self.facts.remove(&record.key);
            }
        }
    }

    /// What a cover worker needs to fill a gap with SteamGridDB's cover for `app` on `host`:
    /// None without its key, or for an app the PC's list does not name.
    pub(super) fn cover_fallback(&self, host: &Host, app: i32) -> Option<(crate::game_info::Shared, Game)> {
        let info = self.game_info.clone().filter(|i| i.keys().art())?;
        let game = host.library()?.games.iter().find(|g| g.app_id as i32 == app)?;
        let store = game.store.as_ref().map(|s| (s.source.as_str(), s.id.as_str()));
        Some((info, crate::game_info::game_of(&game.title, game.game_id.as_deref(), store)))
    }

    /// Facts put in place without a worker, for tests and the demo's screenshots.
    #[cfg(any(test, feature = "demo-library"))]
    pub(crate) fn put_facts(&mut self, key: &str, facts: Facts) {
        self.facts.insert(key.to_owned(), facts);
    }
}
