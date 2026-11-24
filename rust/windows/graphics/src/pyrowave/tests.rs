use super::*;
use std::{cell::Cell, rc::Rc};

struct Resource(Rc<Cell<usize>>);
impl Drop for Resource {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

#[test]
fn repeated_fallbacks_give_one_line_per_period() {
    let period = Duration::from_secs(30);
    let start = Instant::now();
    let mut notice = Notice::default();
    let lines = (0..3000u64)
        .filter(|n| notice.due(start + Duration::from_millis(n * 17), period))
        .count();
    // 3000 frames at 17 ms is 51 s: the first, and one more after 30 s.
    assert_eq!(lines, 2);
    // Frames of different sizes get the same text, so Warnings dedups them to one entry: the
    // message is built from the config (packet size, limit) only, and no frame size is in it.
    let limit = 1_000_000;
    let warnings = butterpollo_media_policy::RecordedWarnings::default();
    let frame_sizes = [limit + 1, limit + 4096, limit * 3, 1_234_567, 99_999_999];
    for frame_bytes in frame_sizes {
        let text = frame_size_message(limit);
        assert!(!text.contains(&frame_bytes.to_string()), "{text}");
        warnings.event("pyrowave_frame_size", &text, period);
    }
    assert_eq!(warnings.snapshot().len(), 1);
    for frame_bytes in frame_sizes {
        let text = row_order_dropped_message(1024);
        assert!(!text.contains(&frame_bytes.to_string()), "{text}");
        warnings.event("pyrowave_row_order", &text, period);
    }
    assert_eq!(warnings.snapshot().len(), 2);
    // A different config value is a different text.
    assert_ne!(frame_size_message(limit), frame_size_message(limit * 2));
}

fn flight(dropped: &Rc<Cell<usize>>, slot: usize, at: Instant) -> Flight<Resource> {
    Flight {
        lease: Resource(dropped.clone()),
        slot,
        release: 2,
        begin: at,
        submitted: at,
        captured: at,
        plan: Plan {
            budget: 100_000,
            kbps: 500_000,
            interval: Duration::from_millis(8),
        },
        seq_offset: 0,
        cache_repeat: None,
    }
}

#[test]
fn only_confirmed_vulkan_completion_releases_the_capture_lease() {
    for code in [
        p::VkResult_VK_SUCCESS,
        p::VkResult_VK_TIMEOUT,
        p::VkResult_VK_ERROR_DEVICE_LOST,
        p::VkResult_VK_ERROR_OUT_OF_DEVICE_MEMORY,
    ] {
        let dropped = Rc::new(Cell::new(0));
        let mut flights = Flights::default();
        let now = Instant::now();
        flights.push(flight(&dropped, 0, now));
        // A waiting completion, as with one frame in flight and in Drop.
        let result = flights.complete(|f| settle(|_| reached(code), f.submitted, now, true));
        if code == p::VkResult_VK_SUCCESS {
            assert_eq!(result.unwrap().unwrap().slot, 0);
            assert!(flights.is_empty());
            assert!(!flights.unconfirmed);
            assert_eq!(dropped.get(), 1);
        } else {
            assert!(result.is_err());
            assert_eq!(flights.len(), 1);
            assert!(flights.unconfirmed);
            assert_eq!(dropped.get(), 0);
            // Quarantining must not run the capture lease's destructor.
            flights.retain();
            assert!(flights.is_empty());
            drop(flights);
            assert_eq!(dropped.get(), 0);
        }
    }
}

#[test]
fn a_poll_waits_for_nothing_and_gives_up_only_past_the_completion_bound() {
    let submitted = Instant::now();
    let mut asked = None;
    let polled = settle(
        |timeout| {
            asked = Some(timeout);
            Ok(false)
        },
        submitted,
        submitted + Duration::from_millis(1),
        false,
    );
    assert!(!polled.unwrap());
    assert_eq!(asked, Some(Duration::ZERO));
    // A waiting completion takes what is left of the bound.
    let waited = settle(
        |timeout| {
            asked = Some(timeout);
            Ok(true)
        },
        submitted,
        submitted + Duration::from_millis(500),
        true,
    );
    assert!(waited.unwrap());
    assert_eq!(asked, Some(COMPLETION_TIMEOUT - Duration::from_millis(500)));
    // Not done within it is an error, for a poll as for a wait.
    let late = submitted + COMPLETION_TIMEOUT;
    assert!(settle(|_| Ok(false), submitted, late, false).is_err());
    assert!(settle(|_| Ok(false), submitted, submitted, true).is_err());
    // Done is done, however late it is seen.
    assert!(settle(|_| Ok(true), submitted, late, false).unwrap());
    // A failed wait stays a device loss for the recovery classifier.
    let error = settle(
        |_| reached(p::VkResult_VK_ERROR_DEVICE_LOST),
        submitted,
        submitted,
        false,
    )
    .unwrap_err();
    assert!(crate::device_loss::DeviceLost::from_error(&error).is_some());
    assert!(!reached(p::VkResult_VK_TIMEOUT).unwrap());
    assert!(reached(p::VkResult_VK_SUCCESS).unwrap());
}

#[test]
fn frames_leave_in_submission_order_through_alternating_slots() {
    let dropped = Rc::new(Cell::new(0));
    let mut flights = Flights::default();
    let start = Instant::now();
    let at = |frame: u64| start + Duration::from_millis(frame * 8);
    let (mut slots, mut order) = (vec![], vec![]);
    for frame in 0..10 {
        // The host polls before it submits past the backlog.
        if flights.len() == MAX_DEPTH {
            let completed = flights.complete(|_| Ok(true)).unwrap().unwrap();
            order.push(completed.begin);
        }
        let slot = flights.free_slot(MAX_DEPTH).unwrap();
        slots.push(slot);
        flights.push(flight(&dropped, slot, at(frame)));
        assert_eq!(
            flights.free_slot(MAX_DEPTH),
            (flights.len() < MAX_DEPTH).then_some(1 - slot)
        );
    }
    // The newer frame done first does not overtake the older one.
    let newer = flights.queue[1].slot;
    assert_eq!(flights.complete(|f| Ok(f.slot == newer)).unwrap(), None);
    flights
        .settle_all(|f| {
            order.push(f.begin);
            Ok(true)
        })
        .unwrap();
    assert_eq!(order, (0..10).map(at).collect::<Vec<_>>());
    assert_eq!(slots, [0, 1, 0, 1, 0, 1, 0, 1, 0, 1]);
    assert_eq!(dropped.get(), 10);
    // One frame in flight always takes the first slot.
    let mut single = Flights::default();
    for frame in 0..3 {
        assert_eq!(single.free_slot(1), Some(0));
        single.push(flight(&dropped, 0, at(frame)));
        assert_eq!(single.free_slot(1), None);
        assert_eq!(
            single.complete(|_| Ok(true)).unwrap().unwrap().begin,
            at(frame)
        );
    }
    assert_eq!(dropped.get(), 13);
}

#[test]
fn every_path_returns_each_lease_once_or_keeps_it_for_good() {
    let start = Instant::now();
    // Stream end, rebuild, reconfigure and resolution change wait for every
    // frame in order; each lease goes back as its frame is confirmed.
    let dropped = Rc::new(Cell::new(0));
    let mut flights = Flights::default();
    flights.push(flight(&dropped, 0, start));
    flights.push(flight(&dropped, 1, start));
    let mut seen = vec![];
    flights
        .settle_all(|f| {
            seen.push((f.slot, dropped.get()));
            Ok(true)
        })
        .unwrap();
    assert_eq!(seen, [(0, 0), (1, 1)]);
    assert_eq!(dropped.get(), 2);
    assert!(!flights.unconfirmed);
    // A device lost under the conversion: every lease is free at once.
    let dropped = Rc::new(Cell::new(0));
    let mut flights = Flights::default();
    flights.push(flight(&dropped, 0, start));
    flights.push(flight(&dropped, 1, start));
    flights.abandon();
    assert!(flights.is_empty());
    assert_eq!(dropped.get(), 2);
    // An unconfirmed frame stops the wait: the frames before it went back,
    // it and those after stay until the quarantine keeps them.
    let dropped = Rc::new(Cell::new(0));
    let mut flights = Flights::default();
    flights.push(flight(&dropped, 0, start));
    flights.push(flight(&dropped, 1, start));
    let result = flights.settle_all(|f| {
        if f.slot == 0 {
            Ok(true)
        } else {
            bail!("PyroWave GPU completion timed out after 2 seconds")
        }
    });
    assert!(result.is_err());
    assert!(flights.unconfirmed);
    assert_eq!((dropped.get(), flights.len()), (1, 1));
    flights.retain();
    drop(flights);
    assert_eq!(dropped.get(), 1);
    // A wait that cannot finish is unconfirmed too.
    let dropped = Rc::new(Cell::new(0));
    let mut flights = Flights::default();
    flights.push(flight(&dropped, 0, start));
    assert!(flights.settle_all(|_| Ok(false)).is_err());
    assert!(flights.unconfirmed);
    assert_eq!(dropped.get(), 0);
    flights.retain();
    // Nothing in flight: nothing to wait for.
    let mut empty = Flights::<Resource>::default();
    assert_eq!(empty.complete(|_| unreachable!()).unwrap(), None);
    assert!(empty.settle_all(|_| unreachable!()).is_ok());
}

#[test]
fn vulkan_device_loss_reaches_the_existing_recovery_classifier() {
    let error = completion_result(p::VkResult_VK_ERROR_DEVICE_LOST)
        .unwrap_err()
        .context("encoding a legacy frame");
    assert_eq!(
        crate::device_loss::DeviceLost::from_error(&error),
        Some(crate::device_loss::DeviceLost(DXGI_ERROR_DEVICE_REMOVED.0))
    );
    assert!(format!("{error:#}").contains("VK_ERROR_DEVICE_LOST"));
    assert!(
        crate::device_loss::DeviceLost::from_error(
            &completion_result(p::VkResult_VK_TIMEOUT).unwrap_err()
        )
        .is_none()
    );
}

#[test]
fn quarantine_survives_recreation_and_stops_concurrent_admission_on_the_same_adapter() {
    let adapters = QuarantinedAdapters::default();
    let failed_adapter = [1, 2, 3, 4, 5, 6, 7, 8];
    let other_adapter = [8, 7, 6, 5, 4, 3, 2, 1];
    // Two streams may already be open when one fails; creation and every
    // subsequent frame must both consult the same adapter identity.
    assert!(adapters.check(failed_adapter).is_ok());
    assert!(adapters.check(failed_adapter).is_ok());
    adapters.block(failed_adapter);
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let adapters = &adapters;
            scope.spawn(move || {
                assert!(adapters.check(failed_adapter).is_err());
                assert!(adapters.check(other_adapter).is_ok());
            });
        }
    });
    assert_eq!(adapters.0.lock().unwrap().len(), 1);
}

