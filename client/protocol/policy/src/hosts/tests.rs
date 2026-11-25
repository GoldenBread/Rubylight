//! Unit tests for pure host schemas, pinned address parsing, alias limits, notice/mode
//! policies, capability snapshot normalization, and unreadable host filtering.

use super::*;
use crate::http::{HostCapabilities, ServerInfo};

#[test]
fn pinned_address_round_trips_ipv4_and_ipv6() {
    let v4 = EndpointSlot::new("192.168.1.20", 47989).unwrap();
    let encoded = PinnedAddress::format(&v4);
    assert_eq!(encoded, "192.168.1.20 47989");
    assert_eq!(PinnedAddress::parse(&encoded), Some(v4));

    let v6 = EndpointSlot::new("[2001:db8::7]", 47989).unwrap();
    assert_eq!(v6.address, "2001:db8::7");
    assert_eq!(v6.to_string(), "[2001:db8::7]:47989");
    let encoded_v6 = PinnedAddress::format(&v6);
    assert_eq!(PinnedAddress::parse(&encoded_v6), Some(v6));

    assert_eq!(PinnedAddress::parse("invalid"), None);
    assert_eq!(PinnedAddress::parse(" 47989"), None);
    assert_eq!(PinnedAddress::parse("192.168.1.1 0"), None);
    assert_eq!(PinnedAddress::parse("192.168.1.1 70000"), None);
}

#[test]
fn alias_normalization_respects_codepoints_and_own_name() {
    assert_eq!(
        HostNames::normalize_alias(Some("  Den PC \n"), Some("DESKTOP-1")),
        Some("Den PC".into())
    );
    let emoji = "🖥".repeat(ALIAS_MAX_CHARS + 5);
    assert_eq!(
        HostNames::normalize_alias(Some(&emoji), Some("x")),
        Some("🖥".repeat(ALIAS_MAX_CHARS))
    );
    assert_eq!(HostNames::normalize_alias(None, Some("PC")), None);
    assert_eq!(HostNames::normalize_alias(Some("   "), Some("PC")), None);
    assert_eq!(HostNames::normalize_alias(Some(" PC "), Some("PC")), None);
    assert_eq!(
        HostNames::normalize_alias(Some("pc"), Some("PC")),
        Some("pc".into())
    );
}

#[test]
fn notices_packed_modes_and_capability_snapshot_behave_consistently() {
    assert_eq!(HostNotice::all_count_check(), 20);
    let mut notices = HostNoticesState::default();
    assert!(notices.claim("pc", HostNotice::VirtualDisplayUnavailable.as_str()));
    assert!(!notices.claim("pc", HostNotice::VirtualDisplayUnavailable.as_str()));
    assert!(notices.claim("other", HostNotice::VirtualDisplayUnavailable.as_str()));
    assert!(notices.reset_everywhere(HostNotice::VirtualDisplayUnavailable.as_str()));
    assert!(notices.claim("pc", HostNotice::VirtualDisplayUnavailable.as_str()));
    assert!(notices.forget("pc"));

    let mut packed = HostPackedModesState::default();
    assert_eq!(packed.cached("pc"), -1);
    assert!(packed.store("pc", 0x15));
    assert_eq!(packed.cached("pc"), 0b0101);
    assert!(!packed.store("pc", 0x15));
    assert!(packed.mark_refused("pc"));
    assert!(packed.refused("pc"));
    // DESCRIBE 0 clears modes while preserving refusal.
    assert!(packed.store("pc", 0));
    assert_eq!(packed.cached("pc"), -1);
    assert!(packed.refused("pc"));
    // Renewing under a new version stamp clears refusal.
    let s1 = HostPackedModesState::stamp("0.9.0", Some("3.11.2"));
    assert!(packed.renew("pc", &s1));
    assert!(!packed.refused("pc"));
    assert_eq!(HostPackedModesState::known(-1, 0b0011), 0b0011);
    assert_eq!(HostPackedModesState::known(0, 0b0011), 0);

    // Capability snapshot normalizes and rejects damaged JSON.
    let info = ServerInfo::parse(
        "<root status_code=\"200\"><hostname>PC</hostname><appversion>7.1.431.-1</appversion><uniqueid>id</uniqueid><state>SUNSHINE_SERVER_BUSY</state><currentgame>0</currentgame><PairStatus>1</PairStatus></root>",
        47989,
    )
    .unwrap();
    let raw = info.capabilities.to_versioned_json().unwrap();
    let snap1 = HostCapabilitiesSnapshot::parse(&raw).unwrap();
    let mut obj: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&raw).unwrap();
    obj.remove("hdrFormats");
    let older_raw = serde_json::Value::Object(obj).to_string();
    let snap2 = HostCapabilitiesSnapshot::parse(&older_raw).unwrap();
    assert_eq!(snap1, snap2);
    assert!(HostCapabilitiesSnapshot::parse("{not json").is_none());
    assert!(!HostCapabilities::UNKNOWN.to_versioned_json().is_some());
}

