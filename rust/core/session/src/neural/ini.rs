//! OptiScaler.ini parsing, add-on frame-generation edits, exact restoration
//! records, and `OptiDllPath` resolution for Neural Rendering.
use super::manifest::{PROVIDER, SETTINGS, entry_path};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The section and values the add-on sets so OptiScaler hands the game's
/// DLSS frame generation to the provider: from the release's own add-on
/// installer. The provider generates from the game's HUD-less image, which
/// `DisableHudless` would take away.
pub const INI_SECTION: &str = "FrameGen";
pub const INI_VALUES: [(&str, &str); 3] = [
    ("FGInput", "nvngxfg"),
    ("FGNvngxReplacement", PROVIDER),
    ("DisableHudless", "false"),
];

/// What the add-on changed in the user's OptiScaler.ini, to put back exactly.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IniRecord {
    /// The ini's path in the game folder (with `/`).
    pub path: String,
    /// Each changed key's line as it was, without its line ending; `None`
    /// for a key the ini did not have.
    pub before: BTreeMap<String, Option<String>>,
    /// The ini had no `[FrameGen]` section; the add-on added it.
    #[serde(default)]
    pub section_added: bool,
    /// A line break was added after the ini's last line: before the added
    /// section, or after a `[FrameGen]` header that ended the file.
    #[serde(default)]
    pub break_added: bool,
}

/// An ini file as lines, each with whether it ended in `\r`, and how it was
/// written.
pub(super) struct Ini {
    bom: bool,
    crlf: bool,
    /// Split on `\n`: the last is what follows the last line break.
    lines: Vec<String>,
}
impl Ini {
    pub(super) fn read(bytes: &[u8]) -> Result<Self, String> {
        let (bom, body) = match bytes.strip_prefix(b"\xef\xbb\xbf") {
            Some(body) => (true, body),
            None => (false, bytes),
        };
        let text = std::str::from_utf8(body).map_err(|_| {
            format!("{SETTINGS} is not UTF-8 text, which Rubylight can change and put back")
        })?;
        Ok(Self {
            bom,
            crlf: text.contains("\r\n"),
            lines: text.split('\n').map(str::to_owned).collect(),
        })
    }
    fn write(&self) -> Vec<u8> {
        let mut bytes = if self.bom {
            b"\xef\xbb\xbf".to_vec()
        } else {
            Vec::new()
        };
        bytes.extend_from_slice(self.lines.join("\n").as_bytes());
        bytes
    }
    fn ending(&self) -> &'static str {
        if self.crlf { "\r" } else { "" }
    }
    /// The line's text without its `\r`.
    fn text(line: &str) -> &str {
        line.strip_suffix('\r').unwrap_or(line)
    }
    fn section_name(line: &str) -> Option<&str> {
        let line = Self::text(line).trim();
        line.strip_prefix('[')?.strip_suffix(']').map(str::trim)
    }
    fn key_of(line: &str) -> Option<&str> {
        let line = Self::text(line).trim_start();
        if line.starts_with(';') || line.starts_with('#') {
            return None;
        }
        line.split_once('=').map(|(key, _)| key.trim())
    }
    /// The section's header line and the end of its lines (the next
    /// header, or the end); an error when it is there twice.
    fn section(&self, name: &str) -> Result<Option<(usize, usize)>, String> {
        let headers: Vec<usize> = (0..self.lines.len())
            .filter(|&i| Self::section_name(&self.lines[i]).is_some())
            .collect();
        let mut found = headers.iter().filter(|&&i| {
            Self::section_name(&self.lines[i]).is_some_and(|n| n.eq_ignore_ascii_case(name))
        });
        let Some(&header) = found.next() else {
            return Ok(None);
        };
        if found.next().is_some() {
            return Err(format!("{SETTINGS} has the section [{name}] twice"));
        }
        let end = headers
            .iter()
            .copied()
            .find(|&i| i > header)
            .unwrap_or(self.lines.len());
        Ok(Some((header, end)))
    }
    /// The line of `key` in the section, an error when it is there twice.
    fn line_of(&self, (header, end): (usize, usize), key: &str) -> Result<Option<usize>, String> {
        let mut found = (header + 1..end)
            .filter(|&i| Self::key_of(&self.lines[i]).is_some_and(|k| k.eq_ignore_ascii_case(key)));
        let first = found.next();
        if found.next().is_some() {
            return Err(format!("{SETTINGS} has {key} twice in [{INI_SECTION}]"));
        }
        Ok(first)
    }
    pub(super) fn get(&self, section: &str, key: &str) -> Result<Option<String>, String> {
        let Some(span) = self.section(section)? else {
            return Ok(None);
        };
        Ok(self.line_of(span, key)?.map(|i| {
            Self::text(&self.lines[i])
                .split_once('=')
                .map_or("", |(_, value)| value)
                .trim()
                .to_owned()
        }))
    }
}

