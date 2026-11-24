//! Game-folder manifests, file placement targets, update transitions, removal
//! plans, pending removal records, and mod-name redaction for Neural Rendering.
use super::{archive::Switch, ini::IniRecord};
use crate::state::App;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// The record of what the host placed in a game folder.
pub const MANIFEST: &str = "rubylight-neural-anr.json";
/// Rubylight 3.0's record of the AMDNR it placed, which the host still removes.
pub const LEGACY_MANIFEST: &str = "rubylight-neural.json";
/// OptiScaler's settings, which its in-game menu rewrites.
pub const SETTINGS: &str = "OptiScaler.ini";
/// The provider's folder in OptiScaler's `custom` folder, and the name
/// OptiScaler.ini's `FGNvngxReplacement` gives it.
pub const PROVIDER: &str = "Apex Neural Rendering FG";
/// The provider's own settings in its folder, which the user may edit.
pub const PROVIDER_SETTINGS: &str = "dlssg/nr_fg.ini";
/// The provider's DLL in its folder.
pub const PROVIDER_DLL: &str = "dlssg/nvngx_dlssg.dll";
/// OptiScaler in the full archive, placed under one of [`FG_PROXIES`].
pub const PROXY_SOURCE: &str = "game/dxgi.dll";
/// What the manifest's `by` says; removal trusts no other manifest.
pub const PLACED_BY: &str = "Rubylight";
/// The app's setting: `{"exe", "release", "proxy", "mode", "switches"}`
/// once the files are in its folder. Only the host writes it.
pub const KEY: &str = "neural-anr";
/// Rubylight 3.0's setting for the AMDNR it installed (`{"exe", "release",
/// "proxy"}`): kept until the files are removed, never set again.
pub const LEGACY_KEY: &str = "neural-rendering";
/// The user's acceptance of the notice and the pending removals.
pub const SETTINGS_FILE: &str = "neural.json";
/// What a second install for a game that has it is told.
pub const ALREADY_ON: &str = "frame generation is already on for this game; turn it off first";
/// What the console shows for a failed download; the host's log has why.
pub const DOWNLOAD_FAILED: &str = "the download or its check failed; Rubylight's log says why";

/// The names an OptiScaler loads under, in the order its guide tries them:
/// where the host looks for an OptiScaler the user installed, and what an
/// AMDNR manifest of Rubylight 3.0 may name.
pub const PROXIES: [&str; 7] = [
    "dxgi.dll",
    "d3d12.dll",
    "winmm.dll",
    "version.dll",
    "dbghelp.dll",
    "winhttp.dll",
    "wininet.dll",
];
/// The names the full package's OptiScaler goes in under, as its own
/// installer allows them.
pub const FG_PROXIES: [&str; 3] = ["dxgi.dll", "winmm.dll", "version.dll"];

/// The files the provider writes into its own `dlssg` folder once a game
/// runs it, besides its logs ([`provider_files`]): `anr-vkloader.dll`, its
/// copy of the system's Vulkan loader. FG-V.B16's log says so on the
/// first frame: "native bridge: our Vulkan loader
/// ...\dlssg\anr-vkloader.dll, a copy of the system's vulkan-1.dll apart
/// from the game's".
pub const PROVIDER_RUNTIME: [&str; 1] = ["anr-vkloader.dll"];

/// How the files go into a game.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// The full package: OptiScaler under a free proxy name, its settings
    /// and the provider in its `OptiScaler` folder.
    #[default]
    Full,
    /// The add-on: the provider in the `custom` folder of the OptiScaler the
    /// user installed, and three values of its OptiScaler.ini changed.
    Addon,
}