impl HostNotice {
    fn all_count_check() -> usize {
        [
            Self::VirtualDisplayUnavailable,
            Self::AutomaticQualityUnavailable,
            Self::PyroWaveUnavailable,
            Self::HdrUnavailable,
            Self::PackedHdrUnavailable,
            Self::PyroWaveHdrUnavailable,
            Self::VulkanHdrLimit,
            Self::FrameWarpHdrLimit,
            Self::PyroWaveHdrLimit,
            Self::CodecUnavailable,
            Self::PyroWaveFallback,
            Self::Host4kUnavailable,
            Self::NativeTouchUnavailable,
            Self::Sdr10Unavailable,
            Self::Sdr10NotAccepted,
            Self::Sdr10HostHdr,
            Self::VulkanHdr10PlusRefused,
            Self::VulkanBehind,
            Self::PackedUnavailable,
            Self::NativeTouchpadUnavailable,
        ]
        .iter()
        .filter(|n| Self::parse(n.as_str()) == Some(**n))
        .count()
    }
}

#[test]
fn sync_shared_from_slots_prefers_local_then_manual_then_ipv6_never_remote() {
    use super::{sync_shared_from_slots, sync_slots_from_shared, EndpointSlot, HostSlots, SharedHostFields};

    // Remote-only MUST leave shared.address empty and never promote remote into shared.address.
    let mut shared = SharedHostFields::default();
    let slots = HostSlots {
        remote: Some(EndpointSlot {
            address: "203.0.113.10".into(),
            port: 47989,
        }),
        ..Default::default()
    };
    sync_shared_from_slots(&mut shared, &slots);
    assert_eq!(shared.address, "");
    assert_eq!(shared.remote_address.as_deref(), Some("203.0.113.10"));
    assert_eq!(shared.http_port, 47989);
    assert!(!shared.manual);
    assert!(slots.has_remote());

    // Manual without local sets manual = true and populates other_addresses with ipv6.
    let mut shared2 = SharedHostFields::default();
    let slots2 = HostSlots {
        manual: Some(EndpointSlot {
            address: "10.0.0.5".into(),
            port: 48000,
        }),
        ipv6: Some(EndpointSlot {
            address: "2001:db8::9".into(),
            port: 47989,
        }),
        remote: Some(EndpointSlot {
            address: "203.0.113.10".into(),
            port: 47989,
        }),
        ..Default::default()
    };
    sync_shared_from_slots(&mut shared2, &slots2);
    assert_eq!(shared2.address, "10.0.0.5");
    assert_eq!(shared2.http_port, 48000);
    assert!(shared2.manual);
    assert_eq!(shared2.remote_address.as_deref(), Some("203.0.113.10"));
    assert_eq!(shared2.other_addresses, vec!["2001:db8::9"]);
    assert_eq!(shared2.other_http_ports, vec![47989]);

    // Local present overrides manual flag and becomes primary while manual & ipv6 move to other_addresses.
    let mut shared3 = SharedHostFields::default();
    let slots3 = HostSlots {
        local: Some(EndpointSlot {
            address: "192.168.1.20".into(),
            port: 47989,
        }),
        manual: slots2.manual.clone(),
        ipv6: slots2.ipv6.clone(),
        remote: slots2.remote.clone(),
        pinned: Some(EndpointSlot {
            address: "192.168.1.20".into(),
            port: 47989,
        }),
    };
    sync_shared_from_slots(&mut shared3, &slots3);
    assert_eq!(shared3.address, "192.168.1.20");
    assert!(!shared3.manual);
    assert_eq!(shared3.pinned_address.as_deref(), Some("192.168.1.20"));
    assert_eq!(shared3.other_addresses, vec!["10.0.0.5", "2001:db8::9"]);

    // Reverse sync recovers missing slots from Desktop flat SharedHostFields.
    let mut recovered = HostSlots::default();
    sync_slots_from_shared(&shared2, &mut recovered);
    assert_eq!(
        recovered.manual.as_ref().map(|e| e.address.as_str()),
        Some("10.0.0.5")
    );
    assert_eq!(
        recovered.remote.as_ref().map(|e| e.address.as_str()),
        Some("203.0.113.10")
    );
    assert!(recovered.local.is_none());
}

