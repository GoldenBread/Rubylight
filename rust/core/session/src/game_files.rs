//! Managed game files (issue #63): keys in a game's own `.ini`, such as the frame limit of an
//! upscaler or frame generation mod (OptiScaler, AMD-NR), that the host sets for a stream and
//! puts back when the session ends.
//!
//! An app lists them in `game-files`, and a device's block in `device-overrides` can list its
//! own: a file, a section, a key and a value. The value is fixed text or a number, or follows
//! the stream's frame rate: `{stream_fps}` with an optional whole offset (`{stream_fps-1}`).
//!
//! Only the declared keys change; the rest of the file stays byte for byte, with its line
//! endings, byte order mark, comments and key order. A key is found as INI readers find it:
//! `[Section]` headers and `key=value` lines, names compared without regard to ASCII case,
//! blanks around names and values ignored, the first match counting. A missing file, section
//! or key is skipped with a warning and never created; the game still starts.
//!
//! The host runs as SYSTEM, so it takes only an existing `.ini` file of at most
//! [`MAX_FILE_BYTES`] inside one of the app's folders (its working directory, the folder of its
//! program, its Steam install folder) once every link and junction is resolved. Network and
//! device paths are refused, and so are a drive's root and the Windows folder as an app's
//! folder. A file is written whole into a temporary file beside it and swapped in, so a reader
//! sees the old file or the new one, and the swap keeps the file's permissions and attributes.
//! While a file is read or written, the folders from the app's down to it are held open, so
//! none can be swapped for a link meanwhile.
//!
//! Restore is per key, not a backup of the file: before a write, the journal records each key's
//! original value and the value written. A restore puts the original back only while the key
//! still holds the written value; a value changed since (in the game's own overlay) is the
//! user's and stays. The host ties the restore to the session (`rust/host/src/game_files.rs`).
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
};

mod ini;
mod io;
mod journal;

pub use ini::{value_of, with_value};
use io::local;
pub use io::{Access, as_host};
#[cfg(all(test, windows))]
use io::{linked, pin};
#[cfg(test)]
use io::{move_in, recover};
pub(crate) use io::{reach, replace};
pub use journal::{Change, Journal, Scope, apply, restore};

/// The app key, and the key in a device's block, that lists the entries.
pub const SETTING: &str = "game-files";
/// The most entries one list holds.
pub const MAX_ENTRIES: usize = 32;
/// The largest file the host edits.
pub const MAX_FILE_BYTES: u64 = 1 << 20;
/// The journal's file name inside the profile (config) directory.
pub const JOURNAL_FILE: &str = "game-files-journal.json";
/// The longest section, key or value.
const MAX_TEXT: usize = 256;
/// The longest path as written.
const MAX_PATH: usize = 1024;
/// The largest offset from the stream's frame rate.
const MAX_OFFSET: i64 = 1000;
const VALUE_FORMS: &str = "a value is fixed text, a number or {stream_fps} with an optional whole offset such as {stream_fps-1}";

/// One key to manage, as an app or a device's block lists it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub path: String,
    #[serde(default)]
    pub section: String,
    pub key: String,
    pub value: Value,
}

/// What an entry's value stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Template {
    Fixed(String),
    /// The stream's frame rate plus this offset.
    StreamFps(i64),
}

impl Template {
    pub fn parse(value: &Value) -> Result<Self> {
        let text = match value {
            Value::String(text) => text.trim().to_owned(),
            Value::Number(number) => number.to_string(),
            Value::Bool(on) => on.to_string(),
            _ => bail!("{VALUE_FORMS}"),
        };
        if let Some(inner) = text.strip_prefix('{') {
            let Some(inner) = inner.strip_suffix('}') else {
                bail!("{VALUE_FORMS}");
            };
            return placeholder(inner);
        }
        if text.contains(['{', '}']) {
            bail!("{VALUE_FORMS}");
        }
        check_text(&text, "value")?;
        Ok(Self::Fixed(text))
    }

    /// The text to write for a stream at `fps`.
    pub fn resolve(&self, fps: Option<u32>) -> Result<String> {
        match self {
            Self::Fixed(text) => Ok(text.clone()),
            Self::StreamFps(offset) => {
                let fps = fps.context("the stream did not state its frame rate")?;
                let value = i64::from(fps) + offset;
                if value < 1 {
                    bail!("the stream's frame rate {fps} with {offset:+} is below 1");
                }
                Ok(value.to_string())
            }
        }
    }
}

