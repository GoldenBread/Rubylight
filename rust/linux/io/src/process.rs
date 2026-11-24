//! Starting programs and owning the whole tree they grow, the list of running processes, and who the host runs as. A Linux host runs as the signed-in user in that user's session, so there is no SYSTEM service, no other session and no elevation to ask for; a tree is followed by its session and process group, not by a job, and a tree whose host dies is not ended with it yet.

use anyhow::{Context, Result, bail};
use butterpollo_session::nucleus::elevation::Account;
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd, RawFd},
    path::Path,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

mod listing;
mod security;
mod session;
mod signal;
pub use listing::{Usage, creation_time, image_path, now, processes, usage};
pub use security::*;
pub use session::*;
pub use signal::stop_processes;

/// Whether this process is the Windows service's SYSTEM account. Never on Linux, where
/// the host runs as the signed-in user.
pub fn is_system() -> bool {
    false
}

fn effective_user() -> u32 {
    // SAFETY: geteuid takes no arguments and cannot fail.
    unsafe { libc::geteuid() }
}

/// The signed-in user, whose folders hold per-user program settings: the user this host
/// runs as, named by its user ID in decimal (`1000`), which is what stands for a Windows
/// SID on Linux.
pub fn user_sid() -> Option<String> {
    Some(effective_user().to_string())
}

/// The account this process runs as, named like [`user_sid`]; on Linux they are the same.
pub fn own_sid() -> Result<String> {
    Ok(effective_user().to_string())
}

/// Whom a program is started as.
pub enum Target {
    /// The signed-in user, who is the user this host runs as. `elevated` asks for
    /// administrator rights, which only a host running as root has.
    User {
        elevated: bool,
    },
    /// The signed-in user, only while they are the account of this ID ([`user_sid`]): the
    /// one whose files a launch checked.
    Account(String),
    /// A program of the Windows service in another session, which Linux has none of.
    SystemSession(u32),
}

/// Whether this process runs with administrator rights: as root.
pub fn elevated() -> Result<bool> {
    Ok(effective_user() == 0)
}

/// What the signed-in user can start a program elevated with: a host running as root
/// starts everything as root, any other user has no rights to give. Linux has nothing
/// like Windows' separate protected administrator.
pub fn user_account() -> Result<Account> {
    Ok(if elevated()? {
        Account::Elevates
    } else {
        Account::Standard
    })
}

/// Fails unless a program can be started as `target` from this process.
fn check_target(target: &Target) -> Result<()> {
    match target {
        Target::User { elevated: false } => Ok(()),
        Target::User { elevated: true } => {
            if !elevated()? {
                bail!(
                    "this elevated command needs root rights, which a Linux host does not ask for: run the command through sudo or pkexec yourself, or leave it unelevated"
                );
            }
            Ok(())
        }
        Target::Account(expected) => {
            anyhow::ensure!(
                *expected == own_sid()?,
                "another account signed in on this PC since the launch began: start it again"
            );
            Ok(())
        }
        Target::SystemSession(_) => Err(crate::unsupported(
            "Starting a program in another session",
        )),
    }
}

/// A process started by [`Process::spawn_piped`]. Dropping the `process` ends it and everything
/// it started.
pub struct PipedChild {
    pub process: Process,
    pub stdin: Option<std::fs::File>,
    pub stdout: Option<std::fs::File>,
    pub stderr: Option<std::fs::File>,
}

/// A started program and the tree it grew: the processes of the session and process group
/// it leads, and their descendants. Dropping it ends the tree (`SIGKILL`), unless
/// [`Process::leave_children_running`] was called.
///
/// The program's exit code is left unread by the kernel until the drop, which keeps its
/// process ID, and so the session and group it names, from going to another process
/// while this lives.
pub struct Process {
    /// Read (reaped) on drop only.
    child: Option<Child>,
    /// The program itself, for waiting without polling; None on a kernel without process
    /// descriptors.
    held: Option<OwnedFd>,
    ends_with_owner: AtomicBool,
    pub pid: u32,
}

