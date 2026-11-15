//! Neural Rendering for an app: frame generation that the host fetches from
//! its authors' GitHub release on this PC, places in the game's folder, keeps
//! on the release Rubylight pins ([`PINNED`]) and removes again. It is ANR FG
//! by Apex Compute (<https://github.com/apex-compute/ANR>, no licence
//! published), a DLSS frame generation provider that OptiScaler (GPL-3.0)
//! loads in place of the game's own: the release's full package brings
//! OptiScaler with it, its add-on goes into an OptiScaler the user installed.
//! Rubylight never hosts, bundles or changes its files.
//!
//! Rubylight 3.0 installed AMDNR by 3zwr1 (an OptiScaler fork, GPL-3.0) for
//! neural upscaling instead. The host no longer offers it, but still removes
//! what it placed then: the [`LEGACY_MANIFEST`] in a game folder names it.
//!
//! This file decides: which release asset may be fetched, whether the files
//! match their checksums, where each file goes, which DLL name OptiScaler
//! loads under, how OptiScaler.ini is changed and put back, what refuses a
//! game folder (anti-cheat, another mod) and which GPU can run it. The
//! host's `neural` module downloads and moves the files.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// The repository downloads come from: its releases only.
pub const REPOSITORY: &str = "apex-compute/ANR";

/// The release Rubylight installs, pinned as Nucleus Co-op's is: no other
/// release is ever taken on its own. To move to another, change it here and
/// only here: the tag, and each file's name and SHA-256 from the release's
/// SHA256SUMS.txt (GitHub's digest must say the same), then run the host's
/// `neural::tests::real_release_into_a_fake_game`. Later switches from the
/// same release (upscaling, ray reconstruction) take their files from these
/// archives as [`Switch`]es.
pub const PINNED: Pin<'static> = Pin {
    tag: "FG-V.B16",
    full: PinnedFile {
        asset: "anr-fg-V.B16-full.zip",
        sha256: "3abebbedb6843efb12648921e191cf7eeec23247206fe4400d2b6f7763413e7d",
    },
    addon: PinnedFile {
        asset: "anr-fg-V.B16-addon.zip",
        sha256: "764f1e182258484c763fb92187f1e16c849dcb7146a342db49367e15477d641d",
    },
    sums: PinnedFile {
        asset: SUMS,
        sha256: "5dd49db220428da33d206400717c43151804f6d0f8db31e125a9eedfdb1f2970",
    },
};

/// A release as Rubylight pins it ([`PINNED`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pin<'a> {
    pub tag: &'a str,
    /// OptiScaler with the provider, for a game without OptiScaler.
    pub full: PinnedFile<'a>,
    /// The provider alone, for an OptiScaler the user installed.
    pub addon: PinnedFile<'a>,
    /// The release's checksum list of its archives.
    pub sums: PinnedFile<'a>,
}

/// A file of a pinned release: its name and SHA-256, lower-case hex.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PinnedFile<'a> {
    pub asset: &'a str,
    pub sha256: &'a str,
}

/// The largest archive the host downloads (the release's are 3 and 11 MB).
pub const MAX_ARCHIVE: u64 = 256 << 20;
/// The largest checksum list the host reads.
pub const MAX_SUMS: u64 = 1 << 20;
/// The record of what the host placed in a game folder.
pub const MANIFEST: &str = "rubylight-neural-anr.json";
/// Rubylight 3.0's record of the AMDNR it placed, which the host still removes.
pub const LEGACY_MANIFEST: &str = "rubylight-neural.json";
/// The release's checksum list. It is checked, not placed.
pub const SUMS: &str = "SHA256SUMS.txt";
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

/// A part of the pinned release the user turns on for a game. Each takes
/// its files from the same archives and OptiScaler.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Switch {
    FrameGeneration,
}
impl Switch {
    pub const ALL: [Switch; 1] = [Switch::FrameGeneration];
    pub fn id(self) -> &'static str {
        match self {
            Switch::FrameGeneration => "frame-generation",
        }
    }
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|switch| switch.id() == id)
    }
}

/// One of the release's archives as the host keeps it in its cache:
/// `files/<prefix>/<path in the archive>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Archive {
    Full,
    Addon,
}
impl Archive {
    pub fn prefix(self) -> &'static str {
        match self {
            Archive::Full => "full",
            Archive::Addon => "addon",
        }
    }
    /// Whether the host takes `path` out of the archive: OptiScaler, its
    /// settings, the provider and OptiScaler's licence. The installer
    /// scripts, readme and package description are left out.
    pub fn takes(self, path: &str) -> bool {
        match self {
            Archive::Full => path.starts_with("game/") || path.starts_with("LICENSES/"),
            Archive::Addon => path.starts_with(&format!("{PROVIDER}/")),
        }
    }
}

