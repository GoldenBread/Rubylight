//! Partner side-effect, restore-through-feature, ownership and deadline control tests.

use super::*;

#[test]
fn a_partner_the_driver_will_switch_off_is_on_disk_before_the_first_write() {
    let fake = panel();
    let (mut c, _dir) = controller(&fake);
    fake.0.borrow_mut().excludes = vec![("chill.enabled", "anti_lag.enabled")];
    c.apply("s1", 1, &[change("chill.enabled", on(true))])
        .unwrap();
    let seen = fake.0.borrow().journal_at_first_write.clone().unwrap();
    let effect = entry_of(&seen, "anti_lag.enabled");
    assert!(effect.side_effect);
    assert_eq!((&effect.original, &effect.written), (&on(true), &on(false)));
    assert_eq!(fake.value("anti_lag.enabled"), on(false));
}

#[test]
fn a_partner_the_driver_leaves_alone_is_harmless_in_the_journal() {
    let fake = panel();
    let (mut c, _dir) = controller(&fake);
    c.apply("s1", 1, &[change("chill.enabled", on(true))])
        .unwrap();
    let seen = fake.0.borrow().journal_at_first_write.clone().unwrap();
    assert!(entry_of(&seen, "anti_lag.enabled").side_effect);
    let writes = fake.writes().len();
    let report = c.restore(Scope::Session("s1")).unwrap();
    assert!(report.is_clean());
    assert_eq!(fake.value("anti_lag.enabled"), on(true));
    // Chill went back; Anti-Lag already was original and was not written.
    assert_eq!(fake.writes().len(), writes + 1);
    assert!(c.journal().unwrap().entries.is_empty());
}

#[test]
fn a_predicted_partner_the_driver_left_alone_stays_journaled_and_restores_as_already_original() {
    let fake = panel();
    let (mut c, _dir) = controller(&fake);
    c.apply("s1", 1, &[change("chill.enabled", on(true))])
        .unwrap();
    // The read-back shows Anti-Lag still on, yet its entry stays: the driver may apply the
    // switch after the read.
    let journal = c.journal().unwrap();
    let partner = entry_of(&journal, "anti_lag.enabled");
    assert!(partner.side_effect);
    assert!(journal.entries.iter().any(|e| e.id == "chill.enabled"));
    let report = c.restore(Scope::Session("s1")).unwrap();
    let result = |id: &str| {
        report
            .outcomes
            .iter()
            .find(|o| o.id == id)
            .map(|o| o.result.clone())
    };
    assert_eq!(
        result("anti_lag.enabled"),
        Some(RestoreResult::AlreadyOriginal)
    );
    assert_eq!(result("chill.enabled"), Some(RestoreResult::Restored));
    assert!(c.journal().unwrap().entries.is_empty());
    // A partner the driver did switch stays, with the value to go back to.
    let fake = panel();
    let (mut c, _dir) = controller(&fake);
    fake.0.borrow_mut().excludes = vec![("chill.enabled", "anti_lag.enabled")];
    c.apply("s1", 1, &[change("chill.enabled", on(true))])
        .unwrap();
    let journal = c.journal().unwrap();
    let effect = entry_of(&journal, "anti_lag.enabled");
    assert_eq!((&effect.original, &effect.written), (&on(true), &on(false)));
}

#[test]
fn enhanced_sync_is_refused_while_vsync_is_on() {
    let mut settings = panel().0.borrow().settings.clone();
    settings.push(toggle("enhanced_sync.enabled", false));
    settings.push(read_only("vsync.enabled", on(true)));
    let fake = Fake::with(settings);
    let (mut c, _dir) = controller(&fake);
    let e = c
        .apply("s1", 1, &[change("enhanced_sync.enabled", on(true))])
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::InvalidValue);
    assert_eq!(e.id.as_deref(), Some("enhanced_sync.enabled"));
    assert!(e.message.contains("vsync"), "{e}");
    assert!(fake.writes().is_empty());
    assert!(c.journal().unwrap().entries.is_empty());
    // With vsync off there is nothing for it to switch.
    fake.set("vsync.enabled", on(false));
    c.apply("s1", 1, &[change("enhanced_sync.enabled", on(true))])
        .unwrap();
}

