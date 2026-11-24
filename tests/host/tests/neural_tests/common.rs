//! Shared fixtures, package builders, and local release server helpers for Neural Rendering tests.
use anyhow::Result;
use butterpollo_session::neural::{self, Placed};
use butterpollo_session::state::App;
use butterpollo_stream::neural::folder::{PACKAGE, Package, Request, Response, Writer, handle};
use butterpollo_stream::neural::*;
use butterpollo_stream::state::Shared;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// A folder of its own under the temp folder, deleted when dropped.
pub(crate) struct Temp(PathBuf);
impl Temp {
    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
pub(crate) fn tempdir() -> Temp {
    let path = std::env::temp_dir().join(format!("rubylight-neural-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    Temp(path)
}

pub(crate) fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// The provider's DLL, settings and a model file as the full package
/// places them in a game.
pub(crate) const PDLL: &str = "OptiScaler/custom/Apex Neural Rendering FG/dlssg/nvngx_dlssg.dll";
pub(crate) const PINI: &str = "OptiScaler/custom/Apex Neural Rendering FG/dlssg/nr_fg.ini";
pub(crate) const PAK: &str = "OptiScaler/custom/Apex Neural Rendering FG/dlssg/kernels/pack.nrx";

/// A cached release `tag` in `root` with `files` (cache paths), as
/// [`extract`] leaves it.
pub(crate) fn package(root: &Path, tag: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let folder = root.join(tag);
    let mut placed = BTreeMap::new();
    for (path, bytes) in files {
        let file = folder.join("files").join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, bytes).unwrap();
        placed.insert(
            path.to_string(),
            Placed {
                sha256: sha(bytes),
                size: bytes.len() as u64,
            },
        );
    }
    let package = Package {
        tag: tag.into(),
        archives: ["full", "addon"]
            .map(|part| {
                (
                    format!("anr-fg-{tag}-{part}.zip"),
                    sha(format!("{tag} {part}").as_bytes()),
                )
            })
            .into(),
        files: placed,
    };
    std::fs::write(
        folder.join(PACKAGE),
        serde_json::to_string(&package).unwrap(),
    )
    .unwrap();
    folder
}

pub(crate) fn release(root: &Path, tag: &str, dll: &[u8]) -> PathBuf {
    let (pdll, pini, pak) = (
        format!("full/game/{PDLL}"),
        format!("full/game/{PINI}"),
        format!("full/game/{PAK}"),
    );
    package(
        root,
        tag,
        &[
            ("full/game/dxgi.dll", dll),
            ("full/game/OptiScaler.ini", b"[FrameGen]\nFGInput=nvngxfg\n"),
            ("full/game/OptiScaler/libxess.dll", b"xess"),
            (&pdll, b"provider"),
            (&pini, b"path = auto\n"),
            (&pak, b"model"),
            ("full/LICENSES/OptiScaler-GPL-3.0.txt", b"GPL"),
            (
                "addon/Apex Neural Rendering FG/dlssg/nvngx_dlssg.dll",
                b"addon provider",
            ),
            (
                "addon/Apex Neural Rendering FG/dlssg/nr_fg.ini",
                b"path = auto\n",
            ),
            (
                "addon/Apex Neural Rendering FG/dlssg/kernels/pack.nrx",
                b"addon model",
            ),
        ],
    )
}

/// Every file and folder below `folder` with each file's SHA-256.
pub(crate) fn snapshot(folder: &Path) -> Vec<(String, Option<String>)> {
    names(folder)
        .into_iter()
        .map(|name| {
            let path = folder.join(&name);
            let hash = path.is_file().then(|| sha(&std::fs::read(&path).unwrap()));
            (name, hash)
        })
        .collect()
}

/// A game folder with its .exe, a game-owned README, its own DLSS frame
/// generation and ReShade as dxgi.dll, in a Steam library's `common`.
pub(crate) fn game(root: &Path) -> PathBuf {
    let folder = root.join("library/common/Game");
    std::fs::create_dir_all(&folder).unwrap();
    for (name, bytes) in [
        ("Game.exe", &b"MZ game"[..]),
        ("README.md", b"the game's readme"),
        ("nvngx_dlssg.dll", b"NVIDIA's DLSS frame generation"),
        ("dxgi.dll", b"ReShade"),
    ] {
        std::fs::write(folder.join(name), bytes).unwrap();
    }
    folder.join("Game.exe")
}

pub(crate) fn names(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = walkdir(folder)
        .into_iter()
        .map(|path| {
            path.strip_prefix(folder)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    names.sort();
    names
}
fn walkdir(folder: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(folder).unwrap().flatten() {
        found.push(entry.path());
        if entry.file_type().unwrap().is_dir() {
            found.extend(walkdir(&entry.path()));
        }
    }
    found
}

/// The cached release `folder` as the host hands it to the helper.
pub(crate) fn cached_at(folder: &Path) -> butterpollo_stream::neural::folder::Cached {
    Package::read(folder).unwrap().with(folder.to_path_buf())
}

pub(crate) fn install(exe: Option<&Path>, install: Option<&Path>, package: &Path) -> Response {
    handle(
        Request::Install {
            exe: exe.map(Path::to_path_buf),
            install: install.map(Path::to_path_buf),
            package: cached_at(package),
        },
        Writer::User,
        &|_, _| {},
    )
}
pub(crate) fn remove(exe: &Path) -> Response {
    handle(
        Request::Remove {
            exe: exe.to_path_buf(),
            beside: None,
            earlier: false,
        },
        Writer::User,
        &|_, _| {},
    )
}
pub(crate) fn prepare(exe: &Path, package: Option<&Path>) -> Response {
    handle(
        Request::Prepare {
            exe: exe.to_path_buf(),
            install: None,
            package: package.map(cached_at),
        },
        Writer::User,
        &|_, _| {},
    )
}

/// `path` open without delete sharing, as a running game or a scanner may
/// hold a DLL: it can be neither replaced nor deleted until dropped.
pub(crate) fn hold(path: &Path) -> std::fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    // FILE_SHARE_READ only.
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(path)
        .unwrap()
}

/// A junction at `link` to `target`, as another account sharing a library
/// could plant one.
pub(crate) fn junction(link: &Path, target: &Path) {
    // cmd takes a `/` in a path for an option.
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .args([link, target].map(|path| path.to_string_lossy().replace('/', "\\")))
        .output()
        .unwrap();
    assert!(made.status.success(), "{made:?}");
}

pub(crate) fn archive(path: &Path, files: &[(&str, &[u8])]) {
    let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, bytes) in files {
        writer.start_file(*name, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap();
}

/// A drive letter with nothing behind it on this PC: an unplugged drive.
pub(crate) fn unconnected_drive() -> PathBuf {
    (b'D'..=b'Z')
        .rev()
        .map(|letter| PathBuf::from(format!("{}:\\", letter as char)))
        .find(|root| !root.exists())
        .expect("a free drive letter")
}

pub(crate) fn switched_on(name: &str, uuid: &str, exe: &Path) -> Value {
    json!({
        "name": name,
        "uuid": uuid,
        KEY: {"exe": exe, "release": "Alpha0.3.5.8", "proxy": "winmm.dll"},
    })
}

pub(crate) fn app_named(name: &str, uuid: &str) -> App {
    App {
        name: name.into(),
        cmd: String::new(),
        working_dir: String::new(),
        prep: Vec::new(),
        computed_id: None,
        aliases: Vec::new(),
        extra: [("uuid".to_string(), json!(uuid))].into_iter().collect(),
    }
}

/// The switch's job for `uuid` once it ended (or after 20 s).
pub(crate) async fn ended(h: &Shared, uuid: &str) -> Job {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let job = h.neural.lock().jobs[uuid].clone();
        if matches!(job.phase, "done" | "failed") || Instant::now() > deadline {
            return job;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// GitHub as [`fetch`] sees it, on this PC: the pinned release by its tag
/// and each file, a part of one by range; every request logged in order.
pub(crate) struct Server {
    pub(crate) base: String,
    pub(crate) log: Arc<Mutex<Vec<String>>>,
}
pub(crate) type Files = Arc<BTreeMap<String, Vec<u8>>>;
pub(crate) async fn serve(listener: tokio::net::TcpListener, list: Value, files: Files) -> Server {
    use axum::{
        extract::{Path as Url, State},
        http::{HeaderMap, StatusCode, header},
        response::IntoResponse,
        routing::get,
    };
    type Shared = (Arc<Value>, Files, Arc<Mutex<Vec<String>>>);
    async fn list_route(State((list, _, log)): State<Shared>) -> axum::Json<Value> {
        log.lock().unwrap().push("list".into());
        axum::Json((*list).clone())
    }
    async fn file_route(
        State((_, files, log)): State<Shared>,
        Url((_, name)): Url<(String, String)>,
        headers: HeaderMap,
    ) -> axum::response::Response {
        let Some(bytes) = files.get(&name) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let range = headers
            .get(header::RANGE)
            .and_then(|range| range.to_str().ok()?.strip_prefix("bytes=")?.split_once('-'))
            .map(|(start, end)| {
                (
                    start.parse::<usize>().unwrap(),
                    end.parse::<usize>().unwrap(),
                )
            });
        match range {
            Some((start, end)) => {
                log.lock().unwrap().push(format!("{name} (part)"));
                (StatusCode::PARTIAL_CONTENT, bytes[start..=end].to_vec()).into_response()
            }
            None => {
                log.lock().unwrap().push(name);
                bytes.clone().into_response()
            }
        }
    }
    let base = format!("http://{}", listener.local_addr().unwrap());
    let log = Arc::new(Mutex::new(Vec::new()));
    let repository = neural::REPOSITORY;
    let app = axum::Router::new()
        .route(
            &format!("/api/repos/{repository}/releases/tags/{{tag}}"),
            get(list_route),
        )
        .route(
            &format!("/{repository}/releases/download/{{tag}}/{{name}}"),
            get(file_route),
        )
        .with_state((Arc::new(list), files, log.clone()));
    tokio::spawn(async move { axum::serve(listener, app).await });
    Server { base, log }
}

/// [`fetch_using`] without holding the release: one fetch at a time.
pub(crate) async fn fetch(
    h: &Shared,
    progress: &(dyn Fn(u64, u64) + Sync),
) -> Result<(PathBuf, Package)> {
    let _one = h.neural.fetching.lock().await;
    fetch_locked(h, progress)
        .await
        .map(|(folder, package, _held)| (folder, package))
}

/// The cached release `folder` held as [`fetch_using`] holds it for an install.
pub(crate) fn pin(h: &Shared, folder: &Path) -> Using {
    *h.neural
        .lock()
        .using
        .entry(folder.to_path_buf())
        .or_default() += 1;
    Using::for_test(h.clone(), folder.to_path_buf(), None)
}

/// The cached release `folder` as the one this host pins: [`fetch`] takes
/// it without asking GitHub.
pub(crate) fn pin_to(h: &Shared, folder: &Path) {
    let package = Package::read(folder).unwrap();
    let file = |part: &str| {
        let (asset, sha256) = package
            .archives
            .iter()
            .find(|(asset, _)| asset.ends_with(part))
            .unwrap();
        neural::PinnedFile {
            asset: asset.clone().leak(),
            sha256: sha256.clone().leak(),
        }
    };
    *h.neural.pin.lock().unwrap() = Some(neural::Pin {
        tag: package.tag.clone().leak(),
        full: file("-full.zip"),
        addon: file("-addon.zip"),
        sums: neural::PinnedFile {
            asset: "SHA256SUMS.txt",
            sha256: "",
        },
    });
}

pub(crate) const TAG: &str = "FG-V.B16";
pub(crate) const FULL: &str = "anr-fg-V.B16-full.zip";
pub(crate) const ADDON: &str = "anr-fg-V.B16-addon.zip";

/// `files` as the release this host pins, with their digests.
pub(crate) fn pin_files(h: &Shared, files: &BTreeMap<String, Vec<u8>>) {
    let file = |asset: &'static str| neural::PinnedFile {
        asset,
        sha256: sha(&files[asset]).leak(),
    };
    *h.neural.pin.lock().unwrap() = Some(neural::Pin {
        tag: TAG,
        full: file(FULL),
        addon: file(ADDON),
        sums: file("SHA256SUMS.txt"),
    });
}

/// The release as GitHub lists it: the full package, the add-on and the
/// checksum list naming both.
pub(crate) fn anr_files() -> BTreeMap<String, Vec<u8>> {
    let temp = tempdir();
    let zip = |name: &str, files: &[(&str, &[u8])]| {
        let path = temp.path().join(name);
        archive(&path, files);
        (name.to_owned(), std::fs::read(path).unwrap())
    };
    let (pdll, pak) = (format!("game/{PDLL}"), format!("game/{PAK}"));
    let mut files: BTreeMap<String, Vec<u8>> = [
        zip(
            FULL,
            &[
                ("game/dxgi.dll", b"dll"),
                ("game/OptiScaler.ini", b"[FrameGen]\n"),
                (&pdll, b"provider"),
                (&pak, b"model"),
            ],
        ),
        zip(
            ADDON,
            &[(
                "Apex Neural Rendering FG/dlssg/nvngx_dlssg.dll",
                b"addon provider",
            )],
        ),
    ]
    .into_iter()
    .collect();
    files.insert("SHA256SUMS.txt".into(), sums_of(&files).into_bytes());
    files
}

/// A checksum list naming `files`.
pub(crate) fn sums_of(files: &BTreeMap<String, Vec<u8>>) -> String {
    files
        .iter()
        .map(|(name, bytes)| format!("{}  {name}\n", sha(bytes)))
        .collect()
}

/// The release `tag` with `files` as GitHub lists it by its tag.
pub(crate) fn release_list(base: &str, tag: &str, files: &BTreeMap<String, Vec<u8>>) -> Value {
    let assets: Vec<Value> = files
        .iter()
        .map(|(name, bytes)| {
            json!({
                "name": name,
                "browser_download_url": format!("{base}/{}/releases/download/{tag}/{name}", neural::REPOSITORY),
                "digest": format!("sha256:{}", sha(bytes)),
                "size": bytes.len(),
            })
        })
        .collect();
    json!({"tag_name": tag, "published_at": "2026-10-01T00:00:00Z", "assets": assets})
}
