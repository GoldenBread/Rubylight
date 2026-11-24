//! Process environment blocks, inheritable pipes, and job-assigned spawning.
use super::{
    Process,
    control::HostStop,
    inspect::shell_command,
    owned, quote, raw,
    tokens::{Target, is_system, target_token},
    wide,
};
use anyhow::{Result, bail};
use std::{
    collections::BTreeMap, ffi::OsString, mem::size_of, os::windows::io::OwnedHandle, path::Path,
};
use windows::{
    Win32::{
        Foundation::*,
        System::{Environment::*, JobObjects::*, Threading::*},
    },
    core::{PCWSTR, PWSTR},
};

struct Environment(*mut std::ffi::c_void);
impl Drop for Environment {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: `self.0` is non-null and came from CreateEnvironmentBlock, and Drop runs
            // once.
            unsafe {
                let _ = DestroyEnvironmentBlock(self.0);
            }
        }
    }
}

/// An attribute list that restricts which handles a new process inherits.
struct AttributeList(Vec<usize>);
impl AttributeList {
    /// Make the list name exactly `handles`. The slice must outlive the process creation call.
    fn handle_list(&mut self, handles: &[HANDLE; 3]) -> Result<LPPROC_THREAD_ATTRIBUTE_LIST> {
        // SAFETY: the buffer is sized by the first call and aligned as usize, as the API needs;
        // `handles` is read during the process creation call that the caller makes while it is
        // alive, and the list is deleted once on drop.
        unsafe {
            let mut size = 0usize;
            // Fails with ERROR_INSUFFICIENT_BUFFER and the size needed; the size is what matters.
            let _ = InitializeProcThreadAttributeList(None, 1, None, &mut size);
            self.0 = vec![0usize; size.div_ceil(size_of::<usize>())];
            let list = LPPROC_THREAD_ATTRIBUTE_LIST(self.0.as_mut_ptr().cast());
            if let Err(e) = InitializeProcThreadAttributeList(Some(list), 1, None, &mut size) {
                // Nothing was initialized, so drop has nothing to delete.
                self.0.clear();
                return Err(e.into());
            }
            if let Err(e) = UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                Some(handles.as_ptr().cast()),
                size_of::<[HANDLE; 3]>(),
                None,
                None,
            ) {
                DeleteProcThreadAttributeList(list);
                self.0.clear();
                return Err(e.into());
            }
            Ok(list)
        }
    }
}
impl Drop for AttributeList {
    fn drop(&mut self) {
        if !self.0.is_empty() {
            // SAFETY: the buffer holds a list initialized by `handle_list`, deleted here once.
            unsafe {
                DeleteProcThreadAttributeList(LPPROC_THREAD_ATTRIBUTE_LIST(
                    self.0.as_mut_ptr().cast(),
                ))
            };
        }
    }
}

/// Handles marked inheritable for as short a time as possible: made so on creation, made
/// private again on drop (the handles must outlive the guard).
struct InheritableHandles([HANDLE; 3]);
impl InheritableHandles {
    fn new(handles: [HANDLE; 3]) -> Result<Self> {
        let guard = Self(handles);
        for handle in handles {
            // SAFETY: the caller owns these handles and keeps them open while this guard lives.
            unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT.0, HANDLE_FLAG_INHERIT)? };
        }
        Ok(guard)
    }
}
impl Drop for InheritableHandles {
    fn drop(&mut self) {
        for handle in self.0 {
            // SAFETY: as in `new`; a handle that cannot be changed was closed, which is as good.
            let _ = unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0)) };
        }
    }
}

pub(super) enum PipeEnd {
    /// The child reads (its stdin).
    Read,
    /// The child writes (its stdout and stderr).
    Write,
}

