//! Tests for holding cuts during calm pictures and releasing holds on sustained or large-frame loss.

use super::*;

// ---- a calm picture: cuts that would change nothing are held ----
fn held_cuts(rig: &Rig) -> u64 {
    rig.lr.snapshot(rig.now())["held_cuts"].as_u64().unwrap()
}

/// Three frames lost to one gap while a menu is up used to halve the bitrate,
/// which then met the game when it moved again. The same loss on a stream that
/// fills its rate is still cut at once.
#[test]
fn a_burst_of_loss_in_a_calm_picture_cuts_nothing() {
    let mut rig = calm(10);
    rig.lose_three();
    rig.clean(5_000);
    assert!(rig.changes.is_empty(), "{:?}", rig.changes);
    assert_eq!(rig.kbps, 33_000);
    assert_eq!(held_cuts(&rig), 1);
    assert_eq!(rig.lr.snapshot(rig.now())["state"], "full");

    let mut rig = Rig::new(33_000);
    rig.clean(3_000);
    rig.lose_three();
    rig.clean(200);
    assert_eq!(rig.cuts(), 1, "{:?}", rig.changes);
    assert_eq!(held_cuts(&rig), 0);
    assert!(rig.kbps < 33_000 * 86 / 100, "{}", rig.kbps);
}

/// A path that cannot carry even the calm picture keeps losing: the first cut
/// is held at half, and the next, at a quarter, is below what the stream
/// sends and is made.
#[test]
fn loss_that_goes_on_in_a_calm_picture_is_cut_below_what_it_sends() {
    let mut rig = calm(30);
    let start = rig.ms;
    rig.run(3_000, |lr, frame, now| {
        if frame % 2 == 0 {
            unrecoverable(lr, frame, now);
        }
    });
    let cuts: Vec<_> = rig
        .changes
        .iter()
        .filter(|(_, c)| c.kind == ChangeKind::StepDown)
        .collect();
    assert!(!cuts.is_empty(), "{:?}", rig.changes);
    let (at, cut) = cuts[0];
    assert!(*at < start + 2_000, "cut after {} ms", at - start);
    assert_eq!(cut.from_kbps, 33_000);
    // Below the 30 % the picture sends, above the lowest bitrate the host uses.
    assert!(cut.to_kbps < 33_000 * 30 / 100, "{cut:?}");
    assert!(cut.to_kbps > 2_000, "{cut:?}");
    assert!(held_cuts(&rig) >= 1);
    // A path's edge was not found: nothing between what was sent and what
    // was allowed was tried.
    assert_eq!(rig.lr.learned_bps(), 0);
}

/// Bursts that keep coming every few seconds count as the cuts they would
/// have been, so a path narrower than the calm picture is still found: each
/// held cut halves where the next starts, until one lowers what is sent.
#[test]
fn held_cuts_add_up_until_one_lowers_what_is_sent() {
    let mut rig = calm(10);
    for _ in 0..3 {
        rig.lose_three();
        rig.clean(3_000);
    }
    assert_eq!(rig.cuts(), 0, "{:?}", rig.changes);
    assert_eq!(held_cuts(&rig), 3);
    assert_eq!(rig.kbps, 33_000);
    rig.lose_three();
    rig.clean(500);
    assert_eq!(rig.cuts(), 1, "{:?}", rig.changes);
    assert!(rig.kbps < 33_000 * 10 / 100, "{}", rig.kbps);
}

/// A held cut is forgotten after `HELD_FOR`, and at once when the stream
/// sends more than it would have left: the next loss on a moving picture is a
/// first cut from the whole rate, at most by half.
#[test]
fn a_held_cut_does_not_outlast_the_calm_picture() {
    let mut rig = calm(10);
    rig.lose_three();
    rig.clean(1_000);
    let level = rig.lr.held.expect("a held cut").0;
    assert!(
        level * 100 >= rig.lr.current * 49 && level < rig.lr.current,
        "{level}"
    );
    // The picture moves again and loses three frames a second later.
    rig.picture_percent = 100;
    rig.busy = true;
    rig.clean(1_000);
    rig.lose_three();
    rig.clean(200);
    assert_eq!(rig.cuts(), 1, "{:?}", rig.changes);
    let cut = rig.changes.last().unwrap().1;
    // Half of the whole rate, not of what the held cut would have left.
    assert!(cut.to_kbps * 100 >= 33_000 * 45, "{cut:?}");
    assert!(rig.lr.held.is_none());

    // Two bursts more than `HELD_FOR` apart: the second starts from the
    // whole rate again.
    let mut rig = calm(10);
    rig.lose_three();
    rig.clean(HELD_FOR.as_millis() as u64 + 1_000);
    rig.lose_three();
    rig.clean(500);
    assert_eq!(rig.cuts(), 0, "{:?}", rig.changes);
    assert_eq!(held_cuts(&rig), 2);
    let again = rig.lr.held.expect("a held cut").0;
    assert_eq!(again, level);
}

