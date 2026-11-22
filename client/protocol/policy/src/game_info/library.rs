//! The library's filters by genre and by length, from the facts of its games.

use serde::{Deserialize, Serialize};

use super::cache::Facts;

const HOUR: u64 = 60 * 60;

/// How long a game takes, by its usual time to beat ([`Facts::normally`], else the hasty time,
/// else the complete one).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Length {
    /// Under 5 hours.
    Short,
    /// 5 to 15 hours.
    Medium,
    /// 15 to 40 hours.
    Long,
    /// 40 hours or more.
    VeryLong,
}

impl Length {
    pub const ALL: [Length; 4] = [Length::Short, Length::Medium, Length::Long, Length::VeryLong];

    /// The bucket of a time to beat in seconds.
    pub fn of_seconds(seconds: u64) -> Length {
        match seconds {
            s if s < 5 * HOUR => Length::Short,
            s if s < 15 * HOUR => Length::Medium,
            s if s < 40 * HOUR => Length::Long,
            _ => Length::VeryLong,
        }
    }

    /// The bucket of a game, None when IGDB has no time for it.
    pub fn of(facts: &Facts) -> Option<Length> {
        facts.normally.or(facts.hastily).or(facts.completely).map(Length::of_seconds)
    }

    /// The stable name both apps store and send: `short`, `medium`, `long`, `very_long`.
    pub fn key(self) -> &'static str {
        match self {
            Length::Short => "short",
            Length::Medium => "medium",
            Length::Long => "long",
            Length::VeryLong => "very_long",
        }
    }

    pub fn from_key(key: &str) -> Option<Length> {
        Length::ALL.into_iter().find(|l| l.key() == key)
    }
}

/// A time to beat in hours as people say it: under 10 to the half hour ("1.5"), else whole ("12").
/// None for no time.
pub fn hours(seconds: Option<u64>) -> Option<String> {
    let seconds = seconds?;
    let halves = (seconds + HOUR / 4) / (HOUR / 2);
    if halves == 0 {
        return Some("0.5".into());
    }
    if halves < 20 {
        return Some(if halves.is_multiple_of(2) { format!("{}", halves / 2) } else { format!("{}.5", halves / 2) });
    }
    Some(format!("{}", (seconds + HOUR / 2) / HOUR))
}

/// A choice offered: a genre's name or a length, and how many of the games have it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice<T> {
    pub value: T,
    pub count: usize,
}

/// The genres of `games` (None for a game without facts), most games first, then by name.
/// Two names that differ only in case are one genre, under the first spelling seen.
pub fn genres(games: &[Option<&Facts>]) -> Vec<Choice<String>> {
    let mut found: Vec<Choice<String>> = Vec::new();
    for facts in games.iter().flatten() {
        let mut seen: Vec<String> = Vec::new();
        for genre in &facts.genres {
            let lower = genre.to_lowercase();
            if seen.contains(&lower) {
                continue;
            }
            seen.push(lower.clone());
            match found.iter_mut().find(|c| c.value.to_lowercase() == lower) {
                Some(choice) => choice.count += 1,
                None => found.push(Choice { value: genre.clone(), count: 1 }),
            }
        }
    }
    found.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.to_lowercase().cmp(&b.value.to_lowercase())));
    found
}

/// The lengths `games` have, shortest first.
pub fn lengths(games: &[Option<&Facts>]) -> Vec<Choice<Length>> {
    Length::ALL
        .into_iter()
        .filter_map(|length| {
            let count = games.iter().flatten().filter(|f| Length::of(f) == Some(length)).count();
            (count > 0).then_some(Choice { value: length, count })
        })
        .collect()
}

/// Whether a game with `facts` is shown under the `genre` and `length` chosen (None: any). A
/// game without facts shows only while neither is chosen.
pub fn shows(facts: Option<&Facts>, genre: Option<&str>, length: Option<Length>) -> bool {
    if genre.is_none() && length.is_none() {
        return true;
    }
    let Some(facts) = facts else { return false };
    let genre_ok = genre.is_none_or(|g| facts.genres.iter().any(|have| have.to_lowercase() == g.to_lowercase()));
    let length_ok = length.is_none_or(|l| Length::of(facts) == Some(l));
    genre_ok && length_ok
}
