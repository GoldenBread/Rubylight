//! Reply sockets: answering each client from the address it reached, pins and their route.
use super::super::{
    Batch, ReplySocket,
    reply::{RECHECK, REFUSAL, REPLY_SOURCES, ROUTE_CHECK},
    send_datagram,
    sys::{Ancillary, local_address},
    unicast_addresses,
};
use anyhow::Result;
use std::{
    net::{IpAddr, SocketAddr, UdpSocket},
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

type Bind = fn() -> std::io::Result<UdpSocket>;

fn dual_stack() -> std::io::Result<UdpSocket> {
    let socket = socket2::Socket::new(
        socket2::Domain::IPV6,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )?;
    socket.set_only_v6(false)?;
    socket.bind(&SocketAddr::from(([0u16; 8], 0)).into())?;
    Ok(socket.into())
}

fn any_v4() -> std::io::Result<UdpSocket> {
    UdpSocket::bind("0.0.0.0:0")
}

/// Sends the host's replies to `peer` on every path, each freshly pinned to `pin`: control,
/// audio, a segmented video batch and video datagrams, calling `sent` after each. Returns
/// the senders they came from at `client`.
fn reply_sources(
    host: &ReplySocket,
    client: &UdpSocket,
    peer: SocketAddr,
    pin: Option<IpAddr>,
    sent: &dyn Fn(&str),
) -> Result<Vec<IpAddr>> {
    let video: Vec<_> = (0..3).map(|i| vec![i; 256]).collect();
    let mut expected = Vec::new();
    let pin = || {
        host.forget(peer);
        host.answer_from(peer, pin);
    };
    pin();
    host.send_to(b"control", peer)?;
    sent("control");
    expected.push(b"control".to_vec());
    pin();
    assert!(send_datagram(host, b"audio", peer)?);
    sent("audio");
    expected.push(b"audio".to_vec());
    let mut batch = Batch::default();
    pin();
    assert_eq!(batch.send(host, &video, peer)?, 3 * 256);
    sent("segmented");
    assert!(batch.offload, "segmentation stays on");
    expected.extend(video.iter().cloned());
    batch.offload = false;
    pin();
    assert_eq!(batch.send(host, &video, peer)?, 3 * 256);
    sent("datagrams");
    expected.extend(video.iter().cloned());
    let mut sources = Vec::new();
    let mut buffer = [0u8; 2048];
    for expected in expected {
        let (size, from) = client.recv_from(&mut buffer)?;
        assert_eq!(&buffer[..size], expected);
        sources.push(from.ip().to_canonical());
    }
    Ok(sources)
}

fn timeouts(host: &ReplySocket, client: &UdpSocket) -> Result<()> {
    host.set_read_timeout(Some(Duration::from_secs(5)))?;
    client.set_read_timeout(Some(Duration::from_secs(5)))?;
    Ok(())
}

/// Issue #54 on loopback, where every 127.x address is this PC's: a client on 127.0.0.1 that
/// reached the host at 127.0.0.2 is replied to from 127.0.0.1 by route, unless pinned.
#[test]
fn replies_leave_from_the_address_the_client_reached() -> Result<()> {
    let reached: IpAddr = [127, 0, 0, 2].into();
    for (name, bind) in [("IPv4", any_v4 as Bind), ("dual-stack IPv4", dual_stack)] {
        let host = ReplySocket::new(bind()?)?;
        let client = UdpSocket::bind("127.0.0.1:0")?;
        timeouts(&host, &client)?;
        client.send_to(b"ping", (reached, host.local_addr()?.port()))?;
        let mut buffer = [0u8; 64];
        let (size, peer, local) = host.receive(&mut buffer)?;
        assert_eq!(&buffer[..size], b"ping", "{name}");
        assert_eq!(local, Some(reached), "{name}");
        host.answer_from(peer, local);
        assert_eq!(host.sources_from(peer.ip()), [(peer, reached)], "{name}");
        assert_eq!(host.source(peer), local, "{name}: routed from elsewhere");
        let sources = reply_sources(&host, &client, peer, local, &|_| {})?;
        assert_eq!(sources, vec![reached; sources.len()], "{name}");
    }
    Ok(())
}

/// A source Linux refuses, such as an address a VPN took away, is dropped: the reply leaves
/// from the routed address, segmentation stays on, and the client's next datagram does not
/// pin it again within REFUSAL. The pin is a documentation address this PC does not have.
#[test]
fn a_refused_source_is_forgotten_and_the_reply_still_sent() -> Result<()> {
    let gone: IpAddr = [192, 0, 2, 2].into();
    assert!(!unicast_addresses()?.contains(&gone), "this PC has {gone}");
    let gone = Some(gone);
    for (name, bind) in [("IPv4", any_v4 as Bind), ("dual-stack IPv4", dual_stack)] {
        let mut host = ReplySocket::new(bind()?)?;
        fake_clock(&mut host);
        let client = UdpSocket::bind("127.0.0.1:0")?;
        timeouts(&host, &client)?;
        let loopback: IpAddr = [127, 0, 0, 1].into();
        client.send_to(b"ping", (loopback, host.local_addr()?.port()))?;
        let (_, peer, local) = host.receive(&mut [0u8; 64])?;
        assert_eq!(local, Some(loopback), "{name}");
        host.answer_from(peer, gone);
        assert_eq!(host.source(peer), gone, "{name}: pinned");
        // Each path tried the pin, and forgot it when refused.
        let sources = reply_sources(&host, &client, peer, gone, &|path| {
            assert_eq!(
                host.source(peer),
                None,
                "{name}: {path} never tried the pin"
            );
        })?;
        assert_eq!(sources, vec![loopback; sources.len()], "{name}");
        host.answer_from(peer, gone);
        assert_eq!(host.source(peer), None, "{name}: pinned a refused source");
        host.forget(peer);
        host.answer_from(peer, gone);
        assert_eq!(host.source(peer), gone, "{name}: forgetting the peer");
    }
    Ok(())
}

/// The one loopback source stays pinned on every path, so Linux accepted each path's pktinfo
/// message: IPv4, a dual-stack socket answering a v4-mapped peer, and IPv6. The route is
/// faked to leave from elsewhere, or loopback would not be pinned.
#[test]
fn every_path_keeps_an_accepted_pin() -> Result<()> {
    loopback_replies(Some(|_| Ok(IpAddr::from([192, 0, 2, 9]))))
}

/// A client Linux already routes replies to from the address it reached is answered
/// unpinned on every path, with the real route lookup.
#[test]
fn a_client_routed_from_the_address_it_reached_is_not_pinned() -> Result<()> {
    loopback_replies(None)
}

fn loopback_replies(route: Option<fn(SocketAddr) -> std::io::Result<IpAddr>>) -> Result<()> {
    let hosts: [(&str, Bind, &str); 3] = [
        ("IPv4", any_v4, "127.0.0.1:0"),
        ("dual-stack IPv4", dual_stack, "127.0.0.1:0"),
        ("IPv6", || UdpSocket::bind("[::]:0"), "[::1]:0"),
    ];
    for (name, bind, client) in hosts {
        // A machine without IPv6 skips its host.
        let (Ok(socket), Ok(client)) = (bind(), UdpSocket::bind(client)) else {
            assert_ne!(name, "IPv4", "no IPv4 loopback");
            continue;
        };
        let mut host = ReplySocket::new(socket)?;
        if let Some(route) = route {
            host.route = route;
        }
        let expected = |local: Option<IpAddr>| local.filter(|_| route.is_some());
        timeouts(&host, &client)?;
        let reached = client.local_addr()?.ip();
        client.send_to(b"ping", (reached, host.local_addr()?.port()))?;
        let (_, peer, local) = host.receive(&mut [0u8; 64])?;
        assert_eq!(local, Some(reached), "{name}");
        if name == "dual-stack IPv4" {
            assert!(matches!(peer.ip(), IpAddr::V6(v6) if v6.to_ipv4_mapped().is_some()));
        }
        let sources = reply_sources(&host, &client, peer, local, &|path| {
            assert_eq!(host.source(peer), expected(local), "{name}: {path}");
            assert_eq!(host.sources_from(peer.ip()), [(peer, reached)], "{name}");
        })?;
        assert_eq!(sources, vec![reached; sources.len()], "{name}");
    }
    Ok(())
}

fn documentation_host() -> Result<ReplySocket> {
    let mut host = ReplySocket::bind("127.0.0.1:0")?;
    fake_clock(&mut host);
    host.route = |_| Ok(IpAddr::from([192, 0, 2, 9]));
    Ok(host)
}

const PEER: SocketAddr = SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 1)), 9);
const PIN: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 2));

