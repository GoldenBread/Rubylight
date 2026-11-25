//! Unit tests for [`Gamepads`] release, device-removal recovery, refresh, and shared helpers.

use super::*;
use std::collections::VecDeque;

mod feedback_touch;
mod steam_profiles;

fn touch(id: u8, touchpad: u8, event: u8, pointer: u32) -> Event {
    Event::ControllerTouch {
        id,
        touchpad,
        event,
        pointer,
        x: 0.25,
        y: 0.5,
        pressure: 1.0,
    }
}

/// Held by the tests that check a slot stays stranded, and by those whose driver unplugs any
/// slot: their `ensure` reclaims every stranded slot, another test's too.
static STRANDED_SLOTS: std::sync::Mutex<()> = std::sync::Mutex::new(());
pub(super) fn stranded_slots() -> std::sync::MutexGuard<'static, ()> {
    STRANDED_SLOTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[test]
fn release_skips_a_pad_that_holds_nothing_and_stays_loud_for_one_that_does() {
    // A backend without a driver handle: every report to it fails.
    let mut pads = Gamepads::open_options(
        butterpollo_input_policy::input_policy::USBIP_DUALSENSE,
        butterpollo_input_policy::input_policy::Policy::resolve(&Default::default()).unwrap(),
        Default::default(),
    )
    .unwrap();
    pads.profile = 0;
    // Plugged by a launch's controller preparation, then released at park.
    pads.active.insert(0, 15);
    pads.profiles.insert(0, 1);
    pads.release_all().unwrap();
    assert_eq!(pads.active.get(&0), Some(&15), "still plugged");

    let neutral = Event::Controller {
        id: 0,
        active: 1,
        buttons: 0,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    };
    pads.states.insert(0, (neutral, Default::default()));
    assert!(
        pads.release_all().is_err(),
        "a pad with a state is released"
    );
    pads.states.clear();
    pads.pointers.insert((0, 17), 0);
    assert!(
        pads.release_all().is_err(),
        "a pad with a touch is released"
    );
    // Drop would unplug through the missing handle and strand slot 15.
    pads.active.clear();
}

/// A Steam Controller's left grip touch and a Switch 2 Pro Controller's C
/// button share a flag; only the Steam Controller's pad may see it
/// (nightjar#125: the host masked the grip away for every pad).
#[test]
fn the_steam_controllers_left_grip_bit_reaches_its_own_pad() {
    use butterpollo_input_policy::input_policy::VHF_STEAM_CONTROLLER;
    use butterpollo_input_policy::switch2::BUTTON_C;
    assert_eq!(BUTTON_C, 0x0040_0000);
    assert_eq!(
        profile_button_word(Some(VHF_STEAM_CONTROLLER), BUTTON_C | 0x1000),
        BUTTON_C | 0x1000
    );
    // Every other pad has no grip sense, and its C press is not a button there.
    for profile in [None, Some(4), Some(6)] {
        assert_eq!(profile_button_word(profile, BUTTON_C | 0x1000), 0x1000);
    }
}

/// Windows retires the virtual pads when it leaves D0; the driver answers every
/// request with ERROR_DEVICE_REMOVED until its slot is created again, and no
/// other driver error is a reason to create one. The driver refusing the
/// handle itself (#138) closes the pads instead, which only its answer marks.
#[test]
fn only_the_drivers_device_removed_error_creates_the_pad_again() {
    use gamepad_backend::{driver_error, driver_lost};
    use windows::Win32::Foundation::{E_HANDLE, ERROR_DEVICE_REMOVED};
    let removed = anyhow::Error::new(windows::core::Error::from_hresult(
        ERROR_DEVICE_REMOVED.to_hresult(),
    ));
    assert!(device_removed(&removed));
    assert!(!driver_lost(&removed));
    assert!(device_removed(
        &removed.context("the gamepad driver could not report feedback")
    ));
    let refused = driver_error(windows::core::Error::from_hresult(E_HANDLE));
    assert!(!device_removed(&refused));
    assert!(driver_lost(&refused));
    assert!(format!("{refused:#}").contains("0x80070006"), "{refused:#}");
    let refused = refused.context("the gamepad driver could not plug a vhf_ds5 controller");
    assert!(driver_lost(&refused), "through added context");
    assert!(!device_removed(&refused));
    // A bare E_HANDLE, not from the driver, is neither.
    let bare = anyhow::Error::new(windows::core::Error::from_hresult(E_HANDLE));
    assert!(!device_removed(&bare));
    assert!(!driver_lost(&bare));
    assert!(!driver_lost(&driver_error(ERROR_DEVICE_REMOVED.into())));
    assert!(!device_removed(&anyhow::anyhow!("injected failure")));
    assert!(!driver_lost(&anyhow::anyhow!("injected failure")));
}