/// Where a cached file goes in the game folder (with `/`), or `None` for one
/// the mode does not place. `proxy` is OptiScaler's name ([`Mode::Full`]),
/// `opti_dir` OptiScaler's folder in the game folder, empty for the game
/// folder itself ([`Mode::Addon`], [`super::opti_dir`]).
pub fn target(mode: Mode, cached: &str, proxy: &str, opti_dir: &str) -> Option<String> {
    match mode {
        Mode::Full => {
            let path = cached.strip_prefix("full/")?;
            if path == PROXY_SOURCE {
                Some(proxy.into())
            } else if let Some(game) = path.strip_prefix("game/") {
                Some(game.into())
            } else {
                // OptiScaler's licence beside it, in the folder the package fills.
                path.strip_prefix("LICENSES/")
                    .map(|licence| format!("OptiScaler/LICENSES/{licence}"))
            }
        }
        Mode::Addon => {
            let path = cached.strip_prefix("addon/")?;
            path.starts_with(&format!("{PROVIDER}/")).then(|| {
                if opti_dir.is_empty() {
                    format!("custom/{path}")
                } else {
                    format!("{opti_dir}/custom/{path}")
                }
            })
        }
    }
}

/// Whether a placed file is a settings file the user changes through
/// OptiScaler's menu or by hand: a removal deletes it however it changed,
/// an update leaves it as it is.
pub fn settings(path: &str) -> bool {
    path == SETTINGS
        || path
            .to_ascii_lowercase()
            .ends_with(&format!("/{PROVIDER}/{PROVIDER_SETTINGS}").to_ascii_lowercase())
}

/// An archive entry's path inside the game folder (with `/`), or `None` for
/// one that would leave it or that Windows cannot name.
pub fn entry_path(name: &str) -> Option<String> {
    let name = name.replace('\\', "/");
    let name = name.strip_suffix('/').unwrap_or(&name);
    if name.is_empty() || name.starts_with('/') {
        return None;
    }
    const RESERVED: [&str; 4] = ["CON", "PRN", "AUX", "NUL"];
    name.split('/')
        .all(|part| {
            let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
            let device = RESERVED.contains(&stem.as_str())
                || ((stem.starts_with("COM") || stem.starts_with("LPT"))
                    && stem.len() == 4
                    && stem.as_bytes()[3].is_ascii_digit());
            !part.is_empty()
                && part != "."
                && part != ".."
                && !part.ends_with(['.', ' '])
                && !device
                && !part
                    .chars()
                    .any(|c| c.is_control() || "<>:\"|?*".contains(c))
        })
        .then(|| name.to_owned())
}

/// A file the host placed in the game folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placed {
    pub sha256: String,
    pub size: u64,
}

/// Exactly what the host placed in one game folder: [`MANIFEST`], or
/// Rubylight 3.0's AMDNR install ([`LEGACY_MANIFEST`]), which has no `mode`,
/// `switches` or `ini`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// [`PLACED_BY`].
    pub by: String,
    /// The release the files came from.
    pub release: String,
    /// The name OptiScaler loads under, one of [`PROXIES`]: the one the host
    /// placed it under, or the user's own OptiScaler's ([`Mode::Addon`]).
    pub proxy: String,
    #[serde(default)]
    pub mode: Mode,
    /// What the user turned on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub switches: Vec<Switch>,
    /// Each placed file by its path in the folder (with `/`).
    pub files: BTreeMap<String, Placed>,
    /// While an update runs: each file it replaces as the release before
    /// placed it. Such a file is the host's in either form, so an update cut
    /// short still leaves only files a removal takes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub previous: BTreeMap<String, Placed>,
    /// Folders the host created, parents first.
    #[serde(default)]
    pub folders: Vec<String>,
    /// Settings files ([`settings`]) that were in a folder an earlier install
    /// left when the host took it over: the user's, used as they are. A
    /// removal leaves them and names them. They are not in `files`, so a
    /// release that does not know this list leaves them too.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub taken_over: Vec<String>,
    /// The user's OptiScaler.ini lines the add-on changed ([`Mode::Addon`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ini: Option<IniRecord>,
}

impl Manifest {
    /// A manifest the host wrote, with paths that stay inside the folder.
    pub fn parse(text: &str) -> Result<Self, String> {
        let manifest: Self =
            serde_json::from_str(text).map_err(|e| format!("the manifest is unreadable: {e}"))?;
        if manifest.by != PLACED_BY {
            return Err("the manifest was not written by Rubylight".into());
        }
        let inside = |path: &String| entry_path(path).as_ref() == Some(path);
        if !PROXIES.contains(&manifest.proxy.as_str())
            || !manifest.files.keys().all(inside)
            || !manifest
                .previous
                .keys()
                .all(|path| manifest.files.contains_key(path))
            || !manifest.folders.iter().all(inside)
            || !manifest.taken_over.iter().all(inside)
            || manifest.ini.as_ref().is_some_and(|ini| !inside(&ini.path))
        {
            return Err("the manifest names a file outside the game folder".into());
        }
        if manifest
            .taken_over
            .iter()
            .any(|path| manifest.files.contains_key(path))
        {
            return Err("the manifest names a file as both placed and taken over".into());
        }
        Ok(manifest)
    }
}

