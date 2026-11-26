//! Private background stream sessions, Rubylight's own handler setup, DirectInput hooks, and Steam link tests.
use super::setup_helper::{admit, app, background, client, refuse, setup};
use butterpollo_config::config::Config;
use butterpollo_session::nucleus::coop::Kind;
use butterpollo_session::nucleus::generic::Setup;
use butterpollo_session::nucleus::install::Problem;
use butterpollo_session::session::Seat;
use butterpollo_stream::nucleus::*;
use std::{path::PathBuf, sync::Arc};

/// #59: the session a launch starts tells the launch's routes the instances
/// it finds, from its start on: no stream has looked for their sound yet.
#[test]
fn a_started_session_tells_the_launchs_routes_its_instances() {
    let (registry, pc, apps) = setup();
    let routes = butterpollo_stream::stream::InstanceRoutes::start(
        |_| None::<anyhow::Result<butterpollo_win_io::audio_app_route::AppRoute>>,
        |_| None,
        false,
    );
    let routes = Arc::new(routes.unwrap());
    let seat = registry
        .admit(Admission {
            launch: "a",
            client: &client("A", false),
            game: Game::Background(&apps[1]),
            size: (1280, 720),
            config: &Config::default(),
            warnings: Default::default(),
            release_parked: &|| Ok(()),
            instances: Some(routes.clone()),
        })
        .unwrap();
    let started = pc.instances.lock().unwrap();
    let given = started[0].as_ref().unwrap();
    let routes_told: butterpollo_win_io::nucleus::session::Instances = routes.clone();
    assert!(Arc::ptr_eq(given, &routes_told));
    // Each player's choice reaches its session's routes: nothing started
    // yet, so nothing to tell.
    assert_eq!(registry.plays_on_pc(seat, true), None);
}

#[test]
fn a_background_stream_is_a_private_session_its_owner_returns_to() {
    let (registry, pc, apps) = setup();
    let owner = client("A", true);
    let config = Config::default();
    let seat = background(&registry, &apps[1], "a", &owner, &config).unwrap();
    assert_eq!(
        seat,
        Seat {
            session: 1,
            slot: 0
        }
    );
    let listed = registry.sessions();
    assert_eq!(listed[0].session.kind, Kind::Background);
    // One player whatever the handler allows, listed under the app.
    assert_eq!(listed[0].session.slots.len(), 1);
    assert_eq!(listed[0].app_id, apps[1].id());
    assert_eq!(pc.last().sizes.lock().unwrap()[..], [(1280, 720)]);
    // Nobody else joins it, by the app or by its co-op entry.
    let other = background(&registry, &apps[1], "b", &client("B", true), &config).unwrap_err();
    assert_eq!(other.status, 409);
    assert!(
            other
                .message
                .starts_with("Portal 2 can't stream in the background: the co-op helper is running Portal 2 for a background stream"),
            "{}",
            other.message
        );
    let coop = registry.entries(&apps, "").remove(0);
    assert_eq!(
        admit(&registry, &coop, "c", &client("B", true))
            .unwrap_err()
            .status,
        409
    );
    // Nor from a session list: its id is no co-op session, for anyone.
    assert_eq!(
        registry.join(seat.session, &client("B", true)),
        Err(JoinError::NoSession)
    );
    assert_eq!(
        registry.join(seat.session, &owner),
        Err(JoinError::NoSession)
    );
    // Its owner returns to its own slot of the same session.
    registry.leave(seat, "a");
    assert_eq!(
        background(&registry, &apps[1], "d", &owner, &config).unwrap(),
        seat
    );
    assert_eq!(pc.started.lock().unwrap().len(), 1);
}