fn refused_error() -> anyhow::Error {
    gamepad_backend::driver_error(windows::core::Error::from_hresult(
        windows::Win32::Foundation::E_HANDLE,
    ))
}

/// #138: a state the driver refuses the handle for marks the pads lost for
/// the gamepad thread to close; the pad is not created again in place.
#[test]
fn a_refused_handle_marks_the_pads_lost_without_creating_the_pad_again() {
    use super::super::gamepad_thread::Pads;
    let mut pads = scripted_pads();
    pads.active.insert(0, 2);
    pads.profiles.insert(0, 4);
    script(&mut pads).submits.push_back(Err(refused_error()));
    let error = Pads::apply(&mut pads, &state(0, 1)).unwrap_err();
    assert!(gamepad_backend::driver_lost(&error));
    assert!(Pads::lost(&pads));
    assert_eq!(script(&mut pads).calls, ["submit 2"], "no plug, no retry");
    assert!(!pads.recreated_at.contains_key(&0));
    // Slot 2 was never reserved here: closing must not free it.
    pads.active.clear();
    // Any other failure leaves the pads open.
    let mut pads = scripted_pads();
    pads.active.insert(0, 2);
    pads.profiles.insert(0, 4);
    script(&mut pads)
        .submits
        .push_back(Err(anyhow::anyhow!("injected failure")));
    assert!(Pads::apply(&mut pads, &state(0, 1)).is_err());
    assert!(!Pads::lost(&pads));
    pads.active.clear();
}

/// #138: the refusal showed first in the feedback poll, 118 times a second.
/// One poll marks the pads lost, and none is a pad to create again.
#[test]
fn a_feedback_poll_refused_the_handle_marks_the_pads_lost() {
    let mut pads = scripted_pads();
    pads.active.insert(0, 2);
    pads.profiles.insert(0, 4);
    script(&mut pads)
        .feedback
        .insert(2, VecDeque::from([Err(refused_error())]));
    assert!(pads.feedback().is_empty());
    assert!(pads.lost());
    assert_eq!(script(&mut pads).calls, ["feedback 2"], "no plug");
    pads.active.clear();
}

/// Lost pads are closed without unplug requests on the refused handle, and
/// their slots are free for the pads opened next rather than stranded.
#[test]
fn lost_pads_free_their_slots_without_unplugging() {
    let _stranded = stranded_slots();
    let slot = Slot::reserve().unwrap();
    let global = slot.id();
    std::mem::forget(slot);
    let mut pads = scripted_pads();
    pads.active.insert(0, global);
    script(&mut pads)
        .unplugs
        .push_back(Err(anyhow::anyhow!("unplug sent on a refused handle")));
    pads.lost = true;
    drop(pads);
    assert!(!STRANDED.lock().unwrap().contains(&global));
    assert!(!SLOTS.lock().unwrap()[global as usize]);
}

/// A re-creation the driver refuses keeps the pad, so its next input tries
/// again, and a pad that is not plugged has nothing to create.
#[test]
fn a_recreate_without_the_driver_keeps_the_pad_for_the_next_attempt() {
    let mut pads = broken_pads();
    assert!(pads.recreate(0).is_ok(), "no pad, nothing to create");
    pads.active.insert(0, 15);
    pads.profiles.insert(0, 4);
    assert!(
        pads.recreate(0).is_err(),
        "the broken driver reports its error"
    );
    assert_eq!(pads.active.get(&0), Some(&15));
    assert_eq!(pads.profiles.get(&0), Some(&4));
    // A ViGEmBus pad goes through the same plug: the client replaces the
    // target in the same slot if its bus ever reports the device gone.
    pads.backend = gamepad_backend::Backend::Vigem(vigem::Client::detached().unwrap());
    pads.profiles
        .insert(0, butterpollo_input_policy::input_policy::VIGEM_X360);
    assert!(pads.recreate(0).is_err());
    assert_eq!(pads.active.get(&0), Some(&15));
    pads.active.clear();
}

