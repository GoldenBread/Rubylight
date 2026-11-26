//! Nucleus window lifecycle, controller unparking, and co-op session admission tests.
use super::setup_helper::{admit, app, background, client, setup};
use butterpollo_config::config::Config;
use butterpollo_session::nucleus::coop::Refused;
use butterpollo_session::session::Seat;
use butterpollo_session::state::Client;
use butterpollo_stream::nucleus::*;
use std::{path::PathBuf, sync::Arc, time::Duration};

#[test]
fn open_needs_the_install_and_no_running_session() {
    let (registry, pc, apps) = setup();
    *pc.installed.lock().unwrap() = None;
    let refused = registry.open("").unwrap_err();
    assert_eq!(
        (refused.status, refused.message.as_str()),
        (503, not_set_up().as_str())
    );
    *pc.installed.lock().unwrap() = Some(PathBuf::from(r"C:\Nucleus"));
    registry.open("").unwrap();
    assert_eq!(pc.opened.load(std::sync::atomic::Ordering::Relaxed), 1);
    let coop = registry.entries(&apps, "").remove(0);
    admit(&registry, &coop, "a", &client("A", false)).unwrap();
    let refused = registry.open("").unwrap_err();
    assert_eq!(
        (refused.status, refused.message.as_str()),
        (
            409,
            "The co-op helper is running Portal 2: end that session first"
        )
    );
    assert_eq!(pc.opened.load(std::sync::atomic::Ordering::Relaxed), 1);
}

#[test]
fn a_session_closes_the_window_the_console_opened_first() {
    use std::sync::atomic::Ordering::Relaxed;
    let (registry, pc, apps) = setup();
    registry.open("").unwrap();
    assert!(registry.opened());
    let window = pc.window();
    let coop = registry.entries(&apps, "").remove(0);
    admit(&registry, &coop, "a", &client("A", false)).unwrap();
    assert_eq!(window.closes.load(Relaxed), 1);
    assert!(!window.open.load(Relaxed));
    assert!(!registry.opened());
    assert_eq!(pc.started.lock().unwrap().len(), 1);

    // A background session closes it too.
    let (registry, pc, apps) = setup();
    registry.open("").unwrap();
    registry
        .admit(Admission {
            launch: "b",
            client: &client("B", false),
            game: Game::Background(&apps[1]),
            size: (1920, 1080),
            config: &Config::default(),
            warnings: Default::default(),
            release_parked: &|| Ok(()),
            instances: None,
        })
        .unwrap();
    assert!(!pc.window().open.load(Relaxed));
    assert_eq!(pc.started.lock().unwrap().len(), 1);
}

/// #56: a launch that starts a session, co-op or background, first unplugs
/// the controllers that streams which ended still park, and one that can't
/// is refused with the reason before Nucleus starts anything. A launch that
/// joins, or that is refused before it would start one, unplugs nothing.
#[test]
fn a_devices_second_screen_takes_no_seat() {
    let (registry, pc, apps) = setup();
    let coop = registry.entries(&apps, "").remove(0);
    let device = client("A", true);
    let second = device.clone().second_screen();
    // Neither the first seat nor a joining one, in co-op or in the background.
    for game in [Game::Coop(&coop), Game::Background(&apps[1])] {
        let refused = registry
            .admit(Admission {
                launch: "a2",
                client: &second,
                game,
                size: (1920, 1080),
                config: &Config::default(),
                warnings: Default::default(),
                release_parked: &|| Ok(()),
                instances: None,
            })
            .unwrap_err();
        assert!(
            refused.message.contains("second screen"),
            "{}",
            refused.message
        );
    }
    assert!(pc.started.lock().unwrap().is_empty());
    admit(&registry, &coop, "a", &device).unwrap();
    assert!(admit(&registry, &coop, "a2", &second).is_err());
}

