//! Covers from SteamGridDB for apps that have none: which requests the host makes, what it
//! reads from the answers and how long it waits after a refusal. The host's
//! `steamgriddb` module makes the requests and saves the covers.
//!
//! Written for Rubylight from SteamGridDB's public API v2 description
//! (<https://www.steamgriddb.com/api/v2>): `Authorization: Bearer <key>` on every API request,
//! `/grids/steam/{appid}` and `/grids/game/{id}` for grids, `/search/autocomplete/{term}` for a
//! game by name.

use serde_json::Value;
use std::time::Duration;

/// The API's base address.
pub const API: &str = "https://www.steamgriddb.com/api/v2";
/// The setting that holds the user's API key.
pub const KEY_SETTING: &str = "steamgriddb_api_key";
/// What the console is shown for a saved key, and a save that sends it back keeps the key.
pub const MASK: &str = "********";

/// The file in the profile's administrators-only `credentials` folder that holds the key the
/// console saved. A key written into `sunshine.conf` by hand is read too.
pub const KEY_FILE: &str = "steamgriddb_api_key";
/// The app field that says its cover came from SteamGridDB: the `image-path` it was given. A
/// launcher's or Playnite's cover replaces such a cover; one that no longer matches it was
/// cleared or replaced, and the app never gets a SteamGridDB cover again.
pub const COVER_FIELD: &str = "steamgriddb-cover";

/// Whether the app's cover is the one SteamGridDB gave it.
pub fn fetched_cover(app: &Value) -> bool {
    let marker = app.get(COVER_FIELD).and_then(Value::as_str).unwrap_or("");
    !marker.is_empty() && app.get("image-path").and_then(Value::as_str) == Some(marker)
}

/// The kind of a downloaded image, `png` or `jpeg`, by its first bytes. Its `Content-Type`
/// only refuses it when it names something else than an image (`text/html`, say): missing,
/// `application/octet-stream` or any `image/` type leave the bytes to decide.
pub fn image_type(content_type: Option<&str>, bytes: &[u8]) -> Option<&'static str> {
    let sniffed = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        "jpeg"
    } else {
        return None;
    };
    let declared = content_type.map(|value| {
        value
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase()
    });
    let image = declared.as_deref().is_none_or(|declared| {
        declared.is_empty()
            || declared.starts_with("image/")
            || declared == "application/octet-stream"
    });
    image.then_some(sniffed)
}

/// Portrait grids as Moonlight shows covers: 600x900, still images the host can convert, without
/// images tagged NSFW, humor or epilepsy.
const GRID_FILTER: &str = "dimensions=600x900&mimes=image/png,image/jpeg&types=static&nsfw=false&humor=false&epilepsy=false";

/// The API key as saved, trimmed; `None` when unset.
pub fn key(value: &str) -> Option<&str> {
    let key = value.trim();
    (!key.is_empty()).then_some(key)
}

/// Why a saved key cannot be used. SteamGridDB's keys are 32 hexadecimal digits; anything but
/// letters and digits would not fit in a request header. The key itself is never in the message.
pub fn check_key(value: &str) -> Result<(), String> {
    let Some(key) = key(value) else {
        return Ok(());
    };
    if key == MASK {
        return Ok(());
    }
    if key.len() > 128 || !key.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err(format!(
            "{KEY_SETTING} must be the API key from your SteamGridDB preferences: letters and digits only"
        ));
    }
    Ok(())
}

/// The 600x900 grids of a Steam game, by its Steam app ID.
pub fn steam_grids_url(appid: u32) -> String {
    format!("{API}/grids/steam/{appid}?{GRID_FILTER}")
}

/// The 600x900 grids of a SteamGridDB game.
pub fn game_grids_url(game: u64) -> String {
    format!("{API}/grids/game/{game}?{GRID_FILTER}")
}

/// The search for a game by name, or `None` for a name with nothing to search for.
pub fn search_url(name: &str) -> Option<String> {
    let name = name.trim();
    if normalized(name).is_empty() {
        return None;
    }
    Some(format!("{API}/search/autocomplete/{}", path_segment(name)))
}