/// `stream_fps`, `stream_fps+N` or `stream_fps-N`; blanks do not matter.
fn placeholder(inner: &str) -> Result<Template> {
    let compact: String = inner.chars().filter(|c| !c.is_whitespace()).collect();
    let Some(rest) = compact.strip_prefix("stream_fps") else {
        bail!("{VALUE_FORMS}");
    };
    if rest.is_empty() {
        return Ok(Template::StreamFps(0));
    }
    let (sign, digits) = if let Some(digits) = rest.strip_prefix('+') {
        (1, digits)
    } else if let Some(digits) = rest.strip_prefix('-') {
        (-1, digits)
    } else {
        bail!("{VALUE_FORMS}");
    };
    if digits.is_empty() || digits.len() > 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        bail!("{VALUE_FORMS}");
    }
    let offset: i64 = digits.parse()?;
    if offset > MAX_OFFSET {
        bail!("the offset from the stream's frame rate is at most {MAX_OFFSET}");
    }
    Ok(Template::StreamFps(sign * offset))
}

/// One line of text of at most [`MAX_TEXT`] characters.
fn check_text(text: &str, what: &str) -> Result<()> {
    if text.chars().count() > MAX_TEXT {
        bail!("the {what} is longer than {MAX_TEXT} characters");
    }
    if text.chars().any(char::is_control) {
        bail!("the {what} must be one line of text");
    }
    Ok(())
}

/// The entries of `list`, an app's or a device block's `game-files`, checked. A null is none.
pub fn parse(list: &Value) -> Result<Vec<Entry>> {
    let items = match list {
        Value::Null => return Ok(Vec::new()),
        Value::Array(items) => items,
        _ => bail!("{SETTING} must be a list of files, sections, keys and values"),
    };
    if items.len() > MAX_ENTRIES {
        bail!("{SETTING} lists at most {MAX_ENTRIES} keys");
    }
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let entry: Entry = serde_json::from_value(item.clone())
                .map_err(|_| anyhow!("it needs a path, a key and a value"))
                .and_then(|entry| check_entry(&entry).map(|()| entry))
                .with_context(|| format!("{SETTING} entry {}", index + 1))?;
            Ok(entry)
        })
        .collect()
}

/// The entries an app record (`extra`) lists, checked; none when it lists none.
pub fn entries(extra: &BTreeMap<String, Value>) -> Result<Vec<Entry>> {
    extra.get(SETTING).map_or(Ok(Vec::new()), parse)
}

fn check_entry(entry: &Entry) -> Result<()> {
    check_path(&entry.path)?;
    let section = entry.section.trim();
    check_text(section, "section")?;
    if section.contains(']') {
        bail!("the section is the name between [ and ], without them");
    }
    let key = entry.key.trim();
    if key.is_empty() {
        bail!("the key is empty");
    }
    check_text(key, "key")?;
    if key.contains('=') || key.starts_with(['[', ';', '#']) {
        bail!("the key is the name before the =");
    }
    Template::parse(&entry.value)?;
    Ok(())
}

/// Whether two entries (as JSON) name the same file, section and key. The host compares
/// names without regard to ASCII case and paths with either slash.
pub fn same_key(a: &Value, b: &Value) -> bool {
    let field = |entry: &Value, name: &str| {
        entry
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .replace('/', "\\")
            .to_ascii_lowercase()
    };
    ["path", "section", "key"]
        .iter()
        .all(|name| field(a, name) == field(b, name))
}