/// What a release asset the host may fetch is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Asset<'a> {
    /// `anr-fg-<version>-full.zip`: OptiScaler (GPL-3.0) with the provider.
    Full(&'a str),
    /// `anr-fg-<version>-addon.zip`: the provider alone.
    Addon(&'a str),
    /// `SHA256SUMS.txt`: the release's checksums of its archives.
    Sums,
}

/// What a release asset is, or `None` for one that is never fetched. Only
/// the two packages and the checksum list may be: the release's developer
/// kits and OptiScaler's source archive are not for games.
pub fn asset(name: &str) -> Option<Asset<'_>> {
    if name == SUMS {
        return Some(Asset::Sums);
    }
    let version = name
        .strip_prefix("anr-fg-")
        .and_then(|rest| rest.strip_suffix(".zip"))?;
    let valid = |version: &str| {
        !version.is_empty()
            && version.len() <= 32
            && version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.')
    };
    if let Some(version) = version.strip_suffix("-full").filter(|v| valid(v)) {
        return Some(Asset::Full(version));
    }
    version
        .strip_suffix("-addon")
        .filter(|v| valid(v))
        .map(Asset::Addon)
}

/// Whether a release asset may be fetched at all; see [`asset`].
pub fn allowed_asset(name: &str) -> bool {
    asset(name).is_some()
}

/// One file of a release, as GitHub lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Download {
    pub asset: String,
    pub url: String,
    /// GitHub's SHA-256 of the file, lower-case hex.
    pub digest: String,
    pub size: u64,
}

/// The pinned release's files as GitHub lists them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub tag: String,
    pub full: Download,
    pub addon: Download,
    pub sums: Download,
}

/// Where release files are downloaded from.
pub const GITHUB: &str = "https://github.com";

/// The release `pin` names as GitHub lists it
/// (`/repos/{REPOSITORY}/releases/tags/{tag}`), its files at `origin`
/// ([`GITHUB`], or a test's own server): only with GitHub's digest of each
/// pinned file the same as the pin's. A draft is not taken; a pre-release
/// is, as the SHA-256 pin decides which files are taken.
pub fn pinned(listed: &Value, origin: &str, pin: &Pin) -> Result<Release, String> {
    let missing = || {
        format!(
            "GitHub does not list the release {} with its packages; nothing was downloaded",
            pin.tag
        )
    };
    if listed["draft"] == true || listed["tag_name"] != pin.tag {
        return Err(missing());
    }
    let assets = listed["assets"].as_array().ok_or_else(missing)?;
    let file = |pinned: &PinnedFile, limit: u64| -> Result<Download, String> {
        let name = pinned.asset;
        let asset = assets
            .iter()
            .find(|asset| asset["name"] == name)
            .ok_or_else(missing)?;
        let url = format!("{origin}/{REPOSITORY}/releases/download/{}/{name}", pin.tag);
        let digest = asset["digest"]
            .as_str()
            .and_then(|d| d.strip_prefix("sha256:"))
            .filter(|d| d.len() == 64 && d.bytes().all(|b| b.is_ascii_hexdigit()))
            .map(str::to_ascii_lowercase);
        let size = asset["size"]
            .as_u64()
            .filter(|size| *size > 0 && *size <= limit);
        match (asset["browser_download_url"] == url.as_str(), digest, size) {
            (true, Some(digest), Some(size)) if digest == pinned.sha256 => Ok(Download {
                asset: name.into(),
                url,
                digest,
                size,
            }),
            (true, Some(_), Some(_)) => Err(format!(
                "GitHub lists {name} of release {} with another SHA-256 than Rubylight pins; nothing was downloaded",
                pin.tag
            )),
            _ => Err(missing()),
        }
    };
    Ok(Release {
        tag: pin.tag.into(),
        full: file(&pin.full, MAX_ARCHIVE)?,
        addon: file(&pin.addon, MAX_ARCHIVE)?,
        sums: file(&pin.sums, MAX_SUMS)?,
    })
}

