//! Unit tests for Windows HDR calibration discovery and ICC profile leases.

use super::*;
use std::cell::RefCell;
/// Associations by display and scope (system or the user's), most recent default first.
#[derive(Default)]
struct Fake(RefCell<BTreeMap<(String, bool), Vec<String>>>);
impl Fake {
    fn with(self, monitor: &Monitor, system: bool, name: &str) -> Self {
        self.add(monitor, system, name).unwrap();
        self
    }
    fn list(&self, monitor: &Monitor, system: bool) -> Vec<String> {
        self.0
            .borrow()
            .get(&(monitor.device_id.clone(), system))
            .cloned()
            .unwrap_or_default()
    }
}
impl Associations for Fake {
    fn get(&self, monitor: &Monitor, system: bool) -> Result<Option<String>> {
        Ok(self
            .0
            .borrow()
            .get(&(monitor.device_id.clone(), system))
            .and_then(|list| list.first().cloned()))
    }
    fn add(&self, monitor: &Monitor, system: bool, name: &str) -> Result<()> {
        let mut associations = self.0.borrow_mut();
        let list = associations
            .entry((monitor.device_id.clone(), system))
            .or_default();
        list.retain(|existing| !existing.eq_ignore_ascii_case(name));
        list.insert(0, name.into());
        Ok(())
    }
    fn remove(&self, monitor: &Monitor, system: bool, name: &str) -> Result<()> {
        let mut associations = self.0.borrow_mut();
        let key = (monitor.device_id.clone(), system);
        let Some(list) = associations.get_mut(&key) else {
            bail!("{name} is not associated");
        };
        let before = list.len();
        list.retain(|existing| !existing.eq_ignore_ascii_case(name));
        anyhow::ensure!(list.len() < before, "{name} is not associated");
        if list.is_empty() {
            associations.remove(&key);
        }
        Ok(())
    }
}
fn monitor(id: &str, primary: bool) -> Monitor {
    Monitor {
        display_name: format!(r"\\.\{id}"),
        friendly_name: format!("{id} panel"),
        primary,
        ..crate::display::monitor(id, 0, true)
    }
}
/// Profiles named `*-cal.icm` carry an MHC2 calibration.
fn peak(name: &str) -> Option<u32> {
    name.ends_with("-cal.icm").then_some(800)
}
#[test]
fn the_physical_calibration_is_the_primary_displays_and_the_users_own_first() -> Result<()> {
    let (side, main) = (monitor("side", false), monitor("main", true));
    let displays = [side.clone(), main.clone()];
    // A profile without MHC2 is no calibration.
    let fake = Fake::default()
        .with(&side, false, "side-cal.icm")
        .with(&main, true, "srgb.icm");
    let found = find_calibration(&fake, &displays, peak)?.expect("side calibration");
    assert_eq!(
        (found.display.as_str(), found.profile.as_str(), found.peak),
        ("side panel", "side-cal.icm", 800)
    );
    let fake = fake
        .with(&main, true, "system-cal.icm")
        .with(&main, false, "user-cal.icm");
    assert_eq!(
        find_calibration(&fake, &displays, peak)?.map(|found| found.profile),
        Some("user-cal.icm".into())
    );
    let fake = Fake::default().with(&main, false, "srgb.icm");
    assert_eq!(find_calibration(&fake, &displays, peak)?, None);
    assert_eq!(find_calibration(&fake, &[], peak)?, None);
    Ok(())
}
#[test]
fn the_virtual_display_gets_back_what_it_had_unless_someone_chose_another() -> Result<()> {
    let display = monitor("virtual", false);
    // None before: the calibration is removed again.
    let fake = Fake::default().with(&display, true, "main-cal.icm");
    restore_with(&fake, &display, None, "main-cal.icm", true)?;
    assert_eq!(fake.get(&display, true)?, None);
    // A profile before comes back, and the stream's profile is disassociated (HW-L34).
    let fake = Fake::default()
        .with(&display, true, "old.icm")
        .with(&display, true, "main-cal.icm");
    restore_with(&fake, &display, Some("old.icm"), "main-cal.icm", true)?;
    assert_eq!(fake.get(&display, true)?, Some("old.icm".into()));
    assert_eq!(fake.list(&display, true), vec!["old.icm"]);
    // One chosen while streaming stays, in either scope.
    let fake =
        Fake::default()
            .with(&display, true, "mine.icm")
            .with(&display, false, "main-cal.icm");
    restore_with(&fake, &display, None, "main-cal.icm", true)?;
    assert_eq!(fake.get(&display, true)?, Some("mine.icm".into()));
    assert_eq!(fake.get(&display, false)?, Some("main-cal.icm".into()));
    // Nothing changed: nothing to put back.
    restore_with(&fake, &display, Some("MINE.icm"), "mine.icm", true)?;
    assert_eq!(fake.get(&display, true)?, Some("mine.icm".into()));
    Ok(())
}
/// An association store whose `add` of one name fails, as a Windows call that is refused.
struct Refusing(Fake, &'static str);
impl Associations for Refusing {
    fn get(&self, monitor: &Monitor, system: bool) -> Result<Option<String>> {
        self.0.get(monitor, system)
    }
    fn add(&self, monitor: &Monitor, system: bool, name: &str) -> Result<()> {
        anyhow::ensure!(name != self.1, "refused");
        self.0.add(monitor, system, name)
    }
    fn remove(&self, monitor: &Monitor, system: bool, name: &str) -> Result<()> {
        self.0.remove(monitor, system, name)
    }
}
fn state(applied: &str, users: usize) -> State {
    State {
        users,
        previous: Some("orig.icm".into()),
        applied: applied.into(),
        system: true,
    }
}
#[test]
fn switching_the_profile_journals_the_new_one_before_it_changes_windows() -> Result<()> {
    let display = monitor("virtual", false);
    let fake = Fake::default().with(&display, true, "old-cal.icm");
    let journal = RefCell::new(Vec::new());
    let mut held = state("old-cal.icm", 1);
    switch_state(
        &fake,
        &display,
        &mut held,
        "new-cal.icm".into(),
        |before, applied| {
            // The journal is written while Windows still has the old profile.
            assert_eq!(fake.get(&display, true)?, Some("old-cal.icm".into()));
            journal.borrow_mut().push((before, applied.to_owned()));
            Ok(())
        },
    )?;
    assert_eq!(fake.get(&display, true)?, Some("new-cal.icm".into()));
    assert_eq!(held.applied, "new-cal.icm");
    assert_eq!(
        journal.into_inner(),
        vec![(Some("orig.icm".into()), "new-cal.icm".to_owned())]
    );
    Ok(())
}
#[test]
fn a_refused_switch_keeps_the_old_profile_and_its_journal_entry() {
    let display = monitor("virtual", false);
    let fake = Refusing(
        Fake::default().with(&display, true, "old-cal.icm"),
        "new-cal.icm",
    );
    let journal = RefCell::new(Some("old-cal.icm".to_owned()));
    let mut held = state("old-cal.icm", 1);
    let result = switch_state(
        &fake,
        &display,
        &mut held,
        "new-cal.icm".into(),
        |_, applied| {
            *journal.borrow_mut() = Some(applied.to_owned());
            Ok(())
        },
    );
    assert!(result.is_err(), "the refusal is reported");
    assert_eq!(
        held.applied, "old-cal.icm",
        "the lease still holds the old profile"
    );
    assert_eq!(
        fake.get(&display, true).unwrap(),
        Some("old-cal.icm".into())
    );
    assert_eq!(journal.into_inner().as_deref(), Some("old-cal.icm"));
    // A failing journal write changes nothing in Windows.
    let fake = Fake::default().with(&display, true, "old-cal.icm");
    let mut held = state("old-cal.icm", 1);
    let result = switch_state(&fake, &display, &mut held, "new-cal.icm".into(), |_, _| {
        anyhow::bail!("journal unwritable")
    });
    assert!(result.is_err());
    assert_eq!(
        fake.get(&display, true).unwrap(),
        Some("old-cal.icm".into())
    );
}
#[test]
fn a_profile_another_client_shares_is_not_switched() {
    let display = monitor("virtual", false);
    let fake = Fake::default().with(&display, true, "old-cal.icm");
    let mut held = state("old-cal.icm", 2);
    let result = switch_state(&fake, &display, &mut held, "new-cal.icm".into(), |_, _| {
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(
        fake.get(&display, true).unwrap(),
        Some("old-cal.icm".into())
    );
    // The same profile is shared as before, and a lost association is put back.
    let fake = Fake::default();
    switch_state(&fake, &display, &mut held, "old-cal.icm".into(), |_, _| {
        Ok(())
    })
    .unwrap();
    assert_eq!(
        fake.get(&display, true).unwrap(),
        Some("old-cal.icm".into())
    );
}
/// Associations that can be shared across threads, one `add` of which waits: `entered` is
/// told when it is called, and it returns when `release` is sent.
struct Gated {
    associations: Mutex<BTreeMap<(String, bool), String>>,
    blocked: &'static str,
    entered: Mutex<std::sync::mpsc::Sender<()>>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
}
impl Associations for Gated {
    fn get(&self, monitor: &Monitor, system: bool) -> Result<Option<String>> {
        Ok(self
            .associations
            .lock()
            .unwrap()
            .get(&(monitor.device_id.clone(), system))
            .cloned())
    }
    fn add(&self, monitor: &Monitor, system: bool, name: &str) -> Result<()> {
        if name == self.blocked {
            self.entered.lock().unwrap().send(()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
        }
        self.associations
            .lock()
            .unwrap()
            .insert((monitor.device_id.clone(), system), name.into());
        Ok(())
    }
    fn remove(&self, _: &Monitor, _: bool, _: &str) -> Result<()> {
        Ok(())
    }
}
type Gate = (
    std::sync::Arc<Gated>,
    std::sync::mpsc::Receiver<()>,
    std::sync::mpsc::Sender<()>,
);
fn gated(blocked: &'static str) -> Gate {
    let (entered, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release) = std::sync::mpsc::channel();
    let api = std::sync::Arc::new(Gated {
        associations: Mutex::default(),
        blocked,
        entered: Mutex::new(entered),
        release: Mutex::new(release),
    });
    (api, entered_rx, release_tx)
}
fn lease_on(api: &Gated, display: &Monitor, profile: &str) -> Result<()> {
    with_slot(&display.device_id, |held| {
        acquire_state(
            held,
            display,
            profile.into(),
            true,
            || Ok(api),
            |_, _| Ok(()),
            || None,
            || Ok(()),
        )
    })
}
#[test]
fn a_blocked_colour_call_does_not_hold_up_another_displays_lease() {
    let (first, second) = (monitor("mhc-first", false), monitor("mhc-second", false));
    let (api, entered, release) = gated("a-cal.icm");
    let blocked = {
        let (api, first) = (api.clone(), first.clone());
        std::thread::spawn(move || lease_on(&api, &first, "a-cal.icm"))
    };
    entered
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the first lease reached its colour call");
    // The first display's call is in flight: the second display leases meanwhile.
    let other = {
        let (api, second) = (api.clone(), second.clone());
        std::thread::spawn(move || lease_on(&api, &second, "b-cal.icm"))
    };
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || done.send(other.join().unwrap()));
    let result = finished.recv_timeout(std::time::Duration::from_secs(5));
    release.send(()).unwrap();
    result
        .expect("the second display's lease was held up behind the first's colour call")
        .unwrap();
    blocked.join().unwrap().unwrap();
}
#[test]
fn two_clients_cannot_lease_the_same_display_with_different_profiles() {
    let display = monitor("mhc-shared", false);
    let (api, entered, release) = gated("a-cal.icm");
    let first = {
        let (api, display) = (api.clone(), display.clone());
        std::thread::spawn(move || lease_on(&api, &display, "a-cal.icm"))
    };
    entered
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the first lease reached its colour call");
    // A second client arrives while the first applies: it waits for the display, then
    // finds it leased with another profile, or shares the same one.
    let different = {
        let (api, display) = (api.clone(), display.clone());
        std::thread::spawn(move || lease_on(&api, &display, "b-cal.icm"))
    };
    let same = {
        let (api, display) = (api.clone(), display.clone());
        std::thread::spawn(move || lease_on(&api, &display, "A-CAL.icm"))
    };
    std::thread::sleep(std::time::Duration::from_millis(100));
    release.send(()).unwrap();
    first.join().unwrap().unwrap();
    assert!(different.join().unwrap().is_err());
    same.join().unwrap().unwrap();
    let slot = slot_for("mhc-shared");
    let held = slot.lock().unwrap();
    let state = held.as_ref().unwrap();
    assert_eq!((state.users, state.applied.as_str()), (2, "a-cal.icm"));
    assert_eq!(
        api.get(&display, true).unwrap().as_deref(),
        Some("a-cal.icm")
    );
}
#[test]
fn mhc2_peak_reads_calibration_and_rejects_truncated_or_negative_tags() {
    let mut bytes = vec![0; 164];
    bytes[128..132].copy_from_slice(&1u32.to_be_bytes());
    bytes[132..136].copy_from_slice(b"MHC2");
    bytes[136..140].copy_from_slice(&144u32.to_be_bytes());
    bytes[140..144].copy_from_slice(&20u32.to_be_bytes());
    bytes[144..148].copy_from_slice(b"MHC2");
    bytes[160..164].copy_from_slice(&(1234i32 * 65536 + 32768).to_be_bytes());
    assert_eq!(mhc2_peak(&bytes), Some(1235));
    assert_eq!(mhc2_peak(&bytes[..163]), None);
    bytes[160..164].copy_from_slice(&(-65536i32).to_be_bytes());
    assert_eq!(mhc2_peak(&bytes), None);
    bytes[136..140].copy_from_slice(&u32::MAX.to_be_bytes());
    assert_eq!(mhc2_peak(&bytes), None);
}
#[test]
fn a_pending_original_profile_wins_over_the_one_left_on_the_display() {
    let left = Some("hdr.icm".to_string());
    assert_eq!(previous_profile(Some(None), left.clone()), None);
    assert_eq!(
        previous_profile(Some(Some("calibrated.icm".into())), left.clone()),
        Some("calibrated.icm".into())
    );
    assert_eq!(previous_profile(None, left.clone()), left);
}
#[test]
fn installed_profile_selection_cannot_escape_the_color_directory() {
    for bad in [
        "",
        ".",
        "..",
        "..\\other.icc",
        "C:\\other.icc",
        "other:stream",
        "a/b.icc",
        "a\0b.icc",
    ] {
        assert!(filename(bad).is_err());
    }
    assert_eq!(
        filename("My HDR Profile.icc").unwrap(),
        "My HDR Profile.icc"
    );
}
#[test]
fn a_refused_acquire_keeps_a_pending_crash_recovery_journal_entry() {
    let display = monitor("virtual", false);
    let fake = Refusing(
        Fake::default().with(&display, true, "crashed-cal.icm"),
        "new-cal.icm",
    );
    let journal = RefCell::new(Some((
        Some("orig.icm".to_owned()),
        "crashed-cal.icm".to_owned(),
    )));
    let mut held = None;
    let result = acquire_state(
        &mut held,
        &display,
        "new-cal.icm".into(),
        true,
        || Ok(&fake),
        |before, applied| {
            let mut entry = journal.borrow_mut();
            let original = entry.take().map_or(before, |(orig, _)| orig);
            *entry = Some((original, applied.to_owned()));
            Ok(())
        },
        || journal.borrow().as_ref().map(|(orig, _)| orig.clone()),
        || {
            *journal.borrow_mut() = None;
            Ok(())
        },
    );
    assert!(result.is_err());
    assert!(held.is_none());
    assert_eq!(
        journal.into_inner(),
        Some((Some("orig.icm".to_owned()), "crashed-cal.icm".to_owned()))
    );
    // Without a pending journal entry, a refused acquire clears the entry it wrote.
    let fake = Refusing(
        Fake::default().with(&display, true, "orig.icm"),
        "new-cal.icm",
    );
    let journal = RefCell::new(None);
    let mut held = None;
    let result = acquire_state(
        &mut held,
        &display,
        "new-cal.icm".into(),
        true,
        || Ok(&fake),
        |before, applied| {
            let mut entry = journal.borrow_mut();
            let original = entry.take().map_or(before, |(orig, _)| orig);
            *entry = Some((original, applied.to_owned()));
            Ok(())
        },
        || journal.borrow().as_ref().map(|(orig, _)| orig.clone()),
        || {
            *journal.borrow_mut() = None;
            Ok(())
        },
    );
    assert!(result.is_err());
    assert_eq!(journal.into_inner(), None);
}
