use super::*;
use crate::state::test_support::Fixture;

const KEY: &str = "0123456789abcdef0123456789abcdef";

/// An uncompressed 600x900 bitmap, which the host converts without a codec or GPU.
fn bitmap() -> Vec<u8> {
    let mut bmp = vec![0u8; 54 + 600 * 900 * 3];
    let size = bmp.len() as u32;
    bmp[..2].copy_from_slice(b"BM");
    bmp[2..6].copy_from_slice(&size.to_le_bytes());
    bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
    bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
    bmp[18..22].copy_from_slice(&600u32.to_le_bytes());
    bmp[22..26].copy_from_slice(&900u32.to_le_bytes());
    bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
    bmp[28..30].copy_from_slice(&24u16.to_le_bytes());
    bmp
}

/// How a fake answers a request, by its address.
type Answer = Box<dyn Fn(&str) -> Result<Reply> + Send + Sync>;

/// Answers by the request's path, and remembers each request with the key it carried.
struct Fake {
    asked: Mutex<Vec<(String, Option<String>)>>,
    answer: Answer,
}
impl Fake {
    fn new(answer: impl Fn(&str) -> Result<Reply> + Send + Sync + 'static) -> Self {
        Self {
            asked: Mutex::new(Vec::new()),
            answer: Box::new(answer),
        }
    }
    fn paths(&self) -> Vec<String> {
        self.asked
            .lock()
            .unwrap()
            .iter()
            .map(|(url, _)| {
                let url = url.split('?').next().unwrap();
                url.strip_prefix(sgdb::API).unwrap_or(url).to_owned()
            })
            .collect()
    }
}
impl Http for Fake {
    fn get(&self, url: &str, key: Option<&str>) -> Result<Reply> {
        self.asked
            .lock()
            .unwrap()
            .push((url.to_owned(), key.map(str::to_owned)));
        (self.answer)(url)
    }
}
fn reply(status: u16, body: &str) -> Result<Reply> {
    Ok(Reply {
        status,
        retry_after: None,
        body: body.as_bytes().to_vec(),
    })
}
const GRID: &str = r#"{"success":true,"data":[{"id":7,"score":5,"url":"https://cdn2.steamgriddb.com/grid/x.png"}]}"#;
const NOT_FOUND: &str = r#"{"success":false,"errors":["Game not found"]}"#;

/// Answers every lookup with a grid and the grid with a bitmap; `steam` decides whether the
/// Steam app ID finds one.
fn steamgriddb(steam: bool) -> Fake {
    Fake::new(move |url| {
        if url.starts_with("https://cdn2.steamgriddb.com/") {
            return Ok(Reply {
                status: 200,
                retry_after: None,
                body: bitmap(),
            });
        }
        if url.contains("/grids/steam/") {
            return if steam {
                reply(200, GRID)
            } else {
                reply(404, NOT_FOUND)
            };
        }
        if url.contains("/search/autocomplete/") {
            return reply(
                200,
                r#"{"success":true,"data":[{"id":99,"name":"Hollow Knight"}]}"#,
            );
        }
        if url.contains("/grids/game/99") {
            return reply(200, GRID);
        }
        reply(404, NOT_FOUND)
    })
}

fn with_key(f: &Fixture) {
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert(sgdb::KEY_SETTING.into(), KEY.into());
    f.host.config_version.fetch_add(1, Ordering::Release);
}
fn add(f: &Fixture, app: Value) {
    crate::steam::update_apps(&f.host, |apps| {
        apps.push(app);
        true
    })
    .unwrap();
}
fn image(f: &Fixture, uuid: &str) -> String {
    f.host
        .apps
        .read()
        .unwrap()
        .iter()
        .find(|app| app.extra.get("uuid").and_then(Value::as_str) == Some(uuid))
        .and_then(|app| app.extra.get("image-path")?.as_str().map(str::to_owned))
        .unwrap_or_default()
}
/// The next tick's pass is due at once, as after a change to the library.
fn changed(f: &Fixture) {
    f.host.config_version.fetch_add(1, Ordering::Release);
}

#[test]
fn nothing_is_asked_without_a_key() {
    let f = Fixture::new();
    add(&f, json!({"name": "Hollow Knight", "uuid": "hk"}));
    let fake = steamgriddb(true);
    watch_with(&f.host, &fake);
    assert!(fake.paths().is_empty());
    assert_eq!(image(&f, "hk"), "");
}

