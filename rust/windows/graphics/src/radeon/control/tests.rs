//! Radeon control validation, journaling and restore unit tests with fake backend.

use super::*;
use crate::radeon::model::{ChoiceOption, Range};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

fn setting(id: &'static str, kind: Kind, value: Value) -> Setting {
    Setting {
        id,
        group: id.split('.').next().unwrap(),
        label: id,
        interface: "fake",
        kind,
        state: State::Configurable,
        supported: Some(true),
        readable: Some(true),
        writable: crate::radeon::model::Writable::Yes,
        value: Some(value),
        unit: None,
        range: None,
        options: vec![],
        depends: vec![],
        notes: vec![],
        adlx: BTreeMap::new(),
    }
}
fn toggle(id: &'static str, on: bool) -> Setting {
    setting(id, Kind::Toggle, Value::Bool(on))
}
fn integer(id: &'static str, value: i64) -> Setting {
    let mut s = setting(id, Kind::Integer, Value::Int(value));
    s.range = Some(Range {
        min: 30,
        max: 300,
        step: 10,
    });
    s
}
fn choice(id: &'static str, value: &str) -> Setting {
    let mut s = setting(id, Kind::Choice, Value::Choice(value.into()));
    s.options = ["a", "b", "c"]
        .iter()
        .enumerate()
        .map(|(raw, name)| ChoiceOption {
            value: (*name).into(),
            raw: raw as i32,
        })
        .collect();
    s
}
fn on(v: bool) -> Value {
    Value::Bool(v)
}
fn change(id: &str, value: Value) -> Change {
    Change {
        id: id.into(),
        value,
    }
}

