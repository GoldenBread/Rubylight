//! AMF HDR metadata retry, IDR coordination and effective metadata unit tests.

use super::super::*;

fn hdr_request(maximum_nits: u16) -> metadata::EncodedMetadata {
    metadata::EncodedMetadata::new(
        1,
        butterpollo_media_policy::hdr::Metadata {
            maximum_nits,
            ..Default::default()
        },
    )
}

#[test]
fn hdr_same_value_retries_after_failure_and_stops_after_success() {
    use metadata::{Retry, State, Update, WriteError, WriteStage};
    let mut state = State::default();
    let now = Instant::now();
    let request = hdr_request(1000);
    let error = WriteError {
        stage: WriteStage::Buffer,
        code: AMF_RESULT_AMF_OUT_OF_MEMORY,
    };
    let mut writes = Vec::new();
    assert_eq!(
        state.update(request, now, |value| {
            writes.push(*value);
            Err(error)
        }),
        Update::Failed {
            error,
            retry: Retry::After(Duration::from_secs(1)),
            report: true
        }
    );
    assert!(!state.needs_idr());
    assert_eq!(
        state.update(request, now + Duration::from_millis(999), |_| {
            panic!("backoff must avoid calling AMF")
        }),
        Update::Deferred
    );
    assert_eq!(
        state.update(request, now + Duration::from_secs(1), |value| {
            writes.push(*value);
            Ok(())
        }),
        Update::Applied
    );
    assert_eq!(writes, [request, request]);
    assert!(state.needs_idr());
    state.submitted(AMF_RESULT_AMF_OK);
    assert!(!state.needs_idr());
    assert_eq!(
        state.update(request, now + Duration::from_secs(2), |_| {
            panic!("successful metadata must not be rewritten every second")
        }),
        Update::Unchanged
    );
    assert!(!state.needs_idr());
}

#[test]
fn hdr_transient_retries_are_capped_and_logs_rate_limited() {
    use metadata::{Retry, State, Update, WriteError, WriteStage};
    let mut state = State::default();
    let start = Instant::now();
    let mut elapsed = 0;
    let mut reports = Vec::new();
    let error = WriteError {
        stage: WriteStage::Property,
        code: AMF_RESULT_AMF_FAIL,
    };
    for delay in [1, 2, 4, 8, 16, 30, 30, 30] {
        let now = start + Duration::from_secs(elapsed);
        let Update::Failed { retry, report, .. } =
            state.update(hdr_request(1000), now, |_| Err(error))
        else {
            panic!("the failed request must remain eligible for retry");
        };
        assert_eq!(retry, Retry::After(Duration::from_secs(delay)));
        if report {
            reports.push(elapsed);
        }
        assert_eq!(
            state.update(hdr_request(1000), now + Duration::from_millis(1), |_| {
                panic!("no spin retry")
            }),
            Update::Deferred
        );
        elapsed += delay;
    }
    assert_eq!(reports, [0, 31, 61, 91]);
}

#[test]
fn hdr_retries_latest_request_and_keeps_last_successful_value() {
    use metadata::{State, Update, WriteError, WriteStage};
    let mut state = State::default();
    let now = Instant::now();
    let original = hdr_request(1000);
    assert_eq!(state.update(original, now, |_| Ok(())), Update::Applied);
    state.submitted(AMF_RESULT_AMF_OK);
    let error = WriteError {
        stage: WriteStage::Property,
        code: AMF_RESULT_AMF_FAIL,
    };
    assert!(matches!(
        state.update(hdr_request(600), now, |_| Err(error)),
        Update::Failed { .. }
    ));
    assert!(!state.needs_idr());
    assert_eq!(
        state.update(original, now, |_| {
            panic!("a failed write must not replace the last applied value")
        }),
        Update::Unchanged
    );
    assert!(matches!(
        state.update(hdr_request(600), now, |_| Err(error)),
        Update::Failed { .. }
    ));
    let latest = hdr_request(1400);
    assert_eq!(
        state.update(latest, now + Duration::from_millis(500), |_| {
            panic!("changing requests must not bypass the driver's backoff")
        }),
        Update::Deferred
    );
    assert_eq!(
        state.update(latest, now + Duration::from_secs(1), |value| {
            assert_eq!(*value, latest);
            Ok(())
        }),
        Update::Applied
    );
    assert!(state.needs_idr());
}

#[test]
fn hdr_unsupported_property_is_not_retried_but_bad_values_can_change() {
    use metadata::{Retry, State, Update, WriteError, WriteStage};
    let now = Instant::now();
    for code in [
        AMF_RESULT_AMF_NOT_SUPPORTED,
        AMF_RESULT_AMF_NOT_IMPLEMENTED,
        AMF_RESULT_AMF_NOT_FOUND,
    ] {
        let mut state = State::default();
        let error = WriteError {
            stage: WriteStage::Property,
            code,
        };
        assert_eq!(
            state.update(hdr_request(1000), now, |_| Err(error)),
            Update::Failed {
                error,
                retry: Retry::Unsupported,
                report: true
            }
        );
        for value in [1000, 600] {
            assert_eq!(
                state.update(hdr_request(value), now + Duration::from_secs(60), |_| {
                    panic!("an unsupported property requires a new encoder/driver")
                }),
                Update::Deferred
            );
        }
    }
    for code in [
        AMF_RESULT_AMF_INVALID_ARG,
        AMF_RESULT_AMF_OUT_OF_RANGE,
        AMF_RESULT_AMF_INVALID_FORMAT,
        AMF_RESULT_AMF_INVALID_DATA_TYPE,
    ] {
        let mut state = State::default();
        let error = WriteError {
            stage: WriteStage::Property,
            code,
        };
        assert_eq!(
            state.update(hdr_request(1000), now, |_| Err(error)),
            Update::Failed {
                error,
                retry: Retry::ChangedValue,
                report: true
            }
        );
        assert_eq!(
            state.update(hdr_request(1000), now + Duration::from_secs(60), |_| {
                panic!("retrying an invalid value cannot repair it")
            }),
            Update::Deferred
        );
        assert_eq!(
            state.update(hdr_request(600), now, |_| Ok(())),
            Update::Applied
        );
    }
    // A buffer failure does not establish that the metadata property is
    // unsupported, even if it happens to return the same numeric error.
    let mut state = State::default();
    let error = WriteError {
        stage: WriteStage::Buffer,
        code: AMF_RESULT_AMF_NOT_SUPPORTED,
    };
    assert_eq!(
        state.update(hdr_request(1000), now, |_| Err(error)),
        Update::Failed {
            error,
            retry: Retry::After(Duration::from_secs(1)),
            report: true
        }
    );
    assert_eq!(
        state.update(hdr_request(1000), now + Duration::from_secs(1), |_| Ok(())),
        Update::Applied
    );
}