/// A placed file as it is now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Now {
    Missing,
    /// As placed.
    Same,
    /// Changed since: someone else's now, unless it is a settings file.
    Changed,
}

/// What removal deletes and keeps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Removal {
    /// The DLLs first, OptiScaler's proxy before the others: a game that
    /// runs holds them, so a removal stops at the first before anything
    /// else changed.
    pub delete: Vec<String>,
    /// Placed files that changed since (they may be someone else's now),
    /// and the settings the host took over, which are the user's.
    pub keep: Vec<String>,
}

/// Removal of a manifest's files: those still as placed, and the settings
/// files ([`settings`]) it placed however they changed; the settings it
/// took over (`taken_over`) stay. Logs and folders follow in [`logs`] and
/// the manifest's `folders`.
pub fn removal(manifest: &Manifest, now: impl Fn(&str) -> Now) -> Removal {
    let mut removal = Removal::default();
    for path in manifest.files.keys() {
        match now(path) {
            Now::Missing => {}
            Now::Same => removal.delete.push(path.clone()),
            Now::Changed if settings(path) => removal.delete.push(path.clone()),
            Now::Changed => removal.keep.push(path.clone()),
        }
    }
    removal.delete.sort_by_key(|path| {
        if *path == manifest.proxy {
            0
        } else if path.to_ascii_lowercase().ends_with(".dll") {
            1
        } else {
            2
        }
    });
    for path in &manifest.taken_over {
        if now(path) != Now::Missing {
            removal.keep.push(path.clone());
        }
    }
    removal
}

/// Whether `name` in the game folder is a log OptiScaler (or Rubylight 3.0's
/// AMDNR) writes there, which goes with it.
pub fn logs(name: &str) -> bool {
    const LOGS: [&str; 5] = [
        "OptiScaler.log",
        "amd_bridge.log",
        "amd_presr.log",
        "lmxxf_backend.log",
        "amdnr_crash.log",
    ];
    LOGS.iter().any(|log| log.eq_ignore_ascii_case(name))
        || (name.starts_with("OptiScaler.previous") && name.ends_with(".log"))
}

/// `text` as a console or a client may show it: a path through the
/// provider's folder, whose name is the mod's, becomes its file's name with
/// "frame generation's" before it (the log keeps the whole path). The name
/// is found whatever its case, as Windows names folders: a leftover folder
/// taken over keeps the case it has on disk.
pub fn shown(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    let provider = PROVIDER.to_ascii_lowercase();
    // ASCII lower case keeps every byte where it was, so `at` holds in `rest`.
    while let Some(at) = rest.to_ascii_lowercase().find(&provider) {
        let stop = |c: char| c.is_whitespace() || "\"'(),;".contains(c);
        // After the stop character, however many bytes it takes (U+3000, U+00A0).
        let start = rest[..at]
            .char_indices()
            .rev()
            .find(|(_, c)| stop(*c))
            .map_or(0, |(i, c)| i + c.len_utf8());
        let tail = &rest[at + PROVIDER.len()..];
        let end = tail.find(stop).unwrap_or(tail.len());
        let file = tail[..end]
            .trim_end_matches(['.', ':'])
            .rsplit(['/', '\\'])
            .find(|part| !part.is_empty());
        out.push_str(&rest[..start]);
        match file {
            Some(file) => {
                out.push_str("frame generation's ");
                out.push_str(file);
            }
            None => out.push_str("frame generation's folder"),
        }
        let cut = tail[..end].len() - tail[..end].trim_end_matches(['.', ':']).len();
        rest = &tail[end - cut..];
    }
    out.push_str(rest);
    out
}