#[derive(Default)]
struct Driver {
    settings: Vec<Setting>,
    log: Vec<String>,
    /// Refuse a write of this id.
    refuse: Option<&'static str>,
    /// Ignore writes of this id (the read-back keeps the old value).
    ignore: Option<&'static str>,
    /// Writing `.0` to true also sets `.1` to false.
    excludes: Vec<(&'static str, &'static str)>,
    /// The journal file, to see what was on disk when a write arrived.
    journal: Option<PathBuf>,
    journal_at_first_write: Option<Journal>,
    unreadable: bool,
    /// The next reads show this id with this older value, as a long-lived process sees a
    /// setting another process wrote: (id, stale value, reads left).
    stale: Option<(&'static str, Value, usize)>,
    /// Writing `.0` fails while the toggle `.1` is off.
    needs_on: Vec<(&'static str, &'static str)>,
    /// Writes of this id after the first n are silently dropped.
    ignore_from: Option<(&'static str, usize)>,
    /// Writing this id turns the journal path into a directory, so the next save fails.
    break_journal_at: Option<&'static str>,
    writes_of: HashMap<String, usize>,
}
#[derive(Clone, Default)]
struct Fake(Rc<RefCell<Driver>>);
impl Fake {
    fn with(settings: Vec<Setting>) -> Self {
        let fake = Self::default();
        fake.0.borrow_mut().settings = settings;
        fake
    }
    fn value(&self, id: &str) -> Value {
        find(&self.0.borrow().settings, id)
            .unwrap()
            .value
            .clone()
            .unwrap()
    }
    fn set(&self, id: &str, value: Value) {
        let mut d = self.0.borrow_mut();
        d.settings.iter_mut().find(|s| s.id == id).unwrap().value = Some(value);
    }
    fn writes(&self) -> Vec<String> {
        self.0.borrow().log.clone()
    }
}
impl RadeonBackend for Fake {
    fn read(&mut self, _gpu: i32) -> Result<Vec<Setting>, BackendError> {
        let mut d = self.0.borrow_mut();
        if d.unreadable {
            return Err(BackendError("ADLX_FAIL".into()));
        }
        let mut settings = d.settings.clone();
        if let Some((id, value, left)) = d.stale.take() {
            if let Some(s) = settings.iter_mut().find(|s| s.id == id) {
                s.value = Some(value.clone());
            }
            if left > 1 {
                d.stale = Some((id, value, left - 1));
            }
        }
        Ok(settings)
    }
    fn write(&mut self, _gpu: i32, id: &str, value: &Value) -> Result<(), BackendError> {
        let mut d = self.0.borrow_mut();
        if d.journal_at_first_write.is_none()
            && let Some(path) = d.journal.clone()
        {
            d.journal_at_first_write = Journal::load(&path).ok();
        }
        d.log.push(format!("{id}={}", value.display()));
        let count = d.writes_of.entry(id.to_string()).or_default();
        *count += 1;
        let count = *count;
        if d.break_journal_at == Some(id)
            && let Some(path) = d.journal.clone()
        {
            let _ = std::fs::remove_file(&path);
            std::fs::create_dir_all(&path).unwrap();
        }
        if d.refuse == Some(id) {
            return Err(BackendError("ADLX_NOT_SUPPORTED".into()));
        }
        if d.ignore == Some(id) {
            return Ok(());
        }
        if d.ignore_from
            .is_some_and(|(ignored, n)| ignored == id && count > n)
        {
            return Ok(());
        }
        let toggle_off = d.needs_on.iter().any(|(field, toggle)| {
            *field == id
                && find(&d.settings, toggle).and_then(|s| s.value.as_ref())
                    == Some(&Value::Bool(false))
        });
        if toggle_off {
            return Err(BackendError("ADLX_FAIL".into()));
        }
        let effects: Vec<&str> = d
            .excludes
            .iter()
            .filter(|(a, _)| *a == id && *value == Value::Bool(true))
            .map(|(_, b)| *b)
            .collect();
        for s in &mut d.settings {
            if s.id == id {
                s.value = Some(value.clone());
            } else if effects.contains(&s.id) {
                s.value = Some(Value::Bool(false));
            }
        }
        Ok(())
    }
}

/// The SID the controller files its entries under: the account the tests run as.
fn me() -> String {
    crate::process_info::own_sid().expect("the test account has a SID")
}
fn controller(fake: &Fake) -> (Controller<Fake>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(JOURNAL_FILE);
    fake.0.borrow_mut().journal = Some(path.clone());
    (Controller::new(fake.clone(), path), dir)
}
fn panel() -> Fake {
    Fake::with(vec![
        toggle("chill.enabled", false),
        integer("pacing.limit", 140),
        integer("chill.max_fps", 140),
        toggle("anti_lag.enabled", true),
        choice("tessellation.mode", "a"),
        {
            let mut s = toggle("rsr.enabled", false);
            s.state = State::ReadOnly;
            s
        },
    ])
}

#[test]
fn invalid_requests_change_nothing_and_leave_no_journal() {
    let fake = panel();
    let (mut c, dir) = controller(&fake);
    let cases = [
        (change("nope", on(true)), ErrorKind::InvalidValue),
        (change("rsr.enabled", on(true)), ErrorKind::NotWritable),
        (
            change("chill.enabled", Value::Int(1)),
            ErrorKind::InvalidValue,
        ),
        (
            change("pacing.limit", Value::Int(20)),
            ErrorKind::InvalidValue,
        ),
        (
            change("pacing.limit", Value::Int(305)),
            ErrorKind::InvalidValue,
        ),
        (
            change("pacing.limit", Value::Int(145)),
            ErrorKind::InvalidValue,
        ),
        (
            change("tessellation.mode", Value::Choice("z".into())),
            ErrorKind::InvalidValue,
        ),
    ];
    for (bad, kind) in cases {
        // A valid change first: the whole request must fail.
        let request = [change("chill.enabled", on(true)), bad.clone()];
        let e = c.apply("s1", 1, &request).unwrap_err();
        assert_eq!(e.kind, kind, "{bad:?}");
        assert_eq!(e.id.as_deref(), Some(bad.id.as_str()));
    }
    let twice = [
        change("chill.enabled", on(true)),
        change("chill.enabled", on(false)),
    ];
    assert_eq!(
        c.apply("s1", 1, &twice).unwrap_err().kind,
        ErrorKind::InvalidValue
    );
    assert_eq!(
        c.apply("s1", 1, &[]).unwrap_err().kind,
        ErrorKind::InvalidValue
    );
    assert!(fake.writes().is_empty());
    assert!(!dir.path().join(JOURNAL_FILE).exists());
}

#[test]
fn the_original_is_on_disk_before_the_first_write_and_the_readback_returns() {
    let fake = panel();
    let (mut c, dir) = controller(&fake);
    let after = c
        .apply(
            "s1",
            1,
            &[
                change("pacing.limit", Value::Int(150)),
                change("tessellation.mode", Value::Choice("c".into())),
            ],
        )
        .unwrap();
    assert_eq!(
        find(&after, "pacing.limit").unwrap().value,
        Some(Value::Int(150))
    );
    let seen = fake.0.borrow().journal_at_first_write.clone().unwrap();
    assert_eq!(
        seen.entries.len(),
        2,
        "both originals journaled before any write"
    );
    assert_eq!(seen.entries[0].original, Value::Int(140));
    assert_eq!(seen.entries[0].written, Value::Int(150));
    let on_disk = Journal::load(&dir.path().join(JOURNAL_FILE)).unwrap();
    assert_eq!(on_disk.entries[1].original, Value::Choice("a".into()));
    assert_eq!(fake.writes(), ["pacing.limit=150", "tessellation.mode=c"]);
}

#[test]
fn the_first_original_is_kept_across_writes_in_a_session() {
    let fake = panel();
    let (mut c, _dir) = controller(&fake);
    c.apply("s1", 1, &[change("pacing.limit", Value::Int(150))])
        .unwrap();
    c.apply("s1", 1, &[change("pacing.limit", Value::Int(160))])
        .unwrap();
    let journal = c.journal().unwrap();
    assert_eq!(journal.entries.len(), 1);
    assert_eq!(journal.entries[0].original, Value::Int(140));
    assert_eq!(journal.entries[0].written, Value::Int(160));
    let report = c.restore(Scope::Session("s1")).unwrap();
    assert_eq!(report.outcomes[0].result, RestoreResult::Restored);
    assert_eq!(fake.value("pacing.limit"), Value::Int(140));
    assert!(c.journal().unwrap().entries.is_empty());
}

#[test]
fn a_refused_change_undoes_the_ones_before_it() {
    let fake = panel();
    let (mut c, _dir) = controller(&fake);
    fake.0.borrow_mut().refuse = Some("tessellation.mode");
    let e = c
        .apply(
            "s1",
            1,
            &[
                change("chill.enabled", on(true)),
                change("pacing.limit", Value::Int(150)),
                change("tessellation.mode", Value::Choice("b".into())),
            ],
        )
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::DriverRejected);
    assert_eq!(e.id.as_deref(), Some("tessellation.mode"));
    assert_eq!(fake.value("chill.enabled"), on(false));
    assert_eq!(fake.value("pacing.limit"), Value::Int(140));
    // The driver switched Anti-Lag off with Chill; undoing the request turned it back on.
    assert!(c.journal().unwrap().entries.is_empty());
}

#[test]
fn undo_also_reverts_what_the_driver_switched_on_its_own() {
    let fake = panel();
    let (mut c, _dir) = controller(&fake);
    fake.0.borrow_mut().excludes = vec![("chill.enabled", "anti_lag.enabled")];
    fake.0.borrow_mut().refuse = Some("tessellation.mode");
    let e = c
        .apply(
            "s1",
            1,
            &[
                change("chill.enabled", on(true)),
                change("tessellation.mode", Value::Choice("b".into())),
            ],
        )
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::DriverRejected);
    assert_eq!(fake.value("chill.enabled"), on(false));
    assert_eq!(fake.value("anti_lag.enabled"), on(true));
    assert!(c.journal().unwrap().entries.is_empty());
}

