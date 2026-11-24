//! The user's Wayland desktop clipboard through wl-clipboard: `wl-paste` reads, `wl-copy`
//! writes, and one `wl-paste --watch` counts the selection changes. wl-clipboard speaks
//! `ext-data-control-v1` and `wlr-data-control-unstable-v1`, which KWin, wlroots compositors
//! and recent Mutter-less setups offer; where neither is, or the programs are not installed,
//! every call fails saying so rather than going quiet.

use anyhow::{Context, Result, bail};
use std::{
    io::{Read, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

use super::{Backend, pick_text};

/// How long a `wl-paste` or `wl-copy` may take.
const TIMEOUT: Duration = Duration::from_secs(5);
/// How long a write waits for its own change to show in the count.
const SETTLE: Duration = Duration::from_millis(300);

/// The Wayland socket of the user's desktop: `WAYLAND_DISPLAY`, else the first `wayland-N`
/// socket in the runtime folder (a service started outside the session).
pub(super) fn desktop_socket() -> Option<String> {
    if let Some(name) = std::env::var("WAYLAND_DISPLAY")
        .ok()
        .filter(|name| !name.is_empty())
    {
        return Some(name);
    }
    let folder = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .ok()?
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| {
            name.strip_prefix("wayland-")
                .is_some_and(|n| n.parse::<u32>().is_ok())
        })
        .collect();
    names.sort();
    names.into_iter().next()
}

pub(super) struct Clipboard {
    socket: String,
    changes: Arc<AtomicU32>,
    watcher: Mutex<Option<Child>>,
}

impl Clipboard {
    pub(super) fn new(socket: &str) -> Result<Self> {
        let this = Self {
            socket: socket.to_owned(),
            changes: Arc::new(AtomicU32::new(0)),
            watcher: Mutex::new(None),
        };
        // Fails now, visibly, without wl-clipboard or a desktop that offers a data-control
        // protocol.
        this.types().context(
            "the Wayland clipboard needs wl-clipboard (wl-paste, wl-copy) and a desktop with \
             wlr-data-control or ext-data-control",
        )?;
        this.watch();
        Ok(this)
    }

    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command.env("WAYLAND_DISPLAY", &self.socket);
        command
    }

    /// Starts the watch if it is not running: `wl-paste --watch echo` prints a line for the
    /// current selection and for each change after it.
    fn watch(&self) {
        let mut slot = self.watcher.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(child) = slot.as_mut()
            && child.try_wait().ok().flatten().is_none()
        {
            return;
        }
        let spawned = self
            .command("wl-paste")
            .args(["--watch", "echo"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        match spawned {
            Ok(mut child) => {
                if let Some(mut out) = child.stdout.take() {
                    let changes = self.changes.clone();
                    std::thread::spawn(move || {
                        let mut buffer = [0u8; 256];
                        while let Ok(n) = out.read(&mut buffer) {
                            if n == 0 {
                                break;
                            }
                            let lines = buffer[..n].iter().filter(|b| **b == b'\n').count();
                            changes.fetch_add(lines as u32, Ordering::Relaxed);
                        }
                    });
                }
                *slot = Some(child);
            }
            Err(error) => tracing::warn!("the Wayland clipboard cannot be watched: {error}"),
        }
    }

    /// The formats the selection offers.
    fn types(&self) -> Result<Vec<String>> {
        let output = run(self.command("wl-paste").arg("--list-types"), 64 * 1024)?;
        if !output.0 {
            // "Nothing is copied" is an empty clipboard, not a failure.
            let message = String::from_utf8_lossy(&output.1);
            if message.contains("Nothing is copied") {
                return Ok(Vec::new());
            }
            bail!("wl-paste failed: {}", message.trim());
        }
        Ok(String::from_utf8_lossy(&output.1)
            .lines()
            .map(str::to_owned)
            .collect())
    }

    fn paste(&self, mime: &str, max: usize) -> Result<Vec<u8>> {
        let (ok, bytes, err) = run_split(
            self.command("wl-paste")
                .args(["--no-newline", "--type", mime]),
            max,
        )?;
        if !ok {
            bail!("wl-paste failed: {}", err.trim());
        }
        Ok(bytes)
    }

    /// Runs `wl-copy` for `mime` with `data`, then waits for the count to move.
    fn copy(&self, mime: &str, data: &[u8]) -> Result<()> {
        let before = self.changes.load(Ordering::Relaxed);
        let mut child = self
            .command("wl-copy")
            .args(["--type", mime])
            .stdin(Stdio::piped())
            // wl-copy's forked server keeps these open for as long as it holds the selection.
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("starting wl-copy (is wl-clipboard installed?)")?;
        let mut stdin = child.stdin.take().context("wl-copy has no stdin")?;
        let data = data.to_vec();
        let writer = std::thread::spawn(move || stdin.write_all(&data));
        let status = wait(&mut child, TIMEOUT)?;
        let _ = writer.join();
        if !status.success() {
            bail!("wl-copy failed ({status})");
        }
        let until = Instant::now() + SETTLE;
        while self.changes.load(Ordering::Relaxed) == before && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }
}

/// Waits for `child` up to `timeout`, killing it after.
fn wait(child: &mut Child, timeout: Duration) -> Result<std::process::ExitStatus> {
    let until = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= until {
            let _ = child.kill();
            let _ = child.wait();
            bail!("the clipboard program did not finish within {timeout:?}");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Runs `command`; whether it succeeded and its stdout, which fails over `max` bytes.
fn run(command: &mut Command, max: usize) -> Result<(bool, Vec<u8>)> {
    let (ok, out, err) = run_split(command, max)?;
    Ok((ok, if ok { out } else { err.into_bytes() }))
}

fn run_split(command: &mut Command, max: usize) -> Result<(bool, Vec<u8>, String)> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "starting {} (is wl-clipboard installed?)",
                command.get_program().to_string_lossy()
            )
        })?;
    let mut out = child.stdout.take().context("no stdout")?;
    let mut err = child.stderr.take().context("no stderr")?;
    let stdout = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = (&mut out).take(max as u64 + 1).read_to_end(&mut bytes);
        // Past the limit the program is killed rather than read on.
        bytes
    });
    let stderr = std::thread::spawn(move || {
        let mut text = Vec::new();
        let _ = (&mut err).take(4096).read_to_end(&mut text);
        String::from_utf8_lossy(&text).into_owned()
    });
    let status = wait(&mut child, TIMEOUT);
    let bytes = stdout.join().unwrap_or_default();
    let text = stderr.join().unwrap_or_default();
    if bytes.len() > max {
        bail!("the clipboard holds more than {max} bytes");
    }
    Ok((status?.success(), bytes, text))
}

