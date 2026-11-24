//! The game's display on Linux: a headless gamescope session at the stream's size and rate,
//! which the game runs inside. gamescope offers its picture on PipeWire, where the capture
//! takes it, and runs an X server of its own, where the keyboard and mouse go.
//!
//! [`wrap`] turns an app's command into one that starts gamescope with the app as its child,
//! so the session lasts exactly as long as the game. The session writes what the host needs
//! into a folder of its own: its children's environment (the X and Wayland displays),
//! gamescope's process id, and gamescope's log, which names its PipeWire node.
//! [`current`] reads the newest session that still runs.
use anyhow::{Context, Result, bail};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

/// The program started for a session; `RUBYLIGHT_GAMESCOPE` names another one.
const PROGRAM: &str = "gamescope";
/// The size and rate when the app's environment names none.
const DEFAULT_MODE: (u32, u32, u32) = (1920, 1080, 60);

/// A running gamescope session, as its folder records it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub folder: PathBuf,
    /// gamescope's process.
    pub pid: u32,
    /// The X display its games use (`DISPLAY`), where keyboard and mouse input goes.
    pub display: Option<String>,
    /// Its Wayland display (`GAMESCOPE_WAYLAND_DISPLAY`).
    pub wayland_display: Option<String>,
    /// The PipeWire node gamescope offers its picture on, once it has said so.
    pub node: Option<u32>,
}

impl Session {
    /// The capture target of this session's picture: `pipewire-node:<id>`.
    pub fn capture_target(&self) -> Option<String> {
        self.node.map(capture_target)
    }
}

/// The capture target that names PipeWire node `node`.
pub fn capture_target(node: u32) -> String {
    format!("pipewire-node:{node}")
}

/// The PipeWire node a capture target names.
pub fn node_of(target: &str) -> Option<u32> {
    target.strip_prefix("pipewire-node:")?.parse().ok()
}

/// Where sessions keep their folders: the user's runtime folder, else the temporary one.
fn root() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(std::env::temp_dir)
        .join("rubylight-gamescope")
}

/// The stream's size and rate from the app's environment (`SUNSHINE_CLIENT_WIDTH`,
/// `SUNSHINE_CLIENT_HEIGHT`, `SUNSHINE_CLIENT_FPS`); a fractional rate is rounded.
pub fn mode(environment: &BTreeMap<String, String>) -> (u32, u32, u32) {
    let read = |key: &str| environment.get(key).map(|value| value.trim());
    let width = read("SUNSHINE_CLIENT_WIDTH").and_then(|v| v.parse().ok());
    let height = read("SUNSHINE_CLIENT_HEIGHT").and_then(|v| v.parse().ok());
    let fps = read("SUNSHINE_CLIENT_FPS")
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|fps| fps.is_finite() && *fps >= 1.0)
        .map(|fps| fps.round() as u32);
    match (width, height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => (w, h, fps.unwrap_or(DEFAULT_MODE.2)),
        _ => (
            DEFAULT_MODE.0,
            DEFAULT_MODE.1,
            fps.unwrap_or(DEFAULT_MODE.2),
        ),
    }
}

/// `text` as one word for `/bin/sh`, in single quotes.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// The shell command that runs `command` inside a new headless gamescope session at the
/// stream's mode, and the session's folder. The folder is made here; the session fills it
/// once gamescope has started its first child.
pub fn wrap(command: &str, environment: &BTreeMap<String, String>) -> Result<(String, PathBuf)> {
    let program = std::env::var("RUBYLIGHT_GAMESCOPE").unwrap_or_else(|_| PROGRAM.into());
    // Without gamescope the wrapped command would exit at once and the launch would look like
    // a game that quit: say so before anything starts.
    if !installed(&program) {
        bail!(
            "gamescope is not installed: a Linux host runs each game inside a gamescope session of its own. Install gamescope, or use an app without a command to stream the desktop."
        );
    }
    wrap_in(&root(), &program, command, environment)
}

/// Whether `program` can be started: a path that exists, or a name on `PATH`.
fn installed(program: &str) -> bool {
    if program.contains('/') {
        return Path::new(program).is_file();
    }
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|folder| folder.join(program).is_file())
    })
}