#[test]
fn a_steam_game_is_looked_up_by_its_app_id_first_and_by_name_when_that_has_none() {
    let f = Fixture::new();
    with_key(&f);
    add(
        &f,
        json!({"name": "Hollow Knight", "uuid": "by-id", "cmd": "steam://rungameid/367520"}),
    );
    let fake = steamgriddb(true);
    watch_with(&f.host, &fake);
    assert_eq!(
        fake.paths(),
        [
            "/grids/steam/367520",
            "https://cdn2.steamgriddb.com/grid/x.png"
        ]
    );
    let cover = image(&f, "by-id");
    assert!(cover.ends_with("covers/sgdb_by-id.png"), "{cover}");
    assert_eq!(
        &std::fs::read(&cover).unwrap()[..8],
        b"\x89PNG\r\n\x1a\n",
        "converted"
    );

    // Steam's own placeholder is no cover; SteamGridDB has nothing by this app ID.
    add(
        &f,
        json!({"name": "Hollow Knight", "uuid": "by-name", "cmd": "steam://rungameid/1",
               "image-path": butterpollo_core::steam::FALLBACK_IMAGE}),
    );
    let fake = steamgriddb(false);
    watch_with(&f.host, &fake);
    assert_eq!(
        fake.paths(),
        [
            "/grids/steam/1",
            "/search/autocomplete/Hollow%20Knight",
            "/grids/game/99",
            "https://cdn2.steamgriddb.com/grid/x.png"
        ]
    );
    assert!(image(&f, "by-name").ends_with("covers/sgdb_by-name.png"));
}

#[test]
fn an_app_without_a_steam_id_is_found_by_name() {
    let f = Fixture::new();
    with_key(&f);
    add(
        &f,
        json!({"name": "Hollow Knight", "uuid": "named", "cmd": "C:/Games/hk.exe"}),
    );
    let fake = steamgriddb(true);
    watch_with(&f.host, &fake);
    assert_eq!(
        fake.paths(),
        [
            "/search/autocomplete/Hollow%20Knight",
            "/grids/game/99",
            "https://cdn2.steamgriddb.com/grid/x.png"
        ]
    );
    assert!(image(&f, "named").ends_with("covers/sgdb_named.png"));
}

#[test]
fn a_cover_the_user_or_a_launcher_set_is_never_replaced() {
    let f = Fixture::new();
    with_key(&f);
    add(
        &f,
        json!({"name": "Hollow Knight", "uuid": "mine", "cmd": "steam://rungameid/367520",
               "image-path": "D:/covers/mine.png"}),
    );
    let fake = steamgriddb(true);
    watch_with(&f.host, &fake);
    assert!(
        fake.paths().is_empty(),
        "the fixture's own apps have covers too: {:?}",
        fake.paths()
    );
    assert_eq!(image(&f, "mine"), "D:/covers/mine.png");

    // A cover set while the lookup ran wins over the one it brings.
    add(&f, json!({"name": "Late", "uuid": "late"}));
    crate::steam::update_apps(&f.host, |apps| {
        let app = apps.iter_mut().find(|app| app["uuid"] == "late").unwrap();
        app["image-path"] = json!("D:/covers/late.png");
        true
    })
    .unwrap();
    assign(
        &f.host,
        "late",
        &f.host.directory.join("covers").join("sgdb_late.png"),
    );
    assert_eq!(image(&f, "late"), "D:/covers/late.png");
}