#[test]
fn hdr_changed_metadata_requests_one_idr_until_submission_succeeds() {
    use metadata::{State, Update};
    let mut state = State::default();
    let now = Instant::now();
    assert_eq!(
        state.update(hdr_request(1000), now, |_| Ok(())),
        Update::Applied
    );
    for result in [
        AMF_RESULT_AMF_INPUT_FULL,
        AMF_RESULT_AMF_FAIL,
        AMF_RESULT_AMF_INPUT_FULL,
    ] {
        assert!(state.needs_idr());
        state.submitted(result);
        assert!(
            state.needs_idr(),
            "failed submission must keep the header/keyframe request"
        );
    }
    // Coalesce another successful update before the first accepted frame.
    assert_eq!(
        state.update(hdr_request(600), now, |_| Ok(())),
        Update::Applied
    );
    assert!(state.needs_idr());
    state.submitted(AMF_RESULT_AMF_OK);
    assert!(!state.needs_idr());
    assert_eq!(
        state.update(hdr_request(600), now, |_| panic!("duplicate")),
        Update::Unchanged
    );
    assert!(!state.needs_idr());
    assert_eq!(
        state.update(hdr_request(1400), now, |_| Ok(())),
        Update::Applied
    );
    assert!(state.needs_idr());
}

#[test]
fn keyframe_acknowledges_all_reasons_only_on_successful_idr_submission() {
    use metadata::State;
    for explicit in [false, true] {
        for changed_metadata in [false, true] {
            for recovery in [false, true] {
                let mut state = State::default();
                let mut recover = recovery;
                if changed_metadata {
                    state.update(hdr_request(1000), Instant::now(), |_| Ok(()));
                }
                let idr = explicit || state.needs_idr() || recover;
                for result in [AMF_RESULT_AMF_INPUT_FULL, AMF_RESULT_AMF_FAIL] {
                    acknowledge_keyframe(idr, result, &mut recover, &mut state);
                    assert_eq!(recover, recovery);
                    assert_eq!(state.needs_idr(), changed_metadata);
                }
                acknowledge_keyframe(idr, AMF_RESULT_AMF_OK, &mut recover, &mut state);
                assert!(!recover);
                assert!(!state.needs_idr());
            }
        }
    }
    // An old frame can time out in poll() after this non-IDR was prepared.
    // Its successful submission does not satisfy that newly requested IDR.
    let mut recover = true;
    let mut state = State::default();
    acknowledge_keyframe(false, AMF_RESULT_AMF_OK, &mut recover, &mut state);
    assert!(recover);
    acknowledge_keyframe(true, AMF_RESULT_AMF_OK, &mut recover, &mut state);
    assert!(!recover);
}

#[test]
fn hdr_effective_metadata_ignores_control_only_values_and_preserves_codec_units() {
    use metadata::{EncodedMetadata, State, Update};
    let metadata = butterpollo_media_policy::hdr::Metadata::display(1000., 0.005, 400.);
    for codec in [1, 2] {
        let encoded = EncodedMetadata::new(codec, metadata);
        let value = encoded.value();
        assert_eq!(
            value.redPrimary,
            if codec == 1 {
                [35400, 14600]
            } else {
                [46399, 19136]
            }
        );
        assert_eq!(
            value.maxMasteringLuminance,
            if codec == 1 { 10_000_000 } else { 256_000 }
        );
        assert_eq!(
            value.minMasteringLuminance,
            if codec == 1 { 50 } else { 81 }
        );
        let mut state = State::default();
        let now = Instant::now();
        assert_eq!(state.update(encoded, now, |_| Ok(())), Update::Applied);
        state.submitted(AMF_RESULT_AMF_OK);
        assert_eq!(
            state.update(
                EncodedMetadata::new(
                    codec,
                    butterpollo_media_policy::hdr::Metadata {
                        full_frame_nits: 600,
                        ..metadata
                    }
                ),
                now,
                |_| panic!("full_frame_nits is only on the control channel")
            ),
            Update::Unchanged
        );
        assert!(!state.needs_idr());
    }
    let extreme = EncodedMetadata::new(
        2,
        butterpollo_media_policy::hdr::Metadata {
            primaries: [[u16::MAX; 2]; 3],
            white: [u16::MAX; 2],
            maximum_nits: u16::MAX,
            minimum: u16::MAX,
            ..metadata
        },
    )
    .value();
    assert_eq!(extreme.redPrimary, [u16::MAX; 2]);
    assert_eq!(extreme.whitePoint, [u16::MAX; 2]);
    assert_eq!(extreme.maxMasteringLuminance, 16_776_960);
    assert_eq!(extreme.minMasteringLuminance, 107_372);
}
