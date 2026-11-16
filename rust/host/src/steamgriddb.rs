//! Covers from SteamGridDB for apps that have none, with the API key the user saved
//! (`steamgriddb_api_key`). What to ask and how to read the answers is decided in
//! `butterpollo_core::steamgriddb`; this module asks, from the housekeeping loop and never from a
//! request, and saves each cover into the profile's covers folder as the app's `image-path`, the
//! file `/appasset` and every client's library read.
//!
//! A cover the user set, or one a launcher sync placed, is never replaced: only an app whose
//! `image-path` is empty or Steam's placeholder gets one. A 429 pauses every request for as long
//! as SteamGridDB asks (or a doubling wait), a refused key pauses them until the key changes, and
//! an app SteamGridDB has no cover for is asked about again after ten minutes, an hour, then a
//! day. The key goes only into the API requests' `Authorization` header, never to the image
//! servers and never into a log line.

use crate::state::Shared;
use anyhow::{Result, bail};
use butterpollo_core::steamgriddb as sgdb;
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
            let response = request
                .send()
                .await
                .map_err(|error| anyhow::anyhow!("{}", error.without_url()))?;
            let status = response.status().as_u16();
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            if response.content_length().is_some_and(|n| n > LIMIT) {
                bail!("the answer is too large");
            }
            let body = response
                .bytes()
                .await
                .map_err(|error| anyhow::anyhow!("{}", error.without_url()))?;
            if body.len() as u64 > LIMIT {
                bail!("the answer is too large");
            }
            Ok(Reply {
                status,
                retry_after,
                body: body.to_vec(),
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
/// The pause after a request that got no answer or a server error.
const UNREACHABLE_WAIT: Duration = Duration::from_secs(5 * 60);

/// One host's requests: when they may go again and why not.
#[derive(Default)]
struct Pace {
    /// No request before this (a 429 or an unreachable server).
    paused_until: Option<Instant>,
    /// The last 429's wait, doubled by the next one without `Retry-After`.
    backoff: Duration,
    /// The fingerprint of the key SteamGridDB refused: nothing is asked until the key changes.
    refused: Option<u64>,
    last_pass: Option<Instant>,
    /// The library and settings as the last pass left them, its own covers included.
    last_version: u64,
    /// The last pass left apps for the next: it comes after [`MORE_AFTER`].
    more: bool,
}
/// By profile directory, so each host (and each test host) keeps its own.
static PACES: LazyLock<Mutex<HashMap<PathBuf, Pace>>> = LazyLock::new(Default::default);

fn fingerprint(key: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}

/// The housekeeping loop's look for covers, on a blocking thread of its own.
pub fn watch(h: &Shared) {
    watch_with(h, &Web);
}

/// An app that may get a cover: no `image-path`, or Steam's placeholder.
struct Wanted {
    uuid: String,
    name: String,
    steam: Option<u32>,
}
fn wants_cover(app: &Value) -> bool {
    app.get("image-path")
        .and_then(Value::as_str)
        .is_none_or(|image| {
            let image = image.trim();
            image.is_empty() || image == butterpollo_core::steam::FALLBACK_IMAGE
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
                steam: butterpollo_core::steam::game_of(&value),
            })
        })
        .collect()
}

/// What a lookup ended with.
enum Lookup {
    /// The image's address.
    Found(String),
    /// SteamGridDB has no cover for it.
    Missing,
    /// The pass stops: the reason is logged and the pace set.
    Stop,
}

/// One pass, if one is due: at once when the library or settings changed, else every
/// [`PASS_EVERY`] while apps miss covers.
pub(crate) fn watch_with(h: &Shared, http: &dyn Http) {
    let key = h
        .config
        .read()
        .unwrap()
        .steamgriddb_api_key()
        .map(str::to_owned);
    let Some(key) = key else {
        return;
    };
    let version = h.config_version.load(Ordering::Acquire);
    {
        let mut paces = PACES.lock().unwrap_or_else(PoisonError::into_inner);
        let pace = paces.entry(h.directory.clone()).or_default();
        let now = Instant::now();
        if pace.paused_until.is_some_and(|until| now < until) {
            return;
        }
        if pace.refused == Some(fingerprint(&key)) {
            return;
        }
        let every = if pace.more { MORE_AFTER } else { PASS_EVERY };
        let due =
            pace.last_pass.is_none_or(|at| at.elapsed() >= every) || pace.last_version != version;
        if !due {
            return;
        }
        pace.last_pass = Some(now);
        pace.more = false;
    }
    let folder = h.directory.join("covers");
    let mut asked = 0;
    let mut more = false;
    for app in wanted(h) {
        let png = folder.join(format!("sgdb_{}.png", app.uuid));
        if png.is_file() {
            assign(h, &app.uuid, &png);
            continue;
        }
        if waiting(&folder, &app.uuid) {
            continue;
        }
        if asked >= PER_PASS {
            more = true;
            continue;
        }
        asked += 1;
        match lookup(h, http, &key, &app) {
            Lookup::Found(url) => match save(http, &folder, &app.uuid, &url) {
                Ok(()) => {
                    tracing::info!(app = %app.name, "SteamGridDB cover added");
                    assign(h, &app.uuid, &png);
                }
                Err(error) => {
                    tracing::debug!(error = %format!("{error:#}"), app = %app.name, "SteamGridDB cover unavailable");
                    failed(&folder, &app.uuid);
                }
            },
            Lookup::Missing => {
                tracing::debug!(app = %app.name, "SteamGridDB has no cover for this app");
                failed(&folder, &app.uuid);
            }
            Lookup::Stop => break,
        }
    }
    // The covers this pass gave changed the library: only someone else's change starts the next
    // pass early.
    let mut paces = PACES.lock().unwrap_or_else(PoisonError::into_inner);
    let pace = paces.entry(h.directory.clone()).or_default();
    pace.last_version = h.config_version.load(Ordering::Acquire);
    pace.more = more;
}

