//! Feedback polling, coalescing, failure throttling, and virtual touchpad mapping tests.

use super::*;

#[test]
fn feedback_failures_warn_per_pad_without_hiding_healthy_reports_or_flooding_logs() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("feedback.log");
    let file = std::fs::File::create(&path).unwrap();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_target(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(move || file.try_clone().unwrap())
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        let active = BTreeMap::from([(0, 2), (1, 5)]);
        let mut last = BTreeMap::new();
        let mut failures = BTreeMap::new();
        let now = Instant::now();
        for millis in [0, 1, 4999, 5000] {
            let output = poll_feedback(
                &active,
                &mut last,
                &mut failures,
                "fake",
                now + Duration::from_millis(millis),
                |slot| {
                    if slot == 2 || millis >= 4999 {
                        bail!("injected poll failure");
                    }
                    Ok(Some((1, vec![7, 8])))
                },
            );
            assert_eq!(
                output,
                if millis == 0 {
                    vec![(1, 1, vec![7, 8])]
                } else {
                    vec![]
                }
            );
        }
        assert_eq!(
            poll_feedback(
                &active,
                &mut last,
                &mut failures,
                "fake",
                now + Duration::from_millis(5001),
                |_| Ok(Some((1, vec![7, 8])))
            ),
            vec![(0, 1, vec![7, 8])]
        );
        assert!(
            poll_feedback(
                &active,
                &mut last,
                &mut failures,
                "fake",
                now + Duration::from_millis(5002),
                |_| bail!("intermittent poll failure")
            )
            .is_empty()
        );
        let log = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = log.lines().collect();
        assert_eq!(lines.len(), 3, "{log}");
        for line in &lines {
            assert!(line.contains("WARN"));
            assert!(line.contains("controller feedback poll failed"));
            assert!(line.contains("backend=\"fake\""));
            assert!(line.contains("injected poll failure"));
        }
        assert!(lines[0].contains("controller=0 slot=2"));
        assert!(lines[0].contains("failures=1"));
        assert!(lines[1].contains("controller=1 slot=5"));
        assert!(lines[1].contains("failures=1"));
        assert!(lines[2].contains("controller=0 slot=2"));
        assert!(lines[2].contains("failures=3"));

        poll_feedback(
            &BTreeMap::new(),
            &mut last,
            &mut failures,
            "fake",
            now + Duration::from_millis(5003),
            |_| unreachable!(),
        );
        assert!(last.is_empty());
        assert!(failures.is_empty());
        poll_feedback(
            &active,
            &mut last,
            &mut failures,
            "fake",
            now + Duration::from_millis(5004),
            |_| bail!("replugged pad failure"),
        );
        let log = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = log.lines().collect();
        assert_eq!(lines.len(), 5, "{log}");
        assert!(lines[3..].iter().all(|line| line.contains("failures=1")));

        // Failures held back by the interval are reported once the pads
        // recover, and only once.
        for (millis, result) in [(5005, false), (5006, true), (10_004, true), (10_005, true)] {
            poll_feedback(
                &active,
                &mut last,
                &mut failures,
                "fake",
                now + Duration::from_millis(millis),
                |_| {
                    if result {
                        Ok(None)
                    } else {
                        bail!("held back failure")
                    }
                },
            );
        }
        let log = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = log.lines().collect();
        assert_eq!(lines.len(), 7, "{log}");
        assert!(lines[5].contains("controller=0 slot=2"));
        assert!(lines[6].contains("controller=1 slot=5"));
        assert!(lines[5..].iter().all(|line| {
            line.contains("controller feedback polls recovered after failing")
                && line.contains("failures=1")
        }));
        assert!(failures.values().all(|failure| failure.unreported == 0));
    });
}