/// Check a path as written, before any file is looked at: one line, an `.ini` file, no `..`,
/// no network or device path, no stream name, and either a drive letter or a name inside the
/// app's folder.
pub fn check_path(path: &str) -> Result<()> {
    let path = path.trim();
    if path.is_empty() {
        bail!("no file is named");
    }
    if path.chars().count() > MAX_PATH || path.chars().any(char::is_control) {
        bail!("the file must be one line of at most {MAX_PATH} characters");
    }
    let bytes = path.as_bytes();
    if matches!(bytes.first(), Some(b'\\' | b'/')) {
        bail!(
            "{path}: network and device paths are not taken; start with a drive letter or name a file in the app's folder"
        );
    }
    let drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    if path[if drive { 2 } else { 0 }..].contains(':') {
        bail!("{path}: a colon belongs only after the drive letter");
    }
    if path.split(['\\', '/']).any(|part| part.trim() == "..") {
        bail!("{path}: .. is not taken; name the file inside the app's folder");
    }
    if !Path::new(path)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("ini"))
    {
        bail!("{path}: only .ini files are managed");
    }
    Ok(())
}

/// Why a file is not taken.
#[derive(Debug)]
pub enum Unusable {
    /// It, or a folder to find it in, does not exist. Saving an app allows this: the game may
    /// not be installed yet.
    Missing(String),
    Refused(String),
}
impl fmt::Display for Unusable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(why) | Self::Refused(why) => f.write_str(why),
        }
    }
}
impl std::error::Error for Unusable {}

/// The folders, of `candidates`, that an app's files may be in: those that exist, resolved
/// through `access`, on a local drive, and neither a drive's root nor inside the Windows
/// folder.
pub fn folders(candidates: impl IntoIterator<Item = PathBuf>, access: Access) -> Vec<PathBuf> {
    let windows = std::env::var_os("SystemRoot").and_then(|root| std::fs::canonicalize(root).ok());
    let mut found: Vec<PathBuf> = Vec::new();
    for candidate in candidates {
        if candidate.as_os_str().is_empty() || !candidate.is_absolute() {
            continue;
        }
        let resolved = reach(access, || {
            std::fs::canonicalize(&candidate)
                .ok()
                .map(|folder| (folder.is_dir(), folder))
        });
        let (is_dir, folder) = match resolved {
            Ok(Some(found)) => found,
            Ok(None) => continue,
            Err(error) => {
                tracing::warn!(folder = %candidate.display(), %error, "this folder of the app cannot be read as the Windows user; managed game files are not taken from it");
                continue;
            }
        };
        let refused = !is_dir
            || !local(&folder)
            || folder.parent().is_none()
            || windows
                .as_ref()
                .is_some_and(|root| folder.starts_with(root));
        if refused {
            tracing::warn!(folder = %candidate.display(), "this folder of the app is a drive's root, the Windows folder or not a local folder; managed game files are not taken from it");
        } else if !found.contains(&folder) {
            found.push(folder);
        }
    }
    found
}

/// The file `path` names: resolved through `access`, an existing `.ini` file of at most
/// [`MAX_FILE_BYTES`] inside one of `folders` (see [`folders`]). A relative path starts at
/// `base`, the app's working directory. Without the access (nobody signed in), it is missing.
pub fn resolve(
    path: &str,
    base: Option<&Path>,
    folders: &[PathBuf],
    access: Access,
) -> Result<PathBuf, Unusable> {
    resolve_in(path, base, folders, access).map(|(file, _)| file)
}

/// [`resolve`], with the outermost of `folders` the file is in.
fn resolve_in(
    path: &str,
    base: Option<&Path>,
    folders: &[PathBuf],
    access: Access,
) -> Result<(PathBuf, PathBuf), Unusable> {
    check_path(path).map_err(|error| Unusable::Refused(format!("{error:#}")))?;
    let path = path.trim();
    let written = Path::new(path);
    let candidate = if written.is_absolute() {
        written.to_path_buf()
    } else {
        let Some(base) = base.filter(|base| !base.as_os_str().is_empty()) else {
            return Err(Unusable::Missing(format!(
                "{path}: the app has no folder to find it in"
            )));
        };
        base.join(written)
    };
    reach(access, || resolve_file(path, &candidate, folders)).unwrap_or_else(|error| {
        Err(Unusable::Missing(format!(
            "{path}: the host cannot act as the Windows user: {error}"
        )))
    })
}

