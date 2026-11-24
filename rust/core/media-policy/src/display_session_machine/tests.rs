//! Unit tests for the pure display session lifecycle state machine covering
//! launch, pause/resume, roam/join, second-screen, and crash-restore transitions.
use super::*;
use crate::display_policy::PrimaryAction;
use std::cell::Cell;

fn join_ask<'a>() -> JoinAsk<'a> {
    JoinAsk {
        client: "b",
        role: SessionRole::Stream,
        resumes: true,
        launched: Some(7),
        virtual_mode: true,
        display_mode: "per_client",
        identity: "b",
        claim_live: false,
        second_screen: false,
    }
}

fn joined_game(display: Option<&str>) -> Option<JoinedGame<'_>> {
    Some(JoinedGame {
        id: 7,
        display,
        background: false,
        desktop: false,
    })
}

#[test]
fn launch_pause_resume_and_restore_phase_transitions() {
    let start = Instant::now();
    let mut machine = DisplaySessionMachine::launch(start, 1, 2, false, false);
    assert_eq!(machine.phase, SessionPhase::Active { streams: 1 });
    assert_eq!(machine.paused_tick(), PausedTickAction::KeepAliveOnly);

    // Concurrent second transport attaches: detaching one does not pause or capture preferences.
    assert_eq!(machine.attach_stream(), SessionPhase::Active { streams: 2 });
    let first_detach = machine.detach_stream();
    assert!(!first_detach.capture_chosen_preferences);
    assert_eq!(first_detach.phase, SessionPhase::Active { streams: 1 });

    // Last transport disconnects (1 -> 0): pauses and captures chosen preferences once.
    let last_detach = machine.detach_stream();
    assert!(last_detach.capture_chosen_preferences);
    assert_eq!(last_detach.phase, SessionPhase::Paused);

    // Repeated detach while already paused never captures again.
    let while_paused = machine.detach_stream();
    assert!(!while_paused.capture_chosen_preferences);
    assert_eq!(while_paused.phase, SessionPhase::Paused);

    // Hold layout while paused opt-in switches paused tick policy.
    machine.hold_layout_while_paused = true;
    assert_eq!(
        machine.paused_tick(),
        PausedTickAction::KeepAliveAndHoldLayout
    );

    // Resume returns to Active(1); final teardown marks Restored.
    assert_eq!(machine.attach_stream(), SessionPhase::Active { streams: 1 });
    machine.mark_restored();
    assert_eq!(machine.phase, SessionPhase::Restored);
    assert!(!machine.recovery.pending);
}

#[test]
fn roam_continue_here_and_multiclient_join_rules() {
    // Stream-here-too joins the running game's display.
    assert_eq!(
        joined_display(&join_ask(), joined_game(Some("a"))).as_deref(),
        Some("a")
    );
    // Continue Here (`claim_live`) gives the new owner a fresh display rather than joining.
    assert_eq!(
        joined_display(
            &JoinAsk {
                claim_live: true,
                ..join_ask()
            },
            joined_game(Some("a"))
        ),
        None
    );
    // Roaming ownership: game follows its new owner unless that owner joined another display.
    assert_eq!(
        game_display_after(Some("a"), true, true, false, Some("b")).as_deref(),
        Some("b")
    );
    assert_eq!(
        game_display_after(Some("a"), true, true, true, Some("b")).as_deref(),
        Some("a")
    );
    // Dead display: next stream's own display inherits the game even without ownership.
    assert_eq!(
        game_display_after(Some("a"), false, false, false, Some("b")).as_deref(),
        Some("b")
    );
    // Background, desktop, remote monitor, physical or shared mode never join.
    assert_eq!(
        joined_display(
            &join_ask(),
            Some(JoinedGame {
                background: true,
                ..joined_game(Some("a")).unwrap()
            })
        ),
        None
    );
    assert_eq!(
        joined_display(
            &join_ask(),
            Some(JoinedGame {
                desktop: true,
                ..joined_game(Some("a")).unwrap()
            })
        ),
        None
    );
    assert_eq!(
        joined_display(
            &JoinAsk {
                role: SessionRole::RemoteMonitor,
                ..join_ask()
            },
            joined_game(Some("a"))
        ),
        None
    );
    assert_eq!(
        joined_display(
            &JoinAsk {
                virtual_mode: false,
                ..join_ask()
            },
            joined_game(Some("a"))
        ),
        None
    );
    assert_eq!(
        joined_display(
            &JoinAsk {
                display_mode: "shared",
                ..join_ask()
            },
            joined_game(Some("a"))
        ),
        None
    );
}