#[test]
fn a_side_effect_that_cannot_be_journaled_undoes_the_request() {
    let mut settings = panel().0.borrow().settings.clone();
    settings.push(toggle("surprise.enabled", true));
    let fake = Fake::with(settings);
    let (mut c, _dir) = controller(&fake);
    // Nothing in the dependency table predicts this one.
    fake.0.borrow_mut().excludes = vec![("chill.enabled", "surprise.enabled")];
    fake.0.borrow_mut().break_journal_at = Some("chill.enabled");
    let e = c
        .apply("s1", 1, &[change("chill.enabled", on(true))])
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Journal, "{e}");
    assert_eq!(fake.value("chill.enabled"), on(false));
    assert_eq!(fake.value("surprise.enabled"), on(true));
}

fn stuck_request() -> [Change; 2] {
    [
        change("pacing.limit", Value::Int(150)),
        change("tessellation.mode", Value::Choice("b".into())),
    ]
}

#[test]
fn a_stuck_undo_is_journaled_with_the_value_it_holds() {
    let fake = panel();
    let (mut c, _dir) = controller(&fake);
    fake.0.borrow_mut().refuse = Some("tessellation.mode");
    // The first write lands, the undo write of the same setting is dropped.
    fake.0.borrow_mut().ignore_from = Some(("pacing.limit", 1));
    let e = c.apply("s1", 1, &stuck_request()).unwrap_err();
    assert_eq!(e.kind, ErrorKind::DriverRejected);
    assert!(e.message.contains("pacing.limit"), "{e}");
    assert_eq!(fake.value("pacing.limit"), Value::Int(150));
    let journal = c.journal().unwrap();
    assert_eq!(journal.entries.len(), 1);
    let stuck = entry_of(&journal, "pacing.limit");
    assert_eq!(stuck.original, Value::Int(140));
    assert_eq!(stuck.written, Value::Int(150));
    assert_eq!(stuck.session, "s1");
    // Once the driver listens again a restore finds it.
    fake.0.borrow_mut().ignore_from = None;
    let report = c.restore(Scope::Session("s1")).unwrap();
    assert_eq!(report.outcomes[0].result, RestoreResult::Restored);
    assert_eq!(fake.value("pacing.limit"), Value::Int(140));
}

#[test]
fn a_stuck_undo_that_cannot_be_journaled_fails_loudly() {
    let fake = panel();
    let (mut c, _dir) = controller(&fake);
    fake.0.borrow_mut().refuse = Some("tessellation.mode");
    fake.0.borrow_mut().ignore_from = Some(("pacing.limit", 1));
    fake.0.borrow_mut().break_journal_at = Some("tessellation.mode");
    let e = c.apply("s1", 1, &stuck_request()).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Journal);
    assert!(e.message.contains("pacing.limit"), "{e}");
    assert!(e.message.contains("journal could not be updated"), "{e}");
}

#[test]
fn restore_switches_a_feature_on_for_its_field_and_puts_it_back() {
    let fake = panel();
    let (mut c, _dir) = controller(&fake);
    fake.0.borrow_mut().needs_on = vec![("chill.max_fps", "chill.enabled")];
    fake.0.borrow_mut().excludes = vec![("chill.enabled", "anti_lag.enabled")];
    c.apply(
        "s1",
        1,
        &[
            change("chill.enabled", on(true)),
            change("chill.max_fps", Value::Int(150)),
        ],
    )
    .unwrap();
    // Chill was turned off on the PC; the driver now refuses chill.max_fps.
    fake.set("chill.enabled", on(false));
    let writes = fake.writes().len();
    let report = c.restore(Scope::Session("s1")).unwrap();
    assert!(report.is_clean(), "{report:?}");
    assert_eq!(fake.value("chill.max_fps"), Value::Int(140));
    assert_eq!(fake.value("chill.enabled"), on(false));
    assert_eq!(fake.value("anti_lag.enabled"), on(true));
    assert_eq!(
        fake.writes()[writes..3 + writes],
        ["chill.enabled=on", "chill.max_fps=140", "chill.enabled=off"]
    );
    assert!(c.journal().unwrap().entries.is_empty());
}