#[test]
fn only_a_starting_session_releases_parked_controllers() {
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    let released = AtomicUsize::new(0);
    let release = || {
        released.fetch_add(1, Relaxed);
        Ok(())
    };
    let (registry, _pc, apps) = setup();
    let coop = registry.entries(&apps, "").remove(0);
    let join = |launch, client: &Client| {
        registry.admit(Admission {
            launch,
            client,
            game: Game::Coop(&coop),
            size: (1920, 1080),
            config: &Config::default(),
            warnings: Default::default(),
            release_parked: &release,
            instances: None,
        })
    };
    join("a", &client("A", true)).unwrap();
    assert_eq!(released.load(Relaxed), 1);
    join("b", &client("B", true)).unwrap();
    assert_eq!(released.load(Relaxed), 1);
    // One Nucleus Co-op session at a time.
    let busy = registry.admit(Admission {
        launch: "c",
        client: &client("C", false),
        game: Game::Background(&apps[1]),
        size: (1920, 1080),
        config: &Config::default(),
        warnings: Default::default(),
        release_parked: &release,
        instances: None,
    });
    assert!(busy.is_err());
    assert_eq!(released.load(Relaxed), 1);

    let (registry, pc, apps) = setup();
    let background = |release_parked: &(dyn Fn() -> anyhow::Result<()> + Sync)| {
        registry.admit(Admission {
            launch: "d",
            client: &client("D", false),
            game: Game::Background(&apps[1]),
            size: (1920, 1080),
            config: &Config::default(),
            warnings: Default::default(),
            release_parked,
            instances: None,
        })
    };
    let stuck = background(&|| anyhow::bail!("the driver did not answer")).unwrap_err();
    assert!(
        stuck.message.contains(
            "the co-op helper could not unplug the controllers of streams that ended: the driver did not answer"
        ),
        "{}",
        stuck.message
    );
    assert!(pc.started.lock().unwrap().is_empty());
    background(&release).unwrap();
    assert_eq!(released.load(Relaxed), 2);
}

#[test]
fn a_window_that_wont_close_refuses_the_session_and_stays_open() {
    use std::sync::atomic::Ordering::Relaxed;
    let (registry, pc, apps) = setup();
    registry.open("").unwrap();
    let window = pc.window();
    window.stuck.store(true, Relaxed);
    let coop = registry.entries(&apps, "").remove(0);
    let refused = admit(&registry, &coop, "a", &client("A", false)).unwrap_err();
    assert_eq!(
        (refused.status, refused.message.as_str()),
        (
            503,
            "The co-op helper did not start Portal 2: the co-op helper (process 9000) did not close within 5 s: close it on the PC"
        )
    );
    // Asked to close, never ended.
    assert_eq!(window.closes.load(Relaxed), 1);
    assert!(window.open.load(Relaxed));
    assert!(registry.opened());
    assert!(pc.started.lock().unwrap().is_empty());
    // Once the user closes it at the PC, the next launch starts.
    window.stuck.store(false, Relaxed);
    window.open.store(false, Relaxed);
    admit(&registry, &coop, "a", &client("A", false)).unwrap();
    assert_eq!(window.closes.load(Relaxed), 1);
}

#[test]
fn a_launch_refused_for_another_reason_leaves_the_window_open() {
    use std::sync::atomic::Ordering::Relaxed;
    let (registry, pc, apps) = setup();
    registry.open("").unwrap();
    let window = pc.window();
    *pc.free.lock().unwrap() = 0;
    let coop = registry.entries(&apps, "").remove(0);
    admit(&registry, &coop, "a", &client("A", false)).unwrap_err();
    assert_eq!(window.closes.load(Relaxed), 0);
    assert!(registry.opened());
    assert!(pc.started.lock().unwrap().is_empty());
}