#[test]
fn second_screen_isolates_virtual_and_physical_displays() {
    // A second screen never joins the first screen's game display even when resuming the game.
    let second = JoinAsk {
        second_screen: true,
        ..join_ask()
    };
    assert_eq!(joined_display(&second, joined_game(Some("a"))), None);
    assert_eq!(joined_display(&second, joined_game(Some("b"))), None);

    // Physical dual-screen picks the first non-virtual monitor other than the first screen's.
    let monitors = [
        (r"\\.\DISPLAY1", false),
        (r"\\.\DISPLAY7", true),
        (r"\\.\DISPLAY2", false),
    ];
    assert_eq!(
        pick_second_monitor(&monitors, Some(r"\\.\DISPLAY1")),
        Some(r"\\.\DISPLAY2")
    );
    assert_eq!(
        pick_second_monitor(&monitors, Some(r"\\.\display2")),
        Some(r"\\.\DISPLAY1")
    );
    assert_eq!(
        pick_second_monitor(
            &[(r"\\.\DISPLAY1", false), (r"\\.\DISPLAY7", true)],
            Some(r"\\.\DISPLAY1")
        ),
        None
    );
}

#[test]
fn crash_restore_baseline_journal_records_once_and_restores_on_last_release() {
    let mut journal = BaselineJournal::new();
    let recorded = Cell::new(0);
    let restored = Cell::new(0);

    // Failed first record leaves zero users.
    assert!(journal.acquire(|| anyhow::bail!("journal locked")).is_err());
    assert_eq!(journal.users(), 0);

    journal
        .acquire(|| {
            recorded.set(recorded.get() + 1);
            Ok("golden-snapshot")
        })
        .unwrap();
    journal
        .acquire(|| panic!("second session joins existing journal"))
        .unwrap();
    assert_eq!(journal.users(), 2);
    assert_eq!(recorded.get(), 1);

    journal.release(|_| restored.set(restored.get() + 1));
    assert_eq!(journal.users(), 1);
    assert_eq!(restored.get(), 0);

    journal.release(|snap| {
        assert_eq!(snap, "golden-snapshot");
        restored.set(restored.get() + 1);
    });
    assert_eq!(journal.users(), 0);
    assert_eq!(restored.get(), 1);
}

#[test]
fn recovery_retries_layout_every_second_but_sets_mode_once() {
    let start = Instant::now();
    let mut machine = DisplaySessionMachine::launch(start, 0, 0, false, false);
    assert!(!machine.recovery.due(start));

    machine.begin_recovery(start);
    assert_eq!(machine.phase, SessionPhase::Recovering { streams: 1 });
    assert_eq!(machine.phase.active_streams(), 1);
    assert!(machine.recovery.due(start));
    assert!(machine.recovery.take_mode());

    // Retry cadence is 1s; mode is not retried after first attempt.
    assert!(!machine.recovery.due(start + Duration::from_millis(200)));
    assert!(machine.recovery.due(start + Duration::from_secs(1)));
    assert!(!machine.recovery.take_mode());

    machine.finish_recovery();
    assert_eq!(machine.phase, SessionPhase::Active { streams: 1 });
    assert!(!machine.recovery.due(start + Duration::from_secs(5)));
}

