//! Release download, SHA-256 verification, ZIP unpacking, and cache folder
//! trust checks for Neural Rendering.
use super::{
    Hold, StreamStarted, Using,
    folder::{self, PACKAGE, Package},
};
use crate::state::Shared;
use anyhow::{Context, Result, anyhow, bail, ensure};
use butterpollo_session::neural::{self, Download, Placed, Release};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

/// `%ProgramData%\Butterpollo\neural-anr` for the installed host, the
/// profile's own `neural-anr` folder for any other.
pub fn cache_root(h: &Shared) -> PathBuf {
    data_root(h).join("neural-anr")
}

/// Where Rubylight 3.0 kept its AMDNR download: `neural` beside the cache.
pub fn legacy_cache_root(h: &Shared) -> PathBuf {
    data_root(h).join("neural")
}

/// The host's own folder: `%ProgramData%\Butterpollo` for the installed
/// host, the profile otherwise. Setup locks it down for the installed host,
/// so it is where [`trusted_root`]'s check of who may change folders ends.
pub fn data_root(h: &Shared) -> PathBuf {
    if h.directory == butterpollo_session::paths::installed_profile() {
        butterpollo_session::paths::data_root()
    } else {
        h.directory.clone()
    }
}

/// Delete Rubylight 3.0's AMDNR download, which nothing installs from any
/// more, once the host starts: only from a cache this host trusts, never
/// through a link on the way. Game folders keep their copies. Blocks.
pub fn clear_legacy_cache(h: &Shared) {
    let root = legacy_cache_root(h);
    if !root.exists() {
        return;
    }
    let cleared = trusted_root(
        &root,
        &data_root(h),
        butterpollo_win_io::process::is_system(),
    )
    .and_then(|()| std::fs::remove_dir_all(&root).map_err(anyhow::Error::from));
    match cleared {
        Ok(()) => {
            tracing::info!(folder = %root.display(), "Neural Rendering: deleted the earlier upscaling download")
        }
        Err(error) => {
            tracing::warn!(folder = %root.display(), error = %format!("{error:#}"), "Neural Rendering: the earlier upscaling download could not be deleted")
        }
    }
}

/// The cache folder, readable by users (the folder helper copies from it as
/// the signed-in user) and writable only by administrators.
fn ensure_root(root: &Path) -> Result<()> {
    if root.is_dir() {
        return Ok(());
    }
    if butterpollo_win_io::process::elevated()? {
        match butterpollo_win_io::process::create_folder_administrators_readable(root) {
            Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => {
                return Err(e).with_context(|| format!("cannot create {}", root.display()));
            }
            _ => {}
        }
    } else {
        std::fs::create_dir_all(root)
            .with_context(|| format!("cannot create {}", root.display()))?;
    }
    Ok(())
}

/// Refuse a cache that is not this host's own: a link on the way to it, or
/// for a SYSTEM host, which copies from it into game folders and deletes in
/// it, a cache anyone but administrators may change, or a folder from it up
/// to the host's data root they don't own or whose folders they may move
/// away and replace. A release planted there would reach every game. Above
/// the data root only the link check applies, and not because those folders are
/// safe: `C:\ProgramData` itself lets other accounts move folders on a PC where
/// it was opened up, so a check there would refuse every user. The callers hold
/// the cache and its data root open instead ([`Hold`]), which is what keeps the
/// paths this check passed naming what is later read.
pub fn trusted_root(root: &Path, data_root: &Path, system: bool) -> Result<()> {
    trusted_root_with(root, data_root, system, folder::administrators_hold)
}

/// [`trusted_root`], with `hold` in place of [`folder::administrators_hold`].
pub fn trusted_root_with(
    root: &Path,
    data_root: &Path,
    system: bool,
    hold: impl Fn(&Path, bool) -> Result<(), String>,
) -> Result<()> {
    use crate::os_ext::MetadataExt;
    ensure!(
        root.starts_with(data_root),
        "{} is not in this host's own folder {}",
        root.display(),
        data_root.display()
    );
    for path in root.ancestors().filter(|path| !path.as_os_str().is_empty()) {
        let meta = std::fs::symlink_metadata(path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        ensure!(
            meta.file_attributes() & 0x400 == 0,
            "{} is a link to elsewhere, and Neural Rendering keeps no download behind one",
            path.display()
        );
        if system && path.starts_with(data_root) {
            hold(path, path == root).map_err(|problem| {
                anyhow!(
                    "Neural Rendering keeps its download only where nobody but administrators may change it: {problem}"
                )
            })?;
        }
    }
    Ok(())
}

/// The release Rubylight installs: [`neural::PINNED`], or a test's.
pub fn pinned(h: &Shared) -> neural::Pin<'static> {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(pin) = *h.neural.pin.lock().unwrap() {
        return pin;
    }
    let _ = h;
    neural::PINNED
}