#[test]
fn site_local_and_public_ip_helpers_classify_rfc1918_v6_and_public() {
    use super::{is_site_local_ip, is_site_local_ipv4, public_ipv4};
    use std::net::{IpAddr, Ipv4Addr};

    assert!(is_site_local_ipv4(Ipv4Addr::new(10, 0, 0, 1)));
    assert!(is_site_local_ipv4(Ipv4Addr::new(172, 16, 5, 4)));
    assert!(!is_site_local_ipv4(Ipv4Addr::new(172, 32, 0, 1)));
    assert!(is_site_local_ipv4(Ipv4Addr::new(192, 168, 1, 1)));
    assert!(!is_site_local_ipv4(Ipv4Addr::new(8, 8, 8, 8)));

    assert!(is_site_local_ip("192.168.1.50".parse::<IpAddr>().unwrap()));
    assert!(is_site_local_ip("fd00::1".parse::<IpAddr>().unwrap()));
    assert!(is_site_local_ip("fec0::1".parse::<IpAddr>().unwrap()));
    assert!(!is_site_local_ip("fe80::1".parse::<IpAddr>().unwrap()));
    assert!(!is_site_local_ip("2001:db8::1".parse::<IpAddr>().unwrap()));
    assert!(!is_site_local_ip("203.0.113.9".parse::<IpAddr>().unwrap()));

    assert_eq!(public_ipv4("8.8.4.4"), Some("8.8.4.4".into()));
    assert_eq!(public_ipv4("203.0.113.9"), None);
    assert_eq!(public_ipv4("192.168.1.5"), None);
    assert_eq!(public_ipv4("127.78.0.2"), None);
    assert_eq!(public_ipv4("not-an-ip"), None);
}