/// Only an unpinned send that succeeds blames the pin: an interface without a route back is
/// refused once and stays refused for REFUSAL, while a network that is down keeps the pin.
#[test]
fn only_a_send_the_pin_broke_drops_it() -> Result<()> {
    let host = documentation_host()?;
    let pinned = |first: i32, then: Result<(), i32>| {
        host.forget(PEER);
        host.answer_from(PEER, Some(PIN));
        let mut tried = Vec::new();
        let (result, source) = host
            .send_pinned(PEER, Some(PIN), |source| {
                tried.push(source);
                Ok::<_, std::convert::Infallible>(if source.is_some() { Err(first) } else { then })
            })
            .unwrap_or_else(|never| match never {});
        (result, source, tried)
    };
    for unreachable in [
        libc::ENETUNREACH,
        libc::EHOSTUNREACH,
        libc::EADDRNOTAVAIL,
        libc::EINVAL,
    ] {
        assert_eq!(
            pinned(unreachable, Ok(())),
            (Ok(()), None, vec![Some(PIN), None]),
            "{unreachable}"
        );
        assert_eq!(host.source(PEER), None);
        host.answer_from(PEER, Some(PIN));
        assert_eq!(host.source(PEER), None, "{unreachable} pinned again");
    }
    // No segmentation fails both sends with EINVAL; a network that is down fails both too.
    for error in [libc::EINVAL, libc::ENETUNREACH] {
        assert_eq!(
            pinned(error, Err(error)),
            (Err(error), Some(PIN), vec![Some(PIN), None])
        );
        assert_eq!(host.source(PEER), Some(PIN), "{error}");
    }
    // Any other error is the peer's, not the pin's.
    assert_eq!(
        pinned(libc::ECONNRESET, Ok(())),
        (Err(libc::ECONNRESET), Some(PIN), vec![Some(PIN)])
    );
    assert_eq!(host.source(PEER), Some(PIN));
    Ok(())
}