#[test]
fn recovery_preserves_stream_count_across_multistream_attach_and_detach() {
    let start = Instant::now();
    let mut machine = DisplaySessionMachine::launch(start, 0, 0, false, false);
    assert_eq!(machine.attach_stream(), SessionPhase::Active { streams: 2 });

    // Recovery preserves the count of attached streams.
    machine.begin_recovery(start);
    assert_eq!(machine.phase, SessionPhase::Recovering { streams: 2 });

    // Detaching one of two streams during recovery must not pause or capture preferences.
    let first_detach = machine.detach_stream();
    assert!(!first_detach.capture_chosen_preferences);
    assert_eq!(first_detach.phase, SessionPhase::Recovering { streams: 1 });

    // Attaching another stream during recovery increments the count instead of resetting to 1.
    assert_eq!(
        machine.attach_stream(),
        SessionPhase::Recovering { streams: 2 }
    );

    // Completing recovery restores Active with both streams intact.
    machine.finish_recovery();
    assert_eq!(machine.phase, SessionPhase::Active { streams: 2 });

    // Re-enter recovery with 2 streams, detach both: only the 1->0 transition captures.
    machine.begin_recovery(start + Duration::from_secs(2));
    assert_eq!(machine.phase, SessionPhase::Recovering { streams: 2 });
    let d1 = machine.detach_stream();
    assert!(!d1.capture_chosen_preferences);
    assert_eq!(d1.phase, SessionPhase::Recovering { streams: 1 });
    let d2 = machine.detach_stream();
    assert!(d2.capture_chosen_preferences);
    assert_eq!(d2.phase, SessionPhase::Paused);
    assert!(machine.recovery.pending);

    // Reattaching while recovery is still pending enters Recovering { streams: 1 }.
    assert_eq!(
        machine.attach_stream(),
        SessionPhase::Recovering { streams: 1 }
    );
    machine.finish_recovery();
    assert_eq!(machine.phase, SessionPhase::Active { streams: 1 });
}

#[test]
fn recovery_while_paused_keeps_paused_phase_until_resume_attaches() {
    let start = Instant::now();
    let mut machine = DisplaySessionMachine::launch(start, 0, 0, false, false);
    assert!(machine.detach_stream().capture_chosen_preferences);
    assert_eq!(machine.phase, SessionPhase::Paused);

    // Resume runs finish_feed() while paused (0 active streams) before attach_stream().
    // Completing recovery before attach must stay Paused so a later resume failure does
    // not leave phase stuck Active, and a successful attach lands at Active { streams: 1 }.
    machine.begin_recovery(start);
    assert_eq!(machine.phase, SessionPhase::Paused);
    assert!(machine.recovery.pending);
    machine.finish_recovery();
    assert_eq!(machine.phase, SessionPhase::Paused);
    assert!(!machine.recovery.pending);

    // Once resume succeeds and attaches, phase reaches Active { streams: 1 } (not 2).
    assert_eq!(machine.attach_stream(), SessionPhase::Active { streams: 1 });
    let detach = machine.detach_stream();
    assert!(detach.capture_chosen_preferences);
    assert_eq!(detach.phase, SessionPhase::Paused);
}

