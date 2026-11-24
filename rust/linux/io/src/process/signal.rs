//! Process descriptors, waiting on them and signalling: how a started tree and a tracked set of processes are waited for and stopped. Linux has no message that asks a program's windows to close, so the polite request is `SIGTERM`.

use super::listing;
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd},
    time::{Duration, Instant},
};

/// A descriptor of the process `pid` (`pidfd_open`, Linux 5.3): it keeps naming that
/// process after its ID went to another, and reads as ready once the process has exited.
pub(super) fn pidfd_open(pid: u32) -> io::Result<OwnedFd> {
    let pid = libc::pid_t::try_from(pid).map_err(io::Error::other)?;
    // SAFETY: pidfd_open takes a process ID and flags, no pointers.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = RawFd::try_from(fd).map_err(io::Error::other)?;
    // SAFETY: the call returned a new descriptor that nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Waits up to `timeout` until one of `fds` reads as ready: which do, all false when the
/// time ran out.
pub(super) fn readable<const N: usize>(
    fds: [BorrowedFd<'_>; N],
    timeout: Duration,
) -> io::Result<[bool; N]> {
    let mut polled = fds.map(|fd| libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    });
    let deadline = Instant::now() + timeout;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        // Rounded up, so a wait never ends before its time.
        let milliseconds = left.as_nanos().div_ceil(1_000_000);
        let milliseconds = libc::c_int::try_from(milliseconds).unwrap_or(libc::c_int::MAX);
        // SAFETY: `polled` is a live array of N entries, and N is the count passed.
        let ready = unsafe { libc::poll(polled.as_mut_ptr(), N as libc::nfds_t, milliseconds) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if ready > 0 || Instant::now() >= deadline {
            return Ok(polled.map(|entry| entry.revents != 0));
        }
    }
}

/// Sends `signal` to the process `pid` if `same` still says it is the one meant, asked
/// once the process is held by a descriptor: an ID that went to another process meanwhile
/// is never signalled. Without process descriptors the signal goes to the ID right after
/// the check.
pub(super) fn signal_if(pid: u32, signal: libc::c_int, same: impl FnOnce() -> bool) {
    let held = pidfd_open(pid);
    if matches!(&held, Err(error) if error.raw_os_error() == Some(libc::ESRCH)) || !same() {
        return;
    }
    match held {
        Ok(held) => {
            // SAFETY: `held` is an open process descriptor; a null info sends a plain signal.
            unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    held.as_raw_fd(),
                    signal,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                );
            }
        }
        Err(_) => {
            if let Ok(pid) = libc::pid_t::try_from(pid)
                && pid > 0
            {
                // SAFETY: kill takes a process ID and a signal number, no pointers.
                unsafe { libc::kill(pid, signal) };
            }
        }
    }
}

/// Sends `signal` to every running process of the tree `root` leads
/// ([`listing::tree`]). `SIGKILL` goes out again to what the tree started meanwhile,
/// until a pass finds nothing new.
pub(super) fn signal_tree(root: u32, signal: libc::c_int) {
    let mut signalled = BTreeSet::new();
    for _ in 0..16 {
        let mut new = false;
        for (pid, start_ticks) in listing::tree(root) {
            if signalled.insert((pid, start_ticks)) {
                new = true;
                signal_if(pid, signal, || listing::same_process(pid, start_ticks));
            }
        }
        if !new || signal != libc::SIGKILL {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Ask `processes` (pid → start time) to end (`SIGTERM`), wait up to `timeout` for them
/// to exit, then end the rest (`SIGKILL`). An ID that now belongs to another process is
/// left alone.
pub fn stop_processes(processes: &BTreeMap<u32, u64>, timeout: Duration) {
    let same = |pid: u32, started: u64| started != 0 && listing::creation_time(pid) == started;
    let alive = || processes.iter().any(|(pid, started)| same(*pid, *started));
    if !alive() {
        return;
    }
    for (&pid, &started) in processes {
        signal_if(pid, libc::SIGTERM, || same(pid, started));
    }
    let deadline = Instant::now() + timeout;
    while alive() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    for (&pid, &started) in processes {
        signal_if(pid, libc::SIGKILL, || same(pid, started));
    }
}