#[test]
fn a_launch_the_pc_cant_start_leaves_the_window_open() {
    // Refused for administrator rights, a handler file that isn't ours,
    // a game list the host can't edit or compatibility settings an
    // earlier session left that don't come back, only after the
    // console's Nucleus window had closed.
    use std::sync::atomic::Ordering::Relaxed;
    let (registry, pc, apps) = setup();
    registry.open("").unwrap();
    let window = pc.window();
    *pc.ready_error.lock().unwrap() = Some("no administrator rights".into());
    let coop = registry.entries(&apps, "").remove(0);
    let refused = admit(&registry, &coop, "a", &client("A", false)).unwrap_err();
    assert_eq!(
        (refused.status, refused.message.as_str()),
        (
            503,
            "The co-op helper did not start Portal 2: no administrator rights"
        )
    );
    let ice = app("Ice", r"C:\NoSuchGames\Ice\Ice.exe");
    let refused = background(
        &registry,
        &ice,
        "b",
        &client("B", false),
        &Config::default(),
    )
    .unwrap_err();
    assert!(
        refused.message.ends_with("no administrator rights"),
        "{refused}"
    );
    assert_eq!(window.closes.load(Relaxed), 0);
    assert!(registry.opened());
    assert!(pc.own.lock().unwrap().is_empty());
    assert!(pc.started.lock().unwrap().is_empty());
}

#[test]
fn a_window_that_cant_be_read_refuses_the_session_with_why() {
    use std::sync::atomic::Ordering::Relaxed;
    let (registry, pc, apps) = setup();
    registry.open("").unwrap();
    let window = pc.window();
    window.unreadable.store(true, Relaxed);
    let coop = registry.entries(&apps, "").remove(0);
    let refused = admit(&registry, &coop, "a", &client("A", false)).unwrap_err();
    assert_eq!(
        (refused.status, refused.message.as_str()),
        (
            503,
            "The co-op helper did not start Portal 2: the co-op helper's window (process 9000) could not be read: access denied"
        )
    );
    assert_eq!(window.closes.load(Relaxed), 0);
    assert!(pc.started.lock().unwrap().is_empty());
}

#[test]
fn open_is_refused_while_a_launch_is_being_admitted() {
    use std::sync::atomic::Ordering::Relaxed;
    let (registry, pc, apps) = setup();
    let registry = Arc::new(registry);
    let coop = registry.entries(&apps, "").remove(0);
    let reads = pc.reads.load(Relaxed);
    *pc.hold.lock().unwrap() = true;
    let launch = {
        let registry = registry.clone();
        std::thread::spawn(move || admit(&registry, &coop, "a", &client("A", false)).map(|_| ()))
    };
    while pc.reads.load(Relaxed) == reads {
        std::thread::sleep(Duration::from_millis(5));
    }
    // The launch holds the admission while it reads the handlers.
    let refused = registry.open("").unwrap_err();
    assert_eq!(
        (refused.status, refused.message.as_str()),
        (
            409,
            "A co-op or background stream is starting: open the co-op helper once it has"
        )
    );
    assert_eq!(pc.opened.load(Relaxed), 0);
    *pc.hold.lock().unwrap() = false;
    pc.release.notify_all();
    launch.join().unwrap().unwrap();
}

#[test]
fn a_nucleus_that_restarted_itself_is_still_the_one_the_console_opened() {
    use std::sync::atomic::Ordering::Relaxed;
    let (registry, pc, _apps) = setup();
    registry.open("").unwrap();
    let window = pc.window();
    *window.restarted_as.lock().unwrap() = Some(9500);
    let refused = registry.open("").unwrap_err();
    assert_eq!(
        (refused.status, refused.message.as_str()),
        (
            409,
            "The co-op helper is already open on the PC's screen: use it there, or close it here"
        )
    );
    assert!(registry.close().unwrap());
    assert_eq!(window.closes.load(Relaxed), 1);
    assert!(!registry.opened());
}

