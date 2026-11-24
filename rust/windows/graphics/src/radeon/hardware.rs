//! Hardware tests for the write layer (plan A5 step 2). They change real driver settings, so they
//! are `#[ignore]`d and only run by hand on a PC whose installed host is idle:
//!
//! ```text
//! cargo test -p butterpollo-win-graphics --lib radeon::hardware -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Every test checks the host is idle first and puts every setting back in a guard that also runs
//! when the test panics. The results are printed and written as JSON next to the repository
//! (`RADEON_HW_OUT`, default `C:\src`).
//!
//! Reads and writes go through `helper()`: a fresh process per call, exactly as the host does it,
//! because a long-lived process's ADLX view goes stale after any other process writes (see
//! `radeon::helper`). Only `session_churn_is_stable` and `measure_view_refresh` use the in-process
//! `AdlxBackend`, and say why.
use super::{
    AdlxBackend, Change, Controller, RadeonBackend, Scope,
    control::JOURNAL_FILE,
    helper::HelperBackend,
    model::{Kind, Setting, State, Value, enum_items},
    probe,
    worker::{Phase, Timing, Worker},
};
use serde::Serialize;
use std::{
    io::{Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    process::Command,
    sync::{Mutex, MutexGuard},
    time::Duration,
};

/// The tests share one GPU, so they run one at a time even without `--test-threads=1`.
static HARDWARE: Mutex<()> = Mutex::new(());
fn exclusive() -> MutexGuard<'static, ()> {
    HARDWARE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The installed host must have no stream, pending or running. No host at all counts as idle.
fn assert_host_idle() {
    let Ok(mut stream) = TcpStream::connect_timeout(
        &"127.0.0.1:47989".parse().expect("address"),
        Duration::from_secs(2),
    ) else {
        return;
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");
    stream
        .write_all(b"GET /serverinfo HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n")
        .expect("request");
    let mut body = String::new();
    let _ = stream.read_to_string(&mut body);
    for tag in ["RustHostSessionCount", "RustHostPendingSessionCount"] {
        let count = body
            .split(&format!("<{tag}>"))
            .nth(1)
            .and_then(|rest| rest.split('<').next())
            .and_then(|n| n.trim().parse::<u32>().ok());
        assert_eq!(count, Some(0), "the installed host is not idle ({tag})");
    }
}

/// The backend the host uses: every call runs in a new process (this test binary, acting as the
/// helper), so every read sees the driver as it is now.
fn helper() -> HelperBackend {
    HelperBackend::with_command(
        std::env::current_exe().expect("test binary"),
        [
            "--exact",
            "radeon::hardware::helper_child",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ]
        .map(String::from)
        .to_vec(),
    )
}
#[test]
#[ignore = "child of the hardware tests"]
fn helper_child() {
    // The cross-account test asks every helper to say which account it runs as: it creates
    // `helper-accounts.log` in the exchange folder, and each helper appends a line.
    let log = xacct_dir().join("helper-accounts.log");
    if log.exists() {
        use std::io::Write;
        let line = format!(
            "pid {} sid {} system {}\n",
            std::process::id(),
            crate::process_info::own_sid().unwrap_or_else(|e| format!("unknown ({e:#})")),
            crate::process_info::is_system()
        );
        if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(&log) {
            let _ = file.write_all(line.as_bytes());
        }
    }
    assert_eq!(super::helper::run(), 0);
}

fn first_gpu() -> i32 {
    probe().expect("probe").gpus.first().expect("an AMD GPU").id
}

fn out_path(name: &str) -> PathBuf {
    let dir = std::env::var("RADEON_HW_OUT").unwrap_or_else(|_| r"C:\src".into());
    Path::new(&dir).join(name)
}

fn scratch_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("radeon-hw-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// Writes every setting back to its starting value on drop, panic or not. A setting that will
/// not go back is named loudly with the value it must return to. It reads and writes through
/// `helper()`, so it sees the driver as it is now and never skips a write because this process
/// believes the value is already right.
struct Restore {
    gpu: i32,
    originals: Vec<(String, Value)>,
}
impl Restore {
    fn new(gpu: i32, start: &[Setting]) -> Self {
        snapshot(start);
        Self {
            gpu,
            originals: start
                .iter()
                .filter(|s| s.kind != Kind::Action)
                .filter_map(|s| Some((s.id.to_string(), s.value.clone()?)))
                .collect(),
        }
    }
    fn differing(&self) -> Vec<(String, Value, Option<Value>)> {
        let Ok(now) = helper().read(self.gpu) else {
            return self
                .originals
                .iter()
                .map(|(id, value)| (id.clone(), value.clone(), None))
                .collect();
        };
        self.originals
            .iter()
            .filter_map(|(id, original)| {
                let current = now.iter().find(|s| s.id == id)?.value.clone();
                (current.as_ref() != Some(original))
                    .then(|| (id.clone(), original.clone(), current))
            })
            .collect()
    }
    /// Write back whatever differs, up to three rounds. Returns what would not go back.
    fn put_back(&self) -> Vec<(String, Value, Option<Value>)> {
        for _ in 0..3 {
            let differing = self.differing();
            if differing.is_empty() {
                return vec![];
            }
            let Ok(now) = helper().read(self.gpu) else {
                continue;
            };
            // Fields before their `.enabled` toggles, so a field of a feature that is off can
            // borrow the toggle for its write.
            let mut order = differing;
            order.sort_by_key(|(id, _, _)| id.ends_with(".enabled"));
            for (id, original, _) in &order {
                let toggle = super::model::feature_toggle(id);
                let borrowed = toggle.filter(|toggle| {
                    now.iter()
                        .find(|s| s.id == toggle.as_str())
                        .and_then(|s| s.value.as_ref())
                        == Some(&Value::Bool(false))
                });
                if let Some(toggle) = &borrowed {
                    let _ = helper().write(self.gpu, toggle, &Value::Bool(true));
                }
                // vsync.enabled follows vsync.mode and has no setter; the write error is expected.
                let _ = helper().write(self.gpu, id, original);
                if let Some(toggle) = &borrowed {
                    let _ = helper().write(self.gpu, toggle, &Value::Bool(false));
                }
            }
        }
        self.differing()
    }
}
impl Drop for Restore {
    fn drop(&mut self) {
        let stuck = self.put_back();
        if stuck.is_empty() {
            return;
        }
        for (id, original, current) in &stuck {
            eprintln!(
                "RESTORE FAILED: {id} must return to {} and is {}",
                original.display(),
                current.as_ref().map_or("unreadable".into(), Value::display)
            );
        }
        if !std::thread::panicking() {
            panic!("could not restore {} setting(s)", stuck.len());
        }
    }
}

/// A different value the validator accepts, close to the current one.
fn alternative(setting: &Setting) -> Option<Value> {
    match (setting.kind, setting.value.as_ref()?) {
        (Kind::Toggle, Value::Bool(on)) => Some(Value::Bool(!on)),
        (Kind::Integer, Value::Int(value)) => {
            let range = setting.range?;
            [value + range.step, value - range.step, range.min, range.max]
                .into_iter()
                .find(|c| {
                    c != value
                        && *c >= range.min
                        && *c <= range.max
                        && (c - range.min) % range.step == 0
                })
                .map(Value::Int)
        }
        (Kind::Choice, Value::Choice(current)) => setting
            .options
            .iter()
            .find(|o| &o.value != current)
            .map(|o| Value::Choice(o.value.clone())),
        _ => None,
    }
}

/// The requests to try for one setting. A setting of a feature that is off may only take a value
/// while the same request turns the feature on, so it is tried alone first and then with the
/// feature's toggle; the matrix records which settings need that.
fn attempts(setting: &Setting, start: &[Setting], tried: &Value) -> Vec<Vec<Change>> {
    let target = Change {
        id: setting.id.into(),
        value: tried.clone(),
    };
    let group_enable = setting
        .id
        .split_once('.')
        .filter(|(_, field)| *field != "enabled")
        .map(|(group, _)| format!("{group}.enabled"))
        .filter(|enable| {
            start.iter().any(|s| {
                s.id == enable
                    && s.state == State::Configurable
                    && s.value == Some(Value::Bool(false))
            })
        });
    let mut attempts = vec![vec![target.clone()]];
    if let Some(enable) = group_enable {
        attempts.push(vec![
            Change {
                id: enable,
                value: Value::Bool(true),
            },
            target,
        ]);
    }
    attempts
}

/// Every setting's value, for the before and after comparison.
fn values(settings: &[Setting]) -> Vec<(&'static str, Option<Value>)> {
    settings.iter().map(|s| (s.id, s.value.clone())).collect()
}
fn diff(start: &[Setting], now: &[Setting]) -> Vec<String> {
    let (a, b) = (values(start), values(now));
    let mut out = vec![];
    for (id, was) in &a {
        let is = b
            .iter()
            .find(|(i, _)| i == id)
            .and_then(|(_, v)| v.as_ref());
        if was.as_ref() != is {
            out.push(format!("{id}: was {was:?}, now {is:?}"));
        }
    }
    out
}

#[derive(Serialize)]
struct Proof {
    id: &'static str,
    kind: Kind,
    /// "yes", "no" or "action".
    proven_writable: &'static str,
    original: Option<Value>,
    tried: Option<Value>,
    detail: String,
}

fn save<T: Serialize>(name: &str, value: &T) {
    let path = out_path(name);
    std::fs::write(&path, serde_json::to_vec_pretty(value).expect("json")).expect("write results");
    println!("results written to {}", path.display());
}

/// Starting values on disk before a test writes anything, so a run that dies hard (an access
/// violation does not run the guard) can be put back with `restore_snapshot`.
fn snapshot(start: &[Setting]) {
    let values: std::collections::BTreeMap<&str, &Value> = start
        .iter()
        .filter(|s| s.kind != Kind::Action)
        .filter_map(|s| Some((s.id, s.value.as_ref()?)))
        .collect();
    // A run that died before restoring leaves changed values in the next run's start, so the
    // snapshot it replaces is kept beside it.
    let path = out_path("rl-a5-snapshot.json");
    if let Ok(previous) = std::fs::read(&path)
        && previous != serde_json::to_vec_pretty(&values).expect("json")
    {
        let _ = std::fs::write(out_path("rl-a5-snapshot.prev.json"), previous);
    }
    save("rl-a5-snapshot.json", &values);
}

const CRASH_DIR: &str = "RADEON_CRASH_DIR";

/// A setting with no dependencies that is quick to see change.
fn crash_target(settings: &[Setting]) -> (&'static str, Value) {
    let wanted = std::env::var("RADEON_CRASH_TARGET").unwrap_or("morphological_aa.enabled".into());
    let setting = settings
        .iter()
        .find(|s| s.id == wanted && s.state == State::Configurable)
        .expect("a configurable crash target");
    (setting.id, alternative(setting).expect("a different value"))
}

/// Child process 1: change a setting through the journal, then die without restoring.
#[test]
#[ignore = "child of crash_then_recover"]
fn crash_child() {
    let Ok(dir) = std::env::var(CRASH_DIR) else {
        return;
    };
    let gpu = first_gpu();
    let settings = helper().read(gpu).expect("read");
    let (id, value) = crash_target(&settings);
    let mut controller = Controller::new(helper(), Path::new(&dir).join(JOURNAL_FILE));
    controller
        .apply(
            "crashed",
            gpu,
            &[Change {
                id: id.into(),
                value,
            }],
        )
        .expect("apply");
    println!("child wrote {id} and now dies without restoring");
    if std::env::var_os("RADEON_CRASH_EXIT").is_some() {
        std::process::exit(3);
    }
    std::process::abort();
}

/// Child process 2: a fresh host start, running the startup recovery with the helper-process
/// backend, with nothing between the crash and this. The crash left the setting changed, so the
/// recovery must have written the original back, not found it already there.
#[test]
#[ignore = "child of crash_then_recover"]
fn recover_child() {
    use super::control::RestoreResult;
    let Ok(dir) = std::env::var(CRASH_DIR) else {
        return;
    };
    let mut controller = Controller::new(helper(), Path::new(&dir).join(JOURNAL_FILE));
    let report = controller.recover_on_startup().expect("recover");
    println!("recovery: {:?}", report.outcomes);
    assert!(report.is_clean(), "recovery failed: {:?}", report.outcomes);
    assert_eq!(report.outcomes.len(), 1, "{:?}", report.outcomes);
    assert_eq!(
        report.outcomes[0].result,
        RestoreResult::Restored,
        "the crash left the setting changed, so recovery must have written it back"
    );
}

/// What a fresh process reads for a setting.
fn fresh_value(gpu: i32, id: &str) -> String {
    helper()
        .read_value(gpu, id)
        .map_or_else(|e| format!("unreadable: {e}"), |v| v.display())
}

fn run_child(test: &str, dir: &Path, exit: bool) -> std::process::ExitStatus {
    let mut child = Command::new(std::env::current_exe().expect("test binary"));
    child
        .args([
            "--exact",
            &format!("radeon::hardware::{test}"),
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CRASH_DIR, dir);
    if exit {
        child.env("RADEON_CRASH_EXIT", "1");
    } else {
        child.env_remove("RADEON_CRASH_EXIT");
    }
    child.status().expect("spawn")
}

/// Write through the journal, kill the process before it restores, and start recovery in a new
/// one at once. `exit` ends the child with `exit(3)` (no destructors); otherwise it `abort()`s,
/// which Windows treats as a fast-fail crash. Returns what a fresh process read the moment after
/// the death and the moment after the recovery.
///
/// The recovery is deliberately not preceded by a check or a wait: it runs in the window where
/// the dead child's write is the newest thing the driver has seen, which is where a recovery that
/// trusted a stale read would wrongly conclude that nothing needs undoing.
fn crash_round(exit: bool) -> (String, Value, String) {
    let gpu = first_gpu();
    let start = helper().read(gpu).expect("read");
    let _restore = Restore::new(gpu, &start);
    snapshot(&start);
    let dir = scratch_dir(if exit { "crash-exit" } else { "crash-abort" });
    let (id, tried) = crash_target(&start);
    let original = start
        .iter()
        .find(|s| s.id == id)
        .and_then(|s| s.value.clone())
        .expect("value");
    let how = if exit { "exit(3)" } else { "abort()" };

    let status = run_child("crash_child", &dir, exit);
    assert!(!status.success(), "the child should have died: {status}");
    let journal = Controller::new(helper(), dir.join(JOURNAL_FILE))
        .journal()
        .expect("journal");
    assert_eq!(journal.entries.len(), 1, "journal: {:?}", journal.entries);
    assert_eq!(journal.entries[0].original, original);
    assert_eq!(journal.entries[0].written, tried);

    // Straight into the recovery, then look: it must have put the original back.
    let waited = std::time::Instant::now();
    let status = run_child("recover_child", &dir, exit);
    assert!(status.success(), "recovery child failed: {status}");
    let now = fresh_value(gpu, id);
    println!(
        "{id}: a new process reads {now} after the recovery ({how}, recovery child took {:?})",
        waited.elapsed()
    );
    assert_eq!(
        now,
        original.display(),
        "{id} should be back to {}",
        original.display()
    );
    // And it stays there: the dead child's write must not land after the recovery.
    std::thread::sleep(Duration::from_secs(5));
    let later = fresh_value(gpu, id);
    assert_eq!(
        later,
        original.display(),
        "{id} changed again after the recovery"
    );
    (id.to_string(), tried, now)
}

/// The host dies after a journaled write. A fresh start restores the original.
#[test]
#[ignore = "changes real driver settings"]
fn crash_then_recover() {
    let _exclusive = exclusive();
    assert_host_idle();
    let (id, tried, held) = crash_round(true);
    println!(
        "exit(3) crash: {id} -> {} (child killed) -> {held} (recovered in a fresh process)",
        tried.display()
    );
    assert_host_idle();
    let (id, tried, held) = crash_round(false);
    println!(
        "abort() crash: {id} -> {} (child killed) -> {held} (recovered in a fresh process)",
        tried.display()
    );
}

/// Where the other account's peer and the test exchange requests (`RADEON_XACCT_DIR`).
fn xacct_dir() -> PathBuf {
    std::env::var("RADEON_XACCT_DIR").map_or_else(|_| PathBuf::from(r"C:\src\rl-xacct"), Into::into)
}

/// Who this process is and in which Windows session, as `whoami` and the session id.
fn identity() -> String {
    // Windows' own whoami, not one a PATH with Git's usr\bin finds first.
    let windir = std::env::var_os("WINDIR").unwrap_or_else(|| r"C:\Windows".into());
    let who = Command::new(PathBuf::from(windir).join(r"System32\whoami.exe"))
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|e| format!("unknown ({e})"));
    let session = crate::process_info::current_session().map_or("?".into(), |s| s.to_string());
    format!("{who} (session {session})")
}

/// Write a file other processes poll for, so they never see half of it.
fn publish(path: &Path, body: &[u8]) {
    let partial = path.with_extension("part");
    std::fs::write(&partial, body).expect("write exchange file");
    std::fs::rename(&partial, path).expect("publish exchange file");
}

/// The installed host runs as SYSTEM in the console session; a development host runs as the
/// signed-in user. Started under the other account (see `docs/hardware-matrix.md`, "Which
/// account"), this answers helper requests that `RemoteBackend` leaves in the exchange folder,
/// by running each in a fresh helper process of its own account, until a `stop` file appears.
#[test]
#[ignore = "child of the cross-account test"]
fn cross_account_server() {
    use std::time::Instant;
    let dir = xacct_dir();
    let mut backend = helper();
    publish(&dir.join("identity.txt"), identity().as_bytes());
    let started = Instant::now();
    while !dir.join("stop").exists() && started.elapsed() < Duration::from_secs(60 * 60) {
        let next = std::fs::read_dir(&dir)
            .expect("exchange folder")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.extension().is_some_and(|e| e == "json")
                    && path
                        .file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with("req-"))
            });
        let Some(path) = next else {
            std::thread::sleep(Duration::from_millis(20));
            continue;
        };
        let body = std::fs::read_to_string(&path).unwrap_or_default();
        let name = path
            .file_name()
            .expect("name")
            .to_string_lossy()
            .replace("req-", "resp-");
        let _ = std::fs::remove_file(&path);
        let response = super::helper::handle(&body, &mut backend);
        publish(
            &dir.join(name),
            serde_json::to_string(&response).expect("json").as_bytes(),
        );
    }
}

mod cross_account_tests;
mod round_trip_tests;
