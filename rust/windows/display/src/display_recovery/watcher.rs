//! Out-of-process display recovery watchdog, named journal mutex, and process identity checks.

use super::{Journal, recover};
use anyhow::{Context, Result, bail};
use std::{
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::Path,
    process::{Child, Command},
    sync::Mutex,
};
use windows::Win32::{Foundation::*, System::Threading::*};

static WATCH: Mutex<Option<(Child, OwnedHandle)>> = Mutex::new(None);

pub(super) fn watch_done(pid: u32) -> Result<OwnedHandle> {
    let name: Vec<u16> = format!("Local\\Butterpollo.DisplayWatch.{pid}\0")
        .encode_utf16()
        .collect();
    // SAFETY: The name is NUL-terminated, and the returned handle is owned exactly once.
    unsafe {
        let event = CreateEventW(None, true, false, windows::core::PCWSTR(name.as_ptr()))?;
        Ok(OwnedHandle::from_raw_handle(event.0))
    }
}

pub(super) fn finish_watch(path: &Path) -> Result<()> {
    let guard = lock(path)?;
    let journal: Journal = serde_json::from_slice(&std::fs::read(path)?)?;
    if !journal.entries.is_empty() {
        return Ok(());
    }
    let mut watch = crate::lock_unpoisoned(&WATCH);
    let Some((process, event)) = watch.as_mut() else {
        return Ok(());
    };
    // SAFETY: The owned event remains live until the watcher exits.
    unsafe { SetEvent(HANDLE(event.as_raw_handle()))? };
    drop(guard);
    // Keep WATCH locked so a new watcher cannot reset this event before the old
    // watcher observes it. Native process exit must not hold the journal lock.
    wait_for_watch(process)?;
    watch.take();
    Ok(())
}

/// How long a killed watcher may take to go. Termination is asynchronous and
/// on a busy PC took longer than 100 ms; only a stalled watcher waits this.
pub(super) const KILLED_WATCH_EXIT_MS: u32 = 2000;

pub(super) fn wait_for_watch(process: &mut Child) -> Result<()> {
    let handle = HANDLE(process.as_raw_handle());
    stop_watch(
        // SAFETY: Child owns the process handle for the entire bounded wait,
        // and still after requesting termination.
        |ms| unsafe { WaitForSingleObject(handle, ms) },
        || Ok(process.kill()?),
    )
}

/// [`wait_for_watch`] with the process wait and kill passed in.
pub(super) fn stop_watch(
    mut wait: impl FnMut(u32) -> WAIT_EVENT,
    kill: impl FnOnce() -> Result<()>,
) -> Result<()> {
    match wait(100) {
        WAIT_OBJECT_0 => {}
        WAIT_TIMEOUT => {
            // Its journal is empty; a stalled watcher has nothing left to restore.
            tracing::warn!("display recovery helper did not exit within 100 ms; stopping it");
            kill()?;
            // Termination is asynchronous. Keep the signaled watcher registered
            // unless it exits, so a new watcher cannot reuse its named event.
            match wait(KILLED_WATCH_EXIT_MS) {
                WAIT_OBJECT_0 => {}
                WAIT_TIMEOUT => bail!(
                    "display recovery helper did not stop within {KILLED_WATCH_EXIT_MS} ms of being killed"
                ),
                _ => return Err(windows::core::Error::from_thread().into()),
            }
        }
        _ => return Err(windows::core::Error::from_thread().into()),
    }
    Ok(())
}