/// Each archive the host downloads must be in the release's checksum list
/// with the digest GitHub reports for it.
pub fn check_archives(
    archives: &[&Download],
    sums: &BTreeMap<String, String>,
) -> Result<(), String> {
    for archive in archives {
        match sums.get(&archive.asset) {
            None => return Err(format!("{} is not in the release's {SUMS}", archive.asset)),
            Some(listed) if *listed != archive.digest => {
                return Err(format!(
                    "{} does not match the release's {SUMS}",
                    archive.asset
                ));
            }
            Some(_) => {}
        }
    }
    Ok(())
}

/// `SHA256SUMS.txt`: `<sha256>  <path>` lines, as path (with `/`) to lower-case hash.
pub fn sums(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut sums = BTreeMap::new();
    for line in text.trim_start_matches('\u{feff}').lines() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        let (hash, path) = line
            .split_once(' ')
            .filter(|(hash, _)| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| format!("{SUMS} has a line that is no checksum: {line:?}"))?;
        let path = path.trim_start_matches([' ', '*']).replace('\\', "/");
        let path = path.trim_start_matches("./");
        let hash = hash.to_ascii_lowercase();
        if sums
            .insert(path.to_owned(), hash.clone())
            .is_some_and(|old| old != hash)
        {
            return Err(format!(
                "{SUMS} lists {path} twice with different checksums"
            ));
        }
    }
    Ok(sums)
}

/// The cache paths a release must hold: OptiScaler, its settings and the
/// provider from the full package, and the provider from the add-on.
pub fn required() -> [String; 4] {
    [
        format!("full/{PROXY_SOURCE}"),
        format!("full/game/{SETTINGS}"),
        format!("full/game/OptiScaler/custom/{PROVIDER}/{PROVIDER_DLL}"),
        format!("addon/{PROVIDER}/{PROVIDER_DLL}"),
    ]
}

/// The whole release is among the unpacked files (cache paths).
pub fn check_whole<V>(files: &BTreeMap<String, V>) -> Result<(), String> {
    match required()
        .into_iter()
        .find(|required| !files.contains_key(required))
    {
        Some(missing) => Err(format!("the release has no {missing}")),
        None => Ok(()),
    }
}

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
/// folder itself ([`Mode::Addon`], [`opti_dir`]).
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

/// What has one of the [`PROXIES`] names in the game folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Occupant {
    Free,
    /// OptiScaler as the host placed it.
    Ours,
    /// Another program's file: a game DLL or another mod's loader.
    Foreign,
    /// An OptiScaler the host did not place.
    OptiScaler,
}

/// The name the full package's OptiScaler loads under: the one the host
/// used before, else the first free of [`FG_PROXIES`]; a game that only uses
/// Vulkan (`vulkan_only`, [`vulkan_only`]) never loads dxgi.dll, so it gets
/// winmm.dll or version.dll. A name another program holds is never taken.
pub fn proxy(
    vulkan_only: bool,
    occupant: impl Fn(&str) -> Occupant,
) -> Result<&'static str, String> {
    let names: &[&'static str] = if vulkan_only {
        &FG_PROXIES[1..]
    } else {
        &FG_PROXIES
    };
    let occupants: Vec<_> = names.iter().map(|name| (*name, occupant(name))).collect();
    occupants
        .iter()
        .find(|(_, o)| *o == Occupant::Ours)
        .or_else(|| occupants.iter().find(|(_, o)| *o == Occupant::Free))
        .map(|(name, _)| *name)
        .ok_or_else(|| {
            format!(
                "other programs' files hold every name OptiScaler can load under ({})",
                names.join(", ")
            )
        })
}

/// Whether a program's bytes name `name` (ASCII lower-case) in ASCII or
/// UTF-16, whatever its case.
fn names(bytes: &[u8], name: &str) -> bool {
    let wide: Vec<u8> = name.bytes().flat_map(|b| [b, 0]).collect();
    let matches = |needle: &[u8]| {
        bytes
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle))
    };
    matches(name.as_bytes()) || matches(&wide)
}

/// Whether a game's program uses Vulkan and never DXGI: it names
/// vulkan-1.dll and none of dxgi.dll, d3d11.dll and d3d12.dll, as the
/// release's installer tells them.
pub fn vulkan_only(program: &[u8]) -> bool {
    names(program, "vulkan-1.dll")
        && !["dxgi.dll", "d3d11.dll", "d3d12.dll"]
            .iter()
            .any(|name| names(program, name))
}