#[test]
fn open_and_close_leave_a_nucleus_the_host_did_not_start_alone() {
    use std::sync::atomic::Ordering::Relaxed;
    let (registry, pc, apps) = setup();
    *pc.foreign.lock().unwrap() = Some(77);
    let refused = registry.open("").unwrap_err();
    assert_eq!(
        (refused.status, refused.message.as_str()),
        (
            409,
            "The co-op helper is already running on the PC (process 77): close it there first"
        )
    );
    assert_eq!(pc.opened.load(Relaxed), 0);
    let refused = registry.close().unwrap_err();
    assert_eq!(
        (refused.status, refused.message.as_str()),
        (
            409,
            "The co-op helper (process 77) was not opened from this console: close it on the PC"
        )
    );
    // A launch is refused as before, loudly.
    let coop = registry.entries(&apps, "").remove(0);
    let refused = admit(&registry, &coop, "a", &client("A", false)).unwrap_err();
    assert!(
            refused
                .message
                .ends_with("The co-op helper is already running on the PC (process 77): close it, then start the stream again"),
            "{refused}"
        );
}

#[test]
fn open_twice_is_refused_and_close_closes_it_once() {
    use std::sync::atomic::Ordering::Relaxed;
    let (registry, pc, _apps) = setup();
    assert!(!registry.close().unwrap(), "nothing to close");
    registry.open("").unwrap();
    let refused = registry.open("").unwrap_err();
    assert_eq!(
        (refused.status, refused.message.as_str()),
        (
            409,
            "The co-op helper is already open on the PC's screen: use it there, or close it here"
        )
    );
    assert_eq!(pc.opened.load(Relaxed), 1);
    // A question it asks keeps it open, and says so.
    pc.window().stuck.store(true, Relaxed);
    let refused = registry.close().unwrap_err();
    assert_eq!(refused.status, 409);
    assert!(refused.message.contains("did not close"), "{refused}");
    assert!(registry.opened());
    pc.window().stuck.store(false, Relaxed);
    assert!(registry.close().unwrap());
    assert!(!registry.opened());
    assert!(!registry.close().unwrap());
    // Closed, it opens again.
    registry.open("").unwrap();
    assert_eq!(pc.opened.load(Relaxed), 2);
}

#[test]
fn the_catalog_is_read_once_for_the_library_and_again_for_a_launch() {
    let (registry, pc, apps) = setup();
    let coop = registry.entries(&apps, "").remove(0);
    registry.entries(&apps, "");
    assert_eq!(pc.reads.load(std::sync::atomic::Ordering::Relaxed), 1);
    admit(&registry, &coop, "a", &client("A", false)).unwrap();
    assert_eq!(pc.reads.load(std::sync::atomic::Ordering::Relaxed), 2);
}

#[test]
fn launches_start_a_session_and_join_it_by_permission_and_free_slot() {
    let (registry, pc, apps) = setup();
    let coop = registry.entries(&apps, "").remove(0);
    let owner = client("A", false);
    assert_eq!(
        admit(&registry, &coop, "a", &owner).unwrap(),
        Seat {
            session: 1,
            slot: 0
        }
    );
    let session = pc.last();
    assert_eq!(session.session.lock().unwrap().slots.len(), 2);
    // Without Join co-op another device is refused.
    let refused = admit(&registry, &coop, "b", &client("B", false)).unwrap_err();
    assert_eq!(refused.status, 409);
    assert_eq!(refused.message, Refused::NotPermitted.to_string());
    let second = client("B", true);
    assert_eq!(
        admit(&registry, &coop, "b", &second).unwrap(),
        Seat {
            session: 1,
            slot: 1
        }
    );
    let full = admit(&registry, &coop, "c", &client("C", true)).unwrap_err();
    assert_eq!(
        full,
        Refusal::busy(
            Refused::Full {
                title: "Portal 2".into()
            }
            .to_string()
        )
    );
    assert_eq!(pc.started.lock().unwrap().len(), 1);
    // The registry hands the stream its session.
    let instance = registry.instance(1).unwrap();
    assert!(instance.capture_target(1, "d").is_err());
    pc.last().show();
    assert_eq!(instance.capture_target(1, "d").unwrap(), "window:101@d");
    assert!(registry.instance(2).is_none());
}