/// [`wrap`] with the sessions' folder and the program to start.
fn wrap_in(
    root: &Path,
    program: &str,
    command: &str,
    environment: &BTreeMap<String, String>,
) -> Result<(String, PathBuf)> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let (width, height, fps) = mode(environment);
    std::fs::create_dir_all(root).with_context(|| format!("creating {}", root.display()))?;
    sweep(root);
    let folder = root.join(format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&folder).with_context(|| format!("creating {}", folder.display()))?;
    let folder_text = folder
        .to_str()
        .context("the session folder's path is not text")?;
    // gamescope's child records gamescope's pid ($PPID) and the environment gamescope gave
    // it, then becomes the app's own shell. The app's command travels as an argument, so no
    // quoting of it is nested inside another.
    let record = r#"echo "$PPID" > "$0/pid.tmp" && mv "$0/pid.tmp" "$0/pid"; env > "$0/env.tmp" && mv "$0/env.tmp" "$0/env"; exec /bin/sh -c "$1""#;
    let wrapped = format!(
        "exec {program} --backend headless -W {width} -H {height} -w {width} -h {height} -r {fps} -- /bin/sh -c {record} {folder} {command} 2>>{log}",
        program = quote(program),
        record = quote(record),
        folder = quote(folder_text),
        command = quote(command),
        log = quote(&format!("{folder_text}/gamescope.log")),
    );
    Ok((wrapped, folder))
}

/// The session recorded in `folder`, if gamescope still runs there.
pub fn read(folder: &Path) -> Option<Session> {
    let pid: u32 = std::fs::read_to_string(folder.join("pid"))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    if !running(pid) {
        return None;
    }
    let environment = std::fs::read_to_string(folder.join("env")).unwrap_or_default();
    let variable = |name: &str| {
        environment
            .lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
            .map(str::to_owned)
    };
    let log = std::fs::read_to_string(folder.join("gamescope.log")).unwrap_or_default();
    Some(Session {
        folder: folder.to_path_buf(),
        pid,
        display: variable("DISPLAY"),
        wayland_display: variable("GAMESCOPE_WAYLAND_DISPLAY"),
        node: node_in_log(&log),
    })
}

/// The newest session this process started that still runs. Sessions other programs or
/// another host started are never chosen, so a second gamescope on the PC cannot take the
/// stream's picture or input.
pub fn current() -> Option<Session> {
    current_in(&root())
}

/// [`current`] among the sessions in `root`.
fn current_in(root: &Path) -> Option<Session> {
    let own = format!("{}-", std::process::id());
    let mut folders: Vec<(u64, PathBuf)> = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let number = name.to_str()?.strip_prefix(&own)?.parse().ok()?;
            Some((number, entry.path()))
        })
        .collect();
    folders.sort_by_key(|folder| std::cmp::Reverse(folder.0));
    folders.iter().find_map(|(_, folder)| read(folder))
}

/// Waits up to `timeout` for a session of this process ([`current`]) to run and name its
/// PipeWire node.
pub fn wait_for_node(timeout: std::time::Duration) -> Result<Session> {
    wait_for_node_in(&root(), timeout)
}

/// [`wait_for_node`] among the sessions in `root`.
fn wait_for_node_in(root: &Path, timeout: std::time::Duration) -> Result<Session> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(session) = current_in(root).filter(|session| session.node.is_some()) {
            return Ok(session);
        }
        if std::time::Instant::now() >= deadline {
            bail!("no gamescope session offered its picture within {timeout:?}");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// The node in gamescope's last "stream available on node ID: N" line.
fn node_in_log(log: &str) -> Option<u32> {
    log.lines()
        .rev()
        .find_map(|line| line.split("stream available on node ID:").nth(1))
        .and_then(|rest| rest.trim().parse().ok())
}

/// Whether process `pid` exists and has not ended.
fn running(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            let state = stat
                .rsplit_once(')')?
                .1
                .split_whitespace()
                .next()?
                .to_owned();
            Some(state != "Z" && state != "X")
        })
        .unwrap_or(false)
}

