//! Unit tests for display arrangement layouts, wake watches, and recall repair.

use super::*;
use butterpollo_media_policy::topology::{Kind, Mode, Position};
use std::sync::Arc;

mod left_watch;
mod ownership_and_arrivals;
mod recall_and_repair;
mod wake_follow;
mod wake_watch;

fn node(id: &str, active: bool, primary: bool) -> Node {
    Node {
        id: id.into(),
        label: id.into(),
        kind: Kind::Physical,
        active,
        primary,
        desired_position: Position { x: 0, y: 0 },
        mode: Mode {
            width: 1920,
            height: 1080,
            refresh_hz: 60.,
        },
        device_id: id.into(),
    }
}
fn snapshot(nodes: Vec<Node>) -> Snapshot {
    Snapshot {
        version: 1,
        hdr: nodes.iter().map(|n| (n.device_id.clone(), true)).collect(),
        scale: nodes.iter().map(|n| (n.device_id.clone(), 125)).collect(),
        rotation: Default::default(),
        sources: Default::default(),
        clone_groups: vec![],
        nodes,
    }
}

fn listed(monitor: bool) -> Vec<String> {
    let mut ids = vec!["a".to_owned(), "tv".to_owned()];
    if monitor {
        ids.push("monitor".to_owned());
    }
    ids
}

/// Tests that lease or lock the arrangement's own state, which is the process's.
static SHARED_STATE: Mutex<()> = Mutex::new(());
/// Says so when dropped: a stream's display in a test.
struct FakeDisplay(Arc<std::sync::atomic::AtomicBool>);
impl FakeDisplay {
    fn new() -> (Self, Arc<std::sync::atomic::AtomicBool>) {
        let gone = Arc::new(std::sync::atomic::AtomicBool::new(false));
        (Self(gone.clone()), gone)
    }
}
impl Drop for FakeDisplay {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
fn gone(flag: &std::sync::atomic::AtomicBool) -> bool {
    flag.load(Ordering::SeqCst)
}
fn wake_end(first: Result<()>, restored: Result<()>) -> WakeEnd {
    WakeEnd {
        woken: vec!["monitor".to_owned()],
        restore: true,
        first,
        restored,
    }
}
/// What a watch wrote, in order.
type Writes = Arc<Mutex<Vec<&'static str>>>;
/// [`run_wake_watch`] with its writes noted and displays as `read` says.
fn noted_watch(
    end: WakeEnd,
    stop: &WatchStop,
    writes: &Writes,
    read: impl FnMut() -> Result<Vec<String>>,
) -> bool {
    let note = |what: &'static str| {
        let writes = writes.clone();
        move || {
            writes.lock().unwrap().push(what);
            Ok(())
        }
    };
    let forget = note("journal");
    run_wake_watch(
        end,
        stop,
        read,
        note("strays"),
        note("entry"),
        note("layout"),
        forget,
    )
}
/// A slow watch on a monitor that is there, 3 s from anything happening, on `watcher`.
fn start_slow_watch(
    watcher: &WakeWatcher,
    writes: &Writes,
    display: FakeDisplay,
    restored: Result<()>,
) -> bool {
    let writes = writes.clone();
    let start = watcher.start(watched_left(), move |stop| {
        let left = noted_watch(wake_end(Ok(()), restored), stop, &writes, || {
            Ok(listed(true))
        });
        drop(display);
        left
    });
    start == WatchStart::Started
}
/// What a watch of [`wake_end`] leaves if it doesn't finish.
fn watched_left() -> LeftWatch {
    LeftWatch {
        woken: vec!["monitor".to_owned()],
        restore: true,
    }
}
/// Far under the watch, whose shortest run is `WAKE_WATCH`, and far over a stop's own
/// cost when the machine is busy building.
const STOP_BOUND: std::time::Duration = std::time::Duration::from_secs(1);

fn at_hz(id: &str, active: bool, primary: bool, refresh_hz: f64) -> Node {
    let mut node = node(id, active, primary);
    node.mode.refresh_hz = refresh_hz;
    node
}

/// Streams holding `arrangement`: the monitor on before them, `applied` now.
fn holding(arrangement: Arrangement, applied: Vec<Node>, persisted: bool) -> State {
    let mut state = State {
        users: 1,
        arrangement: Some(arrangement),
        before: Some(snapshot(vec![node("monitor", true, true)])),
        applied,
        persisted,
        ..Default::default()
    };
    add_owner(&mut state, "r1", persisted);
    state
}