/// An anonymous pipe: (the end the child will use, the end this process keeps). Neither is
/// inheritable yet: `spawn_command_with` makes the child's end inheritable right before it creates
/// the process, so a process another thread starts in between cannot inherit a copy and keep the
/// pipe open. (The protocol also does not depend on the pipes closing.)
pub(super) fn pipe_pair(child: PipeEnd) -> Result<(OwnedHandle, OwnedHandle)> {
    // SAFETY: both out-pointers are live locals, and each new handle is owned once.
    unsafe {
        let (mut read, mut write) = (HANDLE::default(), HANDLE::default());
        windows::Win32::System::Pipes::CreatePipe(&mut read, &mut write, None, 0)?;
        let (read, write) = (owned(read), owned(write));
        Ok(match child {
            PipeEnd::Read => (read, write),
            PipeEnd::Write => (write, read),
        })
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

pub fn user_environment() -> Result<BTreeMap<String, String>> {
    Ok(settable(environment(
        target_token(&Target::User { elevated: false })?.as_ref(),
    )?))
}

/// Without Windows' hidden `=C:` and `=ExitCode` entries, which a host started from a shell
/// inherits. Callers hand these back as a program's variables, and a name with `=` cannot be one:
/// every app with a command failed its launch with "invalid process environment variable".
pub(super) fn settable(mut environment: BTreeMap<String, String>) -> BTreeMap<String, String> {
    environment.retain(|name, _| !name.contains('='));
    environment
}

/// The environment of a program started with `token`: that user's variables, as the desktop
/// gives them. Without a token the program runs as this host's own user, and gets this host's
/// environment as the host started with it, like a program a launcher starts (a variable the
/// user changes later reaches it after a host restart). CreateEnvironmentBlock without a token
/// would return the system's variables
/// instead (USERPROFILE `C:\Users\Default`, USERNAME SYSTEM, TEMP `C:\Windows\Temp`), and they
/// replace the inherited ones; the program then misses its user's caches and settings, such as
/// the AMD shader cache that made the PyroWave probe compile every pipeline again and time out.
pub(super) fn environment(token: Option<&OwnedHandle>) -> Result<BTreeMap<String, String>> {
    let Some(token) = token else {
        return Ok(std::env::vars_os()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().to_uppercase(),
                    value.to_string_lossy().into_owned(),
                )
            })
            .collect());
    };
    // SAFETY: the block from CreateEnvironmentBlock is NUL-terminated UTF-16 strings ending in an
    // empty one, so reads stop at the terminators, and `native` frees it on drop.
    unsafe {
        let mut native = Environment(std::ptr::null_mut());
        CreateEnvironmentBlock(&mut native.0, Some(raw(token)), false)?;
        let mut environment = BTreeMap::new();
        let mut offset = 0;
        while !native.0.is_null() && offset < 1024 * 1024 {
            let p = native.0.cast::<u16>().add(offset);
            if *p == 0 {
                break;
            }
            let mut len = 0;
            while offset + len < 1024 * 1024 && *p.add(len) != 0 {
                len += 1;
            }
            if offset + len >= 1024 * 1024 {
                bail!("process environment exceeds the limit");
            }
            let entry = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
            let split = if let Some(rest) = entry.strip_prefix('=') {
                rest.find('=').map(|i| i + 1)
            } else {
                entry.find('=')
            };
            if let Some(i) = split {
                environment.insert(entry[..i].to_uppercase(), entry[i + 1..].to_owned());
            }
            offset += len + 1;
        }
        Ok(environment)
    }
}

