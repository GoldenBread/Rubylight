//! Unit tests for `nightjar_core::console` covering stores, `.bak` damaged favourites,
//! legacy migration, `ConsoleRepository` parallel refresh, 5-min capability cooldown,
//! `add_to_library`, starred renumbering, and `forget_host`.

use super::*;
use nightjar_policy::http::native_api::capabilities::Capabilities;
use nightjar_policy::http::native_api::library::{Artwork, Game, Library, Store};
use nightjar_policy::http::native_api::session::{Owner, Session, SessionApp, State};
use nightjar_policy::http::{AddedGame, FilePermissions, GameExtra, NativeCapabilities, NativeLibrary};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nightjar-core-console-{name}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn sample_game(install_id: &str, app_id: u32, title: &str, game_id: Option<&str>) -> Game {
    Game {
        install_id: install_id.into(),
        app_id,
        title: title.into(),
        game_id: game_id.map(str::to_owned),
        store: Some(Store {
            source: "steam".into(),
            id: "1".into(),
            name: "Steam".into(),
        }),
        installed: Some(true),
        running: false,
        artwork: Artwork {
            cover: Some(format!("/c/{install_id}")),
            version: "v1".into(),
        },
        last_played: None,
        playtime_minutes: None,
        collections: Vec::new(),
    }
}

fn host_desc(uuid: &str, name: &str) -> HostDescriptor {
    HostDescriptor {
        uuid: uuid.into(),
        name: name.into(),
        online: true,
        paired: true,
        is_native: true,
        address: "127.0.0.1".into(),
        http_port: 47989,
        https_port: 47984,
        server_cert_pem: None,
        classic_apps: None,
        running_app_id: None,
        running_app_uuid: None,
    }
}

struct MockTransport {
    caps_calls: AtomicUsize,
    fail_caps: bool,
    libs: Mutex<BTreeMap<String, NativeLibrary>>,
}

impl NativeTransport for MockTransport {
    fn capabilities(&self, _host: &HostDescriptor) -> Result<NativeCapabilities, String> {
        self.caps_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_caps {
            return Err("caps unreachable".into());
        }
        let mut caps: Capabilities = serde_json::from_str(
            r#"{"api_version":1,"permissions":{"list":true,"view":true,"launch":true}}"#,
        )
        .unwrap();
        caps.features.push("library.unadded".into());
        Ok(NativeCapabilities {
            capabilities: caps,
            files: FilePermissions::default(),
        })
    }

    fn library(
        &self,
        host: &HostDescriptor,
        include_unadded: bool,
    ) -> Result<NativeLibrary, String> {
        let mut lib = self
            .libs
            .lock()
            .unwrap()
            .get(&host.uuid)
            .cloned()
            .ok_or_else(|| "no mock lib".to_owned())?;
        if !include_unadded {
            let keep: Vec<bool> = (0..lib.library.games.len())
                .map(|i| lib.added_at(i))
                .collect();
            let mut idx = 0;
            lib.library.games.retain(|_| {
                let k = keep[idx];
                idx += 1;
                k
            });
            let mut eidx = 0;
            lib.extra.retain(|_| {
                let k = keep[eidx];
                eidx += 1;
                k
            });
        }
        Ok(lib)
    }

    fn session(&self, _host: &HostDescriptor) -> Result<Session, String> {
        Ok(Session {
            revision: 1,
            state: State::Streaming,
            owner: Owner::You,
            app: Some(SessionApp {
                install_id: "i1".into(),
                app_id: 1,
                title: "Portal".into(),
            }),
            game_running: true,
            since_unix_ms: None,
            suspended_until_unix_ms: None,
            launcher: None,
            radeon_profile: None,
            microphone_busy: false,
            playing_on: None,
            moved_to: None,
            owns_game: true,
            press_failed: false,
            actions: Vec::new(),
        })
    }

    fn add_to_library(
        &self,
        _host: &HostDescriptor,
        game_id: &str,
    ) -> Result<Option<AddedGame>, String> {
        Ok(Some(AddedGame {
            game: sample_game("added-9", 99, "Celeste", Some(game_id)),
            extra: GameExtra::default(),
        }))
    }
}