/// Whether `name` in the provider's `dlssg` folder ([`provider_dlssg`]) is
/// a file the provider writes there itself, which goes with it: its logs
/// (`nr_fg-<date>-<time>.log`) and [`PROVIDER_RUNTIME`]. Nowhere else.
pub fn provider_files(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    (lower.starts_with("nr_fg") && lower.ends_with(".log"))
        || PROVIDER_RUNTIME
            .iter()
            .any(|file| file.eq_ignore_ascii_case(name))
}

/// Whether `relative` (with `/`) in the game folder is the provider's
/// `dlssg` folder, whichever OptiScaler folder holds it.
pub fn provider_dlssg(relative: &str) -> bool {
    let lower = relative.to_ascii_lowercase();
    let end = format!("{PROVIDER}/dlssg").to_ascii_lowercase();
    lower == end || lower.ends_with(&format!("/{end}"))
}

/// Files from a previous release that an updated manifest no longer wants
/// (settings files are always kept for the user).
pub fn stale_manifest_paths(manifest: &Manifest, wanted: &[(String, Placed)]) -> Vec<String> {
    manifest
        .files
        .keys()
        .filter(|path| !settings(path) && !wanted.iter().any(|(target, _)| target == *path))
        .cloned()
        .collect()
}

/// Compact a manifest after a partial removal that left `kept` files behind,
/// keeping only surviving files and folders with nothing left to restore.
pub fn retained_manifest(
    mut manifest: Manifest,
    kept: &[String],
    folder_exists: impl Fn(&str) -> bool,
) -> Manifest {
    manifest.files.retain(|path, _| kept.contains(path));
    manifest.folders.retain(|relative| folder_exists(relative));
    manifest.previous.clear();
    manifest.ini = None;
    manifest
}

/// Check one placed file's observed state before a game launch.
pub fn prepare_file_check(path: &str, now: Now, folder_display: &str) -> Result<(), String> {
    if now != Now::Same && !(settings(path) && now == Now::Changed) {
        let how = if now == Now::Missing {
            "is missing"
        } else {
            "changed"
        };
        return Err(format!(
            "Neural Rendering's {path} {how} in {folder_display}: turn it off and on again"
        ));
    }
    Ok(())
}

/// Why paths in the game's folder stop an install. What is in the
/// provider's folder is still there from an earlier install (one of its
/// files changed since, or something put in its folder), anything else is
/// not Rubylight's. Either is the user's to move out: the host deletes
/// neither.
pub fn in_the_way(paths: &[String]) -> String {
    let (earlier, others): (Vec<String>, Vec<String>) = paths.iter().cloned().partition(|path| {
        path.split('/')
            .any(|part| part.eq_ignore_ascii_case(PROVIDER))
    });
    let named = |paths: &[String]| {
        let mut text = paths.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
        if paths.len() > 3 {
            text.push_str(&format!(" and {} more", paths.len() - 3));
        }
        (text, if paths.len() == 1 { "is" } else { "are" })
    };
    let mut clauses = Vec::new();
    if !others.is_empty() {
        let (text, be) = named(&others);
        clauses.push(format!(
            "{text} {be} in the game's folder and {be} not Rubylight's"
        ));
    }
    if !earlier.is_empty() {
        let (text, be) = named(&earlier);
        clauses.push(format!(
            "{text} {be} still in the game's folder from an earlier install"
        ));
    }
    let it = if paths.len() == 1 { "it" } else { "them" };
    format!("{}: move {it} out first", clauses.join("; "))
}

/// Why an install stops at the earlier upscaling's files it would keep
/// (`kept`): once its removal left them (`removed`), or before anything
/// changed.
pub fn kept_text(kept: &[String], removed: bool) -> String {
    let mut named: Vec<String> = kept.iter().take(3).map(|path| shown(path)).collect();
    if kept.len() > 3 {
        named.push(format!("{} more", kept.len() - 3));
    }
    let (be, it, they) = if kept.len() == 1 {
        ("is", "it", "it")
    } else {
        ("are", "them", "they")
    };
    let named = named.join(", ");
    if removed {
        format!(
            "the earlier upscaling was removed, but {named} {be} still in the game's folder: {they} changed since or {be} not Rubylight's, so frame generation was not installed; move {it} out and turn it on again"
        )
    } else {
        format!(
            "the earlier upscaling can't be removed whole: {named} changed since or {be} not Rubylight's and would stay in the game's folder, so nothing was changed; move {it} out and turn it on again"
        )
    }
}

