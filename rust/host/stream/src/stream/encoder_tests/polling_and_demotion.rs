//! Unit tests for traced encoder polling, output wait prediction, and compute queue demotion.

use super::super::*;
use super::{MockEncoder, frame};
use crate::stream::video::{CountedFailures, count_rebuild_failure};

#[test]
fn a_traced_poll_records_its_span_and_a_claim_names_the_one_it_held() -> Result<()> {
    let mut recovery = EncoderRecovery {
        queries: Some(Vec::new()),
        ..Default::default()
    };
    let mut encoder = Some(MockEncoder {
        queued: [frame(true)].into(),
        ..Default::default()
    });
    recovery.collect(&mut encoder, &Default::default(), Instant::now)?;
    let queries = recovery.queries.take().unwrap();
    assert_eq!(queries.len(), 1);
    assert!(queries[0].0 <= queries[0].1);
    let start = Instant::now();
    let ms = |n| start + Duration::from_millis(n);
    let spans = [(ms(0), ms(1)), (ms(2), ms(3))];
    // A picture published during the second poll waited for its end.
    let held = QueryTrace::new(&spans, start + Duration::from_micros(2_400));
    assert_eq!(
        held,
        QueryTrace {
            queries: 2,
            query_us: 2_000,
            published_in_query: true,
            held_us: 600,
        }
    );
    // Between polls, or at a poll's end, nothing held it.
    let free = QueryTrace::new(&spans, ms(1));
    assert!(!free.published_in_query);
    assert_eq!(free.held_us, 0);
    assert!(QueryTrace::new(&[], ms(0)) == QueryTrace::default());
    Ok(())
}

#[test]
fn the_next_output_poll_waits_for_a_predicted_finish_and_never_sooner_than_the_poll() {
    let now = Instant::now();
    assert_eq!(
        next_output_poll(None::<&MockEncoder>, now),
        now + OUTPUT_POLL
    );
    let mut encoder = MockEncoder::default();
    assert_eq!(next_output_poll(Some(&encoder), now), now + OUTPUT_POLL);
    encoder.due = Some(now + Duration::from_millis(3));
    assert_eq!(
        next_output_poll(Some(&encoder), now),
        now + Duration::from_millis(3)
    );
    encoder.due = Some(now);
    assert_eq!(next_output_poll(Some(&encoder), now), now + OUTPUT_POLL);
}

#[test]
fn only_a_blocking_poll_keeps_clear_of_the_claim() {
    let now = Instant::now();
    let close = now + Duration::from_micros(300);
    let mut encoder = MockEncoder::default();
    assert!(!poll_fits(Some(&encoder), now, close));
    assert!(poll_fits(
        Some(&encoder),
        now,
        now + Duration::from_millis(2)
    ));
    encoder.immediate = true;
    assert!(poll_fits(Some(&encoder), now, close));
    assert!(poll_fits(None::<&MockEncoder>, now, close));
}

#[test]
fn an_idle_full_encoder_sleeps_on_its_event_or_prediction_but_not_after_a_device_loss() -> Result<()>
{
    let timer = butterpollo_win_graphics::timing::Timer::new()?;
    let event = butterpollo_win_graphics::timing::Signal::new()?;
    event.set()?;
    let mut recovery = EncoderRecovery::default();
    let mut encoder = MockEncoder {
        queued: [frame(false)].into(),
        event: Some(event),
        ..Default::default()
    };
    // A set event ends the wait at once.
    let started = Instant::now();
    wait_for_output(&timer, &recovery, Some(&encoder))?;
    assert!(started.elapsed() < Duration::from_millis(50));
    // Without an event or prediction the next poll comes at once when it waits in the driver.
    encoder.event = None;
    let started = Instant::now();
    wait_for_output(&timer, &recovery, Some(&encoder))?;
    assert!(started.elapsed() < Duration::from_millis(50));
    // A poll that returns at once sleeps one output poll first, never spinning on a full encoder.
    encoder.immediate = true;
    let started = Instant::now();
    wait_for_output(&timer, &recovery, Some(&encoder))?;
    assert!(started.elapsed() >= OUTPUT_POLL);
    assert!(started.elapsed() < Duration::from_millis(50));
    encoder.immediate = false;
    // A prediction holds the poll until the frame can be done.
    let due = Instant::now() + Duration::from_millis(3);
    encoder.due = Some(due);
    wait_for_output(&timer, &recovery, Some(&encoder))?;
    assert!(Instant::now() >= due);
    // After a device loss nothing is pending, so nothing waits or arms.
    encoder.due = Some(Instant::now() + Duration::from_secs(5));
    recovery.device_loss = Some(DeviceLost(0x887a0005_u32 as i32));
    let started = Instant::now();
    wait_for_output(&timer, &recovery, Some(&encoder))?;
    assert!(started.elapsed() < Duration::from_secs(1));
    // Nor with nothing in flight.
    recovery.device_loss = None;
    encoder.queued.clear();
    let started = Instant::now();
    wait_for_output(&timer, &recovery, Some(&encoder))?;
    assert!(started.elapsed() < Duration::from_secs(1));
    Ok(())
}