/// A program's identity held open throughout audio capture: the descriptor keeps naming
/// the same process even after its ID went to another.
#[derive(Clone)]
pub struct AudioProcess {
    pub identity: butterpollo_session::audio_scope::Identity,
    held: std::sync::Arc<OwnedFd>,
}
impl AudioProcess {
    /// Holds the process `identity` names, which must still be the one that started at
    /// `identity.started` and belong to this host's user.
    pub fn open(identity: butterpollo_session::audio_scope::Identity) -> Result<Self> {
        let held = signal::pidfd_open(identity.pid).context("opening the game process")?;
        let started = creation_time(identity.pid);
        anyhow::ensure!(
            started == identity.started && started != 0,
            "game process identity changed"
        );
        anyhow::ensure!(
            process_session(identity.pid)? == current_session()?,
            "game process belongs to another user"
        );
        let target = Self {
            identity,
            held: std::sync::Arc::new(held),
        };
        anyhow::ensure!(target.alive(), "game process exited before audio capture");
        Ok(target)
    }
    pub fn alive(&self) -> bool {
        matches!(
            signal::readable([self.held.as_fd()], Duration::ZERO),
            Ok([false])
        )
    }
}

/// The environment a program started for the user gets: this host's own, as the user's
/// session started it with. Names keep their case.
pub fn user_environment() -> Result<BTreeMap<String, String>> {
    Ok(std::env::vars_os()
        .map(|(name, value)| {
            (
                name.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            )
        })
        .collect())
}

/// Hands a started program that nothing waits for to a thread that reads its exit code,
/// so it does not stay in the process list after it ends.
fn reap_later(mut child: Child) {
    let pid = child.id();
    let reaper = std::thread::Builder::new()
        .name("child-reaper".into())
        .spawn(move || {
            let _ = child.wait();
        });
    if let Err(error) = reaper {
        tracing::debug!(%error, pid, "a started program's exit code stays unread until the host ends");
    }
}

/// How a program's standard streams are connected.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Streams {
    /// To nothing: what a program without a console writes is lost.
    Null,
    /// To pipes this process holds the other ends of.
    Piped,
}