pub(super) struct JournalLock(HANDLE);
impl Drop for JournalLock {
    fn drop(&mut self) {
        // SAFETY: lock() acquired this mutex on the current thread, and its local
        // guard owns the handle until this single release and close.
        unsafe {
            let _ = ReleaseMutex(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}

pub(super) fn lock(path: &Path) -> Result<JournalLock> {
    let hash = butterpollo_crypto::crypto::hash(path.to_string_lossy().to_lowercase().as_bytes());
    let name: Vec<u16> = format!(
        "Local\\Butterpollo.DisplayJournal.{:x}\0",
        u64::from_le_bytes(hash[..8].try_into().unwrap())
    )
    .encode_utf16()
    .collect();
    // SAFETY: name is terminated and lives through CreateMutexW. The returned handle
    // is either closed on failure or transferred to a guard after this thread acquires it.
    unsafe {
        let handle = CreateMutexW(None, false, windows::core::PCWSTR(name.as_ptr()))?;
        let result = WaitForSingleObject(handle, 30000);
        if result != WAIT_OBJECT_0 && result != WAIT_ABANDONED {
            let _ = CloseHandle(handle);
            bail!("display journal is busy");
        }
        Ok(JournalLock(handle))
    }
}

pub(super) fn identity(pid: u32) -> Result<Option<u64>> {
    // SAFETY: OpenProcess supplies an owned query handle; the four initialized outputs
    // remain writable for GetProcessTimes, and the handle is closed exactly once afterward.
    unsafe {
        if pid == 0 {
            return Ok(None);
        }
        let process = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(handle) => handle,
            Err(e) if e.code() == windows::core::HRESULT::from_win32(ERROR_INVALID_PARAMETER.0) => {
                return Ok(None);
            }
            Err(e) => return Err(e.into()),
        };
        let (mut created, mut exited, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        let result = GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user);
        let _ = CloseHandle(process);
        result?;
        if exited.dwLowDateTime != 0 || exited.dwHighDateTime != 0 {
            return Ok(None);
        }
        Ok(Some(
            u64::from(created.dwLowDateTime) | (u64::from(created.dwHighDateTime) << 32),
        ))
    }
}

pub(super) fn watch(path: &Path) -> Result<()> {
    use std::os::windows::process::CommandExt;
    let mut child = crate::lock_unpoisoned(&WATCH);
    if let Some((process, event)) = child.as_mut()
        && process.try_wait()?.is_none()
    {
        // SAFETY: The watcher entry owns the event for this nonblocking check.
        if unsafe { WaitForSingleObject(HANDLE(event.as_raw_handle()), 0) } == WAIT_OBJECT_0 {
            bail!("display recovery helper is still stopping");
        }
        return Ok(());
    }
    let done = watch_done(std::process::id())?;
    // SAFETY: This new watcher owns the event until it exits; the previous child is already done.
    unsafe { ResetEvent(HANDLE(done.as_raw_handle()))? };
    let spawn = |flags: u32| -> Result<std::process::Child> {
        Ok(Command::new(std::env::current_exe()?)
            .arg("--display-watch")
            .arg(std::process::id().to_string())
            .arg("--config-dir")
            .arg(path.parent().context("journal directory missing")?)
            .creation_flags(flags)
            .spawn()?)
    };
    // Escape the host's job, otherwise a service stop would also kill recovery.
    // A job that forbids breaking away (a launcher, a terminal) refuses that
    // with access denied, and every frame limit and display change failed on
    // it; the helper then stays in the job and still covers a host crash.
    let started = spawn(CREATE_BREAKAWAY_FROM_JOB.0 | CREATE_NO_WINDOW.0).or_else(|error| {
        tracing::debug!(%error, "display recovery helper cannot leave the host's job");
        spawn(CREATE_NO_WINDOW.0)
    });
    *child = Some((
        started.context("cannot start the display recovery helper")?,
        done,
    ));
    Ok(())
}

pub fn wait_and_recover(pid: u32, directory: &Path) -> Result<()> {
    let path = directory.join("display-recovery.json");
    let owner: Journal = if path.exists() {
        serde_json::from_slice(&std::fs::read(&path)?)?
    } else {
        Journal::default()
    };
    if owner.pid != pid {
        return Ok(());
    }
    let done = watch_done(pid)?;
    // SAFETY: The successfully opened process handle has synchronization rights and
    // remains owned until the wait finishes; every exit path closes it exactly once.
    unsafe {
        match OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            pid,
        ) {
            Ok(process) => {
                let identity = identity(pid);
                if identity.is_err() {
                    let _ = CloseHandle(process);
                }
                let result = if identity? == Some(owner.started) {
                    WaitForMultipleObjects(
                        &[process, HANDLE(done.as_raw_handle())],
                        false,
                        u32::MAX,
                    )
                } else {
                    WAIT_OBJECT_0
                };
                let _ = CloseHandle(process);
                if result == WAIT_EVENT(WAIT_OBJECT_0.0 + 1) {
                    return Ok(());
                }
                if result != WAIT_OBJECT_0 {
                    bail!("display watcher wait failed");
                }
            }
            Err(e) if e.code() == windows::core::HRESULT::from_win32(ERROR_INVALID_PARAMETER.0) => {
            }
            Err(e) => return Err(e.into()),
        }
    }
    let _guard = lock(&path)?;
    if path.exists() {
        let journal: Journal = serde_json::from_slice(&std::fs::read(&path)?)?;
        // A newly started host may already have reclaimed the journal.
        if journal.pid != pid {
            return Ok(());
        }
    }
    recover(&path)
}
