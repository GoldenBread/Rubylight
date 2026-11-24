//! Tests for ProtoInput hook refusal fallbacks, Mandatory ASLR message boxes,
//! and unhooked-game player notices.
use super::*;

/// What Nucleus 2.4.2 shows when it can't inject ProtoInput's hooks,
/// into a running game ([`generic::INJECT_FAILED`]'s first) or one it
/// starts (its second).
fn refused(text: &str) -> MessageBox {
    MessageBox {
        window: 500,
        caption: "Error".into(),
        text: text.into(),
    }
}

/// #59: a game that refuses ProtoInput's hooks gets Nucleus' box, and
/// Nucleus ends it once the box is answered. Rubylight's own handler's
/// session ends saying so, and the game's next handler injects them the
/// next way: as Nucleus starts it (refused before its process runs),
/// then from one of its threads, then none. A hub handler's game is
/// not kept.
#[test]
fn a_game_that_refuses_the_hooks_ends_the_session_and_gets_the_next_way() {
    let [runtime, startup] = generic::INJECT_FAILED;
    let exe = r"D:\Games\Black Flag\ACBlackFlag.exe";
    let another = "start the stream again, and Rubylight has Nucleus Co-op hook it another way";
    let none = "start the stream again, and Rubylight starts it without them";
    let host = tempfile::tempdir().unwrap();
    // The way it was started, the box Nucleus shows, whether the game's
    // process runs by then, and what Rubylight does next.
    let ladder = [
        (Inject::Runtime, runtime, true, Inject::Startup, another),
        (Inject::Startup, startup, false, Inject::Stealth, another),
        (Inject::Stealth, runtime, true, Inject::None, none),
    ];
    for (way, text, runs, next, then) in ladder {
        for own in [false, true] {
            assert_eq!(files::inject_for(host.path(), exe).unwrap(), way);
            let t0 = Instant::now();
            let (screen, probe) = (screen(), Probe::default());
            let mut runner = runner(1, &screen, &probe, t0);
            runner.own_game = own.then(|| (host.path().to_path_buf(), exe.to_string()));
            label(&screen);
            assert_eq!(runner.step(t0), None);
            if runs {
                instance(&screen, 0);
                assert_eq!(runner.step(t0), None);
                assert_eq!(runner.shared.session.lock().unwrap().phase, Phase::Running);
            }
            screen
                .0
                .lock()
                .unwrap()
                .boxes
                .push((NUCLEUS, refused(text)));
            let end = runner.step(t0 + PERIOD);
            if !own {
                assert_eq!(end, None, "{way:?}");
                continue;
            }
            let why = format!(
                "Game refused Nucleus Co-op's input hooks (it showed \"Error: {text}\"), and Nucleus Co-op ends a game it can't hook: {then}"
            );
            // The start fails; a game that ran crashed.
            let wanted = if runs {
                End::Crashed(why)
            } else {
                End::Failed(why)
            };
            assert_eq!(end, Some(wanted), "{way:?}");
            assert_eq!(files::inject_for(host.path(), exe).unwrap(), next);
        }
    }
    // By its path, in any case; other games keep the first way.
    assert_eq!(
        files::inject_for(host.path(), &exe.to_uppercase()).unwrap(),
        Inject::None
    );
    assert_eq!(
        files::inject_for(host.path(), r"D:\Games\Other\Game.exe").unwrap(),
        Inject::Runtime
    );

    // Nucleus injects into the running game right after it starts: a
    // minute after its window shows, the boxes are no longer looked
    // through for it.
    let t0 = Instant::now();
    let host = tempfile::tempdir().unwrap();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    runner.own_game = Some((host.path().to_path_buf(), exe.to_string()));
    label(&screen);
    assert_eq!(runner.step(t0), None);
    instance(&screen, 0);
    assert_eq!(runner.step(t0), None);
    screen
        .0
        .lock()
        .unwrap()
        .boxes
        .push((NUCLEUS, refused(runtime)));
    assert_eq!(runner.step(t0 + START_TIMEOUT), None);
    assert_eq!(
        files::inject_for(host.path(), exe).unwrap(),
        Inject::Runtime
    );
}