/// Windows leaves D0: the VHF driver answers input with ERROR_DEVICE_REMOVED
/// until its slot is created again. The same slot is plugged, the state is
/// retried, and no second pad is made.
#[test]
fn a_device_removed_submit_creates_the_same_pad_again_and_retries_it() {
    let mut pads = scripted_pads();
    pads.active.insert(0, 2);
    pads.profiles.insert(0, 4);
    script(&mut pads)
        .submits
        .push_back(Err(device_removed_error()));
    pads.apply(&state(0, 1)).unwrap();
    assert_eq!(
        script(&mut pads).calls,
        ["submit 2", "plug 2", "submit 2"],
        "the same slot is created again and the state retried once"
    );
    assert_eq!(pads.active.get(&0), Some(&2), "one pad, in its slot");
    assert_eq!(pads.active.len(), 1);
    assert_eq!(pads.profiles.get(&0), Some(&4));
    assert!(pads.recreated_at.contains_key(&0));

    // A driver that is still gone waits: no second plug within the second.
    script(&mut pads)
        .submits
        .push_back(Err(device_removed_error()));
    assert!(pads.apply(&state(0, 1)).is_err());
    assert_eq!(
        script(&mut pads)
            .calls
            .iter()
            .filter(|call| call.starts_with("plug"))
            .count(),
        1,
        "throttled"
    );
    pads.active.clear();
}

/// The one-second retry belongs to the pad, not to every pad a client has: a
/// second controller that is lost while the first waits is created again at
/// once.
#[test]
fn a_recreate_waits_a_second_for_that_pad_and_not_for_the_others() {
    let mut pads = scripted_pads();
    pads.active.insert(0, 2);
    pads.active.insert(1, 3);
    pads.profiles.insert(0, 4);
    pads.profiles.insert(1, 4);
    for id in [0, 1] {
        script(&mut pads)
            .submits
            .push_back(Err(device_removed_error()));
        pads.apply(&state(id, 0b11)).unwrap();
    }
    assert_eq!(
        script(&mut pads).calls,
        [
            "submit 2", "plug 2", "submit 2", "submit 3", "plug 3", "submit 3"
        ]
    );
    let now = Instant::now();
    assert!(pads.recreate_due(4, now));
    assert!(!pads.recreate_due(4, now + RECREATE_RETRY / 2));
    assert!(
        pads.recreate_due(4, now + RECREATE_RETRY),
        "asked again after the second"
    );
    pads.active.clear();
}

/// A game's rumble poll can find the pad gone before the player touches
/// anything; the slot is created again before the next controller state.
#[test]
fn a_feedback_poll_that_finds_the_device_gone_creates_the_pad_again() {
    let mut pads = scripted_pads();
    pads.active.insert(0, 2);
    pads.profiles.insert(0, 4);
    script(&mut pads)
        .feedback
        .insert(2, VecDeque::from([Err(device_removed_error())]));
    assert!(pads.feedback().is_empty());
    assert_eq!(script(&mut pads).calls, ["feedback 2", "plug 2"]);
    assert_eq!(pads.active.get(&0), Some(&2));
    // The poll is throttled like the input path while the driver stays gone.
    script(&mut pads)
        .feedback
        .insert(2, VecDeque::from([Err(device_removed_error())]));
    assert!(pads.feedback().is_empty());
    assert_eq!(
        script(&mut pads)
            .calls
            .iter()
            .filter(|call| call.starts_with("plug"))
            .count(),
        1,
        "no plug within the second"
    );
    pads.active.clear();
}

/// A pad the client removed whose unplug the driver refused is never created
/// again by a feedback poll; a sleep that takes the device lets the deferred
/// unplug finish instead.
#[test]
fn a_feedback_poll_never_brings_back_a_pad_the_client_removed() {
    let _stranded = stranded_slots();
    let slot = Slot::reserve().unwrap();
    let global = slot.id();
    std::mem::forget(slot);
    let mut pads = scripted_pads();
    pads.active.insert(0, global);
    pads.profiles.insert(0, 4);
    script(&mut pads)
        .unplugs
        .push_back(Err(anyhow::anyhow!("the driver is busy")));
    // The client's mask no longer has the controller: its removal is deferred.
    pads.apply(&state(0, 0)).unwrap();
    assert_eq!(pads.active.get(&0), Some(&global));
    assert!(pads.dropped.contains(&0));
    // The poll finds the device gone and the unplug now works: it is never
    // plugged again.
    script(&mut pads).feedback.insert(
        u32::from(global),
        VecDeque::from([Err(device_removed_error())]),
    );
    assert!(pads.feedback().is_empty());
    assert!(!pads.active.contains_key(&0), "the pad is gone");
    assert!(!pads.dropped.contains(&0));
    assert!(
        !SLOTS.lock().unwrap()[global as usize],
        "its slot is free again"
    );
    assert_eq!(
        script(&mut pads).calls,
        [
            format!("unplug {global}"),
            format!("feedback {global}"),
            format!("unplug {global}")
        ]
    );
}

