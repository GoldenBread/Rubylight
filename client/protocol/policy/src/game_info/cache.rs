//! What a lookup found for one game, as the apps keep it on disk, and how long it holds.

use serde::{Deserialize, Serialize};

/// A day, in seconds.
pub const DAY: u64 = 24 * 60 * 60;
/// How long what was found holds: art and facts change rarely.
pub const FOUND_FOR: u64 = 30 * DAY;
/// How long "the service has no such game" holds: a new game may be added soon.
pub const NOT_FOUND_FOR: u64 = 7 * DAY;
/// How long a failure (no network, the service down, too many requests) holds before the next try.
pub const FAILED_FOR: u64 = 60 * 60;

/// How one service's part of a lookup ended.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Not asked: no key for it, or not wanted.
    #[default]
    Skipped,
    /// It knows the game and gave what it has.
    Found,
    /// It has no game of that name or id.
    NotFound,
    /// It could not be asked or did not answer usably; tried again after [`FAILED_FOR`].
    Failed,
}

/// Genres, themes and time to beat from IGDB.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Facts {
    /// IGDB's name for the game.
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub themes: Vec<String>,
    /// Average seconds to the credits without extras.
    #[serde(default)]
    pub hastily: Option<u64>,
    /// Average seconds to the credits with some extras.
    #[serde(default)]
    pub normally: Option<u64>,
    /// Average seconds to finish everything.
    #[serde(default)]
    pub completely: Option<u64>,
}

impl Facts {
    /// Whether there is anything to show.
    pub fn is_empty(&self) -> bool {
        self.genres.is_empty() && self.themes.is_empty() && self.hastily.is_none() && self.normally.is_none() && self.completely.is_none()
    }
}

/// One game's lookup, kept under its [`super::names::key`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub key: String,
    /// When it was made, in seconds since the Unix epoch.
    pub fetched_at: u64,
    /// SteamGridDB's 600 by 900 cover, its wide hero and its logo, as image URLs.
    #[serde(default)]
    pub cover: Option<String>,
    #[serde(default)]
    pub hero: Option<String>,
    #[serde(default)]
    pub logo: Option<String>,
    #[serde(default)]
    pub facts: Option<Facts>,
    #[serde(default)]
    pub art: Outcome,
    #[serde(default)]
    pub info: Outcome,
    /// When the art part was asked, when it was kept from an older record ([`Record::merged`]);
    /// None is `fetched_at`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art_at: Option<u64>,
    /// When the facts part was asked, likewise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info_at: Option<u64>,
    /// A hash of the key the art part was asked with (`Keys::art_fingerprint`), so a failure
    /// with a wrong key does not outlive the corrected one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art_keys: Option<u64>,
    /// A hash of the IGDB client ID and secret the facts part was asked with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info_keys: Option<u64>,
}

/// Which services have keys now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Available {
    pub art: bool,
    pub info: bool,
}

/// How long one part holds, from how it ended; None when it is to be asked now.
fn holds_for(outcome: Outcome, available: bool) -> Option<u64> {
    match outcome {
        // A key added since: ask now. Still no key: nothing to ask.
        Outcome::Skipped => (!available).then_some(u64::MAX),
        Outcome::Found => Some(FOUND_FOR),
        Outcome::NotFound => Some(NOT_FOUND_FOR),
        Outcome::Failed => Some(FAILED_FOR),
    }
}

impl Record {
    /// Whether `self` still holds at `now` (seconds since the epoch) with the keys `available`:
    /// each part within its time, and no part skipped for want of a key that is there now. A
    /// record from the future (a clock set back) is taken as made now.
    ///
    /// `available` is a service that has its key *and* is wanted by this lookup: a facts-only
    /// lookup passes `art: false`, so a record without art holds for it.
    pub fn fresh(&self, now: u64, available: Available) -> bool {
        let age = |at: Option<u64>| now.saturating_sub(at.unwrap_or(self.fetched_at));
        [(self.art, available.art, age(self.art_at)), (self.info, available.info, age(self.info_at))]
            .into_iter()
            .all(|(outcome, available, age)| holds_for(outcome, available).is_some_and(|limit| age < limit))
    }

    /// [`Self::fresh`] for a lookup with `keys`: besides, a part wanted that failed with other keys
    /// than these is asked again now (a key corrected after a refusal).
    pub fn fresh_with(&self, now: u64, available: Available, keys: &super::job::Keys) -> bool {
        let failed_elsewhere = |outcome: Outcome, available: bool, asked_with: Option<u64>, now_with: u64| available && outcome == Outcome::Failed && asked_with != Some(now_with);
        self.fresh(now, available)
            && !failed_elsewhere(self.art, available.art, self.art_keys, keys.art_fingerprint())
            && !failed_elsewhere(self.info, available.info, self.info_keys, keys.info_fingerprint())
    }

    /// `self`, a new lookup's record, with the parts it did not ask ([`Outcome::Skipped`]) taken
    /// from `older`, the record kept before, with their own times. A facts-only lookup so keeps
    /// the art found before, and the reverse.
    pub fn merged(mut self, older: Option<&Record>) -> Record {
        let Some(older) = older.filter(|o| o.key == self.key) else { return self };
        if self.art == Outcome::Skipped && older.art != Outcome::Skipped {
            self.art = older.art;
            self.art_at = Some(older.art_at.unwrap_or(older.fetched_at));
            self.art_keys = older.art_keys;
            self.cover = older.cover.clone();
            self.hero = older.hero.clone();
            self.logo = older.logo.clone();
        }
        if self.info == Outcome::Skipped && older.info != Outcome::Skipped {
            self.info = older.info;
            self.info_at = Some(older.info_at.unwrap_or(older.fetched_at));
            self.info_keys = older.info_keys;
            self.facts = older.facts.clone();
        }
        // Facts found before a failed retry stay shown until a lookup gets them again.
        if self.info == Outcome::Failed && self.facts.is_none() {
            self.facts = older.facts.clone();
        }
        if self.art == Outcome::Failed && self.cover.is_none() && self.hero.is_none() && self.logo.is_none() {
            self.cover = older.cover.clone();
            self.hero = older.hero.clone();
            self.logo = older.logo.clone();
        }
        self
    }

    /// The image for `kind` (cover, hero, logo).
    pub fn image(&self, kind: super::job::Art) -> Option<&str> {
        match kind {
            super::job::Art::Cover => self.cover.as_deref(),
            super::job::Art::Hero => self.hero.as_deref(),
            super::job::Art::Logo => self.logo.as_deref(),
        }
    }
}

/// The file name an image URL is kept under: a stable hash of the URL in hex, with the URL's
/// extension when it is one the apps decode.
pub fn image_file(url: &str) -> String {
    // FNV-1a, 64 bits: stable across builds and platforms, unlike `std`'s hasher.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in url.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    let extension = url.rsplit('.').next().map(str::to_ascii_lowercase).filter(|e| matches!(e.as_str(), "png" | "jpg" | "jpeg"));
    match extension {
        Some(extension) => format!("{hash:016x}.{extension}"),
        None => format!("{hash:016x}.img"),
    }
}

/// The file name a record is kept under: its key's hash in hex, so any name is a safe file name.
pub fn record_file(key: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in key.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}.json")
}
