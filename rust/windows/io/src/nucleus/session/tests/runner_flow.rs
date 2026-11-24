//! Tests for runner profile clicking, Play button pressing, window focus,
//! instance tracking, and post-click form handling.
use super::*;

/// #59: a one-player game without ProtoInput's hooks takes its pad only in
/// front (Black Flag waited for a click on its taskbar icon): its window is
/// brought there as it shows, once per window, so the person at the PC can
/// take the focus back. A hooked game's never is.
#[test]
fn an_unhooked_game_is_brought_to_the_front_once_as_it_shows() {
    for to_front in [true, false] {
        let t0 = Instant::now();
        let (screen, probe) = (screen(), Probe::default());
        let mut runner = runner(1, &screen, &probe, t0);
        runner.to_front = to_front;
        label(&screen);
        assert_eq!(runner.step(t0), None);
        instance(&screen, 0);
        for _ in 0..3 {
            assert_eq!(runner.step(t0), None);
        }
        let fronted = screen.0.lock().unwrap().fronted.clone();
        if !to_front {
            assert!(fronted.is_empty());
            continue;
        }
        assert_eq!(fronted, [200]);
        // The game's next window, after its splash screen.
        {
            let mut desktop = screen.0.lock().unwrap();
            desktop.windows.clear();
            desktop.windows.push(window(201, 20, ON_SCREEN));
        }
        for _ in 0..3 {
            assert_eq!(runner.step(t0), None);
        }
        assert_eq!(screen.0.lock().unwrap().fronted, [200, 201]);
        // Back to the first window: it was brought there already.
        {
            let mut desktop = screen.0.lock().unwrap();
            desktop.windows.clear();
            desktop.windows.push(window(200, 20, ON_SCREEN));
        }
        assert_eq!(runner.step(t0), None);
        assert_eq!(screen.0.lock().unwrap().fronted, [200, 201]);
    }
}

/// Someone using the PC keeps their focus: the game's window comes to the
/// front only once nobody does.
#[test]
fn an_unhooked_game_waits_for_nobody_to_use_the_pc() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    runner.to_front = true;
    screen.0.lock().unwrap().pc_busy = true;
    label(&screen);
    assert_eq!(runner.step(t0), None);
    instance(&screen, 0);
    for _ in 0..3 {
        assert_eq!(runner.step(t0), None);
    }
    assert!(screen.0.lock().unwrap().fronted.is_empty());
    screen.0.lock().unwrap().pc_busy = false;
    assert_eq!(runner.step(t0), None);
    assert_eq!(screen.0.lock().unwrap().fronted, [200]);
}

/// #59: each instance is told from the pass that finds it, before its
/// window shows and before a stream looks for its sound, so its sound can
/// leave the PC's device before the game opens it; a process it starts is
/// told with it. Their end is told before Nucleus stops them, so their
/// devices can be put back while they run.
#[test]
fn the_instances_are_told_from_the_pass_that_finds_them() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    let told = Arc::new(Listener {
        probe: probe.clone(),
        ..Default::default()
    });
    runner.instances = Some(told.clone());
    let last = || told.found.lock().unwrap().last().cloned().unwrap();
    label(&screen);
    assert_eq!(runner.step(t0), None);
    assert_eq!(last(), vec![BTreeSet::new()]);
    let add = |pid, parent, started, name| {
        let mut desktop = screen.0.lock().unwrap();
        desktop.processes.push(process(pid, parent, started, name));
    };
    add(20, NUCLEUS, SINCE + 1, "Game.exe");
    assert_eq!(runner.step(t0), None);
    let game = Identity {
        pid: 20,
        started: SINCE + 1,
    };
    assert!(screen.0.lock().unwrap().windows.is_empty());
    assert_eq!(last(), vec![BTreeSet::from([game])]);
    add(21, 20, SINCE + 2, "GameHelper.exe");
    assert_eq!(runner.step(t0), None);
    let helper = Identity {
        pid: 21,
        started: SINCE + 2,
    };
    assert_eq!(last(), vec![BTreeSet::from([game, helper])]);
    runner.finish(End::Ended);
    assert_eq!(*told.stopping.lock().unwrap(), Some((vec![20, 21], false)));
    assert!(*told.stopped.lock().unwrap());
    assert!(probe.stopped.lock().unwrap().is_some());
}

