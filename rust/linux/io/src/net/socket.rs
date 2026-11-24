//! UDP socket setup and waits on Linux: the send buffer, refusing fragmentation, waiting for a
//! datagram, and which errors lose only one datagram. Traffic is not tagged for QoS yet.
use super::{sys::set_option, wait::transient};
use anyhow::Result;
use std::{
    net::{SocketAddr, UdpSocket},
    os::fd::RawFd,
    sync::atomic::{AtomicBool, Ordering},
};

/// Default UDP send buffer applied by [`configure_udp`] (matches WebRTC media).
pub const SEND_BUFFER_DEFAULT: usize = 4 << 20;

/// Prepares a streaming socket: a send buffer of [`SEND_BUFFER_DEFAULT`], which absorbs
/// startup keyframe and initial audio bursts without the non-blocking socket refusing them.
///
/// Linux gives an unprivileged process at most `net.core.wmem_max` (212992 bytes unless the
/// administrator raised it); a host with `CAP_NET_ADMIN` gets the whole buffer. A smaller
/// buffer is not an error: it is said once per run, with the setting to raise.
///
/// An unconnected UDP socket on Linux is never told about the ICMP answer of a closed port, so
/// there is no stale connection reset to turn off as on Windows.
pub fn configure_udp(socket: &UdpSocket) -> Result<()> {
    let wanted = SEND_BUFFER_DEFAULT as i32;
    if set_option(socket, libc::SOL_SOCKET, libc::SO_SNDBUFFORCE, wanted).is_err() {
        socket2::SockRef::from(socket).set_send_buffer_size(SEND_BUFFER_DEFAULT)?;
    }
    // The kernel reports the buffer with its own bookkeeping, twice what was asked for.
    let granted = socket2::SockRef::from(socket).send_buffer_size()?;
    static REPORTED: AtomicBool = AtomicBool::new(false);
    if granted < SEND_BUFFER_DEFAULT && !REPORTED.swap(true, Ordering::Relaxed) {
        tracing::warn!(
            granted,
            wanted = SEND_BUFFER_DEFAULT,
            "the UDP send buffer is smaller than asked for, so a burst of video may be dropped when the socket fills; raise it with `sysctl -w net.core.wmem_max={SEND_BUFFER_DEFAULT}`"
        );
    }
    Ok(())
}

/// Set whether this socket refuses to fragment its later datagrams, so an oversized probe
/// packet fails with `EMSGSIZE` at the sender instead of being fragmented and lost whole
/// on a hop that cannot reassemble it: `IP_MTU_DISCOVER` (`IP_PMTUDISC_DO`, and back to the
/// system's usual `IP_PMTUDISC_WANT`) for IPv4 and `IPV6_DONTFRAG` for IPv6. Both options are
/// tried because a dual-stack socket carries both families; the one the socket does not carry
/// refuses its option harmlessly. The PyroWave probe sets it on its own socket
/// and clears it again; the shared video socket never sets it, since the flag
/// would govern every stream using that process-wide socket.
pub fn set_dont_fragment(socket: &UdpSocket, enabled: bool) -> Result<()> {
    let discover = if enabled {
        libc::IP_PMTUDISC_DO
    } else {
        libc::IP_PMTUDISC_WANT
    };
    let options = [
        (libc::IPPROTO_IP, libc::IP_MTU_DISCOVER, discover),
        (libc::IPPROTO_IPV6, libc::IPV6_DONTFRAG, i32::from(enabled)),
    ];
    let mut applied = false;
    let mut last = None;
    for (level, option, value) in options {
        match set_option(socket, level, option, value) {
            Ok(()) => applied = true,
            // A family the socket does not carry refuses only its own option.
            Err(error)
                if matches!(
                    error.raw_os_error(),
                    Some(
                        libc::ENOPROTOOPT
                            | libc::EINVAL
                            | libc::EOPNOTSUPP
                            | libc::EPROTONOSUPPORT
                    )
                ) =>
            {
                last = Some(error)
            }
            Err(error) => return Err(error.into()),
        }
    }
    match (applied, last) {
        (true, _) => Ok(()),
        (false, Some(error)) => Err(error.into()),
        (false, None) => Err(std::io::Error::from_raw_os_error(libc::EINVAL).into()),
    }
}

/// A send or receive error that loses only this datagram; the socket keeps
/// working. A receive also reports a datagram too large for its buffer this
/// way (`EMSGSIZE`) after discarding it.
pub fn datagram_lost(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
        || error
            .raw_os_error()
            .is_some_and(|code| transient(code) || code == libc::EMSGSIZE)
}

/// Wait until `socket` has a datagram to read, or `timeout_ms` passes;
/// returns whether one is there. Unlike a fixed sleep, input waiting on the
/// socket is handled the moment it arrives.
pub fn wait_readable(socket: RawFd, timeout_ms: i32) -> std::io::Result<bool> {
    wait_readable_sockets([socket], timeout_ms)
}

/// Include host feedback in the same wait as incoming control traffic.
pub fn wait_readable_or_wake(socket: RawFd, wake: RawFd, timeout_ms: i32) -> std::io::Result<bool> {
    wait_readable_sockets([socket, wake], timeout_ms)
}

fn wait_readable_sockets<const N: usize>(
    sockets: [RawFd; N],
    timeout_ms: i32,
) -> std::io::Result<bool> {
    let mut poll = sockets.map(|fd| libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    });
    // SAFETY: `poll` holds N entries and outlives the call.
    let ready = unsafe { libc::poll(poll.as_mut_ptr(), N as libc::nfds_t, timeout_ms) };
    if ready < 0 {
        let error = std::io::Error::last_os_error();
        // A signal ended the wait early: nothing is readable yet.
        return if error.kind() == std::io::ErrorKind::Interrupted {
            Ok(false)
        } else {
            Err(error)
        };
    }
    // A descriptor that is no longer a socket is reported per entry, not as a failed call.
    if poll.iter().any(|entry| entry.revents & libc::POLLNVAL != 0) {
        return Err(std::io::Error::from_raw_os_error(libc::EBADF));
    }
    Ok(ready > 0)
}

/// Nothing to prepare on Linux, where no service tags traffic; false, as on a Windows host
/// without qWAVE.
pub fn warm_up_qos() -> bool {
    false
}

/// A client's datagrams tagged for priority while it lives. On Windows qWAVE tags one client
/// of a socket that several share; Linux marks a whole socket or one message at a time, and
/// this host does neither yet, so no flow is ever made.
pub struct QosFlow(());
impl QosFlow {
    /// Always `None`: there is nothing to tag with, as on a Windows host without qWAVE. The
    /// stream is sent unmarked.
    pub fn new(_socket: &UdpSocket, _peer: SocketAddr, _voice: bool) -> Result<Option<Self>> {
        Ok(None)
    }
}
