//! Simulated-link tests for PyroWave floors, host send loss, encoder floors, and steep drops.

use super::*;

// ---- link simulation ----
#[test]
fn a_stream_above_its_link_settles_near_the_link_rate_with_little_loss() {
    // (link Mbit/s, requested kbps)
    for (link, kbps) in [
        (150., 225_000),
        (80., 120_000),
        (20., 40_000),
        (20., 20_000),
    ] {
        for feedback in [2., 20., 80.] {
            // The host sends a frame in a burst (0.05 of the period); the
            // 5 ms queue only with the packets spread over the whole period.
            for (queue, spread) in [(50., 0.05), (50., 1.), (5., 1.)] {
                let mut sim = Sim::new(q(Link::constant(link), queue), kbps, feedback);
                sim.spread = spread;
                sim.run(75.);
                let what =
                    format!("{link} Mbit/s, {kbps} kbps, fb {feedback}, q {queue}, sp {spread}");
                let wire = sim.mean_wire_bps(55., 75.) / 1e6;
                assert!(wire >= link * 0.65, "{what}: {wire:.1} Mbit/s");
                assert!(wire <= link * 1.08, "{what}: {wire:.1} Mbit/s");
                // Settled, and staying settled: every 5 s window, not the mean.
                for from in (20..75).step_by(5) {
                    let wire = sim.mean_wire_bps(from as f64, from as f64 + 5.) / 1e6;
                    assert!(
                        wire >= link * 0.65,
                        "{what}: {wire:.1} Mbit/s from {from} s"
                    );
                }
                assert!(
                    sim.frame_loss(3., 75.) < 0.02,
                    "{what}: {}",
                    sim.frame_loss(3., 75.)
                );
                assert!(
                    sim.longest_gap(3., 75.) < 0.5,
                    "{what}: {}",
                    sim.longest_gap(3., 75.)
                );
            }
        }
    }
}

#[test]
fn a_pyrowave_stream_above_its_link_comes_down_until_it_fits() {
    // 2026-10-29: PyroWave 1080p60 at 200 Mbps to a phone on 5 GHz Wi-Fi lost
    // about a quarter of what it sent for the whole stream and its bitrate
    // never moved. Here 600 Mbps (about 360 Mbit/s of game scene) meets a link
    // that carries 300, which fits above PyroWave's floor. Without hints the
    // host paces such a client at 1.25 times wire demand, so a picture leaves
    // over about 40 % of the period.
    for feedback in [2., 20., 80.] {
        for (queue, spread) in [(50., 0.4), (5., 0.4), (50., 1.)] {
            let mut sim = Sim::pyrowave(q(Link::constant(300.), queue), 600_000, feedback);
            sim.spread = spread;
            sim.run(60.);
            let what = format!("fb {feedback}, q {queue}, sp {spread}");
            assert!(sim.cuts() >= 1, "{what}");
            assert!(
                sim.frame_loss(15., 60.) < 0.02,
                "{what}: {}",
                sim.frame_loss(15., 60.)
            );
            let wire = sim.mean_wire_bps(30., 60.) / 1e6;
            assert!(wire >= 300. * 0.5, "{what}: {wire:.1} Mbit/s");
            assert!(wire <= 300. * 1.08, "{what}: {wire:.1} Mbit/s");
        }
    }
}