#[test]
fn the_profile_is_clicked_and_the_session_runs_with_every_window() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(2, &screen, &probe, t0);
    assert_eq!(runner.step(t0), None);
    assert!(screen.0.lock().unwrap().clicks.is_empty());
    label(&screen);
    assert_eq!(runner.step(t0), None);
    assert_eq!(screen.0.lock().unwrap().clicks, vec![7]);
    // Clicked once only.
    assert_eq!(runner.step(t0), None);
    assert_eq!(screen.0.lock().unwrap().clicks.len(), 1);
    instance(&screen, 0);
    assert_eq!(runner.step(t0), None);
    assert_eq!(runner.shared.session.lock().unwrap().phase, Phase::Starting);
    instance(&screen, 1);
    assert_eq!(runner.step(t0), None);
    let session = runner.shared.session.lock().unwrap().clone();
    assert_eq!(session.phase, Phase::Running);
    assert_eq!(session.slots[0].window, Some(200));
    assert_eq!(session.slots[1].window, Some(210));
    assert!(runner.shared.roots.lock().unwrap()[1].is_some_and(|r| r.pid == 21));
    // Both windows were parked beside the desktop.
    let placed = screen.0.lock().unwrap().placed.clone();
    assert!(
        placed
            .iter()
            .any(|(id, to)| *id == 210 && i64::from(to.x) > ON_SCREEN.right())
    );
    // Running, the session never times out.
    assert_eq!(runner.step(t0 + 10 * START_TIMEOUT), None);
}

/// #59: a click Nucleus gets while it still makes its setup screen does
/// not load the profile. While it lists the profile and no instance
/// runs, the profile is clicked again; never once it hides the list to
/// play, nor once an instance runs.
#[test]
fn the_profile_is_clicked_again_while_nucleus_still_lists_it() {
    let t0 = Instant::now();
    let clicks = |screen: &Screen| screen.0.lock().unwrap().clicks.len();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    label(&screen);
    assert_eq!(runner.step(t0), None);
    assert_eq!(runner.step(t0 + RECLICK - PERIOD), None);
    assert_eq!(clicks(&screen), 1);
    assert_eq!(runner.step(t0 + RECLICK), None);
    assert_eq!(clicks(&screen), 2);
    // Nucleus plays: the list is gone. A list that comes back before
    // the instance is not clicked: that could start the game twice.
    screen.0.lock().unwrap().labels.clear();
    assert_eq!(runner.step(t0 + 2 * RECLICK), None);
    label(&screen);
    assert_eq!(runner.step(t0 + 3 * RECLICK), None);
    assert_eq!(runner.step(t0 + 5 * RECLICK), None);
    assert_eq!(clicks(&screen), 2);
    instance(&screen, 0);
    assert_eq!(runner.step(t0 + 6 * RECLICK), None);
    assert_eq!(runner.shared.session.lock().unwrap().phase, Phase::Running);
    assert_eq!(runner.step(t0 + 8 * RECLICK), None);
    assert_eq!(clicks(&screen), 2);
}