#[test]
fn a_429_stops_the_pass_and_nothing_is_asked_until_the_wait_is_over() {
    let f = Fixture::new();
    with_key(&f);
    add(&f, json!({"name": "One", "uuid": "one"}));
    add(&f, json!({"name": "Two", "uuid": "two"}));
    let fake = Fake::new(|_| {
        Ok(Reply {
            status: 429,
            retry_after: Some("120".into()),
            body: Vec::new(),
        })
    });
    watch_with(&f.host, &fake);
    assert_eq!(fake.paths().len(), 1, "no retry storm: {:?}", fake.paths());
    {
        let paces = PACES.lock().unwrap();
        let pace = &paces[&f.host.directory];
        assert_eq!(pace.backoff, Duration::from_secs(120));
        let left = pace.paused_until.unwrap() - Instant::now();
        assert!(left > Duration::from_secs(110), "{left:?}");
    }
    // A due pass while paused asks nothing, and the apps are not marked as missing covers.
    changed(&f);
    watch_with(&f.host, &fake);
    assert_eq!(fake.paths().len(), 1);
    assert!(!f.host.directory.join("covers/sgdb_one.failed").exists());

    // After the wait the next pass asks again, and without Retry-After the wait doubles.
    PACES
        .lock()
        .unwrap()
        .get_mut(&f.host.directory)
        .unwrap()
        .paused_until = Some(Instant::now());
    let doubling = Fake::new(|_| reply(429, ""));
    changed(&f);
    watch_with(&f.host, &doubling);
    assert_eq!(doubling.paths().len(), 1);
    assert_eq!(
        PACES.lock().unwrap()[&f.host.directory].backoff,
        Duration::from_secs(240)
    );
}

#[test]
fn an_app_steamgriddb_has_nothing_for_waits_and_a_refused_key_stops_every_request() {
    let f = Fixture::new();
    with_key(&f);
    add(&f, json!({"name": "Nothing Like It", "uuid": "none"}));
    let empty = Fake::new(|_| reply(200, r#"{"success":true,"data":[]}"#));
    watch_with(&f.host, &empty);
    assert_eq!(empty.paths(), ["/search/autocomplete/Nothing%20Like%20It"]);
    assert!(waiting(&f.host.directory.join("covers"), "none"));
    changed(&f);
    watch_with(&f.host, &empty);
    assert_eq!(empty.paths().len(), 1, "it waits ten minutes");

    add(&f, json!({"name": "Other", "uuid": "other"}));
    let refused = Fake::new(|_| reply(401, NOT_FOUND));
    watch_with(&f.host, &refused);
    assert_eq!(refused.paths().len(), 1);
    changed(&f);
    watch_with(&f.host, &refused);
    assert_eq!(refused.paths().len(), 1, "nothing until the key changes");
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert(sgdb::KEY_SETTING.into(), "fedcba9876543210".into());
    changed(&f);
    watch_with(&f.host, &refused);
    assert_eq!(refused.paths().len(), 2, "a new key is tried");
}

#[test]
fn the_key_goes_only_to_the_api_and_never_into_the_log() {
    let f = Fixture::new();
    with_key(&f);
    add(
        &f,
        json!({"name": "Hollow Knight", "uuid": "logged", "cmd": "steam://rungameid/5"}),
    );
    add(&f, json!({"name": "Broken", "uuid": "broken"}));
    let path = f.host.directory.join("sgdb.log");
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(Mutex::new(std::fs::File::create(&path).unwrap()))
        .finish();
    let fake = Fake::new(|url| {
        if url.contains("cdn2") {
            return Ok(Reply {
                status: 200,
                retry_after: None,
                body: bitmap(),
            });
        }
        if url.contains("/grids/steam/") {
            return reply(200, GRID);
        }
        // Every way a request can fail, each logged.
        anyhow::bail!("connection refused")
    });
    crate::test_log::capture_during(subscriber, || {
        watch_with(&f.host, &fake);
        PACES.lock().unwrap().remove(&f.host.directory);
        changed(&f);
        watch_with(&f.host, &Fake::new(|_| reply(500, "")));
        PACES.lock().unwrap().remove(&f.host.directory);
        changed(&f);
        watch_with(&f.host, &Fake::new(|_| reply(429, "")));
        PACES.lock().unwrap().remove(&f.host.directory);
        changed(&f);
        watch_with(&f.host, &Fake::new(|_| reply(403, "")));
    });
    for (url, key) in fake.asked.lock().unwrap().iter() {
        if url.starts_with(sgdb::API) {
            assert_eq!(key.as_deref(), Some(KEY), "{url}");
        } else {
            assert_eq!(key, &None, "the image server never gets the key: {url}");
        }
        assert!(!url.contains(KEY));
    }
    let log = std::fs::read_to_string(&path).unwrap();
    assert!(log.contains("SteamGridDB did not answer"), "{log}");
    assert!(log.contains("asked for fewer requests"), "{log}");
    assert!(log.contains("refused the API key"), "{log}");
    assert!(!log.contains(KEY), "{log}");
}
