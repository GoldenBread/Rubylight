//! The JSON calls the Android app makes through `libnightjar_core` (`protocol/android/src/game_info.rs`).
//! Every request is an object with an `op`; every answer is `{"ok": ...}` or `{"error": "invalid",
//! "detail": why}`. The app's `GameInfoModelTest` and this module's test read the same contract file.
//!
//! - `{"op": "clean", "name": n, "game_id": id | null}`: `{"clean": text, "key": key, "steam": id | null}`
//! - `{"op": "start", "game": {"name", "steam"}, "keys": {...}, "wants": {...}, "session": s | null,
//!   "wall_secs": t, "now_ms": t}`: `{"job": job, "step": step}`
//! - `{"op": "next", "job": job, "reply": {"status", "body", "retry_after"} | null, "now_ms": t}`:
//!   `{"job": job, "step": step}`. A step is `{"send": request}`, `{"wait": ms}` or `{"done":
//!   {"record", "session", "log"}}`.
//! - `{"op": "fresh", "record": r, "now_secs": t, "keys": {...}}`: `{"fresh": bool}`
//! - `{"op": "library", "games": [facts | null], "genre": g | null, "length": l | null}`:
//!   `{"genres": [{"value", "count"}], "lengths": [{"value", "count"}], "shown": [place]}`
//! - `{"op": "facts", "facts": f}`: `{"length": l | null, "hastily": h | null, "normally": h | null,
//!   "completely": h | null}` with hours as text
//! - `{"op": "files", "key": k, "url": u | null}`: `{"record": name, "image": name | null}`
//! - `{"op": "redact", "text": t, "secrets": [s]}`: `{"text": t}`

use serde::Deserialize;
use serde_json::{json, Value};

use super::cache::{image_file, record_file, Facts, Record};
use super::job::{Game, Job, Keys, Reply, Session, Wants};
use super::library::{self, Length};
use super::names;
use super::redact::redact;

#[derive(Deserialize)]
#[allow(clippy::large_enum_variant)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Call {
    Clean {
        name: String,
        #[serde(default)]
        game_id: Option<String>,
    },
    Start {
        game: Game,
        keys: Keys,
        wants: Wants,
        #[serde(default)]
        session: Option<Session>,
        wall_secs: u64,
        now_ms: u64,
    },
    Next {
        job: Job,
        #[serde(default)]
        reply: Option<Reply>,
        now_ms: u64,
    },
    Fresh {
        record: Record,
        now_secs: u64,
        keys: Keys,
    },
    Library {
        games: Vec<Option<Facts>>,
        #[serde(default)]
        genre: Option<String>,
        #[serde(default)]
        length: Option<Length>,
    },
    Facts {
        facts: Facts,
    },
    Files {
        key: String,
        #[serde(default)]
        url: Option<String>,
    },
    Redact {
        text: String,
        secrets: Vec<String>,
    },
}

fn answer(call: Call) -> Value {
    match call {
        Call::Clean { name, game_id } => {
            let steam = names::steam_id(game_id.as_deref());
            json!({"clean": names::clean(&name), "key": names::key(&name, steam), "steam": steam})
        }
        Call::Start { game, keys, wants, session, wall_secs, now_ms } => {
            let mut job = Job::new(game, keys, wants, session, wall_secs);
            let step = job.next(None, now_ms);
            json!({"job": job, "step": step})
        }
        Call::Next { mut job, reply, now_ms } => {
            let step = job.next(reply, now_ms);
            json!({"job": job, "step": step})
        }
        Call::Fresh { record, now_secs, keys } => json!({"fresh": record.fresh(now_secs, keys.available())}),
        Call::Library { games, genre, length } => {
            let facts: Vec<Option<&Facts>> = games.iter().map(Option::as_ref).collect();
            let shown: Vec<usize> = facts.iter().enumerate().filter(|(_, f)| library::shows(**f, genre.as_deref(), length)).map(|(at, _)| at).collect();
            json!({"genres": library::genres(&facts), "lengths": library::lengths(&facts), "shown": shown})
        }
        Call::Facts { facts } => json!({
            "length": Length::of(&facts),
            "hastily": library::hours(facts.hastily),
            "normally": library::hours(facts.normally),
            "completely": library::hours(facts.completely),
        }),
        Call::Files { key, url } => json!({"record": record_file(&key), "image": url.as_deref().map(image_file)}),
        Call::Redact { text, secrets } => {
            let secrets: Vec<&str> = secrets.iter().map(String::as_str).collect();
            json!({"text": redact(&text, &secrets)})
        }
    }
}

/// Answers one JSON request (see the module's documentation).
pub fn call(request: &str) -> String {
    let answer = match serde_json::from_str::<Call>(request) {
        Ok(call) => json!({"ok": answer(call)}),
        // The error names a field, never a value: serde's messages quote no input for these types.
        Err(error) => json!({"error": "invalid", "detail": redact(&error.to_string(), &[])}),
    };
    answer.to_string()
}
