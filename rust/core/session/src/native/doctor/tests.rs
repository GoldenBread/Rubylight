//! Stream Doctor trials and snapshots without a host: apply, undo, replaced and refused.
use super::*;

/// No `max_bitrate` and a codec that takes any rate.
const OPEN: u32 = u32::MAX;

fn encoder(live_bitrate_kbps: u32) -> EncoderFacts {
    EncoderFacts {
        backend: "amf".into(),
        codec: "hevc".into(),
        hdr: false,
        width: 2560,
        height: 1440,
        fps: 120,
        live_bitrate_kbps,
        fec_percent: 20,
    }
}

fn facts(owner: bool, live: u32) -> Facts {
    Facts {
        session_active: true,
        owner,
        may_change: true,
        ceiling_kbps: OPEN,
        encoder: Some(encoder(live)),
        stages: None,
    }
}

#[test]
fn a_step_down_takes_a_fifth_off_and_undoes_to_the_rate_it_left() {
    let mut trials = Trials::default();
    let offered = snapshot(facts(true, 45_000), &trials);
    let remedy = &offered.eligible_remedies[0];
    assert!(remedy.configurable && remedy.reason.is_none());
    assert_eq!(remedy.target_kbps, Some(36_000));
    let target = trials
        .check_apply(true, RemedyId::BitrateStepdown, 45_000, OPEN)
        .unwrap();
    let id = trials.applied(45_000, 20, target);
    let applied = snapshot(facts(true, 36_000), &trials);
    assert_eq!(applied.active_trials.len(), 1);
    assert!(applied.active_trials[0].can_undo);
    assert_eq!(applied.active_trials[0].trial_id, id);
    assert_eq!(
        applied.eligible_remedies[0].reason.as_deref(),
        Some("trial_active")
    );
    assert_eq!(
        trials.begin_undo(true, RemedyId::BitrateStepdown, Some(&id), 36_000, OPEN),
        Ok((45_000, 20)),
        "the bitrate and the launch's FEC share inside it"
    );
    trials.undone();
    assert!(
        snapshot(facts(true, 45_000), &trials)
            .active_trials
            .is_empty()
    );
}

#[test]
fn a_change_since_the_trial_wins_over_its_undo() {
    let mut trials = Trials::default();
    let id = trials.applied(20_000, 0, 16_000);
    // The player moved the bitrate slider to 12 Mbps.
    let replaced = snapshot(facts(true, 12_000), &trials);
    assert!(!replaced.active_trials[0].can_undo);
    assert!(
        replaced.eligible_remedies[0].configurable,
        "a replaced trial blocks nothing"
    );
    assert_eq!(
        trials.begin_undo(true, RemedyId::BitrateStepdown, Some(&id), 12_000, OPEN),
        Err(Refusal::Replaced)
    );
    assert_eq!(Refusal::Replaced.code(), ErrorCode::NotWritable);
    assert_eq!(
        trials,
        Trials {
            issued: 1,
            bitrate: None
        }
    );
}

#[test]
fn the_pcs_limit_caps_the_step_and_holds_an_undo_it_would_break() {
    let mut trials = Trials::default();
    // PyroWave at 1.4 Gbps under its 2 Gbps cap: a fifth off, as offered.
    let mut pyrowave = facts(true, 1_400_000);
    pyrowave.ceiling_kbps = 2_000_000;
    let offer = &snapshot(pyrowave, &trials).eligible_remedies[0];
    assert_eq!(offer.target_kbps, Some(1_120_000));
    assert_eq!(
        trials.check_apply(true, RemedyId::BitrateStepdown, 1_400_000, 2_000_000),
        Ok(1_120_000)
    );
    // `max_bitrate` lowered mid-stream below the step: the offer says what the PC applies.
    let mut capped = facts(true, 45_000);
    capped.ceiling_kbps = 30_000;
    assert_eq!(
        snapshot(capped, &trials).eligible_remedies[0].target_kbps,
        Some(30_000)
    );
    // An undo above the limit is refused, and the trial stays to undo later.
    let id = trials.applied(45_000, 20, 36_000);
    assert_eq!(
        trials.begin_undo(true, RemedyId::BitrateStepdown, Some(&id), 36_000, 40_000),
        Err(Refusal::Capped)
    );
    assert_eq!(Refusal::Capped.code(), ErrorCode::NotWritable);
    assert_eq!(
        trials.begin_undo(true, RemedyId::BitrateStepdown, Some(&id), 36_000, OPEN),
        Ok((45_000, 20))
    );
}

