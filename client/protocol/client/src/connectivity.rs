//! The client connectivity test (`LiTestClientConnectivity` of moonlight-common-c, which the
//! Android app used from its C library): which of the stream's ports can this device reach on a
//! test server that answers on all of them. Shown after a connection failed, so the player learns
//! which port a firewall or router blocks. Port flags are `ML_PORT_FLAG_*`.

use std::io;
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::thread;
use std::time::Duration;

/// What the test returns when it could not run at all (`ML_TEST_RESULT_INCONCLUSIVE`).
pub const INCONCLUSIVE: u32 = 0xFFFF_FFFF;

/// How long one port gets to answer.
pub const PORT_TIMEOUT: Duration = Duration::from_secs(3);

/// A UDP test packet's size (an MTU's worth, so a path that drops large datagrams shows).
const UDP_TEST_BYTES: usize = 1040;
const UDP_TEST_PACKETS: usize = 3;
const UDP_TEST_GAP: Duration = Duration::from_millis(50);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Tcp,
    Udp,
}

/// The ports with a flag, in flag order: `(flag, transport, port)`.
pub const PORTS: [(u32, Transport, u16); 7] = [
    (0x0001, Transport::Tcp, 47984),
    (0x0002, Transport::Tcp, 47989),
    (0x0004, Transport::Tcp, 48010),
    (0x0100, Transport::Udp, 47998),
    (0x0200, Transport::Udp, 47999),
    (0x0400, Transport::Udp, 48000),
    (0x0800, Transport::Udp, 48010),
];

/// Every flag in [`PORTS`].
pub const VALID_FLAGS: u32 = 0x0001 | 0x0002 | 0x0004 | 0x0100 | 0x0200 | 0x0400 | 0x0800;

/// Tests the ports in `flags` (others are ignored) on `test_server`, resolved with
/// `reference_port`: the flags of the ports that did not answer, 0 when all did, or
/// [`INCONCLUSIVE`] when the server did not resolve.
pub fn test_client_connectivity(test_server: &str, reference_port: u16, flags: u32) -> u32 {
    let flags = flags & VALID_FLAGS;
    if flags == 0 {
        return 0;
    }
    let Some(server) = (test_server, reference_port)
        .to_socket_addrs()
        .ok()
        .and_then(|mut a| a.next())
    else {
        return INCONCLUSIVE;
    };
    let ports: Vec<_> = PORTS
        .iter()
        .copied()
        .filter(|(flag, ..)| flags & flag != 0)
        .collect();
    failing_flags(server.ip(), &ports, PORT_TIMEOUT)
}

/// The flags of `ports` that did not answer on `server`, each tried in its own thread.
pub fn failing_flags(server: IpAddr, ports: &[(u32, Transport, u16)], timeout: Duration) -> u32 {
    thread::scope(|scope| {
        let tests: Vec<_> = ports
            .iter()
            .map(|&(flag, transport, port)| {
                let address = SocketAddr::new(server, port);
                (
                    flag,
                    scope.spawn(move || test_port(address, transport, timeout)),
                )
            })
            .collect();
        tests
            .into_iter()
            .fold(0, |failing, (flag, test)| match test.join() {
                Ok(Ok(())) => failing,
                _ => failing | flag,
            })
    })
}

fn test_port(address: SocketAddr, transport: Transport, timeout: Duration) -> io::Result<()> {
    match transport {
        Transport::Tcp => TcpStream::connect_timeout(&address, timeout).map(drop),
        Transport::Udp => {
            let socket = UdpSocket::bind(match address {
                SocketAddr::V4(_) => "0.0.0.0:0",
                SocketAddr::V6(_) => "[::]:0",
            })?;
            // Connected, so an ICMP error for an earlier packet surfaces as the receive's error.
            socket.connect(address)?;
            socket.set_read_timeout(Some(timeout))?;
            let mut packet = [0u8; UDP_TEST_BYTES];
            packet[..15].copy_from_slice(b"moonlight-ctest");
            // A few packets, as UDP is unreliable.
            for _ in 0..UDP_TEST_PACKETS {
                socket.send(&packet)?;
                thread::sleep(UDP_TEST_GAP);
            }
            socket.recv(&mut packet).map(drop)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, TcpListener};

    const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
    const SHORT: Duration = Duration::from_millis(400);

    fn free_tcp_port() -> u16 {
        TcpListener::bind((LOOPBACK, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    #[test]
    fn a_listening_tcp_port_passes_and_a_closed_one_fails() {
        let listener = TcpListener::bind((LOOPBACK, 0)).unwrap();
        let open = listener.local_addr().unwrap().port();
        let closed = free_tcp_port();
        let ports = [(1, Transport::Tcp, open), (2, Transport::Tcp, closed)];
        assert_eq!(failing_flags(LOOPBACK, &ports, SHORT), 2);
    }

    #[test]
    fn a_udp_port_passes_when_the_server_answers_and_fails_when_it_does_not() {
        let echo = UdpSocket::bind((LOOPBACK, 0)).unwrap();
        let answering = echo.local_addr().unwrap().port();
        let silent = UdpSocket::bind((LOOPBACK, 0)).unwrap();
        let silent_port = silent.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let mut buffer = [0u8; 2048];
            echo.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            // Stays up for every packet the client sends: a closed port answers the later ones
            // with an ICMP error, which the client reads as a failed port.
            for _ in 0..UDP_TEST_PACKETS {
                let (read, from) = echo.recv_from(&mut buffer).unwrap();
                assert_eq!(read, UDP_TEST_BYTES);
                assert!(buffer.starts_with(b"moonlight-ctest"));
                echo.send_to(&buffer[..1], from).unwrap();
            }
        });
        let ports = [
            (0x100, Transport::Udp, answering),
            (0x200, Transport::Udp, silent_port),
        ];
        assert_eq!(failing_flags(LOOPBACK, &ports, SHORT), 0x200);
        server.join().unwrap();
    }

    #[test]
    fn unknown_flags_are_ignored_and_an_unresolvable_server_is_inconclusive() {
        assert_eq!(test_client_connectivity("127.0.0.1", 1, 0), 0);
        assert_eq!(test_client_connectivity("127.0.0.1", 1, 0x0008_0000), 0);
        assert_eq!(
            test_client_connectivity("not a host name.invalid", 443, 0x0001),
            INCONCLUSIVE
        );
    }

    #[test]
    fn the_port_table_matches_the_stream_ports() {
        let flags: u32 = PORTS
            .iter()
            .map(|(flag, ..)| flag)
            .fold(0, |all, flag| all | flag);
        assert_eq!(flags, VALID_FLAGS);
        assert_eq!(PORTS[0], (0x0001, Transport::Tcp, 47984));
        assert_eq!(PORTS[6], (0x0800, Transport::Udp, 48010));
    }
}