#[test]
fn a_readback_that_differs_is_rejected_and_undone() {
    let fake = panel();
    let (mut c, _dir) = controller(&fake);
    fake.0.borrow_mut().ignore = Some("pacing.limit");
    let e = c
        .apply(
            "s1",
            1,
            &[
                change("chill.enabled", on(true)),
                change("pacing.limit", Value::Int(150)),
            ],
        )
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::DriverRejected);
    assert_eq!(e.id.as_deref(), Some("pacing.limit"));
    assert!(e.message.contains("reads back 140"), "{e}");
    assert_eq!(fake.value("chill.enabled"), on(false));
}

#[test]
fn driver_side_effects_are_journaled_and_restored() {
    let fake = panel();
    let (mut c, _dir) = controller(&fake);
    fake.0.borrow_mut().excludes = vec![("chill.enabled", "anti_lag.enabled")];
    let after = c
        .apply("s1", 1, &[change("chill.enabled", on(true))])
        .unwrap();
    assert_eq!(
        find(&after, "anti_lag.enabled").unwrap().value,
        Some(on(false))
    );
    let journal = c.journal().unwrap();
    let effect = journal
        .entries
        .iter()
        .find(|e| e.id == "anti_lag.enabled")
        .unwrap();
    assert!(effect.side_effect);
    assert_eq!((&effect.original, &effect.written), (&on(true), &on(false)));
    let report = c.restore(Scope::Session("s1")).unwrap();
    assert!(report.is_clean());
    assert_eq!(fake.value("chill.enabled"), on(false));
    // Chill went off first, then Anti-Lag came back.
    assert_eq!(fake.value("anti_lag.enabled"), on(true));
    assert_eq!(fake.writes().last().unwrap(), "anti_lag.enabled=on");
}