/// #59: a profile that plays by itself starts the game twice when Nucleus
/// paints during the start. Rubylight's doesn't: Play is pressed, as soon as
/// Nucleus shows it and exactly once, even if Nucleus takes its time.
#[test]
fn play_is_pressed_once_after_the_profile_is_loaded() {
    const BUTTON: u64 = 9;
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    runner.plays_itself = false;
    let clicks = |screen: &Screen| screen.0.lock().unwrap().clicks.clone();
    label(&screen);
    assert_eq!(runner.step(t0), None);
    assert_eq!(clicks(&screen), vec![7]);
    // No Play yet: a player has no controller. Nothing to press.
    assert_eq!(runner.step(t0 + PERIOD), None);
    assert_eq!(clicks(&screen), vec![7]);
    screen
        .0
        .lock()
        .unwrap()
        .buttons
        .push((NUCLEUS, PLAY.into(), BUTTON));
    assert_eq!(runner.step(t0 + 2 * PERIOD), None);
    assert_eq!(clicks(&screen), vec![7, BUTTON]);
    // Pressed again it is Stop, and the profile clicked again reloads it.
    assert_eq!(runner.step(t0 + 2 * RECLICK), None);
    assert_eq!(runner.step(t0 + 4 * RECLICK), None);
    assert_eq!(clicks(&screen), vec![7, BUTTON]);
    instance(&screen, 0);
    assert_eq!(runner.step(t0 + 5 * RECLICK), None);
    assert_eq!(runner.shared.session.lock().unwrap().phase, Phase::Running);
    assert_eq!(clicks(&screen), vec![7, BUTTON]);
}

/// A profile that plays by itself shows Play as well: pressing it would be
/// the second start.
#[test]
fn play_is_not_pressed_for_a_profile_that_plays_by_itself() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    label(&screen);
    screen
        .0
        .lock()
        .unwrap()
        .buttons
        .push((NUCLEUS, PLAY.into(), 9));
    assert_eq!(runner.step(t0), None);
    assert_eq!(runner.step(t0 + PERIOD), None);
    assert_eq!(screen.0.lock().unwrap().clicks, vec![7]);
}

/// Play under another window is pressed when it can be, and the start that
/// never comes after a press says so.
#[test]
fn play_covered_is_pressed_later_and_a_press_that_starts_nothing_says_so() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    runner.plays_itself = false;
    label(&screen);
    assert_eq!(runner.step(t0), None);
    {
        let mut desktop = screen.0.lock().unwrap();
        desktop.buttons.push((NUCLEUS, PLAY.into(), 9));
        desktop.covered = true;
    }
    assert_eq!(runner.step(t0 + PERIOD), None);
    assert!(!runner.played);
    screen.0.lock().unwrap().covered = false;
    assert_eq!(runner.step(t0 + 2 * PERIOD), None);
    assert!(runner.played);
    let End::Failed(why) = runner.step(t0 + START_TIMEOUT).unwrap() else {
        panic!("no failure");
    };
    assert!(why.contains("pressed Play"), "{why}");
    // Covered until the start times out: that is why, not a press that started nothing.
    let (screen, probe) = (super::screen(), Probe::default());
    let mut runner = super::runner(1, &screen, &probe, t0);
    runner.plays_itself = false;
    label(&screen);
    assert_eq!(runner.step(t0), None);
    {
        let mut desktop = screen.0.lock().unwrap();
        desktop.buttons.push((NUCLEUS, PLAY.into(), 9));
        desktop.covered = true;
    }
    assert_eq!(runner.step(t0 + PERIOD), None);
    assert_eq!(
        runner.step(t0 + START_TIMEOUT),
        Some(End::Failed(
            "Nucleus Co-op did not start Game: covered".into()
        ))
    );
    assert!(!runner.played);
}

/// With no Play to press yet, the setup screen is named by what comes next there.
#[test]
fn a_profile_waiting_for_play_says_so() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    runner.plays_itself = false;
    label(&screen);
    assert_eq!(runner.step(t0), None);
    assert_eq!(runner.step(t0 + RECLICK), None);
    let End::Failed(why) = runner.step(t0 + START_TIMEOUT).unwrap() else {
        panic!("no failure");
    };
    assert!(
        why.contains("which shows Play once every player has a controller there"),
        "{why}"
    );
}

/// #56: Nucleus 2.4.2 can't load a profile clicked before its setup screen
/// found the PC's screens, a 3 s timer. The profile is clicked once Nucleus
/// listed it for [`SETTLE`], counted from when it is first seen.
#[test]
fn the_profile_is_clicked_once_nucleus_listed_it_for_a_while() {
    let t0 = Instant::now();
    let clicks = |screen: &Screen| screen.0.lock().unwrap().clicks.len();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    runner.settle = SETTLE;
    assert_eq!(runner.step(t0), None);
    let listed = t0 + 10 * PERIOD;
    label(&screen);
    assert_eq!(runner.step(listed), None);
    assert_eq!(runner.step(listed + SETTLE - PERIOD), None);
    assert_eq!(clicks(&screen), 0);
    assert_eq!(runner.step(listed + SETTLE), None);
    assert_eq!(clicks(&screen), 1);
}