#[test]
fn paused_desktop_change_is_buffered_and_armed_at_resume() {
    let t0 = Instant::now();
    let mut machine = DisplaySessionMachine::launch(t0, 0, 0, true, false);
    machine.shown_primary = Some(true);
    machine.primary_watch.settled(t0);

    // Pause session and observe a departure while paused.
    machine.detach_stream();
    let t1 = t0 + Duration::from_secs(2);
    assert!(machine.notice_departure(1, t1));
    assert!(machine.departure_unsettled(1));
    let change = machine.observe_desktop_change(false, 1, true, false, t1);
    assert_eq!(change, Some(PrimaryChange::Display));

    // While paused, prepare_primary_check buffers the change and does not probe.
    assert!(!machine.prepare_primary_check(t1, change, false, false, Arrangement::Primary, true));
    assert_eq!(machine.paused_change, Some(PrimaryChange::Display));

    // Resume arms the buffered change and allows primary watch restore after settle.
    let t2 = t1 + Duration::from_secs(10);
    machine.attach_stream();
    assert!(machine.prepare_primary_check(t2, None, true, false, Arrangement::Primary, true));
    assert_eq!(machine.paused_change, None);
    assert_eq!(machine.primary_watch.observe(true, t2), PrimaryAction::Wait);
    assert_eq!(
        machine
            .primary_watch
            .observe(true, t2 + LayoutWatch::SETTLE),
        PrimaryAction::Restore
    );
    assert!(machine.take_start_primary_mismatch());
    assert!(!machine.take_start_primary_mismatch());
}

#[test]
fn browser_layout_and_mode_compatibility_decisions() {
    assert!(browser_keeps_monitors(
        SessionTransport::WebRtc,
        true,
        "exclusive",
        true
    ));
    assert!(!browser_keeps_monitors(
        SessionTransport::Moonlight,
        true,
        "exclusive",
        true
    ));
    assert!(!browser_keeps_monitors(
        SessionTransport::WebRtc,
        true,
        "extended",
        true
    ));
    assert_eq!(
        layout_owner(
            SessionRole::Stream,
            "dev-1",
            true,
            false,
            Some(Arrangement::Extended),
            Some(Arrangement::Extended)
        )
        .as_deref(),
        Some("dev-1")
    );
    assert_eq!(
        layout_owner(
            SessionRole::Stream,
            "dev-1",
            true,
            true,
            Some(Arrangement::Primary),
            Some(Arrangement::Primary)
        ),
        None
    );

    let mode = (2560, 1440, 60_000, true, false);
    assert!(mode_resumes(mode, (2560, 1440, 120_000, true, false)));
    assert!(!mode_resumes(mode, (1920, 1080, 120_000, true, false)));
    assert!(!mode_resumes(mode, (2560, 1440, 60_000, false, false)));
    assert!(!mode_resumes(mode, (2560, 1440, 60_000, true, true)));

    assert!(display_serves(
        Some(HostDisplay::Virtual(Arrangement::Exclusive)),
        Some("exclusive"),
        Some(HostDisplay::Virtual(Arrangement::Exclusive)),
        "exclusive"
    ));
    assert!(!display_serves(
        Some(HostDisplay::Virtual(Arrangement::Exclusive)),
        Some("exclusive"),
        Some(HostDisplay::Physical),
        "exclusive"
    ));
    assert!(display_shows_game(Some("a"), Some("a")));
    assert!(!display_shows_game(Some("b"), Some("a")));
    assert!(display_shows_game(None, Some("a")));

    assert_eq!(
        display_changes(false, true, true),
        DisplayChanges {
            hdr_profile: true,
            scale: true,
            mode: true
        }
    );
    assert_eq!(
        display_changes(true, true, true),
        DisplayChanges {
            hdr_profile: false,
            scale: false,
            mode: false
        }
    );
    assert_eq!(
        display_ask(
            Some((2560, 1440, 120_000)),
            (1280, 720),
            Rate(60_000),
            Some(true)
        ),
        ((2560, 1440), Rate(120_000), None)
    );
    assert_eq!(
        joined_hdr_mismatch(true, Some(true), Some(false)),
        Some(true)
    );
    assert_eq!(
        resumed_profile(true, None, Some("oled")),
        Some(Some("oled".into()))
    );
    assert_eq!(resumed_profile(true, Some("oled"), None), Some(None));
    assert!(should_retry_desktop_refused(true, false));
    assert!(!should_retry_desktop_refused(true, true));
    assert!(brings_back_game(false, true));
    assert!(!brings_back_game(true, true));
    assert!(!brings_back_game(false, false));
}
