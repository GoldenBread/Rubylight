//! Unit tests for pure Wake-on-LAN target parsing, burst planning, poll cooldown, and retry state.

use super::*;
use rubylight_native_api::wake::{Target, Wake};

#[test]
fn leading_zero_ipv4_literals_normalize_and_non_literals_drop() {
    assert_eq!(parse_ipv4_literal("192.168.004.255"), Some([192, 168, 4, 255]));
    assert_eq!(parse_ipv4_literal("0.0.0.0"), Some([0, 0, 0, 0]));
    assert_eq!(parse_ipv4_literal("256.0.0.1"), None);
    assert_eq!(parse_ipv4_literal("1.2.3"), None);
    assert_eq!(parse_ipv4_literal("1.2.3.4.5"), None);
    assert_eq!(parse_ipv4_literal("pc.lan"), None);
    assert_eq!(parse_ipv4_literal("192.168.1.1 "), None);
    assert_eq!(parse_ipv4_literal("192.168.0001.1"), None);
}

#[test]
fn targets_normalize_leading_zero_broadcasts() {
    let mut wake: Wake = serde_json::from_str(
        r#"{"revision":1,"primary":"02:7C:16:B4:13:4F","targets":[],"system":{"sleep":"s3","modern_standby":false,"hibernate":false,"fast_startup":"inactive","shutdown_wake":"firmware_unknown"},"idle_sleep":{"state":"off","minutes":0,"reason":null}}"#,
    )
    .unwrap();
    wake.targets = vec![Target {
        mac: "02:7c:16:b4:13:4f".into(),
        broadcasts: vec!["192.168.004.255".into(), "pc.example".into(), "192.168.4.255".into()],
        ..serde_json::from_str(r#"{"mac":"","kind":"ethernet","connected":true,"route":true,"broadcasts":[],"state":"ready","checks":[]}"#).unwrap()
    }];
    let (macs, broadcasts) = targets(&wake);
    assert_eq!(macs, ["02:7c:16:b4:13:4f"]);
    assert_eq!(broadcasts, ["192.168.4.255"]);
}

#[test]
fn build_plan_combines_endpoints_broadcasts_and_ipv6_scopes() {
    let links = Links {
        broadcasts: BTreeSet::from([Ipv4Addr::new(192, 168, 1, 255)]),
        ipv6: BTreeSet::from([3]),
    };
    let plan = build_plan(
        &["02-7C-16-B4-13-4F", "02:7c:16:b4:13:4f"],
        &[(IpAddr::V4(Ipv4Addr::new(192, 168, 4, 20)), 47989)],
        &[47989],
        &["10.0.0.255", "192.168.001.255"],
        &links,
    )
    .unwrap();
    assert_eq!(plan.payloads.len(), 1);
    assert!(plan.destinations.contains(&"192.168.4.20:47998".parse().unwrap()));
    assert!(plan.destinations.contains(&"255.255.255.255:9".parse().unwrap()));
    assert!(plan.destinations.contains(&"192.168.1.255:7".parse().unwrap()));
    assert!(plan.destinations.contains(&"10.0.0.255:47009".parse().unwrap()));
    assert_eq!(plan.best_effort.len(), ports(47989).len());
}

#[test]
fn wake_launch_and_poll_state_enforce_timeouts_and_reservations() {
    let mut launch = WakeLaunch::with(1_000, 60_000, 8_000);
    assert_eq!(launch.next(false, 1_000), WakeStep::Send);
    assert_eq!(launch.next(false, 8_999), WakeStep::Wait);
    assert_eq!(launch.next(false, 9_000), WakeStep::Send);
    assert_eq!(launch.next(false, 61_000), WakeStep::TimedOut);
    assert_eq!(launch.next(true, 61_000), WakeStep::Online);

    let mut poll = WakeTargetsPollState::new();
    let t1 = poll.reserve("pc", true, true, 1_000).unwrap();
    assert!(poll.is_active("pc", t1));
    assert_eq!(poll.reserve("pc", true, true, 30_999), None);
    let t2 = poll.reserve("pc", true, true, 31_000).unwrap();
    assert!(!poll.is_active("pc", t1));
    assert!(poll.is_active("pc", t2));
    poll.drop_ask("pc", t2);
    assert!(!poll.is_active("pc", t2));
    let t3 = poll.reserve("pc", true, true, 31_000).unwrap();
    poll.forget("pc");
    assert!(!poll.is_active("pc", t3));
    assert_eq!(poll.reserve("pc", false, true, 32_000), None);
    let t4 = poll.reserve("pc", true, true, 32_000).unwrap();
    poll.shutdown();
    assert!(!poll.is_active("pc", t4));
}
