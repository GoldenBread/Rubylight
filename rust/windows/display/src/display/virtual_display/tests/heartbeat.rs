//! Tests for virtual display driver retries, startup presence handover, and heartbeats.

use super::super::*;
#[test]
fn a_busy_driver_request_recovers_within_the_same_call() {
    let mut attempts = 0;
    let result = retry_busy(
        || {
            attempts += 1;
            if attempts < 3 {
                Err(windows::core::Error::from_hresult(ERROR_BUSY.to_hresult()).into())
            } else {
                Ok(vec![17, 23])
            }
        },
        Duration::from_secs(1),
    )
    .unwrap();
    assert_eq!(result, [17, 23]);
    assert_eq!(attempts, 3);
}
#[test]
fn driver_retries_stop_at_the_deadline_and_preserve_other_errors() {
    for code in [
        ERROR_BUSY,
        ERROR_ACCESS_DENIED,
        ERROR_NOT_FOUND,
        ERROR_GEN_FAILURE,
    ] {
        let mut attempts = 0;
        let timeout = if code == ERROR_BUSY {
            Duration::ZERO
        } else {
            Duration::from_secs(1)
        };
        let error = retry_busy::<()>(
            || {
                attempts += 1;
                Err(windows::core::Error::from_hresult(code.to_hresult()).into())
            },
            timeout,
        )
        .unwrap_err();
        assert_eq!(attempts, 1);
        assert_eq!(
            error.downcast_ref::<windows::core::Error>().unwrap().code(),
            code.to_hresult()
        );
    }
}
#[test]
fn an_owned_monitor_can_reappear_during_session_handover() {
    let mut attempts = 0;
    let found = wait_for_monitor(
        || {
            attempts += 1;
            Ok((attempts == 3).then(|| crate::display::monitor("owned", 4, false)))
        },
        || Ok(()),
        Duration::from_secs(1),
    )
    .unwrap();
    assert_eq!(found.device_id, "owned");
    assert_eq!(attempts, 3);
}
#[test]
fn startup_recovers_a_connected_monitor_instead_of_only_polling_it() {
    let renewals = std::cell::Cell::new(0);
    let found = wait_for_monitor(
        || Ok((renewals.get() == 2).then(|| crate::display::monitor("owned", 4, false))),
        || {
            renewals.set(renewals.get() + 1);
            Ok(())
        },
        Duration::from_secs(2),
    )
    .unwrap();
    assert_eq!(found.device_id, "owned");
    assert_eq!(renewals.get(), 2);
}
#[test]
fn startup_does_not_reactivate_a_present_monitor_or_hide_a_lease_error() {
    assert!(
        wait_for_monitor(
            || Ok(Some(crate::display::monitor("owned", 4, false))),
            || panic!("an active display needs no recovery"),
            Duration::ZERO,
        )
        .is_ok()
    );
    let error = wait_for_monitor(
        || Ok(None),
        || Err(windows::core::Error::from_hresult(ERROR_ACCESS_DENIED.to_hresult()).into()),
        Duration::from_secs(1),
    )
    .err()
    .unwrap();
    assert_eq!(
        error.downcast_ref::<windows::core::Error>().unwrap().code(),
        ERROR_ACCESS_DENIED.to_hresult()
    );
}
#[test]
fn an_absent_owned_monitor_or_failed_query_does_not_wait_forever() {
    let mut attempts = 0;
    assert!(
        wait_for_monitor(
            || {
                attempts += 1;
                Ok(None)
            },
            || panic!("expired lookup must not change the topology"),
            Duration::ZERO,
        )
        .is_err()
    );
    assert_eq!(attempts, 1);
    let error = wait_for_monitor(
        || Err(windows::core::Error::from_hresult(ERROR_ACCESS_DENIED.to_hresult()).into()),
        || panic!("failed query must not change the topology"),
        Duration::from_secs(1),
    )
    .err()
    .unwrap();
    assert_eq!(
        error.downcast_ref::<windows::core::Error>().unwrap().code(),
        ERROR_ACCESS_DENIED.to_hresult()
    );
}
#[test]
fn only_a_missing_driver_lease_allows_recreation() {
    assert!(lease_exists(Ok(())).unwrap());
    for code in [
        ERROR_BUSY,
        ERROR_RETRY,
        ERROR_GEN_FAILURE,
        ERROR_ACCESS_DENIED,
        ERROR_INVALID_HANDLE,
    ] {
        let error = windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(code.0));
        assert!(lease_exists(Err(error.into())).is_err(), "{code:?}");
        assert!(
            lease_exists(Ok(())).unwrap(),
            "the next heartbeat can succeed"
        );
    }
    let error =
        windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(ERROR_NOT_FOUND.0));
    assert!(!lease_exists(Err(error.into())).unwrap());
}
#[test]
fn heartbeat_recreates_only_a_missing_lease_and_switches_on_a_display_windows_turned_off() {
    let failure = |code: WIN32_ERROR| -> anyhow::Error {
        windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(code.0)).into()
    };
    let busy = || lease_exists(Err(failure(ERROR_BUSY)));
    let missing = || lease_exists(Err(failure(ERROR_NOT_FOUND)));
    // The log this guards against: the lease renewed, Windows had switched
    // the display off, and a duplicate create returned ERROR_BUSY.
    assert_eq!(
        heartbeat(
            Ok(true),
            true,
            || Ok(true),
            || panic!("present needs no relisting lookup"),
            || panic!("present needs no full query")
        )
        .unwrap(),
        Heartbeat::Present
    );
    assert_eq!(
        heartbeat(Ok(true), true, || Ok(false), || Ok(false), || Ok(true)).unwrap(),
        Heartbeat::SwitchOn
    );
    // Issue 56: Windows showed the display under another adapter and
    // target, its old route stayed listed but unavailable, and switching
    // that route on failed every second. A display Windows shows is
    // followed, never switched on.
    assert_eq!(
        heartbeat(
            Ok(true),
            true,
            || Ok(false),
            || Ok(true),
            || panic!("a relisted display is not switched on")
        )
        .unwrap(),
        Heartbeat::Relisted
    );
    // Not yet shown anywhere, or a query failed: retry, never create.
    assert!(heartbeat(Ok(true), true, || Ok(false), || Ok(false), || Ok(false)).is_err());
    assert!(
        heartbeat(
            Ok(true),
            true,
            || Err(failure(ERROR_BUSY)),
            || panic!("no relisting lookup"),
            || panic!("no full query")
        )
        .is_err()
    );
    assert!(
        heartbeat(
            Ok(true),
            true,
            || Ok(false),
            || Err(failure(ERROR_BUSY)),
            || panic!("no full query")
        )
        .is_err()
    );
    assert!(
        heartbeat(
            Ok(true),
            true,
            || Ok(false),
            || Ok(false),
            || Err(failure(ERROR_BUSY))
        )
        .is_err()
    );
    // A renewal failure other than a missing lease is retried as well,
    // without touching the topology.
    assert!(
        heartbeat(
            busy(),
            true,
            || panic!("no topology query"),
            || panic!("no topology query"),
            || panic!("no topology query")
        )
        .is_err()
    );
    // A lease the driver no longer knows is a real loss: recreate at once,
    // whatever Windows still lists.
    assert_eq!(
        heartbeat(
            missing(),
            true,
            || panic!("no topology query"),
            || panic!("no topology query"),
            || panic!("no topology query")
        )
        .unwrap(),
        Heartbeat::Recreate
    );
}
#[test]
fn a_paused_heartbeat_keeps_local_layout_changes_and_defers_recreation() {
    // A Win+P layout can leave our display connected but inactive; a
    // driver restart can remove its lease. Neither permits a paused game
    // to change the desktop. No topology lookup is needed in either case.
    for exists in [true, false] {
        assert_eq!(
            heartbeat(
                Ok(exists),
                false,
                || panic!("paused renewal must not query the desktop"),
                || panic!("paused renewal must not look the display up"),
                || panic!("paused renewal must not query connected displays"),
            )
            .unwrap(),
            Heartbeat::Keep
        );
        assert_eq!(
            heartbeat(Ok(exists), true, || Ok(false), || Ok(false), || Ok(true)).unwrap(),
            if exists {
                Heartbeat::SwitchOn
            } else {
                Heartbeat::Recreate
            },
            "an active stream or resume still repairs the display"
        );
    }
    let error = heartbeat(
        Err(windows::core::Error::from_hresult(ERROR_ACCESS_DENIED.to_hresult()).into()),
        false,
        || panic!("no topology query"),
        || panic!("no topology query"),
        || panic!("no topology query"),
    )
    .unwrap_err();
    assert_eq!(
        error.downcast_ref::<windows::core::Error>().unwrap().code(),
        ERROR_ACCESS_DENIED.to_hresult(),
        "paused renewal must not hide transport failures"
    );
}