/// Review 2026-10-29: 1080p120 at 200 Mbps sits under PyroWave's 373 Mbps
/// floor. Until 2026-11-17 the floor was the stream's own bitrate, so client
/// loss cut nothing and a path that carried less held it there, losing
/// pictures (the 4K120 field case). Half the bitrate is its floor now: loss
/// cuts down to it, the stream stays there while the loss goes on, instead of
/// flipping between floor and full, and climbs once the loss stops.
#[test]
fn a_pyrowave_stream_under_its_own_floor_cuts_to_half_and_stays_there_while_it_loses() {
    let negotiated = Negotiated {
        fps: 120,
        ..pyrowave(200_000)
    };
    let lr = LinkRate::new(true, &negotiated, 0, 20);
    assert!(lr.pyrowave_no_room());
    assert_eq!(lr.floor_kbps, 100_000);
    assert!(!LinkRate::new(true, &pyrowave(400_000), 0, 20).pyrowave_no_room());
    assert!(!LinkRate::new(true, &stream(40_000), 0, 20).pyrowave_no_room());
    let mut rig = Rig::with(lr, 200_000);
    rig.clean(1_000);
    let mut lowest = rig.kbps;
    for _ in 0..40 {
        rig.lose_three();
        rig.clean(1_200);
        lowest = lowest.min(rig.kbps);
    }
    let first = &rig.changes[0].1;
    assert_eq!(first.kind, ChangeKind::StepDown);
    assert!((100_000..200_000).contains(&first.to_kbps), "{first:?}");
    assert_eq!(lowest, 100_000, "{:?}", rig.changes);
    assert!(rig.kinds().contains(&ChangeKind::Floor));
    assert!(!rig.kinds().contains(&ChangeKind::Full));
    rig.clean(12_000);
    assert!(rig.kinds().contains(&ChangeKind::Probe));
}

fn pyrowave_after_host_send_loss() -> Rig {
    // 1080p120 at 200 Mbps is under PyroWave's 373 Mbps floor, so half the
    // bitrate is its floor: a host socket that overflows steps it down there.
    let negotiated = Negotiated {
        fps: 120,
        ..pyrowave(200_000)
    };
    let lr = LinkRate::new(true, &negotiated, 0, 20);
    assert!(lr.pyrowave_no_room());
    let mut rig = Rig::with(lr, 200_000);
    rig.clean(1_000);
    let mut left = 3;
    rig.run(200, |lr, frame, now| {
        if left > 0 {
            left -= 1;
            lr.on_send_loss(frame, now);
        }
    });
    rig.clean(300);
    rig
}

#[test]
fn a_pyrowave_stream_under_its_floor_still_cuts_on_host_send_loss() {
    let rig = pyrowave_after_host_send_loss();
    let change = rig
        .changes
        .iter()
        .map(|(_, c)| c)
        .find(|c| matches!(c.kind, ChangeKind::StepDown | ChangeKind::Floor))
        .unwrap_or_else(|| panic!("no change: {:?}", rig.changes));
    assert_eq!(change.reason, "send_losses");
    assert!(change.from_kbps >= 200_000, "{change:?}");
    assert!(change.to_kbps < 200_000, "{change:?}");
    assert!(change.to_kbps >= 100_000, "{change:?}");
    assert!(rig.lr.pyrowave_no_room(), "client loss still has no room");
}

#[test]
fn pyrowave_host_send_loss_cuts_even_when_client_loss_takes_priority() {
    let negotiated = Negotiated {
        fps: 120,
        ..pyrowave(200_000)
    };
    let mut rig = Rig::with(LinkRate::new(true, &negotiated, 0, 20), 200_000);
    rig.clean(1_000);
    let mut left = 3;
    rig.run(200, |lr, frame, now| {
        if left > 0 {
            left -= 1;
            lr.on_send_loss(frame, now);
            unrecoverable(lr, frame, now);
        }
    });
    let change = &rig.changes.first().expect("mixed losses must cut").1;
    assert_eq!(change.reason, "unrecoverable_frames");
    assert_eq!(change.evidence.send_losses, 3);
    assert!(matches!(change.kind, ChangeKind::StepDown));
    assert!((100_000..200_000).contains(&change.to_kbps), "{change:?}");
}

#[test]
fn pyrowave_client_loss_cannot_lower_a_host_limited_rate_further() {
    let mut rig = pyrowave_after_host_send_loss();
    assert!(rig.kbps < 200_000);
    // Wait past the evidence fence, but stay before the first recovery probe.
    rig.clean(500);
    let before = rig.kbps;
    let cuts = rig.cuts();
    rig.lose_three();
    rig.clean(300);
    assert_eq!(rig.kbps, before);
    assert_eq!(rig.cuts(), cuts);
}

