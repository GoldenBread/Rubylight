//! The requests to SteamGridDB, Twitch and IGDB, and the reading of their answers. Built from the
//! SteamGridDB API v2 reference and the IGDB API documentation.

use serde::Deserialize;
use serde_json::Value;

use super::cache::Facts;
use super::job::{Request, Service};
use super::names::path_segment;

pub const STEAMGRIDDB: &str = "https://www.steamgriddb.com/api/v2";
pub const TWITCH_TOKEN: &str = "https://id.twitch.tv/oauth2/token";
pub const IGDB: &str = "https://api.igdb.com/v4";

/// IGDB's id for Steam among the sources of `external_games` (the `category` enum's `steam`).
pub const IGDB_STEAM: u64 = 1;

/// The art filters every SteamGridDB image request carries: still images in formats both apps
/// decode, without the ones flagged adult, joke or flashing.
const ART_FILTERS: &str = "types=static&nsfw=false&humor=false&epilepsy=false";

fn sgdb(key: &str, path: String) -> Request {
    Request { service: Service::SteamGridDb, method: "GET".into(), url: format!("{STEAMGRIDDB}{path}"), headers: vec![("Authorization".into(), format!("Bearer {key}"))], body: None }
}

/// SteamGridDB's game for a Steam app id.
pub fn sgdb_by_steam(key: &str, app: u64) -> Request {
    sgdb(key, format!("/games/steam/{app}"))
}

/// SteamGridDB's games whose names are like `term`.
pub fn sgdb_search(key: &str, term: &str) -> Request {
    sgdb(key, format!("/search/autocomplete/{}", path_segment(term)))
}

/// A game's 600 by 900 covers.
pub fn sgdb_covers(key: &str, game: u64) -> Request {
    sgdb(key, format!("/grids/game/{game}?dimensions=600x900&mimes=image/png,image/jpeg&{ART_FILTERS}&limit=5"))
}

/// A game's wide heroes.
pub fn sgdb_heroes(key: &str, game: u64) -> Request {
    sgdb(key, format!("/heroes/game/{game}?dimensions=1920x620,3840x1240,1600x650&mimes=image/png,image/jpeg&{ART_FILTERS}&limit=5"))
}

/// A game's logos, PNG only (they need transparency).
pub fn sgdb_logos(key: &str, game: u64) -> Request {
    sgdb(key, format!("/logos/game/{game}?mimes=image/png&{ART_FILTERS}&limit=5"))
}

/// The id of the game in a SteamGridDB `/games/...` answer.
pub fn sgdb_game_id(body: &str) -> Option<u64> {
    let value: Value = serde_json::from_str(body).ok()?;
    value.get("success")?.as_bool().filter(|s| *s)?;
    value.get("data")?.get("id")?.as_u64()
}

/// The (id, name) of each game in a SteamGridDB search answer, in its order.
pub fn sgdb_games(body: &str) -> Option<Vec<(u64, String)>> {
    let value: Value = serde_json::from_str(body).ok()?;
    let data = value.get("data")?.as_array()?;
    Some(data.iter().filter_map(|g| Some((g.get("id")?.as_u64()?, g.get("name")?.as_str()?.to_owned()))).collect())
}

/// The URL of the best image in a SteamGridDB grids, heroes or logos answer: the highest score,
/// the service's own order deciding a tie. Some(None) for an answer without images; None for one
/// that is not such an answer.
pub fn sgdb_best_image(body: &str) -> Option<Option<String>> {
    let value: Value = serde_json::from_str(body).ok()?;
    let data = value.get("data").and_then(Value::as_array)?;
    let mut best: Option<(i64, &str)> = None;
    for image in data {
        let Some(url) = image.get("url").and_then(Value::as_str).filter(|u| u.starts_with("https://")) else { continue };
        let score = image.get("score").and_then(Value::as_i64).unwrap_or(0);
        if best.is_none_or(|(top, _)| score > top) {
            best = Some((score, url));
        }
    }
    Some(best.map(|(_, url)| url.to_owned()))
}

/// The client-credentials token request. Its URL carries the secret, so it is never logged.
pub fn twitch_token(client_id: &str, secret: &str) -> Request {
    Request {
        service: Service::Twitch,
        method: "POST".into(),
        url: format!("{TWITCH_TOKEN}?client_id={}&client_secret={}&grant_type=client_credentials", path_segment(client_id), path_segment(secret)),
        headers: Vec::new(),
        body: Some(String::new()),
    }
}