#[test]
fn compute_conversion_moves_after_two_failures_within_the_window_only() {
    let start = Instant::now();
    let mut demotion = ComputeDemotion::default();
    // Two failures ten minutes apart keep compute.
    assert!(!demotion.failed(start, true, false));
    assert!(!demotion.failed(start + Duration::from_secs(600), true, false));
    assert!(!demotion.demoted());
    // Failures while the graphics queue converted do not count against compute.
    assert!(!demotion.failed(start + Duration::from_secs(610), false, false));
    assert!(!demotion.demoted());
    // A second compute failure within the window moves it, once.
    let moved = start + Duration::from_secs(600) + COMPUTE_FAILURE_WINDOW;
    assert!(demotion.failed(moved, true, false));
    assert!(demotion.demoted());
    assert!(!demotion.failed(moved + Duration::from_secs(1), true, false));
}

#[test]
fn a_demoted_session_tries_compute_again_after_quiet_minutes() {
    let start = Instant::now();
    let mut demotion = ComputeDemotion::default();
    assert!(!demotion.failed(start, true, false));
    assert!(demotion.failed(start + Duration::from_secs(5), true, false));
    let moved = start + Duration::from_secs(5);
    let almost = moved + COMPUTE_RETRY_QUIET - Duration::from_secs(1);
    assert!(!demotion.retry_due(almost));
    // A failure on the graphics queue restarts the quiet period.
    let failed = moved + Duration::from_secs(120);
    assert!(!demotion.failed(failed, false, false));
    assert!(!demotion.retry_due(moved + COMPUTE_RETRY_QUIET));
    assert!(demotion.retry_due(failed + COMPUTE_RETRY_QUIET));
    assert!(!demotion.demoted());
    assert!(!demotion.retry_due(failed + COMPUTE_RETRY_QUIET * 2));
    // Back on compute, one failure alone does not move it again.
    let back = failed + COMPUTE_RETRY_QUIET;
    assert!(!demotion.failed(back + Duration::from_secs(1), true, false));
    assert!(demotion.failed(back + Duration::from_secs(30), true, false));
}

#[test]
fn a_creation_fallback_keeps_the_graphics_queue_sticky() {
    let start = Instant::now();
    let mut demotion = ComputeDemotion::default();
    demotion.demote_sticky(start);
    assert!(demotion.demoted() && demotion.sticky());
    assert!(!demotion.retry_due(start + COMPUTE_RETRY_QUIET * 10));
    // A later failure keeps the graphics queue without a second move.
    assert!(!demotion.failed(start + Duration::from_secs(1), false, true));
}

#[test]
fn a_running_nvenc_d3d12_failure_demotes_on_its_second_failure_and_sticks() {
    let start = Instant::now();
    let mut demotion = ComputeDemotion::default();
    // A failure, a rebuild on D3D12, then a second failure in the window.
    assert!(!demotion.failed(start, true, true));
    assert!(demotion.failed(start + Duration::from_secs(1), true, true));
    assert!(demotion.demoted() && demotion.sticky());
    assert!(!demotion.retry_due(start + COMPUTE_RETRY_QUIET * 10));
}

#[test]
fn a_rebuild_without_a_new_failure_does_not_count_twice() {
    let start = Instant::now();
    let mut recovery = EncoderRecovery::default();
    let mut counted = CountedFailures::default();
    let mut demotion = ComputeDemotion::default();
    // The first failure, then the rebuild that follows it.
    let since = recovery.record_failure(start);
    assert!(!count_rebuild_failure(
        &recovery,
        &mut counted,
        &mut demotion,
        start,
        true,
        true
    ));
    // The rebuild produced no frame and is rebuilt again with no new failure:
    // the same event must not count as the second failure.
    assert!(!count_rebuild_failure(
        &recovery,
        &mut counted,
        &mut demotion,
        start + Duration::from_secs(1),
        true,
        true
    ));
    assert!(!demotion.demoted());
    // A second real failure, still in the same recovery episode, counts, so a
    // D3D12 path that fails, rebuilds and fails again demotes stickily.
    assert_eq!(
        recovery.record_failure(start + Duration::from_secs(2)),
        since
    );
    assert!(count_rebuild_failure(
        &recovery,
        &mut counted,
        &mut demotion,
        start + Duration::from_secs(2),
        true,
        true
    ));
    assert!(demotion.demoted() && demotion.sticky());
}

#[test]
fn a_device_loss_never_counts_towards_compute_demotion() {
    let start = Instant::now();
    let mut recovery = EncoderRecovery::default();
    let mut counted = CountedFailures::default();
    let mut demotion = ComputeDemotion::default();
    recovery.record_failure(start);
    counted.device_loss(recovery.failure_epoch);
    assert!(!count_rebuild_failure(
        &recovery,
        &mut counted,
        &mut demotion,
        start,
        true,
        true
    ));
    // The next real failure in the same episode counts on its own, and a
    // second one in the window moves conversion to the graphics queue.
    recovery.record_failure(start + Duration::from_secs(1));
    assert!(!count_rebuild_failure(
        &recovery,
        &mut counted,
        &mut demotion,
        start + Duration::from_secs(1),
        true,
        true
    ));
    recovery.record_failure(start + Duration::from_secs(2));
    assert!(count_rebuild_failure(
        &recovery,
        &mut counted,
        &mut demotion,
        start + Duration::from_secs(2),
        true,
        true
    ));
    assert!(demotion.demoted() && demotion.sticky());
}