#[test]
fn legacy_and_record_frames_do_not_accumulate_a_pause_budget() {
    for records in [false, true] {
        let config = Negotiated {
            fps: 60,
            pyrowave_records: records,
            ..Default::default()
        };
        let mut interval = interval(&config);
        let now = Instant::now();
        let period = butterpollo_media_policy::framegen::Rate(config.fps_millihz()).period();
        assert_eq!(interval.next(now, false), period);
        assert_eq!(interval.next(now + Duration::from_secs(5), true), period);
        assert_eq!(
            interval.next(
                now + Duration::from_secs(5) + Duration::from_millis(1),
                false
            ),
            period
        );
    }
}

#[test]
fn warmup_budget_covers_configured_streams_and_degenerate_inputs() {
    let hdr_120 = Negotiated {
        width: 1920,
        height: 1080,
        fps: 120,
        bitrate_kbps: 794_995,
        codec: 3,
        hdr: true,
        packet_size: 1392,
        pyrowave_records: true,
        min_fec: 2,
        ..Default::default()
    };
    let budget = warmup_budget(&hdr_120, true, 20);
    assert!(budget >= WARMUP_MIN_BUDGET);
    assert!(budget <= pyrowave::max_frame_bytes(hdr_120.packet_size, true));
    assert_eq!(budget & 3, 0);

    let legacy = Negotiated {
        pyrowave_records: false,
        ..hdr_120.clone()
    };
    let legacy_budget = warmup_budget(&legacy, false, 0);
    assert!(legacy_budget >= WARMUP_MIN_BUDGET);
    assert_eq!(legacy_budget & 3, 0);

    // Default config, zero bitrate, and tiny/zero packet size all produce valid aligned budgets.
    let default_budget = warmup_budget(&Negotiated::default(), false, 0);
    assert!(default_budget >= WARMUP_MIN_BUDGET);
    assert_eq!(default_budget & 3, 0);
    for packet_size in [0, 16] {
        assert_eq!(
            warmup_budget(
                &Negotiated {
                    packet_size,
                    ..Default::default()
                },
                false,
                0,
            ),
            WARMUP_MIN_BUDGET
        );
    }
    assert_eq!(
        warmup_budget(
            &Negotiated {
                packet_size: 1392,
                bitrate_kbps: 0,
                fps: 60,
                ..Default::default()
            },
            false,
            0,
        ),
        WARMUP_MIN_BUDGET
    );
}

