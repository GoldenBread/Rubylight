//! Reconnect harness port-slot reservation and UDP datagram resilience tests.
use super::{
    Arc, CLAIM, Duration, Instant, LossySocket, PORT_SLOTS, Ports, Result, claim_slot, port_base,
    reserve, thread, udp,
};

#[test]
fn port_bases_start_per_process_and_stay_in_range() {
    assert_eq!(port_base(0, 0), 22000);
    assert_eq!(port_base(7, 0), 22000 + 7 * 32);
    assert_eq!(port_base(PORT_SLOTS - 1, 1), 22000);
    let bases: std::collections::HashSet<_> = (0..PORT_SLOTS).map(|i| port_base(300, i)).collect();
    assert_eq!(bases.len(), usize::from(PORT_SLOTS));
    assert!(bases.iter().all(|b| (22000..40000 - 31).contains(b)));
}

/// While one harness holds a slot's claim, another run checking that slot skips it, even
/// before the host has bound any of the slot's ports.
#[test]
fn a_claimed_slot_is_skipped_until_its_claim_is_dropped() {
    let (ports, claim) = (0..PORT_SLOTS)
        .find_map(|i| claim_slot(port_base(150, i)))
        .expect("a free slot");
    let used = |p: Ports| {
        [
            p.http, p.https, p.web, p.rtsp, p.video, p.control, p.audio, p.mic,
        ]
    };
    let base = ports.http;
    assert!(!used(ports).contains(&(base + CLAIM)));
    assert!(!used(Ports::from_base(base + 32)).contains(&(base + CLAIM)));
    assert!(
        reserve(ports).is_ok(),
        "the claim holds none of the host's ports"
    );
    assert!(claim_slot(base).is_none());
    drop(claim);
    assert!(claim_slot(base).is_some());
}

/// A client can send its ENet connect before the host binds the control port. The port
/// unreachable that comes back loses that datagram; the client's worker goes on and connects on
/// ENet's retry.
#[test]
fn the_client_socket_outlives_a_datagram_to_a_closed_port() -> Result<()> {
    let closed = udp().local_addr()?;
    let mut socket = LossySocket {
        socket: udp(),
        blocked: Arc::default(),
        moved: Arc::default(),
    };
    rusty_enet::Socket::send(&mut socket, closed, b"connect")?;
    let mut buffer = [0; rusty_enet::MTU_MAX];
    let end = Instant::now() + Duration::from_millis(200);
    while Instant::now() < end {
        assert!(rusty_enet::Socket::receive(&mut socket, &mut buffer)?.is_none());
        thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}