thread_local! {
    /// The time `fake_clock` sockets on this test's thread read.
    static NOW: std::cell::Cell<Instant> = std::cell::Cell::new(Instant::now());
}
/// Makes `host` read the time from NOW, which only `advance` moves.
fn fake_clock(host: &mut ReplySocket) {
    host.now = || NOW.with(std::cell::Cell::get);
}
fn advance(by: Duration) {
    NOW.with(|now| now.set(now.get() + by));
}

/// A refused pin lapses after REFUSAL, so a VPN that went down and came back mid-stream is
/// pinned again.
#[test]
fn a_refused_pin_is_taken_again_after_the_refusal_lapses() -> Result<()> {
    let host = documentation_host()?;
    let one_ms = Duration::from_millis(1);
    let refused = || {
        let (result, source) = host
            .send_pinned(PEER, Some(PIN), |source| {
                Ok::<_, std::convert::Infallible>(match source {
                    Some(_) => Err(libc::EADDRNOTAVAIL),
                    None => Ok(()),
                })
            })
            .unwrap_or_else(|never| match never {});
        assert_eq!((result, source), (Ok(()), None));
    };
    host.answer_from(PEER, Some(PIN));
    assert_eq!(host.source(PEER), Some(PIN));
    refused();
    advance(REFUSAL - one_ms);
    host.answer_from(PEER, Some(PIN));
    assert_eq!(host.source(PEER), None, "still refused");
    advance(one_ms);
    host.answer_from(PEER, Some(PIN));
    assert_eq!(host.source(PEER), Some(PIN), "pinned again");
    assert!(host.sources.read().is_ok_and(|s| s.refused.is_empty()));
    // Refused again, it lapses anew.
    refused();
    advance(REFUSAL - one_ms);
    host.answer_from(PEER, Some(PIN));
    assert_eq!(host.source(PEER), None);
    Ok(())
}

/// A full table evicts the oldest pin, not every pin.
#[test]
fn a_full_table_evicts_the_oldest_pin() -> Result<()> {
    let mut host = ReplySocket::bind("127.0.0.1:0")?;
    host.route = |_| Ok(IpAddr::from([192, 0, 2, 9]));
    let local = Some(PIN);
    let peers: Vec<SocketAddr> = (0..=REPLY_SOURCES as u16)
        .map(|port| SocketAddr::from(([192, 0, 2, 1], 1000 + port)))
        .collect();
    for peer in &peers[..REPLY_SOURCES] {
        host.answer_from(*peer, local);
    }
    // The first peer's newer datagram makes the second the oldest.
    let newer = IpAddr::from([192, 0, 2, 3]);
    host.answer_from(peers[0], Some(newer));
    host.answer_from(peers[REPLY_SOURCES], local);
    assert_eq!(host.source(peers[1]), None);
    assert_eq!(host.source(peers[0]), Some(newer));
    for peer in &peers[2..] {
        assert_eq!(host.source(*peer), local, "{peer}");
    }
    host.forget(peers[2]);
    assert_eq!(host.source(peers[2]), None);
    Ok(())
}

/// The address the fake route below answers with.
static ROUTED: std::sync::Mutex<Option<IpAddr>> = std::sync::Mutex::new(None);