/// #59: a game Nucleus fails to inject into as it starts it runs
/// suspended for a moment, is ended, and only then does Nucleus show its
/// box: the instance that exited waits for the box, and the game gets
/// the next way. Without a box, it exited. A hub handler's instance is
/// not held.
#[test]
fn an_instance_ended_before_the_startup_box_shows_is_a_refusal() {
    let [_, startup] = generic::INJECT_FAILED;
    let exe = r"D:\Games\Black Flag\ACBlackFlag.exe";
    let suspended = |screen: &Screen| {
        screen
            .0
            .lock()
            .unwrap()
            .processes
            .push(process(20, NUCLEUS, SINCE + 1, "Game.exe"));
    };
    let ended = |screen: &Screen| screen.0.lock().unwrap().processes.retain(|p| p.pid != 20);
    for (own, box_shows) in [(true, true), (true, false), (false, false)] {
        let t0 = Instant::now();
        let host = tempfile::tempdir().unwrap();
        files::refuse_inject(host.path(), exe).unwrap();
        let (screen, probe) = (screen(), Probe::default());
        let mut runner = runner(1, &screen, &probe, t0);
        runner.own_game = own.then(|| (host.path().to_path_buf(), exe.to_string()));
        label(&screen);
        assert_eq!(runner.step(t0), None);
        suspended(&screen);
        assert_eq!(runner.step(t0 + PERIOD), None);
        assert_eq!(
            runner.shared.roots.lock().unwrap().iter().flatten().count(),
            1
        );
        ended(&screen);
        let exited = Some(End::Failed("Player 1's instance of Game exited".into()));
        let end = runner.step(t0 + 2 * PERIOD);
        if !own {
            assert_eq!(end, exited);
            continue;
        }
        assert_eq!(end, None);
        if !box_shows {
            assert_eq!(runner.step(t0 + 2 * PERIOD + REFUSAL_GRACE / 2), None);
            assert_eq!(runner.step(t0 + 2 * PERIOD + REFUSAL_GRACE), exited);
            assert_eq!(
                files::inject_for(host.path(), exe).unwrap(),
                Inject::Startup
            );
            continue;
        }
        screen
            .0
            .lock()
            .unwrap()
            .boxes
            .push((NUCLEUS, refused(startup)));
        let end = runner.step(t0 + 3 * PERIOD);
        assert!(
            matches!(&end, Some(End::Failed(why)) if why.contains("hook it another way")),
            "{end:?}"
        );
        assert_eq!(
            files::inject_for(host.path(), exe).unwrap(),
            Inject::Stealth
        );
    }
}

/// #59: a box that was up before Nucleus started is an earlier
/// session's, not this game's.
#[test]
fn a_refused_hooks_box_from_before_the_start_is_not_this_games() {
    let refused = refused(generic::INJECT_FAILED[0]);
    let exe = r"D:\Games\Black Flag\ACBlackFlag.exe";
    let t0 = Instant::now();
    let host = tempfile::tempdir().unwrap();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    runner.own_game = Some((host.path().to_path_buf(), exe.to_string()));
    runner.before = BTreeSet::from([refused.window]);
    label(&screen);
    assert_eq!(runner.step(t0), None);
    instance(&screen, 0);
    assert_eq!(runner.step(t0), None);
    screen.0.lock().unwrap().boxes.push((NUCLEUS, refused));
    assert_eq!(runner.step(t0 + PERIOD), None);
    assert_eq!(
        files::inject_for(host.path(), exe).unwrap(),
        Inject::Runtime
    );
}