/// A box Nucleus shows after the click (its End-key reminder) holds the
/// start: no click goes through it until it is answered.
#[test]
fn a_box_nucleus_shows_holds_the_second_click() {
    let t0 = Instant::now();
    let clicks = |screen: &Screen| screen.0.lock().unwrap().clicks.len();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    label(&screen);
    assert_eq!(runner.step(t0), None);
    let reminder = MessageBox {
        window: 400,
        caption: "Nucleus Co-op".into(),
        text: "Press End to unlock the inputs.".into(),
    };
    screen.0.lock().unwrap().boxes.push((NUCLEUS, reminder));
    assert_eq!(runner.step(t0 + RECLICK), None);
    assert_eq!(runner.step(t0 + 2 * RECLICK), None);
    assert_eq!(clicks(&screen), 1);
    screen.0.lock().unwrap().boxes.clear();
    assert_eq!(runner.step(t0 + 3 * RECLICK), None);
    assert_eq!(clicks(&screen), 2);
}

/// Looking for the list again after the click goes through the passes
/// that may fail, not ending the start at once.
#[test]
fn a_failed_look_for_the_list_after_the_click_fails_the_pass() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    label(&screen);
    assert_eq!(runner.step(t0), None);
    // Two entries named alike: the fake's lookup fails.
    label(&screen);
    for pass in 1..FAILED_PASSES {
        assert_eq!(runner.step(t0 + pass * PERIOD), None);
    }
    assert_eq!(
        runner.step(t0 + FAILED_PASSES * PERIOD),
        Some(End::Failed(
            "looking for Nucleus Co-op's profile list: two labels".into()
        ))
    );
}

/// #59: Nucleus 2.4.2 answers a click that comes before its setup screen
/// is made with "The profile can't be loaded", a form that disables its
/// window until someone closes it, so no later click lands. The session
/// closes it and clicks the profile again; one Nucleus shows before any
/// click is left alone.
#[test]
fn a_box_nucleus_shows_after_the_click_is_closed_and_the_profile_clicked_again() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    let form = |covered: bool| {
        let mut desktop = screen.0.lock().unwrap();
        desktop.forms.push((NUCLEUS, unloaded(300)));
        desktop.covered = covered;
    };
    form(true);
    assert_eq!(runner.step(t0), None);
    assert!(screen.0.lock().unwrap().closed.is_empty());
    screen.0.lock().unwrap().forms.clear();
    screen.0.lock().unwrap().covered = false;
    label(&screen);
    assert_eq!(runner.step(t0 + PERIOD), None);
    assert_eq!(screen.0.lock().unwrap().clicks.len(), 1);
    // The click failed: the box covers the window.
    form(true);
    assert_eq!(runner.step(t0 + 2 * PERIOD), None);
    assert_eq!(screen.0.lock().unwrap().closed, [300]);
    screen.0.lock().unwrap().covered = false;
    assert_eq!(runner.step(t0 + PERIOD + RECLICK), None);
    assert_eq!(screen.0.lock().unwrap().clicks.len(), 2);
    instance(&screen, 0);
    assert_eq!(runner.step(t0 + PERIOD + RECLICK + PERIOD), None);
    assert_eq!(runner.shared.session.lock().unwrap().phase, Phase::Running);

    // A box after every click ends the start saying so, counting each
    // box once though it takes passes to close.
    let (screen, probe) = (super::screen(), Probe::default());
    let mut runner = super::runner(1, &screen, &probe, t0);
    label(&screen);
    let mut at = t0;
    while at < t0 + START_TIMEOUT {
        let window = 300 + screen.0.lock().unwrap().clicks.len() as u64;
        screen
            .0
            .lock()
            .unwrap()
            .forms
            .push((NUCLEUS, unloaded(window)));
        assert_eq!(runner.step(at), None);
        at += PERIOD;
    }
    let (clicks, closed) = {
        let desktop = screen.0.lock().unwrap();
        let distinct: BTreeSet<u64> = desktop.closed.iter().copied().collect();
        (desktop.clicks.len(), distinct.len())
    };
    assert!(clicks > 2);
    assert!(screen.0.lock().unwrap().closed.len() > closed);
    assert_eq!(
        runner.step(at),
        Some(End::Failed(format!(
            "Nucleus Co-op did not start Game: after the last click on the profile Rubylight it showed its own untitled message box, whose text can't be read (in Nucleus Co-op 2.4.2 it says the profile can't be loaded, or that the game's folder is missing) over its window ({closed} closed since the launch)"
        )))
    );

    // One early box, then a wait for a controller: the wait is why.
    let (screen, probe) = (super::screen(), Probe::default());
    let mut runner = super::runner(1, &screen, &probe, t0);
    label(&screen);
    assert_eq!(runner.step(t0), None);
    screen
        .0
        .lock()
        .unwrap()
        .forms
        .push((NUCLEUS, unloaded(300)));
    assert_eq!(runner.step(t0 + PERIOD), None);
    assert_eq!(runner.step(t0 + RECLICK), None);
    assert_eq!(screen.0.lock().unwrap().clicks.len(), 2);
    assert_eq!(
        runner.step(t0 + RECLICK + START_TIMEOUT),
        Some(End::Failed(
            "Nucleus Co-op did not start Game: it still shows its setup screen with the profile Rubylight, which starts the game once every player has a controller there".into()
        ))
    );
}