#[test]
fn a_background_stream_nucleus_cannot_run_is_refused_with_the_reason() {
    let (registry, pc, apps) = setup();
    let owner = client("A", false);
    let config = Config::default();
    // A launcher's link, or no program at all.
    let store = app(
        "Store Game",
        "com.epicgames.launcher://apps/x?action=launch",
    );
    let missing = background(&registry, &store, "a", &owner, &config).unwrap_err();
    assert_eq!(missing.status, 404);
    assert_eq!(
        missing.message,
        "Store Game can't stream in the background: the co-op helper starts games by their .exe, and Store Game does not name one: set the app's command to the game's .exe"
    );
    let desktop = background(&registry, &apps[0], "a", &owner, &config).unwrap_err();
    assert_eq!(desktop.status, 404);
    assert!(
        desktop.message.starts_with(
            "Desktop can't stream in the background: the co-op helper starts games by their .exe"
        ),
        "{}",
        desktop.message
    );
    // Windows' own program, or a game on a network share, which the
    // host as SYSTEM would open: refused before anything reads it.
    let notepad = app("Notepad", r"C:\Windows\notepad.exe");
    let refused = background(&registry, &notepad, "a", &owner, &config).unwrap_err();
    assert_eq!(refused.status, 404);
    assert_eq!(
        refused.message,
        r"Notepad can't stream in the background: C:\Windows\notepad.exe is part of Windows, not a game"
    );
    let shared = app("Shared", r"\\nas\games\Ice\Ice.exe");
    let refused = background(&registry, &shared, "a", &owner, &config).unwrap_err();
    assert!(
        refused.message.ends_with(
            "is on a network share: Rubylight starts only a game on this PC's own drives"
        ),
        "{refused}"
    );
    assert!(pc.own.lock().unwrap().is_empty());
    // A background_mode that is no mode runs none.
    let bad = Config::parse("background_mode = rdp\n").unwrap();
    let refused = background(&registry, &apps[1], "a", &owner, &bad).unwrap_err();
    assert_eq!(refused.status, 503);
    assert!(
        refused
            .message
            .contains("background_mode = rdp is not a mode"),
        "{}",
        refused.message
    );
    // Values earlier builds wrote run in Nucleus Co-op.
    for old in ["auto", "child_session", "virtual_display"] {
        let config = Config::parse(&format!("background_mode = {old}\n")).unwrap();
        let seat = background(&registry, &apps[1], old, &owner, &config).unwrap();
        assert!(registry.end(seat.session), "{old}");
    }
    // Where Nucleus Co-op can't run, the reason.
    registry.use_pc(Arc::new(fake::Pc::unavailable()));
    let unavailable = background(&registry, &apps[1], "a", &owner, &config).unwrap_err();
    assert_eq!(unavailable.status, 503);
    assert_eq!(
        unavailable.message,
        "Portal 2 can't stream in the background: not on this test PC"
    );
    assert_eq!(pc.started.lock().unwrap().len(), 3);
}