#[test]
fn precreated_slots_are_reused_until_an_established_source_changes() {
    let f16 = (1920, 1080, Pixel::RgbaF16);
    let pq10 = (1920, 1080, Pixel::Rgba10Pq);
    let resized = (2560, 1440, Pixel::RgbaF16);
    // An encoder opened without a sample image keeps its pre-created slots on frame 1.
    assert!(!source_rebuilds_slots(None, f16));
    // Matching source reuses the pre-created slots.
    assert!(!source_rebuilds_slots(Some(f16), f16));
    // Format or dimension changes still rebuild slots.
    assert!(source_rebuilds_slots(Some(f16), pq10));
    assert!(source_rebuilds_slots(Some(f16), resized));
}

#[test]
fn warmup_outcome_quarantines_unconfirmed_gpu_work_but_not_d3d_device_loss() {
    let loss = crate::device_loss::DeviceLost(DXGI_ERROR_DEVICE_REMOVED.0);

    // Confirmed warmup on healthy devices stays clean.
    let (mut unconfirmed, mut lost) = (false, None);
    assert!(finish_warmup(Ok(true), None, &mut unconfirmed, &mut lost).is_ok());
    assert!(!unconfirmed);
    assert_eq!(lost, None);

    // Confirmed Vulkan wait followed by a lost D3D device reports DeviceLost without quarantining.
    let (mut unconfirmed, mut lost) = (false, None);
    let error = finish_warmup(Ok(true), Some(loss), &mut unconfirmed, &mut lost).unwrap_err();
    assert!(!unconfirmed);
    assert_eq!(lost, Some(loss));
    assert_eq!(
        crate::device_loss::DeviceLost::from_error(&error),
        Some(loss)
    );

    // Unconfirmed wait (timeout) marks unconfirmed for quarantine.
    let (mut unconfirmed, mut lost) = (false, None);
    assert!(finish_warmup(Ok(false), None, &mut unconfirmed, &mut lost).is_err());
    assert!(unconfirmed);
    assert_eq!(lost, None);

    // Vulkan device loss during wait marks unconfirmed and records the loss.
    let (mut unconfirmed, mut lost) = (false, None);
    let vk_loss = completion_result(p::VkResult_VK_ERROR_DEVICE_LOST).map(|()| true);
    let error = finish_warmup(vk_loss, None, &mut unconfirmed, &mut lost).unwrap_err();
    assert!(unconfirmed);
    assert_eq!(lost, Some(loss));
    assert_eq!(
        crate::device_loss::DeviceLost::from_error(&error),
        Some(loss)
    );
}