/// `text` as one URL path segment: every byte but the unreserved ones percent-encoded.
fn path_segment(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// A name for comparison: lower case, letters and digits only, words joined by one space.
fn normalized(name: &str) -> String {
    name.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The answer's `data` list when it says it succeeded.
fn data(body: &[u8]) -> Option<Vec<Value>> {
    let value: Value = serde_json::from_slice(body).ok()?;
    if value.get("success").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    value.get("data")?.as_array().cloned()
}

/// Whether `url` is an HTTPS address on SteamGridDB's own servers, where its images are.
fn steamgriddb_image(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    !host.contains(['@', ':']) && (host == "steamgriddb.com" || host.ends_with(".steamgriddb.com"))
}

/// The best-scored grid's image address in a grids answer: the first of the highest score, on
/// SteamGridDB's servers. `None` when the game has none.
pub fn best_grid(body: &[u8]) -> Option<String> {
    let grids = data(body)?;
    let mut best: Option<(i64, String)> = None;
    for grid in &grids {
        let Some(url) = grid.get("url").and_then(Value::as_str) else {
            continue;
        };
        if !steamgriddb_image(url) {
            continue;
        }
        let score = grid.get("score").and_then(Value::as_i64).unwrap_or(0);
        if best.as_ref().is_none_or(|(top, _)| score > *top) {
            best = Some((score, url.to_owned()));
        }
    }
    best.map(|(_, url)| url)
}

/// The SteamGridDB game in a search answer that is the app `name`: the first whose name equals
/// it, ignoring case and punctuation, else the first whose name starts with it or that it starts
/// with as whole words ("The Witcher 3" and "The Witcher 3: Wild Hunt"). Another game is never
/// taken, so an app named after no game gets no cover rather than a wrong one.
pub fn matching_game(body: &[u8], name: &str) -> Option<u64> {
    let games = data(body)?;
    let wanted = normalized(name);
    if wanted.is_empty() {
        return None;
    }
    let named: Vec<(u64, String)> = games
        .iter()
        .filter_map(|game| {
            Some((
                game.get("id")?.as_u64()?,
                normalized(game.get("name")?.as_str()?),
            ))
        })
        .collect();
    let starts = |long: &str, short: &str| {
        long.strip_prefix(short)
            .is_some_and(|rest| rest.starts_with(' '))
    };
    named
        .iter()
        .find(|(_, found)| *found == wanted)
        .or_else(|| {
            named
                .iter()
                .find(|(_, found)| starts(found, &wanted) || starts(&wanted, found))
        })
        .map(|(id, _)| *id)
}

/// The shortest and longest waits after SteamGridDB asked for fewer requests (HTTP 429).
pub const BACKOFF_MIN: Duration = Duration::from_secs(60);
pub const BACKOFF_MAX: Duration = Duration::from_secs(60 * 60);

/// How long to wait after a 429: the `Retry-After` seconds it gave, at least
/// [`BACKOFF_MIN`] and at most a day, else twice the last wait (`previous`, zero for the first),
/// between [`BACKOFF_MIN`] and [`BACKOFF_MAX`].
pub fn backoff(retry_after: Option<&str>, previous: Duration) -> Duration {
    if let Some(seconds) = retry_after.and_then(|value| value.trim().parse::<u64>().ok()) {
        return Duration::from_secs(seconds).clamp(BACKOFF_MIN, Duration::from_secs(86_400));
    }
    previous.saturating_mul(2).clamp(BACKOFF_MIN, BACKOFF_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_requests_ask_for_portrait_grids_and_encode_the_name() {
        assert_eq!(
            steam_grids_url(570),
            "https://www.steamgriddb.com/api/v2/grids/steam/570?dimensions=600x900&mimes=image/png,image/jpeg&types=static&nsfw=false&humor=false&epilepsy=false"
        );
        assert!(
            game_grids_url(42)
                .starts_with("https://www.steamgriddb.com/api/v2/grids/game/42?dimensions=600x900")
        );
        assert_eq!(
            search_url(" Half-Life 2: Episode One/Two? ").unwrap(),
            "https://www.steamgriddb.com/api/v2/search/autocomplete/Half-Life%202%3A%20Episode%20One%2FTwo%3F"
        );
        assert_eq!(
            search_url("Pokémon").unwrap(),
            "https://www.steamgriddb.com/api/v2/search/autocomplete/Pok%C3%A9mon"
        );
        assert_eq!(search_url(" ?! "), None);
    }

    #[test]
    fn the_best_grid_is_the_top_score_on_steamgriddb_servers() {
        let body = br#"{"success":true,"data":[
            {"id":1,"score":3,"url":"https://cdn2.steamgriddb.com/grid/a.png"},
            {"id":2,"score":9,"url":"http://cdn2.steamgriddb.com/grid/plain.png"},
            {"id":3,"score":9,"url":"https://evil.example/grid.png"},
            {"id":4,"score":9,"url":"https://cdn2.steamgriddb.com@evil.example/grid.png"},
            {"id":5,"score":7,"url":"https://cdn2.steamgriddb.com/grid/b.jpg"},
            {"id":6,"score":7,"url":"https://cdn2.steamgriddb.com/grid/c.jpg"}
        ]}"#;
        assert_eq!(
            best_grid(body).as_deref(),
            Some("https://cdn2.steamgriddb.com/grid/b.jpg")
        );
        assert_eq!(best_grid(br#"{"success":true,"data":[]}"#), None);
        assert_eq!(
            best_grid(br#"{"success":false,"errors":["Game not found"]}"#),
            None
        );
        assert_eq!(best_grid(b"not json"), None);
    }

    #[test]
    fn a_search_takes_only_the_game_the_app_is_named_after() {
        let body = br#"{"success":true,"data":[
            {"id":10,"name":"The Witcher 3: Wild Hunt"},
            {"id":11,"name":"The Witcher"},
            {"id":12,"name":"THE WITCHER 3"}
        ]}"#;
        assert_eq!(
            matching_game(body, "the witcher 3"),
            Some(12),
            "the exact name first"
        );
        assert_eq!(matching_game(body, "The Witcher 3 Wild Hunt"), Some(10));
        assert_eq!(
            matching_game(body, "The Witcher 3: Wild Hunt GOTY"),
            Some(10)
        );
        assert_eq!(matching_game(body, "The Witch"), None, "only whole words");
        assert_eq!(matching_game(body, "Remote Desktop"), None);
        assert_eq!(
            matching_game(br#"{"success":true,"data":[]}"#, "Doom"),
            None
        );
    }

    #[test]
    fn only_png_or_jpeg_bytes_not_sent_as_another_type_are_an_image() {
        let png = b"\x89PNG\r\n\x1a\nrest";
        let jpeg = [0xff, 0xd8, 0xff, 0xe0];
        assert_eq!(image_type(Some("image/png"), png), Some("png"));
        assert_eq!(
            image_type(Some("Image/JPEG; charset=x"), &jpeg),
            Some("jpeg")
        );
        assert_eq!(image_type(None, png), Some("png"));
        assert_eq!(
            image_type(Some("image/jpeg"), png),
            Some("png"),
            "the bytes decide"
        );
        assert_eq!(
            image_type(Some("application/octet-stream"), &jpeg),
            Some("jpeg")
        );
        assert_eq!(image_type(Some("application/json"), &jpeg), None);
        assert_eq!(image_type(Some("text/html"), png), None);
        assert_eq!(image_type(Some("image/png"), b"<html>"), None);
        assert_eq!(image_type(Some("image/webp"), b"RIFF....WEBP"), None);
    }

    #[test]
    fn a_cover_is_steamgriddbs_while_its_mark_matches() {
        use serde_json::json;
        assert!(fetched_cover(
            &json!({"image-path": "C:/c/sgdb_a.png", COVER_FIELD: "C:/c/sgdb_a.png"})
        ));
        assert!(
            !fetched_cover(&json!({"image-path": "", COVER_FIELD: "C:/c/sgdb_a.png"})),
            "cleared"
        );
        assert!(!fetched_cover(
            &json!({"image-path": "D:/mine.png", COVER_FIELD: "C:/c/sgdb_a.png"})
        ));
        assert!(!fetched_cover(&json!({"image-path": "D:/mine.png"})));
    }

    #[test]
    fn a_429_waits_what_it_asks_or_doubles() {
        assert_eq!(
            backoff(Some("120"), Duration::ZERO),
            Duration::from_secs(120)
        );
        assert_eq!(backoff(Some("1"), Duration::ZERO), BACKOFF_MIN);
        assert_eq!(
            backoff(Some("999999"), Duration::ZERO),
            Duration::from_secs(86_400)
        );
        assert_eq!(backoff(None, Duration::ZERO), BACKOFF_MIN);
        assert_eq!(backoff(Some("soon"), BACKOFF_MIN), BACKOFF_MIN * 2);
        assert_eq!(backoff(None, BACKOFF_MAX), BACKOFF_MAX);
    }

    #[test]
    fn a_key_is_letters_and_digits_and_the_mask_keeps_the_saved_one() {
        assert!(check_key("").is_ok());
        assert!(check_key(" 0123456789abcdef0123456789abcdef ").is_ok());
        assert!(check_key(MASK).is_ok());
        let error = check_key("abc def\r\nX-Other: 1").unwrap_err();
        assert!(!error.contains("abc def"), "{error}");
        assert_eq!(key("  "), None);
        assert_eq!(key(" k "), Some("k"));
    }

    #[test]
    fn the_setting_reads_a_saved_key_or_nothing() {
        use crate::config::Config;
        let read = |text: &str| {
            Config::parse(text)
                .unwrap()
                .steamgriddb_api_key()
                .map(str::to_owned)
        };
        assert_eq!(read(""), None, "unset by default");
        assert_eq!(
            read("steamgriddb_api_key = abc123\n").as_deref(),
            Some("abc123")
        );
        assert_eq!(
            read("steamgriddb_api_key = \"abc123\"\n").as_deref(),
            Some("abc123")
        );
        assert_eq!(
            read("steamgriddb_api_key = ********\n"),
            None,
            "the mask is no key"
        );
        assert_eq!(read("steamgriddb_api_key = a b\n"), None);
    }
}