fn resolve_file(
    path: &str,
    candidate: &Path,
    folders: &[PathBuf],
) -> Result<(PathBuf, PathBuf), Unusable> {
    let file = match std::fs::canonicalize(candidate) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(Unusable::Missing(format!("{path}: no such file")));
        }
        Err(error) => return Err(Unusable::Refused(format!("{path}: {error}"))),
    };
    let folder = folders
        .iter()
        .filter(|folder| file.starts_with(folder))
        .min_by_key(|folder| folder.components().count());
    let Some(folder) = folder.filter(|_| local(&file)) else {
        return Err(Unusable::Refused(format!(
            "{path} is outside the app's folders (its working directory, the folder of its program and its Steam install folder)"
        )));
    };
    if !file
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("ini"))
    {
        return Err(Unusable::Refused(format!(
            "{path} leads to a file that is not an .ini file"
        )));
    }
    let metadata =
        std::fs::metadata(&file).map_err(|error| Unusable::Refused(format!("{path}: {error}")))?;
    if !metadata.is_file() {
        return Err(Unusable::Refused(format!("{path} is not a file")));
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err(Unusable::Refused(format!(
            "{path} is larger than 1 MiB, more than a settings file"
        )));
    }
    Ok((file, folder.clone()))
}

/// Whether `key` names a frame limit: `fps`, `framerate` or `limit` is part of it.
pub fn limits_frames(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    ["fps", "framerate", "limit"]
        .iter()
        .any(|word| key.contains(word))
}

/// A key to set for a stream: its file resolved and its value worked out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub file: PathBuf,
    /// The app's folder the file is in, resolved: the folders from it down to the file are
    /// held while the file is read and written (see [`pin`]).
    pub folder: PathBuf,
    pub section: String,
    pub key: String,
    pub value: String,
}
impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} [{}] {}",
            self.file.display(),
            self.section.trim(),
            self.key.trim()
        )
    }
}

/// The targets that set a frame limit other than `limiter`, the rate the host's frame limiter
/// holds the game to. 0 (no limit in most tools) is no other limit.
pub fn limit_conflicts(targets: &[Target], limiter: u32) -> impl Iterator<Item = &Target> {
    targets.iter().filter(move |target| {
        limits_frames(&target.key)
            && target
                .value
                .trim()
                .parse::<f64>()
                .is_ok_and(|value| value > 0.0 && (value - f64::from(limiter)).abs() >= 0.5)
    })
}

/// What a write or a restore did. Each line names the file, section and key.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub written: Vec<String>,
    pub restored: Vec<String>,
    /// Restores left alone: the key holds another value than the host wrote, the user's.
    pub kept: Vec<String>,
    /// Keys not written or not put back, with why; nothing of theirs changed.
    pub skipped: Vec<String>,
    /// Keys that stay journaled because their file could not be written, with why.
    pub failed: Vec<String>,
}

/// The keys `entries` set for a stream at `fps`, with each file resolved (see [`resolve`]).
/// An entry that cannot be used is skipped with a warning; the rest go on.
pub fn targets(
    entries: &[Entry],
    fps: Option<u32>,
    base: Option<&Path>,
    folders: &[PathBuf],
    access: Access,
    report: &mut Report,
) -> Vec<Target> {
    let mut found: Vec<Target> = Vec::new();
    for entry in entries {
        let named = format!(
            "{} [{}] {}",
            entry.path.trim(),
            entry.section.trim(),
            entry.key.trim()
        );
        let target = check_entry(entry)
            .and_then(|()| Template::parse(&entry.value)?.resolve(fps))
            .and_then(|value| {
                let (file, folder) = resolve_in(&entry.path, base, folders, access)?;
                Ok(Target {
                    file,
                    folder,
                    section: entry.section.trim().to_owned(),
                    key: entry.key.trim().to_owned(),
                    value,
                })
            });
        match target {
            Ok(target) => {
                found.retain(|other| {
                    !(other.file == target.file && same(other, &target.section, &target.key))
                });
                found.push(target);
            }
            Err(error) => skip(report, &named, &format!("{error:#}")),
        }
    }
    found
}

fn same(target: &Target, section: &str, key: &str) -> bool {
    target.section.eq_ignore_ascii_case(section.trim())
        && target.key.eq_ignore_ascii_case(key.trim())
}

pub(super) fn skip(report: &mut Report, what: &str, why: &str) {
    tracing::warn!(
        key = what,
        reason = why,
        "a managed game file key is skipped; the game starts without it"
    );
    report.skipped.push(format!("{what}: {why}"));
}

#[cfg(test)]
mod tests;