#[test]
fn restore_journals_the_temporary_switch_before_it_writes() {
    let fake = panel();
    let (mut c, _dir) = controller(&fake);
    {
        let mut d = fake.0.borrow_mut();
        d.needs_on = vec![("chill.max_fps", "chill.enabled")];
        d.excludes = vec![("chill.enabled", "anti_lag.enabled")];
    }
    fake.set("chill.max_fps", Value::Int(150));
    let mut journal = Journal::default();
    journal.record(Entry {
        user: me(),
        gpu: 1,
        id: "chill.max_fps".into(),
        session: "s1".into(),
        original: Value::Int(140),
        written: Value::Int(150),
        side_effect: false,
    });
    journal.save(&c.journal).unwrap();
    let report = c.restore(Scope::All).unwrap();
    assert!(report.is_clean(), "{report:?}");
    let seen = fake.0.borrow().journal_at_first_write.clone().unwrap();
    let toggle = entry_of(&seen, "chill.enabled");
    assert_eq!((&toggle.original, &toggle.written), (&on(false), &on(true)));
    let partner = entry_of(&seen, "anti_lag.enabled");
    assert_eq!(
        (&partner.original, &partner.written),
        (&on(true), &on(false))
    );
    assert_eq!(fake.value("chill.max_fps"), Value::Int(140));
    assert_eq!(fake.value("chill.enabled"), on(false));
    assert_eq!(fake.value("anti_lag.enabled"), on(true));
    assert!(c.journal().unwrap().entries.is_empty());
}

#[test]
fn a_failed_restore_through_the_feature_leaves_the_temporary_entries() {
    let fake = panel();
    let (mut c, _dir) = controller(&fake);
    fake.0.borrow_mut().needs_on = vec![("chill.max_fps", "chill.enabled")];
    fake.set("chill.max_fps", Value::Int(150));
    // The field write fails, so the feature stays on and the entries stay.
    fake.0.borrow_mut().refuse = Some("chill.max_fps");
    let mut journal = Journal::default();
    journal.record(Entry {
        user: me(),
        gpu: 1,
        id: "chill.max_fps".into(),
        session: "s1".into(),
        original: Value::Int(140),
        written: Value::Int(150),
        side_effect: false,
    });
    journal.save(&c.journal).unwrap();
    let report = c.restore(Scope::All).unwrap();
    assert_eq!(report.failed().count(), 1);
    // The feature the restore switched on is switched back off before it gives up.
    assert_eq!(fake.value("chill.enabled"), on(false));
    let RestoreResult::Failed(message) = &report.failed().next().unwrap().result else {
        unreachable!()
    };
    assert!(
        message.contains("chill.enabled was switched back"),
        "{message}"
    );
    let journal = c.journal().unwrap();
    assert!(journal.entries.iter().any(|e| e.id == "chill.enabled"));
    fake.0.borrow_mut().refuse = None;
    let report = c.recover_on_startup().unwrap();
    assert!(report.is_clean(), "{report:?}");
    assert_eq!(fake.value("chill.max_fps"), Value::Int(140));
    assert_eq!(fake.value("chill.enabled"), on(false));
    assert!(c.journal().unwrap().entries.is_empty());
}

#[test]
fn a_field_that_needs_its_feature_on_is_refused_unless_it_is_on_by_then() {
    let fake = panel();
    let (mut c, _dir) = controller(&fake);
    let cases: [Vec<Change>; 2] = [
        vec![change("chill.max_fps", Value::Int(150))],
        vec![
            change("chill.max_fps", Value::Int(150)),
            change("chill.enabled", on(true)),
        ],
    ];
    for request in cases {
        let e = c.apply("s1", 1, &request).unwrap_err();
        assert_eq!(e.kind, ErrorKind::InvalidValue);
        assert_eq!(e.id.as_deref(), Some("chill.max_fps"));
        assert!(e.message.contains("chill.enabled"), "{e}");
    }
    assert!(fake.writes().is_empty());
    c.apply(
        "s1",
        1,
        &[
            change("chill.enabled", on(true)),
            change("chill.max_fps", Value::Int(150)),
        ],
    )
    .unwrap();
    // On now, so the field alone is fine; switching it off first is not.
    c.apply("s1", 1, &[change("chill.max_fps", Value::Int(160))])
        .unwrap();
    let e = c
        .apply(
            "s1",
            1,
            &[
                change("chill.enabled", on(false)),
                change("chill.max_fps", Value::Int(170)),
            ],
        )
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::InvalidValue);
}

fn read_only(id: &'static str, value: Value) -> Setting {
    let mut s = setting(id, Kind::Toggle, value);
    s.state = State::ReadOnly;
    s
}

#[test]
fn enabling_a_group_that_excludes_a_setting_the_host_cannot_restore_is_refused() {
    let fake = Fake::with(vec![
        toggle("boost.enabled", false),
        read_only("rsr.enabled", on(true)),
    ]);
    let (mut c, dir) = controller(&fake);
    let e = c
        .apply("s1", 1, &[change("boost.enabled", on(true))])
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::InvalidValue);
    assert_eq!(e.id.as_deref(), Some("boost.enabled"));
    assert!(e.message.contains("rsr.enabled"), "{e}");
    assert!(fake.writes().is_empty());
    assert!(!dir.path().join(JOURNAL_FILE).exists());
    // Switching it off, or enabling it while RSR is off, is fine.
    fake.set("rsr.enabled", on(false));
    c.apply("s1", 1, &[change("boost.enabled", on(true))])
        .unwrap();
}