/// #59: Black Flag Resynced shows its own box, "Disable Mandatory ASLR",
/// and waits there with its window up. When Windows forces Mandatory ASLR
/// on it, any way of hooking it shows the box: the session ends saying how
/// the player can exempt it, and Rubylight's own handler keeps its way, as
/// it does when the policy can't be read. A game Windows doesn't force it
/// on refused the way, and gets the next, down to none. A hub handler's
/// game ends with the reason too, and nothing is kept for it.
#[test]
fn a_game_waiting_on_its_mandatory_aslr_box_ends_and_keeps_its_way_unless_it_refused_it() {
    let exe = r"D:\Games\Black Flag\ACBlackFlag.exe";
    let aslr = MessageBox {
        window: 600,
        caption: String::new(),
        text: "Disable Mandatory ASLR".into(),
    };
    let exempt = |program: &str| {
        format!(
            "turn off \"Force randomization for images (Mandatory ASLR)\" for {program} in Windows Security, App & browser control, Exploit protection, Program settings"
        )
    };
    let waits = "Game showed \"Disable Mandatory ASLR\" and waits there for an answer";
    let forced = format!(
        "{waits}: Windows forces randomization for images (Mandatory ASLR) on it. To start it, you can {}",
        exempt("ACBlackFlag.exe")
    );
    let unread = |program: &str| {
        format!(
            "{waits}; Rubylight could not read whether Windows forces randomization for images (Mandatory ASLR) on it: access denied. If it does, you can {}",
            exempt(program)
        )
    };
    let refused = "Game refused Nucleus Co-op's input hooks (it showed \"Disable Mandatory ASLR\"), and it waits there for an answer: start the stream again, and Rubylight";
    let host = tempfile::tempdir().unwrap();
    files::refuse_inject(host.path(), exe).unwrap();
    // The way it was started, whether it is Rubylight's own handler's game,
    // whether Windows forces Mandatory ASLR on it (None: can't be read),
    // whether its process's path is known, why it ends and the way kept.
    let ladder = [
        (
            Inject::Startup,
            true,
            Some(true),
            false,
            forced.clone(),
            Inject::Startup,
        ),
        (
            Inject::Startup,
            true,
            Some(true),
            true,
            forced.clone(),
            Inject::Startup,
        ),
        (
            Inject::Startup,
            true,
            None,
            false,
            unread("ACBlackFlag.exe"),
            Inject::Startup,
        ),
        (
            Inject::Startup,
            true,
            Some(false),
            false,
            format!("{refused} has Nucleus Co-op hook it another way"),
            Inject::Stealth,
        ),
        (
            Inject::Stealth,
            true,
            Some(false),
            false,
            format!("{refused} starts it without them"),
            Inject::None,
        ),
        (
            Inject::None,
            true,
            Some(false),
            false,
            format!("{waits}, even without Nucleus Co-op's input hooks"),
            Inject::None,
        ),
        (
            Inject::None,
            false,
            Some(true),
            true,
            forced.clone(),
            Inject::None,
        ),
        (
            Inject::None,
            false,
            None,
            false,
            unread("the game's .exe"),
            Inject::None,
        ),
        (
            Inject::None,
            false,
            Some(false),
            false,
            waits.to_string(),
            Inject::None,
        ),
    ];
    for (way, own, forced, known, why, kept) in ladder {
        assert_eq!(files::inject_for(host.path(), exe).unwrap(), way);
        let t0 = Instant::now();
        let (screen, probe) = (screen(), Probe::default());
        let mut runner = runner(1, &screen, &probe, t0);
        runner.own_game = own.then(|| (host.path().to_path_buf(), exe.to_string()));
        label(&screen);
        assert_eq!(runner.step(t0), None);
        instance(&screen, 0);
        assert_eq!(runner.step(t0), None);
        assert_eq!(runner.shared.session.lock().unwrap().phase, Phase::Running);
        // The game's own box, not Nucleus'.
        {
            let mut desktop = screen.0.lock().unwrap();
            desktop.boxes.push((20, aslr.clone()));
            desktop.aslr.extend(forced.map(|forced| (20, forced)));
            if known {
                let linked = r"C:\Nucleus\content\Game\Instance0\ACBlackFlag.exe";
                desktop.paths.insert(20, linked.to_string());
            }
        }
        let end = runner.step(t0 + PERIOD);
        assert_eq!(end, Some(End::Crashed(why)), "{way:?} {own} {forced:?}");
        assert_eq!(files::inject_for(host.path(), exe).unwrap(), kept);
    }
}

