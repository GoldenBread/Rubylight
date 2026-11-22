//! Covers from SteamGridDB for apps that have none, with the API key the user saved
//! (`steamgriddb_api_key`). What to ask and how to read the answers is decided in
//! `butterpollo_session::steamgriddb`; this module asks, from the housekeeping loop and never from a
//! request, and saves each cover into the profile's covers folder as the app's `image-path`, the
//! file `/appasset` and every client's library read.
//!
//! A cover the user set, or one a launcher sync placed, is never replaced: only an app whose
//! `image-path` is empty or Steam's placeholder gets one, and it is marked
//! ([`sgdb::COVER_FIELD`]) so a launcher's or Playnite's cover replaces it later and a cover the
//! user cleared is not put back. A 429 pauses every request for as long as SteamGridDB asks (or a
//! doubling wait), a refused key pauses them until the key changes and tells the console, and an
//! app whose lookup failed is asked about again after ten minutes, an hour, then a day. The key
//! lives in the administrators-only `credentials` folder, goes only into the API requests'
//! `Authorization` header, never to the image servers and never into a log line.

use crate::state::Shared;
use anyhow::{Result, bail};
use butterpollo_session::steamgriddb as sgdb;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex, PoisonError, atomic::Ordering},
    time::{Duration, Instant},
};

/// One HTTP answer, as much as the lookup reads of it.
pub(crate) struct Reply {
    pub status: u16,
    /// The `Retry-After` header of a 429.
    pub retry_after: Option<String>,
    /// The `Content-Type` header.
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

/// The requests a pass makes, so tests can answer them.
pub(crate) trait Http {
    /// GET `url`, with `key` as the bearer token when given. An `Err` is a request that got no
    /// answer (no network, a timeout); its message must not hold the key.
    fn get(&self, url: &str, key: Option<&str>) -> Result<Reply>;
}

/// The real requests, with one client for every pass: no redirects, three seconds to connect,
/// fifteen in all, and at most 16 MiB read.
struct Web;
static CLIENT: LazyLock<Result<reqwest::Client, String>> = LazyLock::new(|| {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(15))
        .user_agent(concat!("Rubylight/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| error.to_string())
});
impl Http for Web {
    fn get(&self, url: &str, key: Option<&str>) -> Result<Reply> {
        let client = CLIENT
            .as_ref()
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| anyhow::anyhow!("SteamGridDB requests need the host's runtime"))?;
        runtime.block_on(async {
            let mut request = client.get(url);
            if let Some(key) = key {
                let mut value = reqwest::header::HeaderValue::from_str(&format!("Bearer {key}"))
                    .map_err(|_| {
                        anyhow::anyhow!(
                            "the SteamGridDB API key has characters a request cannot carry"
                        )
                    })?;
                value.set_sensitive(true);
                request = request.header(reqwest::header::AUTHORIZATION, value);
            }
            // Errors go to the log without their URL, and the header with the key is never in one.
            let mut response = request
                .send()
                .await
                .map_err(|error| anyhow::anyhow!("{}", error.without_url()))?;
            let status = response.status().as_u16();
            let header = |name| {
                response
                    .headers()
                    .get(name)
                    .and_then(|value: &reqwest::header::HeaderValue| value.to_str().ok())
                    .map(str::to_owned)
            };
            let retry_after = header(reqwest::header::RETRY_AFTER);
            let content_type = header(reqwest::header::CONTENT_TYPE);
            if response.content_length().is_some_and(|n| n > LIMIT) {
                bail!("the answer is too large");
            }
            // Read chunk by chunk: an answer without a length stops at the limit too.
            let mut body = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|error| anyhow::anyhow!("{}", error.without_url()))?
            {
                if (body.len() + chunk.len()) as u64 > LIMIT {
                    bail!("the answer is too large");
                }
                body.extend_from_slice(&chunk);
            }
            Ok(Reply {
                status,
                retry_after,
                content_type,
                body,
            })
        })
    }
}
const LIMIT: u64 = 16 << 20;