/// Removes the folders of sessions that ended, and of ones that never started within a
/// minute.
fn sweep(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let folder = entry.path();
        if read(&folder).is_some() {
            continue;
        }
        let young = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age < std::time::Duration::from_secs(60));
        if !folder.join("pid").exists() && young {
            continue;
        }
        let _ = std::fs::remove_dir_all(&folder);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_gamescope_is_found_before_anything_starts() {
        assert!(installed("sh"));
        assert!(installed("/bin/sh"));
        assert!(!installed("no-such-gamescope-here"));
        assert!(!installed("/nowhere/gamescope"));
    }

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn the_mode_comes_from_the_apps_environment() {
        let mode_of = |pairs: &[(&str, &str)]| mode(&env(pairs));
        assert_eq!(
            mode_of(&[
                ("SUNSHINE_CLIENT_WIDTH", "2560"),
                ("SUNSHINE_CLIENT_HEIGHT", "1440"),
                ("SUNSHINE_CLIENT_FPS", "119.88"),
            ]),
            (2560, 1440, 120)
        );
        assert_eq!(mode_of(&[]), DEFAULT_MODE);
        assert_eq!(
            mode_of(&[
                ("SUNSHINE_CLIENT_WIDTH", "0"),
                ("SUNSHINE_CLIENT_HEIGHT", "720")
            ]),
            DEFAULT_MODE
        );
    }

    #[test]
    fn a_node_is_read_from_the_last_line_that_names_one() {
        let log = "[gamescope] [Info]  pipewire: stream available on node ID: 95\nother\n\
                   [gamescope] [Info]  pipewire: stream available on node ID: 102\n";
        assert_eq!(node_in_log(log), Some(102));
        assert_eq!(node_in_log("nothing here"), None);
        assert_eq!(node_of(&capture_target(7)), Some(7));
        assert_eq!(node_of("gamescope"), None);
    }

    #[test]
    fn only_a_running_session_of_this_process_is_current() {
        let root = std::env::temp_dir().join(format!("gamescope-own-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // Every session names this test process as gamescope, which runs.
        let session = |name: &str, node: u32| {
            let folder = root.join(name);
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(folder.join("pid"), std::process::id().to_string()).unwrap();
            let line = format!("stream available on node ID: {node}\n");
            std::fs::write(folder.join("gamescope.log"), line).unwrap();
        };
        let own = std::process::id();
        session(&format!("{own}-1"), 11);
        session(&format!("{own}-2"), 12);
        // Another host's session, made last and with a higher number.
        session(&format!("{}-9", own.wrapping_add(1)), 99);
        session("not-a-session", 98);
        assert_eq!(current_in(&root).and_then(|s| s.node), Some(12));
        std::fs::write(root.join(format!("{own}-2/pid")), "0").unwrap();
        assert_eq!(
            current_in(&root).and_then(|s| s.node),
            Some(11),
            "an ended one is skipped"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_wrapped_command_records_its_session_and_runs_the_app() {
        let folder = std::env::temp_dir().join(format!("gamescope-wrap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        // A stand-in for gamescope: it logs a node and runs its command as a child of its
        // own, as gamescope does, so the session's process is this script.
        let fake = folder.join("fake-gamescope");
        std::fs::write(
            &fake,
            "#!/bin/sh\necho 'pipewire: stream available on node ID: 42' >&2\n\
             while [ \"$1\" != -- ]; do shift; done; shift\n\"$@\"\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let out = folder.join("out");
        let app = format!("echo \"it's $DISPLAY\" > '{}'; sleep 2", out.display());
        let root = folder.join("sessions");
        let (command, session_folder) = wrap_in(
            &root,
            fake.to_str().unwrap(),
            &app,
            &env(&[
                ("SUNSHINE_CLIENT_WIDTH", "640"),
                ("SUNSHINE_CLIENT_HEIGHT", "480"),
            ]),
        )
        .unwrap();
        assert!(command.contains("-W 640 -H 480"), "{command}");
        let mut child = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&command)
            .env("DISPLAY", ":9")
            .spawn()
            .unwrap();
        let session = wait_for_node_in(&root, std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(session.folder, session_folder);
        assert_eq!(session.node, Some(42));
        assert_eq!(session.display.as_deref(), Some(":9"));
        assert_eq!(
            session.capture_target().as_deref(),
            Some("pipewire-node:42")
        );
        child.wait().unwrap();
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "it's :9\n");
        assert!(
            read(&session_folder).is_none(),
            "the session ended with its app"
        );
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
