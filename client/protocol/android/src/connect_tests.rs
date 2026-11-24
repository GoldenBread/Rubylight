//! Unit tests for Android connection preflight packet sizing, metrics poller slab,
//! and XML-free `ServerInfo` construction against special-character UUIDs.

use super::*;
use nightjar_core::connect::actions::serverinfo_confirms_ended_session;
use nightjar_policy::http::{AppRole, PERMISSION_LAUNCH, PERMISSION_VIEW};

#[test]
fn metrics_poller_slab_handles_terminal_403_and_clear() {
    let handle = METRICS_POLLERS.insert(HardwareMetricsPoller::new());
    let poller = METRICS_POLLERS.get(handle).expect("poller");
    let r#gen = poller.due(1000, true).expect("due");
    let (show, _, unavail) =
        poller.finish_step_sample(r#gen, 1100, Err(HardwareStepError::Unavailable));
    assert!(!show);
    assert!(unavail);
    assert!(poller.is_unavailable());
    METRICS_POLLERS.remove(handle);
    assert!(METRICS_POLLERS.get(handle).is_none());
}

#[test]
fn build_server_info_accepts_xml_special_chars_without_panicking() {
    for bad in ["bad<uuid", "a&b", "x]]>y\"'"] {
        let info = build_server_info(true, 42, bad, false, true, true);
        assert!(info.paired);
        assert_eq!(info.current_game, 42);
        assert_eq!(info.current_game_uuid.as_deref(), Some(bad));

        let idle_info = ServerInfo::synthetic(
            true,
            0,
            bad,
            true,
            false,
            false,
            Some(PERMISSION_VIEW | PERMISSION_LAUNCH),
        );
        assert!(!serverinfo_confirms_ended_session(
            false,
            AppRole::Stream,
            false,
            &idle_info,
        ));
    }
}

#[test]
fn resolve_packet_size_auto_vpn_uses_1024_while_explicit_remote_keeps_1392() {
    let probe_unused = |_, _, _, _| -> Result<Measurement, http::Error> {
        panic!("probe should not run for remote or resume")
    };
    assert_eq!(
        resolve_packet_size_values(0, false, true, 8192, true, 1392, false, false, probe_unused),
        [1024, 0]
    );
    assert_eq!(
        resolve_packet_size_values(0, false, true, 8192, true, 1392, true, false, probe_unused),
        [1024, 0]
    );
    assert_eq!(
        resolve_packet_size_values(2, false, true, 8192, true, 1392, false, false, probe_unused),
        [1392, 0]
    );
    assert_eq!(
        resolve_packet_size_values(0, false, true, 8192, true, 1200, false, false, probe_unused),
        [1200, 0]
    );
    // Resume skips jumbo probe even on Local + jumbo_frames = true.
    assert_eq!(
        resolve_packet_size_values(1, true, false, 8192, true, 1392, true, true, probe_unused),
        [1392, 0]
    );
}