#[test]
fn pyrowave_client_loss_rolls_a_probe_back_to_where_it_began() {
    let mut rig = pyrowave_after_host_send_loss();
    for _ in 0..100 {
        if rig.lr.pending.iter().any(Option::is_some) {
            break;
        }
        rig.clean(100);
    }
    assert!(rig.lr.pending.iter().any(Option::is_some));
    let before = rig.lr.kbps_for(rig.lr.good, 20, PERIOD);
    assert!(rig.kbps < 200_000);
    let changes = rig.changes.len();
    rig.lose_three();
    rig.clean(300);
    let failure = rig.changes[changes..]
        .iter()
        .find(|(_, c)| c.kind == ChangeKind::ProbeFailed)
        .expect("client loss must evaluate the pending probe");
    assert_eq!(failure.1.to_kbps, before);
    assert_eq!(rig.kbps, before);
}

#[test]
fn pyrowave_client_loss_after_host_loss_recovery_cuts_no_lower_than_half() {
    let mut rig = pyrowave_after_host_send_loss();
    assert!(rig.kbps < 200_000);
    rig.clean(120_000);
    assert_eq!(rig.lr.state(), State::Full);
    assert_eq!(rig.kbps, 200_000);
    let changes = rig.changes.len();
    rig.lose_three();
    rig.clean(600);
    // Client loss at the requested rate lowers it, to half at most.
    assert!(
        (100_000..200_000).contains(&rig.kbps),
        "{} {:?}",
        rig.kbps,
        &rig.changes[changes..]
    );
    assert!(rig.lr.pyrowave_no_room());
    assert!(rig.changes.len() > changes, "client evidence was evaluated");
    assert!(
        rig.changes[changes..]
            .iter()
            .any(|(_, c)| c.to_kbps < 200_000),
        "{:?}",
        &rig.changes[changes..]
    );
}

#[test]
fn a_pyrowave_stream_on_a_link_under_its_floor_stops_there_and_says_so() {
    // The field case itself: 200 Mbps into a link that carries 90. PyroWave
    // needs 187 Mbps at 1080p60, so the controller stops at that floor
    // and its warning names HEVC and AV1 instead of cutting to 2 Mbps.
    let minimum = crate::pyrowave::minimum_kbps(1920, 1080, 60_000);
    let mut sim = Sim::pyrowave(q(Link::constant(90.), 50.), 200_000, 20.);
    sim.spread = 0.4;
    sim.run(60.);
    assert!(sim.cuts() >= 1);
    assert!(
        sim.changes.iter().any(|(_, c)| c.kind == ChangeKind::Floor),
        "{:?}",
        sim.changes
    );
    let lowest = sim.kbps_log.iter().map(|&(_, kbps)| kbps).min().unwrap();
    assert!(lowest >= minimum * 99 / 100, "{lowest}");
    assert!(lowest <= minimum * 105 / 100, "{lowest}");
}

#[test]
fn random_detail_loss_never_cuts_a_pyrowave_stream() {
    // Review 2026-10-29: with every lost detail packet counted as a lost
    // frame, 0.1 % random loss on a link with room to spare cut 200 Mbps.
    for share in [0.0001, 0.0005, 0.001] {
        for feedback in [2., 20., 80.] {
            let mut sim = Sim::pyrowave(lossy(Link::constant(1000.), share), 200_000, feedback);
            sim.spread = 0.4;
            sim.run(60.);
            assert_eq!(sim.cuts(), 0, "{share}, fb {feedback}: {:?}", sim.changes);
        }
    }
}

