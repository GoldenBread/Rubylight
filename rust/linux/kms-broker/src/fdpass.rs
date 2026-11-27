//! One packet with descriptors attached (SCM_RIGHTS), over a sequenced-packet unix socket.
use crate::protocol::{MAX_FDS, MAX_MESSAGE};
use anyhow::{Result, bail};
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};

/// The control buffer for [`MAX_FDS`] descriptors, aligned for `cmsghdr`.
#[repr(C, align(8))]
struct Control([u8; 64]);

const _: () = assert!(size_of::<Control>() >= 16 + MAX_FDS * 4);

/// Sends `bytes` with `fds` attached.
pub fn send(socket: BorrowedFd<'_>, bytes: &[u8], fds: &[BorrowedFd<'_>]) -> Result<()> {
    if fds.len() > MAX_FDS || bytes.len() > MAX_MESSAGE {
        bail!(
            "a message of {} bytes and {} descriptors is too large",
            bytes.len(),
            fds.len()
        );
    }
    let mut iov = libc::iovec {
        iov_base: bytes.as_ptr().cast_mut().cast(),
        iov_len: bytes.len(),
    };
    let mut control = Control([0; 64]);
    // SAFETY: plain data; zero is a valid start.
    let mut header: libc::msghdr = unsafe { std::mem::zeroed() };
    header.msg_iov = &raw mut iov;
    header.msg_iovlen = 1;
    if !fds.is_empty() {
        let raw: Vec<RawFd> = fds.iter().map(|fd| fd.as_raw_fd()).collect();
        let data = size_of_val(raw.as_slice()) as u32;
        // SAFETY: CMSG_SPACE and CMSG_LEN only compute sizes.
        let (space, length) = unsafe { (libc::CMSG_SPACE(data), libc::CMSG_LEN(data)) };
        header.msg_control = control.0.as_mut_ptr().cast();
        header.msg_controllen = space as usize;
        // SAFETY: the control buffer holds `space` bytes (checked against MAX_FDS above), so
        // the first header and its data fit.
        unsafe {
            let cmsg = libc::CMSG_FIRSTHDR(&raw const header);
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = length as usize;
            std::ptr::copy_nonoverlapping(
                raw.as_ptr(),
                libc::CMSG_DATA(cmsg).cast::<RawFd>(),
                raw.len(),
            );
        }
    }
    loop {
        // SAFETY: the header points at live buffers for the call.
        let sent =
            unsafe { libc::sendmsg(socket.as_raw_fd(), &raw const header, libc::MSG_NOSIGNAL) };
        if sent >= 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error.into());
        }
    }
}

/// Receives one packet and its descriptors. `Ok(None)` when the other side closed.
pub fn receive(socket: BorrowedFd<'_>) -> std::io::Result<Option<(Vec<u8>, Vec<OwnedFd>)>> {
    let mut buffer = vec![0u8; MAX_MESSAGE];
    let mut iov = libc::iovec {
        iov_base: buffer.as_mut_ptr().cast(),
        iov_len: buffer.len(),
    };
    let mut control = Control([0; 64]);
    // SAFETY: plain data; zero is a valid start.
    let mut header: libc::msghdr = unsafe { std::mem::zeroed() };
    header.msg_iov = &raw mut iov;
    header.msg_iovlen = 1;
    header.msg_control = control.0.as_mut_ptr().cast();
    header.msg_controllen = control.0.len();
    let received = loop {
        // SAFETY: the header points at live buffers for the call.
        let n =
            unsafe { libc::recvmsg(socket.as_raw_fd(), &raw mut header, libc::MSG_CMSG_CLOEXEC) };
        if n >= 0 {
            break n as usize;
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    };
    let mut fds = Vec::new();
    // SAFETY: the kernel filled `header`'s control buffer; the CMSG macros walk it within
    // `msg_controllen`, and each SCM_RIGHTS descriptor is ours to own.
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&raw const header);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let data = (*cmsg).cmsg_len - libc::CMSG_LEN(0) as usize;
                let first = libc::CMSG_DATA(cmsg).cast::<RawFd>();
                for i in 0..data / size_of::<RawFd>() {
                    fds.push(OwnedFd::from_raw_fd(first.add(i).read_unaligned()));
                }
            }
            cmsg = libc::CMSG_NXTHDR(&raw const header, cmsg);
        }
    }
    if header.msg_flags & (libc::MSG_CTRUNC | libc::MSG_TRUNC) != 0 {
        return Err(std::io::Error::other(
            "a message or its descriptors were cut short",
        ));
    }
    if received == 0 && fds.is_empty() {
        return Ok(None);
    }
    buffer.truncate(received);
    Ok(Some((buffer, fds)))
}

/// A connected pair of sequenced-packet sockets, for tests.
pub fn pair() -> std::io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    // SAFETY: `fds` has room for the two descriptors.
    let rc = unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
            0,
            fds.as_mut_ptr(),
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: both descriptors were just made for us.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}