/// Whether an OptiScaler DLL can load a frame generation provider from its
/// `custom` folder: OptiScaler builds that can log "Custom DLSSG {}
/// initialized". AMDNR's fork and older OptiScaler releases cannot.
pub fn custom_providers(dll: &[u8]) -> bool {
    dll.windows(12).any(|window| window == b"Custom DLSSG")
}

/// Whether a file is a sign that a game has DLSS frame generation for the
/// provider to take the place of: NVIDIA's DLSS frame generation DLL or
/// Streamline's plugin for it.
pub fn dlss_frame_generation(name: &str) -> bool {
    ["nvngx_dlssg.dll", "sl.dlss_g.dll"]
        .iter()
        .any(|file| name.eq_ignore_ascii_case(file))
}

/// Another mod in the game's folder that replaces the same calls (another
/// OptiScaler, or an upscaler or frame-generation replacement), by one of its
/// file names. ReShade and other loaders that only share a DLL name are not
/// in the list: OptiScaler takes another name beside them.
pub fn other_mod(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    let starts = |prefix: &str| lower.starts_with(prefix);
    Some(match lower.as_str() {
        "optiscaler.dll" | "optiscaler.ini" | "optiscaler.asi" | "optiscaler" | "nvngx.ini" => {
            "OptiScaler"
        }
        "fakenvapi.dll" | "fakenvapi.ini" => "fakenvapi (an OptiScaler add-on)",
        "specialk64.dll" | "specialk32.dll" => "Special K",
        _ if starts("dlss-enabler") => "DLSS Enabler",
        _ if starts("dlssg_to_fsr3") => "dlssg-to-fsr3",
        _ if starts("uniscaler") => "Uniscaler",
        _ => return None,
    })
}