/// How often a host with apps still missing covers looks again, unless the library changes.
const PASS_EVERY: Duration = Duration::from_secs(10 * 60);
/// Apps looked up in one pass; the rest wait for the next.
const PER_PASS: usize = 8;
/// The pause between two API requests, so a pass never bursts.
#[cfg(not(test))]
const GAP: Duration = Duration::from_millis(1500);
#[cfg(test)]
const GAP: Duration = Duration::ZERO;
/// The next pass after one that reached [`PER_PASS`] with apps left.
const MORE_AFTER: Duration = Duration::from_secs(60);
/// The pause after a request that got no answer, a server error or a 403.
const UNREACHABLE_WAIT: Duration = Duration::from_secs(5 * 60);
/// The console's warning while SteamGridDB refuses the key.
pub const REFUSED_WARNING: &str = "steamgriddb_key_refused";

/// One host's requests: when they may go again and why not.
#[derive(Default)]
struct Pace {
    /// No request before this (a 429, a 403 or an unreachable server).
    paused_until: Option<Instant>,
    /// A pause was set: the next pass comes as soon as it is over.
    resume: bool,
    /// The last 429's wait, doubled by the next one without `Retry-After`.
    backoff: Duration,
    /// The fingerprint of the key SteamGridDB refused: nothing is asked until the key changes.
    refused: Option<u64>,
    last_pass: Option<Instant>,
    /// The library and settings as the last pass left them: as it found them plus its own covers.
    last_version: u64,
    /// The last pass left apps for the next: it comes after [`MORE_AFTER`].
    more: bool,
}
/// By profile directory, so each host (and each test host) keeps its own.
static PACES: LazyLock<Mutex<HashMap<PathBuf, Pace>>> = LazyLock::new(Default::default);
fn with_pace<T>(h: &Shared, change: impl FnOnce(&mut Pace) -> T) -> T {
    let mut paces = PACES.lock().unwrap_or_else(PoisonError::into_inner);
    change(paces.entry(h.directory.clone()).or_default())
}

fn fingerprint(key: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}

/// Where the console keeps the key: the profile's `credentials` folder, which the service keeps
/// to SYSTEM and Administrators.
fn key_file(h: &Shared) -> PathBuf {
    h.directory.join("credentials").join(sgdb::KEY_FILE)
}

/// The saved key: the console's, else one written into `sunshine.conf` by hand.
pub fn api_key(h: &Shared) -> Option<String> {
    let saved = std::fs::read_to_string(key_file(h)).ok();
    if let Some(key) = saved
        .as_deref()
        .and_then(sgdb::key)
        .filter(|key| *key != sgdb::MASK && sgdb::check_key(key).is_ok())
    {
        return Some(key.to_owned());
    }
    sgdb::api_key(&h.config.read().unwrap()).map(str::to_owned)
}

/// Whether a key is saved anywhere, for the console's mask.
pub fn has_key(h: &Shared) -> bool {
    key_file(h).is_file()
        || h.config
            .read()
            .unwrap()
            .values
            .get(sgdb::KEY_SETTING)
            .is_some_and(|value| !value.trim().is_empty())
}

/// Saves `key` (`None` or empty removes it) where only administrators read it, after
/// [`sgdb::check_key`]. The caller takes the key out of `sunshine.conf`.
pub fn store_key(h: &Shared, key: Option<&str>) -> Result<()> {
    let file = key_file(h);
    match key.and_then(sgdb::key) {
        Some(key) => {
            sgdb::check_key(key).map_err(|message| anyhow::anyhow!(message))?;
            let folder = file.parent().unwrap_or(&h.directory);
            let created = !folder.is_dir();
            std::fs::create_dir_all(folder)?;
            if created
                && butterpollo_win_io::process::is_system()
                && let Err(error) = butterpollo_win_io::process::restrict_to_administrators(folder)
            {
                tracing::warn!(%error, "the credentials folder could not be kept to administrators");
            }
            butterpollo_session::state::atomic_write(&file, key.as_bytes())?;
        }
        None => match std::fs::remove_file(&file) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        },
    }
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    /// Whether a test's host runs as SYSTEM.
    pub static AS_SYSTEM: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