#[test]
fn a_big_overshoot_is_one_cut_not_a_cascade() {
    // The 2026-10-23 link-speed run: 179 Mbit/s requested on a link that
    // carries about 115, which the opt-in controller cut to 24.7 in nine seconds.
    for feedback in [2., 20., 80.] {
        for (queue, spread) in [(50., 0.05), (50., 1.), (5., 1.)] {
            let mut sim = Sim::new(q(Link::constant(138.), queue), 179_000, feedback);
            sim.spread = spread;
            sim.run(75.);
            let what = format!("fb {feedback}, q {queue}, sp {spread}");
            let early = sim
                .changes
                .iter()
                .filter(|(t, c)| *t < 10. && c.kind == ChangeKind::StepDown)
                .count();
            assert!(
                early <= 2,
                "{what}: {early} cuts, {:?}",
                sim.changes.first()
            );
            let lowest = sim.kbps_log.iter().skip(2).map(|(_, k)| *k).min().unwrap();
            assert!(lowest >= 55_000, "{what}: fell to {lowest} kbps");
            assert!(sim.mean_kbps(55., 75.) >= 80_000., "{what}");
            for from in (20..75).step_by(5) {
                let kbps = sim.mean_kbps(from as f64, from as f64 + 5.);
                assert!(kbps >= 70_000., "{what}: {kbps:.0} kbps from {from} s");
            }
            assert!(sim.frame_loss(3., 75.) < 0.02, "{what}");
        }
    }
}

#[test]
fn an_encoder_that_cannot_go_below_the_link_is_not_cut_in_a_runaway() {
    // H.264 on AMD sends at least 14 Mbit/s on noisy content however low it
    // is asked. On a 12 Mbit/s link the controller used to cut again at every
    // fence, down to 2 Mbit/s, and say the network could not carry even that.
    for feedback in [2., 20., 80.] {
        let mut sim = Sim::new(q(Link::constant(12.), 50.), 40_000, feedback);
        sim.encoder_min_kbps = 14_000;
        sim.run(60.);
        let what = format!("fb {feedback}");
        let kinds: Vec<_> = sim.changes.iter().map(|(_, c)| c.kind).collect();
        assert!(
            kinds.contains(&ChangeKind::EncoderFloor),
            "{what}: {kinds:?}"
        );
        assert!(!kinds.contains(&ChangeKind::Floor), "{what}: {kinds:?}");
        assert!(sim.cuts() <= 3, "{what}: {} cuts, {kinds:?}", sim.cuts());
        let found = sim
            .changes
            .iter()
            .find(|(_, c)| c.kind == ChangeKind::EncoderFloor)
            .map(|(t, c)| (*t, c.to_kbps))
            .unwrap();
        assert!(found.0 < 15., "{what}: found at {} s", found.0);
        // Nothing asks for less than the encoder sends, from then on.
        let lowest = sim
            .kbps_log
            .iter()
            .filter(|(t, _)| *t > found.0 + 1.)
            .map(|(_, k)| *k)
            .min()
            .unwrap();
        assert!(lowest >= 12_000, "{what}: asked for {lowest} kbps");
        let after = sim.changes.iter().filter(|(t, _)| *t > found.0);
        assert!(
            after
                .clone()
                .all(|(_, c)| c.kind != ChangeKind::StepDown || c.to_kbps >= 12_000),
            "{what}"
        );
        // It keeps trying upward from the floor, not from the minimum.
        assert!(
            after.clone().any(|(_, c)| c.kind == ChangeKind::Probe),
            "{what}: {kinds:?}"
        );
        assert!(
            sim.lr.snapshot(sim.at(60.))["encoder_floor_kbps"]
                .as_u64()
                .unwrap()
                >= 12_000
        );
    }
}

#[test]
fn an_encoder_that_follows_its_bitrate_never_gets_an_encoder_floor() {
    for (link, kbps) in [(150., 225_000), (20., 40_000)] {
        let mut sim = Sim::new(q(Link::constant(link), 50.), kbps, 20.);
        sim.run(75.);
        assert!(
            sim.changes
                .iter()
                .all(|(_, c)| c.kind != ChangeKind::EncoderFloor),
            "{link}: {:?}",
            sim.changes.iter().map(|(_, c)| c.kind).collect::<Vec<_>>()
        );
    }
}