/// The anti-cheat a file or folder name in a game's install belongs to.
/// OptiScaler injects a DLL into the game, which kernel and online
/// anti-cheat treat as cheating; such games are refused.
pub fn anti_cheat(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    let starts = |prefix: &str| lower.starts_with(prefix);
    Some(match lower.as_str() {
        _ if starts("easyanticheat")
            || starts("eac_launcher")
            || lower == "start_protected_game.exe" =>
        {
            "Easy Anti-Cheat"
        }
        "battleye" => "BattlEye",
        _ if starts("beservice") || starts("beclient") || lower.ends_with("_be.exe") => "BattlEye",
        "vgc.exe" | "vgk.sys" | "riot vanguard" => "Riot Vanguard",
        _ if lower.contains("randgrid") => "Ricochet",
        _ if starts("mhyprot") => "mhyprot",
        "gameguard" | "nprotect" | "gameguard.des" => "nProtect GameGuard",
        _ if starts("npgg") && lower.ends_with(".des") => "nProtect GameGuard",
        "x3.xem" => "XIGNCODE3",
        _ if starts("xigncode") || starts("xhunter") => "XIGNCODE3",
        "pbsvc.exe" | "punkbuster" => "PunkBuster",
        _ if starts("pnkbstr") => "PunkBuster",
        _ if starts("equ8") => "EQU8",
        _ if starts("faceit") => "FACEIT",
        "anticheatexpert" => "Anti-Cheat Expert",
        _ if starts("sguard") || starts("ace-base") => "Anti-Cheat Expert",
        _ if starts("eaanticheat") => "EA Javelin",
        _ if lower.contains("denuvo") && lower.contains("anti") => "Denuvo Anti-Cheat",
        _ if starts("neacsafe") || starts("neaccontroller") => "NetEase Anti-Cheat",
        "hshield" | "ehsvc.dll" => "AhnLab HackShield",
        _ => return None,
    })
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

/// The files the provider writes into its own `dlssg` folder once a game
/// runs it, besides its logs ([`provider_files`]): `anr-vkloader.dll`, its
/// copy of the system's Vulkan loader. FG-V.B16's log says so on the
/// first frame: "native bridge: our Vulkan loader
/// ...\dlssg\anr-vkloader.dll, a copy of the system's vulkan-1.dll apart
/// from the game's".
pub const PROVIDER_RUNTIME: [&str; 1] = ["anr-vkloader.dll"];

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

// ---- OptiScaler.ini ----

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
struct Ini {
    bom: bool,
    crlf: bool,
    /// Split on `\n`: the last is what follows the last line break.
    lines: Vec<String>,
}
impl Ini {
    fn read(bytes: &[u8]) -> Result<Self, String> {
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
    fn get(&self, section: &str, key: &str) -> Result<Option<String>, String> {
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

// ---- The GPU ----

/// A GPU as the AMD driver's compute runtime reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gpu {
    pub name: String,
    /// `gfx1100` and so on.
    pub arch: String,
    pub compute_units: u32,
    /// The adapter's LUID (low part, then high part), when the runtime says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub luid: Option<u64>,
}

/// A hardware GPU as DXGI names it: the one the host captures and encodes
/// with, or another in the PC.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adapter {
    pub name: String,
    /// Its LUID: the low part, then the high part.
    pub luid: u64,
    /// Its PCI vendor and device ID.
    pub vendor: u32,
    pub device: u32,
}

pub const AMD: u32 = 0x1002;
pub const NVIDIA: u32 = 0x10de;
/// The first NVIDIA device ID of the Turing generation (GeForce GTX 16 and
/// RTX 20), the oldest the provider runs on; every later one is newer.
pub const MIN_NVIDIA_DEVICE: u32 = 0x1e00;
/// The Visual C++ 2015-2022 runtime (x64) OptiScaler links: without it the
/// game starts without OptiScaler and so without frame generation.
pub const VC_RUNTIME: [&str; 4] = [
    "MSVCP140.dll",
    "MSVCP140_ATOMIC_WAIT.dll",
    "VCRUNTIME140.dll",
    "VCRUNTIME140_1.dll",
];

/// AMD device IDs of RDNA graphics: the release's `path_table.json` ranges
/// (discrete Radeon RX 5000 to 9000, Steam Deck) and the integrated RDNA 2,
/// 3 and 3.5 graphics it does not list (Rembrandt, Raphael, Mendocino,
/// Phoenix, Hawk Point, Strix Point, Strix Halo, Krackan Point). Used only
/// when the compute runtime can't say.
const RDNA: [(u32, u32); 19] = [
    (0x7310, 0x731f),
    (0x7340, 0x734f),
    (0x7360, 0x736f),
    (0x73a0, 0x73ff),
    (0x7420, 0x743f),
    (0x7440, 0x74ff),
    (0x7550, 0x759f),
    (0x163f, 0x163f),
    (0x1435, 0x1435),
    (0x1681, 0x1681),
    (0x164e, 0x164e),
    (0x1506, 0x1506),
    (0x15bf, 0x15bf),
    (0x15c8, 0x15c8),
    (0x1900, 0x1901),
    (0x150e, 0x150e),
    (0x1586, 0x1586),
    (0x1114, 0x1114),
    (0x73f0, 0x73f0),
];

/// The GPU frame generation runs on, as [`capable`] found it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capable {
    pub gpu: String,
    /// The GPU the host streams with, when it is not that one.
    pub streams: Option<String>,
}
impl Capable {
    /// What the switch says when frame generation runs on a GPU the host
    /// does not stream with: the host can't know which GPU a game renders on.
    pub fn note(&self) -> Option<String> {
        self.streams.as_ref().map(|streams| {
            format!(
                "Frame generation runs when the game renders on {}; Rubylight streams with {streams}.",
                self.gpu
            )
        })
    }
}

/// Whether devices are offered frame generation at all: any of the PC's
/// hardware GPUs (`adapters`) is AMD's or NVIDIA's, by vendor alone.
pub fn offered(adapters: &[Adapter]) -> bool {
    adapters
        .iter()
        .any(|adapter| matches!(adapter.vendor, AMD | NVIDIA))
}

/// Whether [`capable`] needs the AMD compute runtime's GPUs: an AMD GPU is
/// among them, and the GPU the host streams with is no NVIDIA one that runs
/// it already.
pub fn needs_probe(streams: &Adapter, adapters: &[Adapter]) -> bool {
    !(streams.vendor == NVIDIA && streams.device >= MIN_NVIDIA_DEVICE)
        && std::iter::once(streams)
            .chain(adapters)
            .any(|adapter| adapter.vendor == AMD)
}

/// What the provider runs on, as its release says: AMD RDNA (Radeon RX 5000
/// and newer, and RDNA integrated graphics) and NVIDIA Turing and newer
/// (GeForce GTX 16, RTX 20 and newer). The GPU the host streams with
/// (`streams`) counts first; else any other of the PC's hardware GPUs
/// (`adapters`, AMD or NVIDIA), since a game may render on a GPU the host
/// does not stream with, as on a laptop whose display runs on its
/// integrated graphics. An AMD GPU is judged by the compute runtime's
/// architecture (`gpus`, matched by LUID, else by a name no other GPU has),
/// else by its PCI device ID; one neither identifies is refused.
pub fn capable(
    streams: &Adapter,
    adapters: &[Adapter],
    gpus: Result<&[Gpu], &str>,
) -> Result<Capable, String> {
    let supported = "frame generation runs on AMD Radeon RX 5000 and newer graphics (RDNA) and NVIDIA GeForce GTX 16, RTX 20 and newer";
    let first = verdict(streams, gpus);
    if first == Verdict::Runs {
        return Ok(Capable {
            gpu: streams.name.clone(),
            streams: None,
        });
    }
    let others: Vec<(&Adapter, Verdict)> = adapters
        .iter()
        .filter(|adapter| adapter.luid != streams.luid && matches!(adapter.vendor, AMD | NVIDIA))
        .map(|adapter| (adapter, verdict(adapter, gpus)))
        .collect();
    if let Some((adapter, _)) = others.iter().find(|(_, verdict)| *verdict == Verdict::Runs) {
        return Ok(Capable {
            gpu: adapter.name.clone(),
            streams: Some(streams.name.clone()),
        });
    }
    let name = &streams.name;
    let mut reason = match first {
        Verdict::Older(None) => format!("{supported}; this PC streams with {name}, which is older"),
        Verdict::Older(Some(arch)) => {
            format!("{supported}; this PC streams with {name} ({arch}), which is older")
        }
        Verdict::Unknown(why) => format!(
            "Rubylight can't identify the GPU it streams with ({name}, device {:#06x}) as one frame generation runs on: {why}",
            streams.device
        ),
        Verdict::Other | Verdict::Runs => format!("{supported}; this PC streams with {name}"),
    };
    for (adapter, verdict) in others {
        let name = &adapter.name;
        reason.push_str(&match verdict {
            Verdict::Older(None) => format!("; it also has {name}, which is older"),
            Verdict::Older(Some(arch)) => format!("; it also has {name} ({arch}), which is older"),
            Verdict::Unknown(why) => format!(
                "; it also has {name} (device {:#06x}), which Rubylight can't identify: {why}",
                adapter.device
            ),
            Verdict::Other | Verdict::Runs => String::new(),
        });
    }
    Err(reason)
}

/// The Visual C++ runtime OptiScaler needs, of which this PC lacks
/// `missing`.
pub fn runtime(missing: &[&str]) -> Result<(), String> {
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "frame generation needs the Microsoft Visual C++ 2015-2022 Redistributable (x64), which this PC lacks ({})",
            missing.join(", ")
        ))
    }
}