#[test]
fn feedback_drains_each_pads_backlog_and_forwards_the_newest_of_each_kind() {
    // Pad 0 has a rumble and its stop pending; pad 1 a game's rumble
    // writes beyond what one poll reads.
    let mut queues = BTreeMap::from([
        (2u32, VecDeque::from([(4u16, vec![9; 8]), (4, vec![0; 8])])),
        (5, (0..10u8).map(|n| (4, vec![n])).collect()),
    ]);
    let active = BTreeMap::from([(0, 2), (1, 5)]);
    let mut last = BTreeMap::new();
    let mut failures = BTreeMap::new();
    let mut calls = 0;
    let mut poll = |slot| {
        calls += 1;
        Ok(queues.get_mut(&slot).unwrap().pop_front())
    };
    let output = poll_feedback(
        &active,
        &mut last,
        &mut failures,
        "fake",
        Instant::now(),
        &mut poll,
    );
    // The stop is the last rumble pad 0 sends, so its motors don't stick.
    assert_eq!(
        output,
        vec![(0, 4, vec![0; 8]), (1, 4, vec![FEEDBACK_DRAIN as u8 - 1])]
    );
    let output = poll_feedback(
        &active,
        &mut last,
        &mut failures,
        "fake",
        Instant::now(),
        &mut poll,
    );
    assert_eq!(output, vec![(1, 4, vec![9])]);
    // Pad 0: two reports and the empty answer, then one empty answer;
    // pad 1: a full drain, then the rest and the empty answer.
    assert_eq!(calls, 3 + FEEDBACK_DRAIN + 1 + 3);

    // A report read before a failing call still goes out, and the failure
    // counts.
    let mut answers = VecDeque::from([Ok(Some((4, vec![7]))), Err(anyhow::anyhow!("gone"))]);
    let output = poll_feedback(
        &BTreeMap::from([(0, 2)]),
        &mut last,
        &mut failures,
        "fake",
        Instant::now(),
        |_| answers.pop_front().unwrap(),
    );
    assert_eq!(output, vec![(0, 4, vec![7])]);
    assert_eq!(failures[&0].unreported, 0, "the first failure is logged");
    assert!(failures[&0].reported.is_some());
}

#[test]
fn a_rumble_only_report_does_not_hide_the_lights_and_trigger_effects_before_it() {
    // PlayStation reports: rumble in bytes 0-3, lightbar 4-6, flags in 7
    // (1 lightbar, 2 trigger effects), the effects from byte 10.
    let report = |rumble: u8, flags: u8| {
        let mut data = vec![0; 32];
        data[..4].fill(rumble);
        data[4..7].copy_from_slice(&[1, 2, 3]);
        data[7] = flags;
        data[10] = 0x21;
        (5u16, data)
    };
    let poll_once = |queue: Vec<(u16, Vec<u8>)>| {
        let mut queue = VecDeque::from(queue);
        poll_feedback(
            &BTreeMap::from([(0, 2)]),
            &mut BTreeMap::new(),
            &mut BTreeMap::new(),
            "fake",
            Instant::now(),
            |_| Ok(queue.pop_front()),
        )
    };
    let with = |(kind, data): (u16, Vec<u8>)| (0, kind, data);
    // Triggers off and lights, then a rumble-only report: both go, the
    // newest rumble last.
    assert_eq!(
        poll_once(vec![report(0xff, 3), report(0, 0)]),
        vec![with(report(0xff, 3)), with(report(0, 0))]
    );
    assert_eq!(
        poll_once(vec![report(0xff, 2), report(0x80, 1), report(0, 0)]),
        vec![
            with(report(0xff, 2)),
            with(report(0x80, 1)),
            with(report(0, 0))
        ]
    );
    // A newer report that carries everything an older one did replaces it.
    assert_eq!(
        poll_once(vec![report(0xff, 0), report(0x80, 1), report(0, 3)]),
        vec![with(report(0, 3))]
    );
}

fn submit_touch(
    pointers: &mut BTreeMap<(u8, u32), u8>,
    profile: u16,
    global: u16,
    input: &Event,
) -> Option<Vec<u8>> {
    submit_touch_on(pointers, profile, global, input, false)
}

fn submit_touch_on(
    pointers: &mut BTreeMap<(u8, u32), u8>,
    profile: u16,
    global: u16,
    input: &Event,
    dual: bool,
) -> Option<Vec<u8>> {
    let update = gamepad_touch_request(pointers, profile, global, input, dual)?;
    let packet = update.packet.clone();
    update.submitted(pointers);
    Some(packet)
}

#[test]
fn failed_touch_submission_does_not_acquire_or_release_contact_slots() {
    let mut pointers = BTreeMap::new();
    drop(gamepad_touch_request(&pointers, 6, 2, &touch(2, 0, 1, 17), false).unwrap());
    assert!(pointers.is_empty());
    submit_touch(&mut pointers, 6, 2, &touch(2, 0, 1, 17)).unwrap();
    for event in [2, 4, 6, 7] {
        drop(gamepad_touch_request(&pointers, 6, 2, &touch(2, 0, event, 17), false).unwrap());
        assert_eq!(pointers, BTreeMap::from([((2, 17), 0)]));
    }
}

#[test]
fn ds4_keeps_primary_contact_slots_and_ignores_secondary_surfaces() {
    let mut pointers = BTreeMap::new();
    for pointer in [17, 18] {
        submit_touch(&mut pointers, 5, 2, &touch(2, 0, 1, pointer)).unwrap();
    }
    assert!(submit_touch(&mut pointers, 5, 2, &touch(2, 0, 1, 19)).is_none());
    assert!(submit_touch(&mut pointers, 5, 2, &touch(2, 1, 7, 17)).is_none());
    assert_eq!(pointers.len(), 2);
    submit_touch(&mut pointers, 5, 2, &touch(2, 0, 7, 17)).unwrap();
    assert!(pointers.is_empty());
}