#[test]
fn a_one_player_game_without_a_hub_handler_streams_in_the_background_with_ours() {
    let (registry, pc, mut apps) = setup();
    let owner = client("A", false);
    let config = Config::default();
    apps.push(app("Ice", r#""C:\NoSuchGames\Ice\Ice.exe" -windowed"#));
    let seat = background(&registry, &apps[3], "a", &owner, &config).unwrap();
    let own = pc.own.lock().unwrap().clone();
    assert_eq!(own.len(), 1);
    assert_eq!(own[0].exe_path, r"C:\NoSuchGames\Ice\Ice.exe");
    assert_eq!(own[0].name, "Ice");
    // The game starts with the app's arguments.
    assert_eq!(own[0].arguments, "-windowed");
    // At the stream's size, which an Unreal game renders at (#56).
    assert_eq!(own[0].size, Some((1280, 720)));
    // It reads no DirectInput, so it keeps its raw input and a pad read
    // through it.
    assert!(!own[0].direct_input);
    // One player, our handler's game, listed under the app.
    let listed = registry.sessions();
    assert_eq!(listed[0].session.kind, Kind::Background);
    assert_eq!(listed[0].session.slots.len(), 1);
    assert_eq!(listed[0].session.game, own[0].guid());
    assert_eq!(listed[0].app_id, apps[3].id());
    // Its owner returns to it without another write, Steam read or
    // folder scan.
    assert_eq!(pc.lookups.load(std::sync::atomic::Ordering::Relaxed), 1);
    registry.leave(seat, "a");
    assert_eq!(
        background(&registry, &apps[3], "b", &owner, &config).unwrap(),
        seat
    );
    assert_eq!(pc.lookups.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(pc.own.lock().unwrap().len(), 1);
    assert_eq!(pc.started.lock().unwrap().len(), 1);
    // Co-op still needs a hub handler: no entry, and a co-op launch is refused.
    assert!(
        registry
            .entries(&apps, "")
            .iter()
            .all(|coop| coop.app.name != "Ice")
    );
    let coop = CoopApp {
        app: apps[3].clone(),
        ..registry.entries(&apps, "").remove(0)
    };
    let refused = admit(&registry, &coop, "c", &client("B", true)).unwrap_err();
    assert_eq!(refused.status, 404);
    assert!(
        refused.message.starts_with("No co-op handler for Ice.exe"),
        "{refused}"
    );
}

/// NieR:Automata reads the PC's keyboard through DirectInput's raw input
/// (#56): our handler takes a DirectInput game's raw input, and only its. A
/// game whose .exe can't be read gets no handler written from a guess.
#[test]
fn a_direct_input_games_handler_takes_its_raw_input() {
    let (registry, pc, mut apps) = setup();
    let owner = client("A", false);
    let config = Config::default();
    apps.push(app("NieR", r#""C:\NoSuchGames\NieR\NieRAutomata.exe""#));
    pc.direct_input_error
        .lock()
        .unwrap()
        .replace("the game's file is unreadable".into());
    let refused = background(&registry, &apps[3], "a", &owner, &config).unwrap_err();
    assert!(
        refused.message.contains("the game's file is unreadable"),
        "{refused}"
    );
    assert!(pc.own.lock().unwrap().is_empty());
    pc.direct_input_error.lock().unwrap().take();
    pc.direct_input
        .lock()
        .unwrap()
        .insert(r"C:\NoSuchGames\NieR\NieRAutomata.exe".into());
    background(&registry, &apps[3], "a", &owner, &config).unwrap();
    let own = pc.own.lock().unwrap().clone();
    assert_eq!(own.len(), 1);
    assert!(own[0].direct_input);
    assert!(own[0].handler().contains("RegisterRawInputHook = true"));
}

#[test]
fn ours_runs_in_the_folder_the_user_chose() {
    // Every folder the host runs is the pinned release's, whose game list
    // Rubylight knows: the one chosen as well as an earlier install.
    let (registry, pc, mut apps) = setup();
    let owner = client("A", false);
    let config = Config::parse("nucleus_path = D:\\Nucleus\n").unwrap();
    apps.push(app("Ice", r"C:\NoSuchGames\Ice\Ice.exe"));
    let seat = background(&registry, &apps[3], "a", &owner, &config).unwrap();
    assert_eq!(pc.own.lock().unwrap().len(), 1);
    // Its user's copy is made from the chosen folder.
    assert_eq!(
        *pc.copied_from.lock().unwrap(),
        [PathBuf::from(r"D:\Nucleus")]
    );
    assert!(registry.end(seat.session));
}

#[test]
fn a_launch_without_nucleus_is_refused_with_the_reason_and_installs_nothing() {
    let (registry, pc, mut apps) = setup();
    let owner = client("A", false);
    apps.push(app("Ice", r"C:\NoSuchGames\Ice\Ice.exe"));
    let coop = registry.entries(&apps, "").remove(0);
    *pc.installed.lock().unwrap() = None;
    let refused = admit(&registry, &coop, "a", &owner).unwrap_err();
    assert_eq!((refused.status, refused.message), (503, not_set_up()));
    let refused = background(&registry, &apps[3], "b", &owner, &Config::default()).unwrap_err();
    assert_eq!(
        (refused.status, refused.message),
        (
            503,
            format!(
                "Ice can't stream in the background: {}",
                not_set_up().replacen("The co-op", "the co-op", 1)
            )
        )
    );
    // A chosen folder that's another release, or one others can change,
    // is refused with its reason: the earlier install never stands in.
    *pc.installed.lock().unwrap() = Some(PathBuf::from(r"C:\Nucleus"));
    let config = Config::parse("nucleus_path = D:\\Old\n").unwrap();
    let why = r"The co-op helper in D:\Old isn't version 2.4.2: download 2.4.2 from its release page and choose its folder";
    refuse(&pc, r"D:\Old", Problem::Version, why);
    let refused = background(&registry, &apps[3], "c", &owner, &config).unwrap_err();
    assert_eq!(
        (refused.status, refused.message),
        (
            503,
            format!("Ice can't stream in the background: the co-op helper is not ready: {why}")
        )
    );
    assert!(pc.own.lock().unwrap().is_empty());
    assert!(pc.started.lock().unwrap().is_empty());
}

#[test]
fn a_steam_game_with_ours_starts_nucleus_with_its_app_id() {
    let (registry, pc, _) = setup();
    let owner = client("A", false);
    let config = Config::default();
    let link = app("Black Flag", "steam://rungameid/242050");
    pc.one_player.lock().unwrap().insert(
        link.name.clone(),
        Setup::new(
            r"D:\Steam\steamapps\common\Black Flag\AC4BFSP.exe",
            &link.name,
            Some((242050, r"D:\Steam\steamapps\common\Black Flag")),
        ),
    );
    background(&registry, &link, "a", &owner, &config).unwrap();
    assert_eq!(pc.own.lock().unwrap().len(), 1);
    assert_eq!(*pc.steam_apps.lock().unwrap(), [Some(242050)]);
    // A game started by its own .exe gets none.
    let ice = app("Ice", r"C:\NoSuchGames\Ice\Ice.exe");
    let first = registry.sessions()[0].session.id;
    assert!(registry.end(first));
    background(&registry, &ice, "b", &owner, &config).unwrap();
    assert_eq!(*pc.steam_apps.lock().unwrap(), [Some(242050), None]);
}
