//! Tests for tray icon retry logging, console URLs, and launch error notices.
use super::*;

#[test]
fn a_refused_tray_icon_is_logged_once_and_again_when_it_keeps_failing() {
    let start = Instant::now();
    let mut log = AddLog::default();
    assert!(!log.is_added());
    assert_eq!(log.attempt(false, start), Some(AddReport::Refused));
    // The 3 s retries say nothing more until a minute has passed, then once.
    assert_eq!(log.attempt(false, start + Duration::from_secs(3)), None);
    assert_eq!(
        log.attempt(false, start + ADD_REFUSED_LONG),
        Some(AddReport::StillRefused)
    );
    log.note_still_refused(true);
    assert_eq!(log.attempt(false, start + Duration::from_secs(120)), None);
    assert_eq!(
        log.attempt(true, start + Duration::from_secs(123)),
        Some(AddReport::Added(Some(Duration::from_secs(123))))
    );
    assert!(log.is_added());
    // Explorer restarting adds it again: no line; a later refusal is new.
    assert_eq!(log.attempt(true, start + Duration::from_secs(200)), None);
    assert_eq!(
        log.attempt(false, start + Duration::from_secs(300)),
        Some(AddReport::Refused)
    );
    assert!(!log.is_added());
}

#[test]
fn taskbar_creation_restarts_retries_only_when_it_gave_up_without_a_taskbar() {
    let start = Instant::now();
    let mut log = AddLog::default();
    assert_eq!(log.attempt(false, start), Some(AddReport::Refused));
    assert_eq!(
        log.attempt(false, start + ADD_REFUSED_LONG),
        Some(AddReport::StillRefused)
    );
    log.note_still_refused(false);
    log.on_taskbar_created();
    assert_eq!(
        log.attempt(false, start + Duration::from_secs(120)),
        Some(AddReport::Refused)
    );
    assert_eq!(
        log.attempt(false, start + Duration::from_secs(180)),
        Some(AddReport::StillRefused)
    );
    // Once it gave up with the taskbar present, another TaskbarCreated does not warn again.
    log.note_still_refused(true);
    log.on_taskbar_created();
    assert_eq!(log.attempt(false, start + Duration::from_secs(240)), None);
}

#[test]
fn the_first_tray_icon_added_is_logged() {
    let mut log = AddLog::default();
    assert_eq!(
        log.attempt(true, Instant::now()),
        Some(AddReport::Added(None))
    );
    assert_eq!(log.attempt(true, Instant::now()), None);
}

#[test]
fn the_pairing_pages_lead_to_the_devices_page() {
    assert_eq!(console_url(47990, "/"), "https://localhost:47990/");
    assert_eq!(
        console_url(47990, PAGE_ENTER_PIN),
        "https://localhost:47990/devices?pair=pin"
    );
    assert_eq!(
        console_url(47990, PAGE_PAIR_QR),
        "https://localhost:47990/devices?pair=qr"
    );
    assert_eq!(
        console_url(47990, PAGE_BACKUP),
        "https://localhost:47990/troubleshoot#backup"
    );
    assert!(open_web_page(47990, "//evil.example/").is_err());
}

#[test]
fn launch_error_uses_vibepollos_wording_and_signed_exit_code() {
    assert_eq!(
        launch_error_notice("Game", 1),
        (
            "Launch Error",
            "Application Game exited too fast with code 1. Click here to terminate the stream."
                .into()
        )
    );
    assert!(
        launch_error_notice("Game", 0xc0000005)
            .1
            .contains("code -1073741819.")
    );
}
