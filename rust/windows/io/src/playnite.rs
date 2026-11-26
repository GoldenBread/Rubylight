//! Playnite on this PC: where it is installed, where its extensions go, and
//! the named pipe its Rubylight (Vibepollo) plugin serves.

use anyhow::{Context, Result, bail};
use std::{
    fs::File,
    io::{Read, Write},
    os::windows::{
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::{ERROR_IO_PENDING, HANDLE, WAIT_OBJECT_0},
    Storage::FileSystem::{FILE_FLAG_OVERLAPPED, ReadFile, SECURITY_IDENTIFICATION, WriteFile},
    System::{
        IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
        Pipes::{GetNamedPipeServerSessionId, PeekNamedPipe, WaitNamedPipeW},
        Registry::{HKEY_LOCAL_MACHINE, HKEY_USERS},
        RemoteDesktop::ProcessIdToSessionId,
        Threading::{CreateEventW, GetCurrentProcessId, WaitForSingleObject},
    },
};

const MAX_PENDING_BYTES: usize = 4 * 1024 * 1024;
const PIPE_WRITE_TIMEOUT: Duration = Duration::from_secs(2);

use crate::steam::registry_string;

const PIPE: &str = r"\\.\pipe\Sunshine.PlayniteExtension";
pub const PROCESSES: [&str; 2] = ["Playnite.DesktopApp.exe", "Playnite.FullscreenApp.exe"];

fn session(pid: u32) -> Option<u32> {
    let mut session = 0;
    // SAFETY: `session` is a local u32 that outlives the call.
    unsafe { ProcessIdToSessionId(pid, &mut session).ok()? };
    Some(session)
}
fn in_session(
    processes: Vec<butterpollo_session::steam::Process>,
    current: u32,
    mut session: impl FnMut(u32) -> Option<u32>,
) -> Vec<butterpollo_session::steam::Process> {
    processes
        .into_iter()
        .filter(|p| session(p.pid) == Some(current))
        .collect()
}
/// Processes in the streaming user's session, including games handed off
/// to a store client rather than started as Playnite children.
pub fn session_processes() -> Result<Vec<butterpollo_session::steam::Process>> {
    // SAFETY: GetCurrentProcessId takes no arguments and cannot fail.
    let current =
        session(unsafe { GetCurrentProcessId() }).context("reading the host's Windows session")?;
    Ok(in_session(crate::process::processes()?, current, session))
}
/// The running Playnite process (id and program), if any.
pub fn running() -> Option<(u32, PathBuf)> {
    // SAFETY: GetCurrentProcessId takes no arguments and cannot fail.
    let current = session(unsafe { GetCurrentProcessId() })?;
    find_running(
        crate::process::processes().ok()?,
        current,
        session,
        crate::process::image_path,
    )
}
fn find_running(
    processes: Vec<butterpollo_session::steam::Process>,
    current: u32,
    session: impl FnMut(u32) -> Option<u32>,
    mut image: impl FnMut(u32) -> Option<String>,
) -> Option<(u32, PathBuf)> {
    in_session(processes, current, session)
        .into_iter()
        .filter(|p| {
            PROCESSES
                .iter()
                .any(|name| p.name.eq_ignore_ascii_case(name))
        })
        .find_map(|p| Some((p.pid, PathBuf::from(image(p.pid)?))))
}
/// The running Playnite's folder, then the user's association, installer
/// record and default installation folder.
pub fn install_dir() -> Option<PathBuf> {
    if let Some((_, exe)) = running() {
        return exe.parent().map(PathBuf::from);
    }
    let mut candidates = Vec::new();
    if let Some(sid) = crate::process::user_sid() {
        if let Some(command) = registry_string(
            HKEY_USERS,
            &format!("{sid}\\Software\\Classes\\playnite\\shell\\open\\command"),
            "",
        ) {
            candidates.extend(
                Path::new(crate::process::command_target(&command))
                    .parent()
                    .map(PathBuf::from),
            );
        }
        if let Some(folder) = registry_string(
            HKEY_USERS,
            &format!(
                "{sid}\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Playnite_is1"
            ),
            "InstallLocation",
        ) {
            candidates.push(PathBuf::from(folder));
        }
    }
    if let Some(command) = registry_string(
        HKEY_LOCAL_MACHINE,
        "Software\\Classes\\playnite\\shell\\open\\command",
        "",
    ) {
        candidates.extend(
            Path::new(crate::process::command_target(&command))
                .parent()
                .map(PathBuf::from),
        );
    }
    if let Ok(environment) = crate::process::user_environment()
        && let Some(local) = environment.get("LOCALAPPDATA")
    {
        candidates.push(PathBuf::from(local).join("Playnite"));
    }
    find_install(candidates)
}
fn find_install(candidates: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    candidates.into_iter().find(|dir| executable(dir).is_some())
}
/// A wall-clock time on this PC (Unix seconds as if it were UTC) as Unix seconds,
/// by the time zone's rule on that date: Playnite's times without an offset.
pub fn local_to_utc(wall: i64) -> Option<i64> {
    use windows::Win32::{
        Foundation::{FILETIME, SYSTEMTIME},
        System::Time::{
            FileTimeToSystemTime, SystemTimeToFileTime, TzSpecificLocalTimeToSystemTime,
        },
    };
    // FILETIME counts 100 ns from 1601.
    const EPOCH: i64 = 11_644_473_600;
    let ticks = u64::try_from(wall.checked_add(EPOCH)?.checked_mul(10_000_000)?).ok()?;
    let file = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let (mut local, mut utc, mut out) = (
        SYSTEMTIME::default(),
        SYSTEMTIME::default(),
        FILETIME::default(),
    );
    // SAFETY: every pointer is to a local that outlives the call.
    unsafe {
        FileTimeToSystemTime(&file, &mut local).ok()?;
        TzSpecificLocalTimeToSystemTime(None, &local, &mut utc).ok()?;
        SystemTimeToFileTime(&utc, &mut out).ok()?;
    }
    let ticks = (u64::from(out.dwHighDateTime) << 32) | u64::from(out.dwLowDateTime);
    Some(i64::try_from(ticks / 10_000_000).ok()? - EPOCH)
}
pub fn executable(dir: &Path) -> Option<PathBuf> {
    PROCESSES
        .iter()
        .map(|name| dir.join(name))
        .find(|exe| exe.is_file())
}
/// Where Playnite loads extensions from: the user's roaming folder for an
/// installed Playnite, the program folder for a portable one.
pub fn extensions_dir() -> Option<PathBuf> {
    let dir = install_dir()?;
    let environment = crate::process::user_environment().ok();
    extensions_at(
        &dir,
        environment
            .as_ref()
            .and_then(|env| env.get("APPDATA"))
            .map(Path::new),
    )
}
fn extensions_at(dir: &Path, roaming: Option<&Path>) -> Option<PathBuf> {
    // Playnite itself uses the uninstaller's presence to choose its data folder.
    if dir.join("unins000.exe").is_file() {
        Some(roaming?.join("Playnite").join("Extensions"))
    } else {
        Some(dir.join("Extensions"))
    }
}
/// Start the resolved executable in the signed-in user's session, without
/// depending on a URI association or cmd.exe reporting a launch failure.
pub fn launch(
    program: &Path,
    args: &[&str],
    environment: &std::collections::BTreeMap<String, String>,
) -> Result<()> {
    let mut command = crate::process::quote(program.as_os_str())?;
    for arg in args {
        command.push(' ');
        command.push_str(&crate::process::quote(std::ffi::OsStr::new(arg))?);
    }
    crate::process::Process::spawn_command(
        program,
        &command,
        program.parent(),
        crate::process::Target::User { elevated: false },
        environment,
        false,
        true,
    )
    .with_context(|| {
        format!(
            "starting Playnite executable {} in the signed-in user's session",
            program.display()
        )
    })?;
    Ok(())
}

fn handle(file: &File) -> HANDLE {
    HANDLE(file.as_raw_handle())
}
fn check_session(file: &File, current: u32) -> Result<()> {
    let mut server_session = 0;
    // SAFETY: `file` keeps the pipe handle open, and `server_session` outlives the call.
    unsafe { GetNamedPipeServerSessionId(handle(file), &mut server_session)? };
    if server_session != current {
        bail!(
            "Playnite plugin pipe belongs to Windows session {server_session}, but the host is in session {current}"
        );
    }
    Ok(())
}
/// Bytes waiting in the pipe; an error once it is closed.
fn available(file: &File) -> Result<u32> {
    let mut count = 0;
    // SAFETY: `file` keeps the pipe handle open; only `count` is written, and it outlives the call.
    unsafe { PeekNamedPipe(handle(file), None, 0, None, Some(&mut count), None)? };
    Ok(count)
}
fn open_with_flags(name: &str, wait: Duration, extra_flags: u32) -> Result<File> {
    let deadline = Instant::now() + wait;
    loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            // A connector in our session must not borrow the host's token.
            .security_qos_flags(SECURITY_IDENTIFICATION.0)
            .custom_flags(extra_flags)
            .open(name)
        {
            Ok(file) => return Ok(file),
            // Busy: another client is mid-handshake.
            Err(error) if error.raw_os_error() == Some(231) && Instant::now() < deadline => {
                let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
                // SAFETY: `wide` is a NUL-terminated pipe name that outlives the call.
                unsafe {
                    let _ = WaitNamedPipeW(windows::core::PCWSTR(wide.as_ptr()), 500);
                }
            }
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(error).with_context(|| format!("opening {name}")),
        }
    }
}
fn open(name: &str, wait: Duration) -> Result<File> {
    open_with_flags(name, wait, 0)
}
fn pipe_event() -> Result<OwnedHandle> {
    // SAFETY: CreateEventW returns a fresh manual-reset event handle owned only by this value.
    unsafe {
        Ok(OwnedHandle::from_raw_handle(
            CreateEventW(None, true, false, None)?.0,
        ))
    }
}
fn overlapped_read(file: &File, buf: &mut [u8]) -> Result<usize> {
    let event = pipe_event()?;
    let mut ov = OVERLAPPED {
        hEvent: HANDLE(event.as_raw_handle()),
        ..Default::default()
    };
    let mut read = 0;
    // SAFETY: `buf`, `ov`, and `event` remain valid until `GetOverlappedResult(..., true)` returns.
    unsafe {
        if let Err(error) = ReadFile(handle(file), Some(buf), None, Some(&mut ov))
            && error.code() != ERROR_IO_PENDING.to_hresult()
        {
            return Err(error.into());
        }
        GetOverlappedResult(handle(file), &ov, &mut read, true)?;
    }
    Ok(read as usize)
}
fn overlapped_write(file: &File, data: &[u8], timeout: Duration) -> Result<()> {
    let event = pipe_event()?;
    let mut ov = OVERLAPPED {
        hEvent: HANDLE(event.as_raw_handle()),
        ..Default::default()
    };
    let mut written = 0;
    // SAFETY: `data`, `ov`, and `event` live past `GetOverlappedResult(..., true)`, which waits
    // for any `CancelIoEx` completion before returning.
    unsafe {
        if let Err(error) = WriteFile(handle(file), Some(data), None, Some(&mut ov))
            && error.code() != ERROR_IO_PENDING.to_hresult()
        {
            return Err(error.into());
        }
        let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
        if WaitForSingleObject(HANDLE(event.as_raw_handle()), ms) != WAIT_OBJECT_0 {
            let _ = CancelIoEx(handle(file), Some(&ov));
            let _ = GetOverlappedResult(handle(file), &ov, &mut written, true);
            bail!("writing to the Playnite data pipe timed out");
        }
        GetOverlappedResult(handle(file), &ov, &mut written, true)?;
    }
    if written as usize != data.len() {
        bail!("incomplete write to the Playnite data pipe");
    }
    Ok(())
}
/// A connection to the plugin: newline-separated JSON both ways.
pub struct Pipe {
    file: Arc<File>,
    pub lines: mpsc::Receiver<String>,
    stop: Arc<AtomicBool>,
    reader: Option<std::thread::JoinHandle<()>>,
}
impl Pipe {
    /// Connect and introduce this side with `hello`. The plugin answers on
    /// its well-known pipe with the name of a private one.
    pub fn connect(hello: &serde_json::Value) -> Result<Self> {
        Self::connect_to(PIPE, hello)
    }
    fn connect_to(control_name: &str, hello: &serde_json::Value) -> Result<Self> {
        // SAFETY: GetCurrentProcessId takes no arguments and cannot fail.
        let current = session(unsafe { GetCurrentProcessId() })
            .context("reading the host's Windows session")?;
        let mut control = open(control_name, Duration::from_secs(2))
            .context("the Playnite plugin is not running")?;
        check_session(&control, current).context("checking the Playnite control pipe session")?;
        let deadline = Instant::now() + Duration::from_secs(2);
        while available(&control).context("reading the Playnite control pipe handshake")? < 80 {
            if Instant::now() > deadline {
                bail!(
                    "the Playnite plugin did not send its 80-byte control pipe handshake within 2 seconds"
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut message = [0u8; 80];
        control
            .read_exact(&mut message)
            .context("reading the Playnite private pipe name")?;
        let units: Vec<u16> = message
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .take_while(|unit| *unit != 0)
            .collect();
        let name = String::from_utf16(&units)?;
        if name.is_empty() || name.contains(['\\', '/']) {
            bail!("the Playnite plugin sent an invalid pipe name");
        }
        control
            .write_all(&[2])
            .context("acknowledging the Playnite pipe handshake")?;
        let file = open_with_flags(
            &format!(r"\\.\pipe\{name}"),
            Duration::from_secs(5),
            FILE_FLAG_OVERLAPPED.0,
        )
        .context("connecting to the Playnite private pipe")?;
        check_session(&file, current).context("checking the Playnite private pipe session")?;
        drop(control);
        let file = Arc::new(file);
        let (sender, lines) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let reader = {
            let (file, stop) = (file.clone(), stop.clone());
            std::thread::Builder::new()
                .name("playnite-pipe".into())
                .spawn(move || {
                    let mut pending = Vec::new();
                    while !stop.load(Ordering::Acquire) {
                        let read = match available(&file) {
                            Ok(0) => Ok(0),
                            Ok(count) => {
                                let mut chunk = vec![0; count.min(1 << 20) as usize];
                                overlapped_read(&file, &mut chunk).inspect(|&n| {
                                    if pending.len() + n <= MAX_PENDING_BYTES {
                                        pending.extend_from_slice(&chunk[..n]);
                                    }
                                })
                            }
                            Err(error) => Err(error),
                        };
                        match read {
                            Ok(0) => std::thread::sleep(Duration::from_millis(30)),
                            Ok(n) if pending.len() + n > MAX_PENDING_BYTES => {
                                tracing::warn!(
                                    limit = MAX_PENDING_BYTES,
                                    "Playnite data pipe line exceeded maximum size; closing"
                                );
                                return;
                            }
                            Ok(_) => {
                                while let Some(end) = pending.iter().position(|b| *b == b'\n') {
                                    let line: Vec<u8> = pending.drain(..=end).collect();
                                    let line = String::from_utf8_lossy(&line)
                                        .trim_end_matches(['\r', '\n'])
                                        .to_owned();
                                    if !line.is_empty() && sender.send(line).is_err() {
                                        return;
                                    }
                                }
                            }
                            // Closed: the receiver sees the channel end.
                            Err(error) => {
                                tracing::warn!(error = %format!("{error:#}"), "Playnite data pipe closed or failed");
                                return;
                            }
                        }
                    }
                })?
        };
        let pipe = Self {
            file,
            lines,
            stop,
            reader: Some(reader),
        };
        pipe.send(hello)
            .context("sending the Playnite plugin hello")?;
        Ok(pipe)
    }
    pub fn send(&self, message: &serde_json::Value) -> Result<()> {
        let mut line = serde_json::to_vec(message)?;
        line.push(b'\n');
        overlapped_write(&self.file, &line, PIPE_WRITE_TIMEOUT)
    }
}
impl Drop for Pipe {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // SAFETY: `self.file` stays live across CancelIoEx, unblocking any pending overlapped I/O.
        let _ = unsafe { CancelIoEx(handle(&self.file), None) };
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

#[cfg(test)]
mod tests;