/// Whether the cached `package` is the `pin`ned release: the same archives
/// by name and SHA-256, not only the same tag.
pub fn serves(package: &Package, pin: &neural::Pin) -> bool {
    neural::serves(package, pin)
}

/// The cached release's files against its package.json, once per host
/// start: a file changed in the cache must not reach a game. Blocks.
pub fn verify_cache(h: &Shared, folder: &Path, package: &Package) -> Result<()> {
    if h.neural.lock().verified.as_deref() == Some(folder) {
        return Ok(());
    }
    for (path, placed) in &package.files {
        let file = folder.join("files").join(path);
        let (sha256, size) = folder::sha256_file(&file)
            .with_context(|| format!("cannot read {}", file.display()))?;
        ensure!(
            sha256 == placed.sha256 && size == placed.size,
            "the downloaded {path} in {} changed since it was checked",
            folder.display()
        );
    }
    h.neural.lock().verified = Some(folder.to_path_buf());
    Ok(())
}

/// The pinned release for a launch to finish an update cut short with,
/// from a cache this host trusts. None when it is not cached. Not rehashed:
/// an update copies each file checked against the release as it goes, and
/// most launches copy nothing. The [`Hold`] comes back with it, so the cache
/// stays where it was checked until the caller's copy is done.
pub fn launch_cache(h: &Shared) -> Result<Option<(folder::Cached, Hold)>> {
    let root = cache_root(h);
    if !root.exists() {
        return Ok(None);
    }
    let data_root = data_root(h);
    // Held before the check, as the NVIDIA components' folders are: what the
    // check approves is what is read after it, not a same-named replacement.
    let held = Hold::new([root.as_path(), data_root.as_path()])
        .context("cannot open the Neural Rendering cache")?;
    trusted_root(&root, &data_root, butterpollo_win_io::process::is_system())?;
    let pin = pinned(h);
    Ok(cached(&root)
        .filter(|(_, package)| serves(package, &pin))
        .map(|(folder, package)| (package.with(folder), held)))
}

/// Where GitHub's release list and the release files are: GitHub, or a
/// test's own server.
fn origins(h: &Shared) -> (String, String) {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(base) = h.neural.origin.lock().unwrap().clone() {
        return (format!("{base}/api"), base);
    }
    let _ = h;
    ("https://api.github.com".into(), neural::GITHUB.to_owned())
}

/// The updater's client for GitHub (HTTPS only, a User-Agent, which
/// GitHub's API requires), a plain one for a test's own server.
pub fn http(h: &Shared, timeout: Duration) -> Result<reqwest::Client> {
    #[cfg(any(test, feature = "test-support"))]
    if h.neural.origin.lock().unwrap().is_some() {
        return Ok(reqwest::Client::builder().timeout(timeout).build()?);
    }
    let _ = h;
    crate::updater::client(timeout)
}

/// The release in the cache, the newest when a crash left two.
pub fn cached(root: &Path) -> Option<(PathBuf, Package)> {
    std::fs::read_dir(root)
        .ok()?
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| !entry.file_name().to_string_lossy().ends_with(".partial"))
        .filter_map(|entry| {
            let folder = entry.path();
            let modified = std::fs::metadata(folder.join(PACKAGE))
                .and_then(|m| m.modified())
                .ok()?;
            let package = folder::read_package(&folder).ok()?;
            Some((modified, folder, package))
        })
        .max_by_key(|(modified, ..)| *modified)
        .map(|(_, folder, package)| (folder, package))
}

/// The pinned release with the whole mod for an install, in the cache:
/// downloaded and checked unless it is there already, and kept there until
/// the [`Using`] goes. A download gives up when a stream starts
/// ([`StreamStarted`]).
pub async fn fetch_using(
    h: &Shared,
    progress: &(dyn Fn(u64, u64) + Sync),
) -> Result<(Using, Package)> {
    // Older releases go only under this lock: the release is kept before
    // another fetch may clear it.
    let _one = h.neural.fetching.lock().await;
    let (folder, package, held) = fetch_locked(h, progress).await?;
    *h.neural.lock().using.entry(folder.clone()).or_default() += 1;
    Ok((
        Using {
            h: h.clone(),
            folder,
            held: Some(held),
        },
        package,
    ))
}

