//! Tests for `ReplySocket` source-address pinning, route checks, and send/receive path cost.
use super::super::datagram::{WaitBudget, send_one};
use super::super::reply::{RECHECK, REFUSAL, REPLY_SOURCES, ROUTE_CHECK};
use super::super::*;
use anyhow::Result;
use std::{
    net::{IpAddr, SocketAddr, UdpSocket},
    os::windows::io::AsRawSocket,
    sync::atomic::Ordering,
};
use windows::Win32::Networking::WinSock::*;

/// The send path's own cost on loopback, per call: a 64-datagram video
/// batch and one audio datagram, to a peer never heard from ("routed"),
/// one Windows routes from the address it reached ("matched", a LAN
/// client) and one it does not ("pinned"). Run with --release.
#[test]
#[ignore = "timing; run with --release --ignored --nocapture"]
fn send_path_cost() -> Result<()> {
    let receiver = UdpSocket::bind("127.0.0.1:0")?;
    socket2::SockRef::from(&receiver).set_recv_buffer_size(64 << 20)?;
    receiver.set_read_timeout(Some(std::time::Duration::from_millis(200)))?;
    let peer = receiver.local_addr()?;
    let drain = std::thread::spawn(move || {
        let mut buffer = [0u8; 2048];
        while receiver.recv(&mut buffer).is_ok() {}
    });
    let video = vec![vec![7u8; 1200]; 64];
    let audio = vec![9u8; 400];
    // "before" makes the calls 848095f08 made, without a ReplySocket
    // lookup.
    let modes = ["before", "routed", "matched", "pinned"];
    let mut senders = Vec::new();
    for mode in modes {
        let mut sender = ReplySocket::bind("0.0.0.0:0")?;
        configure_udp(&sender)?;
        if mode == "pinned" {
            sender.route = |_, _| Ok(IpAddr::from([192, 0, 2, 9]));
        }
        if mode == "matched" || mode == "pinned" {
            sender.answer_from(peer, Some(peer.ip()));
            assert_eq!(sender.sources_from(peer.ip()), [(peer, peer.ip())]);
        }
        senders.push((mode, sender, Batch::default(), Vec::new(), Vec::new()));
    }
    // Short turns in a rotating order: this PC's send cost changes over
    // seconds, so each mode sees the same mix.
    for round in 0..40 {
        for i in 0..modes.len() {
            let (mode, sender, batch, batches, datagrams) = &mut senders[(i + round) % modes.len()];
            let started = std::time::Instant::now();
            for _ in 0..200 {
                if *mode == "before" {
                    let mut budget = WaitBudget::default();
                    batch
                        .send_segmented(sender, &video, peer, None, &mut budget)?
                        .unwrap();
                } else {
                    batch.send(sender, &video, peer)?;
                }
            }
            batches.push(started.elapsed().as_nanos() / 200);
            let started = std::time::Instant::now();
            for _ in 0..2000 {
                if *mode == "before" {
                    send_one(sender, &audio, peer, None, &mut WaitBudget::default(), None)?
                        .unwrap();
                } else {
                    send_datagram(sender, &audio, peer)?;
                }
            }
            datagrams.push(started.elapsed().as_nanos() / 2000);
        }
    }
    for (mode, sender, batch, batches, datagrams) in &mut senders {
        // A refused pin would have measured the unpinned path.
        assert_eq!(
            sender.source(peer),
            (*mode == "pinned").then_some(peer.ip()),
            "{mode}"
        );
        assert_eq!(batch.dropped, 0, "{mode}");
        assert!(batch.offload, "{mode}");
        let mean = |values: &[u128]| values.iter().sum::<u128>() / values.len() as u128;
        batches.sort();
        datagrams.sort();
        println!(
            "{mode}: batch mean {} ns, median {} ns; datagram mean {} ns, median {} ns",
            mean(batches),
            batches[batches.len() / 2],
            mean(datagrams),
            datagrams[datagrams.len() / 2]
        );
    }
    drop(senders);
    drain.join().unwrap();
    Ok(())
}