/// Whether the host runs as SYSTEM, where the service keeps the credentials folder to
/// administrators.
fn as_system() -> bool {
    #[cfg(any(test, feature = "test-support"))]
    return AS_SYSTEM.with(std::cell::Cell::get);
    #[cfg(not(any(test, feature = "test-support")))]
    butterpollo_win_io::process::is_system()
}

/// When the host runs as SYSTEM, a key typed into `sunshine.conf` by hand (`config`) moves to the
/// credentials folder and out of `config`: true when it moved. The caller writes `config`.
pub fn take_conf_key(h: &Shared, config: &mut butterpollo_config::config::Config) -> Result<bool> {
    if !as_system() {
        return Ok(false);
    }
    let Some(key) = sgdb::api_key(config).map(str::to_owned) else {
        return Ok(false);
    };
    let saved = std::fs::read_to_string(key_file(h)).ok();
    if saved
        .as_deref()
        .and_then(sgdb::key)
        .is_some_and(|saved| saved != key)
    {
        tracing::info!(
            "the SteamGridDB API key typed into sunshine.conf replaces the one saved in the console"
        );
    }
    store_key(h, Some(&key))?;
    config.values.remove(sgdb::KEY_SETTING);
    key_changed(h);
    Ok(true)
}

/// At start: a key typed into `sunshine.conf` by hand moves to the credentials folder.
pub fn migrate_key(h: &Shared) {
    let moved = (|| -> Result<bool> {
        let _save = h.config_save.lock().unwrap();
        let mut config = if h.port_override.is_some() {
            butterpollo_config::config::Config::load(&h.config_path)?
        } else {
            h.config.read().unwrap().clone()
        };
        if !take_conf_key(h, &mut config)? {
            return Ok(false);
        }
        butterpollo_session::state::atomic_write(&h.config_path, config.text().as_bytes())?;
        if let Some(port) = h.port_override {
            config.values.insert("port".into(), port.to_string());
        }
        *h.config.write().unwrap() = config;
        h.config_version.fetch_add(1, Ordering::Release);
        Ok(true)
    })();
    match moved {
        Ok(true) => tracing::info!(
            "the SteamGridDB API key moved from sunshine.conf to the credentials folder"
        ),
        Ok(false) => {}
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), "the SteamGridDB API key could not be moved out of sunshine.conf")
        }
    }
}

/// The key was set, changed, saved again or emptied: a refusal of the last one is over, and its
/// warning is taken back, so a later refusal shows again even if this one was dismissed.
pub fn key_changed(h: &Shared) {
    with_pace(h, |pace| pace.refused = None);
    withdraw_refusal(h);
}
fn withdraw_refusal(h: &Shared) {
    crate::host_warnings::withdraw(&h.directory, &h.warnings, REFUSED_WARNING);
    h.metadata.lock().unwrap().take();
}
#[cfg(any(test, feature = "test-support"))]
pub fn refused(h: &Shared) -> bool {
    with_pace(h, |pace| pace.refused.is_some())
}
#[cfg(any(test, feature = "test-support"))]
pub fn refuse(h: &Shared, key: &str) {
    with_pace(h, |pace| pace.refused = Some(fingerprint(key)));
    h.warnings.set(REFUSED_WARNING, "refused");
}

/// The housekeeping loop's look for covers, on a blocking thread of its own.
pub fn watch(h: &Shared) {
    watch_with(h, &Web);
}