/// The folder the app's launcher installed the game in, when it says.
pub fn app_install_dir(app: &App) -> Option<PathBuf> {
    ["steam-install-dir", "launcher-install-dir"]
        .iter()
        .find_map(|key| app.extra.get(*key)?.as_str())
        .map(str::trim)
        .filter(|folder| !folder.is_empty())
        .map(PathBuf::from)
}

fn exe_of(app: &App, key: &str) -> Option<PathBuf> {
    app.extra.get(key)?.get("exe")?.as_str().map(PathBuf::from)
}

/// The .exe the files sit beside, once they are in.
pub fn app_placed_exe(app: &App) -> Option<PathBuf> {
    exe_of(app, KEY)
}

/// The .exe Rubylight 3.0 placed AMDNR beside, until it is removed.
pub fn app_legacy_exe(app: &App) -> Option<PathBuf> {
    exe_of(app, LEGACY_KEY)
}

/// Every folder the host placed files in for the app: this release's and
/// Rubylight 3.0's, each once.
pub fn app_placed_exes(app: &App) -> Vec<PathBuf> {
    let mut exes: Vec<PathBuf> = app_placed_exe(app).into_iter().collect();
    if let Some(legacy) = app_legacy_exe(app).filter(|legacy| !exes.contains(legacy)) {
        exes.push(legacy);
    }
    exes
}

/// Whether the host placed files for the app, now or in Rubylight 3.0.
pub fn app_has_files(app: &App) -> bool {
    app.extra.contains_key(KEY) || app.extra.contains_key(LEGACY_KEY)
}

/// What the app's key says once the files are in.
pub fn app_key_value(exe: &Path, release: &str, proxy: &str, mode: Mode) -> Value {
    json!({
        "exe": exe,
        "release": release,
        "proxy": proxy,
        "mode": mode,
        "switches": [Switch::FrameGeneration],
    })
}

/// A saved app keeps the host's keys: the console never sets them.
pub fn app_keep_key(stored: Option<&App>, app: &mut App) {
    for key in [KEY, LEGACY_KEY] {
        app.extra.remove(key);
        if let Some(value) = stored.and_then(|stored| stored.extra.get(key)) {
            app.extra.insert(key.into(), value.clone());
        }
    }
}

/// The apps with files placed that a library change left out: an
/// uninstalled game the launcher sync dropped.
pub fn dropped_apps(before: &[App], after: &[App]) -> Vec<App> {
    let uuid = |app: &App| {
        app.extra
            .get("uuid")
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    before
        .iter()
        .filter(|app| app_has_files(app))
        .filter(|app| {
            uuid(app).is_some_and(|id| {
                !after
                    .iter()
                    .any(|other| uuid(other).as_deref() == Some(&id))
            })
        })
        .cloned()
        .collect()
}

/// A game folder the files are still to be removed from: its app left the
/// library while the folder could not be reached or the removal failed.
/// Rubylight 3.0's records name AMDNR's folders the same way.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub name: String,
    pub exe: PathBuf,
    /// The app's, whose switch slot the removal takes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
}

impl Pending {
    /// One for each folder the host placed files in for the app.
    pub fn of(app: &App) -> Vec<Self> {
        let uuid = app
            .extra
            .get("uuid")
            .and_then(Value::as_str)
            .map(str::to_owned);
        app_placed_exes(app)
            .into_iter()
            .map(|exe| Self {
                name: app.name.clone(),
                exe,
                uuid: uuid.clone(),
            })
            .collect()
    }
}

/// Merge new pending removals into an existing list without duplicating `exe`.
pub fn merge_pending(existing: &mut Vec<Pending>, added: &[Pending]) {
    for removal in added {
        if !existing.iter().any(|old| old.exe == removal.exe) {
            existing.push(removal.clone());
        }
    }
}

/// Drop completed removals from a pending list.
pub fn retain_pending_not_done(existing: &mut Vec<Pending>, done: &[PathBuf]) {
    existing.retain(|removal| !done.contains(&removal.exe));
}
