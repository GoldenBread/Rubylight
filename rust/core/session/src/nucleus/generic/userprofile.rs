//! In-place editing of Nucleus Co-op's userprofile.json game list.

/// `json` (Nucleus' `content\userprofile.json`, None when there is none yet)
/// with the game added as Nucleus' "add game" does (`UserGameInfo
/// .InitializeDefault`). Every other byte stays: the user's games, their
/// order and the file's layout. None when the game is already there.
pub fn add_game(json: Option<&str>, guid: &str, exe_path: &str) -> Result<Option<String>, String> {
    let exe_path = exe_path.replace('I', "i");
    let text = json.unwrap_or_default();
    let body = text.trim_start_matches('\u{feff}');
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let entry = |indent: &str| {
        format!(
            "{indent}{{{newline}{indent}  \"GameGuid\": {},{newline}{indent}  \"ExePath\": {}{newline}{indent}}}",
            serde_json::Value::from(guid),
            serde_json::Value::from(exe_path.as_str()),
        )
    };
    if body.trim().is_empty() {
        return Ok(Some(format!(
            "{{{newline}  \"Games\": [{newline}{}{newline}  ]{newline}}}",
            entry("    ")
        )));
    }
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| format!("Nucleus Co-op's userprofile.json is not readable: {e}"))?;
    let object = value
        .as_object()
        .ok_or("Nucleus Co-op's userprofile.json is not an object")?;
    match object.get("Games") {
        Some(serde_json::Value::Array(games)) => {
            if games
                .iter()
                .any(|game| game.get("GameGuid").and_then(|g| g.as_str()) == Some(guid))
            {
                return Ok(None);
            }
            let (open, close) = games_array(text)
                .ok_or("Nucleus Co-op's userprofile.json has an unreadable game list")?;
            let inside = &text[open + 1..close];
            Ok(Some(if inside.trim().is_empty() {
                format!(
                    "{}[{newline}{}{newline}  ]{}",
                    &text[..open],
                    entry("    "),
                    &text[close + 1..]
                )
            } else {
                let last = open + 1 + inside.trim_end().len();
                format!(
                    "{},{newline}{}{}",
                    &text[..last],
                    entry("    "),
                    &text[last..]
                )
            }))
        }
        // Nucleus leaves out an empty list.
        None | Some(serde_json::Value::Null) => {
            let mut object = object.clone();
            object.insert(
                "Games".into(),
                serde_json::json!([{ "GameGuid": guid, "ExePath": exe_path }]),
            );
            let bom = if text.starts_with('\u{feff}') {
                "\u{feff}"
            } else {
                ""
            };
            let pretty = serde_json::to_string_pretty(&object).map_err(|e| e.to_string())?;
            Ok(Some(format!("{bom}{}", pretty.replace('\n', newline))))
        }
        Some(_) => Err("Nucleus Co-op's userprofile.json has no game list".into()),
    }
}

/// `json` (Nucleus' `userprofile.json`) without the games of `guid`, which
/// only Rubylight's own handler uses. Every other byte stays. None when
/// there is none.
pub fn remove_game(json: &str, guid: &str) -> Result<Option<String>, String> {
    let Some((open, close)) = games_array(json) else {
        return Ok(None);
    };
    // Each game: the byte offsets of its `{` and `}`.
    let bytes = json.as_bytes();
    let mut games = Vec::new();
    let (mut depth, mut start, mut at) = (0, 0, open + 1);
    while at < close {
        match bytes[at] {
            b'"' => {
                at += 1;
                while at < close && bytes[at] != b'"' {
                    at += if bytes[at] == b'\\' { 2 } else { 1 };
                }
            }
            b'{' | b'[' => {
                if depth == 0 {
                    start = at;
                }
                depth += 1;
            }
            b'}' | b']' => {
                depth -= 1;
                if depth == 0 {
                    games.push((start, at + 1));
                }
            }
            _ => {}
        }
        at += 1;
    }
    let ours = |&(start, end): &(usize, usize)| {
        serde_json::from_str::<serde_json::Value>(&json[start..end])
            .map_err(|e| format!("Nucleus Co-op's userprofile.json is not readable: {e}"))
            .map(|game| game.get("GameGuid").and_then(|g| g.as_str()) == Some(guid))
    };
    let mut kept = Vec::new();
    for game in &games {
        if !ours(game)? {
            kept.push(*game);
        }
    }
    if kept.len() == games.len() {
        return Ok(None);
    }
    let inside = match (games.first(), games.last(), kept.is_empty()) {
        (Some(&(first, _)), Some(&(_, last)), false) => {
            let separator = match games.get(..2) {
                Some([(_, end), (next, _)]) => &json[*end..*next],
                _ => ",",
            };
            let kept: Vec<&str> = kept.iter().map(|&(s, e)| &json[s..e]).collect();
            format!(
                "{}{}{}",
                &json[open + 1..first],
                kept.join(separator),
                &json[last..close]
            )
        }
        _ => String::new(),
    };
    Ok(Some(format!(
        "{}{inside}{}",
        &json[..=open],
        &json[close..]
    )))
}

/// The byte offsets of the `[` and `]` of a JSON object's top-level
/// `"Games"` array.
fn games_array(text: &str) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut depth = 0;
    let mut games_key = false;
    let mut open = None;
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'"' => {
                let start = at + 1;
                at += 1;
                while at < bytes.len() && bytes[at] != b'"' {
                    at += if bytes[at] == b'\\' { 2 } else { 1 };
                }
                if depth == 1 && text.get(start..at) == Some("Games") {
                    games_key = true;
                }
            }
            b',' if depth == 1 => games_key = false,
            b'[' | b'{' => {
                if bytes[at] == b'[' && depth == 1 && games_key && open.is_none() {
                    open = Some(at);
                }
                depth += 1;
            }
            b']' | b'}' => {
                depth -= 1;
                if let (1, b']', Some(open)) = (depth, bytes[at], open) {
                    return Some((open, at));
                }
            }
            _ => {}
        }
        at += 1;
    }
    None
}