#[test]
fn a_link_that_drops_to_an_eighth_recovers_within_twenty_five_seconds() {
    // 80 -> 10 -> 80 Mbit/s at a 40 Mbit/s request; the link returns at 40 s.
    for (feedback, queue, spread) in [
        (2., 50., 0.05),
        (20., 50., 0.05),
        (20., 50., 1.),
        (20., 5., 1.),
        (80., 50., 0.05),
        (80., 50., 1.),
    ] {
        let mut sim = Sim::new(q(Link::step(80., 10., 20., 40.), queue), 40_000, feedback);
        sim.spread = spread;
        sim.run(75.);
        let what = format!("fb {feedback}, q {queue}, sp {spread}");
        // An eighth of the rate takes two cuts: the first is held to half,
        // the second, right after it, goes to the delivered rate.
        let (gap, loss) = (1.5, 0.03);
        assert!(
            sim.longest_gap(19., 45.) <= gap,
            "{what}: gap {}",
            sim.longest_gap(19., 45.)
        );
        assert!(
            sim.frame_loss(3., 75.) <= loss,
            "{what}: loss {}",
            sim.frame_loss(3., 75.)
        );
        let back = sim.reaches(40., 36_000).expect("back to 90 %");
        // The last probe that failed on the dip keeps probes under its rate
        // for the quiet hold (KNEE_QUIET), and a step at a time past it.
        assert!(back <= 65., "{what}: 90 % at {back:.1} s");
        assert!(sim.cuts() <= 2, "{what}: {} cuts", sim.cuts());
    }
}

#[test]
fn a_link_that_drops_to_a_tenth_recovers_within_twenty_seconds() {
    for (feedback, queue, spread) in [
        (2., 50., 0.05),
        (20., 50., 0.05),
        (20., 5., 1.),
        (80., 50., 0.05),
        (80., 50., 1.),
    ] {
        let mut sim = Sim::new(q(Link::step(300., 30., 20., 40.), queue), 40_000, feedback);
        sim.spread = spread;
        sim.run(75.);
        let what = format!("fb {feedback}, q {queue}, sp {spread}");
        assert!(
            sim.longest_gap(19., 45.) <= 1.5,
            "{what}: gap {}",
            sim.longest_gap(19., 45.)
        );
        assert!(
            sim.frame_loss(3., 75.) <= 0.03,
            "{what}: loss {}",
            sim.frame_loss(3., 75.)
        );
        let back = sim.reaches(40., 36_000).expect("back to 90 %");
        assert!(back <= 60., "{what}: 90 % at {back:.1} s");
        assert_eq!(sim.cuts(), 1, "{what}");
    }
}

#[test]
fn a_pyrowave_stream_under_its_own_floor_settles_at_what_the_path_carries() {
    // 1080p120 at 350 Mbps is under PyroWave's 373 Mbps floor, so there is no
    // quality floor to cut to. The field case (2026-11-17): a 4K120 stream was
    // held at its requested rate on a path that carried less, and cut only
    // when the host's socket overflowed. Now loss cuts it (to half at most),
    // repeated failures at the edge teach a ceiling, and it stays under the
    // path: review 2026-11-17 H2 found the first build standing 5 % over it,
    // its ceiling a floor no cut could lower.
    let mut sim = Sim::pyrowave_no_room(q(Link::constant(200.), 50.), 350_000, 20.);
    sim.spread = 0.4;
    sim.run(300.);
    assert!(sim.cuts() >= 1, "no cut: {:?}", sim.changes);
    let collapses = sim
        .changes
        .iter()
        .filter(|(_, c)| c.kind == ChangeKind::StepDown && c.to_kbps * 100 < c.from_kbps * 90)
        .count();
    assert!(collapses <= 2, "{collapses} collapses: {:?}", sim.changes);
    assert!(
        sim.frame_loss(30., 300.) < 0.02,
        "loss {}",
        sim.frame_loss(30., 300.)
    );
    let wire = sim.mean_wire_bps(240., 300.) / 1e6;
    assert!((130. ..=200.).contains(&wire), "{wire:.0} Mbit/s");
    let r = sim.report(30., 300.);
    assert!(r.over_link <= 0.1 && r.p95_queue_ms < 40., "{r:?}");
    // A learned ceiling below the requested rate, and no probe back to it.
    assert!(sim.lr.learned_bps() > 0);
    assert!(
        sim.mean_kbps(240., 300.) < 350_000.,
        "still at the request: {}",
        sim.mean_kbps(240., 300.)
    );
}
