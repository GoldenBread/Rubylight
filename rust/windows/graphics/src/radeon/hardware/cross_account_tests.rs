//! Cross-account ADLX round-trip and stale-view refresh measurement hardware tests.

use super::*;

/// A backend that has every call run by the account behind `cross_account_server`.
#[derive(Clone)]
struct RemoteBackend {
    dir: PathBuf,
}
impl RemoteBackend {
    fn call(
        &self,
        request: &crate::radeon::helper::Request,
    ) -> Result<crate::radeon::helper::Response, crate::radeon::BackendError> {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let answer = self.dir.join(format!("resp-{id}.json"));
        let _ = std::fs::remove_file(&answer);
        publish(
            &self.dir.join(format!("req-{id}.json")),
            serde_json::to_string(request).expect("json").as_bytes(),
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        while !answer.exists() {
            if std::time::Instant::now() > deadline {
                return Err(crate::radeon::BackendError(
                    "the other account did not answer".into(),
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let text = std::fs::read_to_string(&answer)
            .map_err(|e| crate::radeon::BackendError(format!("cannot read the answer: {e}")))?;
        let _ = std::fs::remove_file(&answer);
        serde_json::from_str(&text)
            .map_err(|e| crate::radeon::BackendError(format!("unreadable answer: {e}")))
    }
}
impl RadeonBackend for RemoteBackend {
    fn read(&mut self, gpu: i32) -> Result<Vec<Setting>, crate::radeon::BackendError> {
        match self.call(&crate::radeon::helper::Request::Read { gpu })? {
            crate::radeon::helper::Response::Settings { settings } => Ok(settings),
            crate::radeon::helper::Response::Error { message } => {
                Err(crate::radeon::BackendError(message))
            }
            crate::radeon::helper::Response::Done => {
                Err(crate::radeon::BackendError("read answered done".into()))
            }
        }
    }
    fn write(
        &mut self,
        gpu: i32,
        id: &str,
        value: &Value,
    ) -> Result<(), crate::radeon::BackendError> {
        let request = crate::radeon::helper::Request::Write {
            gpu,
            id: id.into(),
            value: value.clone(),
        };
        match self.call(&request)? {
            crate::radeon::helper::Response::Done => Ok(()),
            crate::radeon::helper::Response::Error { message } => {
                Err(crate::radeon::BackendError(message))
            }
            crate::radeon::helper::Response::Settings { .. } => Err(crate::radeon::BackendError(
                "write answered with settings".into(),
            )),
        }
    }
}

#[derive(Serialize)]
struct CrossAccountRow {
    writer: String,
    reader: String,
    id: &'static str,
    wrote: Option<Value>,
    /// What the writer's own read-back showed for the setting.
    writer_saw: Option<Value>,
    /// What a fresh helper of the other account showed for it.
    reader_saw: Option<Value>,
    /// Settings (any, not just this one) on which the two accounts' fresh reads differ.
    differing: Vec<String>,
    /// The request was refused, or needed the feature switched on.
    detail: String,
}

/// Write every configurable setting as `writer_name` (through the journal) and read it back from a
/// fresh helper of `reader_name`, restoring afterwards.
fn cross_account_direction<W: RadeonBackend, R: RadeonBackend>(
    writer_name: &str,
    writer: W,
    reader_name: &str,
    reader: &mut R,
    start: &[Setting],
    gpu: i32,
) -> Vec<CrossAccountRow> {
    let dir = scratch_dir(&format!("xacct-{writer_name}"));
    let mut controller = Controller::new(writer, dir.join(JOURNAL_FILE));
    let mut rows = vec![];
    for setting in start
        .iter()
        .filter(|s| s.state == State::Configurable && s.kind != Kind::Action)
    {
        assert_host_idle();
        let Some(tried) = alternative(setting) else {
            continue;
        };
        let mut row = CrossAccountRow {
            writer: writer_name.into(),
            reader: reader_name.into(),
            id: setting.id,
            wrote: Some(tried.clone()),
            writer_saw: None,
            reader_saw: None,
            differing: vec![],
            detail: String::new(),
        };
        let mut refusals = vec![];
        for changes in &attempts(setting, start, &tried) {
            match controller.apply("xacct", gpu, changes) {
                Ok(after) => {
                    let theirs = reader.read(gpu).expect("the other account reads");
                    row.writer_saw = after
                        .iter()
                        .find(|s| s.id == setting.id)
                        .and_then(|s| s.value.clone());
                    row.reader_saw = theirs
                        .iter()
                        .find(|s| s.id == setting.id)
                        .and_then(|s| s.value.clone());
                    row.differing = diff(&after, &theirs);
                    if changes.len() > 1 {
                        row.detail = format!("with {} on", changes[0].id);
                    }
                    let report = controller
                        .restore(Scope::Session("xacct"))
                        .expect("restore");
                    assert!(
                        report.is_clean(),
                        "restoring {} failed: {:?}",
                        setting.id,
                        report.outcomes
                    );
                    let moved = diff(start, &reader.read(gpu).expect("read"));
                    assert!(
                        moved.is_empty(),
                        "after restoring {} the {reader_name} sees: {moved:?}",
                        setting.id
                    );
                    refusals.clear();
                    break;
                }
                Err(e) => refusals.push(e.to_string()),
            }
        }
        if !refusals.is_empty() {
            row.detail = format!("refused: {}", refusals.join("; "));
        }
        println!(
            "{writer_name}->{reader_name} {:<36} wrote {:<12} writer saw {:<12} reader saw {:<12} differing {:?} {}",
            setting.id,
            tried.display(),
            row.writer_saw.as_ref().map_or("-".into(), Value::display),
            row.reader_saw.as_ref().map_or("-".into(), Value::display),
            row.differing,
            row.detail
        );
        rows.push(row);
    }
    assert!(controller.journal().expect("journal").entries.is_empty());
    rows
}

/// The installed host is SYSTEM and a development host the signed-in user: is a setting one
/// account writes the setting the other reads? Needs `cross_account_server` running as the other
/// account; this test is the user. Both directions, every configurable setting.
#[test]
#[ignore = "changes real driver settings"]
fn cross_account_round_trip() {
    let _exclusive = exclusive();
    assert_host_idle();
    let dir = xacct_dir();
    let peer_identity = std::fs::read_to_string(dir.join("identity.txt"))
        .expect("cross_account_server is not running (no identity.txt)");
    let mine = identity();
    println!("peer: {peer_identity}\nthis test: {mine}");
    assert_ne!(peer_identity, mine, "the peer runs as the same account");
    let accounts = dir.join("helper-accounts.log");
    std::fs::write(&accounts, b"").expect("start the helper account log");
    let my_sid = crate::process_info::own_sid().expect("this test's SID");
    let peer = RemoteBackend { dir };
    let gpu = first_gpu();
    let start = helper().read(gpu).expect("read");
    let _restore = Restore::new(gpu, &start);
    let theirs = peer.clone().read(gpu).expect("the peer reads");
    let moved = diff(&start, &theirs);
    assert!(
        moved.is_empty(),
        "the two accounts read different values before any write: {moved:?}"
    );

    let mut rows =
        cross_account_direction("peer", peer.clone(), "user", &mut helper(), &start, gpu);
    rows.extend(cross_account_direction(
        "user",
        helper(),
        "peer",
        &mut peer.clone(),
        &start,
        gpu,
    ));
    let disagree: Vec<_> = rows
        .iter()
        .filter(|r| !r.differing.is_empty() || r.writer_saw != r.reader_saw)
        .map(|r| format!("{}->{} {}: {:?}", r.writer, r.reader, r.id, r.differing))
        .collect();
    let written = rows.iter().filter(|r| r.writer_saw.is_some()).count();
    save("rl-a5-xacct.json", &rows);
    assert!(disagree.is_empty(), "the accounts disagree: {disagree:?}");
    // Every helper the peer started ran as this test's user, none as the peer's own account.
    let log = std::fs::read_to_string(&accounts).expect("helper account log");
    let lines: Vec<&str> = log.lines().collect();
    println!(
        "{} helpers started by the peer ({peer_identity}); first: {}",
        lines.len(),
        lines.first().copied().unwrap_or("none")
    );
    assert!(!lines.is_empty(), "no helper reported its account");
    let wrong: Vec<_> = lines
        .iter()
        .filter(|l| !l.contains(&format!("sid {my_sid} system false")))
        .collect();
    assert!(
        wrong.is_empty(),
        "helpers not running as {my_sid}: {wrong:?}"
    );
    let end = helper().read(gpu).expect("read");
    let moved = diff(&start, &end);
    assert!(moved.is_empty(), "final probe diff is not empty: {moved:?}");
    println!(
        "{written} writes across both directions, all read back identically by the other account; final probe diff: empty"
    );
}

/// What makes this process see a write another process made? After an `abort()` of the writer
/// this process kept reading the old value for 30 s, while a new process saw the new one at once.
/// Each method is tried against a fresh abort: re-reading (every read already re-acquires the
/// services and interfaces), one `ADLXTerminate` before the next read, and
/// `ADLXInitializeWithIncompatibleDriver`.
#[test]
#[ignore = "changes real driver settings"]
fn measure_view_refresh() {
    let _exclusive = exclusive();
    assert_host_idle();
    let gpu = first_gpu();
    let start = helper().read(gpu).expect("read");
    let restore = Restore::new(gpu, &start);
    snapshot(&start);
    let (id, tried) = crash_target(&start);
    let original = start
        .iter()
        .find(|s| s.id == id)
        .and_then(|s| s.value.clone())
        .expect("value");
    // The one place that reads in this process on purpose: its view is what is being measured.
    let read_target = || {
        AdlxBackend
            .read(gpu)
            .expect("read")
            .into_iter()
            .find(|s| s.id == id)
            .and_then(|s| s.value)
    };
    // `terminate` crashed this process with an access violation the one time it was tried
    // (0xC0000005 on the read after it), so it only runs when asked for.
    let methods = std::env::var("RADEON_REFRESH_METHODS")
        .unwrap_or_else(|_| "re-read,incompatible-driver,wait".into());
    for method in methods.split(',') {
        assert_host_idle();
        assert_eq!(read_target().as_ref(), Some(&original));
        let dir = scratch_dir("refresh");
        let status = run_child("crash_child", &dir, false);
        assert!(!status.success(), "the child should have died: {status}");
        let fresh = fresh_value(gpu, id);
        let began = std::time::Instant::now();
        let mut seen = None;
        let mut applied = String::new();
        while began.elapsed() < Duration::from_secs(12) && seen.is_none() {
            if applied.is_empty() {
                // The shared ADLX lock is held only while the method runs, so the reads below
                // can take it again.
                let runtime = crate::adlx_system::acquire().expect("ADLX");
                let library = runtime.library();
                applied = match method {
                    "terminate" => {
                        // SAFETY: No ADLX session is alive on this thread.
                        let result = unsafe {
                            library
                                .get::<unsafe extern "C" fn() -> i32>(b"ADLXTerminate\0")
                                .map(|f| f())
                        };
                        format!("ADLXTerminate -> {result:?}")
                    }
                    "incompatible-driver" => {
                        let mut system = std::ptr::null_mut();
                        type Init = unsafe extern "C" fn(u64, *mut crate::telemetry::Handle) -> i32;
                        // SAFETY: Same signature as ADLXInitialize; the output is a live pointer.
                        let result = unsafe {
                            library
                                .get::<Init>(b"ADLXInitializeWithIncompatibleDriver\0")
                                .map(|f| f(crate::telemetry::ADLX_VERSION, &mut system))
                        };
                        format!("ADLXInitializeWithIncompatibleDriver -> {result:?}")
                    }
                    _ => "nothing".into(),
                };
            }
            if read_target().as_ref() == Some(&tried) {
                seen = Some(began.elapsed());
            } else {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        println!(
            "REFRESH {method}: a new process read {fresh}; this process {} ({applied})",
            seen.map_or("did not see it within 12 s".to_string(), |t| format!(
                "saw it after {t:?}"
            ))
        );
        // Put back through fresh helper processes: this process's own writes are not trusted
        // while its view is stale (a write of the value it believes is current is dropped).
        let stuck = restore.put_back();
        assert!(stuck.is_empty(), "could not restore: {stuck:?}");
        assert_eq!(fresh_value(gpu, id), original.display(), "{id} not back");
    }
}