#[test]
fn a_derived_read_only_partner_does_not_block_its_driving_setting() {
    let derived = |mode_state: State| {
        let mut mode = choice("vsync.mode", "a");
        mode.state = mode_state;
        Fake::with(vec![
            toggle("enhanced_sync.enabled", false),
            mode,
            read_only("vsync.enabled", on(true)),
        ])
    };
    // vsync.enabled follows vsync.mode, which the host can write and restore, so the
    // "cannot restore" refusal does not apply; the unmeasured-vsync one still does.
    let fake = derived(State::Configurable);
    let (mut c, _dir) = controller(&fake);
    let e = c
        .apply("s1", 1, &[change("enhanced_sync.enabled", on(true))])
        .unwrap_err();
    assert!(e.message.contains("not measured"), "{e}");
    let fake = derived(State::ReadOnly);
    let (mut c, _dir) = controller(&fake);
    let e = c
        .apply("s1", 1, &[change("enhanced_sync.enabled", on(true))])
        .unwrap_err();
    assert!(e.message.contains("vsync.enabled"), "{e}");
}

#[test]
fn a_setting_the_host_cannot_write_that_moved_undoes_the_request() {
    let fake = panel();
    let (mut c, _dir) = controller(&fake);
    fake.set("rsr.enabled", on(true));
    // The dependency table does not know about this one.
    fake.0.borrow_mut().excludes = vec![("chill.enabled", "rsr.enabled")];
    let e = c
        .apply("s1", 1, &[change("chill.enabled", on(true))])
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::DriverRejected);
    assert_eq!(e.id.as_deref(), Some("rsr.enabled"));
    assert!(e.message.contains("cannot write or restore"), "{e}");
    assert_eq!(fake.value("chill.enabled"), on(false));
    assert!(c.journal().unwrap().entries.is_empty());
}

#[test]
fn a_local_change_between_writes_becomes_the_value_to_go_back_to() {
    let fake = panel();
    let (mut c, _dir) = controller(&fake);
    c.apply("s1", 1, &[change("pacing.limit", Value::Int(150))])
        .unwrap();
    fake.set("pacing.limit", Value::Int(200));
    c.apply("s1", 1, &[change("pacing.limit", Value::Int(160))])
        .unwrap();
    let journal = c.journal().unwrap();
    assert_eq!(entry_of(&journal, "pacing.limit").original, Value::Int(200));
    c.restore(Scope::Session("s1")).unwrap();
    assert_eq!(fake.value("pacing.limit"), Value::Int(200));
}

fn live_process() -> std::process::Child {
    std::process::Command::new("cmd")
        .args(["/c", "ping -n 30 127.0.0.1 >nul"])
        .spawn()
        .unwrap()
}
fn journal_owned_by(path: &Path, pid: u32, started: u64, entries: Vec<Entry>) {
    let journal = Journal {
        version: 1,
        owner: Some(Owner { pid, started }),
        entries,
    };
    state::write_json(path, &journal).unwrap();
}
fn some_entry() -> Entry {
    Entry {
        user: me(),
        gpu: 1,
        id: "pacing.limit".into(),
        session: "other".into(),
        original: Value::Int(140),
        written: Value::Int(150),
        side_effect: false,
    }
}

#[test]
fn a_second_host_does_not_touch_a_live_hosts_journal() {
    let fake = panel();
    fake.set("pacing.limit", Value::Int(150));
    let (mut c, _dir) = controller(&fake);
    let mut other = live_process();
    let started = owner::identity(other.id()).unwrap().unwrap();
    journal_owned_by(&c.journal, other.id(), started, vec![some_entry()]);
    assert!(c.recover_on_startup().is_err());
    assert!(c.restore(Scope::Session("other")).is_err());
    let e = c
        .apply("s2", 1, &[change("chill.enabled", on(true))])
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Journal);
    assert!(e.message.contains("another host"), "{e}");
    assert!(fake.writes().is_empty());
    assert_eq!(c.journal().unwrap().entries.len(), 1);
    other.kill().unwrap();
    let _ = other.wait();
    // Once that host is gone its entries are restored like a crash's.
    let report = c.recover_on_startup().unwrap();
    assert!(report.is_clean());
    assert_eq!(fake.value("pacing.limit"), Value::Int(140));
}

