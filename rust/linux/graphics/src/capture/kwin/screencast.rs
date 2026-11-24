//! A KWin screencast of one output, asked for through KWin's own protocol
//! `zkde_screencast_unstable_v1` instead of the desktop portal, so no dialog appears. A thread
//! owns the Wayland connection and keeps dispatching, because KWin ends a stream whose client
//! goes away; it reports the end through [`KwinStream::ended`]. The stream flow (find the
//! `wl_output` by its v4 name, `stream_output`, wait for `created`) follows `kwingrab.cpp` of
//! Vibepollo (Nonary/Vibepollo, GPL-3.0), which carries the KWin capture of Sunshine and Apollo.

use super::permission::{self, Outcome};
use anyhow::{Context, Result, anyhow, bail};
use std::{
    os::fd::AsRawFd,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::Duration,
};
use wayland_client::{
    Connection, Dispatch, QueueHandle,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_output, wl_registry},
};
use wayland_protocols_plasma::screencast::v1::client::{
    zkde_screencast_stream_unstable_v1::{self as stream_v1, ZkdeScreencastStreamUnstableV1},
    zkde_screencast_unstable_v1::ZkdeScreencastUnstableV1,
};

/// How long KWin has to say the stream exists.
const CREATED_WAIT: Duration = Duration::from_secs(5);
/// How long KWin has to read a permission file this host just wrote.
const PERMISSION_WAIT: Duration = Duration::from_secs(3);
/// How often the dispatch thread looks at the stop flag.
const POLL_MS: i32 = 100;
/// The `pointer` argument: the cursor drawn into the picture, or left out.
const POINTER_HIDDEN: u32 = 1;
const POINTER_EMBEDDED: u32 = 2;

/// What the thread and its owner share.
struct Shared {
    stop: AtomicBool,
    /// Why the stream is over, once it is.
    ended: Mutex<Option<String>>,
}

impl Shared {
    fn end(&self, reason: impl Into<String>) {
        self.ended.lock().unwrap().get_or_insert(reason.into());
    }
}

/// An open KWin screencast of one output. Dropping it closes the stream.
pub struct KwinStream {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl KwinStream {
    /// Asks KWin for a stream of the output named `output_name`, with the cursor drawn in when
    /// `pointer` is set. Returns the stream and the PipeWire node of the user's own PipeWire
    /// daemon that carries it. Fails, saying what KWin did, within a few seconds.
    pub fn open(output_name: &str, pointer: bool) -> Result<(Self, u32)> {
        let permission = permission::ensure_for_current_exe();
        if let Ok(Outcome::Written(_)) = &permission {
            tracing::info!("waiting for KWin to read the new permission file");
            std::thread::sleep(PERMISSION_WAIT);
        }
        let situation = match &permission {
            Ok(outcome) => outcome.describe(),
            Err(error) => format!("the permission file could not be made: {error:#}"),
        };
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            ended: Mutex::new(None),
        });
        let (tx, rx) = mpsc::channel();
        let (name, thread_shared) = (output_name.to_owned(), shared.clone());
        let thread = std::thread::Builder::new()
            .name("kwin-screencast".into())
            .spawn(move || run(&name, pointer, &situation, &thread_shared, &tx))
            .context("starting the KWin screencast thread")?;
        match rx.recv_timeout(CREATED_WAIT) {
            Ok(Ok(node)) => Ok((
                Self {
                    shared,
                    thread: Some(thread),
                },
                node,
            )),
            Ok(Err(reason)) => {
                let _ = thread.join();
                Err(anyhow!(reason))
            }
            Err(_) => {
                // The thread may sit in a call that never returns: leave it to stop alone.
                shared.stop.store(true, Ordering::Relaxed);
                bail!("KWin did not answer the screencast request within {CREATED_WAIT:?}")
            }
        }
    }

    /// Why KWin or the connection ended the stream, once it has.
    pub fn ended(&self) -> Option<String> {
        self.shared.ended.lock().unwrap().clone()
    }
}

