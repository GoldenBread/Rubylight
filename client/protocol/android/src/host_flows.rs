//! JNI bridges for pure host flow policies (`NightjarPolicyJni`) and RNG-backed pairing
//! helpers (`NightjarHostsJni`) shared by `SleepFlow`, `PcPowerFlow`, `HostUpdateFlow`,
//! `PairingWatch`, `Invites`, `HostActions`, and `ComputerManagerService`.

use std::ptr;

use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::{JNI_FALSE, JNI_TRUE, jboolean, jint, jlong, jstring};
use nightjar_policy::hosts::{
    APPLIST_POLL_PERIOD_MS, APPLIST_RETRY_PERIOD_MS, EMPTY_APPLIST_THRESHOLD, HostSlots,
    INITIAL_POLL_TRIES, INTERNET_PLAY_READ_INTERVAL_MS, InviteErrorKind, OFFLINE_POLL_TRIES,
    PAIRING_WATCH_INTERVAL_MS, PC_POWER_REFUSAL_POLL_MS, PC_POWER_REFUSAL_READS,
    POLL_DATA_TTL_MS, PairFailureKind, PairFailureState, PairingWatchCommand, PairingWatchFinish,
    PairingWatchTracker, SLEEP_TOKEN_REFRESH_MS, SleepUnavailableReason, TokenStamp,
    UPDATE_MAX_WAIT_MS, UPDATE_POLL_MS, UPDATE_WAIT_MS, UnpairFailureKind, UpdateStepAction,
    UpdateTracker, accept_applist_response, already_paired, classify_invite_error,
    classify_pairing_failure, classify_pairing_http_refusal, classify_sleep_unavailable,
    classify_unpair, is_internet_play_read_due, is_tunnel_address, next_applist_interval_ms,
    pairing_refused_status, port_matches_active_address, probe_plan, punches_now,
    should_clear_cert_on_unpair, strip_bidi_controls, update_target,
};
use nightjar_policy::http::HostUpdate;

use crate::http::new_jstring;
use crate::slab::MutexSlab;

use super::jstr;

static UPDATE_TRACKERS: MutexSlab<UpdateTracker> = MutexSlab::new();
static PAIRING_WATCHES: MutexSlab<PairingWatchTracker> = MutexSlab::new();