/// An app that may get a cover.
struct Wanted {
    uuid: String,
    name: String,
    steam: Option<u32>,
}
/// No `image-path`, or Steam's placeholder, and never a SteamGridDB cover before: a marked app
/// has one, or the user cleared or replaced it.
fn wants_cover(app: &Value) -> bool {
    let marked = app
        .get(sgdb::COVER_FIELD)
        .and_then(Value::as_str)
        .is_some_and(|marker| !marker.is_empty());
    !marked
        && app
            .get("image-path")
            .and_then(Value::as_str)
            .is_none_or(|image| {
                let image = image.trim();
                image.is_empty() || image == butterpollo_session::steam::FALLBACK_IMAGE
            })
}
fn wanted(h: &Shared) -> Vec<Wanted> {
    h.apps
        .read()
        .unwrap()
        .iter()
        .filter_map(|app| {
            let value = serde_json::to_value(app).ok()?;
            if !wants_cover(&value) {
                return None;
            }
            let uuid = value.get("uuid")?.as_str()?.to_owned();
            let safe = !uuid.is_empty()
                && uuid
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
            safe.then(|| Wanted {
                uuid,
                name: app.name.clone(),
                steam: butterpollo_session::steam::game_of(&value),
            })
        })
        .collect()
}

/// What one API request ended with.
enum Answer {
    Body(Vec<u8>),
    /// 404: SteamGridDB has nothing there.
    NotFound,
    /// Another 4xx: this request for this app is refused; the pass goes on.
    Refused,
    /// The pass stops, with the pace set; `app_failed` when the app waits as after a failure
    /// (no answer, a server error or 403) rather than not at all (429, a refused key).
    Stop {
        app_failed: bool,
    },
}
/// What a lookup ended with.
enum Lookup {
    /// The image's address.
    Found(String),
    /// SteamGridDB has no cover for it, or refused its request.
    Missing,
    Stop {
        app_failed: bool,
    },
}

/// One pass, if one is due: at once when the library or settings changed or a pause ended, else
/// every [`PASS_EVERY`] while apps miss covers.
pub(crate) fn watch_with(h: &Shared, http: &dyn Http) {
    let Some(key) = api_key(h) else {
        // No key: nothing is refused any more.
        if with_pace(h, |pace| pace.refused.take()).is_some() {
            withdraw_refusal(h);
        }
        return;
    };
    let version = h.config_version.load(Ordering::Acquire);
    let mut other_key = false;
    let go = with_pace(h, |pace| {
        let now = Instant::now();
        if pace.paused_until.is_some_and(|until| now < until) {
            return false;
        }
        match pace.refused {
            Some(refused) if refused == fingerprint(&key) => return false,
            Some(_) => {
                pace.refused = None;
                other_key = true;
            }
            None => {}
        }
        let every = if pace.more { MORE_AFTER } else { PASS_EVERY };
        let due = pace.resume
            || pace.last_pass.is_none_or(|at| at.elapsed() >= every)
            || pace.last_version != version;
        if due {
            pace.last_pass = Some(now);
            pace.more = false;
            pace.resume = false;
        }
        due
    });
    if other_key {
        withdraw_refusal(h);
    }
    if !go {
        return;
    }
    let folder = h.directory.join("covers");
    let mut asked = 0;
    let mut more = false;
    let mut own_changes = 0;
    for app in wanted(h) {
        let png = folder.join(format!("sgdb_{}.png", app.uuid));
        if png.is_file() {
            own_changes += u64::from(assign(h, &app.uuid, &png));
            continue;
        }
        if waiting(&folder, &app.uuid) {
            continue;
        }
        if asked >= PER_PASS {
            more = true;
            continue;
        }
        // A key changed or removed while the pass ran ends it: the next pass uses the new one.
        if api_key(h).as_deref() != Some(key.as_str()) {
            break;
        }
        asked += 1;
        match lookup(h, http, &key, &app) {
            Lookup::Found(url) => match save(http, &folder, &app.uuid, &url) {
                Ok(()) => {
                    tracing::info!(app = %app.name, "SteamGridDB cover added");
                    own_changes += u64::from(assign(h, &app.uuid, &png));
                }
                Err(error) => {
                    tracing::info!(error = %format!("{error:#}"), app = %app.name, "SteamGridDB cover unavailable");
                    failed(&folder, &app.uuid);
                }
            },
            Lookup::Missing => {
                tracing::debug!(app = %app.name, "SteamGridDB has no cover for this app");
                failed(&folder, &app.uuid);
            }
            Lookup::Stop { app_failed } => {
                if app_failed {
                    failed(&folder, &app.uuid);
                }
                break;
            }
        }
    }
    // The covers this pass gave changed the library: only someone else's change, also one made
    // while the pass ran, starts the next pass early.
    with_pace(h, |pace| {
        pace.last_version = version + own_changes;
        pace.more = more;
    });
}