#[test]
fn two_announced_touchpads_share_the_surface_side_by_side() {
    let mut pointers = BTreeMap::new();
    // A Steam Deck's trackpads: the same finger id on each, both down.
    let left = submit_touch_on(&mut pointers, 6, 5, &touch(2, 0, 1, 0), true).unwrap();
    let right = submit_touch_on(&mut pointers, 6, 5, &touch(2, 1, 1, 0), true).unwrap();
    assert_eq!(&left[12..14], &[0, 1]);
    assert_eq!(&right[12..14], &[1, 1]);
    // x = 0.25 of each half is 1/8 of the surface and 5/8 of it.
    assert_eq!(&left[14..16], &(8191u16).to_le_bytes());
    assert_eq!(&right[14..16], &(40959u16).to_le_bytes());
    assert_eq!(pointers.len(), 2);
    // Lifting one finger frees only its contact.
    let up = submit_touch_on(&mut pointers, 6, 5, &touch(2, 1, 2, 0), true).unwrap();
    assert_eq!(&up[12..14], &[1, 2]);
    assert_eq!(pointers, BTreeMap::from([((2, 0), 0)]));
    let moved = submit_touch_on(&mut pointers, 6, 5, &touch(2, 0, 3, 0), true).unwrap();
    assert_eq!(&moved[12..14], &[0, 3]);
    // Neither a third finger nor a third touchpad finds a slot or a place.
    submit_touch_on(&mut pointers, 6, 5, &touch(2, 1, 1, 1), true).unwrap();
    assert!(submit_touch_on(&mut pointers, 6, 5, &touch(2, 0, 1, 1), true).is_none());
    assert!(submit_touch_on(&mut pointers, 6, 5, &touch(2, 2, 1, 2), true).is_none());
    // Cancelling clears both touchpads' contacts.
    submit_touch_on(&mut pointers, 6, 5, &touch(2, 0, 7, 0), true).unwrap();
    assert!(pointers.is_empty());
}

#[test]
fn the_right_touchpad_stays_ignored_when_the_client_did_not_announce_two() {
    let mut pointers = BTreeMap::new();
    assert!(submit_touch_on(&mut pointers, 5, 2, &touch(2, 1, 1, 0), false).is_none());
    let full = submit_touch_on(&mut pointers, 5, 2, &touch(2, 0, 1, 0), false).unwrap();
    // A single touchpad keeps the whole surface.
    assert_eq!(&full[14..16], &(16383u16).to_le_bytes());
    assert_eq!(pointers.len(), 1);
    // Pads without a touch surface take no touch even when announced dual.
    for profile in [3, 4, 7] {
        assert!(submit_touch_on(&mut pointers, profile, 2, &touch(2, 1, 1, 5), true).is_none());
    }
}

#[test]
fn the_steam_controllers_contacts_are_its_two_pads() {
    use butterpollo_input_policy::input_policy::VHF_STEAM_CONTROLLER as STEAM;
    let mut pointers = BTreeMap::new();
    // Two touchpads: each is its own pad at its full width.
    let left = submit_touch_on(&mut pointers, STEAM, 4, &touch(2, 0, 1, 0), true).unwrap();
    let right = submit_touch_on(&mut pointers, STEAM, 4, &touch(2, 1, 1, 0), true).unwrap();
    assert_eq!(&left[12..16], &[0, 1, 255, 63]);
    assert_eq!(&right[12..16], &[1, 1, 255, 63]);
    let up = submit_touch_on(&mut pointers, STEAM, 4, &touch(2, 1, 2, 0), true).unwrap();
    assert_eq!(&up[12..14], &[1, 2]);
    assert_eq!(pointers, BTreeMap::from([((2, 0), 0)]));
    submit_touch_on(&mut pointers, STEAM, 4, &touch(2, 0, 7, 0), true).unwrap();
    assert!(pointers.is_empty());
    // One touchpad: its left half is the left pad, stretched to full width.
    let half = submit_touch_on(&mut pointers, STEAM, 4, &touch(2, 0, 1, 9), false).unwrap();
    assert_eq!(&half[12..16], &[0, 1, 255, 127]);
    let mut right_half = touch(2, 0, 1, 10);
    if let Event::ControllerTouch { x, .. } = &mut right_half {
        *x = 0.75;
    }
    let right = submit_touch_on(&mut pointers, STEAM, 4, &right_half, false).unwrap();
    assert_eq!(&right[12..16], &[1, 1, 255, 127]);
    // A move keeps the pad the finger went down on.
    let mut moved = touch(2, 0, 3, 10);
    if let Event::ControllerTouch { x, .. } = &mut moved {
        *x = 0.25;
    }
    let moved = submit_touch_on(&mut pointers, STEAM, 4, &moved, false).unwrap();
    assert_eq!(&moved[12..16], &[1, 3, 0, 0]);
    assert!(submit_touch_on(&mut pointers, STEAM, 4, &touch(2, 1, 1, 11), false).is_none());
    // A second finger on the held left pad is ignored, and the first one's
    // lift frees the pad.
    assert!(submit_touch_on(&mut pointers, STEAM, 4, &touch(2, 0, 1, 12), false).is_none());
    assert!(submit_touch_on(&mut pointers, STEAM, 4, &touch(2, 0, 2, 12), false).is_none());
    assert_eq!(pointers, BTreeMap::from([((2, 9), 0), ((2, 10), 1)]));
    submit_touch_on(&mut pointers, STEAM, 4, &touch(2, 0, 2, 9), false).unwrap();
    assert_eq!(pointers, BTreeMap::from([((2, 10), 1)]));
}

