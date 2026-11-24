//! JNI bridges for pure host flow policies (`NightjarPolicyJni`) and RNG-backed pairing
//! helpers (`NightjarHostsJni`) shared by `SleepFlow`, `PcPowerFlow`, `HostUpdateFlow`,
//! `PairingWatch`, `Invites`, `HostActions`, and `ComputerManagerService`.

use std::ptr;

use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::{JNI_FALSE, JNI_TRUE, jboolean, jint, jlong, jstring};
use nightjar_policy::hosts::{
    InviteErrorKind, PC_POWER_REFUSAL_POLL_MS, PC_POWER_REFUSAL_READS, PairFailureKind,
    PairFailureState, PairingWatchCommand, PairingWatchFinish, PairingWatchTracker,
    SLEEP_TOKEN_REFRESH_MS, SleepUnavailableReason, UPDATE_MAX_WAIT_MS, UPDATE_POLL_MS,
    UPDATE_WAIT_MS, already_paired, classify_invite_error, classify_pairing_failure,
    classify_pairing_http_refusal, classify_sleep_unavailable, is_tunnel_address,
    pairing_refused_status, strip_bidi_controls,
};

use crate::http::new_jstring;
use crate::slab::MutexSlab;

use super::jstr;

static PAIRING_WATCHES: MutexSlab<PairingWatchTracker> = MutexSlab::new();

fn jb(val: bool) -> jboolean {
    if val { JNI_TRUE } else { JNI_FALSE }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hostSleepTokenRefreshMs(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    SLEEP_TOKEN_REFRESH_MS as jlong
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hostPcPowerRefusalPollMs(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    PC_POWER_REFUSAL_POLL_MS as jlong
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hostPcPowerRefusalReads(
    _: JNIEnv,
    _: JClass,
) -> jint {
    PC_POWER_REFUSAL_READS as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hostUpdateWaitMs(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    UPDATE_WAIT_MS as jlong
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hostUpdateMaxWaitMs(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    UPDATE_MAX_WAIT_MS as jlong
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hostUpdatePollMs(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    UPDATE_POLL_MS as jlong
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_sleepUnavailableReason(
    _: JNIEnv,
    _: JClass,
    sleep_supported: jboolean,
    sleep_allowed: jboolean,
) -> jint {
    match classify_sleep_unavailable(sleep_supported != 0, sleep_allowed != 0) {
        SleepUnavailableReason::Unsupported => 0,
        SleepUnavailableReason::NotAllowed => 1,
        SleepUnavailableReason::Busy => 2,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_pairingWatchCreate(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    PAIRING_WATCHES.insert(PairingWatchTracker::new())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_pairingWatchDestroy(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    PAIRING_WATCHES.remove(handle);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_pairingWatchRefreshSoon(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    let _ = PAIRING_WATCHES.with(handle, PairingWatchTracker::refresh_soon);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_pairingWatchPause(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    let _ = PAIRING_WATCHES.with(handle, PairingWatchTracker::pause);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_pairingWatchBeginPoll(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
    now_ms: jlong,
    has_asked_hosts: jboolean,
) -> jlong {
    match PAIRING_WATCHES.with(handle, |w| {
        w.begin_poll(now_ms.max(0) as u64, has_asked_hosts != 0)
    }) {
        Some(PairingWatchCommand::StartPoll { generation }) => generation as jlong,
        Some(PairingWatchCommand::PublishEmpty) => -1,
        _ => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_pairingWatchFinishPoll(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    generation: jlong,
    ids_json: JString,
) -> jint {
    let ids = jstr(&mut env, &ids_json)
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default();
    match PAIRING_WATCHES.with(handle, |w| w.finish_poll(generation.max(0) as u64, ids)) {
        Some(PairingWatchFinish::Changed) => 2,
        Some(PairingWatchFinish::Unchanged) => 1,
        _ => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_stripBidiControls(
    mut env: JNIEnv,
    _: JClass,
    input: JString,
) -> jstring {
    let Some(text) = jstr(&mut env, &input) else {
        return ptr::null_mut();
    };
    new_jstring(&env, &strip_bidi_controls(&text))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_pairingRefusedStatus(
    _: JNIEnv,
    _: JClass,
    status: jint,
) -> jboolean {
    jb((0..=u16::MAX as i32).contains(&status) && pairing_refused_status(status as u16))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_alreadyPaired(
    _: JNIEnv,
    _: JClass,
    is_paired: jboolean,
    has_link: jboolean,
    has_pinned_cert: jboolean,
) -> jboolean {
    jb(already_paired(
        is_paired != 0,
        has_link != 0,
        has_pinned_cert != 0,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_classifyPairingFailure(
    _: JNIEnv,
    _: JClass,
    state_ordinal: jint,
    from_link: jboolean,
    one_time: jboolean,
    in_game: jboolean,
) -> jint {
    let state = match state_ordinal {
        0 => PairFailureState::NotPaired,
        1 => PairFailureState::Paired,
        2 => PairFailureState::PinWrong,
        3 => PairFailureState::Failed,
        4 => PairFailureState::AlreadyInProgress,
        5 => PairFailureState::CertMismatch,
        6 => PairFailureState::ServerUnverified,
        _ => PairFailureState::Failed,
    };
    pair_failure_code(classify_pairing_failure(
        state,
        from_link != 0,
        one_time != 0,
        in_game != 0,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_classifyPairingHttpRefusal(
    _: JNIEnv,
    _: JClass,
    from_link: jboolean,
    one_time: jboolean,
) -> jint {
    pair_failure_code(classify_pairing_http_refusal(from_link != 0, one_time != 0))
}

pub(super) fn pair_failure_code(kind: PairFailureKind) -> jint {
    match kind {
        PairFailureKind::PinWrong => 0,
        PairFailureKind::OneTimePinWrong => 1,
        PairFailureKind::LinkPinWrong => 2,
        PairFailureKind::AlreadyInProgress => 3,
        PairFailureKind::ServerUnverified => 4,
        PairFailureKind::InGame => 5,
        PairFailureKind::Declined => 6,
        PairFailureKind::LinkCertMismatch => 7,
        PairFailureKind::Network => 8,
        PairFailureKind::Refused => 9,
        PairFailureKind::Failed => 10,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_classifyInviteError(
    mut env: JNIEnv,
    _: JClass,
    msg: JString,
) -> jint {
    let text = jstr(&mut env, &msg).unwrap_or_default();
    match classify_invite_error(&text) {
        InviteErrorKind::NotAnInvite => 0,
        InviteErrorKind::Expired => 1,
        InviteErrorKind::Refused => 2,
        InviteErrorKind::InternetOff => 3,
        InviteErrorKind::KnownPc => 4,
        InviteErrorKind::Other => 5,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_isTunnelAddress(
    mut env: JNIEnv,
    _: JClass,
    ip: JString,
) -> jboolean {
    let Some(text) = jstr(&mut env, &ip) else {
        return JNI_FALSE;
    };
    jb(is_tunnel_address(&text))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_randomPairingPin(
    env: JNIEnv,
    _: JClass,
) -> jstring {
    new_jstring(&env, &nightjar_core::hosts::pairing::random_pin())
}