/// The receive path's own cost on loopback, per datagram, as the control
/// thread pays it before input is applied: "before" is recv_from,
/// "settled" WSARecvMsg and answer_from for a peer checked within
/// ROUTE_CHECK, "checked" the same with Windows' route asked every time,
/// as happens once a second per peer. Run with --release.
#[test]
#[ignore = "timing; run with --release --ignored --nocapture"]
fn receive_path_cost() -> Result<()> {
    let modes = ["before", "settled", "checked"];
    let sender = UdpSocket::bind("127.0.0.1:0")?;
    let datagram = [5u8; 64];
    let mut receivers = Vec::new();
    for mode in modes {
        let mut receiver = ReplySocket::bind("0.0.0.0:0")?;
        configure_udp(&receiver)?;
        if mode == "checked" {
            // Every lookup is a route check: the clock jumps past ROUTE_CHECK.
            static TICKS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            static BASE: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
            receiver.now = || {
                *BASE.get_or_init(std::time::Instant::now)
                    + ROUTE_CHECK * TICKS.fetch_add(2, Ordering::Relaxed) as u32
            };
        }
        let port = receiver.local_addr()?.port();
        let target = SocketAddr::from(([127, 0, 0, 1], port));
        receivers.push((mode, receiver, target, Vec::new()));
    }
    let mut buffer = [0u8; 2048];
    for round in 0..40 {
        for i in 0..modes.len() {
            let (mode, receiver, target, costs) = &mut receivers[(i + round) % modes.len()];
            let mut elapsed = std::time::Duration::ZERO;
            for _ in 0..500 {
                sender.send_to(&datagram, *target)?;
                while !wait_readable(receiver.as_raw_socket(), 100)? {}
                let started = std::time::Instant::now();
                if *mode == "before" {
                    receiver.socket.recv_from(&mut buffer)?;
                } else {
                    let (_, peer, local) = receiver.receive(&mut buffer)?;
                    assert!(local.is_some(), "{mode}");
                    receiver.answer_from(peer, local);
                }
                elapsed += started.elapsed();
            }
            costs.push(elapsed.as_nanos() / 500);
        }
    }
    for (mode, _, _, costs) in &mut receivers {
        costs.sort();
        println!(
            "{mode}: datagram mean {} ns, median {} ns, max turn {} ns",
            costs.iter().sum::<u128>() / costs.len() as u128,
            costs[costs.len() / 2],
            costs[costs.len() - 1]
        );
    }
    Ok(())
}

fn dual_stack() -> std::io::Result<UdpSocket> {
    let socket = socket2::Socket::new(
        socket2::Domain::IPV6,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )?;
    socket.set_only_v6(false)?;
    socket.bind(&"[::]:0".parse::<SocketAddr>().unwrap().into())?;
    Ok(socket.into())
}

