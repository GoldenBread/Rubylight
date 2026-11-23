//! Unit tests for pure mDNS discovery deduplication, generation guard, and IPv6 selection.

use super::*;

fn v6(s: &str) -> Ipv6Addr {
    s.parse().unwrap()
}

#[test]
fn ipv6_filters_and_slaac_suffix_matching() {
    let addrs = [
        v6("::"),
        v6("ff02::1"),
        v6("::1"),
        v6("fe80::1234:5678:9abc:def0"),
        v6("fec0::1"),
        v6("fc00::1"),
        v6("fd12:3456:789a::1"),
        v6("2002:c000:0201::1"),
        v6("2001:0::1"),
        v6("2001:db8::aaaa:bbbb:cccc:dddd"),
        v6("2001:db8::1234:5678:9abc:def0"),
    ];
    assert_eq!(link_local_ipv6_index(&addrs), Some(3));
    assert_eq!(local_ipv6_index(&addrs), Some(3));
    assert_eq!(best_ipv6_index(&addrs), Some(10));

    // Falls back to first non-excluded global when no SLAAC suffix matches.
    let no_match = [v6("fe80::1"), v6("2001:db8::2"), v6("2001:db8::3")];
    assert_eq!(best_ipv6_index(&no_match), Some(1));

    // Unspecified and multicast alone yield None.
    assert_eq!(best_ipv6_index(&[v6("::"), v6("ff02::1")]), None);
    assert_eq!(local_ipv6_index(&[v6("2001:db8::1")]), None);
    assert!(is_local_ipv6(v6("fd00::1")));
}

#[test]
fn discovery_state_dedup_generation_and_answered_replacement() {
    let mut state = DiscoveryState::new();
    let gen1 = state.reset();
    assert_eq!(gen1, 1);

    let pc_a = CandidateKey {
        name: "PC".into(),
        port: 47989,
        local: Some("192.0.2.10".into()),
        v6: None,
    };
    let pc_b = CandidateKey {
        name: "PC".into(),
        port: 47989,
        local: Some("192.0.2.20".into()),
        v6: None,
    };
    let other = CandidateKey {
        name: "Other".into(),
        port: 47989,
        local: Some("192.0.2.30".into()),
        v6: None,
    };

    // Stale generation is ignored without mutating computers.
    assert_eq!(
        state.begin_candidate(pc_a.clone(), 0),
        ProbeDecision::StaleGeneration
    );
    assert!(state.computers().is_empty());

    assert_eq!(state.begin_candidate(pc_a.clone(), gen1), ProbeDecision::Probe);
    assert_eq!(
        state.begin_candidate(pc_a.clone(), gen1),
        ProbeDecision::Duplicate
    );
    state.mark_answered(pc_a.clone());
    assert_eq!(
        state.begin_candidate(other.clone(), gen1),
        ProbeDecision::Probe
    );
    state.mark_answered(other.clone());

    // New session: PC at B is probed and replaces PC at A in `answered`.
    let gen2 = state.reset();
    assert_eq!(state.begin_candidate(pc_b.clone(), gen2), ProbeDecision::Probe);
    state.mark_answered(pc_b.clone());
    assert_eq!(
        state.begin_candidate(other.clone(), gen2),
        ProbeDecision::Duplicate
    );

    // Moving back to A is probed again because answered kept only B for "PC".
    let gen3 = state.reset();
    assert_eq!(state.begin_candidate(pc_a.clone(), gen3), ProbeDecision::Probe);
    state.mark_answered(pc_a.clone());

    // Same address in session 4 is skipped until forget_answered.
    let gen4 = state.reset();
    assert_eq!(
        state.begin_candidate(pc_a.clone(), gen4),
        ProbeDecision::Duplicate
    );
    state.forget_answered();
    let gen5 = state.reset();
    assert_eq!(state.begin_candidate(pc_a, gen5), ProbeDecision::Probe);
}