/// A peer whose route leaves from the address it reached is answered unpinned and one whose
/// route leaves from elsewhere pinned. A route that changes is noticed on the first datagram
/// a second after the last check, or RECHECK after it once a send failed.
#[test]
fn the_route_decides_whether_replies_are_pinned() -> Result<()> {
    let mut host = ReplySocket::bind("127.0.0.1:0")?;
    fake_clock(&mut host);
    host.route = |_| Ok(ROUTED.lock().ok().and_then(|routed| *routed).unwrap_or(PIN));
    let route = |address: [u8; 4]| {
        if let Ok(mut routed) = ROUTED.lock() {
            *routed = Some(address.into());
        }
    };
    route([192, 0, 2, 2]);
    host.answer_from(PEER, Some(PIN));
    assert_eq!(host.source(PEER), None, "routed from the address reached");
    route([192, 0, 2, 9]);
    host.answer_from(PEER, Some(PIN));
    assert_eq!(host.source(PEER), None, "checked within a second");
    advance(ROUTE_CHECK);
    host.answer_from(PEER, Some(PIN));
    assert_eq!(host.source(PEER), Some(PIN), "routed from elsewhere");
    route([192, 0, 2, 2]);
    advance(ROUTE_CHECK);
    host.answer_from(PEER, Some(PIN));
    assert_eq!(host.source(PEER), None, "routed back");
    assert!(
        !host.any_pinned.load(Ordering::Acquire),
        "sends skip the lock"
    );
    // A route that changed, found by a failed unpinned send: the next datagram RECHECK
    // after the last check pins the peer.
    route([192, 0, 2, 9]);
    let (result, source) = host
        .send_pinned(PEER, None, |source| {
            assert_eq!(source, None);
            Ok::<_, std::convert::Infallible>(Err::<(), _>(libc::ENETUNREACH))
        })
        .unwrap_or_else(|never| match never {});
    assert_eq!((result, source), (Err(libc::ENETUNREACH), None));
    host.answer_from(PEER, Some(PIN));
    assert_eq!(host.source(PEER), None, "checked within RECHECK");
    advance(RECHECK);
    host.answer_from(PEER, Some(PIN));
    assert_eq!(host.source(PEER), Some(PIN), "rechecked after a failure");
    // A route Linux cannot name keeps what was decided.
    host.route = |_| Err(std::io::Error::from_raw_os_error(libc::ENETUNREACH));
    advance(ROUTE_CHECK);
    host.answer_from(PEER, Some(PIN));
    assert_eq!(host.source(PEER), Some(PIN));
    host.forget(PEER);
    assert!(!host.any_pinned.load(Ordering::Acquire));
    Ok(())
}

/// Route lookups by the fake route below.
static LOOKUPS: AtomicUsize = AtomicUsize::new(0);

/// Failed sends never ask Linux for the route themselves, and however many fail, the
/// receiving thread asks at most once per RECHECK.
#[test]
fn failed_sends_check_the_route_at_most_once_per_recheck() -> Result<()> {
    let mut host = ReplySocket::bind("127.0.0.1:0")?;
    fake_clock(&mut host);
    host.route = |_| {
        LOOKUPS.fetch_add(1, Ordering::Relaxed);
        Ok(PIN)
    };
    host.answer_from(PEER, Some(PIN));
    assert_eq!(LOOKUPS.load(Ordering::Relaxed), 1);
    let fail = || {
        let (result, _) = host
            .send_pinned(PEER, None, |_| {
                Ok::<_, std::convert::Infallible>(Err::<(), _>(libc::ENETUNREACH))
            })
            .unwrap_or_else(|never| match never {});
        assert_eq!(result, Err(libc::ENETUNREACH));
    };
    for _ in 0..1000 {
        fail();
    }
    assert_eq!(
        LOOKUPS.load(Ordering::Relaxed),
        1,
        "not on the sending thread"
    );
    advance(RECHECK);
    for _ in 0..1000 {
        fail();
        host.answer_from(PEER, Some(PIN));
    }
    assert_eq!(LOOKUPS.load(Ordering::Relaxed), 2, "once per RECHECK");
    Ok(())
}

#[test]
fn local_address_reads_pktinfo_after_other_control_messages() {
    let mut control = Ancillary::default();
    control.segment(1200);
    control.source([10, 1, 2, 3].into());
    assert_eq!(
        local_address(&control.bytes[..control.len]),
        Some([10, 1, 2, 3].into())
    );
    let mut control = Ancillary::default();
    let address: IpAddr = "fd7a:115c:a1e0::1".parse().unwrap_or(IpAddr::from([0; 4]));
    control.source(address);
    assert_eq!(local_address(&control.bytes[..control.len]), Some(address));
    // A v4-mapped source is sent, and read back, as IPv4.
    let mut control = Ancillary::default();
    control.source("::ffff:100.64.0.1".parse().unwrap_or(IpAddr::from([0; 4])));
    assert_eq!(
        local_address(&control.bytes[..control.len]),
        Some([100, 64, 0, 1].into())
    );
    // Truncated or malformed control data has no address.
    assert_eq!(local_address(&control.bytes[..20]), None);
    assert_eq!(local_address(&[0u8; 32]), None);
}