/// Sends `host`'s replies to `peer` on every path, each freshly pinned to
/// `pin`: control, audio, a segmented video batch and video datagrams,
/// calling `sent` after each. Returns the senders they came from at
/// `client`.
fn reply_sources(
    host: &ReplySocket,
    client: &UdpSocket,
    peer: SocketAddr,
    pin: Option<IpAddr>,
    sent: &dyn Fn(&str),
) -> Result<Vec<IpAddr>> {
    use anyhow::Context;
    let video: Vec<_> = (0..3).map(|i| vec![i; 256]).collect();
    let mut expected = Vec::new();
    let pin = || {
        host.forget(peer);
        host.answer_from(peer, pin);
    };
    pin();
    host.send_to(b"control", peer).context("control")?;
    sent("control");
    expected.push(b"control".to_vec());
    pin();
    assert!(send_datagram(host, b"audio", peer).context("audio")?);
    sent("audio");
    expected.push(b"audio".to_vec());
    let mut batch = Batch::default();
    pin();
    assert_eq!(
        batch.send(host, &video, peer).context("segmented")?,
        3 * 256
    );
    sent("segmented");
    assert!(batch.offload, "segmentation stays on");
    expected.extend(video.iter().cloned());
    batch.offload = false;
    pin();
    assert_eq!(
        batch.send(host, &video, peer).context("datagrams")?,
        3 * 256
    );
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

/// Issue #54: a host on every address answers each client from the
/// address the client reached, not the one Windows routes by. To one of
/// its own addresses, Windows replies from that address, so a client on
/// one address of this PC that reached another sees the wrong source
/// without pinning. Skipped without two such addresses of one family.
#[test]
fn replies_leave_from_the_address_the_client_reached() -> Result<()> {
    let addresses = unicast_addresses()?;
    let mut tested = Vec::new();
    type Bind = fn() -> std::io::Result<UdpSocket>;
    let hosts: [(&str, Bind, bool); 3] = [
        ("IPv4", || UdpSocket::bind("0.0.0.0:0"), false),
        ("dual-stack IPv4", dual_stack, false),
        ("IPv6", || UdpSocket::bind("[::]:0"), true),
    ];
    for (name, bind, v6) in hosts {
        let same: Vec<_> = addresses.iter().filter(|a| a.is_ipv6() == v6).collect();
        'pairs: for client_ip in &same {
            for reached in &same {
                if client_ip == reached {
                    continue;
                }
                let host = ReplySocket::new(bind()?)?;
                let Ok(client) = UdpSocket::bind((**client_ip, 0)) else {
                    continue;
                };
                host.set_read_timeout(Some(std::time::Duration::from_millis(300)))?;
                client.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
                let target = SocketAddr::new(**reached, host.local_addr()?.port());
                if client.send_to(b"ping", target).is_err() {
                    continue;
                }
                let mut buffer = [0u8; 64];
                let Ok((size, peer, local)) = host.receive(&mut buffer) else {
                    continue;
                };
                assert_eq!(&buffer[..size], b"ping", "{name}");
                assert_eq!(local, Some(**reached), "{name}");
                host.answer_from(peer, local);
                assert_eq!(host.sources_from(peer.ip()), [(peer, **reached)], "{name}");
                let sources = reply_sources(&host, &client, peer, local, &|_| {})?;
                assert!(
                    sources.iter().all(|source| source == *reached),
                    "{name}: {client_ip} reached {reached}; replies came from {sources:?}"
                );
                tested.push(name);
                break 'pairs;
            }
        }
    }
    println!("tested {tested:?} among {addresses:?}");
    if tested.len() < hosts.len() {
        println!(
            "skipped {:?}: no two addresses of one family reach each other",
            hosts
                .iter()
                .map(|(name, ..)| *name)
                .filter(|name| !tested.contains(name))
                .collect::<Vec<_>>()
        );
    }
    Ok(())
}