/// #59: a Mandatory ASLR box Windows shows for the game
/// (MB_SERVICE_NOTIFICATION) is csrss.exe's, whose policy can't be read:
/// the game instance's process is read instead, so a game Windows doesn't
/// force it on still gets the next way.
#[test]
fn a_mandatory_aslr_box_windows_shows_reads_the_games_policy() {
    use crate::nucleus::instances::tests::CSRSS;
    let exe = r"D:\Games\Black Flag\ACBlackFlag.exe";
    let host = tempfile::tempdir().unwrap();
    files::refuse_inject(host.path(), exe).unwrap();
    let aslr = MessageBox {
        window: 600,
        caption: String::new(),
        text: "Disable Mandatory ASLR".into(),
    };
    for (forced, said, kept) in [
        (true, "Windows forces randomization", Inject::Startup),
        (false, "hook it another way", Inject::Stealth),
    ] {
        let t0 = Instant::now();
        let (screen, probe) = (screen(), Probe::default());
        let mut runner = runner(1, &screen, &probe, t0);
        runner.own_game = Some((host.path().to_path_buf(), exe.to_string()));
        label(&screen);
        assert_eq!(runner.step(t0), None);
        instance(&screen, 0);
        assert_eq!(runner.step(t0), None);
        {
            let mut desktop = screen.0.lock().unwrap();
            desktop.boxes.push((CSRSS, aslr.clone()));
            desktop.aslr.insert(20, forced);
        }
        let end = runner.step(t0 + PERIOD);
        assert!(
            matches!(&end, Some(End::Crashed(why)) if why.contains(said)),
            "{end:?}"
        );
        assert_eq!(files::inject_for(host.path(), exe).unwrap(), kept);
    }
}

/// #59: the stream of a game started without the hooks it refused every
/// way tells its player that the PC's keyboard, mouse and pads reach it
/// too; one still hooked another way is not told. A hub handler's game,
/// whose handler Rubylight doesn't write, is not told.
#[test]
fn a_game_started_without_the_hooks_it_refused_is_told() {
    let host = tempfile::tempdir().unwrap();
    let exe = r"D:\Games\Black Flag\ACBlackFlag.exe";
    let hub = handler();
    let own = Handler {
        own: true,
        ..handler()
    };
    let start_of = |handler, players| Start {
        id: 1,
        kind: kind(players),
        folder: host.path(),
        user_copy: None,
        handler,
        exe_path: exe,
        steam_app: None,
        title: "Black Flag",
        players,
        owner: ("phone", "Phone"),
        size: (1280, 720),
        instances: None,
    };
    let start = |handler| start_of(handler, 1);
    for _ in [Inject::Runtime, Inject::Startup, Inject::Stealth] {
        assert_eq!(unhooked(&start(&own)).unwrap(), None);
        assert!(!brought_to_front(&start(&own)).unwrap(), "hooked");
        files::refuse_inject(host.path(), exe).unwrap();
    }
    assert_eq!(
        unhooked(&start(&own)).unwrap().as_deref(),
        Some(
            "Black Flag refused every way Nucleus Co-op injects its input hooks, so it runs without them: a keyboard, mouse or pad on the PC reaches it too, and it takes the controller only while its window is in front on the PC, where Rubylight puts it as it opens unless someone uses the PC. If the game pauses when it is not in front, the picture stops too"
        )
    );
    assert_eq!(unhooked(&start(&hub)).unwrap(), None);
    // Only a one-player game comes to the front: co-op windows share the
    // screen.
    assert!(brought_to_front(&start(&own)).unwrap());
    assert!(!brought_to_front(&start_of(&own, 2)).unwrap(), "co-op");
    assert!(!brought_to_front(&start(&hub)).unwrap(), "hub handler");
}
