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