#[test]
fn poll_policy_probe_plan_punches_now_and_tuple_transitions() {
    use super::{
        accept_applist_response, port_matches_active_address, probe_plan, punches_now,
        should_clear_cert_on_unpair, CandidateSlot, EndpointSlot, HostReachState, HostSlots,
        PollTupleState,
    };

    let slots = HostSlots {
        local: Some(EndpointSlot { address: "192.168.1.10".into(), port: 47989 }),
        manual: Some(EndpointSlot { address: "10.0.0.2".into(), port: 48000 }),
        ipv6: Some(EndpointSlot { address: "2001:db8::2".into(), port: 47989 }),
        // Duplicate of local to confirm dedup:
        remote: Some(EndpointSlot { address: "192.168.1.10".into(), port: 47989 }),
        pinned: None,
    };
    let plan = probe_plan(&slots, true, Some(48000), 47984);
    assert_eq!(plan.len(), 3);
    assert_eq!(plan[0].slot, CandidateSlot::Local);
    assert_eq!(plan[0].https_port, 0);
    assert_eq!(plan[1].slot, CandidateSlot::Manual);
    assert_eq!(plan[1].https_port, 47984);
    assert_eq!(plan[2].slot, CandidateSlot::Ipv6);

    // Pinned suppresses all other slots.
    let mut pinned_slots = slots.clone();
    pinned_slots.pinned = Some(EndpointSlot { address: "10.0.0.2".into(), port: 48000 });
    let pinned_plan = probe_plan(&pinned_slots, true, Some(48000), 47984);
    assert_eq!(pinned_plan.len(), 1);
    assert_eq!(pinned_plan[0].slot, CandidateSlot::Pinned);
    assert_eq!(pinned_plan[0].https_port, 47984);

    assert_eq!(port_matches_active_address(false, Some(47989), 47989, 47984), 0);
    assert!(!punches_now(false, false, false));
    assert!(punches_now(true, true, false));
    assert!(punches_now(true, false, true));

    assert!(should_clear_cert_on_unpair(false, false, true));
    assert!(!should_clear_cert_on_unpair(true, false, true));
    assert!(!should_clear_cert_on_unpair(false, true, true));
    assert!(!should_clear_cert_on_unpair(false, false, false));

    let mut tuple = PollTupleState::default();
    assert!(!tuple.record_poll_failure(true)); // 1
    assert!(!tuple.record_poll_failure(true)); // 2
    assert!(tuple.record_poll_failure(true));  // offline after threshold
    assert_eq!(tuple.state, HostReachState::Offline);
    tuple.record_poll_success(1_000);
    assert_eq!(tuple.state, HostReachState::Online);
    assert!(!tuple.is_ttl_expired(31_000));
    assert!(tuple.is_ttl_expired(31_001));
    tuple.enforce_ttl(31_001);
    assert_eq!(tuple.state, HostReachState::Unknown);

    let mut empty = 0;
    assert!(!accept_applist_response(false, true, &mut empty));
    assert!(!accept_applist_response(true, true, &mut empty));
    assert!(!accept_applist_response(true, true, &mut empty));
    assert!(accept_applist_response(true, true, &mut empty));
    assert!(accept_applist_response(true, false, &mut empty));
    assert_eq!(empty, 0);
}

#[test]
fn power_and_update_tracker_handle_suspend_and_waiting_budgets() {
    use super::{
        classify_sleep_unavailable, sleep_option, SleepOfferState, SleepUnavailableReason,
        TokenStamp, UpdateStepAction, UpdateTracker, SLEEP_TOKEN_REFRESH_MS,
    };
    use crate::http::{Evidence, HostCapabilities, HostUpdate};

    let stamp = TokenStamp { mono_ms: 10_000, wall_ms: 1_000_000 };
    assert!(!stamp.is_stale(10_000 + SLEEP_TOKEN_REFRESH_MS - 1, 1_000_000 + SLEEP_TOKEN_REFRESH_MS - 1));
    // Wall clock jump triggers refresh even if monotonic didn't advance.
    assert!(stamp.is_stale(10_500, 1_000_000 + SLEEP_TOKEN_REFRESH_MS));

    assert_eq!(classify_sleep_unavailable(false, false), SleepUnavailableReason::Unsupported);
    assert_eq!(classify_sleep_unavailable(true, false), SleepUnavailableReason::NotAllowed);
    assert_eq!(classify_sleep_unavailable(true, true), SleepUnavailableReason::Busy);

    let caps_denied = HostCapabilities::from_evidence(Evidence {
        paired: true,
        rust_host_version: Some("3.4.0".into()),
        power_action_version: 1,
        sleep_supported: true,
        sleep_allowed: false,
        ..Evidence::default()
    });
    assert_eq!(sleep_option(true, true, Some(&caps_denied)), SleepOfferState::PermissionDenied);
    assert_eq!(sleep_option(false, true, Some(&caps_denied)), SleepOfferState::Hidden);

    let caps_offered = HostCapabilities::from_evidence(Evidence {
        paired: true,
        rust_host_version: Some("3.4.0".into()),
        power_action_version: 1,
        sleep_supported: true,
        sleep_allowed: true,
        ..Evidence::default()
    });
    assert_eq!(sleep_option(true, true, Some(&caps_offered)), SleepOfferState::Offered);

    let mut tracker = UpdateTracker::with_limits("3.5.0", 2, 4);
    let waiting_reply = HostUpdate {
        current_version: "3.4.0".into(),
        latest_version: Some("3.5.0".into()),
        phase: "waiting".into(),
        waiting_for: Some("Stream running".into()),
        ..Default::default()
    };
    assert_eq!(
        tracker.step(None, Some(&waiting_reply)),
        UpdateStepAction::Waiting { reason: "Stream running".into() }
    );
    // Repeated waiting polls do not consume counted budget until max_polls.
    assert_eq!(tracker.step(None, Some(&waiting_reply)), UpdateStepAction::Continue);
    assert_eq!(tracker.counted, 0);
    // Once gone and version matches target, it completes.
    assert_eq!(tracker.step(None, None), UpdateStepAction::Continue);
    assert!(tracker.gone);
    assert_eq!(
        tracker.step(Some("v3.5.0"), None),
        UpdateStepAction::Back { version: "3.5.0".into() }
    );
}

