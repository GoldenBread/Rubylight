//! Unit tests for PCP and NAT-PMP port mapping, nonce matching, and epoch handling.

use super::*;

/// A start after an unclean stop deletes the guest ports it may have left forwarded. The
/// first request is PCP with no lifetime; the router's version 0 answer says it speaks
/// NAT-PMP, whose delete for every guest port asks for lifetime 0 and no external port.
#[tokio::test]
async fn a_start_deletes_leftover_guest_leases_with_lifetime_zero() {
    use std::sync::{Arc, Mutex};
    let router = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = router.local_addr().unwrap();
    let seen: Arc<Mutex<Vec<Vec<u8>>>> = Arc::default();
    let log = seen.clone();
    tokio::spawn(async move {
        let mut buffer = [0u8; 128];
        loop {
            let Ok((length, from)) = router.recv_from(&mut buffer).await else {
                return;
            };
            let request = buffer[..length].to_vec();
            // A NAT-PMP router answers PCP with version 0, and a NAT-PMP mapping request
            // or delete with the internal port asked and no external port.
            let reply = match request[0] {
                2 => vec![0, 0x81, 0, 1, 0, 0, 0, 0],
                _ => {
                    let mut reply = vec![0, 0x80 | request[1], 0, 0, 0, 0, 0, 1];
                    reply.extend_from_slice(&request[4..6]);
                    reply.extend_from_slice(&[0, 0]);
                    reply.extend_from_slice(&0u32.to_be_bytes());
                    reply
                }
            };
            log.lock().unwrap().push(request);
            router.send_to(&reply, from).await.unwrap();
        }
    });
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    socket.connect(address).await.unwrap();
    let mut mapper = PortMapper::new(socket, "127.0.0.1".parse().unwrap(), b"key");
    let guest = [
        Mapping {
            protocol: Protocol::TCP,
            port: 47991,
        },
        Mapping {
            protocol: Protocol::UDP,
            port: 8002,
        },
    ];
    assert_eq!(mapper.delete(&guest).await, guest);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 3, "{seen:?}");
    // One PCP delete, under this host's nonce, with no lifetime and the guest port.
    assert_eq!(seen[0][0], 2);
    assert_eq!(u32::from_be_bytes(seen[0][4..8].try_into().unwrap()), 0);
    assert_eq!(u16::from_be_bytes([seen[0][40], seen[0][41]]), 47991);
    // Then a NAT-PMP delete per guest port: no external port, no lifetime.
    for (request, port, protocol) in [(&seen[1], 47991, 2), (&seen[2], 8002, 1)] {
        assert_eq!(request[0], 0);
        assert_eq!(request[1], protocol);
        assert_eq!(u16::from_be_bytes([request[4], request[5]]), port);
        assert_eq!(u16::from_be_bytes([request[6], request[7]]), 0);
        assert_eq!(u32::from_be_bytes(request[8..12].try_into().unwrap()), 0);
    }
}

/// A NAT-PMP-only router on loopback: it refuses PCP with version 0, maps with NAT-PMP and
/// reports its address.
#[tokio::test]
async fn a_natpmp_router_is_used_after_its_pcp_refusal() {
    let router = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = router.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buffer = [0u8; 128];
        loop {
            let Ok((length, from)) = router.recv_from(&mut buffer).await else {
                return;
            };
            let request = &buffer[..length];
            let reply: Vec<u8> = match request[0] {
                2 => vec![0, 0x81, 0, 1, 0, 0, 0, 0],
                _ if request[1] == 0 => vec![0, 128, 0, 0, 0, 0, 0, 1, 203, 0, 113, 9],
                _ => {
                    let mut reply = vec![0, 0x80 | request[1], 0, 0, 0, 0, 0, 1];
                    reply.extend_from_slice(&request[4..6]);
                    reply.extend_from_slice(&request[4..6]);
                    reply.extend_from_slice(&3600u32.to_be_bytes());
                    reply
                }
            };
            router.send_to(&reply, from).await.unwrap();
        }
    });
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    socket.connect(address).await.unwrap();
    let mut mapper = PortMapper::new(socket, "127.0.0.1".parse().unwrap(), b"key");
    let wanted = [
        Mapping {
            protocol: Protocol::TCP,
            port: 47984,
        },
        Mapping {
            protocol: Protocol::UDP,
            port: 47998,
        },
    ];
    let outcome = mapper.renew(&wanted).await;
    assert_eq!(mapper.method(), Some(Method::NatPmp));
    assert_eq!(outcome.mapped, vec![(wanted[0], 47984), (wanted[1], 47998)]);
    assert!(outcome.failed.is_empty());
    assert_eq!(mapper.external, Some("203.0.113.9".parse().unwrap()));
    // A second renewal within the lease asks nothing.
    let again = mapper.renew(&wanted).await;
    assert_eq!(again.mapped.len(), 2);
    mapper.close().await;
    assert!(mapper.leases.is_empty());
}

