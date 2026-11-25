use super::*;

#[test]
fn a_remote_monitors_repair_that_failed_says_so_on_the_launchs_card() {
    let warnings = Warnings::default();
    report_repair(&warnings, Ok(()));
    assert!(warnings.snapshot().is_empty());
    report_repair(
        &warnings,
        Err(anyhow::anyhow!("refused").context("putting it back")),
    );
    let card = warnings.snapshot();
    assert_eq!(card.len(), 1);
    assert_eq!(card[0].code, "display_monitor_repair");
    assert!(card[0].message.contains("putting it back: refused"));
}

#[test]
fn a_monitor_being_made_holds_no_lock_others_read_and_a_second_request_waits_for_it() {
    let f = crate::state::test_support::Fixture::new();
    let h = &f.host;
    let mode = (1920, 1080, 60_000, false);
    let Reserved::Slot(slot) = reserve(h, "a", mode).unwrap() else {
        panic!("nothing was made yet");
    };
    // `/applist`, `/serverinfo` and the maintenance loop read the map meanwhile.
    assert!(h.monitors.try_lock().is_ok());
    let second = std::thread::spawn({
        let h = h.clone();
        move || match reserve(&h, "a", mode).unwrap() {
            Reserved::Existing(lease) => lease,
            Reserved::Slot(_) => panic!("made twice"),
        }
    });
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(!second.is_finished());
    let mut made = Retained::inert("a");
    made.mode = mode;
    let lease = publish(slot, Arc::new(made)).unwrap();
    assert!(Arc::ptr_eq(&second.join().unwrap(), &lease));
    assert_eq!(outputs(h), ["a"]);
    h.monitors.lock().unwrap().clear();
}

#[test]
fn monitors_being_made_count_against_the_capacity_and_a_disconnect_cancels_one() {
    let f = crate::state::test_support::Fixture::new();
    let h = &f.host;
    let mode = (1280, 720, 60_000, false);
    let [a, b, c, d] = ["a", "b", "c", "d"].map(|client| match reserve(h, client, mode).unwrap() {
        Reserved::Slot(slot) => slot,
        Reserved::Existing(_) => panic!("nothing was made yet"),
    });
    assert!(reserve(h, "e", mode).is_err(), "four being made fill it");
    h.monitors_creating.cancel(Some("a"));
    let mut made = Retained::inert("a");
    made.mode = mode;
    let error = publish(a, Arc::new(made)).err().unwrap();
    assert!(format!("{error:#}").contains("disconnected"), "{error:#}");
    assert!(h.monitors.lock().unwrap().is_empty());
    drop((b, c, d));
    assert!(matches!(reserve(h, "e", mode), Ok(Reserved::Slot(_))));
    assert!(h.monitors_creating.locked().is_empty());
}

#[test]
fn disconnecting_published_monitor_removes_entry_for_post_publish_check() {
    let f = crate::state::test_support::Fixture::new();
    let h = &f.host;
    let mode = (1920, 1080, 60_000, false);
    let Reserved::Slot(slot) = reserve(h, "c1", mode).unwrap() else {
        panic!("expected new slot");
    };
    let mut made = Retained::inert("c1");
    made.mode = mode;
    let lease = publish(slot, Arc::new(made)).unwrap();
    assert!(
        h.monitors
            .lock()
            .unwrap()
            .get("c1")
            .is_some_and(|m| Arc::ptr_eq(m, &lease))
    );
    // Simulate a concurrent disconnect removing `c1` before `activate` calls retain(..., true).
    h.monitors.lock().unwrap().remove("c1");
    assert!(
        !h.monitors
            .lock()
            .unwrap()
            .get("c1")
            .is_some_and(|m| Arc::ptr_eq(m, &lease))
    );
}