#[test]
fn pairing_and_invite_policies_sanitize_classify_and_track_generations() {
    use super::{
        already_paired, classify_invite_error, classify_pairing_failure, classify_unpair,
        invite_claims_known_pc, is_tunnel_address, strip_bidi_controls, InviteErrorKind,
        PairFailureKind, PairFailureState, PairingWatchCommand, PairingWatchFinish,
        PairingWatchTracker, UnpairFailureKind, KNOWN_PC,
    };

    assert_eq!(strip_bidi_controls("PC\u{202E}txt\u{200D}🎮\u{2066}"), "PCtxt\u{200D}🎮");
    assert!(!already_paired(true, true, false));
    assert!(already_paired(true, true, true));
    assert_eq!(
        classify_pairing_failure(PairFailureState::PinWrong, false, true, false),
        PairFailureKind::OneTimePinWrong
    );
    assert_eq!(
        classify_unpair(Some(401), false, false, false),
        UnpairFailureKind::NotPaired
    );

    let mut watch = PairingWatchTracker::new();
    let PairingWatchCommand::StartPoll { generation: g1 } = watch.begin_poll(1_000, true) else {
        panic!("expected StartPoll");
    };
    watch.pause();
    assert_eq!(watch.finish_poll(g1, vec!["req1".into()]), PairingWatchFinish::Stale);
    let PairingWatchCommand::StartPoll { generation: g2 } = watch.begin_poll(1_500, true) else {
        panic!("expected StartPoll after pause");
    };
    assert_eq!(watch.finish_poll(g2, vec!["req1".into()]), PairingWatchFinish::Changed);
    assert_eq!(watch.begin_poll(2_000, false), PairingWatchCommand::PublishEmpty);

    assert_eq!(classify_invite_error(KNOWN_PC), InviteErrorKind::KnownPc);
    assert!(invite_claims_known_pc(Some([1u8; 32]), [2u8; 32]));
    assert!(!invite_claims_known_pc(Some([1u8; 32]), [1u8; 32]));
    assert!(!invite_claims_known_pc(None, [1u8; 32]));
    assert!(is_tunnel_address("127.78.0.2"));
    assert!(!is_tunnel_address("127.0.0.1"));
}

#[test]
fn sync_slots_from_shared_never_promotes_other_addresses_ipv6_with_default_port() {
    use crate::hosts::{sync_shared_from_slots, sync_slots_from_shared, HostSlots, SharedHostFields};

    let mut shared = SharedHostFields {
        unique_id: "u-v6-other".into(),
        name: "V6Other".into(),
        http_port: 47989,
        other_addresses: vec!["2001:db8::42".into()],
        ..Default::default()
    };
    let mut slots = HostSlots::default();
    sync_slots_from_shared(&shared, &mut slots);
    assert_eq!(slots.ipv6, None, "other_addresses IPv6 must not become ipv6 slot");

    sync_shared_from_slots(&mut shared, &slots);
    assert_eq!(shared.address, "", "primary address must stay empty");
    assert_eq!(shared.other_addresses, vec!["2001:db8::42".to_string()]);
}

