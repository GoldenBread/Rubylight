//! Concurrency unit tests for `Monitor` DDI guard and teardown synchronization.

use super::*;
use std::sync::Barrier;
use std::thread;

#[test]
fn dekker_ddi_guard_and_teardown_never_overlap() {
    for _ in 0..200 {
        let monitor = Arc::new(Monitor::pending(1, 1, 100, Vec::new()));
        let in_ddi = Arc::new(AtomicU32::new(0));
        let departed = Arc::new(AtomicBool::new(false));
        let start = Arc::new(Barrier::new(3));

        let mut callers = Vec::new();
        for _ in 0..2 {
            let m = monitor.clone();
            let in_ddi = in_ddi.clone();
            let departed = departed.clone();
            let start = start.clone();
            callers.push(thread::spawn(move || {
                start.wait();
                for _ in 0..50 {
                    let Some(_guard) = m.ddi() else {
                        break;
                    };
                    in_ddi.fetch_add(1, Ordering::SeqCst);
                    assert!(
                        !departed.load(Ordering::SeqCst),
                        "DDI entered after teardown/departure"
                    );
                    thread::yield_now();
                    in_ddi.fetch_sub(1, Ordering::SeqCst);
                }
            }));
        }

        start.wait();
        monitor.teardown();
        departed.store(true, Ordering::SeqCst);
        assert_eq!(in_ddi.load(Ordering::SeqCst), 0);
        assert!(monitor.ddi().is_none());

        for handle in callers {
            handle.join().unwrap();
        }
    }
}

#[test]
fn swap_chain_processor_reports_not_running_before_run() {
    let monitor = Monitor::pending(1, 1, 101, Vec::new());
    let proc = SwapChainProcessor::new();
    assert!(!proc.is_running());
    *lock(&monitor.swap) = Some(proc);
    // An unstarted SwapChainProcessor does not advertise MONITOR_SWAP_CHAIN.
    assert_eq!(monitor.status().flags & control::MONITOR_SWAP_CHAIN, 0);
    drop(monitor.take_swap());
}

#[test]
fn ring_handoff_reports_settled_and_timeout() {
    let handoff = RingHandoff::new();
    let worker = handoff.enter_worker();
    assert!(!handoff.wait_cleared(2, Duration::from_millis(20)));
    handoff.note_settled(worker, None, 2);
    assert!(handoff.wait_cleared(2, Duration::from_millis(20)));
    handoff.exit_worker(worker);
}

#[test]
fn clear_frame_ring_returns_not_found_for_missing_session() {
    assert_eq!(clear_frame_ring(999, 12345), crate::STATUS_NOT_FOUND);
    assert_eq!(
        clear_frame_ring_generation(999, 12345, 1),
        crate::STATUS_NOT_FOUND
    );
}