pub async fn fetch_locked(
    h: &Shared,
    progress: &(dyn Fn(u64, u64) + Sync),
) -> Result<(PathBuf, Package, Hold)> {
    let root = cache_root(h);
    ensure_root(&root)?;
    let data_root = data_root(h);
    // Held before the check, as the NVIDIA components' folders are: the cache
    // that was checked cannot be renamed away and replaced while it is read,
    // verified or downloaded into.
    let held = Hold::new([root.as_path(), data_root.as_path()])
        .context("cannot open the Neural Rendering cache")?;
    trusted_root(&root, &data_root, butterpollo_win_io::process::is_system())?;
    let pin = pinned(h);
    // Its files as checked when they came, or it goes and comes again.
    if let Some(have) = cached(&root).filter(|(_, package)| serves(package, &pin)) {
        let h = h.clone();
        let checked = tokio::task::spawn_blocking(move || {
            let (folder, package) = have;
            verify_cache(&h, &folder, &package).map(|()| (folder, package))
        })
        .await?;
        match checked {
            Ok((folder, package)) => return Ok((folder, package, held)),
            Err(error) => {
                tracing::warn!(error = %format!("{error:#}"), "Neural Rendering: the cached release is damaged; it is downloaded again")
            }
        }
    }
    let (api_origin, files_origin) = origins(h);
    let api = http(h, Duration::from_secs(35))?;
    let listed: Value = api
        .get(format!(
            "{api_origin}/repos/{}/releases/tags/{}",
            neural::REPOSITORY,
            pin.tag
        ))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let release = neural::pinned(&listed, &files_origin, &pin).map_err(anyhow::Error::msg)?;
    // The release's checksum list (itself pinned) names both archives with
    // the digests GitHub reports.
    let sums_path = root.join(format!("{}.partial", release.sums.asset));
    let text = match download(h, &release.sums, &sums_path, &|_, _| {}).await {
        Ok(()) => std::fs::read_to_string(&sums_path)
            .with_context(|| format!("the release's {} is unreadable", release.sums.asset)),
        Err(error) => Err(error),
    };
    let _ = std::fs::remove_file(&sums_path);
    let sums = neural::sums(&text?).map_err(anyhow::Error::msg)?;
    let archives = [&release.full, &release.addon];
    neural::check_archives(&archives, &sums).map_err(|e| anyhow!("{e}; nothing was downloaded"))?;
    let paths: Vec<PathBuf> = archives
        .iter()
        .map(|archive| root.join(format!("{}.partial", archive.asset)))
        .collect();
    let discard = || {
        for path in &paths {
            let _ = std::fs::remove_file(path);
        }
    };
    let total = archives.iter().map(|archive| archive.size).sum();
    let mut before = 0;
    for (archive, path) in archives.iter().zip(&paths) {
        let progress = |done: u64, _: u64| progress(before + done, total);
        if let Err(error) = download(h, archive, path, &progress).await {
            discard();
            return Err(error);
        }
        before += archive.size;
    }
    let (folder, package) = {
        let (h, root, release, paths) = (h.clone(), root.clone(), release.clone(), paths.clone());
        tokio::task::spawn_blocking(move || {
            // No copy reads the cache while a release in it is replaced
            // or older ones go.
            let _writing = h.neural.writing();
            let (folder, package) = extract(&paths, &root, &release)?;
            // Games have their own copies; older releases in the cache serve
            // nothing once no install is about to copy them.
            let using = h.neural.lock().using.clone();
            for entry in std::fs::read_dir(&root).into_iter().flatten().flatten() {
                if entry.path() != folder && !using.contains_key(&entry.path()) {
                    let _ = if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                        std::fs::remove_dir_all(entry.path())
                    } else {
                        std::fs::remove_file(entry.path())
                    };
                }
            }
            h.neural.lock().verified = Some(folder.clone());
            Ok::<_, anyhow::Error>((folder, package))
        })
        .await?
    }
    .inspect_err(|_| discard())?;
    discard();
    tracing::info!(release = %release.tag, folder = %folder.display(), "Neural Rendering: downloaded and checked the frame generation release");
    Ok((folder, package, held))
}