/// A source Windows refuses, such as an address a VPN took away, is
/// dropped: the reply leaves from the routed address, segmentation stays
/// on, and the client's next datagram does not pin it again within
/// REFUSAL. The pin is a documentation address this PC does not have,
/// which Windows always refuses, so this also shows that every path
/// sends from the pinned address. Not 127.0.0.2: for minutes after any
/// process sent from it, such as udp_shaper, Windows's path cache lets
/// every socket send from it.
#[test]
fn a_refused_source_is_forgotten_and_the_reply_still_sent() -> Result<()> {
    let gone: IpAddr = [192, 0, 2, 2].into();
    assert!(!unicast_addresses()?.contains(&gone), "this PC has {gone}");
    let gone = Some(gone);
    type Bind = fn() -> std::io::Result<UdpSocket>;
    let hosts: [(&str, Bind); 2] = [
        ("IPv4", || UdpSocket::bind("0.0.0.0:0")),
        ("dual-stack IPv4", dual_stack),
    ];
    for (name, bind) in hosts {
        let mut host = ReplySocket::new(bind()?)?;
        fake_clock(&mut host);
        let client = UdpSocket::bind("127.0.0.1:0")?;
        host.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
        client.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
        let loopback: IpAddr = [127, 0, 0, 1].into();
        client.send_to(b"ping", (loopback, host.local_addr()?.port()))?;
        let mut buffer = [0u8; 64];
        let (_, peer, local) = host.receive(&mut buffer)?;
        assert_eq!(local, Some(loopback), "{name}");
        host.answer_from(peer, gone);
        assert_eq!(host.source(peer), gone, "{name}: pinned");
        // Each path tried the pin, and forgot it when refused.
        let sources = reply_sources(&host, &client, peer, gone, &|path| {
            assert_eq!(
                host.source(peer),
                None,
                "{name}: {path} never tried the pin"
            )
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

/// The one loopback source Windows sends from stays pinned on every
/// path, so Windows accepted each path's pktinfo message: IPv4, a
/// dual-stack socket answering a v4-mapped peer, and IPv6. The route is
/// faked to leave from elsewhere, or loopback would not be pinned.
#[test]
fn every_path_keeps_an_accepted_pin() -> Result<()> {
    loopback_replies(Some(|_, _| Ok(IpAddr::from([192, 0, 2, 9]))))
}

/// A client Windows already routes replies to from the address it
/// reached, as on a plain LAN, is answered as before the fix: unpinned,
/// on every path, with Windows's real route lookup.
#[test]
fn a_client_routed_from_the_address_it_reached_is_not_pinned() -> Result<()> {
    loopback_replies(None)
}

/// Replies on every path to loopback clients, with `route` faked or
/// Windows's own; pinned exactly when the route is faked.
fn loopback_replies(
    route: Option<fn(SOCKET, SocketAddr) -> std::io::Result<IpAddr>>,
) -> Result<()> {
    type Bind = fn() -> std::io::Result<UdpSocket>;
    let hosts: [(&str, Bind, &str); 3] = [
        ("IPv4", || UdpSocket::bind("0.0.0.0:0"), "127.0.0.1:0"),
        ("dual-stack IPv4", dual_stack, "127.0.0.1:0"),
        ("IPv6", || UdpSocket::bind("[::]:0"), "[::1]:0"),
    ];
    for (name, bind, client) in hosts {
        let mut host = ReplySocket::new(bind()?)?;
        if let Some(route) = route {
            host.route = route;
        }
        let expected = |local: Option<IpAddr>| local.filter(|_| route.is_some());
        let client = UdpSocket::bind(client)?;
        host.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
        client.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
        let reached = client.local_addr()?.ip();
        client.send_to(b"ping", (reached, host.local_addr()?.port()))?;
        let mut buffer = [0u8; 64];
        let (_, peer, local) = host.receive(&mut buffer)?;
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

/// Only an unpinned send that succeeds blames the pin: an interface
/// without a route back is refused once and stays refused for REFUSAL,
/// while a NIC
/// without UDP segmentation or a network that is down keeps the pin.
#[test]
fn only_a_send_the_pin_broke_drops_it() -> Result<()> {
    let mut host = ReplySocket::bind("127.0.0.1:0")?;
    fake_clock(&mut host);
    host.route = |_, _| Ok(IpAddr::from([192, 0, 2, 9]));
    let peer: SocketAddr = "192.0.2.1:9".parse().unwrap();
    let pin: IpAddr = [192, 0, 2, 2].into();
    let pinned = |first: WSA_ERROR, then: Result<(), WSA_ERROR>| {
        host.forget(peer);
        host.answer_from(peer, Some(pin));
        let mut tried = Vec::new();
        let (result, source) = host
            .send_pinned(peer, Some(pin), |source| {
                tried.push(source);
                Ok::<_, std::convert::Infallible>(if source.is_some() { Err(first) } else { then })
            })
            .unwrap();
        (result, source, tried)
    };
    for unreachable in [WSAENETUNREACH, WSAEHOSTUNREACH, WSAEADDRNOTAVAIL, WSAEINVAL] {
        assert_eq!(
            pinned(unreachable, Ok(())),
            (Ok(()), None, vec![Some(pin), None]),
            "{unreachable:?}"
        );
        assert_eq!(host.source(peer), None);
        host.answer_from(peer, Some(pin));
        assert_eq!(host.source(peer), None, "{unreachable:?} pinned again");
    }
    // No segmentation fails both sends with WSAEINVAL; a network that is
    // down fails both too.
    for error in [WSAEINVAL, WSAENETUNREACH] {
        assert_eq!(
            pinned(error, Err(error)),
            (Err(error), Some(pin), vec![Some(pin), None])
        );
        assert_eq!(host.source(peer), Some(pin), "{error:?}");
    }
    // Any other error is the peer's, not the pin's.
    assert_eq!(
        pinned(WSAECONNRESET, Ok(())),
        (Err(WSAECONNRESET), Some(pin), vec![Some(pin)])
    );
    assert_eq!(host.source(peer), Some(pin));
    Ok(())
}

thread_local! {
    /// The time `fake_clock` sockets on this test's thread read.
    static NOW: std::cell::Cell<std::time::Instant> =
        std::cell::Cell::new(std::time::Instant::now());
}

/// Makes `host` read the time from NOW, which only `advance` moves.
fn fake_clock(host: &mut ReplySocket) {
    host.now = || NOW.with(std::cell::Cell::get);
}

fn advance(by: std::time::Duration) {
    NOW.with(|now| now.set(now.get() + by));
}

/// A refused pin lapses after REFUSAL, so a VPN that went down and came
/// back mid-stream is pinned again.
#[test]
fn a_refused_pin_is_taken_again_after_the_refusal_lapses() -> Result<()> {
    let mut host = ReplySocket::bind("127.0.0.1:0")?;
    fake_clock(&mut host);
    host.route = |_, _| Ok(IpAddr::from([192, 0, 2, 9]));
    let peer: SocketAddr = "192.0.2.1:9".parse().unwrap();
    let pin: IpAddr = [192, 0, 2, 2].into();
    let refused = |host: &ReplySocket| {
        let (result, source) = host
            .send_pinned(peer, Some(pin), |source| {
                Ok::<_, std::convert::Infallible>(match source {
                    Some(_) => Err(WSAEADDRNOTAVAIL),
                    None => Ok(()),
                })
            })
            .unwrap();
        assert_eq!((result, source), (Ok(()), None));
    };
    host.answer_from(peer, Some(pin));
    assert_eq!(host.source(peer), Some(pin));
    refused(&host);
    advance(REFUSAL - std::time::Duration::from_millis(1));
    host.answer_from(peer, Some(pin));
    assert_eq!(host.source(peer), None, "still refused");
    advance(std::time::Duration::from_millis(1));
    host.answer_from(peer, Some(pin));
    assert_eq!(host.source(peer), Some(pin), "pinned again");
    assert!(host.sources.read().unwrap().refused.is_empty());
    // Refused again, it lapses anew.
    refused(&host);
    advance(REFUSAL - std::time::Duration::from_millis(1));
    host.answer_from(peer, Some(pin));
    assert_eq!(host.source(peer), None);
    Ok(())
}

/// A full table evicts the oldest pin, not every pin.
#[test]
fn a_full_table_evicts_the_oldest_pin() -> Result<()> {
    let mut host = ReplySocket::bind("127.0.0.1:0")?;
    host.route = |_, _| Ok(IpAddr::from([192, 0, 2, 9]));
    let local = Some(IpAddr::from([192, 0, 2, 2]));
    let peers: Vec<SocketAddr> = (0..=REPLY_SOURCES as u16)
        .map(|port| SocketAddr::from(([192, 0, 2, 1], 1000 + port)))
        .collect();
    for peer in &peers[..REPLY_SOURCES] {
        host.answer_from(*peer, local);
    }
    // The first peer's newer datagram makes the second the oldest.
    host.answer_from(peers[0], Some(IpAddr::from([192, 0, 2, 3])));
    host.answer_from(peers[REPLY_SOURCES], local);
    assert_eq!(host.source(peers[1]), None);
    assert_eq!(host.source(peers[0]), Some(IpAddr::from([192, 0, 2, 3])));
    for peer in &peers[2..] {
        assert_eq!(host.source(*peer), local, "{peer}");
    }
    host.forget(peers[2]);
    assert_eq!(host.source(peers[2]), None);
    Ok(())
}

/// The address the fake route below answers with.
static ROUTED: std::sync::Mutex<Option<IpAddr>> = std::sync::Mutex::new(None);

/// A peer whose route leaves from the address it reached is answered
/// unpinned and one whose route leaves from elsewhere pinned. A route
/// that changes is noticed on the first datagram a second after the last
/// check, or RECHECK after it once a send failed as when a VPN came up.
#[test]
fn the_route_decides_whether_replies_are_pinned() -> Result<()> {
    let mut host = ReplySocket::bind("127.0.0.1:0")?;
    fake_clock(&mut host);
    host.route = |_, _| Ok(ROUTED.lock().unwrap().unwrap());
    let route = |address: [u8; 4]| *ROUTED.lock().unwrap() = Some(address.into());
    let peer: SocketAddr = "192.0.2.1:9".parse().unwrap();
    let local = IpAddr::from([192, 0, 2, 2]);
    route([192, 0, 2, 2]);
    host.answer_from(peer, Some(local));
    assert_eq!(host.source(peer), None, "routed from the address reached");
    route([192, 0, 2, 9]);
    host.answer_from(peer, Some(local));
    assert_eq!(host.source(peer), None, "checked within a second");
    advance(ROUTE_CHECK);
    host.answer_from(peer, Some(local));
    assert_eq!(host.source(peer), Some(local), "routed from elsewhere");
    route([192, 0, 2, 2]);
    advance(ROUTE_CHECK);
    host.answer_from(peer, Some(local));
    assert_eq!(host.source(peer), None, "routed back");
    assert!(
        !host.any_pinned.load(Ordering::Acquire),
        "sends skip the lock"
    );
    // A route that changed, found by a failed unpinned send: the next
    // datagram RECHECK after the last check pins the peer.
    route([192, 0, 2, 9]);
    let (result, source) = host.send_pinned(peer, None, |source| {
        assert_eq!(source, None);
        Ok::<_, std::convert::Infallible>(Err::<(), _>(WSAENETUNREACH))
    })?;
    assert_eq!((result, source), (Err(WSAENETUNREACH), None));
    host.answer_from(peer, Some(local));
    assert_eq!(host.source(peer), None, "checked within RECHECK");
    advance(RECHECK);
    host.answer_from(peer, Some(local));
    assert_eq!(host.source(peer), Some(local), "rechecked after a failure");
    // A route Windows cannot name keeps what was decided.
    host.route = |_, _| Err(std::io::Error::from_raw_os_error(WSAENETUNREACH.0));
    advance(ROUTE_CHECK);
    host.answer_from(peer, Some(local));
    assert_eq!(host.source(peer), Some(local));
    host.forget(peer);
    assert!(!host.any_pinned.load(Ordering::Acquire));
    Ok(())
}

/// Route lookups by the fake route below.
static LOOKUPS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Failed sends never ask Windows for the route themselves, and however
/// many fail, the receiving thread asks at most once per RECHECK.
#[test]
fn failed_sends_check_the_route_at_most_once_per_recheck() -> Result<()> {
    let mut host = ReplySocket::bind("127.0.0.1:0")?;
    fake_clock(&mut host);
    host.route = |_, _| {
        LOOKUPS.fetch_add(1, Ordering::Relaxed);
        Ok(IpAddr::from([192, 0, 2, 2]))
    };
    let peer: SocketAddr = "192.0.2.1:9".parse().unwrap();
    let local = IpAddr::from([192, 0, 2, 2]);
    host.answer_from(peer, Some(local));
    assert_eq!(LOOKUPS.load(Ordering::Relaxed), 1);
    let fail = |host: &ReplySocket| {
        let (result, _) = host
            .send_pinned(peer, None, |_| {
                Ok::<_, std::convert::Infallible>(Err::<(), _>(WSAENETUNREACH))
            })
            .unwrap();
        assert_eq!(result, Err(WSAENETUNREACH));
    };
    for _ in 0..1000 {
        fail(&host);
    }
    assert_eq!(
        LOOKUPS.load(Ordering::Relaxed),
        1,
        "not on the sending thread"
    );
    advance(RECHECK);
    for _ in 0..1000 {
        fail(&host);
        host.answer_from(peer, Some(local));
    }
    assert_eq!(LOOKUPS.load(Ordering::Relaxed), 2, "once per RECHECK");
    Ok(())
}