#[test]
fn restore_leaves_a_setting_that_was_changed_on_the_pc() {
    let fake = panel();
    let (mut c, _dir) = controller(&fake);
    c.apply(
        "s1",
        1,
        &[
            change("pacing.limit", Value::Int(150)),
            change("chill.enabled", on(true)),
        ],
    )
    .unwrap();
    fake.set("pacing.limit", Value::Int(200));
    let report = c.restore(Scope::Session("s1")).unwrap();
    let by_id = |id: &str| &report.outcomes.iter().find(|o| o.id == id).unwrap().result;
    assert_eq!(by_id("chill.enabled"), &RestoreResult::Restored);
    assert_eq!(
        by_id("pacing.limit"),
        &RestoreResult::ChangedElsewhere {
            current: Value::Int(200),
            written: Value::Int(150)
        }
    );
    assert_eq!(fake.value("pacing.limit"), Value::Int(200));
    assert_eq!(fake.value("chill.enabled"), on(false));
    assert!(
        c.journal().unwrap().entries.is_empty(),
        "the local change wins and the entry goes"
    );
}

#[test]
fn restore_drops_an_entry_whose_write_never_happened() {
    let fake = panel();
    let (mut c, _dir) = controller(&fake);
    // A crash between the journal and the write leaves the original in place.
    let mut journal = Journal::default();
    journal.record(Entry {
        user: me(),
        gpu: 1,
        id: "pacing.limit".into(),
        session: "s1".into(),
        original: Value::Int(140),
        written: Value::Int(150),
        side_effect: false,
    });
    journal.save(&c.journal).unwrap();
    let report = c.recover_on_startup().unwrap();
    assert_eq!(report.outcomes[0].result, RestoreResult::AlreadyOriginal);
    // Startup writes the original once more, which changes nothing here.
    assert_eq!(fake.writes(), ["pacing.limit=140"]);
    assert_eq!(fake.value("pacing.limit"), Value::Int(140));
    assert!(c.journal().unwrap().entries.is_empty());
}

#[test]
fn a_failed_restore_stays_journaled_and_is_retried() {
    let fake = panel();
    let (mut c, _dir) = controller(&fake);
    c.apply("s1", 1, &[change("pacing.limit", Value::Int(150))])
        .unwrap();
    fake.0.borrow_mut().refuse = Some("pacing.limit");
    let report = c.restore(Scope::Session("s1")).unwrap();
    assert!(!report.is_clean());
    assert_eq!(c.journal().unwrap().entries.len(), 1);
    assert_eq!(fake.value("pacing.limit"), Value::Int(150));
    fake.0.borrow_mut().refuse = None;
    let report = c.recover_on_startup().unwrap();
    assert!(report.is_clean());
    assert_eq!(fake.value("pacing.limit"), Value::Int(140));
    assert!(c.journal().unwrap().entries.is_empty());
}

#[test]
fn restore_by_session_leaves_the_other_sessions() {
    let fake = panel();
    let (mut c, _dir) = controller(&fake);
    c.apply("s1", 1, &[change("pacing.limit", Value::Int(150))])
        .unwrap();
    c.apply(
        "s2",
        1,
        &[change("tessellation.mode", Value::Choice("b".into()))],
    )
    .unwrap();
    c.restore(Scope::Session("s2")).unwrap();
    assert_eq!(fake.value("tessellation.mode"), Value::Choice("a".into()));
    assert_eq!(fake.value("pacing.limit"), Value::Int(150));
    assert_eq!(c.journal().unwrap().entries.len(), 1);
}

#[test]
fn restoring_named_settings_leaves_the_rest_of_the_session_journaled() {
    let fake = panel();
    fake.0.borrow_mut().excludes = vec![("chill.enabled", "anti_lag.enabled")];
    let (mut c, _dir) = controller(&fake);
    // The device's own write, then a profile's (which switches Anti-Lag off as a result).
    c.apply("s1", 1, &[change("pacing.limit", Value::Int(150))])
        .unwrap();
    c.apply("s1", 1, &[change("chill.enabled", on(true))])
        .unwrap();
    assert_eq!(fake.value("anti_lag.enabled"), on(false));
    let ids = ["chill.enabled".to_string()];
    let report = c.restore(Scope::Settings("s1", &ids)).unwrap();
    assert!(report.is_clean());
    assert_eq!(fake.value("chill.enabled"), on(false));
    assert_eq!(fake.value("anti_lag.enabled"), on(true));
    // The device's own setting stays changed and journaled for its session's end.
    assert_eq!(fake.value("pacing.limit"), Value::Int(150));
    let left = c.journal().unwrap().entries;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].id, "pacing.limit");
    // And another session's entry of the same setting is never named by this scope.
    c.apply("s2", 1, &[change("chill.enabled", on(true))])
        .unwrap();
    c.restore(Scope::Settings("s1", &ids)).unwrap();
    assert_eq!(fake.value("chill.enabled"), on(true));
}