/// The steps for one app: its Steam app ID's grids when it has one, else (or when SteamGridDB has
/// none by that ID) the game its name finds, then that game's grids.
fn lookup(h: &Shared, http: &dyn Http, key: &str, app: &Wanted) -> Lookup {
    if let Some(appid) = app.steam {
        match ask(h, http, key, &app.name, &sgdb::steam_grids_url(appid)) {
            Answer::Body(body) => {
                if let Some(url) = sgdb::best_grid(&body) {
                    return Lookup::Found(url);
                }
            }
            Answer::NotFound | Answer::Refused => {}
            Answer::Stop { app_failed } => return Lookup::Stop { app_failed },
        }
    }
    let Some(search) = sgdb::search_url(&app.name) else {
        return Lookup::Missing;
    };
    let game = match ask(h, http, key, &app.name, &search) {
        Answer::Body(body) => sgdb::matching_game(&body, &app.name),
        Answer::NotFound | Answer::Refused => None,
        Answer::Stop { app_failed } => return Lookup::Stop { app_failed },
    };
    let Some(game) = game else {
        return Lookup::Missing;
    };
    match ask(h, http, key, &app.name, &sgdb::game_grids_url(game)) {
        Answer::Body(body) => sgdb::best_grid(&body).map_or(Lookup::Missing, Lookup::Found),
        Answer::NotFound | Answer::Refused => Lookup::Missing,
        Answer::Stop { app_failed } => Lookup::Stop { app_failed },
    }
}

/// One API request for the app `name`, after the gap, with the pace set by its answer.
fn ask(h: &Shared, http: &dyn Http, key: &str, name: &str, url: &str) -> Answer {
    if !GAP.is_zero() {
        std::thread::sleep(GAP);
    }
    let pause = |wait: Duration| {
        with_pace(h, |pace| {
            pace.paused_until = Some(Instant::now() + wait);
            pace.resume = true;
        })
    };
    let reply = match http.get(url, Some(key)) {
        Ok(reply) => reply,
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), app = %name, wait_seconds = UNREACHABLE_WAIT.as_secs(), "SteamGridDB did not answer; covers wait");
            pause(UNREACHABLE_WAIT);
            return Answer::Stop { app_failed: true };
        }
    };
    match reply.status {
        200..=299 => {
            with_pace(h, |pace| pace.backoff = Duration::ZERO);
            if h.warnings
                .snapshot()
                .iter()
                .any(|warning| warning.code == REFUSED_WARNING)
            {
                withdraw_refusal(h);
            }
            Answer::Body(reply.body)
        }
        404 => Answer::NotFound,
        401 => {
            tracing::warn!(
                app = %name,
                "SteamGridDB refused the API key; covers wait until it is changed in Settings"
            );
            with_pace(h, |pace| pace.refused = Some(fingerprint(key)));
            h.warnings.set(
                REFUSED_WARNING,
                "SteamGridDB refused the API key, so apps get no new covers. Paste a current key under Settings › Games › Covers.",
            );
            h.metadata.lock().unwrap().take();
            Answer::Stop { app_failed: false }
        }
        429 => {
            let wait = with_pace(h, |pace| {
                pace.backoff = sgdb::backoff(reply.retry_after.as_deref(), pace.backoff);
                pace.paused_until = Some(Instant::now() + pace.backoff);
                pace.resume = true;
                pace.backoff
            });
            tracing::info!(
                app = %name,
                wait_seconds = wait.as_secs(),
                "SteamGridDB asked for fewer requests; covers wait"
            );
            Answer::Stop { app_failed: false }
        }
        status @ 400..=499 if status != 403 => {
            tracing::info!(
                app = %name,
                status,
                "SteamGridDB refused the request for this app; it waits, the others go on"
            );
            Answer::Refused
        }
        status => {
            tracing::warn!(
                app = %name,
                status,
                wait_seconds = UNREACHABLE_WAIT.as_secs(),
                "SteamGridDB answered with an error; covers wait"
            );
            pause(UNREACHABLE_WAIT);
            Answer::Stop { app_failed: true }
        }
    }
}