#[test]
fn cgnat_tailscale_classification_and_extra_endpoint_probing() {
    use super::{
        is_cgnat_ipv4, is_overlay_ip, is_site_local_ip, probe_plan_with_extras, public_ipv4,
        CandidateSlot, EndpointSlot, HostSlots,
    };
    use std::net::{IpAddr, Ipv4Addr};

    // RFC 6598 100.64.0.0/10 (Tailscale / CGNAT) is overlay, neither LAN nor public WAN.
    assert!(is_cgnat_ipv4(Ipv4Addr::new(100, 64, 0, 1)));
    assert!(is_cgnat_ipv4(Ipv4Addr::new(100, 127, 255, 254)));
    assert!(!is_cgnat_ipv4(Ipv4Addr::new(100, 63, 255, 255)));
    assert!(!is_cgnat_ipv4(Ipv4Addr::new(100, 128, 0, 1)));
    assert_eq!(public_ipv4("100.64.1.2"), None);
    assert_eq!(public_ipv4("100.127.255.254"), None);
    assert_eq!(public_ipv4("100.128.0.1"), Some("100.128.0.1".into()));
    assert!(is_overlay_ip("100.100.1.2".parse::<IpAddr>().unwrap()));
    assert!(is_overlay_ip("fd7a:115c:a1e0::42".parse::<IpAddr>().unwrap()));
    assert!(!is_site_local_ip("100.100.1.2".parse::<IpAddr>().unwrap()));
    assert!(!is_site_local_ip("fd7a:115c:a1e0::42".parse::<IpAddr>().unwrap()));
    assert!(is_site_local_ip("fd12:3456::1".parse::<IpAddr>().unwrap()));

    // 3+ host endpoints: LAN + Tailscale + WAN + extra saved addresses are all probed.
    let slots = HostSlots {
        local: Some(EndpointSlot { address: "192.168.1.10".into(), port: 47989 }),
        manual: Some(EndpointSlot { address: "100.64.1.2".into(), port: 47989 }),
        ipv6: None,
        remote: Some(EndpointSlot { address: "8.8.4.4".into(), port: 47989 }),
        pinned: None,
    };
    let extras = vec![
        "192.168.1.10".into(),      // duplicate of local
        "10.0.0.50".into(),         // extra LAN
        "100.100.5.6".into(),       // extra Tailscale
        "1.1.1.1".into(),           // extra WAN
        "2001:db8::9".into(),       // extra IPv6
    ];
    let plan = probe_plan_with_extras(&slots, &extras, 47989, false, None, 0);
    let addrs: Vec<&str> = plan.iter().map(|c| c.address.as_str()).collect();
    assert_eq!(
        addrs,
        vec!["192.168.1.10", "100.64.1.2", "8.8.4.4", "10.0.0.50", "100.100.5.6", "1.1.1.1", "2001:db8::9"]
    );
    assert_eq!(plan[3].slot, CandidateSlot::Manual);
    assert_eq!(plan[4].slot, CandidateSlot::Manual);
    assert_eq!(plan[5].slot, CandidateSlot::Remote);
    assert_eq!(plan[6].slot, CandidateSlot::Ipv6);

    // Pinned address still suppresses all extra candidates.
    let mut pinned = slots;
    pinned.pinned = Some(EndpointSlot { address: "100.64.1.2".into(), port: 47989 });
    let pinned_plan = probe_plan_with_extras(&pinned, &extras, 47989, false, None, 0);
    assert_eq!(pinned_plan.len(), 1);
    assert_eq!(pinned_plan[0].slot, CandidateSlot::Pinned);
}