/// The user's OptiScaler.ini (`bytes`, at `path` in the game folder) with
/// [`INI_VALUES`] set in `[FrameGen]`, and the record that puts it back.
/// Its byte order mark, line endings and every other line stay as they were.
/// `record` is an earlier install's: its lines as they were are kept, never
/// taken again from an ini the add-on already changed.
pub fn ini_apply(
    bytes: &[u8],
    path: &str,
    record: Option<&IniRecord>,
) -> Result<(Vec<u8>, IniRecord), String> {
    let mut ini = Ini::read(bytes)?;
    let ending = ini.ending();
    let mut new = IniRecord {
        path: path.into(),
        before: BTreeMap::new(),
        section_added: false,
        break_added: false,
    };
    let span = match ini.section(INI_SECTION)? {
        // A header that ends the file with no line break after it gets the
        // file's own, as an added section does, before lines go under it.
        Some((header, _)) if header == ini.lines.len() - 1 => {
            new.break_added = true;
            ini.lines[header].push_str(ending);
            ini.lines.push(String::new());
            (header, header + 2)
        }
        Some(span) => span,
        None => {
            new.section_added = true;
            // After the last line, which a line break must end.
            let last = ini.lines.len() - 1;
            if !ini.lines[last].is_empty() {
                new.break_added = true;
                ini.lines[last].push_str(ending);
                ini.lines.push(String::new());
            }
            let at = ini.lines.len() - 1;
            ini.lines.insert(at, format!("[{INI_SECTION}]{ending}"));
            (at, at + 1)
        }
    };
    let (header, _) = span;
    let mut inserted = 0;
    for (key, value) in INI_VALUES {
        let span = ini.section(INI_SECTION)?.unwrap_or(span);
        match ini.line_of(span, key)? {
            Some(at) => {
                new.before
                    .insert(key.into(), Some(Ini::text(&ini.lines[at]).to_owned()));
                let cr = if ini.lines[at].ends_with('\r') {
                    "\r"
                } else {
                    ""
                };
                ini.lines[at] = format!("{key}={value}{cr}");
            }
            None => {
                new.before.insert(key.into(), None);
                inserted += 1;
                ini.lines
                    .insert(header + inserted, format!("{key}={value}{ending}"));
            }
        }
    }
    let record = match record {
        Some(old) => old.clone(),
        None => new,
    };
    Ok((ini.write(), record))
}

/// The user's OptiScaler.ini with the lines [`ini_apply`] changed put back
/// as `record` has them: a key it did not have is removed again, and a
/// section it added goes once it holds nothing else.
pub fn ini_restore(bytes: &[u8], record: &IniRecord) -> Result<Vec<u8>, String> {
    let mut ini = Ini::read(bytes)?;
    if let Some(span) = ini.section(INI_SECTION)? {
        let mut remove = Vec::new();
        for (key, before) in &record.before {
            let Some(at) = ini.line_of(span, key)? else {
                continue;
            };
            match before {
                Some(line) => {
                    let cr = if ini.lines[at].ends_with('\r') {
                        "\r"
                    } else {
                        ""
                    };
                    ini.lines[at] = format!("{line}{cr}");
                }
                None => remove.push(at),
            }
        }
        remove.sort_unstable();
        for at in remove.into_iter().rev() {
            ini.lines.remove(at);
        }
        // The line break added after a header that ended the file goes once
        // nothing is under it again.
        if record.break_added
            && !record.section_added
            && let Some((header, end)) = ini.section(INI_SECTION)?
            && end == ini.lines.len()
            && header + 2 == end
            && ini.lines[header + 1].is_empty()
        {
            ini.lines.pop();
            if let Some(line) = ini.lines.last_mut()
                && line.ends_with('\r')
            {
                line.pop();
            }
        }
        // A section the add-on added goes once nothing else is in it: at the
        // end, only what follows the last line break may follow it.
        if record.section_added
            && let Some((header, end)) = ini.section(INI_SECTION)?
        {
            let last = ini.lines.len() - 1;
            let empty = (header + 1..end).all(|i| i == last && ini.lines[i].is_empty());
            if empty {
                ini.lines.remove(header);
                if record.break_added && end == last + 1 && ini.lines.len() >= 2 {
                    // The line break added after the ini's last line goes too.
                    ini.lines.pop();
                    if let Some(line) = ini.lines.last_mut()
                        && line.ends_with('\r')
                    {
                        line.pop();
                    }
                }
            }
        }
    }
    Ok(ini.write())
}

/// OptiScaler's folder in the game folder (with `/`, empty for the game
/// folder itself), from OptiScaler.ini's `[Libraries] OptiDllPath`: `auto`
/// (or none) is the `OptiScaler` folder when there is one
/// (`optiscaler_folder`), else the game folder. A path that leaves the game
/// folder is refused: the add-on writes only inside it.
pub fn opti_dir(ini: &[u8], optiscaler_folder: bool) -> Result<String, String> {
    let value = Ini::read(ini)?.get("Libraries", "OptiDllPath")?;
    let value = value
        .as_deref()
        .map(|v| v.trim().trim_matches('"').trim())
        .unwrap_or("");
    if value.is_empty() || value.eq_ignore_ascii_case("auto") {
        return Ok(if optiscaler_folder {
            "OptiScaler".into()
        } else {
            String::new()
        });
    }
    let mut path = value.replace('\\', "/");
    while let Some(rest) = path.strip_prefix("./") {
        path = rest.to_owned();
    }
    let path = path.trim_end_matches('/').to_owned();
    if path.is_empty() || path == "." {
        return Ok(String::new());
    }
    let outside = || {
        format!(
            "OptiScaler.ini's OptiDllPath ({value}) points outside the game's folder, where Rubylight changes nothing"
        )
    };
    if path.starts_with('/') || path.contains(':') {
        return Err(outside());
    }
    entry_path(&path).filter(|p| *p == path).ok_or_else(outside)
}