/// The grid at `url` (never sent the key) as `sgdb_<uuid>.png` in `folder`, when it is a PNG or
/// JPEG image by both its type and its bytes.
fn save(http: &dyn Http, folder: &Path, uuid: &str, url: &str) -> Result<()> {
    let reply = http.get(url, None)?;
    if !(200..300).contains(&reply.status) {
        bail!("the image server answered {}", reply.status);
    }
    if sgdb::image_type(reply.content_type.as_deref(), &reply.body).is_none() {
        bail!("the image is not a PNG or JPEG");
    }
    std::fs::create_dir_all(folder)?;
    let download = folder.join(format!("sgdb_{uuid}.download"));
    butterpollo_session::state::atomic_write(&download, &reply.body)?;
    let converted = butterpollo_win_graphics::image::to_png(
        &download,
        &folder.join(format!("sgdb_{uuid}.png")),
    );
    let _ = std::fs::remove_file(&download);
    converted?;
    let _ = std::fs::remove_file(folder.join(format!("sgdb_{uuid}.failed")));
    Ok(())
}

/// Whether the app's last lookups failed recently enough to leave it alone, with the waits of
/// Steam's store portraits: ten minutes, an hour, then a day.
fn waiting(folder: &Path, uuid: &str) -> bool {
    let marker = folder.join(format!("sgdb_{uuid}.failed"));
    let failures =
        std::fs::read(&marker).map_or(0, |bytes| crate::steam::portrait_failures(&bytes));
    failures > 0
        && std::fs::metadata(&marker)
            .and_then(|meta| meta.modified())
            .is_ok_and(|at| {
                at.elapsed()
                    .is_ok_and(|age| age < crate::steam::portrait_retry_after(failures))
            })
}
fn failed(folder: &Path, uuid: &str) {
    let marker = folder.join(format!("sgdb_{uuid}.failed"));
    let failures =
        std::fs::read(&marker).map_or(0, |bytes| crate::steam::portrait_failures(&bytes));
    let _ = std::fs::create_dir_all(folder);
    let _ = std::fs::write(&marker, failures.saturating_add(1).to_string());
}

/// `png` becomes the app's cover, marked as SteamGridDB's, unless the app got one meanwhile or is
/// gone. True when the library changed.
fn assign(h: &Shared, uuid: &str, png: &Path) -> bool {
    let image = png.to_string_lossy().replace('\\', "/");
    let result = crate::steam::update_apps(h, |apps| {
        let Some(app) = apps
            .iter_mut()
            .find(|app| app.get("uuid").and_then(Value::as_str) == Some(uuid))
        else {
            return false;
        };
        if !wants_cover(app) {
            return false;
        }
        app["image-path"] = json!(image);
        app[sgdb::COVER_FIELD] = json!(image);
        true
    });
    result.unwrap_or_else(|error| {
        tracing::warn!(error = %format!("{error:#}"), "the SteamGridDB cover could not be saved to the app");
        false
    })
}

#[cfg(test)]
mod tests;