/// Whether frame generation runs on one GPU ([`capable`]).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Verdict {
    Runs,
    /// Older than it runs on; an AMD GPU's architecture when the runtime said.
    Older(Option<String>),
    /// An AMD GPU neither the runtime nor its device ID identifies, and why.
    Unknown(String),
    /// Neither AMD's nor NVIDIA's.
    Other,
}

fn verdict(adapter: &Adapter, gpus: Result<&[Gpu], &str>) -> Verdict {
    match adapter.vendor {
        NVIDIA if adapter.device >= MIN_NVIDIA_DEVICE => Verdict::Runs,
        NVIDIA => Verdict::Older(None),
        AMD => {
            let found = gpus.ok().and_then(|gpus| {
                gpus.iter()
                    .find(|gpu| gpu.luid == Some(adapter.luid))
                    .or_else(|| {
                        let mut named = gpus.iter().filter(|gpu| gpu.name == adapter.name);
                        match (named.next(), named.next()) {
                            (Some(gpu), None) => Some(gpu),
                            _ => None,
                        }
                    })
            });
            if let Some(gpu) = found {
                let arch = gpu.arch.to_ascii_lowercase();
                let number = arch.strip_prefix("gfx").unwrap_or("");
                let rdna =
                    number.len() >= 4 && number[..2].parse::<u32>().is_ok_and(|major| major >= 10);
                return if rdna {
                    Verdict::Runs
                } else {
                    Verdict::Older(Some(gpu.arch.clone()))
                };
            }
            if RDNA
                .iter()
                .any(|(first, last)| (*first..=*last).contains(&adapter.device))
            {
                return Verdict::Runs;
            }
            Verdict::Unknown(match gpus {
                Err(error) => format!("the AMD compute runtime can't say ({error})"),
                Ok(_) => "the AMD compute runtime doesn't list it".into(),
            })
        }
        _ => Verdict::Other,
    }
}

#[cfg(test)]
mod tests;
