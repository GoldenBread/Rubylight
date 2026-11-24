//! Release pins, asset allowlists, checksum verification, and cached archive
//! package validation for Neural Rendering.
use super::manifest::{PROVIDER, PROVIDER_DLL, PROXY_SOURCE, Placed, SETTINGS, entry_path};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf};

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
/// The release's checksum list. It is checked, not placed.
pub const SUMS: &str = "SHA256SUMS.txt";
/// A cached package's description, beside its `files` folder.
pub const PACKAGE: &str = "package.json";
/// Where release files are downloaded from.
pub const GITHUB: &str = "https://github.com";

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

/// Check that a downloaded file's observed size and SHA-256 digest match what
/// GitHub published for `release`.
pub fn verify_download(size: u64, sha256: &str, release: &Download) -> Result<(), String> {
    if size != release.size {
        return Err(format!(
            "{} is {size} bytes, its release says {}",
            release.asset, release.size
        ));
    }
    if sha256 != release.digest {
        return Err(format!(
            "{} does not match its release's SHA-256 digest; nothing was installed",
            release.asset
        ));
    }
    Ok(())
}

/// A release in the host's cache: `files/full/` and `files/addon/` hold
/// what the host takes from its two archives, each checked against the pin,
/// GitHub's digest and the release's SHA256SUMS.txt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Package {
    pub tag: String,
    /// Each archive's name and SHA-256: the cache serves the release only
    /// while its archives are these.
    pub archives: BTreeMap<String, String>,
    /// Each file by its cache path (`full/...`, `addon/...`).
    pub files: BTreeMap<String, Placed>,
}

/// A cached release and the files it holds as the host checked them: the
/// helper takes these hashes, never the cache's package.json read again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cached {
    pub folder: PathBuf,
    pub package: Package,
}

impl Package {
    /// Parse a cached package's `package.json` text and validate that every
    /// file it names stays inside the cache folder.
    pub fn parse(text: &str, folder_label: &str) -> Result<Self, String> {
        let package: Self = serde_json::from_str(text)
            .map_err(|e| format!("the cached release in {folder_label} is damaged: {e}"))?;
        package.validate_paths(folder_label)?;
        Ok(package)
    }

    /// Whether each file it names is one inside the cached release folder.
    pub fn validate_paths(&self, folder_label: &str) -> Result<(), String> {
        if self
            .files
            .keys()
            .all(|path| entry_path(path).as_ref() == Some(path))
        {
            Ok(())
        } else {
            Err(format!(
                "the cached release in {folder_label} names a file outside it"
            ))
        }
    }

    /// Whether this cached package is the `pin`ned release: the same archives
    /// by name and SHA-256, not only the same tag.
    pub fn serves(&self, pin: &Pin) -> bool {
        let wanted: BTreeMap<String, String> = [pin.full, pin.addon]
            .into_iter()
            .map(|file| (file.asset.to_owned(), file.sha256.to_owned()))
            .collect();
        self.tag == pin.tag && self.archives == wanted
    }

    /// Pair this validated package with its cache folder path.
    pub fn with(self, folder: PathBuf) -> Cached {
        Cached {
            folder,
            package: self,
        }
    }
}

/// Whether the cached `package` is the `pin`ned release: the same archives
/// by name and SHA-256, not only the same tag.
pub fn serves(package: &Package, pin: &Pin) -> bool {
    package.serves(pin)
}
