//! Integration tests verifying `HostRecord` typed endpoint slots stay synchronized with flat fields.

use nightjar_desktop_config::config::HostRecord;
use nightjar_policy::hosts::{EndpointSlot, HostSlots};

#[test]
fn host_record_syncs_shared_fields_and_slots_without_promoting_remote() {
    let mut remote_only = HostRecord {
        unique_id: "u-remote".into(),
        name: "RemoteOnly".into(),
        http_port: 47989,
        ..Default::default()
    };
    remote_only.set_slots(&HostSlots {
        remote: Some(EndpointSlot { address: "203.0.113.9".into(), port: 47989 }),
        ..Default::default()
    });
    remote_only.sync_shared_from_slots();
    assert_eq!(remote_only.address, "", "remote slot must never be promoted into primary address");
    assert_eq!(remote_only.remote_address.as_deref(), Some("203.0.113.9"));

    let mut legacy = HostRecord {
        unique_id: "u-legacy".into(),
        name: "Legacy".into(),
        address: "192.168.1.20".into(),
        http_port: 47989,
        remote_address: Some("203.0.113.10".into()),
        ..Default::default()
    };
    legacy.sync_slots_from_shared();
    assert_eq!(legacy.slots().local, Some(EndpointSlot { address: "192.168.1.20".into(), port: 47989 }));
    assert_eq!(legacy.slots().remote, Some(EndpointSlot { address: "203.0.113.10".into(), port: 47989 }));

    let mut other_only = HostRecord {
        unique_id: "u-other-v6".into(),
        name: "OtherV6".into(),
        http_port: 47989,
        other_addresses: vec!["2001:db8::7".into()],
        ..Default::default()
    };
    other_only.sync_slots_from_shared();
    assert_eq!(other_only.slots().ipv6, None);
    other_only.sync_shared_from_slots();
    assert_eq!(other_only.address, "");
    assert_eq!(other_only.other_addresses, vec!["2001:db8::7".to_string()]);
}