impl Process {
    /// A descriptor of one of this program's open files, duplicated into this process
    /// (`pidfd_getfd`, Linux 5.6): `handle` is the descriptor's number in the program. The
    /// kernel allows it for a program this process may debug, as its own child is.
    pub fn duplicate_resource(&self, handle: usize) -> Result<OwnedFd> {
        let theirs = RawFd::try_from(handle)
            .ok()
            .context("invalid child resource handle")?;
        let opened;
        let held = match &self.held {
            Some(held) => held,
            None => {
                opened = signal::pidfd_open(self.pid).context("opening the child process")?;
                &opened
            }
        };
        // SAFETY: pidfd_getfd takes two descriptor numbers and flags, no pointers.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_getfd, held.as_raw_fd(), theirs, 0) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error())
                .context("duplicating the child's descriptor");
        }
        let fd = RawFd::try_from(fd)?;
        // SAFETY: the call returned a new descriptor that nothing else owns.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
    /// Starts `program` with `args` in a session of its own. `extras` are added to this
    /// host's environment. `hidden` is for Windows' console windows and changes nothing
    /// here; the program's standard streams lead nowhere.
    pub fn spawn(
        program: &Path,
        args: &[OsString],
        directory: Option<&Path>,
        target: Target,
        extras: &BTreeMap<String, String>,
        _hidden: bool,
    ) -> Result<Self> {
        let mut command = Command::new(program);
        command.args(args);
        let child = start(command, directory, &target, extras, Streams::Null)?;
        Ok(Self::owning(child))
    }
    /// Starts `program` and lets go of it: it and what it starts outlive this call and
    /// the host.
    pub fn spawn_detached(program: &Path, args: &[OsString], target: Target) -> Result<()> {
        let mut command = Command::new(program);
        command.args(args);
        reap_later(start(
            command,
            None,
            &target,
            &BTreeMap::new(),
            Streams::Null,
        )?);
        Ok(())
    }
    /// Runs `command` through `/bin/sh -c` ([`shell_command`]) and owns its tree.
    pub fn shell(
        command: &str,
        directory: Option<&Path>,
        elevated: bool,
        extras: &BTreeMap<String, String>,
    ) -> Result<Self> {
        Ok(Self::owning(shell_child(
            command, directory, elevated, extras,
        )?))
    }
    /// [`Self::shell`] for a command that may leave programs running once it has ended
    /// ([`Self::leave_children_running`]). Linux has no job around the host to leave, so
    /// the two start a command the same way.
    pub fn shell_leaving_children(
        command: &str,
        directory: Option<&Path>,
        elevated: bool,
        extras: &BTreeMap<String, String>,
    ) -> Result<Self> {
        Self::shell(command, directory, elevated, extras)
    }
    /// [`Self::shell`] without owning the tree: it outlives this call and the host.
    pub fn shell_detached(
        command: &str,
        directory: Option<&Path>,
        elevated: bool,
        extras: &BTreeMap<String, String>,
    ) -> Result<()> {
        reap_later(shell_child(command, directory, elevated, extras)?);
        Ok(())
    }
    /// Start `program` as `target` with its standard input, output and error on pipes, in
    /// the program's own folder.
    pub fn spawn_piped(program: &Path, args: &[OsString], target: Target) -> Result<PipedChild> {
        let mut command = Command::new(program);
        command.args(args);
        let directory = program.parent().filter(|folder| folder.is_dir());
        let mut child = start(command, directory, &target, &BTreeMap::new(), Streams::Piped)?;
        let stdin = child.stdin.take().map(|end| OwnedFd::from(end).into());
        let stdout = child.stdout.take().map(|end| OwnedFd::from(end).into());
        let stderr = child.stderr.take().map(|end| OwnedFd::from(end).into());
        Ok(PipedChild {
            process: Self::owning(child),
            stdin,
            stdout,
            stderr,
        })
    }
    fn owning(child: Child) -> Self {
        let pid = child.id();
        let held = signal::pidfd_open(pid)
            .inspect_err(|error| {
                tracing::debug!(%error, pid, "no process descriptor; the program is waited for by polling");
            })
            .ok();
        Self {
            child: Some(child),
            held,
            ends_with_owner: AtomicBool::new(true),
            pid,
        }
    }
    /// The program's exit code once it has ended: what it returned, or 128 plus the
    /// signal that ended it, as a shell reports it. The code stays readable until the drop.
    pub fn exit_code(&self) -> Result<Option<u32>> {
        let pid = libc::id_t::from(self.pid);
        loop {
            // SAFETY: an all-zero siginfo_t is a valid value of that plain C structure.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            // SAFETY: `info` is a live local the call fills. WNOWAIT leaves the exit code
            // unread, WNOHANG returns at once.
            let waited = unsafe {
                libc::waitid(
                    libc::P_PID,
                    pid,
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            if waited != 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error).context("reading the process's exit code");
            }
            // SAFETY: waitid filled `info` for a child's state change, which carries these
            // fields; with WNOHANG and nothing to report it left the zeroes.
            let (ended, status) = unsafe { (info.si_pid(), info.si_status()) };
            if ended == 0 {
                return Ok(None);
            }
            let status = u32::try_from(status).unwrap_or(0);
            return Ok(Some(if info.si_code == libc::CLD_EXITED {
                status
            } else {
                128 + status
            }));
        }
    }
    /// How many processes of the tree still run.
    pub fn active_processes(&self) -> Result<u32> {
        Ok(u32::try_from(listing::tree(self.pid).len()).unwrap_or(u32::MAX))
    }
    /// The running processes of the tree.
    pub fn process_ids(&self) -> Result<Vec<u32>> {
        Ok(listing::tree(self.pid)
            .into_iter()
            .map(|(pid, _)| pid)
            .collect())
    }
    /// Waits up to `timeout` for the program (not its tree) to end, or for `cancel`.
    fn wait_for(&self, timeout: Duration, cancel: Option<&CommandCancellation>) -> Result<Waited> {
        if let Some(held) = &self.held {
            return Ok(match cancel {
                Some(cancel) => match signal::readable([cancel.0.as_fd(), held.as_fd()], timeout)? {
                    [true, _] => Waited::Cancelled,
                    [false, true] => Waited::Ended,
                    [false, false] => Waited::TimedOut,
                },
                None => match signal::readable([held.as_fd()], timeout)? {
                    [true] => Waited::Ended,
                    [false] => Waited::TimedOut,
                },
            });
        }
        let deadline = Instant::now() + timeout;
        loop {
            if cancel.is_some_and(CommandCancellation::requested) {
                return Ok(Waited::Cancelled);
            }
            if self.exit_code()?.is_some() {
                return Ok(Waited::Ended);
            }
            if Instant::now() >= deadline {
                return Ok(Waited::TimedOut);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    /// Waits up to `timeout` for the program to end: its exit code ([`Self::exit_code`]).
    pub fn wait(&self, timeout: Duration) -> Result<u32> {
        match self.wait_for(timeout, None)? {
            Waited::Ended => self.exit_code()?.context("process still running"),
            _ => bail!("process timed out"),
        }
    }
    /// [`Self::wait`] that `cancel` ends early; a cancelled or timed-out command's whole
    /// tree is ended before this returns, so the owner can run its undo commands.
    pub fn wait_cancellable(
        &self,
        timeout: Duration,
        cancel: &CommandCancellation,
    ) -> Result<Option<u32>> {
        match self.wait_for(timeout, Some(cancel))? {
            Waited::Cancelled => {
                self.stop()?;
                Ok(None)
            }
            Waited::Ended => self.exit_code()?.context("process still running").map(Some),
            Waited::TimedOut => {
                self.stop()?;
                bail!("process timed out")
            }
        }
    }
    /// Lets what the command started and left running outlive this `Process`. For a
    /// command that ended by itself; one that is stopped, timed out or whose wait failed
    /// is dropped without it and takes its tree along.
    pub fn leave_children_running(&self) -> Result<()> {
        self.ends_with_owner.store(false, Ordering::Release);
        Ok(())
    }
    /// Wait up to `timeout` for a command that may leave programs running: one that ended by
    /// itself leaves them ([`Self::leave_children_running`]); otherwise the error, and dropping
    /// `self` ends its whole tree.
    pub fn wait_leaving_children(&self, timeout: Duration) -> Result<u32> {
        let code = self.wait(timeout)?;
        self.leave_children_running_or_warn();
        Ok(code)
    }
    /// [`Self::leave_children_running`], which cannot fail on Linux.
    pub fn leave_children_running_or_warn(&self) {
        if let Err(error) = self.leave_children_running() {
            tracing::warn!(%error, pid = self.pid, "what the command started ends with it");
        }
    }
    /// End the whole tree (`SIGKILL`), without waiting for it to be gone.
    pub fn terminate(&self) -> Result<()> {
        signal::signal_tree(self.pid, libc::SIGKILL);
        Ok(())
    }
    /// End the whole tree (`SIGKILL`) and wait up to 10 seconds for the program itself.
    pub fn stop(&self) -> Result<()> {
        signal::signal_tree(self.pid, libc::SIGKILL);
        self.wait(Duration::from_secs(10))?;
        Ok(())
    }
    /// Ask the whole tree to end (`SIGTERM`), give it up to `timeout`, then
    /// [`Self::stop`] what is left.
    pub fn stop_graceful(&self, timeout: Duration) -> Result<()> {
        if timeout.is_zero() || self.active_processes()? == 0 {
            return self.stop();
        }
        signal::signal_tree(self.pid, libc::SIGTERM);
        let deadline = Instant::now() + timeout;
        while self.active_processes()? != 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        self.stop()
    }
    /// A descriptor of this process cannot be put into a program that already runs:
    /// Linux hands descriptors over as the program starts or over a socket.
    pub fn duplicate_into_raw(&self, _handle: usize) -> Result<u64> {
        Err(crate::unsupported(
            "Handing a descriptor to a program that already runs",
        ))
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        if self.ends_with_owner.load(Ordering::Acquire) {
            signal::signal_tree(self.pid, libc::SIGKILL);
        }
        if let Some(mut child) = self.child.take()
            && !matches!(child.try_wait(), Ok(Some(_)))
        {
            // Still running, or just told to end: its exit code is read when it has.
            reap_later(child);
        }
    }
}

/// How a wait for a program ended.
enum Waited {
    Ended,
    Cancelled,
    TimedOut,
}

/// Starts `command` as the leader of a new session, which is what its tree is followed
/// and stopped by, with `extras` added to this host's environment.
fn start(
    mut command: Command,
    directory: Option<&Path>,
    target: &Target,
    extras: &BTreeMap<String, String>,
    streams: Streams,
) -> Result<Child> {
    use std::os::unix::process::CommandExt;
    check_target(target)?;
    for (key, value) in extras {
        if key.is_empty() || key.contains(['=', '\0']) || value.contains('\0') {
            bail!("invalid process environment variable");
        }
        command.env(key, value);
    }
    if let Some(directory) = directory {
        command.current_dir(directory);
    }
    let stream = || match streams {
        Streams::Null => Stdio::null(),
        Streams::Piped => Stdio::piped(),
    };
    command.stdin(stream()).stdout(stream()).stderr(stream());
    // SAFETY: the closure runs in the new process between fork and exec and only calls
    // setsid, which is async-signal-safe and touches no memory of this process.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command
        .spawn()
        .with_context(|| format!("starting {}", command.get_program().to_string_lossy()))
}

fn shell_child(
    command: &str,
    directory: Option<&Path>,
    elevated: bool,
    extras: &BTreeMap<String, String>,
) -> Result<Child> {
    if command.contains('\0') {
        bail!("shell command contains NUL");
    }
    let mut shell = Command::new("/bin/sh");
    shell.arg("-c").arg(shell_command(command));
    start(
        shell,
        directory,
        &Target::User { elevated },
        extras,
        Streams::Null,
    )
}

/// A shared, manual-reset signal for cancelling owned application commands: once
/// cancelled it stays so.
pub struct CommandCancellation(OwnedFd);
impl CommandCancellation {
    pub fn new() -> Result<Self> {
        // SAFETY: eventfd takes a counter value and flags, no pointers.
        let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error()).context("creating a cancellation event");
        }
        // SAFETY: the call returned a new descriptor that nothing else owns.
        Ok(Self(unsafe { OwnedFd::from_raw_fd(fd) }))
    }
    pub fn cancel(&self) -> Result<()> {
        let one = 1u64.to_ne_bytes();
        // SAFETY: `one` is eight live bytes, the size an eventfd write takes.
        let written = unsafe { libc::write(self.0.as_raw_fd(), one.as_ptr().cast(), one.len()) };
        if written < 0 {
            let error = std::io::Error::last_os_error();
            // The counter is full: it was cancelled often enough already.
            if error.kind() != std::io::ErrorKind::WouldBlock {
                return Err(error).context("cancelling the command");
            }
        }
        Ok(())
    }
    pub fn requested(&self) -> bool {
        matches!(
            signal::readable([self.0.as_fd()], Duration::ZERO),
            Ok([true])
        )
    }
}

/// The stop request of the Windows service that supervises a host. Nothing supervises a
/// Linux host this way, so no stop is ever requested through it.
pub struct StopSignal(());
impl StopSignal {
    /// Without a `source`, a host nothing supervises. A source names a Windows service's
    /// stop event, which a Linux host cannot be given.
    pub fn new(source: Option<&str>) -> Result<Self> {
        match source {
            None => Ok(Self(())),
            Some(_) => Err(crate::unsupported("A host supervised by a service")),
        }
    }
    pub fn requested(&self) -> bool {
        false
    }
    /// Whether a supervisor can ask for the stop.
    pub fn supervised(&self) -> bool {
        false
    }
    /// Without a supervisor nothing can ask, so this returns at once.
    pub fn wait(&self) {}
}

/// The program a command line starts: its first argument, without quotes (double, or the
/// shell's single ones).
pub fn command_target(command: &str) -> &str {
    let command = command.trim_start();
    for quote in ['"', '\''] {
        if let Some(rest) = command.strip_prefix(quote) {
            return rest.split(quote).next().unwrap_or(rest);
        }
    }
    command.split(char::is_whitespace).next().unwrap_or("")
}
/// What a command line passes its program: everything after
/// [`command_target`], trimmed.
pub fn command_arguments(command: &str) -> &str {
    let command = command.trim_start();
    for quote in ['"', '\''] {
        if let Some(rest) = command.strip_prefix(quote) {
            return rest.split_once(quote).map_or("", |(_, rest)| rest).trim();
        }
    }
    command
        .split_once(char::is_whitespace)
        .map_or("", |(_, rest)| rest)
        .trim()
}
/// Whether the desktop must open the target through its association: a URL
/// (`steam://`, `heroic://`). A shell cannot start one.
fn opens_through_desktop(target: &str) -> bool {
    target.split_once(':').is_some_and(|(scheme, _)| {
        scheme.len() > 1
            && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    })
}
/// `text` as one word of a shell command.
fn shell_word(text: &OsStr) -> String {
    format!("'{}'", text.to_string_lossy().replace('\'', "'\\''"))
}
/// The command for `/bin/sh -c`. A URL is opened as the desktop opens it, through
/// `xdg-open`, which returns once it has handed the URL over: unlike on Windows, the
/// command does not last as long as the program the URL starts.
pub fn shell_command(command: &str) -> String {
    let target = command_target(command);
    if target.is_empty() || !opens_through_desktop(target) {
        return command.to_owned();
    }
    let rest = command_arguments(command);
    let open = format!("xdg-open {}", shell_word(OsStr::new(target)));
    if rest.is_empty() {
        open
    } else {
        format!("{open} {rest}")
    }
}

#[cfg(test)]
mod tests;