/// What Nucleus 2.4.2 shows for a click before its setup screen found the
/// PC's screens: "The profile can't be loaded" in its own untitled box,
/// which draws the text, so nothing of it reads (#56).
fn unloaded(window: u64) -> MessageBox {
    MessageBox {
        window,
        caption: String::new(),
        text: String::new(),
    }
}

/// #59: a profile Nucleus loaded but never played says so, not to load
/// it by hand: Nucleus plays it once each player has a controller.
#[test]
fn a_profile_nucleus_does_not_play_says_where_it_stopped() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    label(&screen);
    assert_eq!(runner.step(t0), None);
    assert_eq!(runner.step(t0 + RECLICK), None);
    assert_eq!(
        runner.step(t0 + START_TIMEOUT),
        Some(End::Failed(
            "Nucleus Co-op did not start Game: it still shows its setup screen with the profile Rubylight, which starts the game once every player has a controller there".into()
        ))
    );
    assert!(screen.0.lock().unwrap().clicks.len() > 1);
    // It hid the list, and no instance came.
    let (screen, probe) = (super::screen(), Probe::default());
    let mut runner = super::runner(1, &screen, &probe, t0);
    label(&screen);
    assert_eq!(runner.step(t0), None);
    screen.0.lock().unwrap().labels.clear();
    assert_eq!(runner.step(t0 + RECLICK), None);
    assert_eq!(
        runner.step(t0 + START_TIMEOUT),
        Some(End::Failed(
            "Nucleus Co-op did not start Game after loading the profile Rubylight".into()
        ))
    );
}

#[test]
fn no_profile_entry_in_time_fails_says_what_to_do_and_stops_nucleus() {
    let t0 = Instant::now();
    let (screen, probe) = (screen(), Probe::default());
    let mut runner = runner(1, &screen, &probe, t0);
    assert_eq!(runner.step(t0 + START_TIMEOUT / 2), None);
    let end = runner.step(t0 + START_TIMEOUT).unwrap();
    assert_eq!(end, End::Failed("Nucleus Co-op showed no window".into()));
    let shared = runner.shared.clone();
    runner.finish(end.clone());
    assert_eq!(shared.session.lock().unwrap().phase, Phase::Ended(end));
    assert_eq!(*probe.stopped.lock().unwrap(), Some(BTreeMap::new()));
}