/// A PCP router that refuses one port: the other maps, the refusal says why.
#[tokio::test]
async fn a_pcp_refusal_names_its_reason() {
    let router = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = router.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buffer = [0u8; 128];
        loop {
            let Ok((length, from)) = router.recv_from(&mut buffer).await else {
                return;
            };
            let request = buffer[..length].to_vec();
            let port = u16::from_be_bytes([request[40], request[41]]);
            let mut reply = vec![2, 0x81, 0, if port == 47998 { 8 } else { 0 }];
            reply.extend_from_slice(&7200u32.to_be_bytes());
            reply.extend_from_slice(&[0, 0, 0, 1]);
            reply.extend_from_slice(&[0; 12]);
            reply.extend_from_slice(&request[24..44]);
            reply.extend_from_slice(&Ipv4Addr::new(203, 0, 113, 9).to_ipv6_mapped().octets());
            router.send_to(&reply, from).await.unwrap();
        }
    });
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    socket.connect(address).await.unwrap();
    let mut mapper = PortMapper::new(socket, "127.0.0.1".parse().unwrap(), b"key");
    let wanted = [
        Mapping {
            protocol: Protocol::TCP,
            port: 47984,
        },
        Mapping {
            protocol: Protocol::UDP,
            port: 47998,
        },
    ];
    let outcome = mapper.renew(&wanted).await;
    assert_eq!(mapper.method(), Some(Method::Pcp));
    assert_eq!(outcome.mapped, vec![(wanted[0], 47984)]);
    assert_eq!(outcome.failed.len(), 1);
    assert!(
        outcome.failed[0].1.contains("out of mappings"),
        "{:?}",
        outcome.failed
    );
    assert_eq!(mapper.external, Some("203.0.113.9".parse().unwrap()));
}

/// A PCP router that holds each mapping for the nonce that made it, as RFC 6887 asks: a
/// mapper made after the first one went (a lost renewal, a host restart) renews the same
/// ports, and a dropped mapper deletes its leases.
#[tokio::test]
async fn a_new_mapper_renews_what_an_earlier_one_mapped() {
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };
    let router = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = router.local_addr().unwrap();
    // Internal port and protocol to (nonce, lifetime).
    type Held = HashMap<(u16, u8), ([u8; 12], u32)>;
    let held: Arc<Mutex<Held>> = Arc::default();
    let table = held.clone();
    tokio::spawn(async move {
        let mut buffer = [0u8; 128];
        loop {
            let Ok((length, from)) = router.recv_from(&mut buffer).await else {
                return;
            };
            let request = buffer[..length].to_vec();
            let lifetime = u32::from_be_bytes(request[4..8].try_into().unwrap());
            let nonce: [u8; 12] = request[24..36].try_into().unwrap();
            let key = (u16::from_be_bytes([request[40], request[41]]), request[36]);
            let result = {
                let mut table = table.lock().unwrap();
                match table.get(&key) {
                    Some((held, _)) if *held != nonce => 2,
                    _ if lifetime == 0 => {
                        table.remove(&key);
                        0
                    }
                    _ => {
                        table.insert(key, (nonce, lifetime));
                        0
                    }
                }
            };
            let mut reply = vec![2, 0x81, 0, result];
            reply.extend_from_slice(&lifetime.to_be_bytes());
            reply.extend_from_slice(&[0, 0, 0, 1]);
            reply.extend_from_slice(&[0; 12]);
            reply.extend_from_slice(&request[24..44]);
            reply.extend_from_slice(&Ipv4Addr::new(203, 0, 113, 9).to_ipv6_mapped().octets());
            router.send_to(&reply, from).await.unwrap();
        }
    });
    let mapper = move || async move {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        socket.connect(address).await.unwrap();
        PortMapper::new(socket, "127.0.0.1".parse().unwrap(), b"key")
    };
    let wanted = [Mapping {
        protocol: Protocol::UDP,
        port: 47998,
    }];
    let mut first = mapper().await;
    assert_eq!(first.renew(&wanted).await.mapped.len(), 1);
    // The first mapper is forgotten without a word (its renewal got no answer, or the host
    // crashed); the next one still renews the port.
    first.leases.clear();
    drop(first);
    let mut second = mapper().await;
    let outcome = second.renew(&wanted).await;
    assert_eq!(
        outcome.mapped,
        vec![(wanted[0], 47998)],
        "{:?}",
        outcome.failed
    );
    // Another host's nonce is refused, so the router does hold the rule.
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    socket.connect(address).await.unwrap();
    let mut other = PortMapper::new(socket, "127.0.0.1".parse().unwrap(), b"other");
    assert_eq!(other.renew(&wanted).await.failed.len(), 1);
    other.leases.clear();
    // Dropped with its lease, the second mapper deletes it.
    drop(second);
    for _ in 0..50 {
        if held.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(held.lock().unwrap().is_empty());
}

