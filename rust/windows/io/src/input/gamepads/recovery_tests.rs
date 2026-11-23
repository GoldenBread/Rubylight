//! Unit tests for `DriverLost` (`0x80070006`) propagation and dead-handle cleanup in `Gamepads` (#138).

use super::*;
use std::collections::VecDeque;

fn handle_lost() -> anyhow::Error {
    gamepad_backend::driver_error(windows::core::Error::from_hresult(E_HANDLE))
}

fn fake_pads(driver: gamepad_backend::fake::Driver) -> Gamepads {
    let mut pads = Gamepads::open_options(
        butterpollo_input_policy::input_policy::USBIP_DUALSENSE,
        butterpollo_input_policy::input_policy::Policy::resolve(&Default::default()).unwrap(),
        Default::default(),
    )
    .unwrap();
    pads.profile = 0;
    pads.available = !0;
    pads.backend = gamepad_backend::Backend::Fake(driver);
    pads
}

#[test]
fn plug_in_free_slot_bails_immediately_on_driver_lost() {
    let slots = std::sync::Mutex::new([false; 16]);
    let mut tried = Vec::new();
    let error = plug_in_free_slot(&slots, true, |slot| {
        tried.push(slot);
        Err(handle_lost())
    })
    .unwrap_err();
    assert!(gamepad_backend::driver_lost(&error));
    assert_eq!(
        tried,
        vec![0],
        "must not hammer all 16 slots on a dead handle"
    );
    assert!(!slots.lock().unwrap()[0], "slot reservation released");

    // Non-lost per-slot errors still advance to the next slot when every_slot is true.
    let mut tried = Vec::new();
    let global = plug_in_free_slot(&slots, true, |slot| {
        tried.push(slot);
        if slot == 0 {
            anyhow::bail!("slot busy");
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(global, Some(1));
    assert_eq!(tried, vec![0, 1]);
}

#[test]
fn reclaim_until_lost_stops_on_first_driver_lost() {
    let _stranded = super::tests::stranded_slots();
    let s1 = Slot::reserve().unwrap();
    let s2 = Slot::reserve().unwrap();
    let g1 = s1.id();
    let g2 = s2.id();
    std::mem::forget(s1);
    std::mem::forget(s2);
    STRANDED.lock().unwrap().insert(g1);
    STRANDED.lock().unwrap().insert(g2);

    let mut calls = Vec::new();
    let mut snapshot = *SLOTS.lock().unwrap();
    let lost = reclaim_until_lost(&mut snapshot, |global| {
        calls.push(global);
        Err(handle_lost())
    });
    assert!(lost.as_ref().is_some_and(gamepad_backend::driver_lost));
    assert_eq!(
        calls.len(),
        1,
        "must stop hammering stranded slots once handle is refused"
    );
    assert!(STRANDED.lock().unwrap().contains(&g1));
    assert!(STRANDED.lock().unwrap().contains(&g2));

    // Clean up test slots.
    STRANDED.lock().unwrap().remove(&g1);
    STRANDED.lock().unwrap().remove(&g2);
    let mut slots = SLOTS.lock().unwrap();
    slots[g1 as usize] = false;
    slots[g2 as usize] = false;
}

#[test]
fn controller_disconnect_unplug_refusal_marks_pads_lost_instead_of_swallowing() {
    let _stranded = super::tests::stranded_slots();
    let slot = Slot::reserve().unwrap();
    let global = slot.id();
    std::mem::forget(slot);

    let mut driver = gamepad_backend::fake::Driver::default();
    driver.unplugs.push_back(Err(handle_lost()));
    let mut pads = fake_pads(driver);
    pads.active.insert(0, global);
    pads.profiles.insert(0, 1);

    // Controller 0 leaves (e.g. AFK disconnect): active mask drops bit 0.
    let error = pads
        .apply(&Event::Controller {
            id: 0,
            active: 0,
            buttons: 0,
            left_trigger: 0,
            right_trigger: 0,
            sticks: [0; 4],
        })
        .unwrap_err();
    assert!(gamepad_backend::driver_lost(&error), "{error:#}");
    assert!(
        pads.lost(),
        "dead handle must be flagged for immediate close"
    );

    drop(pads);
    assert!(
        !SLOTS.lock().unwrap()[global as usize],
        "slot freed on lost drop"
    );
    assert!(
        !STRANDED.lock().unwrap().contains(&global),
        "lost handle must not strand the slot"
    );
}

#[test]
fn feedback_marks_pads_lost_when_deferred_unplug_or_recreate_fails_with_driver_lost() {
    let _stranded = super::tests::stranded_slots();
    let slot = Slot::reserve().unwrap();
    let global = slot.id();
    std::mem::forget(slot);

    let removed_err = || -> anyhow::Error {
        windows::core::Error::from_hresult(ERROR_DEVICE_REMOVED.to_hresult()).into()
    };
    let mut driver = gamepad_backend::fake::Driver::default();
    driver
        .feedback
        .insert(u32::from(global), VecDeque::from([Err(removed_err())]));
    driver.unplugs.push_back(Err(handle_lost()));
    let mut pads = fake_pads(driver);
    pads.active.insert(0, global);
    pads.profiles.insert(0, 1);
    pads.dropped.insert(0);

    let _ = pads.feedback();
    assert!(
        pads.lost(),
        "deferred unplug DriverLost in feedback must mark pads lost"
    );
    drop(pads);
    assert!(!SLOTS.lock().unwrap()[global as usize]);

    // Also cover the non-dropped recreate() path hitting DriverLost during feedback().
    let slot = Slot::reserve().unwrap();
    let global = slot.id();
    std::mem::forget(slot);
    let mut driver = gamepad_backend::fake::Driver::default();
    driver
        .feedback
        .insert(u32::from(global), VecDeque::from([Err(removed_err())]));
    driver.unplugs.push_back(Ok(()));
    driver.plugs.push_back(Err(handle_lost()));
    let mut pads = fake_pads(driver);
    pads.active.insert(0, global);
    pads.profiles.insert(0, 1);

    let _ = pads.feedback();
    assert!(
        pads.lost(),
        "recreate DriverLost in feedback must mark pads lost"
    );
    drop(pads);
    assert!(!SLOTS.lock().unwrap()[global as usize]);
}

#[test]
fn drop_frees_slots_without_stranding_when_unplug_hits_driver_lost() {
    let _stranded = super::tests::stranded_slots();
    let s1 = Slot::reserve().unwrap();
    let s2 = Slot::reserve().unwrap();
    let g1 = s1.id();
    let g2 = s2.id();
    std::mem::forget(s1);
    std::mem::forget(s2);

    let mut driver = gamepad_backend::fake::Driver::default();
    driver.unplugs.push_back(Err(handle_lost()));
    // If drop tried a second unplug on the dead handle, it would return a
    // non-lost error and strand g2; stopping on the first DriverLost prevents it.
    driver
        .unplugs
        .push_back(Err(anyhow::anyhow!("should not be called")));
    let mut pads = fake_pads(driver);
    pads.active.insert(0, g1);
    pads.active.insert(1, g2);
    drop(pads);

    let slots = SLOTS.lock().unwrap();
    assert!(!slots[g1 as usize] && !slots[g2 as usize]);
    let stranded = STRANDED.lock().unwrap();
    assert!(!stranded.contains(&g1) && !stranded.contains(&g2));
}