impl Drop for KwinStream {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// One `wl_output` and the name it announced.
struct Output {
    proxy: wl_output::WlOutput,
    name: Option<String>,
}

/// The dispatch state of the thread.
#[derive(Default)]
struct State {
    outputs: Vec<Output>,
    node: Option<u32>,
    failed: Option<String>,
    closed: bool,
}

/// The index of the output named `wanted` among the names the outputs announced, or an error
/// that lists the names seen.
fn find_output(names: &[Option<String>], wanted: &str) -> Result<usize> {
    names
        .iter()
        .position(|name| name.as_deref() == Some(wanted))
        .ok_or_else(|| {
            let seen: Vec<&str> = names
                .iter()
                .map(|name| {
                    name.as_deref()
                        .unwrap_or("(no name: wl_output older than v4)")
                })
                .collect();
            anyhow!("KWin has no output named {wanted:?}; it lists {seen:?}")
        })
}

/// The error for a compositor that does not offer the screencast protocol to this program.
fn no_protocol(situation: &str) -> String {
    format!("KWin does not offer its screencast to this program: {situation} (or this is not KWin)")
}

/// The thread: sets the stream up, reports the node (or the error) once, then dispatches until
/// the owner drops the stream or KWin ends it.
fn run(
    wanted: &str,
    pointer: bool,
    situation: &str,
    shared: &Shared,
    tx: &mpsc::Sender<std::result::Result<u32, String>>,
) {
    match serve(wanted, pointer, situation, shared, tx) {
        Ok(()) => {}
        Err(error) => {
            let reason = format!("{error:#}");
            shared.end(reason.clone());
            let _ = tx.send(Err(reason));
        }
    }
}

fn serve(
    wanted: &str,
    pointer: bool,
    situation: &str,
    shared: &Shared,
    tx: &mpsc::Sender<std::result::Result<u32, String>>,
) -> Result<()> {
    let conn = Connection::connect_to_env().context("connecting to the Wayland display")?;
    let (globals, mut queue) =
        registry_queue_init::<State>(&conn).context("reading the Wayland registry")?;
    let qh = queue.handle();
    let mut state = State::default();
    let screencast: ZkdeScreencastUnstableV1 = globals
        .bind(&qh, 1..=1, ())
        .map_err(|_| anyhow!(no_protocol(situation)))?;
    for global in globals.contents().clone_list() {
        if global.interface == "wl_output" {
            let proxy = globals.registry().bind::<wl_output::WlOutput, _, _>(
                global.name,
                global.version.min(4),
                &qh,
                (),
            );
            state.outputs.push(Output { proxy, name: None });
        }
    }
    // The second round trip brings the outputs' names.
    queue.roundtrip(&mut state)?;
    queue.roundtrip(&mut state)?;
    let names: Vec<Option<String>> = state.outputs.iter().map(|o| o.name.clone()).collect();
    let output = &state.outputs[find_output(&names, wanted)?].proxy;
    let pointer = if pointer {
        POINTER_EMBEDDED
    } else {
        POINTER_HIDDEN
    };
    let stream = screencast.stream_output(output, pointer, &qh, ());
    conn.flush()?;
    let mut announced = false;
    while !shared.stop.load(Ordering::Relaxed) {
        queue.dispatch_pending(&mut state)?;
        if let Some(error) = state.failed.take() {
            if announced {
                shared.end(format!("KWin ended the screencast: {error}"));
            } else {
                bail!("KWin refused the screencast of {wanted:?}: {error}");
            }
            break;
        }
        if state.closed {
            shared.end("KWin closed the screencast");
            break;
        }
        if let (Some(node), false) = (state.node, announced) {
            announced = true;
            tracing::info!(node, output = wanted, "KWin screencast created");
            let _ = tx.send(Ok(node));
        }
        conn.flush()?;
        let Some(guard) = conn.prepare_read() else {
            continue;
        };
        let mut poll = libc::pollfd {
            fd: guard.connection_fd().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `poll` points to one valid pollfd for the call, and the descriptor stays
        // open while `guard` holds the connection.
        let ready = unsafe { libc::poll(&mut poll, 1, POLL_MS) };
        if ready > 0 {
            guard.read().context("reading from KWin")?;
        }
    }
    stream.close();
    let _ = conn.flush();
    if !announced {
        bail!("{}", no_stream(&state));
    }
    Ok(())
}

/// What to say when the loop ended before `created`.
fn no_stream(state: &State) -> String {
    if state.closed {
        "KWin closed the screencast before it started".into()
    } else {
        "the screencast stopped before KWin created it".into()
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_output::WlOutput, ()> for State {
    fn event(
        state: &mut Self,
        output: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event
            && let Some(entry) = state.outputs.iter_mut().find(|o| &o.proxy == output)
        {
            entry.name = Some(name);
        }
    }
}

impl Dispatch<ZkdeScreencastUnstableV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZkdeScreencastUnstableV1,
        _: <ZkdeScreencastUnstableV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZkdeScreencastStreamUnstableV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZkdeScreencastStreamUnstableV1,
        event: stream_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            stream_v1::Event::Created { node } => state.node = Some(node),
            stream_v1::Event::Failed { error } => state.failed = Some(error),
            stream_v1::Event::Closed => state.closed = true,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[Option<&str>]) -> Vec<Option<String>> {
        list.iter().map(|n| n.map(str::to_owned)).collect()
    }

    #[test]
    fn the_output_is_matched_by_exact_name() {
        let seen = names(&[Some("DP-1"), Some("Virtual-1")]);
        assert_eq!(find_output(&seen, "Virtual-1").unwrap(), 1);
        assert!(find_output(&seen, "Virtual").is_err());
    }

    #[test]
    fn a_missing_output_lists_what_was_seen() {
        let error = find_output(&names(&[Some("DP-1"), None]), "HDMI-A-1")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("HDMI-A-1") && error.contains("DP-1") && error.contains("older than v4")
        );
        assert!(find_output(&[], "x").is_err());
    }

    #[test]
    fn the_missing_protocol_names_the_permission_file_situation() {
        let text = no_protocol(&Outcome::Written("/h/a.desktop".into()).describe());
        assert!(text.starts_with("KWin does not offer its screencast to this program: "));
        assert!(
            text.contains("/h/a.desktop") && text.contains("KWIN_WAYLAND_NO_PERMISSION_CHECKS")
        );
    }
}
