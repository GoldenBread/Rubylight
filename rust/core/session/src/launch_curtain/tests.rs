//! The curtain's way down, the stuck-curtain paths first.
use super::*;

fn at(seconds: f32) -> Look {
    Look {
        elapsed: Duration::from_secs_f32(seconds),
        app_running: true,
        now_unix_ms: 1_000 + (seconds * 1000.0) as u64,
        ..Default::default()
    }
}

fn notice(kind: LauncherKind) -> LauncherNotice {
    LauncherNotice {
        launcher: "steam".into(),
        kind,
        window_title: "Steam Guard - Computer Authorization Required".into(),
        since_unix_ms: 777,
    }
}

#[test]
fn launch_curtain_times_out_in_every_phase_with_a_reason() {
    let stuck = [
        Look::default(),
        Look {
            game_up: true,
            ..Default::default()
        },
        Look {
            window: GameWindow::BehindOther,
            ..Default::default()
        },
        Look {
            window: GameWindow::Minimized,
            ..Default::default()
        },
        Look {
            launcher: Some(notice(LauncherKind::Update)),
            ..Default::default()
        },
        Look {
            secure_desktop: true,
            ..Default::default()
        },
    ];
    for look in stuck {
        let mut curtain = Curtain::new(false);
        let mut t = 0.0;
        while t < 44.9 {
            curtain.look(&at(t).with(&look));
            assert!(curtain.active(), "{look:?} lifted at {t}");
            t += 0.25;
        }
        assert!(curtain.look(&at(45.0).with(&look)), "{look:?}");
        assert_eq!(curtain.state().phase, Phase::TimedOut, "{look:?}");
        assert_eq!(curtain.state().reason, REASON_TIMED_OUT);
        assert!(!curtain.active());
        assert!(!curtain.state().has(flags::DISMISSIBLE));
        // Lifted for good: nothing brings it back.
        assert!(!curtain.look(&at(46.0)));
        assert!(!curtain.dismiss(Duration::from_secs(47)));
        assert_eq!(curtain.state().phase, Phase::TimedOut);
    }
}

impl Look {
    /// `self`'s time with what `other` saw.
    fn with(&self, other: &Look) -> Look {
        Look {
            elapsed: self.elapsed,
            now_unix_ms: self.now_unix_ms,
            app_running: self.app_running,
            ..other.clone()
        }
    }
}

#[test]
fn launch_curtain_fails_when_the_app_ends() {
    let mut curtain = Curtain::new(false);
    assert!(curtain.look(&Look {
        app_running: false,
        ..at(3.0)
    }));
    assert_eq!(curtain.state().phase, Phase::Failed);
    assert_eq!(curtain.state().reason, REASON_APP_ENDED);
    assert!(!curtain.active());
}

#[test]
fn launch_curtain_lifts_when_the_device_dismisses_it() {
    let mut curtain = Curtain::new(false);
    curtain.look(&Look {
        launcher: Some(notice(LauncherKind::SignIn)),
        ..at(1.0)
    });
    let before = curtain.state().sequence;
    assert!(curtain.dismiss(Duration::from_secs(2)));
    assert_eq!(curtain.state().phase, Phase::Dismissed);
    assert!(curtain.state().sequence > before);
    assert!(!curtain.active());
    assert!(!curtain.dismiss(Duration::from_secs(3)), "once");
}

#[test]
fn launch_curtain_lifts_after_two_looks_with_the_game_in_front() {
    let mut curtain = Curtain::new(false);
    assert_eq!(curtain.state().phase, Phase::StartingApp);
    assert!(curtain.look(&Look {
        game_up: true,
        ..at(1.0)
    }));
    assert_eq!(curtain.state().phase, Phase::WaitingWindow);
    assert!(curtain.look(&Look {
        game_up: true,
        window: GameWindow::BehindClient,
        ..at(2.0)
    }));
    assert_eq!(curtain.state().phase, Phase::FocusingWindow);
    assert!(curtain.state().has(flags::FIRST_FRAME_SEEN));
    curtain.look(&Look {
        game_up: true,
        window: GameWindow::Front,
        ..at(2.25)
    });
    assert!(curtain.active(), "one look in front is not enough");
    // A launcher taking the front again starts the count over.
    curtain.look(&Look {
        game_up: true,
        window: GameWindow::BehindClient,
        ..at(2.5)
    });
    curtain.look(&Look {
        game_up: true,
        window: GameWindow::Front,
        ..at(2.75)
    });
    assert!(curtain.active());
    assert!(curtain.look(&Look {
        game_up: true,
        window: GameWindow::Front,
        ..at(3.0)
    }));
    assert_eq!(curtain.state().phase, Phase::Ready);
    assert!(!curtain.active());
    assert!(!curtain.state().has(flags::DISMISSIBLE));
    assert!(curtain.state().has(flags::FIRST_FRAME_SEEN));
}