/// A client that sends the controller active again says the removal is over:
/// the pad must not be forgotten as removed by the next feedback poll.
#[test]
fn a_state_that_turns_a_pad_back_on_clears_its_deferred_removal() {
    let mut pads = scripted_pads();
    pads.active.insert(0, 2);
    pads.profiles.insert(0, 4);
    script(&mut pads)
        .unplugs
        .push_back(Err(anyhow::anyhow!("the driver is busy")));
    // The client's mask no longer has the controller: its removal is deferred.
    pads.apply(&state(0, 0)).unwrap();
    assert!(pads.dropped.contains(&0));
    // The next state has it back before the poll.
    pads.apply(&state(0, 1)).unwrap();
    assert!(
        !pads.dropped.contains(&0),
        "a pad the client turns on again is not being removed"
    );
    script(&mut pads).calls.clear();
    script(&mut pads)
        .feedback
        .insert(2, VecDeque::from([Err(device_removed_error())]));
    assert!(pads.feedback().is_empty());
    assert_eq!(
        script(&mut pads).calls,
        ["feedback 2", "plug 2"],
        "the poll re-creates the active pad instead of forgetting it"
    );
    pads.active.clear();
}

/// A back-button timeout due on two pads: one pad's failed submit must not
/// skip the other's update, and the last error is reported.
#[test]
fn refresh_updates_every_pad_and_returns_the_last_failure() {
    let mut pads = scripted_pads();
    pads.policy.back_button_timeout = Some(Duration::ZERO);
    for id in [0u16, 1] {
        pads.active.insert(id, id + 2);
        pads.profiles.insert(id, 4);
        pads.apply(&Event::Controller {
            id,
            active: 0b11,
            buttons: 0x20,
            left_trigger: 0,
            right_trigger: 0,
            sticks: [0; 4],
        })
        .unwrap();
    }
    // The timeout has passed: both pads' back buttons are due.
    std::thread::sleep(Duration::from_millis(150));
    script(&mut pads).calls.clear();
    script(&mut pads).submits = VecDeque::from([
        Err(anyhow::anyhow!("first pad failed")),
        Err(anyhow::anyhow!("second pad failed")),
    ]);
    let error = pads.refresh().unwrap_err();
    assert!(format!("{error:#}").contains("second pad failed"));
    assert_eq!(
        script(&mut pads).calls,
        ["submit 2", "submit 3"],
        "the second pad was updated after the first failed"
    );
    pads.active.clear();
}

/// The back-button update takes the same device-removed path as a controller
/// state: the slot is created again and the update retried.
#[test]
fn refresh_recreates_a_pad_whose_driver_device_was_removed() {
    let mut pads = scripted_pads();
    pads.policy.back_button_timeout = Some(Duration::ZERO);
    pads.active.insert(0, 2);
    pads.profiles.insert(0, 4);
    pads.apply(&Event::Controller {
        id: 0,
        active: 1,
        buttons: 0x20,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(150));
    script(&mut pads).calls.clear();
    script(&mut pads).submits = VecDeque::from([Err(device_removed_error()), Ok(())]);
    pads.refresh().unwrap();
    assert_eq!(
        script(&mut pads).calls,
        ["submit 2", "plug 2", "submit 2"],
        "the slot was created again and the update retried"
    );
    pads.active.clear();
}

/// A driver whose answers a test scripts, for the paths that only a real
/// sleep can otherwise reach.
fn scripted_pads() -> Gamepads {
    let mut pads = broken_pads();
    pads.profile = butterpollo_input_policy::input_policy::VHF_XBOX_SERIES;
    pads.available = 1 << (butterpollo_input_policy::input_policy::VHF_XBOX_SERIES - 1);
    pads.backend = gamepad_backend::Backend::Fake(Default::default());
    pads
}

fn script(pads: &mut Gamepads) -> &mut gamepad_backend::fake::Driver {
    match &mut pads.backend {
        gamepad_backend::Backend::Fake(driver) => driver,
        _ => unreachable!("scripted pads"),
    }
}

fn state(id: u16, active: u16) -> Event {
    Event::Controller {
        id,
        active,
        buttons: 0,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    }
}

fn device_removed_error() -> anyhow::Error {
    anyhow::Error::new(windows::core::Error::from_hresult(
        windows::Win32::Foundation::ERROR_DEVICE_REMOVED.to_hresult(),
    ))
}

/// Gamepads whose driver handle is missing: every plug and unplug fails.
fn broken_pads() -> Gamepads {
    let mut pads = Gamepads::open_options(
        butterpollo_input_policy::input_policy::USBIP_DUALSENSE,
        butterpollo_input_policy::input_policy::Policy::resolve(&Default::default()).unwrap(),
        Default::default(),
    )
    .unwrap();
    pads.profile = 0;
    pads
}
