//! Owned child process lifecycle control, job queries, and supervisor stop events.
use super::{Process, inspect::processes, owned, raw, tokens::token_elevated};
use anyhow::{Context, Result, bail};
use std::{
    mem::size_of,
    os::windows::io::{AsRawHandle, OwnedHandle},
    time::Duration,
};
use windows::{
    Win32::{
        Foundation::*,
        Security::*,
        System::{JobObjects::*, Threading::*},
    },
    core::{BOOL, PCWSTR},
};

impl Process {
    /// Whether Windows gave this process administrator rights: its token,
    /// which is not always what its [`super::Target`] asked for (an administrator
    /// with User Account Control off has only their full token).
    pub fn elevated(&self) -> Result<bool> {
        // SAFETY: `self.handle` is the child's owned process handle, and
        // `token` a valid out-pointer whose handle is owned once filled.
        let token = unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(raw(&self.handle), TOKEN_QUERY, &mut token)
                .context("opening the process's token")?;
            owned(token)
        };
        token_elevated(&token)
    }
    /// Import an unnamed resource from this owned child. The child never gets
    /// a handle to the service process or its credentials.
    pub fn duplicate_resource(&self, handle: usize) -> Result<OwnedHandle> {
        anyhow::ensure!(
            handle != 0 && handle != usize::MAX,
            "invalid child resource handle"
        );
        // SAFETY: `self.handle` is the child's owned process handle and the duplicate is owned
        // once.
        unsafe {
            let mut duplicate = HANDLE::default();
            DuplicateHandle(
                raw(&self.handle),
                HANDLE(handle as *mut _),
                GetCurrentProcess(),
                &mut duplicate,
                0,
                false,
                DUPLICATE_SAME_ACCESS,
            )?;
            Ok(owned(duplicate))
        }
    }
    /// A handle of this process duplicated into the child, as the value the
    /// child uses; the child's handle closes when it exits.
    pub fn duplicate_into(&self, handle: HANDLE) -> Result<u64> {
        // SAFETY: `self.handle` is the child's owned process handle and `handle` is valid in this
        // process.
        unsafe {
            let mut duplicate = HANDLE::default();
            DuplicateHandle(
                GetCurrentProcess(),
                handle,
                raw(&self.handle),
                &mut duplicate,
                0,
                false,
                DUPLICATE_SAME_ACCESS,
            )?;
            Ok(duplicate.0 as u64)
        }
    }
    pub fn exit_code(&self) -> Result<Option<u32>> {
        // SAFETY: `self.handle` is an owned process handle that stays open while `self` lives.
        unsafe {
            if WaitForSingleObject(raw(&self.handle), 0) == WAIT_TIMEOUT {
                return Ok(None);
            }
            let mut code = 0;
            GetExitCodeProcess(raw(&self.handle), &mut code)?;
            Ok(Some(code))
        }
    }
    pub fn active_processes(&self) -> Result<u32> {
        // SAFETY: `self.job` is an owned job handle and `info` is writable for the size passed.
        unsafe {
            let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
            QueryInformationJobObject(
                Some(raw(&self.job)),
                JobObjectBasicAccountingInformation,
                (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                None,
            )?;
            Ok(info.ActiveProcesses)
        }
    }
    pub fn process_ids(&self) -> Result<Vec<u32>> {
        let mut words = vec![0usize; 4098];
        let list = words.as_mut_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>();
        // SAFETY: `words` is usize-aligned, outlives the call and its byte size is passed, and
        // `count` is checked against the 4096 ids it can hold before the slice is made.
        unsafe {
            QueryInformationJobObject(
                Some(raw(&self.job)),
                JobObjectBasicProcessIdList,
                list.cast(),
                (words.len() * size_of::<usize>()) as u32,
                None,
            )?;
            let count = (*list).NumberOfProcessIdsInList as usize;
            if count > 4096 {
                bail!("application process group exceeds its limit");
            }
            Ok(
                std::slice::from_raw_parts((*list).ProcessIdList.as_ptr(), count)
                    .iter()
                    .filter_map(|id| u32::try_from(*id).ok())
                    .collect(),
            )
        }
    }
    pub fn wait(&self, timeout: Duration) -> Result<u32> {
        // SAFETY: `self.handle` is an owned process handle that stays open while `self` lives.
        unsafe {
            match WaitForSingleObject(
                raw(&self.handle),
                timeout.as_millis().min(u32::MAX as u128) as u32,
            ) {
                WAIT_OBJECT_0 => {}
                WAIT_TIMEOUT => bail!("process timed out"),
                _ => return Err(windows::core::Error::from_thread().into()),
            }
        }
        self.exit_code()?.context("process still running")
    }
    /// Wait without polling; cancellation terminates this command's entire
    /// job before the owner can run its undo commands.
    pub fn wait_cancellable(
        &self,
        timeout: Duration,
        cancel: &CommandCancellation,
    ) -> Result<Option<u32>> {
        // SAFETY: Both owned handles remain live for the synchronous wait.
        let result = unsafe {
            WaitForMultipleObjects(
                &[raw(&cancel.0.0), raw(&self.handle)],
                false,
                timeout.as_millis().min(u32::MAX as u128) as u32,
            )
        };
        if result == WAIT_OBJECT_0 {
            self.stop()?;
            Ok(None)
        } else if result == WAIT_EVENT(WAIT_OBJECT_0.0 + 1) {
            self.exit_code()?.context("process still running").map(Some)
        } else if result == WAIT_TIMEOUT {
            self.stop()?;
            bail!("process timed out")
        } else {
            Err(windows::core::Error::from_thread().into())
        }
    }
    /// Waits, up to `timeout`, until a GUI process started here is pumping
    /// messages. False when it timed out or the process has no message loop.
    pub fn wait_input_idle(&self, timeout: Duration) -> bool {
        // SAFETY: `self.handle` is an owned process handle that stays open while `self` lives.
        unsafe {
            WaitForInputIdle(
                raw(&self.handle),
                timeout.as_millis().min(u32::MAX as u128) as u32,
            ) == 0
        }
    }
    /// Take the kill-on-close limit off the job, so the processes the command started and left
    /// running outlive this `Process` (issue 135). For a command that ended by itself; one that
    /// is stopped, timed out or whose wait failed is dropped without it and takes its tree along.
    pub fn leave_children_running(&self) -> Result<()> {
        let limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
            BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
                LimitFlags: JOB_OBJECT_LIMIT_BREAKAWAY_OK,
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: `self.job` is an owned job handle, and `limits` is a live local whose size is
        // passed.
        unsafe {
            SetInformationJobObject(
                raw(&self.job),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )?;
        }
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
    /// [`Self::leave_children_running`]; when Windows refuses, the programs end with `self` as
    /// before.
    pub fn leave_children_running_or_warn(&self) {
        if let Err(error) = self.leave_children_running() {
            tracing::warn!(%error, pid = self.pid, "what the command started ends with it");
        }
    }
    /// Ask Windows to end the whole job, without waiting for it to be gone.
    pub fn terminate(&self) -> Result<()> {
        // SAFETY: `self.job` is an owned job handle that stays open while `self` lives.
        unsafe { TerminateJobObject(raw(&self.job), 0)? };
        Ok(())
    }
    pub fn stop(&self) -> Result<()> {
        // SAFETY: `self.job` is an owned job handle that stays open while `self` lives.
        unsafe {
            TerminateJobObject(raw(&self.job), 0)?;
        }
        self.wait(Duration::from_secs(10))?;
        Ok(())
    }
    pub fn stop_graceful(&self, timeout: Duration) -> Result<()> {
        if timeout.is_zero() || self.active_processes()? == 0 {
            return self.stop();
        }
        if self.close_windows(self.process_ids()?.into_iter().collect())? {
            let deadline = std::time::Instant::now() + timeout;
            while self.active_processes()? != 0 && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        self.stop()
    }
    /// The job's processes whose image is `image`: the one it started, or
    /// one that restarted itself.
    pub(super) fn image_ids(&self, image: &str) -> Result<std::collections::BTreeSet<u32>> {
        let job: std::collections::BTreeSet<u32> = self.process_ids()?.into_iter().collect();
        Ok(processes()?
            .into_iter()
            .filter(|p| job.contains(&p.pid) && p.name.eq_ignore_ascii_case(image))
            .map(|p| p.pid)
            .collect())
    }
    /// Whether a process of the job whose image is `image` still runs.
    pub fn running_image(&self, image: &str) -> Result<bool> {
        Ok(!self.image_ids(image)?.is_empty())
    }
    /// Ask the windows of the job's `image` processes to close and wait up to
    /// `timeout` for them to exit. Nothing is ended, and the job's other
    /// processes (a browser or a game they started) are left alone: whether
    /// they all exited.
    pub fn close_image(&self, image: &str, timeout: Duration) -> Result<bool> {
        let ids = self.image_ids(image)?;
        if ids.is_empty() {
            return Ok(true);
        }
        if self.close_windows(ids)? {
            let deadline = std::time::Instant::now() + timeout;
            while self.running_image(image)? && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        Ok(!self.running_image(image)?)
    }
    /// Post WM_CLOSE to each top-level window of the processes `ids`: whether
    /// one was sent.
    fn close_windows(&self, ids: std::collections::BTreeSet<u32>) -> Result<bool> {
        use windows::Win32::UI::WindowsAndMessaging::*;
        struct Closing {
            ids: std::collections::BTreeSet<u32>,
            sent: bool,
        }
        unsafe extern "system" fn close_window(window: HWND, data: LPARAM) -> BOOL {
            // SAFETY: EnumWindows passes the `&mut Closing` from close_windows as `data`, which
            // outlives the enumeration and is not otherwise used during it.
            unsafe {
                let closing = &mut *(data.0 as *mut Closing);
                let mut pid = 0;
                GetWindowThreadProcessId(window, Some(&mut pid));
                if closing.ids.contains(&pid)
                    && PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0)).is_ok()
                {
                    closing.sent = true;
                }
                BOOL(1)
            }
        }
        let mut closing = Closing { ids, sent: false };
        // SAFETY: `closing` outlives the synchronous EnumWindows call that reads it through
        // `close_window`.
        unsafe {
            EnumWindows(
                Some(close_window),
                LPARAM((&mut closing as *mut Closing) as isize),
            )?;
        }
        Ok(closing.sent)
    }
    pub fn shutdown_host(&self) -> Result<HostShutdown> {
        if let Some(code) = self.exit_code()? {
            return Ok(HostShutdown::Graceful(code));
        }
        self.host_stop
            .as_ref()
            .context("host has no supervisor stop channel")?
            .request()?;
        match self.wait(Duration::from_secs(20)) {
            Ok(code) => Ok(HostShutdown::Graceful(code)),
            Err(error) => {
                tracing::warn!(pid=self.pid, %error, "host shutdown timed out; terminating its owned job");
                self.stop()?;
                Ok(HostShutdown::Forced)
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum HostShutdown {
    Graceful(u32),
    Forced,
}

/// A shared, manual-reset event for cancelling owned application commands.
pub struct CommandCancellation(pub(super) HostStop);
impl CommandCancellation {
    pub fn new() -> Result<Self> {
        Ok(Self(HostStop::new()?))
    }
    pub fn cancel(&self) -> Result<()> {
        self.0.request()
    }
    pub fn requested(&self) -> bool {
        // SAFETY: The event is owned for this nonblocking state check.
        unsafe { WaitForSingleObject(raw(&self.0.0), 0) == WAIT_OBJECT_0 }
    }
}

pub(super) struct HostStop(pub(super) OwnedHandle);
impl HostStop {
    pub(super) fn new() -> Result<Self> {
        // SAFETY: CreateEventW returns a new event handle that this struct then owns.
        Ok(Self(owned(unsafe {
            CreateEventW(None, true, false, PCWSTR::null())?
        })))
    }
    pub(super) fn source(&self) -> String {
        format!(
            "{}:{:x}",
            // SAFETY: GetCurrentProcessId has no preconditions.
            unsafe { GetCurrentProcessId() },
            self.0.as_raw_handle() as usize
        )
    }
    pub(super) fn request(&self) -> Result<()> {
        // SAFETY: `self.0` is the owned event handle, open while `self` lives.
        unsafe {
            SetEvent(raw(&self.0))?;
        }
        Ok(())
    }
}

pub(super) fn parse_stop_source(source: &str) -> Result<(u32, usize)> {
    let (pid, handle) = source
        .split_once(':')
        .context("invalid supervisor stop source")?;
    if pid.is_empty()
        || !pid.bytes().all(|b| b.is_ascii_digit())
        || handle.is_empty()
        || !handle.bytes().all(|b| b.is_ascii_hexdigit())
    {
        bail!("invalid supervisor stop source");
    }
    let pid: u32 = pid.parse()?;
    let handle = usize::from_str_radix(handle, 16)?;
    if pid == 0 || handle == 0 || handle == usize::MAX {
        bail!("invalid supervisor stop source");
    }
    Ok((pid, handle))
}

fn parent_pid() -> Result<u32> {
    use windows::Win32::System::Diagnostics::ToolHelp::*;
    // SAFETY: `entry` has dwSize set to its size and the owned snapshot handle is open for every
    // call.
    unsafe {
        let snapshot = owned(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)?);
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        Process32FirstW(raw(&snapshot), &mut entry)?;
        loop {
            if entry.th32ProcessID == GetCurrentProcessId() {
                return Ok(entry.th32ParentProcessID);
            }
            Process32NextW(raw(&snapshot), &mut entry)
                .context("host parent process unavailable")?;
        }
    }
}

pub(super) fn duplicate_stop_event(pid: u32, handle: usize) -> Result<OwnedHandle> {
    // SAFETY: the parent process handle is owned for the call, DuplicateHandle validates `handle`
    // in that process, and the copy is owned once.
    unsafe {
        let parent = owned(OpenProcess(PROCESS_DUP_HANDLE, false, pid)?);
        let mut duplicate = HANDLE::default();
        DuplicateHandle(
            raw(&parent),
            HANDLE(handle as *mut _),
            GetCurrentProcess(),
            &mut duplicate,
            SYNCHRONIZATION_SYNCHRONIZE.0,
            false,
            DUPLICATE_HANDLE_OPTIONS(0),
        )?;
        Ok(owned(duplicate))
    }
}

pub struct StopSignal(pub(super) Option<OwnedHandle>);
impl StopSignal {
    pub fn new(source: Option<&str>) -> Result<Self> {
        let Some(source) = source else {
            return Ok(Self(None));
        };
        let (pid, handle) = parse_stop_source(source)?;
        if pid != parent_pid()? {
            bail!("stop source is not this host's supervisor");
        }
        Ok(Self(Some(
            duplicate_stop_event(pid, handle).context("connecting supervisor stop event")?,
        )))
    }
    pub fn requested(&self) -> bool {
        self.0
            .as_ref()
            // SAFETY: `event` is an owned handle borrowed from `self`, open for the wait.
            .is_some_and(|event| unsafe { WaitForSingleObject(raw(event), 0) == WAIT_OBJECT_0 })
    }
    /// Whether a supervisor can ask for the stop.
    pub fn supervised(&self) -> bool {
        self.0.is_some()
    }
    /// Block until the supervisor asks for the stop, or the wait fails. Without a supervisor
    /// nothing can ask, so this returns at once.
    pub fn wait(&self) {
        if let Some(event) = &self.0 {
            // SAFETY: `event` is an owned handle borrowed from `self`, open for the wait.
            unsafe { WaitForSingleObject(raw(event), INFINITE) };
        }
    }
}