#[test]
fn a_live_hosts_empty_journal_does_not_block_a_second_host() {
    let fake = panel();
    let (mut c, _dir) = controller(&fake);
    let mut other = live_process();
    let started = owner::identity(other.id()).unwrap().unwrap();
    journal_owned_by(&c.journal, other.id(), started, vec![]);
    c.apply("s2", 1, &[change("chill.enabled", on(true))])
        .unwrap();
    assert_eq!(c.journal().unwrap().owner, Some(Owner::current().unwrap()));
    other.kill().unwrap();
    let _ = other.wait();
}

/// Radeon settings are per user. An entry is filed under the user who made it, and a restore
/// for another user (or while nobody is signed in) leaves it in the journal.
#[test]
fn restore_leaves_another_users_entries_in_place() {
    use std::sync::{Arc, Mutex};
    const A: &str = "S-1-5-21-1-2-3-1001";
    const B: &str = "S-1-5-21-1-2-3-1002";
    let fake = panel();
    let (c, _dir) = controller(&fake);
    let who = Arc::new(Mutex::new(Some(A.to_string())));
    let seen = who.clone();
    let mut c = c.with_user(move || seen.lock().unwrap().clone());
    let sign_in = |user: Option<&str>| *who.lock().unwrap() = user.map(String::from);

    c.apply("s1", 1, &[change("pacing.limit", Value::Int(150))])
        .unwrap();
    assert_eq!(c.journal().unwrap().entries[0].user, A);

    // B is signed in: A's entry is neither restored nor dropped, and nothing is written.
    sign_in(Some(B));
    let writes = fake.writes().len();
    let report = c.recover_on_startup().unwrap();
    assert!(report.outcomes.is_empty(), "{:?}", report.outcomes);
    assert_eq!(fake.writes().len(), writes);
    assert_eq!(fake.value("pacing.limit"), Value::Int(150));
    assert_eq!(c.journal().unwrap().entries.len(), 1);

    // B's own change to the same setting is a separate entry with B's original.
    c.apply("s2", 1, &[change("pacing.limit", Value::Int(160))])
        .unwrap();
    let journal = c.journal().unwrap();
    assert_eq!(journal.entries.len(), 2);
    let b = journal.entries.iter().find(|e| e.user == B).unwrap();
    assert_eq!(
        (&b.original, &b.written),
        (&Value::Int(150), &Value::Int(160))
    );
    let a = journal.entries.iter().find(|e| e.user == A).unwrap();
    assert_eq!(
        (&a.original, &a.written),
        (&Value::Int(140), &Value::Int(150))
    );

    // B's restore puts back B's value only.
    assert!(c.restore(Scope::All).unwrap().is_clean());
    assert_eq!(fake.value("pacing.limit"), Value::Int(150));
    let journal = c.journal().unwrap();
    assert_eq!(journal.entries.len(), 1);
    assert_eq!(journal.entries[0].user, A);

    // Nobody signed in: nothing is restored and nothing can be changed.
    sign_in(None);
    assert!(c.recover_on_startup().unwrap().outcomes.is_empty());
    assert_eq!(c.journal().unwrap().entries.len(), 1);
    let e = c
        .apply("s3", 1, &[change("pacing.limit", Value::Int(170))])
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Unavailable);
    assert!(e.message.contains("nobody"), "{e}");

    // A comes back and gets A's value back.
    sign_in(Some(A));
    assert!(c.recover_on_startup().unwrap().is_clean());
    assert_eq!(fake.value("pacing.limit"), Value::Int(140));
    assert!(c.journal().unwrap().entries.is_empty());
}

#[test]
fn restore_stops_at_its_deadline_and_keeps_what_is_left() {
    let fake = panel();
    let (mut c, _dir) = controller(&fake);
    c.apply("s1", 1, &[change("pacing.limit", Value::Int(150))])
        .unwrap();
    c.set_deadline(Some(Instant::now() - std::time::Duration::from_secs(1)));
    let report = c.recover_on_startup().unwrap();
    assert_eq!(report.failed().count(), 1);
    assert_eq!(fake.value("pacing.limit"), Value::Int(150));
    c.set_deadline(None);
    assert!(c.recover_on_startup().unwrap().is_clean());
    assert_eq!(fake.value("pacing.limit"), Value::Int(140));
}