#[test]
fn steam_controller_feedback_is_never_coalesced_or_deduplicated() {
    let click = (6u16, [vec![4, 0, 0, 0, 0x82, 0, 2, 0], vec![0; 8]].concat());
    let settings = (7u16, vec![0x8e; 32]);
    let mut queue = VecDeque::from([
        click.clone(),
        click.clone(),
        settings.clone(),
        settings.clone(),
    ]);
    let mut last = BTreeMap::new();
    let output = poll_feedback(
        &BTreeMap::from([(0, 2)]),
        &mut last,
        &mut BTreeMap::new(),
        "fake",
        Instant::now(),
        |_| Ok(queue.pop_front()),
    );
    let with = |(kind, data): (u16, Vec<u8>)| (0, kind, data);
    assert_eq!(
        output,
        vec![
            with(click.clone()),
            with(click),
            with(settings.clone()),
            with(settings)
        ]
    );
    assert!(
        last.is_empty(),
        "a Steam report does not hide a later rumble"
    );
}

#[test]
fn secondary_touchpad_cannot_move_release_or_cancel_primary_contacts() {
    let mut pointers = BTreeMap::new();
    let first = submit_touch(&mut pointers, 6, 5, &touch(2, 0, 1, 17)).unwrap();
    assert_eq!(first.len(), 22);
    assert_eq!(&first[8..12], &5u32.to_le_bytes());
    assert_eq!(&first[12..], &[0, 1, 255, 63, 255, 127, 255, 255, 0, 0]);
    for index in [1, 2, 255] {
        for event in 0..=7 {
            let before = pointers.clone();
            assert!(submit_touch(&mut pointers, 6, 5, &touch(2, index, event, 17)).is_none());
            assert_eq!(pointers, before);
        }
    }
    let moved = submit_touch(&mut pointers, 6, 5, &touch(2, 0, 3, 17)).unwrap();
    assert_eq!(&moved[12..14], &[0, 3]);
    let second = submit_touch(&mut pointers, 6, 5, &touch(2, 0, 1, 18)).unwrap();
    assert_eq!(&second[12..14], &[1, 1]);
    assert!(submit_touch(&mut pointers, 6, 5, &touch(2, 0, 1, 19)).is_none());
    let released = submit_touch(&mut pointers, 6, 5, &touch(2, 0, 2, 17)).unwrap();
    assert_eq!(&released[12..14], &[0, 2]);
    let reused = submit_touch(&mut pointers, 6, 5, &touch(2, 0, 1, 19)).unwrap();
    assert_eq!(&reused[12..14], &[0, 1]);
}

#[test]
fn virtual_touch_mapping_keeps_controller_lifecycles_and_profiles_separate() {
    let mut pointers = BTreeMap::new();
    for id in [2, 3] {
        submit_touch(&mut pointers, 5, u16::from(id), &touch(id, 0, 1, 17)).unwrap();
    }
    let cancel = submit_touch(&mut pointers, 5, 2, &touch(2, 0, 7, 0)).unwrap();
    assert_eq!(&cancel[12..14], &[0, 5]);
    assert_eq!(pointers, BTreeMap::from([((3, 17), 0)]));
    for profile in [1, 2, 3, 4, 7, 8] {
        assert!(submit_touch(&mut pointers, profile, 3, &touch(3, 0, 7, 0)).is_none());
        assert_eq!(pointers, BTreeMap::from([((3, 17), 0)]));
    }
    let cancel_contact = submit_touch(&mut pointers, 6, 3, &touch(3, 0, 6, 17)).unwrap();
    assert_eq!(&cancel_contact[12..14], &[0, 4]);
    assert!(pointers.is_empty());
}