#[test]
fn stores_handle_sanitize_recents_cap_damaged_favourites_and_forget() {
    let dir = temp_dir("stores");
    assert_eq!(sanitize_host_id("a/b:c"), "a_b_c");
    assert_eq!(sanitize_host_id(".."), "_");

    let recents = RecentsStore::new(&dir);
    for i in 1..=25u32 {
        recents
            .record(Recent {
                host: "h1".into(),
                install_id: format!("id-{i}"),
                app_id: i,
                title: format!("Game {i}"),
                at: i as u64,
            })
            .unwrap();
    }
    assert_eq!(recents.list().len(), ANDROID_RECENT_LIMIT);
    assert_eq!(recents.list()[0].app_id, 25);

    // Legacy damaged favourites creates .bak while reading valid entries.
    let favs = FavouritesStore::new(&dir);
    assert!(favs
        .import_legacy_json(r#"[{"host":"h1","install_id":"i1","app_id":1,"title":"Good"},{"bad":true}]"#)
        .unwrap());
    assert_eq!(favs.list().len(), 1);
    assert!(dir.join("console").join("console_favourites.json.bak").exists());

    forget_host(&dir, "h1");
    assert!(recents.list().is_empty());
    assert!(favs.list().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn repository_refreshes_respects_cooldown_and_reconciles_favourites() {
    let dir = temp_dir("repo");
    let repo = ConsoleRepository::new(&dir);
    repo.set_favourite(Favourite {
        host: "pc-1".into(),
        install_id: "i1".into(),
        app_id: 500,
        title: "Old".into(),
    })
    .unwrap();

    let mut lib = NativeLibrary::from(Library {
        revision: "r1".into(),
        games: vec![
            sample_game("i1", 1, "Portal", Some("steam:400")),
            sample_game("", 0, "Celeste", Some("gog:c")),
        ],
    });
    lib.extra[1].added = false;
    let mut map = BTreeMap::new();
    map.insert("pc-1".into(), lib);

    let failing_caps = MockTransport {
        caps_calls: AtomicUsize::new(0),
        fail_caps: true,
        libs: Mutex::new(map.clone()),
    };
    let hosts = [host_desc("pc-1", "Desk")];
    let snap1 = repo.refresh(&hosts, &failing_caps);
    assert_eq!(failing_caps.caps_calls.load(Ordering::SeqCst), 1);
    // Second refresh hits 5-minute cooldown instead of retrying capabilities.
    let _ = repo.refresh(&hosts, &failing_caps);
    assert_eq!(failing_caps.caps_calls.load(Ordering::SeqCst), 1);

    // Classic renumbered favourite was repaired on cached/refresh snapshot.
    assert_eq!(snap1.starred[0].app_id, 1);
    assert_eq!(snap1.starred[0].title, "Portal");
    assert_eq!(snap1.continue_playing.len(), 1);

    // add_to_library returns the added entry without mutating the cached library until refresh.
    let working = MockTransport {
        caps_calls: AtomicUsize::new(0),
        fail_caps: false,
        libs: Mutex::new(map),
    };
    let added = repo.add_to_library(&hosts[0], "gog:c", &working).unwrap().unwrap();
    assert_eq!(added.game.install_id, "added-9");
    let snap2 = repo.cached(&hosts);
    assert_eq!(snap2.titles.len(), 1);
    assert_eq!(snap2.titles[0].title, "Portal");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn find_cached_library_cover_handles_precedence_unadded_explicit_none_and_formats() {
    let dir = temp_dir("cached-cover");
    let store = LibraryCacheStore::new(&dir);

    assert_eq!(find_cached_library_cover(&dir, "", 1, "i1"), None);
    assert_eq!(find_cached_library_cover(&dir, "missing", 1, "i1"), None);

    // Envelope format: unadded entry skipped, install_id case-insensitive takes precedence over app_id,
    // explicitly_no_cover set when cover is None and version is empty.
    let mut g_unadded = sample_game("UNADDED-ID", 10, "Unadded", None);
    g_unadded.artwork.cover = Some("/c/unadded".into());
    g_unadded.artwork.version = "v9".into();

    let mut g_by_appid = sample_game("other-id", 42, "By AppId", None);
    g_by_appid.artwork.cover = Some("/c/42".into());
    g_by_appid.artwork.version = "v42".into();

    let mut g_by_install = sample_game("Target-Install", 77, "By InstallId", None);
    g_by_install.artwork.cover = Some("/c/77".into());
    g_by_install.artwork.version = "v77".into();

    let mut g_no_cover = sample_game("no-cover-id", 88, "No Cover", None);
    g_no_cover.artwork.cover = None;
    g_no_cover.artwork.version = String::new();

    let mut lib = NativeLibrary::from(Library {
        revision: "r1".into(),
        games: vec![g_unadded, g_by_appid, g_by_install, g_no_cover],
    });
    lib.extra[0].added = false;
    store.save_entry("pc-env", &lib, 1000, &[]).unwrap();

    assert_eq!(find_cached_library_cover(&dir, "pc-env", 10, "UNADDED-ID"), None);
    assert_eq!(
        find_cached_library_cover(&dir, "pc-env", 42, "target-install"),
        Some(CachedLibraryCover {
            explicitly_no_cover: false,
            app_id: 77,
            title: "By InstallId".into(),
            cover_version: "v77".into(),
        })
    );
    assert_eq!(
        find_cached_library_cover(&dir, "pc-env", 42, ""),
        Some(CachedLibraryCover {
            explicitly_no_cover: false,
            app_id: 42,
            title: "By AppId".into(),
            cover_version: "v42".into(),
        })
    );
    assert_eq!(
        find_cached_library_cover(&dir, "pc-env", 88, ""),
        Some(CachedLibraryCover {
            explicitly_no_cover: true,
            app_id: 88,
            title: "No Cover".into(),
            cover_version: String::new(),
        })
    );

    // Bare format (`{"revision":..,"games":..}`) also parses cleanly.
    let bare = serde_json::json!({
        "revision": "r-bare",
        "games": [{
            "install_id": "bare-1",
            "app_id": 123,
            "title": "Bare Game",
            "running": false,
            "added": true,
            "artwork": { "cover": "/c/bare", "version": "vbare" }
        }]
    });
    std::fs::write(
        dir.join("console-library").join("pc-bare.json"),
        serde_json::to_vec(&bare).unwrap(),
    )
    .unwrap();
    assert_eq!(
        find_cached_library_cover(&dir, "pc-bare", 123, ""),
        Some(CachedLibraryCover {
            explicitly_no_cover: false,
            app_id: 123,
            title: "Bare Game".into(),
            cover_version: "vbare".into(),
        })
    );

    let _ = std::fs::remove_dir_all(&dir);
}
