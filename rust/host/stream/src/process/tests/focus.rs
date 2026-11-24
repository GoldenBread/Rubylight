//! Unit tests for launched game window focus and bring-back rules.

use super::super::*;

/// An app that launched a game Steam started; nothing is started and the
/// tracker follows no process, so only the launch focus's decisions are looked at.
fn launching_app() -> RunningApp {
    let mut app = RunningApp::with_environment(&App::desktop(), BTreeMap::new()).unwrap();
    app.mark_launching();
    app.steam = Some(butterpollo_session::steam::Tracker::new(
        &[],
        "C:\\not-a-game",
        Duration::ZERO,
    ));
    app
}

#[test]
fn a_launched_games_window_is_brought_back_when_minimised_or_behind_steam() {
    // Issue #131: a game that switched the streamed display to another
    // exclusive-fullscreen mode was left minimised, or behind the Steam window.
    let mut app = launching_app();
    let now = std::time::Instant::now();
    let steam = |pid| pid == 99;
    let game = vec![(30, false)];
    // Nothing of the game is on the desktop yet: the minute has not started.
    assert!(app.bring_back_open(now));
    assert_eq!(app.bring_back(now, vec![], Some(99), steam), None);
    assert_eq!(app.focus, None);
    // In front: the minute starts, and nothing is taken from it.
    assert_eq!(app.bring_back(now, game.clone(), Some(30), steam), None);
    assert!(app.focus.is_some());
    // Steam's window in front, as the game's mode switch can leave it.
    assert_eq!(
        app.bring_back(now + Duration::from_secs(2), game, Some(99), steam),
        Some(30)
    );
    // Spaced: a window still down a moment later is not yanked again.
    assert_eq!(
        app.bring_back(
            now + Duration::from_secs(3),
            vec![(30, true)],
            Some(99),
            steam
        ),
        None
    );
    assert_eq!(
        app.bring_back(
            now + Duration::from_secs(5),
            vec![(30, true)],
            Some(99),
            steam
        ),
        Some(30)
    );
    assert_eq!(
        app.bring_back(
            now + Duration::from_secs(9),
            vec![(30, true)],
            Some(99),
            steam
        ),
        Some(30)
    );
    // Three is the limit: a click in the taskbar, not a fight with the game.
    assert_eq!(
        app.bring_back(
            now + Duration::from_secs(12),
            vec![(30, true)],
            Some(99),
            steam
        ),
        None
    );
    assert!(!app.bring_back_open(now + Duration::from_secs(12)));
    // The window that is down is the one brought back.
    let mut other = launching_app();
    assert_eq!(
        other.bring_back(now, vec![(30, false), (31, true)], Some(99), |pid| pid
            == 99),
        Some(31)
    );
}

#[test]
fn a_launched_games_window_is_never_taken_from_another_program_the_user_uses() {
    let mut app = launching_app();
    let now = std::time::Instant::now();
    let game = vec![(30, false)];
    // A program that is no store client's is the user's, and stays in front, even if the game
    // minimised when the user switched to it.
    for foreground in [Some(50), None] {
        assert_eq!(
            app.bring_back(now, game.clone(), foreground, |_| false),
            None
        );
        assert_eq!(
            app.bring_back(
                now + Duration::from_secs(2),
                game.clone(),
                foreground,
                |_| false
            ),
            None
        );
    }
    assert_eq!(
        app.bring_back(
            now + Duration::from_secs(4),
            vec![(30, true)],
            Some(50),
            |_| false
        ),
        None
    );
    // Minimised with only the desktop/shell in front (foreground=None) IS restored.
    assert_eq!(
        app.bring_back(now + Duration::from_secs(6), vec![(30, true)], None, |_| {
            false
        }),
        Some(30)
    );
}

#[test]
fn a_launched_games_window_is_brought_back_only_for_its_own_generation_and_excludes_playnite() {
    let mut app = launching_app();
    let now = std::time::Instant::now();
    assert_eq!(
        app.bring_back_for("wrong-generation", now, vec![(30, true)], None, |_| false),
        None
    );
    let generation = app.generation.clone();
    assert_eq!(
        app.bring_back_for(&generation, now, vec![(30, true)], None, |_| false),
        Some((30, app.name.clone()))
    );
    app.playnite = Some(crate::playnite::Launch::with_game_processes(vec![]));
    assert!(!app.bring_back_open(now));
    assert!(app.focus_snapshot(now).is_none());
    assert!(app.focus_windows().unwrap().is_empty());
}