/// The steps for one app: its Steam app ID's grids when it has one, else (or when SteamGridDB has
/// none by that ID) the game its name finds, then that game's grids.
fn lookup(h: &Shared, http: &dyn Http, key: &str, app: &Wanted) -> Lookup {
    if let Some(appid) = app.steam {
        match ask(h, http, key, &sgdb::steam_grids_url(appid)) {
            Some(Some(body)) => {
                if let Some(url) = sgdb::best_grid(&body) {
                    return Lookup::Found(url);
                }
            }
            Some(None) => {}
            None => return Lookup::Stop,
        }
    }
    let Some(search) = sgdb::search_url(&app.name) else {
        return Lookup::Missing;
    };
    let game = match ask(h, http, key, &search) {
        Some(Some(body)) => sgdb::matching_game(&body, &app.name),
        Some(None) => None,
        None => return Lookup::Stop,
    };
    let Some(game) = game else {
        return Lookup::Missing;
    };
    match ask(h, http, key, &sgdb::game_grids_url(game)) {
        Some(Some(body)) => sgdb::best_grid(&body).map_or(Lookup::Missing, Lookup::Found),
        Some(None) => Lookup::Missing,
        None => Lookup::Stop,
    }
}

/// One API request after the gap: `Some(Some(body))` for an answer, `Some(None)` for "not
/// found", `None` when the pass must stop (429, refused key, no answer), with the pace set.
fn ask(h: &Shared, http: &dyn Http, key: &str, url: &str) -> Option<Option<Vec<u8>>> {
    if !GAP.is_zero() {
        std::thread::sleep(GAP);
    }
    let pause = |wait: Duration| {
        let mut paces = PACES.lock().unwrap_or_else(PoisonError::into_inner);
        paces.entry(h.directory.clone()).or_default().paused_until = Some(Instant::now() + wait);
    };
    let reply = match http.get(url, Some(key)) {
        Ok(reply) => reply,
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), wait_seconds = UNREACHABLE_WAIT.as_secs(), "SteamGridDB did not answer; covers wait");
            pause(UNREACHABLE_WAIT);
            return None;
        }
    };
    match reply.status {
        200..=299 => {
            let mut paces = PACES.lock().unwrap_or_else(PoisonError::into_inner);
            paces.entry(h.directory.clone()).or_default().backoff = Duration::ZERO;
            Some(Some(reply.body))
        }
        404 => Some(None),
        401 | 403 => {
            tracing::warn!(
                status = reply.status,
                "SteamGridDB refused the API key; covers wait until it is changed in Settings"
            );
            let mut paces = PACES.lock().unwrap_or_else(PoisonError::into_inner);
            paces.entry(h.directory.clone()).or_default().refused = Some(fingerprint(key));
            None
        }
        429 => {
            let wait = {
                let mut paces = PACES.lock().unwrap_or_else(PoisonError::into_inner);
                let pace = paces.entry(h.directory.clone()).or_default();
                pace.backoff = sgdb::backoff(reply.retry_after.as_deref(), pace.backoff);
                pace.paused_until = Some(Instant::now() + pace.backoff);
                pace.backoff
            };
            tracing::info!(
                wait_seconds = wait.as_secs(),
                "SteamGridDB asked for fewer requests; covers wait"
            );
            None
        }
        status => {
            tracing::warn!(
                status,
                wait_seconds = UNREACHABLE_WAIT.as_secs(),
                "SteamGridDB answered with an error; covers wait"
            );
            pause(UNREACHABLE_WAIT);
            None
        }
    }
}

/// The grid at `url` (never sent the key) as `sgdb_<uuid>.png` in `folder`.
fn save(http: &dyn Http, folder: &Path, uuid: &str, url: &str) -> Result<()> {
    let reply = http.get(url, None)?;
    if !(200..300).contains(&reply.status) {
        bail!("the image server answered {}", reply.status);
    }
    std::fs::create_dir_all(folder)?;
    let download = folder.join(format!("sgdb_{uuid}.download"));
    butterpollo_core::state::atomic_write(&download, &reply.body)?;
    let converted =
        butterpollo_windows::image::to_png(&download, &folder.join(format!("sgdb_{uuid}.png")));
    let _ = std::fs::remove_file(&download);
    converted?;
    let _ = std::fs::remove_file(folder.join(format!("sgdb_{uuid}.failed")));
    Ok(())
}

/// Whether the app's last lookups found nothing recently enough to leave it alone, with the waits
/// of Steam's store portraits: ten minutes, an hour, then a day.
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

/// `png` becomes the app's cover, unless the app got one meanwhile or is gone.
fn assign(h: &Shared, uuid: &str, png: &Path) {
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
        true
    });
    if let Err(error) = result {
        tracing::warn!(error = %format!("{error:#}"), "the SteamGridDB cover could not be saved to the app");
    }
}

#[cfg(test)]
mod tests;
