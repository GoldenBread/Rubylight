//! Pure cross-host console library domain model, shared between Desktop and Android:
//! config/list records (`Recent`, `Favourite`, `HomeGroup`, `HomeSort`, `HomeOrder`, `ClassicApps`),
//! hidden-game tracking, cross-host merging, session/detail state, and Home filtering/grouping/sorting.

pub mod filters;
pub mod hidden;
pub mod merge;
pub mod projections;

#[cfg(test)]
mod tests;

pub use filters::{
    filtered, fit_row, home_rows, library_filters, offered_groups, offered_sorts, shown_group,
    store_name, FilterChip, HomeFilterKey, HomeProjection, HomeRow, LibraryFilter, LibraryMemo,
    Played, ProjectedCard, ProjectedChip, RowKind, Sort,
};
pub use hidden::{
    build_hidden_listings, hide_keys, hide_keys_by_id, is_hidden, is_hidden_entry, not_hidden,
    not_hidden_by_id, reconcile_starred, split_hidden, HiddenEntry, HiddenListing, HiddenTracker,
};
pub use merge::{
    classic_library, has_library, has_native_library, merge_library, merge_rich_library,
    opens_on_home, Copy, Entry, Merged, RichSource, Source, CLASSIC_REVISION,
};
pub use projections::{
    active, classic_app, continue_playing, detail, entry_key_of, favourite_for, favourite_games,
    favourite_of, host_line, is_starred, last_played, live, neighbour, recheck_running,
    recently_shown, saved_classic_app, stream_summary, Continue, Detail, EndAction, GameState,
    HostLine, Playing, Recently, Running, Starred,
};

use crate::http::App;
use crate::http::AppControl;
use crate::library_order::{Direction, SortKey};
use serde::{Deserialize, Serialize};

/// How Home groups the library rows (Android's `home_view_group`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HomeGroup {
    /// The whole library as one row.
    None,
    /// The games of every store in one row, the apps of no store (launchers, the desktop) in
    /// their own.
    Type,
    /// A row per Steam collection or Playnite category the PCs name, a game in several in each,
    /// then the games in none. Offered while they name any.
    Collection,
    /// The games some PC has installed, then the rest. Offered while the library has both.
    Installation,
    /// A row per store, then the apps of no store. Also what an unknown value reads as; keep it
    /// last: serde wants the catch-all there.
    #[default]
    #[serde(other)]
    Store,
}

impl HomeGroup {
    pub const ALL: [Self; 5] = [
        Self::Store,
        Self::Type,
        Self::Collection,
        Self::Installation,
        Self::None,
    ];

    pub fn english_label(self) -> &'static str {
        match self {
            Self::Store => "Store",
            Self::Type => "Type",
            Self::Collection => "Collection",
            Self::Installation => "Installation",
            Self::None => "None",
        }
    }
}

/// What Home's rows and All games sort by (Android's `home_view_sort`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HomeSort {
    /// When the PCs say the game was last played.
    LastPlayed,
    /// How long the PCs say the game was played.
    Playtime,
    /// Also what an unknown value reads as; keep it last: serde wants the catch-all there.
    #[default]
    #[serde(other)]
    Title,
}

impl HomeSort {
    pub const ALL: [Self; 3] = [Self::Title, Self::LastPlayed, Self::Playtime];

    pub fn english_label(self) -> &'static str {
        match self {
            Self::Title => "Title",
            Self::LastPlayed => "Last played",
            Self::Playtime => "Play time",
        }
    }

    pub fn key(self) -> SortKey {
        match self {
            Self::Title => SortKey::Title,
            Self::LastPlayed => SortKey::LastPlayed,
            Self::Playtime => SortKey::Playtime,
        }
    }

    pub fn from_wire(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "last_played" | "lastplayed" => Self::LastPlayed,
            "playtime" | "play_time" => Self::Playtime,
            _ => Self::Title,
        }
    }
}

/// Which way Home sorts (Android's `home_view_order`): ascending is A to Z, oldest first and least
/// played first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HomeOrder {
    Descending,
    /// Also what an unknown value reads as; keep it last: serde wants the catch-all there.
    #[default]
    #[serde(other)]
    Ascending,
}

impl HomeOrder {
    pub const ALL: [Self; 2] = [Self::Ascending, Self::Descending];

    pub fn english_label(self) -> &'static str {
        match self {
            Self::Ascending => "Ascending",
            Self::Descending => "Descending",
        }
    }

    pub fn direction(self) -> Direction {
        match self {
            Self::Ascending => Direction::Ascending,
            Self::Descending => Direction::Descending,
        }
    }

    pub fn from_wire(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "descending" | "desc" => Self::Descending,
            _ => Self::Ascending,
        }
    }
}

/// A recently launched game record, keyed by host UUID + install id (falling back to app id).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recent {
    pub host: String,
    pub install_id: String,
    pub app_id: u32,
    pub title: String,
    /// Seconds since the Unix epoch.
    pub at: u64,
}

/// The game a start of Nightjar goes on to, named like a [`Recent`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenGame {
    pub host: String,
    pub install_id: String,
    pub app_id: u32,
    pub title: String,
}

