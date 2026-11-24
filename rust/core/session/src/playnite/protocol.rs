//! Playnite plugin JSON message parsing, ISO 8601 time conversion, and manifest version extraction.
use serde_json::Value;

/// A game as the plugin describes it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Game {
    pub id: String,
    pub name: String,
    pub exe: String,
    pub args: String,
    pub working_dir: String,
    pub install_dir: String,
    pub categories: Vec<String>,
    pub plugin_id: String,
    pub plugin_name: String,
    /// Minutes played, `None` when the plugin sent none or one that doesn't parse.
    pub playtime_minutes: Option<u64>,
    /// Unix seconds, if Playnite has a valid last played time.
    pub last_played: Option<i64>,
    /// Playnite sent a last played time that doesn't parse: when is unknown, not never.
    pub last_played_unknown: bool,
    pub box_art_path: String,
    pub icon_path: String,
    pub installed: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    SnapshotStart,
    SnapshotComplete,
    Plugins(Vec<(String, String)>),
    Categories(Vec<(String, String)>),
    Games(Vec<Game>),
    /// gameStarted, gameStopped, stopRequested or playniteExiting.
    Status {
        name: String,
        id: String,
        install_dir: String,
        exe: String,
    },
    /// The answer to a command that asked for one (`set-cover`).
    CommandResult {
        command: String,
        request_id: String,
        success: bool,
        error: String,
    },
    Other,
}
fn text(value: &Value, key: &str) -> String {
    match value.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}