fn jb(val: bool) -> jboolean {
    if val { JNI_TRUE } else { JNI_FALSE }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hostPollDataTtlMs(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    POLL_DATA_TTL_MS as jlong
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hostOfflinePollTries(
    _: JNIEnv,
    _: JClass,
) -> jint {
    OFFLINE_POLL_TRIES as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hostInitialPollTries(
    _: JNIEnv,
    _: JClass,
) -> jint {
    INITIAL_POLL_TRIES as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hostApplistPollPeriodMs(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    APPLIST_POLL_PERIOD_MS as jlong
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hostApplistRetryPeriodMs(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    APPLIST_RETRY_PERIOD_MS as jlong
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hostEmptyApplistThreshold(
    _: JNIEnv,
    _: JClass,
) -> jint {
    EMPTY_APPLIST_THRESHOLD as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hostInternetPlayReadIntervalMs(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    INTERNET_PLAY_READ_INTERVAL_MS as jlong
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
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_hostPairingWatchIntervalMs(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    PAIRING_WATCH_INTERVAL_MS as jlong
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_sleepTokenIsStale(
    _: JNIEnv,
    _: JClass,
    mono_start_ms: jlong,
    mono_now_ms: jlong,
    wall_start_ms: jlong,
    wall_now_ms: jlong,
) -> jboolean {
    let stamp = TokenStamp {
        mono_ms: mono_start_ms.max(0) as u64,
        wall_ms: wall_start_ms.max(0) as u64,
    };
    jb(stamp.is_stale(mono_now_ms.max(0) as u64, wall_now_ms.max(0) as u64))
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
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_updateTarget(
    mut env: JNIEnv,
    _: JClass,
    current_version: JString,
    latest_version: JString,
    install_supported: jboolean,
) -> jstring {
    let update = HostUpdate {
        current_version: jstr(&mut env, &current_version).unwrap_or_default(),
        latest_version: jstr(&mut env, &latest_version),
        install_supported: install_supported != 0,
        ..Default::default()
    };
    match update_target(&update) {
        Ok(target) => new_jstring(&env, &target),
        Err(_) => ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_updateTrackerCreate(
    mut env: JNIEnv,
    _: JClass,
    target: JString,
    polls_limit: jint,
    max_polls: jint,
) -> jlong {
    let target = jstr(&mut env, &target).unwrap_or_default();
    UPDATE_TRACKERS.insert(UpdateTracker::with_limits(
        target,
        polls_limit.max(0) as u32,
        max_polls.max(0) as u32,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_updateTrackerStep(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    running_version: JString,
    update_json: JString,
) -> jstring {
    let running = jstr(&mut env, &running_version);
    let update = jstr(&mut env, &update_json)
        .and_then(|json| serde_json::from_str::<HostUpdate>(&json).ok());
    let action = UPDATE_TRACKERS
        .with(handle, |tracker| tracker.step(running.as_deref(), update.as_ref()))
        .unwrap_or(UpdateStepAction::TimedOut {
            target: String::new(),
        });
    let json = serde_json::to_string(&action).unwrap_or_else(|_| r#"{"action":"continue"}"#.into());
    new_jstring(&env, &json)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_updateTrackerDestroy(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    UPDATE_TRACKERS.remove(handle);
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

fn pair_failure_code(kind: PairFailureKind) -> jint {
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
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_classifyUnpair(
    _: JNIEnv,
    _: JClass,
    status_or_neg1: jint,
    is_not_paired: jboolean,
    is_unknown_host: jboolean,
    is_not_found: jboolean,
) -> jint {
    let status = (status_or_neg1 >= 0 && status_or_neg1 <= u16::MAX as i32)
        .then_some(status_or_neg1 as u16);
    match classify_unpair(
        status,
        is_not_paired != 0,
        is_unknown_host != 0,
        is_not_found != 0,
    ) {
        UnpairFailureKind::NotPaired => 0,
        UnpairFailureKind::UnknownHost => 1,
        UnpairFailureKind::NotFound => 2,
        UnpairFailureKind::Failed => 3,
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
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_pollPunchesNow(
    _: JNIEnv,
    _: JClass,
    has_local: jboolean,
    offline: jboolean,
    active_is_tunnel: jboolean,
) -> jboolean {
    jb(punches_now(
        has_local != 0,
        offline != 0,
        active_is_tunnel != 0,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_pollShouldClearCertOnUnpair(
    _: JNIEnv,
    _: JClass,
    paired: jboolean,
    info_path_plain: jboolean,
    same_cert: jboolean,
) -> jboolean {
    jb(should_clear_cert_on_unpair(
        paired != 0,
        info_path_plain != 0,
        same_cert != 0,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_pollPortMatchesActiveAddress(
    _: JNIEnv,
    _: JClass,
    online: jboolean,
    active_port: jint,
    candidate_port: jint,
    known_https_port: jint,
) -> jint {
    let active = (active_port > 0 && active_port <= u16::MAX as i32).then_some(active_port as u16);
    port_matches_active_address(
        online != 0,
        active,
        candidate_port.clamp(0, u16::MAX as i32) as u16,
        known_https_port.clamp(0, u16::MAX as i32) as u16,
    ) as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_pollAcceptApplistResponse(
    _: JNIEnv,
    _: JClass,
    non_empty_xml: jboolean,
    apps_empty: jboolean,
    empty_count: jint,
) -> jint {
    let mut count = empty_count.max(0) as u32;
    let accepted = accept_applist_response(non_empty_xml != 0, apps_empty != 0, &mut count);
    ((if accepted { 1i32 } else { 0i32 }) << 16) | ((count as i32) & 0xffff)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_pollNextApplistIntervalMs(
    _: JNIEnv,
    _: JClass,
    received_app_list: jboolean,
) -> jlong {
    next_applist_interval_ms(received_app_list != 0) as jlong
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_pollInternetPlayReadDue(
    _: JNIEnv,
    _: JClass,
    last_read_ms_or_neg1: jlong,
    now_ms: jlong,
) -> jboolean {
    let last = (last_read_ms_or_neg1 >= 0).then_some(last_read_ms_or_neg1 as u64);
    jb(is_internet_play_read_due(last, now_ms.max(0) as u64))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_pollBuildProbePlan(
    mut env: JNIEnv,
    _: JClass,
    slots_json: JString,
    online: jboolean,
    active_port: jint,
    known_https_port: jint,
) -> jstring {
    let slots = jstr(&mut env, &slots_json)
        .and_then(|s| serde_json::from_str::<HostSlots>(&s).ok())
        .unwrap_or_default();
    let active = (active_port > 0 && active_port <= u16::MAX as i32).then_some(active_port as u16);
    let plan = probe_plan(
        &slots,
        online != 0,
        active,
        known_https_port.clamp(0, u16::MAX as i32) as u16,
    );
    let json = serde_json::to_string(&plan).unwrap_or_else(|_| "[]".into());
    new_jstring(&env, &json)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_randomPairingPin(
    env: JNIEnv,
    _: JClass,
) -> jstring {
    new_jstring(&env, &nightjar_core::hosts::pairing::random_pin())
}