impl Backend for Clipboard {
    fn sequence(&self) -> u32 {
        self.watch();
        self.changes.load(Ordering::Relaxed)
    }

    fn text(&self, max: usize) -> Result<Option<String>> {
        let types = self.types()?;
        let Some(mime) = pick_text(&types, &["STRING", "TEXT"]) else {
            return Ok(None);
        };
        let bytes = self.paste(mime, max)?;
        Ok(Some(
            String::from_utf8(bytes).context("the clipboard's text is not UTF-8")?,
        ))
    }

    fn png(&self, max: usize) -> Result<Option<Vec<u8>>> {
        if !self.types()?.iter().any(|t| t == "image/png") {
            return Ok(None);
        }
        self.paste("image/png", max).map(Some)
    }

    fn set_text(&self, text: &str) -> Result<()> {
        self.copy("text/plain;charset=utf-8", text.as_bytes())
    }

    fn set_png(&self, png: &[u8]) -> Result<()> {
        self.copy("image/png", png)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_over_the_limit_fails() {
        let error = run_split(Command::new("sh").args(["-c", "printf aaaaaaaaaa"]), 4).unwrap_err();
        assert!(format!("{error:#}").contains("more than 4"));
        let (ok, out, _) = run_split(Command::new("sh").args(["-c", "printf aaaa"]), 4).unwrap();
        assert!(ok);
        assert_eq!(out, b"aaaa");
    }

    #[test]
    fn a_missing_program_says_so() {
        let error = run(&mut Command::new("rubylight-no-such-program"), 4).unwrap_err();
        assert!(format!("{error:#}").contains("wl-clipboard"));
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// Sets and reads the desktop's text once; the user's text is put back afterwards. Skipped
    /// (passes) when the clipboard holds something that is not text, which could not be
    /// restored.
    #[test]
    #[ignore = "changes the desktop clipboard"]
    fn desktop_text_round_trip() {
        let socket = desktop_socket().expect("no Wayland desktop");
        let clipboard = Clipboard::new(&socket).unwrap();
        let types = clipboard.types().unwrap();
        let previous = if types.is_empty() {
            None
        } else if pick_text(&types, &[]).is_some() && !types.iter().any(|t| t.starts_with("image/"))
        {
            clipboard.text(1 << 20).unwrap()
        } else {
            eprintln!("the clipboard holds {types:?}; not touched");
            return;
        };
        let before = clipboard.sequence();
        clipboard.set_text("rubylight clipboard test é").unwrap();
        assert!(
            clipboard.sequence() != before,
            "the change count did not move"
        );
        assert_eq!(
            clipboard.text(1 << 20).unwrap().as_deref(),
            Some("rubylight clipboard test é")
        );
        if let Some(previous) = previous {
            clipboard.set_text(&previous).unwrap();
        }
    }
}