/// Two Wi-Fi gaps a second apart in a menu used to end at the lowest bitrate
/// (half, then 0.85 of what the menu delivered). The second is held like the
/// first: a held cut lowered nothing, so it says nothing about the estimate.
#[test]
fn two_bursts_a_second_apart_in_a_calm_picture_cut_nothing() {
    let mut rig = calm(10);
    rig.lose_three();
    rig.clean(1_000);
    rig.lose_three();
    rig.clean(3_000);
    assert!(rig.changes.is_empty(), "{:?}", rig.changes);
    assert_eq!(rig.kbps, 33_000);
    assert_eq!(held_cuts(&rig), 2);
}

/// A keyframe or a scene cut among the small frames of a calm picture may be
/// what the path lost to, and a lower bitrate makes those smaller: with a
/// large frame among the lost ones, or a few frames before them, the cut is
/// made.
#[test]
fn loss_around_a_large_frame_in_a_calm_picture_is_cut() {
    for lead in [0, 1, u64::from(LARGE_FRAME_LEAD) - 1] {
        let mut rig = calm(10);
        rig.picture_percent = 100;
        rig.clean(PERIOD_MS);
        rig.picture_percent = 10;
        rig.clean(PERIOD_MS * lead);
        rig.lose_three();
        rig.clean(200);
        assert_eq!(rig.cuts(), 1, "lead {lead}: {:?}", rig.changes);
        assert_eq!(held_cuts(&rig), 0, "lead {lead}");
    }
    // One sent well before the loss is not what it came from.
    let mut rig = calm(10);
    rig.picture_percent = 100;
    rig.clean(PERIOD_MS);
    rig.picture_percent = 10;
    rig.clean(500);
    rig.lose_three();
    rig.clean(200);
    assert_eq!(rig.cuts(), 0, "{:?}", rig.changes);
    assert_eq!(held_cuts(&rig), 1);
}

/// What became of the keyframe that repairs a loss, as far as the host knows
/// when it weighs the cut.
#[derive(Clone, Copy)]
enum Repair {
    /// No report about it yet.
    Unreported,
    /// It lost packets and error correction rebuilt it.
    Rebuilt,
    /// It could not be rebuilt either.
    Lost,
}

/// Three frames of a calm picture go out and are lost, the keyframe that
/// repairs them goes out behind them, and only then do the client's reports
/// arrive. `large` is that keyframe's size in percent of a frame at the full
/// bitrate.
fn lose_three_then_a_keyframe(rig: &mut Rig, large: u64, repair: Repair) {
    let calm = rig.picture_percent;
    rig.clean(PERIOD_MS * 3);
    let lost = [rig.frame - 2, rig.frame - 1, rig.frame];
    rig.picture_percent = large;
    rig.clean(PERIOD_MS);
    rig.picture_percent = calm;
    let keyframe = rig.frame;
    assert_eq!(keyframe, lost[2] + 1);
    let now = rig.now();
    for frame in lost {
        unrecoverable(&mut rig.lr, frame, now);
    }
    match repair {
        Repair::Unreported => {}
        Repair::Rebuilt => repaired(&mut rig.lr, keyframe, 2, now),
        Repair::Lost => lost_keyframe(&mut rig.lr, keyframe, now),
    }
}

/// The keyframe that repairs a loss is sent because of it: it is in the
/// evidence window when the cut is weighed, three times the size of a frame
/// at the full bitrate, and must not count as the frame the path lost to, or
/// one Wi-Fi gap in a menu halves the bitrate for every device that recovers
/// with a keyframe.
#[test]
fn the_keyframe_that_repairs_a_burst_does_not_undo_the_hold() {
    // Whether its own report is still out or says that error correction
    // rebuilt it: a large frame right after a gap often lacks a packet or two.
    for repair in [Repair::Unreported, Repair::Rebuilt] {
        let mut rig = calm(10);
        lose_three_then_a_keyframe(&mut rig, 300, repair);
        rig.clean(5_000);
        assert!(rig.changes.is_empty(), "{:?}", rig.changes);
        assert_eq!(rig.kbps, 33_000);
        assert_eq!(held_cuts(&rig), 1);

        // Nor does it at the second gap a second later.
        let mut rig = calm(10);
        lose_three_then_a_keyframe(&mut rig, 300, repair);
        rig.clean(1_000);
        lose_three_then_a_keyframe(&mut rig, 300, repair);
        rig.clean(3_000);
        assert!(rig.changes.is_empty(), "{:?}", rig.changes);
        assert_eq!(held_cuts(&rig), 2);
    }
}

/// A repair keyframe that is lost too is a large frame the path lost: the cut
/// is made, and its smaller successor has a better chance.
#[test]
fn a_repair_keyframe_that_is_lost_too_is_cut() {
    let mut rig = calm(10);
    lose_three_then_a_keyframe(&mut rig, 300, Repair::Lost);
    rig.clean(200);
    assert_eq!(rig.cuts(), 1, "{:?}", rig.changes);
    assert_eq!(held_cuts(&rig), 0);
}

/// With a queue standing in the path the loss is the path's, whatever the
/// picture needs: cut as before.
#[test]
fn a_calm_picture_behind_a_standing_queue_is_cut_at_once() {
    let mut rig = calm(10);
    rig.rtt = Duration::from_millis(60);
    rig.clean(500);
    rig.lose_three();
    rig.clean(200);
    assert_eq!(rig.cuts(), 1, "{:?}", rig.changes);
    assert_eq!(held_cuts(&rig), 0);
}
