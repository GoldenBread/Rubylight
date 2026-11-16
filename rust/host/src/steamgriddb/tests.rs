use super::*;
use crate::state::test_support::Fixture;
use std::sync::OnceLock;

const KEY: &str = "0123456789abcdef0123456789abcdef";

/// An uncompressed 600x900 bitmap.
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
/// A real 600x900 PNG, as SteamGridDB serves grids: the bitmap converted once.
fn png() -> Vec<u8> {
    static PNG: OnceLock<Vec<u8>> = OnceLock::new();
    PNG.get_or_init(|| {
        let folder = std::env::temp_dir().join(format!("sgdb-png-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let (bmp, png) = (folder.join("grid.bmp"), folder.join("grid.png"));
        std::fs::write(&bmp, bitmap()).unwrap();
        butterpollo_windows::image::to_png(&bmp, &png).unwrap();
        let bytes = std::fs::read(&png).unwrap();
        let _ = std::fs::remove_dir_all(&folder);
        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        bytes
    })
    .clone()
}

/// How a fake answers a request, by its address.
type Responder = Box<dyn Fn(&str) -> Result<Reply> + Send + Sync>;

/// Answers by the request's address, and remembers each request with the key it carried.
struct Fake {
    asked: Mutex<Vec<(String, Option<String>)>>,
    answer: Responder,
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
        content_type: Some("application/json".into()),
        body: body.as_bytes().to_vec(),
    })
}
fn image_reply(content_type: &str, body: Vec<u8>) -> Result<Reply> {
    Ok(Reply {
        status: 200,
        retry_after: None,
        content_type: Some(content_type.into()),
        body,
    })
}
const GRID: &str = r#"{"success":true,"data":[{"id":7,"score":5,"url":"https://cdn2.steamgriddb.com/grid/x.png"}]}"#;
const NOT_FOUND: &str = r#"{"success":false,"errors":["Game not found"]}"#;
const CDN: &str = "https://cdn2.steamgriddb.com/";

/// SteamGridDB as it answers: every name finds the game of that name (id 99), every game and
/// Steam app ID has a grid unless `steam` is false, and the grid is a PNG.
fn steamgriddb(steam: bool) -> Fake {
    Fake::new(move |url| {
        if url.starts_with(CDN) {
            return image_reply("image/png", png());
        }
        if url.contains("/grids/steam/") {
            return if steam {
                reply(200, GRID)
            } else {
                reply(404, NOT_FOUND)
            };
        }
        if let Some(name) = url.split("/search/autocomplete/").nth(1) {
            let name = name.replace("%20", " ");
            return reply(
                200,
                &json!({"success": true, "data": [{"id": 99, "name": name}]}).to_string(),
            );
        }
        if url.contains("/grids/game/99") {
            return reply(200, GRID);
        }
        reply(404, NOT_FOUND)
    })
}