impl Process {
    /// Keep the supervisor's unnamed stop event alive for the entire child.
    /// The child duplicates a wait-only handle; handle inheritance cannot cross
    /// Windows sessions, and session-local object names cannot reach the service.
    pub fn spawn_host(
        program: &Path,
        args: &[OsString],
        directory: Option<&Path>,
        target: Target,
    ) -> Result<Self> {
        let stop = HostStop::new()?;
        let mut args = args.to_vec();
        args.extend(["--service-stop-source".into(), stop.source().into()]);
        let mut process = Self::spawn(program, &args, directory, target, &BTreeMap::new(), true)?;
        process.host_stop = Some(stop);
        Ok(process)
    }
    pub fn spawn(
        program: &Path,
        args: &[OsString],
        directory: Option<&Path>,
        target: Target,
        extras: &BTreeMap<String, String>,
        hidden: bool,
    ) -> Result<Self> {
        let mut command = quote(program.as_os_str())?;
        for arg in args {
            command.push(' ');
            command.push_str(&quote(arg)?);
        }
        Self::spawn_command(program, &command, directory, target, extras, hidden, false)
    }
    pub fn spawn_detached(program: &Path, args: &[OsString], target: Target) -> Result<()> {
        let mut command = quote(program.as_os_str())?;
        for arg in args {
            command.push(' ');
            command.push_str(&quote(arg)?);
        }
        Self::spawn_command(
            program,
            &command,
            None,
            target,
            &BTreeMap::new(),
            true,
            true,
        )?;
        Ok(())
    }
    pub fn shell(
        command: &str,
        directory: Option<&Path>,
        elevated: bool,
        extras: &BTreeMap<String, String>,
    ) -> Result<Self> {
        Self::shell_inner(command, directory, elevated, extras, false, false)
    }
    /// [`Self::shell`] for a command that may leave programs running once it has ended
    /// ([`Self::leave_children_running`]): like a detached command it is created outside the
    /// host's own job where Windows lets it leave, so what it leaves outlives the host, while its
    /// own job still ends it and everything it started when it is stopped or the host dies first.
    pub fn shell_leaving_children(
        command: &str,
        directory: Option<&Path>,
        elevated: bool,
        extras: &BTreeMap<String, String>,
    ) -> Result<Self> {
        Self::shell_inner(command, directory, elevated, extras, false, true)
    }
    pub fn shell_detached(
        command: &str,
        directory: Option<&Path>,
        elevated: bool,
        extras: &BTreeMap<String, String>,
    ) -> Result<()> {
        Self::shell_inner(command, directory, elevated, extras, true, true)?;
        Ok(())
    }
    fn shell_inner(
        command: &str,
        directory: Option<&Path>,
        elevated: bool,
        extras: &BTreeMap<String, String>,
        detached: bool,
        breakaway: bool,
    ) -> Result<Self> {
        if command.contains('\0') {
            bail!("shell command contains NUL");
        }
        let executable = std::path::PathBuf::from(
            std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into()),
        )
        .join("System32")
        .join("cmd.exe");
        let command = shell_command(command);
        let line = format!("{} /D /S /C \"{command}\"", quote(executable.as_os_str())?);
        Self::spawn_command_with(
            &executable,
            &line,
            directory,
            Target::User { elevated },
            extras,
            true,
            detached,
            breakaway,
            None,
        )
    }
    /// Start `program` as `target` (inside the usual job, so it dies with the owner) with its
    /// standard input, output and error on pipes. A SYSTEM host uses this to run a program as the
    /// signed-in user: the child holds the user's token from the start, nothing in it runs as SYSTEM.
    /// It cannot start a program through the desktop shell's token (an elevated administrator host
    /// that is not SYSTEM), because that path cannot hand over pipes.
    pub fn spawn_piped(program: &Path, args: &[OsString], target: Target) -> Result<PipedChild> {
        let mut command = quote(program.as_os_str())?;
        for arg in args {
            command.push(' ');
            command.push_str(&quote(arg)?);
        }
        let (stdin_read, stdin_write) = pipe_pair(PipeEnd::Read)?;
        let (stdout_write, stdout_read) = pipe_pair(PipeEnd::Write)?;
        let (stderr_write, stderr_read) = pipe_pair(PipeEnd::Write)?;
        let process = Self::spawn_command_with(
            program,
            &command,
            // The working directory a SYSTEM parent has (its profile) is not the user's to open.
            program.parent(),
            target,
            &BTreeMap::new(),
            true,
            false,
            false,
            Some([raw(&stdin_read), raw(&stdout_write), raw(&stderr_write)]),
        )?;
        // The child holds its ends now; the parent's copies would keep the pipes open forever.
        drop((stdin_read, stdout_write, stderr_write));
        Ok(PipedChild {
            process,
            stdin: Some(std::fs::File::from(stdin_write)),
            stdout: Some(std::fs::File::from(stdout_read)),
            stderr: Some(std::fs::File::from(stderr_read)),
        })
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn spawn_command(
        program: &Path,
        line: &str,
        directory: Option<&Path>,
        target: Target,
        extras: &BTreeMap<String, String>,
        hidden: bool,
        detached: bool,
    ) -> Result<Self> {
        Self::spawn_command_with(
            program, line, directory, target, extras, hidden, detached, detached, None,
        )
    }
    /// `stdio` is the child's (stdin, stdout, stderr), inheritable handles that only the child
    /// inherits. A `detached` process's job has no kill-on-close; one that may `breakaway` is
    /// created outside the host's own job, or inside it where Windows refuses.
    #[allow(clippy::too_many_arguments)]
    fn spawn_command_with(
        program: &Path,
        line: &str,
        directory: Option<&Path>,
        target: Target,
        extras: &BTreeMap<String, String>,
        hidden: bool,
        detached: bool,
        breakaway: bool,
        stdio: Option<[HANDLE; 3]>,
    ) -> Result<Self> {
        let token = target_token(&target)?;
        // Only SYSTEM assigns primary tokens; an elevated administrator gets
        // the desktop shell's token for non-elevated programs.
        let shell_token = token.is_some() && !is_system();
        if shell_token && stdio.is_some() {
            bail!("cannot give pipes to a program started through the desktop shell's token");
        }
        let program = wide(program.as_os_str())?;
        let directory = directory.map(|p| wide(p.as_os_str())).transpose()?;
        let mut line = wide(std::ffi::OsStr::new(line))?;
        let mut environment = environment(token.as_ref())?;
        // SAFETY: the strings given to CreateProcess are NUL-terminated locals, and each new
        // handle is owned once.
        unsafe {
            for (key, value) in extras {
                if key.is_empty() || key.contains(['=', '\0']) || value.contains('\0') {
                    bail!("invalid process environment variable");
                }
                environment.insert(key.to_uppercase(), value.clone());
            }
            let mut env: Vec<u16> = environment
                .iter()
                .flat_map(|(k, v)| format!("{k}={v}\0").encode_utf16().collect::<Vec<_>>())
                .collect();
            env.extend([0, 0]);
            let job = owned(CreateJobObjectW(None, PCWSTR::null())?);
            let limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
                BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
                    LimitFlags: JOB_OBJECT_LIMIT_BREAKAWAY_OK
                        | if detached {
                            JOB_OBJECT_LIMIT(0)
                        } else {
                            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                        },
                    ..Default::default()
                },
                ..Default::default()
            };
            SetInformationJobObject(
                raw(&job),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )?;
            let mut desktop: Vec<u16> = "winsta0\\default\0".encode_utf16().collect();
            let mut startup = STARTUPINFOEXW {
                StartupInfo: STARTUPINFOW {
                    cb: if stdio.is_some() {
                        size_of::<STARTUPINFOEXW>()
                    } else {
                        size_of::<STARTUPINFOW>()
                    } as u32,
                    lpDesktop: PWSTR(desktop.as_mut_ptr()),
                    dwFlags: if hidden {
                        STARTF_USESHOWWINDOW
                    } else {
                        STARTUPINFOW_FLAGS(0)
                    },
                    wShowWindow: if hidden { 0 } else { 1 },
                    ..Default::default()
                },
                lpAttributeList: LPPROC_THREAD_ATTRIBUTE_LIST(std::ptr::null_mut()),
            };
            // Only the three standard handles are inherited, not every inheritable handle of this
            // process.
            let inherited = stdio.unwrap_or_default();
            let mut attributes = AttributeList(Vec::new());
            if let Some(stdio) = stdio {
                startup.StartupInfo.dwFlags |= STARTF_USESTDHANDLES;
                startup.StartupInfo.hStdInput = stdio[0];
                startup.StartupInfo.hStdOutput = stdio[1];
                startup.StartupInfo.hStdError = stdio[2];
                startup.lpAttributeList = attributes.handle_list(&inherited)?;
            }
            let startup = startup;
            let mut info = PROCESS_INFORMATION::default();
            // The process is created suspended and assigned to its own job below. When it may
            // break away from the host's job, it is in no job between creation and that
            // assignment: a host that dies in that instant leaves a suspended process that never
            // ran anything, as detached commands always could.
            let flags = CREATE_SUSPENDED
                | CREATE_UNICODE_ENVIRONMENT
                | if stdio.is_some() {
                    EXTENDED_STARTUPINFO_PRESENT
                } else {
                    PROCESS_CREATION_FLAGS(0)
                }
                | if breakaway {
                    CREATE_BREAKAWAY_FROM_JOB
                } else {
                    PROCESS_CREATION_FLAGS(0)
                }
                | if hidden {
                    CREATE_NO_WINDOW
                } else {
                    PROCESS_CREATION_FLAGS(0)
                };
            let current = directory
                .as_ref()
                .map_or(PCWSTR::null(), |d| PCWSTR(d.as_ptr()));
            let mut create = |flags: PROCESS_CREATION_FLAGS| -> windows::core::Result<()> {
                if let Some(token) = &token
                    && shell_token
                {
                    // Administrators lack SeAssignPrimaryTokenPrivilege, so
                    // the secondary logon service starts the program. It is
                    // never created inside this job; breakaway cannot apply.
                    CreateProcessWithTokenW(
                        raw(token),
                        CREATE_PROCESS_LOGON_FLAGS(0),
                        PCWSTR(program.as_ptr()),
                        Some(PWSTR(line.as_mut_ptr())),
                        PROCESS_CREATION_FLAGS(flags.0 & !CREATE_BREAKAWAY_FROM_JOB.0),
                        Some(env.as_ptr().cast()),
                        current,
                        &startup.StartupInfo,
                        &mut info,
                    )?;
                } else if let Some(token) = &token {
                    CreateProcessAsUserW(
                        Some(raw(token)),
                        PCWSTR(program.as_ptr()),
                        Some(PWSTR(line.as_mut_ptr())),
                        None,
                        None,
                        stdio.is_some(),
                        flags,
                        Some(env.as_ptr().cast()),
                        current,
                        &startup.StartupInfo,
                        &mut info,
                    )?;
                } else {
                    CreateProcessW(
                        PCWSTR(program.as_ptr()),
                        Some(PWSTR(line.as_mut_ptr())),
                        None,
                        None,
                        stdio.is_some(),
                        flags,
                        Some(env.as_ptr().cast()),
                        current,
                        &startup.StartupInfo,
                        &mut info,
                    )?;
                }
                Ok(())
            };
            // The child's pipe ends are inheritable only while the process is being created.
            let inheritable = stdio.map(InheritableHandles::new).transpose()?;
            match create(flags) {
                Err(e)
                    if breakaway
                        && e.code()
                            == windows::core::HRESULT::from_win32(ERROR_ACCESS_DENIED.0) =>
                {
                    tracing::debug!(
                        "the job around this host does not allow leaving it: what the command leaves running ends with the host"
                    );
                    create(PROCESS_CREATION_FLAGS(
                        flags.0 & !CREATE_BREAKAWAY_FROM_JOB.0,
                    ))?;
                }
                result => result?,
            }
            drop(inheritable);
            let process = Self {
                handle: owned(info.hProcess),
                job,
                host_stop: None,
                pid: info.dwProcessId,
            };
            let thread = owned(info.hThread);
            if let Err(e) = AssignProcessToJobObject(raw(&process.job), raw(&process.handle)) {
                let _ = TerminateProcess(raw(&process.handle), 1);
                return Err(e.into());
            }
            if ResumeThread(raw(&thread)) == u32::MAX {
                let _ = TerminateProcess(raw(&process.handle), 1);
                bail!("cannot resume the application process");
            }
            Ok(process)
        }
    }
}
