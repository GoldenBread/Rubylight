//! JNI bridge exports for `NightjarDiscoveryJni` mDNS discovery deduplication, generation
//! tracking, and scope-preserving IPv6 ranking (`nightjar_core::mdns`).

use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::{jint, jintArray, jlong, jstring};
use nightjar_core::mdns::{
    CandidateKey, MdnsDiscoveryTracker, ProbeDecision, link_local_ipv6_strings,
    select_ipv6_strings,
};

use crate::http::new_jstring;
use crate::slab::Slab;

static TRACKERS: Slab<MdnsDiscoveryTracker> = Slab::new();

fn jstr(env: &mut JNIEnv, value: &JString) -> Option<String> {
    if value.is_null() {
        return None;
    }
    env.get_string(value).ok().map(Into::into)
}

fn parse_string_array(json_str: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(json_str).unwrap_or_default()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_mdnsCreate(
    _: JNIEnv,
    _: JClass,
) -> jlong {
    TRACKERS.insert(MdnsDiscoveryTracker::new())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_mdnsDestroy(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    TRACKERS.remove(handle);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_mdnsReset(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jlong {
    TRACKERS
        .get(handle)
        .map_or(0, |tracker| tracker.reset() as jlong)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_mdnsGeneration(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jlong {
    TRACKERS
        .get(handle)
        .map_or(0, |tracker| tracker.generation() as jlong)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_mdnsSelectIpv6(
    mut env: JNIEnv,
    _: JClass,
    addrs_json: JString,
) -> jintArray {
    let list = jstr(&mut env, &addrs_json)
        .map(|s| parse_string_array(&s))
        .unwrap_or_default();
    let refs: Vec<&str> = list.iter().map(String::as_str).collect();
    let (best, local) = select_ipv6_strings(&refs);
    let out = [
        best.map_or(-1, |i| i as jint),
        local.map_or(-1, |i| i as jint),
    ];
    let Ok(arr) = env.new_int_array(2) else {
        return std::ptr::null_mut();
    };
    if env.set_int_array_region(&arr, 0, &out).is_err() {
        return std::ptr::null_mut();
    }
    arr.into_raw()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_mdnsLinkLocalIpv6(
    mut env: JNIEnv,
    _: JClass,
    addrs_json: JString,
) -> jint {
    let list = jstr(&mut env, &addrs_json)
        .map(|s| parse_string_array(&s))
        .unwrap_or_default();
    let refs: Vec<&str> = list.iter().map(String::as_str).collect();
    link_local_ipv6_strings(&refs).map_or(-1, |i| i as jint)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_mdnsBeginCandidate(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    name: JString,
    port: jint,
    local_addr: JString,
    v6_addr: JString,
    generation: jlong,
) -> jint {
    let Some(tracker) = TRACKERS.get(handle) else {
        return 0;
    };
    let Some(name) = jstr(&mut env, &name) else {
        return 0;
    };
    let candidate = CandidateKey {
        name,
        port: port.clamp(0, u16::MAX as i32) as u16,
        local: jstr(&mut env, &local_addr),
        v6: jstr(&mut env, &v6_addr),
    };
    match tracker.begin_candidate(candidate, generation.max(0) as u64) {
        ProbeDecision::StaleGeneration => 0,
        ProbeDecision::Duplicate => 1,
        ProbeDecision::Probe => 2,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_mdnsMarkAnswered(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    name: JString,
    port: jint,
    local_addr: JString,
    v6_addr: JString,
) {
    let Some(tracker) = TRACKERS.get(handle) else {
        return;
    };
    let Some(name) = jstr(&mut env, &name) else {
        return;
    };
    let candidate = CandidateKey {
        name,
        port: port.clamp(0, u16::MAX as i32) as u16,
        local: jstr(&mut env, &local_addr),
        v6: jstr(&mut env, &v6_addr),
    };
    tracker.mark_answered(candidate);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_mdnsForgetAnswered(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    if let Some(tracker) = TRACKERS.get(handle) {
        tracker.forget_answered();
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_mdnsComputers(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jstring {
    let computers = TRACKERS
        .get(handle)
        .map_or_else(Vec::new, |tracker| tracker.computers());
    let json = serde_json::to_string(&computers).unwrap_or_else(|_| "[]".into());
    new_jstring(&env, &json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nightjar_core::mdns::{CandidateKey, ProbeDecision};

    #[test]
    fn tracker_slab_and_ipv6_selection_work() {
        let h = TRACKERS.insert(MdnsDiscoveryTracker::new());
        assert!(h > 0);
        let tracker = TRACKERS.get(h).unwrap();
        let gen1 = tracker.generation();
        let key = CandidateKey {
            name: "PC".into(),
            port: 47989,
            local: Some("192.168.1.5".into()),
            v6: None,
        };
        assert_eq!(
            tracker.begin_candidate(key.clone(), gen1),
            ProbeDecision::Probe
        );
        assert_eq!(
            tracker.begin_candidate(key, gen1.saturating_add(1)),
            ProbeDecision::StaleGeneration
        );
        TRACKERS.remove(h);
        assert!(TRACKERS.get(h).is_none());

        let picked = select_ipv6_strings(&["fe80::1%wlan0", "2001:db8::2"]);
        assert_eq!(picked, (Some(1), Some(0)));
    }
}