fn with_key(f: &Fixture) {
    store_key(&f.host, Some(KEY)).unwrap();
    changed(f);
}
fn add(f: &Fixture, app: Value) {
    crate::steam::update_apps(&f.host, |apps| {
        apps.push(app);
        true
    })
    .unwrap();
}
fn app(f: &Fixture, uuid: &str) -> Value {
    f.host
        .apps
        .read()
        .unwrap()
        .iter()
        .find(|app| app.extra.get("uuid").and_then(Value::as_str) == Some(uuid))
        .map(|app| serde_json::to_value(app).unwrap())
        .unwrap()
}
fn image(f: &Fixture, uuid: &str) -> String {
    app(f, uuid)["image-path"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}
/// The next tick's pass is due at once, as after a change to the library.
fn changed(f: &Fixture) {
    f.host.config_version.fetch_add(1, Ordering::Release);
}
/// The pause is over, as if its time had passed.
fn pause_over(f: &Fixture) {
    with_pace(&f.host, |pace| pace.paused_until = Some(Instant::now()));
}
fn failed_marker(f: &Fixture, uuid: &str) -> PathBuf {
    f.host
        .directory
        .join("covers")
        .join(format!("sgdb_{uuid}.failed"))
}
fn warned(f: &Fixture) -> bool {
    f.host
        .warnings
        .snapshot()
        .iter()
        .any(|warning| warning.code == REFUSED_WARNING)
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
    assert_eq!(app(&f, "by-id")[sgdb::COVER_FIELD], cover, "marked as ours");

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
    assert!(!assign(
        &f.host,
        "late",
        &f.host.directory.join("covers").join("sgdb_late.png"),
    ));
    assert_eq!(image(&f, "late"), "D:/covers/late.png");
}

#[test]
fn a_cover_the_user_cleared_is_not_put_back_and_a_launchers_replaces_ours() {
    let f = Fixture::new();
    with_key(&f);
    add(&f, json!({"name": "Hollow Knight", "uuid": "cleared"}));
    watch_with(&f.host, &steamgriddb(true));
    assert!(image(&f, "cleared").ends_with("sgdb_cleared.png"));
    crate::steam::update_apps(&f.host, |apps| {
        let app = apps
            .iter_mut()
            .find(|app| app["uuid"] == "cleared")
            .unwrap();
        app["image-path"] = json!("");
        true
    })
    .unwrap();
    let fake = steamgriddb(true);
    watch_with(&f.host, &fake);
    assert!(fake.paths().is_empty(), "{:?}", fake.paths());
    assert_eq!(
        image(&f, "cleared"),
        "",
        "the cached cover is not put back either"
    );

    // A launcher's cover replaces a cover from SteamGridDB, and the mark goes with it.
    let ours = "C:/profile/covers/sgdb_ubi.png";
    let mut apps = vec![
        json!({"name": "Mirage", "uuid": "ubi", "launcher-source": "ubisoft",
        "launcher-id": "13504", "cmd": "uplay://launch/13504",
        "image-path": ours, sgdb::COVER_FIELD: ours}),
    ];
    let mut game =
        butterpollo_core::library::ubisoft("13504", Path::new("F:/Games/Mirage"), "Mirage")
            .unwrap();
    game.cover = Some(PathBuf::from("C:/profile/covers/launcher_ubi.png"));
    let change = butterpollo_core::library::reconcile(
        &mut apps,
        &[game],
        &butterpollo_core::library::Settings::default(),
        &butterpollo_core::library::Source::ALL,
        &[],
    );
    assert_eq!(change.covered, 1);
    assert_eq!(apps[0]["image-path"], "C:/profile/covers/launcher_ubi.png");
    assert!(apps[0].get(sgdb::COVER_FIELD).is_none());
}

#[test]
fn a_429_stops_the_pass_and_the_next_comes_when_its_wait_is_over() {
    let f = Fixture::new();
    with_key(&f);
    add(&f, json!({"name": "One", "uuid": "one"}));
    add(&f, json!({"name": "Two", "uuid": "two"}));
    let fake = Fake::new(|_| {
        Ok(Reply {
            status: 429,
            retry_after: Some("120".into()),
            content_type: None,
            body: Vec::new(),
        })
    });
    watch_with(&f.host, &fake);
    assert_eq!(fake.paths().len(), 1, "no retry storm: {:?}", fake.paths());
    with_pace(&f.host, |pace| {
        assert_eq!(pace.backoff, Duration::from_secs(120));
        let left = pace.paused_until.unwrap() - Instant::now();
        assert!(left > Duration::from_secs(110), "{left:?}");
    });
    // A due pass while paused asks nothing, and the apps are not marked as missing covers.
    changed(&f);
    watch_with(&f.host, &fake);
    assert_eq!(fake.paths().len(), 1);
    assert!(!failed_marker(&f, "one").exists());

    // When the wait is over the next pass runs at once, with nothing else changed, and without
    // Retry-After the wait doubles.
    pause_over(&f);
    let doubling = Fake::new(|_| reply(429, ""));
    watch_with(&f.host, &doubling);
    assert_eq!(doubling.paths().len(), 1, "no PASS_EVERY wait");
    with_pace(&f.host, |pace| {
        assert_eq!(pace.backoff, Duration::from_secs(240))
    });
    // And once it ran, the next waits for its time again.
    pause_over(&f);
    let found = steamgriddb(true);
    watch_with(&f.host, &found);
    assert!(image(&f, "one").ends_with("sgdb_one.png"));
    assert!(image(&f, "two").ends_with("sgdb_two.png"));
    add(&f, json!({"name": "Three", "uuid": "three"}));
    with_pace(&f.host, |pace| pace.last_version += 1);
    let idle = steamgriddb(true);
    watch_with(&f.host, &idle);
    assert!(idle.paths().is_empty(), "{:?}", idle.paths());
}

#[test]
fn a_change_made_while_a_pass_ran_starts_the_next_pass_but_its_own_covers_do_not() {
    let f = Fixture::new();
    with_key(&f);
    add(&f, json!({"name": "One", "uuid": "one"}));
    let host = f.host.clone();
    // Someone saves a setting while SteamGridDB is asked.
    let fake = Fake::new(move |url| {
        if url.contains("/search/") {
            host.config_version.fetch_add(1, Ordering::Release);
        }
        reply(200, r#"{"success":true,"data":[]}"#)
    });
    watch_with(&f.host, &fake);
    let first = with_pace(&f.host, |pace| pace.last_pass);
    watch_with(&f.host, &steamgriddb(true));
    assert_ne!(
        with_pace(&f.host, |pace| pace.last_pass),
        first,
        "the change made during the pass is not absorbed: the next pass runs at once"
    );

    // A pass that only adds its own covers leaves nothing due.
    add(&f, json!({"name": "Two", "uuid": "two"}));
    watch_with(&f.host, &steamgriddb(true));
    assert!(image(&f, "two").ends_with("sgdb_two.png"));
    let now = f.host.config_version.load(Ordering::Acquire);
    with_pace(&f.host, |pace| assert_eq!(pace.last_version, now));
}

#[test]
fn a_request_refused_for_one_app_leaves_the_others_and_a_server_error_skips_it_next_time() {
    let f = Fixture::new();
    with_key(&f);
    add(&f, json!({"name": "Bad Name", "uuid": "bad"}));
    add(&f, json!({"name": "Hollow Knight", "uuid": "good"}));
    let base = steamgriddb(true);
    let fake = Fake::new(move |url| {
        if url.contains("Bad%20Name") {
            return reply(400, r#"{"success":false,"errors":["bad term"]}"#);
        }
        base.get(url, None)
    });
    watch_with(&f.host, &fake);
    assert!(failed_marker(&f, "bad").exists(), "only that app waits");
    assert!(
        image(&f, "good").ends_with("sgdb_good.png"),
        "the pass went on"
    );

    // A server error pauses every request and also marks the app, so the next pass, once the
    // pause is over, starts with the app after it.
    add(&f, json!({"name": "Stuck", "uuid": "stuck"}));
    add(&f, json!({"name": "After", "uuid": "after"}));
    let base = steamgriddb(true);
    let fake = Fake::new(move |url| {
        if url.contains("Stuck") {
            return reply(500, "");
        }
        base.get(url, None)
    });
    watch_with(&f.host, &fake);
    assert_eq!(
        fake.paths(),
        ["/search/autocomplete/Stuck"],
        "the pass stopped"
    );
    assert!(failed_marker(&f, "stuck").exists());
    assert!(with_pace(&f.host, |pace| pace
        .paused_until
        .is_some_and(|until| until > Instant::now())));
    pause_over(&f);
    watch_with(&f.host, &fake);
    assert_eq!(fake.paths()[1], "/search/autocomplete/After");
    assert!(image(&f, "after").ends_with("sgdb_after.png"));
    assert_eq!(image(&f, "stuck"), "");
}

#[test]
fn only_a_401_refuses_the_key_and_tells_the_console_until_the_key_changes() {
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

    // A 403 is a timed pause, as a server error: the key stays usable and nothing is told.
    add(&f, json!({"name": "Other", "uuid": "other"}));
    let forbidden = Fake::new(|_| reply(403, ""));
    watch_with(&f.host, &forbidden);
    assert_eq!(forbidden.paths().len(), 1);
    with_pace(&f.host, |pace| {
        assert!(pace.refused.is_none());
        assert!(
            pace.paused_until
                .is_some_and(|until| until > Instant::now())
        );
    });
    assert!(!warned(&f));

    std::fs::remove_file(failed_marker(&f, "other")).unwrap();
    pause_over(&f);
    let refused = Fake::new(|_| reply(401, NOT_FOUND));
    watch_with(&f.host, &refused);
    assert_eq!(refused.paths().len(), 1);
    assert!(warned(&f), "the console says so");
    assert!(!failed_marker(&f, "other").exists(), "not the app's fault");
    changed(&f);
    watch_with(&f.host, &refused);
    assert_eq!(refused.paths().len(), 1, "nothing until the key changes");

    store_key(&f.host, Some("fedcba9876543210")).unwrap();
    changed(&f);
    let found = steamgriddb(true);
    watch_with(&f.host, &found);
    assert!(!warned(&f), "a new key takes the warning back");
    assert!(image(&f, "other").ends_with("sgdb_other.png"));
}

#[test]
fn only_a_png_or_jpeg_image_that_says_so_becomes_a_cover() {
    for (case, (content_type, body)) in [
        ("a bitmap", ("image/png", bitmap())),
        ("a page", ("text/html", png())),
        ("html", ("image/png", b"<html>error</html>".to_vec())),
    ] {
        let f = Fixture::new();
        with_key(&f);
        add(&f, json!({"name": "Hollow Knight", "uuid": "typed"}));
        let base = steamgriddb(true);
        let body = Mutex::new(Some(body));
        let fake = Fake::new(move |url| {
            if url.starts_with(CDN) {
                return image_reply(content_type, body.lock().unwrap().take().unwrap());
            }
            base.get(url, None)
        });
        watch_with(&f.host, &fake);
        assert_eq!(image(&f, "typed"), "", "case {case}");
        assert!(failed_marker(&f, "typed").exists(), "case {case}");
        assert!(
            !f.host.directory.join("covers/sgdb_typed.png").exists(),
            "case {case}"
        );
    }
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
        if url.starts_with(CDN) {
            return image_reply("image/png", png());
        }
        if url.contains("/grids/steam/") {
            return reply(200, GRID);
        }
        anyhow::bail!("connection refused")
    });
    // Every way a request can fail, each logged.
    let again = |f: &Fixture| {
        PACES.lock().unwrap().remove(&f.host.directory);
        let _ = std::fs::remove_file(failed_marker(f, "broken"));
    };
    crate::test_log::capture_during(subscriber, || {
        watch_with(&f.host, &fake);
        for status in [500, 403, 400, 429, 401] {
            again(&f);
            watch_with(&f.host, &Fake::new(move |_| reply(status, "")));
        }
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
    for said in [
        "SteamGridDB did not answer",
        "answered with an error",
        "refused the request for this app",
        "asked for fewer requests",
        "refused the API key",
        "app=Broken",
    ] {
        assert!(log.contains(said), "{said}: {log}");
    }
    assert!(!log.contains(KEY), "{log}");
}

/// `key` typed into the host's `sunshine.conf` by hand.
fn typed_key(f: &Fixture, key: &str) {
    let mut config = f.host.config.write().unwrap();
    config.values.insert(sgdb::KEY_SETTING.into(), key.into());
    butterpollo_core::state::atomic_write(&f.host.config_path, config.text().as_bytes()).unwrap();
}
fn in_conf(f: &Fixture) -> bool {
    butterpollo_core::config::Config::load(&f.host.config_path)
        .unwrap()
        .values
        .contains_key(sgdb::KEY_SETTING)
        || f.host
            .config
            .read()
            .unwrap()
            .values
            .contains_key(sgdb::KEY_SETTING)
}

#[test]
fn a_key_typed_into_sunshine_conf_moves_to_the_credentials_folder_on_a_system_host() {
    let f = Fixture::new();
    typed_key(&f, KEY);
    let file = f.host.directory.join("credentials").join(sgdb::KEY_FILE);
    // Not as SYSTEM: the folder protects nothing, and the key stays where it was typed.
    AS_SYSTEM.with(|system| system.set(false));
    migrate_key(&f.host);
    assert!(in_conf(&f));
    assert!(!file.exists());
    assert_eq!(api_key(&f.host).as_deref(), Some(KEY), "it is read there");

    AS_SYSTEM.with(|system| system.set(true));
    f.host
        .warnings
        .set(REFUSED_WARNING, "an older key was refused");
    let before = f.host.config_version.load(Ordering::Acquire);
    migrate_key(&f.host);
    AS_SYSTEM.with(|system| system.set(false));
    assert!(
        !in_conf(&f),
        "gone from sunshine.conf and the settings in use"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), KEY);
    assert_eq!(api_key(&f.host).as_deref(), Some(KEY));
    assert!(f.host.config_version.load(Ordering::Acquire) > before);
    assert!(!warned(&f), "a moved key is a changed key");
}

#[test]
fn a_changed_or_removed_key_ends_a_refusal_and_takes_its_warning_back() {
    let f = Fixture::new();
    with_key(&f);
    refuse(&f.host, KEY);
    assert!(warned(&f));
    // Saved again, even with the same value.
    key_changed(&f.host);
    assert!(!refused(&f.host));
    assert!(!warned(&f));

    // Removed: the next tick takes a refusal back although no key is left to try.
    refuse(&f.host, KEY);
    store_key(&f.host, None).unwrap();
    watch_with(&f.host, &steamgriddb(true));
    assert!(!refused(&f.host));
    assert!(!warned(&f));

    // A refusal the user dismissed shows again for the next refused key.
    store_key(&f.host, Some(KEY)).unwrap();
    add(&f, json!({"name": "Hollow Knight", "uuid": "again"}));
    watch_with(&f.host, &Fake::new(|_| reply(401, "")));
    crate::host_warnings::dismiss(&f.host.directory, &f.host.warnings, REFUSED_WARNING).unwrap();
    let visible = |f: &Fixture| {
        crate::host_warnings::visible(&f.host.directory, &f.host.warnings)
            .iter()
            .any(|warning| warning.code == REFUSED_WARNING)
    };
    assert!(!visible(&f));
    store_key(&f.host, Some("fedcba9876543210")).unwrap();
    key_changed(&f.host);
    changed(&f);
    watch_with(&f.host, &Fake::new(|_| reply(401, "")));
    assert!(
        visible(&f),
        "withdrawn, not only cleared, so it shows again"
    );
}

#[test]
fn a_key_changed_during_a_pass_ends_it() {
    let f = Fixture::new();
    with_key(&f);
    add(&f, json!({"name": "One", "uuid": "one"}));
    add(&f, json!({"name": "Two", "uuid": "two"}));
    let host = f.host.clone();
    let base = steamgriddb(true);
    let fake = Fake::new(move |url| {
        if url.starts_with(CDN) {
            // The user saves another key while the first cover downloads.
            store_key(&host, Some("fedcba9876543210")).unwrap();
        }
        base.get(url, None)
    });
    watch_with(&f.host, &fake);
    assert!(image(&f, "one").ends_with("sgdb_one.png"));
    assert_eq!(image(&f, "two"), "", "not asked with the old key");
    for (url, key) in fake.asked.lock().unwrap().iter() {
        if url.starts_with(sgdb::API) {
            assert_eq!(key.as_deref(), Some(KEY), "{url}");
        }
    }
    assert!(!fake.paths().iter().any(|path| path.contains("Two")));
}
