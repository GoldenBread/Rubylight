//! Unit tests for `pin_ui` lock screen, timeout and store state transitions.

use super::*;
use crate::pin_lock::Clock;
use std::sync::atomic::{AtomicI64, Ordering};

struct Still(AtomicI64);

impl Clock for Still {
    fn elapsed(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
    fn boot(&self) -> i64 {
        1
    }
}

struct TestCrypto;

impl crate::pin_lock::PinCrypto for TestCrypto {
    fn create(&self, pin: &[u8], iterations: u32) -> Result<String, String> {
        let text = std::str::from_utf8(pin).map_err(|_| "bad utf-8".to_owned())?;
        Ok(format!("v1${iterations}$test${text}"))
    }
    fn verify(&self, stored: &str, pin: &[u8]) -> bool {
        let Ok(text) = std::str::from_utf8(pin) else { return false };
        stored.split('$').nth(3) == Some(text)
    }
    fn well_formed(&self, stored: &str) -> bool {
        let parts: Vec<&str> = stored.split('$').collect();
        parts.len() == 4 && parts[0] == "v1" && !parts[2].is_empty() && !parts[3].is_empty()
    }
    fn calibrate(&self) -> u32 {
        1_000
    }
    fn random_token(&self) -> Result<String, String> {
        Ok("01".repeat(32))
    }
    fn token_hash(&self, token: &str) -> String {
        let mut h = 0xcbf2_9ce4_8422_2325_u64;
        for b in token.bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
        format!("fnv1a:{h:016x}")
    }
}

fn lock(tag: &str) -> (PinLock, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("nightjar-pin-ui-{tag}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    (PinLock::with_store(PinStore::new(&dir, Arc::new(Still(AtomicI64::new(5_000_000))), Arc::new(TestCrypto))), dir)
}

fn type_pin(lock: &mut PinLock, pin: &str) {
    for d in pin.bytes() {
        lock.key(PadKey::Digit(d - b'0'));
    }
}

/// Waits for the worker's answer and takes it, as the next frame would.
fn finish(lock: &mut PinLock) -> Vec<Command> {
    let mut commands = Vec::new();
    for _ in 0..500 {
        if lock.job.is_none() {
            break;
        }
        lock.poll(&mut commands);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(lock.job.is_none(), "the worker answered");
    commands
}

#[test]
fn setting_a_pin_twice_turns_the_app_lock_on_without_locking_now() {
    let (mut lock, dir) = lock("set");
    lock.open_settings();
    assert!(lock.page && lock.pad.is_none(), "no PIN yet: the page opens straight away");
    lock.pad = Some(Pad::new(Purpose::Choose, &lock.store));
    type_pin(&mut lock, "2468");
    lock.key(PadKey::Confirm);
    assert!(matches!(lock.pad.as_ref().unwrap().purpose, Purpose::Confirm(_)));
    type_pin(&mut lock, "2469");
    assert_eq!(lock.pad.as_ref().unwrap().purpose, Purpose::Choose, "a mismatch starts over");
    assert!(lock.pad.as_ref().unwrap().error.is_some());
    type_pin(&mut lock, "2468");
    lock.key(PadKey::Confirm);
    type_pin(&mut lock, "2468");
    finish(&mut lock);
    assert!(lock.store.has_pin() && lock.store.mode() == Mode::App);
    assert!(!lock.app_locked() && lock.page, "the app stays open after setting it");
    // Leaving the window past the grace time locks it, and the right PIN opens it.
    lock.store.set_grace_minutes(0);
    lock.left();
    assert!(!lock.page, "the page asks again next time");
    lock.returned();
    assert!(lock.app_locked());
    type_pin(&mut lock, "1111");
    finish(&mut lock);
    assert!(lock.app_locked() && lock.pad.as_ref().unwrap().error.is_some(), "wrong PIN");
    type_pin(&mut lock, "2468");
    let commands = finish(&mut lock);
    assert!(!lock.app_locked() && matches!(commands[..], [Command::PinPassed]));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_stream_prompt_grants_one_start_or_cancels_it() {
    let (mut lock, dir) = lock("stream");
    lock.store.set_pin(b"1357").unwrap();
    lock.store.set_mode(Mode::Selected);
    lock.store.set_pcs(vec!["PC-1".into()]);
    assert!(!lock.needs_pin_for_stream("pc-2"));
    assert!(lock.needs_pin_for_stream("pc-1"));
    lock.ask_for_stream("pc-1", "Desk");
    let mut commands = Vec::new();
    lock.cancel(&mut commands);
    assert!(matches!(commands[..], [Command::PinCancelled]) && lock.pad.is_none());
    lock.ask_for_stream("pc-1", "Desk");
    type_pin(&mut lock, "1357");
    assert!(matches!(finish(&mut lock)[..], [Command::PinPassed]));
    assert!(!lock.needs_pin_for_stream("pc-1"), "granted once");
    assert!(lock.needs_pin_for_stream("pc-1"), "and used up");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_damaged_lock_file_is_named_on_the_lock_screen_and_takes_no_tries() {
    let dir = std::env::temp_dir().join(format!("nightjar-pin-ui-damaged-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("pin_lock.json"), "{ not json").unwrap();
    let mut lock = PinLock::with_store(PinStore::new(&dir, Arc::new(Still(AtomicI64::new(5_000_000))), Arc::new(TestCrypto)));
    assert!(lock.app_locked());
    let problem = lock.store.problem().expect("damaged");
    let said = problem_text(&problem, lock.store.path());
    assert!(said.contains(&dir.join("pin_lock.json").display().to_string()), "the full path: {said}");
    assert!(said.contains("Delete this file to remove the PIN lock"), "{said}");
    type_pin(&mut lock, "1234");
    lock.key(PadKey::Confirm);
    assert!(lock.job.is_none() && lock.pad.as_ref().unwrap().digits.is_empty(), "no try is made or counted");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn removing_the_pin_asks_for_it_first() {
    let (mut lock, dir) = lock("remove");
    lock.store.set_pin(b"8642").unwrap();
    lock.store.set_mode(Mode::Streams);
    lock.open_settings();
    assert!(!lock.page && matches!(lock.pad.as_ref().unwrap().purpose, Purpose::Verify(After::Open)));
    type_pin(&mut lock, "8642");
    finish(&mut lock);
    assert!(lock.page);
    lock.pad = Some(Pad::new(Purpose::Verify(After::Remove), &lock.store));
    type_pin(&mut lock, "8642");
    finish(&mut lock);
    assert!(!lock.store.has_pin() && lock.store.mode() == Mode::Off);
    std::fs::remove_dir_all(dir).ok();
}