/// The token and its lifetime in seconds from a token answer.
pub fn twitch_token_answer(body: &str) -> Option<(String, u64)> {
    #[derive(Deserialize)]
    struct Answer {
        access_token: String,
        expires_in: u64,
    }
    let answer: Answer = serde_json::from_str(body).ok()?;
    (!answer.access_token.is_empty()).then_some((answer.access_token, answer.expires_in))
}

fn igdb(client_id: &str, token: &str, endpoint: &str, body: String) -> Request {
    Request {
        service: Service::Igdb,
        method: "POST".into(),
        url: format!("{IGDB}/{endpoint}"),
        headers: vec![("Client-ID".into(), client_id.to_owned()), ("Authorization".into(), format!("Bearer {token}")), ("Accept".into(), "application/json".into())],
        body: Some(body),
    }
}

/// `text` as an Apicalypse string's content: no quote or backslash can end it early.
fn quoted(text: &str) -> String {
    text.chars().filter(|c| !matches!(c, '"' | '\\') && !c.is_control()).collect()
}

/// The games IGDB knows under a Steam app id.
pub fn igdb_external(client_id: &str, token: &str, app: u64) -> Request {
    igdb(client_id, token, "external_games", format!("fields game,uid,external_game_source,category; where uid = \"{app}\"; limit 50;"))
}

/// The IGDB game of the Steam entries in an `external_games` answer, if one is Steam's.
pub fn igdb_external_game(body: &str) -> Option<Option<u64>> {
    let entries: Vec<Value> = serde_json::from_str(body).ok()?;
    let steam = |e: &Value, field: &str| e.get(field).and_then(Value::as_u64) == Some(IGDB_STEAM);
    Some(entries.iter().find(|e| steam(e, "external_game_source") || steam(e, "category")).and_then(|e| e.get("game")?.as_u64()))
}

const GAME_FIELDS: &str = "fields name,genres.name,themes.name;";

/// IGDB's main games (not editions) whose names are like `term`.
pub fn igdb_search(client_id: &str, token: &str, term: &str) -> Request {
    igdb(client_id, token, "games", format!("search \"{}\"; {GAME_FIELDS} where version_parent = null; limit 10;", quoted(term)))
}

/// One IGDB game by its id.
pub fn igdb_game(client_id: &str, token: &str, id: u64) -> Request {
    igdb(client_id, token, "games", format!("{GAME_FIELDS} where id = {id};"))
}

/// A game's time to beat.
pub fn igdb_time_to_beat(client_id: &str, token: &str, id: u64) -> Request {
    igdb(client_id, token, "game_time_to_beats", format!("fields game_id,hastily,normally,completely; where game_id = {id};"))
}

/// The (id, facts without time to beat) of each game in a `games` answer, in its order.
pub fn igdb_games(body: &str) -> Option<Vec<(u64, Facts)>> {
    let games: Vec<Value> = serde_json::from_str(body).ok()?;
    let names = |game: &Value, field: &str| -> Vec<String> {
        game.get(field).and_then(Value::as_array).map(|list| list.iter().filter_map(|g| g.get("name")?.as_str().map(str::to_owned)).collect()).unwrap_or_default()
    };
    Some(
        games
            .iter()
            .filter_map(|game| {
                let id = game.get("id")?.as_u64()?;
                let name = game.get("name")?.as_str()?.to_owned();
                Some((id, Facts { name, genres: names(game, "genres"), themes: names(game, "themes"), ..Facts::default() }))
            })
            .collect(),
    )
}

/// `facts` with the time to beat of a `game_time_to_beats` answer (seconds; a zero is no time).
pub fn igdb_time_to_beat_answer(body: &str, facts: &mut Facts) -> Option<()> {
    let entries: Vec<Value> = serde_json::from_str(body).ok()?;
    if let Some(entry) = entries.first() {
        let seconds = |field: &str| entry.get(field).and_then(Value::as_u64).filter(|s| *s > 0);
        facts.hastily = seconds("hastily");
        facts.normally = seconds("normally");
        facts.completely = seconds("completely");
    }
    Some(())
}
