//! Path validation, environment variable expansion, and store keys for per-device game settings.
use super::{MAX_FILES, SETTING};
use crate::state;
use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

const MAX_PATH: usize = 1024;

/// The paths an app lists, checked; none when it lists none.
pub fn paths(extra: &BTreeMap<String, Value>) -> Result<Vec<String>> {
    let list = match extra.get(SETTING) {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(list)) => list,
        Some(_) => bail!("{SETTING} must be a list of paths"),
    };
    let mut paths = Vec::new();
    let mut seen = BTreeSet::new();
    for path in list {
        let path = path
            .as_str()
            .with_context(|| format!("{SETTING} must be a list of paths"))?
            .trim();
        if path.is_empty() {
            continue;
        }
        check_path(path)?;
        if !seen.insert(normal(path)) {
            bail!("{path} is listed twice");
        }
        paths.push(path.to_owned());
    }
    if paths.len() > MAX_FILES {
        bail!("at most {MAX_FILES} game settings files per app");
    }
    Ok(paths)
}

/// Check a path as written: one line, a drive letter or a `%VAR%` first, no network or device
/// path, no `..`, and every `%` part of a `%NAME%`.
pub fn check_path(path: &str) -> Result<()> {
    let path = path.trim();
    if path.is_empty() {
        bail!("no file is named");
    }
    if path.chars().count() > MAX_PATH || path.chars().any(char::is_control) {
        bail!("{path}: a path is one line of at most {MAX_PATH} characters");
    }
    let names = variables(path)?;
    if !path.starts_with('%') {
        check_absolute(path)?;
    } else if names.is_empty() {
        bail!(
            "{path}: start with a drive letter, such as C:\\, or a %NAME% such as %LOCALAPPDATA%"
        );
    }
    let bare = names.iter().fold(path.to_owned(), |rest, name| {
        rest.replace(&format!("%{name}%"), "x")
    });
    if bare.get(2..).is_some_and(|rest| rest.contains(':')) {
        bail!("{path}: a colon belongs only after the drive letter");
    }
    if path.split(['\\', '/']).any(|part| part.trim() == "..") {
        bail!("{path}: .. is not taken; name the file's own folder");
    }
    if path.ends_with(['\\', '/']) {
        bail!("{path} names a folder; name the settings file in it");
    }
    Ok(())
}

/// The `%NAME%` parts of `path`, in order. An unpaired `%` is an error.
fn variables(path: &str) -> Result<Vec<String>> {
    let parts: Vec<&str> = path.split('%').collect();
    if parts.len().is_multiple_of(2) {
        bail!("{path}: a % has no closing %; environment variables are written %NAME%");
    }
    parts
        .iter()
        .skip(1)
        .step_by(2)
        .map(|name| {
            if name.is_empty()
                || !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '(' | ')' | '-'))
            {
                bail!("{path}: %{name}% is not an environment variable name");
            }
            Ok((*name).to_owned())
        })
        .collect()
}

/// A drive letter, a colon and a slash first.
fn check_absolute(path: &str) -> Result<()> {
    let bytes = path.as_bytes();
    if matches!(bytes.first(), Some(b'\\' | b'/')) {
        bail!("{path}: network and device paths are not taken; start with a drive letter");
    }
    let drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    if !drive {
        bail!(
            "{path}: start with a drive letter, such as C:\\, or a %NAME% such as %LOCALAPPDATA%"
        );
    }
    Ok(())
}

/// `path` with its `%NAME%` parts replaced by `lookup` (the Windows user's environment), checked
/// again as a file on a drive. A name that is not set is an error.
pub fn expand(path: &str, lookup: impl Fn(&str) -> Option<String>) -> Result<PathBuf> {
    check_path(path)?;
    let path = path.trim();
    let mut expanded = String::new();
    for (index, part) in path.split('%').enumerate() {
        if index % 2 == 0 {
            expanded.push_str(part);
        } else {
            let value = lookup(part)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| anyhow!("{path}: %{part}% is not set for the Windows user"))?;
            expanded.push_str(value.trim().trim_end_matches(['\\', '/']));
        }
    }
    check_absolute(&expanded).with_context(|| format!("{path} becomes {expanded}"))?;
    if expanded.get(2..).is_some_and(|rest| rest.contains(':'))
        || expanded.split(['\\', '/']).any(|part| part.trim() == "..")
    {
        bail!("{path} becomes {expanded}, which is not a plain file path");
    }
    Ok(PathBuf::from(expanded.replace('/', "\\")))
}

/// A path compared without regard to ASCII case or slashes.
pub(super) fn normal(path: &str) -> String {
    path.trim().replace('/', "\\").to_ascii_lowercase()
}

/// A name for the store: letters, digits, `.`, `-` and `_` only, with a hash of the original when
/// anything had to change.
pub(super) fn store_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    let clean = clean.trim_matches('.').to_owned();
    if !clean.is_empty() && clean == name {
        clean
    } else {
        format!("{clean}-{:08x}", crc32fast::hash(name.as_bytes()))
    }
}

/// The store's name for a file as the app lists it: its file name and a hash of the whole path.
pub fn file_key(path: &str) -> String {
    let normal = normal(path);
    let name = normal.rsplit('\\').next().unwrap_or_default();
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    format!("{clean}-{:08x}", crc32fast::hash(normal.as_bytes()))
}

/// The store's name for an app: its `uuid`, else its id.
pub fn app_key(app: &state::App) -> String {
    app.extra
        .get("uuid")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|uuid| !uuid.is_empty())
        .map_or_else(|| app.id().to_string(), str::to_ascii_lowercase)
}