fn pairs(value: Option<&Value>) -> Vec<(String, String)> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| (text(item, "id"), text(item, "name")))
                .filter(|(id, name)| !id.is_empty() || !name.is_empty())
                .collect()
        })
        .unwrap_or_default()
}
/// One line from the plugin (a UTF-8 BOM is allowed), reading times without an
/// offset as UTC.
pub fn parse(line: &str) -> Message {
    parse_with(line, &Some)
}
/// [`parse`], with `local` turning a time without an offset (this PC's wall clock,
/// read as if UTC) into Unix seconds; `None` when it can't.
pub fn parse_with(line: &str, local: &dyn Fn(i64) -> Option<i64>) -> Message {
    let Ok(message) = serde_json::from_str::<Value>(line.trim_start_matches('\u{feff}').trim())
    else {
        return Message::Other;
    };
    match message.get("type").and_then(Value::as_str).unwrap_or("") {
        "snapshotStart" => Message::SnapshotStart,
        "snapshotComplete" => Message::SnapshotComplete,
        "plugins" => Message::Plugins(pairs(message.get("payload"))),
        "categories" => Message::Categories(pairs(message.get("payload"))),
        "games" => Message::Games(
            message
                .get("payload")
                .and_then(Value::as_array)
                .map(|games| games.iter().filter_map(|g| game(g, local)).collect())
                .unwrap_or_default(),
        ),
        "status" => {
            let status = message.get("status").cloned().unwrap_or_default();
            Message::Status {
                name: text(&status, "name"),
                id: text(&status, "id"),
                install_dir: text(&status, "installDir"),
                exe: text(&status, "exe"),
            }
        }
        "commandResult" => Message::CommandResult {
            command: text(&message, "command"),
            request_id: text(&message, "requestId"),
            success: message.get("success").and_then(Value::as_bool) == Some(true),
            error: text(&message, "error"),
        },
        _ => Message::Other,
    }
}
fn game(value: &Value, local: &dyn Fn(i64) -> Option<i64>) -> Option<Game> {
    let id = text(value, "id");
    if uuid::Uuid::parse_str(&id).is_err() {
        return None;
    }
    let flag = |key: &str| value.get(key).map(|v| v.as_bool().unwrap_or(false));
    // Without either flag the game counts as installed, as in Vibepollo.
    let installed = match (flag("installed"), flag("isInstalled")) {
        (None, None) => true,
        (a, b) => a.unwrap_or(false) || b.unwrap_or(false),
    };
    // The plugin sends "" for never played.
    let played = text(value, "lastPlayed");
    let last_played = parse_time_with(&played, local);
    Some(Game {
        name: text(value, "name"),
        exe: text(value, "exe"),
        args: text(value, "args"),
        working_dir: text(value, "workingDir"),
        install_dir: text(value, "installDir"),
        categories: value
            .get("categories")
            .and_then(Value::as_array)
            .map(|c| {
                c.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        plugin_id: text(value, "pluginId"),
        plugin_name: text(value, "pluginName"),
        playtime_minutes: text(value, "playtimeMinutes").trim().parse().ok(),
        last_played,
        last_played_unknown: !played.trim().is_empty() && last_played.is_none(),
        box_art_path: text(value, "boxArtPath"),
        icon_path: text(value, "iconPath"),
        installed,
        id,
    })
}
/// An ISO 8601 time (`2026-08-29T17:49:56.1234567+02:00`, `+0200`, `+02`, `...Z`)
/// as Unix seconds; one without an offset is read as UTC.
pub fn parse_time(value: &str) -> Option<i64> {
    parse_time_with(value, &Some)
}
/// [`parse_time`], with `local` for a time without an offset: .NET's round-trip
/// format, which the plugin sends, leaves the offset out of a wall-clock time.
pub fn parse_time_with(value: &str, local: &dyn Fn(i64) -> Option<i64>) -> Option<i64> {
    let value = value.trim();
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        let part = value.get(range)?;
        part.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| part.parse().ok())?
    };
    let separators = value.as_bytes();
    if separators.len() < 19
        || separators[4] != b'-'
        || separators[7] != b'-'
        || !matches!(separators[10], b'T' | b't' | b' ')
        || separators[13] != b':'
        || separators[16] != b':'
    {
        return None;
    }
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut rest = &value[19..];
    if let Some(fraction) = rest.strip_prefix('.') {
        rest = fraction.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    let offset = match rest.as_bytes().first() {
        None => None,
        Some(b'Z' | b'z') if rest.len() == 1 => Some(0),
        Some(sign @ (b'+' | b'-')) => {
            let zone = &rest[1..];
            // Only digits and a colon are a zone, and `split_at` needs a character boundary.
            if !zone.is_ascii() {
                return None;
            }
            // `+02:00`, `+0200` or `+02`.
            let (h, m) = match zone.split_once(':') {
                Some(parts) => parts,
                None if zone.len() == 4 => zone.split_at(2),
                None if zone.len() == 2 => (zone, "00"),
                None => return None,
            };
            if h.len() != 2
                || m.len() != 2
                || !(h.bytes().chain(m.bytes())).all(|b| b.is_ascii_digit())
            {
                return None;
            }
            let minutes = h.parse::<i64>().ok()? * 60 + m.parse::<i64>().ok()?;
            Some(if *sign == b'+' { minutes } else { -minutes })
        }
        _ => return None,
    };
    // Days from the civil date (Howard Hinnant's algorithm).
    let (y, m) = if month <= 2 {
        (year - 1, month + 9)
    } else {
        (year, month - 3)
    };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let wall = days * 86400 + hour * 3600 + minute * 60 + second;
    match offset {
        Some(offset) => Some(wall - offset * 60),
        None => local(wall),
    }
}
/// Unix seconds as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn format_time(seconds: i64) -> String {
    let days = seconds.div_euclid(86400);
    let rest = seconds.rem_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest / 60 % 60,
        rest % 60
    )
}

/// `Version: 0.4.14` from an `extension.yaml`.
pub fn plugin_version(manifest: &str) -> Option<String> {
    manifest.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case("version")
            .then(|| {
                value
                    .trim()
                    .trim_matches(['"', '\''])
                    .trim_start_matches(['v', 'V'])
                    .to_owned()
            })
            .filter(|v| !v.is_empty())
    })
}
