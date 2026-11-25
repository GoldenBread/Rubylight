//! The connection on stdin: who is on the other end, and one bounded line from them.
//!
//! The protocol follows libvirtualdisplay's `vibeshine-vkms` broker (Nonary/libvirtualdisplay,
//! MIT, Copyright (c) 2026 Chase Payne).
use anyhow::{Result, bail};
use std::io::Read;
use std::os::unix::io::RawFd;
use std::time::Duration;

/// The longest request, newline included.
pub const MAX_REQUEST: usize = 128;
/// How long the caller has to send its line.
pub const READ_TIMEOUT: Duration = Duration::from_secs(2);

/// The kernel's account of the caller, taken when it connected.
#[derive(Clone, Debug, Default)]
pub struct Peer {
    pub pid: i32,
    pub uid: u32,
    pub gid: u32,
    /// The caller's supplementary groups (SO_PEERGROUPS).
    pub groups: Vec<u32>,
}

impl Peer {
    /// Read the credentials of the unix socket `fd`.
    pub fn of(fd: RawFd) -> Result<Self> {
        let mut cred = libc::ucred {
            pid: 0,
            uid: u32::MAX,
            gid: u32::MAX,
        };
        let mut size = size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: `cred` and `size` are valid for the SO_PEERCRED read, which writes at most
        // `size` bytes.
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&raw mut cred).cast(),
                &mut size,
            )
        };
        if rc != 0 || cred.uid == u32::MAX {
            bail!(
                "could not read the caller's credentials: {}",
                std::io::Error::last_os_error()
            );
        }
        Ok(Self {
            pid: cred.pid,
            uid: cred.uid,
            gid: cred.gid,
            groups: peer_groups(fd),
        })
    }
}

fn peer_groups(fd: RawFd) -> Vec<u32> {
    let mut groups = vec![0u32; 64];
    for _ in 0..2 {
        let mut size = (groups.len() * size_of::<u32>()) as libc::socklen_t;
        // SAFETY: the buffer holds `size` bytes; the kernel writes at most that many and sets
        // `size` to the bytes needed (ERANGE) or written.
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERGROUPS,
                groups.as_mut_ptr().cast(),
                &mut size,
            )
        };
        let count = size as usize / size_of::<u32>();
        if rc == 0 {
            groups.truncate(count);
            return groups;
        }
        if std::io::Error::last_os_error().raw_os_error() == Some(libc::ERANGE) {
            groups.resize(count, 0);
        } else {
            break;
        }
    }
    Vec::new()
}

/// Make reads on `fd` fail after `timeout` (SO_RCVTIMEO).
pub fn set_read_timeout(fd: RawFd, timeout: Duration) -> Result<()> {
    let tv = libc::timeval {
        tv_sec: timeout.as_secs() as libc::time_t,
        tv_usec: timeout.subsec_micros() as libc::suseconds_t,
    };
    // SAFETY: `tv` is a valid timeval for the SO_RCVTIMEO option and lives through the call.
    let rc = unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            (&raw const tv).cast(),
            size_of::<libc::timeval>() as libc::socklen_t,
        )
    };
    if rc != 0 {
        bail!(
            "could not set the read timeout: {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(())
}

/// One line of at most [`MAX_REQUEST`] bytes (newline included), without the newline. None for
/// a timeout, a closed connection, no newline in time, or bytes that are not plain text.
pub fn read_line(from: &mut impl Read) -> Option<String> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match from.read(&mut byte) {
            Ok(1) => {}
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            _ => return None,
        }
        if byte[0] == b'\n' {
            break;
        }
        if line.len() + 1 >= MAX_REQUEST {
            return None;
        }
        line.push(byte[0]);
    }
    let text = String::from_utf8(line).ok()?;
    (!text.chars().any(char::is_control)).then_some(text)
}