impl OpenGame {
    pub fn same_game(&self, other: &OpenGame) -> bool {
        self.host == other.host
            && if !self.install_id.is_empty() && !other.install_id.is_empty() {
                self.install_id.eq_ignore_ascii_case(&other.install_id)
            } else {
                self.app_id == other.app_id
            }
    }
}

/// What the automatic start keeps between runs (`open_play.json`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenPlayState {
    /// The game chosen in its menu.
    pub game: Option<OpenGame>,
    /// An automatic start began and has not ended well yet (`crate::open_play`).
    pub armed: bool,
}

/// How many recent games Desktop keeps.
pub const RECENT_LIMIT: usize = 12;
/// How many recent games Android keeps.
pub const ANDROID_RECENT_LIMIT: usize = 20;

impl Recent {
    pub fn same_game(&self, other: &Recent) -> bool {
        self.host == other.host
            && if !self.install_id.is_empty() && !other.install_id.is_empty() {
                self.install_id.eq_ignore_ascii_case(&other.install_id)
            } else {
                self.app_id == other.app_id
            }
    }
}

/// Puts `entry` first; the same game earlier in the list is replaced, and the list is capped at `limit`.
pub fn remember_with_limit(list: &mut Vec<Recent>, entry: Recent, limit: usize) {
    list.retain(|r| !r.same_game(&entry));
    list.insert(0, entry);
    list.truncate(limit);
}

/// Puts `entry` first; the same game earlier in the list is replaced, and the list is capped at [`RECENT_LIMIT`].
pub fn remember(list: &mut Vec<Recent>, entry: Recent) {
    remember_with_limit(list, entry, RECENT_LIMIT);
}

/// A game the player starred (or hid), keyed by host UUID + install id (falling back to app id),
/// so the same title on two PCs stays distinct and reordered classic libraries keep working.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Favourite {
    pub host: String,
    pub install_id: String,
    pub app_id: u32,
    /// Shown while the PC's library is not known (it is offline and nothing is saved).
    pub title: String,
}

impl Favourite {
    pub fn same_game(&self, other: &Favourite) -> bool {
        self.host == other.host
            && if !self.install_id.is_empty() && !other.install_id.is_empty() {
                self.install_id.eq_ignore_ascii_case(&other.install_id)
            } else {
                self.app_id == other.app_id
            }
    }

    /// A hidden game by either id: the PC's list knows it by its classic UUID and Home by its
    /// install id, and the app id is the same in both.
    pub fn same_hidden(&self, other: &Favourite) -> bool {
        self.same_game(other) || (self.host == other.host && self.app_id == other.app_id)
    }
}

/// Hides the `games` or shows them again, by the keys a favourite has. Returns whether the list
/// changed.
pub fn set_hidden(list: &mut Vec<Favourite>, games: Vec<Favourite>, hidden: bool) -> bool {
    let before = list.len();
    list.retain(|h| !games.iter().any(|game| game.same_hidden(h)));
    let dropped = before - list.len();
    if !hidden {
        return dropped > 0;
    }
    let added = games.len();
    list.extend(games);
    added != dropped
}

/// Stars `entry` (first in the list; a game already starred moves up and takes the new title)
/// or, when it is starred already, removes it. Returns whether the game is starred now.
pub fn toggle_favourite(list: &mut Vec<Favourite>, entry: Favourite) -> bool {
    let before = list.len();
    list.retain(|f| !f.same_game(&entry));
    if list.len() < before {
        return false;
    }
    list.insert(0, entry);
    true
}

/// The starred games in `text`, skipping entries that do not parse, and whether anything was
/// skipped (or the whole text was unreadable), so the caller can keep the original aside.
pub fn parse_favourites(text: &str) -> (Vec<Favourite>, bool) {
    let Ok(entries) = serde_json::from_str::<Vec<serde_json::Value>>(text) else {
        return (Vec::new(), true);
    };
    let total = entries.len();
    let list: Vec<Favourite> = entries
        .into_iter()
        .filter_map(|e| serde_json::from_value(e).ok())
        .collect();
    let damaged = list.len() < total;
    (list, damaged)
}

/// The version of the saved app list's format. A later format that only adds fields keeps it;
/// fields this build does not know are ignored, and missing ones take their defaults.
pub const CLASSIC_APPS_VERSION: u32 = 1;

/// A PC's app list as saved for Home: its games, without the control tiles.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassicApps {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub apps: Vec<ClassicApp>,
}

/// One game of a saved app list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassicApp {
    pub id: i32,
    #[serde(default)]
    pub name: String,
    /// The host's UUID for the app; empty when it sends none.
    #[serde(default)]
    pub uuid: String,
    /// The box art's version, empty when the host sends none.
    #[serde(default)]
    pub art_version: String,
}

impl ClassicApps {
    /// The games of a loaded app list, in its order: the control tiles (Resume, Disconnect,
    /// Terminate, Remote Input...) are not games and stay on the PC's own screen.
    pub fn of(apps: &[App]) -> Self {
        let apps = apps
            .iter()
            .filter(|a| a.control == AppControl::None)
            .map(|a| ClassicApp {
                id: a.id,
                name: a.name.clone(),
                uuid: a.uuid.clone(),
                art_version: a.art_version.clone(),
            })
            .collect();
        Self {
            version: CLASSIC_APPS_VERSION,
            apps,
        }
    }
}