#[tokio::test]
async fn pcp_error_matching_rejects_stale_60_byte_replies_and_epoch_reset_remaps() {
    let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let server_addr = server.local_addr().unwrap();
    let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    client.connect(server_addr).await.unwrap();
    let wanted = [
        Mapping {
            protocol: Protocol::TCP,
            port: 47989,
        },
        Mapping {
            protocol: Protocol::UDP,
            port: 47998,
        },
    ];
    let local_ip = "127.0.0.1".parse().unwrap();
    let secret = b"epoch-secret";
    let expected_nonce = nonce(secret, local_ip);

    let reply_pkt = |result: u8, epoch: u32, reply_nonce: [u8; 12], proto: u8, port: u16| {
        let mut buf = [0u8; pcp::REQUEST_BYTES];
        buf[0] = pcp::VERSION;
        buf[1] = 0x80 | 1u8;
        buf[3] = result;
        buf[4..8].copy_from_slice(&LIFETIME.to_be_bytes());
        buf[8..12].copy_from_slice(&epoch.to_be_bytes());
        buf[24..36].copy_from_slice(&reply_nonce);
        buf[36] = proto;
        buf[40..42].copy_from_slice(&port.to_be_bytes());
        buf[42..44].copy_from_slice(&port.to_be_bytes());
        buf[56..60].copy_from_slice(&[198, 51, 100, 1]);
        buf
    };

    let srv = tokio::spawn(async move {
        let mut buf = [0u8; 128];
        // Round 1, port 47989: first send a stale 60-byte error for WRONG port/nonce, then real OK (epoch 100).
        let (len, peer) = server.recv_from(&mut buf).await.unwrap();
        assert_eq!(len, pcp::REQUEST_BYTES);
        let wrong = reply_pkt(7, 99, [9u8; 12], pcp::UDP, 47998);
        server.send_to(&wrong, peer).await.unwrap();
        let ok1 = reply_pkt(0, 100, expected_nonce, pcp::TCP, 47989);
        server.send_to(&ok1, peer).await.unwrap();

        // Round 1, port 47998: OK (epoch 101).
        let (_, peer) = server.recv_from(&mut buf).await.unwrap();
        let ok2 = reply_pkt(0, 101, expected_nonce, pcp::UDP, 47998);
        server.send_to(&ok2, peer).await.unwrap();

        // Round 2 after forcing lease due for 47989: router rebooted (epoch dropped to 5)!
        // Port 47989 triggers epoch reset, so PortMapper MUST immediately remap 47998 in this same round too.
        let (_, peer) = server.recv_from(&mut buf).await.unwrap();
        let port_req1 = u16::from_be_bytes([buf[40], buf[41]]);
        assert_eq!(port_req1, 47989);
        let ok3 = reply_pkt(0, 5, expected_nonce, pcp::TCP, 47989);
        server.send_to(&ok3, peer).await.unwrap();

        let (_, peer) = server.recv_from(&mut buf).await.unwrap();
        let port_req2 = u16::from_be_bytes([buf[40], buf[41]]);
        assert_eq!(port_req2, 47998);
        let ok4 = reply_pkt(0, 5, expected_nonce, pcp::UDP, 47998);
        server.send_to(&ok4, peer).await.unwrap();
    });

    let mut mapper = PortMapper::new(client, local_ip, secret);
    let round1 = mapper.renew(&wanted).await;
    assert_eq!(round1.renewed, wanted);
    assert!(round1.failed.is_empty());

    // Expire ONLY port 47989; keep 47998 valid far in the future.
    mapper.leases[0].renew_at = Instant::now() - Duration::from_secs(1);
    mapper.leases[1].renew_at = Instant::now() + Duration::from_secs(3600);
    let round2 = mapper.renew(&wanted).await;
    assert_eq!(
        round2.renewed, wanted,
        "epoch reset must remap both wanted ports immediately"
    );
    assert_eq!(round2.mapped.len(), 2);
    srv.await.unwrap();
}