#[test]
fn a_launched_games_window_is_brought_back_for_a_minute_after_it_was_first_seen() {
    let mut app = launching_app();
    let now = std::time::Instant::now();
    // The window shows up in front: the minute starts then, not at the launch.
    assert_eq!(
        app.bring_back(now, vec![(30, false)], Some(30), |_| false),
        None
    );
    assert_eq!(
        app.bring_back(
            now + Duration::from_secs(59),
            vec![(30, true)],
            Some(99),
            |pid| pid == 99
        ),
        Some(30)
    );
    // Past the minute since the first sight, the game is the user's business.
    assert_eq!(
        app.bring_back(
            now + Duration::from_secs(61),
            vec![(30, true)],
            Some(99),
            |pid| pid == 99
        ),
        None
    );
}

#[test]
fn a_background_game_or_a_program_the_desktop_started_is_never_brought_back() {
    let now = std::time::Instant::now();
    let mut background = launching_app();
    background.background = true;
    assert!(!background.bring_back_open(now));
    assert!(background.focus_windows().unwrap().is_empty());
    assert_eq!(
        background.bring_back(now, vec![(30, true)], Some(99), |_| true),
        None
    );
    let mut not_launching = launching_app();
    not_launching.launches = false;
    assert!(!not_launching.bring_back_open(now));
    // An app that only opens a launcher's own window has no game to bring back,
    // and one whose game never shows up is looked for no longer than a launcher
    // prompt is.
    let mut launcher = launching_app();
    launcher.launcher_only = true;
    assert!(!launcher.bring_back_open(now));
    let mut late = launching_app();
    late.started = std::time::Instant::now()
        - butterpollo_session::launcher_prompt::WATCH_LIMIT
        - Duration::from_secs(1);
    assert!(!late.bring_back_open(now));
    assert_eq!(
        late.bring_back(now, vec![(30, true)], Some(99), |_| true),
        None
    );
    // The desktop starts nothing: no game window to bring back.
    let desktop = RunningApp::with_environment(&App::desktop(), BTreeMap::new()).unwrap();
    assert!(!desktop.bring_back_open(now));
    assert!(desktop.focus_windows().unwrap().is_empty());
    // A program explorer.exe started is the user's, even when a tracker followed it.
    let process = |pid, parent, started, name: &str| butterpollo_session::steam::Process {
        pid,
        parent,
        started,
        name: name.into(),
    };
    let processes = [
        process(10, 1, 100, "explorer.exe"),
        process(20, 10, 500, "player.exe"),
        process(30, 40, 500, "game.exe"),
        process(31, 30, 501, "launcher.exe"),
        process(40, 1, 200, "steam.exe"),
    ];
    let owned = BTreeMap::from([(20, 500), (30, 500)]);
    assert_eq!(
        focus_candidates(&owned, &processes),
        BTreeSet::from([30, 31])
    );
    // A store client or shell in the owned tree (e.g. `steam.exe -applaunch`) is never treated
    // as a game window candidate itself, while its descendant game processes still are.
    let via_steam = BTreeMap::from([(40, 200)]);
    assert_eq!(
        focus_candidates(&via_steam, &processes),
        BTreeSet::from([30, 31])
    );
    // Foreign game processes (e.g. Epic/EA launches) are included in FocusSnapshot
    // candidates, while explorer.exe-started processes stay excluded.
    let mut foreign_app = RunningApp::with_environment(&App::desktop(), BTreeMap::new()).unwrap();
    foreign_app.mark_launching();
    foreign_app.foreign.insert(20, 500);
    foreign_app.foreign.insert(30, 500);
    let snapshot = foreign_app.focus_snapshot(now).unwrap();
    assert_eq!(snapshot.candidates_in(&processes), BTreeSet::from([30, 31]));
}