#[test]
fn a_crash_is_recovered_by_a_new_controller_from_the_journal() {
    let fake = panel();
    fake.0.borrow_mut().excludes = vec![("chill.enabled", "anti_lag.enabled")];
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(JOURNAL_FILE);
    {
        let mut c = Controller::new(fake.clone(), path.clone());
        c.apply(
            "s1",
            1,
            &[
                change("chill.enabled", on(true)),
                change("pacing.limit", Value::Int(150)),
            ],
        )
        .unwrap();
        // The process dies here: nothing restores, the controller is just gone.
    }
    assert_eq!(fake.value("pacing.limit"), Value::Int(150));
    let mut restarted = Controller::new(fake.clone(), path);
    let report = restarted.recover_on_startup().unwrap();
    assert!(report.is_clean());
    // Both requested changes, and Anti-Lag, which Chill is expected to switch off.
    assert_eq!(report.outcomes.len(), 3);
    assert_eq!(fake.value("chill.enabled"), on(false));
    assert_eq!(fake.value("anti_lag.enabled"), on(true));
    assert_eq!(fake.value("pacing.limit"), Value::Int(140));
    assert!(restarted.journal().unwrap().entries.is_empty());
}

#[test]
fn startup_writes_the_original_back_even_when_the_read_says_it_is_there() {
    let fake = panel();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(JOURNAL_FILE);
    Controller::new(fake.clone(), path.clone())
        .apply("s1", 1, &[change("chill.enabled", on(true))])
        .unwrap();
    // The read still shows the original, though the write has landed.
    fake.0.borrow_mut().stale = Some(("chill.enabled", on(false), 1));
    let report = Controller::new(fake.clone(), path.clone())
        .recover_on_startup()
        .unwrap();
    assert_eq!(report.outcomes[0].result, RestoreResult::AlreadyOriginal);
    assert_eq!(
        fake.value("chill.enabled"),
        on(false),
        "written back anyway"
    );
    assert!(report.is_clean());
}

#[test]
fn a_session_restore_does_not_write_what_reads_as_original() {
    let fake = panel();
    let (mut c, _dir) = controller(&fake);
    c.apply("s1", 1, &[change("pacing.limit", Value::Int(150))])
        .unwrap();
    fake.set("pacing.limit", Value::Int(140));
    let writes = fake.writes().len();
    c.restore(Scope::Session("s1")).unwrap();
    assert_eq!(fake.writes().len(), writes);
}

#[test]
fn an_unreadable_gpu_fails_a_restore_without_dropping_the_entry() {
    let fake = panel();
    let (mut c, _dir) = controller(&fake);
    c.apply("s1", 1, &[change("pacing.limit", Value::Int(150))])
        .unwrap();
    fake.0.borrow_mut().unreadable = true;
    let report = c.recover_on_startup().unwrap();
    assert_eq!(report.failed().count(), 1);
    assert_eq!(c.journal().unwrap().entries.len(), 1);
}

#[test]
fn a_corrupt_journal_is_an_error_not_an_empty_one() {
    let fake = panel();
    let (mut c, dir) = controller(&fake);
    std::fs::write(dir.path().join(JOURNAL_FILE), b"{ not json").unwrap();
    assert!(c.recover_on_startup().is_err());
    let e = c
        .apply("s1", 1, &[change("chill.enabled", on(true))])
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Journal);
    assert!(fake.writes().is_empty());
    // A blank file or a missing one holds nothing.
    std::fs::write(dir.path().join(JOURNAL_FILE), b"  \n").unwrap();
    assert!(c.journal().unwrap().entries.is_empty());
}

#[test]
fn an_unreadable_driver_fails_the_request_before_anything_is_journaled() {
    let fake = panel();
    let (mut c, dir) = controller(&fake);
    fake.0.borrow_mut().unreadable = true;
    let e = c
        .apply("s1", 1, &[change("chill.enabled", on(true))])
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Unavailable);
    assert!(!dir.path().join(JOURNAL_FILE).exists());
}

fn entry_of<'a>(journal: &'a Journal, id: &str) -> &'a Entry {
    journal.entries.iter().find(|e| e.id == id).unwrap()
}

mod restore_and_ownership_tests;