#[test]
fn launch_curtain_bypass_is_lifted_from_the_start() {
    let curtain = Curtain::new(true);
    assert_eq!(curtain.state().phase, Phase::Ready);
    assert!(curtain.state().has(flags::DESKTOP_OR_LAUNCHER));
    assert!(!curtain.active());
    assert_eq!(curtain.state().sequence, 1);
}

#[test]
fn launch_curtain_launcher_prompt_asks_for_the_player_and_clears() {
    let mut curtain = Curtain::new(false);
    assert!(curtain.look(&Look {
        launcher: Some(notice(LauncherKind::SignIn)),
        ..at(4.0)
    }));
    let state = curtain.state().clone();
    assert_eq!(state.phase, Phase::AttentionNeeded);
    assert_eq!(state.prompt, PromptKind::SignIn);
    assert_eq!(state.launcher, "steam");
    assert!(state.has(flags::DISMISSIBLE));
    let owner = curtain.snapshot(3, true);
    assert_eq!(owner.phase, CurtainPhase::AttentionNeeded);
    assert!(owner.dismissible);
    let prompt = owner.prompt.unwrap();
    assert_eq!(prompt.kind, LauncherKind::SignIn);
    assert_eq!(prompt.since_unix_ms, 777);
    assert_eq!(prompt.window_title, state.title);
    let guest = curtain.snapshot(3, false);
    assert!(!guest.dismissible);
    assert_eq!(guest.prompt.unwrap().window_title, "");
    // The prompt went: back to waiting, without a prompt.
    assert!(curtain.look(&Look {
        game_up: true,
        ..at(5.0)
    }));
    assert_eq!(curtain.state().phase, Phase::WaitingWindow);
    assert_eq!(curtain.state().prompt, PromptKind::None);
    assert!(curtain.state().title.is_empty());
}

#[test]
fn launch_curtain_secure_desktop_asks_at_once_and_holds_the_lift() {
    let mut curtain = Curtain::new(false);
    assert!(curtain.look(&Look {
        secure_desktop: true,
        window: GameWindow::Front,
        ..at(0.25)
    }));
    let state = curtain.state();
    assert_eq!(state.phase, Phase::AttentionNeeded);
    assert_eq!(state.prompt, PromptKind::Uac);
    assert_eq!(state.launcher, WINDOWS);
    assert!(state.has(flags::UAC_SECURE_DESKTOP));
    curtain.look(&Look {
        secure_desktop: true,
        window: GameWindow::Front,
        ..at(0.5)
    });
    assert_eq!(curtain.state().phase, Phase::AttentionNeeded, "UAC holds");
    assert_eq!(
        curtain.snapshot(1, true).prompt.unwrap().kind,
        LauncherKind::Uac
    );
    assert_eq!(
        curtain.snapshot(1, true).prompt.unwrap().since_unix_ms,
        1_250
    );
    curtain.look(&Look {
        window: GameWindow::Front,
        ..at(0.75)
    });
    assert!(!curtain.state().has(flags::UAC_SECURE_DESKTOP));
    assert!(curtain.active());
    curtain.look(&Look {
        window: GameWindow::Front,
        ..at(1.0)
    });
    assert_eq!(curtain.state().phase, Phase::Ready);
}

#[test]
fn launch_curtain_repeats_an_unchanged_state_as_a_heartbeat() {
    let mut curtain = Curtain::new(false);
    let mut sent = vec![];
    let mut t = 0.0;
    while t < 10.0 {
        if curtain.look(&at(t)) {
            sent.push((t, curtain.state().sequence, curtain.state().elapsed_ms));
        }
        t += 0.25;
    }
    // Every two seconds, each with a newer sequence and the time since the launch.
    assert_eq!(sent.len(), 4, "{sent:?}");
    assert_eq!(sent[0].0, 2.0);
    for pair in sent.windows(2) {
        assert!(pair[1].1 > pair[0].1);
        assert!(pair[1].2 >= pair[0].2 + 2_000);
    }
    assert_eq!(curtain.state().phase, Phase::StartingApp);
}

#[test]
fn launch_curtain_clips_long_prompt_titles_for_the_wire() {
    let mut curtain = Curtain::new(false);
    curtain.look(&Look {
        launcher: Some(LauncherNotice {
            launcher: "a-launcher-with-a-very-long-identifier-indeed".into(),
            kind: LauncherKind::Prompt,
            window_title: "語".repeat(300),
            since_unix_ms: 1,
        }),
        ..at(1.0)
    });
    let wire = curtain.state().encode().expect("clipped to fit");
    assert!(rubylight_protocol::launch_curtain::CurtainState::decode(&wire).is_some());
}