#[test]
fn guests_floors_and_unknown_rates_are_refused_with_their_reason() {
    let trials = Trials::default();
    let cases = [
        (false, 45_000, Refusal::NotOwner),
        (true, 0, Refusal::BitrateUnknown),
        (true, TRIAL_FLOOR_KBPS, Refusal::AtMinimum),
    ];
    for (owner, live, refusal) in cases {
        assert_eq!(
            trials.check_apply(owner, RemedyId::BitrateStepdown, live, OPEN),
            Err(refusal)
        );
        let remedy = &snapshot(facts(owner, live), &trials).eligible_remedies[0];
        assert!(!remedy.configurable);
        assert_eq!(remedy.reason.as_deref(), Some(refusal.reason()));
        assert!(remedy.message.is_some());
    }
    assert_eq!(Refusal::NotOwner.code(), ErrorCode::NotSessionOwner);
    assert_eq!(
        trials.check_apply(true, RemedyId::Unknown, 45_000, OPEN),
        Err(Refusal::UnknownRemedy)
    );
    assert_eq!(
        stepdown_kbps(600),
        TRIAL_FLOOR_KBPS,
        "never below the floor"
    );
    // A device that may only watch is told so before it asks.
    let mut viewer = facts(true, 45_000);
    viewer.may_change = false;
    let remedy = &snapshot(viewer, &trials).eligible_remedies[0];
    assert_eq!(remedy.reason.as_deref(), Some("permission_required"));
    assert_eq!(Refusal::NoPermission.code(), ErrorCode::PermissionRequired);
    let mut other = Trials::default();
    let id = other.applied(20_000, 0, 16_000);
    assert_eq!(
        other.begin_undo(true, RemedyId::BitrateStepdown, Some("t-9"), 16_000, OPEN),
        Err(Refusal::NoTrial)
    );
    assert_eq!(
        other.begin_undo(false, RemedyId::BitrateStepdown, Some(&id), 16_000, OPEN),
        Err(Refusal::NotOwner)
    );
}

#[test]
fn without_a_stream_nothing_is_offered() {
    let mut trials = Trials::default();
    trials.applied(20_000, 0, 16_000);
    let idle = snapshot(
        Facts {
            session_active: false,
            owner: false,
            may_change: true,
            ceiling_kbps: OPEN,
            encoder: None,
            stages: None,
        },
        &trials,
    );
    assert!(idle.active_trials.is_empty() && idle.eligible_remedies.is_empty());
    assert_eq!(idle.window_seconds, WINDOW_SECONDS);
}

#[test]
fn the_stages_carry_the_reports_attribution_and_rates() {
    use butterpollo_media_policy::stutter_report::{Picture, Totals};
    use std::time::Duration;
    let usual = Picture::new(2_000, 1_000, 4_000, 500, 500);
    let mut totals = Totals::default();
    totals.add(&vec![usual; 30], Duration::from_millis(8));
    let mut stalled = vec![usual; 30];
    stalled[3] = Picture::new(2_000, 1_000, 30_000, 500, 500);
    totals.add(&stalled, Duration::from_millis(8));
    assert_eq!(stages(&Totals::default().report(), None), None);
    let rates = Rates {
        window_ms: 1000,
        source_mhz: Some(120_000),
        capture_mhz: Some(120_000),
        submit_mhz: Some(100_000),
        output_mhz: Some(100_000),
        repeat_mhz: None,
    };
    let stages = stages(&totals.report(), Some(&rates)).unwrap();
    assert_eq!((stages.pictures, stages.late_pictures), (60, 1));
    assert_eq!(stages.dominant_stage, Some(HostStage::Encode));
    assert_eq!(stages.lost_us.encode, 26_000);
    assert_eq!(stages.pipeline_rates.unwrap().submit_mhz, Some(100_000));
}