pub async fn download(
    h: &Shared,
    release: &Download,
    path: &Path,
    progress: &(dyn Fn(u64, u64) + Sync),
) -> Result<()> {
    // `pinned` offers nothing else; this keeps it so.
    ensure!(
        neural::allowed_asset(&release.asset),
        "{} is not a file Rubylight fetches",
        release.asset
    );
    let _ = std::fs::remove_file(path);
    let mut response = http(h, Duration::from_secs(1800))?
        .get(&release.url)
        .send()
        .await?
        .error_for_status()?;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await
        .with_context(|| format!("cannot create {}", path.display()))?;
    let mut hash = Sha256::new();
    let mut size = 0;
    while let Some(chunk) = response.chunk().await? {
        if crate::updater::streaming(h) {
            return Err(StreamStarted.into());
        }
        size += chunk.len() as u64;
        ensure!(
            size <= release.size,
            "{} is larger than its release says",
            release.asset
        );
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
        hash.update(&chunk);
        progress(size, release.size);
    }
    file.sync_all().await?;
    verified(size, &hex::encode(hash.finalize()), release)
}

/// The file is exactly what GitHub published for the release.
pub fn verified(size: u64, sha256: &str, release: &Download) -> Result<()> {
    neural::verify_download(size, sha256, release).map_err(anyhow::Error::msg)
}

/// Unpack the release into the cache: from each archive (the full package,
/// then the add-on, as downloaded and checked) what [`neural::Archive`]
/// takes, under `files/<prefix>/`. Blocks.
pub fn extract(archives: &[PathBuf], root: &Path, release: &Release) -> Result<(PathBuf, Package)> {
    let staging = root.join(format!("{}.partial", release.tag));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    let into = staging.join("files");
    std::fs::create_dir_all(&into)?;
    let [full, addon] = archives else {
        bail!("the release's two archives are not there to unpack");
    };
    let mut total = 0u64;
    let mut files = BTreeMap::new();
    for (path, kind, download) in [
        (full, neural::Archive::Full, &release.full),
        (addon, neural::Archive::Addon, &release.addon),
    ] {
        files.extend(unpack(path, &download.asset, kind, &into, &mut total)?);
    }
    neural::check_whole(&files)
        .map_err(|e| anyhow!("{}: {e}; nothing was installed", release.tag))?;
    let package = Package {
        tag: release.tag.clone(),
        archives: [&release.full, &release.addon]
            .into_iter()
            .map(|download| (download.asset.clone(), download.digest.clone()))
            .collect(),
        files,
    };
    butterpollo_session::state::write_json(&staging.join(PACKAGE), &package)?;
    let folder = root.join(&release.tag);
    // The same release there before (damaged, or changed on GitHub) goes
    // aside whole, never half deleted under an install about to copy it;
    // the cache's clearing takes it out after.
    let aside = root.join(format!("{}.replaced.partial", release.tag));
    if aside.exists() {
        std::fs::remove_dir_all(&aside)?;
    }
    let replacing = folder.exists();
    if replacing {
        std::fs::rename(&folder, &aside)
            .with_context(|| format!("cannot replace {}", folder.display()))?;
    }
    if let Err(error) = std::fs::rename(&staging, &folder) {
        let back = if !replacing || std::fs::rename(&aside, &folder).is_ok() {
            ""
        } else {
            ", and the release before it stays aside"
        };
        return Err(error).with_context(|| format!("cannot place {}{back}", folder.display()));
    }
    Ok((folder, package))
}

/// Unpack the entries of one archive that `kind` takes into
/// `into/<prefix>/`, with their SHA-256, keyed by that cache path. Every
/// entry must name a path inside the folder, taken or not. Blocks.
fn unpack(
    archive: &Path,
    asset: &str,
    kind: neural::Archive,
    into: &Path,
    total: &mut u64,
) -> Result<BTreeMap<String, Placed>> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(archive)?)
        .with_context(|| format!("{asset} is no zip archive"))?;
    let mut files = BTreeMap::new();
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let name = entry.name().to_owned();
        let path = neural::entry_path(&name)
            .with_context(|| format!("{asset} has a file outside its folder: {name:?}"))?;
        if entry.is_dir() || !kind.takes(&path) {
            continue;
        }
        let cached = format!("{}/{path}", kind.prefix());
        ensure!(!files.contains_key(&cached), "{asset} has {path} twice");
        let target = into.join(&cached);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut output = std::fs::File::create_new(&target)?;
        let mut hash = Sha256::new();
        let mut buffer = vec![0; 1 << 20];
        let mut size = 0u64;
        loop {
            let read = entry.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            size += read as u64;
            *total += read as u64;
            ensure!(
                *total <= 4 * neural::MAX_ARCHIVE,
                "{asset} unpacks to more than it should"
            );
            hash.update(&buffer[..read]);
            output.write_all(&buffer[..read])?;
        }
        output.sync_all()?;
        files.insert(
            cached,
            Placed {
                sha256: hex::encode(hash.finalize()),
                size,
            },
        );
    }
    Ok(files)
}
