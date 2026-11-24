//! WebRTC's datagrams on Linux: runs of equal-sized datagrams to one peer, each run in one
//! `sendmsg` with UDP segmentation (`UDP_SEGMENT`).
use super::{
    batch::NO_SEGMENTATION,
    sys::{Ancillary, Errno, send_message},
};
use std::{
    io::IoSlice,
    net::{SocketAddr, UdpSocket},
};

/// Datagrams of any size from a plain socket to one peer, in order and in as
/// few system calls as UDP segmentation allows: each run of equal-sized
/// datagrams, the last of a run maybe shorter, goes in one `sendmsg`. For
/// WebRTC, whose RTP packets are equal but for each frame's last and whose
/// other traffic is any size. Without segmentation they go one by one.
pub struct Segments {
    pub(super) offload: bool,
    pub system_calls: u64,
}
impl Default for Segments {
    fn default() -> Self {
        Self {
            offload: true,
            system_calls: 0,
        }
    }
}
/// What became of one [`Segments::send`].
#[derive(Debug, Default)]
pub struct Sent {
    pub datagrams: usize,
    pub bytes: usize,
    /// Datagrams lost to send errors, and the last such error.
    pub failed: usize,
    pub error: Option<std::io::Error>,
}
impl Segments {
    /// How many of the leading `packets` one segmented send carries: equal
    /// sizes, then at most one shorter, at most 64 and 65507 bytes in all.
    /// An empty datagram is never in a run: segmentation would not send it.
    pub fn run<T: AsRef<[u8]>>(packets: &[T]) -> usize {
        let Some(size) = packets.first().map(|p| p.as_ref().len()) else {
            return 0;
        };
        if size == 0 {
            return 1;
        }
        let (mut count, mut total) = (0, 0);
        for len in packets.iter().take(64).map(|p| p.as_ref().len()) {
            if len > size || len == 0 || total + len > 65507 {
                break;
            }
            count += 1;
            total += len;
            if len < size {
                break;
            }
        }
        count.max(1)
    }
    /// Send `packets` to `peer`. An error loses only the datagrams it hit:
    /// the caller counts them and goes on.
    pub fn send<T: AsRef<[u8]>>(
        &mut self,
        socket: &UdpSocket,
        packets: &[T],
        peer: SocketAddr,
    ) -> Sent {
        let mut sent = Sent::default();
        let mut rest = packets;
        while !rest.is_empty() {
            let (run, tail) = rest.split_at(if self.offload { Self::run(rest) } else { 1 });
            rest = tail;
            if run.len() > 1 {
                let expected: usize = run.iter().map(|p| p.as_ref().len()).sum();
                match self.send_run(socket, run, peer) {
                    Ok(bytes) if bytes == expected => {
                        sent.datagrams += run.len();
                        sent.bytes += bytes;
                        continue;
                    }
                    // A datagram socket sends all or nothing; a short count
                    // would be a kernel bug. Loud, counted as lost, and not
                    // sent again: some of it may have gone.
                    Ok(bytes) => {
                        tracing::warn!(bytes, expected, "Linux sent part of a UDP batch");
                        sent.failed += run.len();
                        sent.error = Some(std::io::Error::other("Linux sent part of a UDP batch"));
                        continue;
                    }
                    Err(error) if NO_SEGMENTATION.contains(&error) => {
                        self.offload = false;
                        tracing::warn!(
                            code = error,
                            "UDP segmentation unavailable; WebRTC sends individual datagrams with more CPU overhead. Update the kernel or the network driver if sending stalls"
                        );
                    }
                    // Nothing of the run went. One by one, as before
                    // segmentation, a full buffer or a route that comes back
                    // loses only the datagrams it hits.
                    Err(_) => {}
                }
            }
            for packet in run {
                self.system_calls += 1;
                match socket.send_to(packet.as_ref(), peer) {
                    Ok(_) => {
                        sent.datagrams += 1;
                        sent.bytes += packet.as_ref().len();
                    }
                    Err(error) => {
                        sent.failed += 1;
                        sent.error = Some(error);
                    }
                }
            }
        }
        sent
    }
    /// One `sendmsg` of a [`Segments::run`], segmented at its first
    /// datagram's size. The error is the OS's number.
    fn send_run<T: AsRef<[u8]>>(
        &mut self,
        socket: &UdpSocket,
        packets: &[T],
        peer: SocketAddr,
    ) -> Result<usize, Errno> {
        // A run is at most 65507 bytes, so its first datagram's size fits.
        let Some(size) = packets
            .first()
            .and_then(|p| u16::try_from(p.as_ref().len()).ok())
        else {
            return Err(libc::EMSGSIZE);
        };
        let mut control = Ancillary::default();
        control.segment(size);
        let mut buffers = [IoSlice::new(&[]); 64];
        for (buffer, packet) in buffers.iter_mut().zip(packets) {
            *buffer = IoSlice::new(packet.as_ref());
        }
        self.system_calls += 1;
        send_message(
            socket,
            &buffers[..packets.len().min(64)],
            &socket2::SockAddr::from(peer),
            &control,
        )
    }
}
