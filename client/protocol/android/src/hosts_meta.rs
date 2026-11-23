//! JNI bridge exports for `NightjarHostsJni` per-host metadata (`HostMetaStore`: notices,
//! packed 4:4:4 modes, capability cache, aliases, pinned addresses, and one-time legacy imports).

use std::collections::BTreeMap;

use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::{JNI_FALSE, JNI_TRUE, jboolean, jint, jlong, jstring};
use nightjar_core::hosts::{EndpointSlot, HostMetaStore, HostNames, HostPackedModesState};
use serde_json::Value;

use crate::http::new_jstring;
use crate::slab::Slab;

static META_STORES: Slab<HostMetaStore> = Slab::new();

fn jstr(env: &mut JNIEnv, value: &JString) -> Option<String> {
    if value.is_null() {
        return None;
    }
    env.get_string(value).ok().map(Into::into)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_metaStoreOpen(
    mut env: JNIEnv,
    _: JClass,
    dir_path: JString,
) -> jlong {
    let Some(dir) = jstr(&mut env, &dir_path) else {
        return 0;
    };
    META_STORES.insert(HostMetaStore::open(dir))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_metaStoreDestroy(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    META_STORES.remove(handle);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_metaNeedsImport(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    source: JString,
) -> jboolean {
    let Some(store) = META_STORES.get(handle) else {
        return JNI_FALSE;
    };
    let Some(source) = jstr(&mut env, &source) else {
        return JNI_FALSE;
    };
    if store.needs_import(&source) {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_metaImportLegacy(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    source: JString,
    entries_json: JString,
) -> jboolean {
    let Some(store) = META_STORES.get(handle) else {
        return JNI_FALSE;
    };
    let Some(source) = jstr(&mut env, &source) else {
        return JNI_FALSE;
    };
    let raw = jstr(&mut env, &entries_json).unwrap_or_else(|| "{}".into());
    let Ok(entries) = serde_json::from_str::<BTreeMap<String, Value>>(&raw) else {
        return JNI_FALSE;
    };
    if store
        .import_legacy_source(&source, &entries)
        .unwrap_or(false)
    {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_metaClaimNotice(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
    notice_name: JString,
) -> jboolean {
    let Some(store) = META_STORES.get(handle) else {
        return JNI_TRUE;
    };
    let Some(uuid) = jstr(&mut env, &uuid) else {
        return JNI_TRUE;
    };
    let Some(notice) = jstr(&mut env, &notice_name) else {
        return JNI_TRUE;
    };
    if store.claim_notice(&uuid, &notice).unwrap_or(true) {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_metaResetNotice(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
    notice_name: JString,
) {
    if let (Some(store), Some(uuid), Some(notice)) = (
        META_STORES.get(handle),
        jstr(&mut env, &uuid),
        jstr(&mut env, &notice_name),
    ) {
        let _ = store.reset_notice(&uuid, &notice);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_metaResetNoticeEverywhere(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    notice_name: JString,
) {
    if let (Some(store), Some(notice)) = (META_STORES.get(handle), jstr(&mut env, &notice_name)) {
        let _ = store.reset_notice_everywhere(&notice);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_metaForgetNotices(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
) {
    if let (Some(store), Some(uuid)) = (META_STORES.get(handle), jstr(&mut env, &uuid)) {
        let _ = store.forget_notices(&uuid);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_metaPackedCached(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
) -> jint {
    let Some(store) = META_STORES.get(handle) else {
        return -1;
    };
    let Some(uuid) = jstr(&mut env, &uuid) else {
        return -1;
    };
    store.packed_cached(&uuid)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_metaPackedStore(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
    describe_modes: jint,
) -> jboolean {
    let Some(store) = META_STORES.get(handle) else {
        return JNI_FALSE;
    };
    let Some(uuid) = jstr(&mut env, &uuid) else {
        return JNI_FALSE;
    };
    if store.packed_store(&uuid, describe_modes).unwrap_or(false) {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_metaPackedMarkRefused(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
) -> jboolean {
    let Some(store) = META_STORES.get(handle) else {
        return JNI_FALSE;
    };
    let Some(uuid) = jstr(&mut env, &uuid) else {
        return JNI_FALSE;
    };
    if store.packed_mark_refused(&uuid).unwrap_or(false) {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_metaPackedRefused(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
) -> jboolean {
    let Some(store) = META_STORES.get(handle) else {
        return JNI_FALSE;
    };
    let Some(uuid) = jstr(&mut env, &uuid) else {
        return JNI_FALSE;
    };
    if store.packed_refused(&uuid) {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_metaPackedStamp(
    mut env: JNIEnv,
    _: JClass,
    app_version: JString,
    host_version: JString,
) -> jstring {
    let app = jstr(&mut env, &app_version).unwrap_or_default();
    let host = jstr(&mut env, &host_version);
    new_jstring(&env, &HostPackedModesState::stamp(&app, host.as_deref()))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_metaPackedRenew(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
    stamp: JString,
) {
    if let (Some(store), Some(uuid), Some(stamp)) = (
        META_STORES.get(handle),
        jstr(&mut env, &uuid),
        jstr(&mut env, &stamp),
    ) {
        let _ = store.packed_renew(&uuid, &stamp);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_metaPackedClearRefusals(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    if let Some(store) = META_STORES.get(handle) {
        let _ = store.packed_clear_refusals();
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_metaPackedKnown(
    _: JNIEnv,
    _: JClass,
    server_info_modes: jint,
    cached_modes: jint,
) -> jint {
    HostPackedModesState::known(server_info_modes, cached_modes)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_metaCapLoad(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
) -> jstring {
    let Some(store) = META_STORES.get(handle) else {
        return std::ptr::null_mut();
    };
    let Some(uuid) = jstr(&mut env, &uuid) else {
        return std::ptr::null_mut();
    };
    let Some(json) = store.load_capabilities(&uuid) else {
        return std::ptr::null_mut();
    };
    new_jstring(&env, &json)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_metaCapSave(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
    caps_json: JString,
) -> jboolean {
    let Some(store) = META_STORES.get(handle) else {
        return JNI_FALSE;
    };
    let Some(uuid) = jstr(&mut env, &uuid) else {
        return JNI_FALSE;
    };
    let Some(caps_json) = jstr(&mut env, &caps_json) else {
        return JNI_FALSE;
    };
    if store.save_capabilities(&uuid, &caps_json).unwrap_or(false) {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_metaCapForget(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
) {
    if let (Some(store), Some(uuid)) = (META_STORES.get(handle), jstr(&mut env, &uuid)) {
        let _ = store.forget_capabilities(&uuid);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_metaAliasGet(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
) -> jstring {
    let Some(store) = META_STORES.get(handle) else {
        return std::ptr::null_mut();
    };
    let Some(uuid) = jstr(&mut env, &uuid) else {
        return std::ptr::null_mut();
    };
    let Some(alias) = store.get_alias(&uuid) else {
        return std::ptr::null_mut();
    };
    new_jstring(&env, &alias)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_metaAliasNormalize(
    mut env: JNIEnv,
    _: JClass,
    typed: JString,
    own_name: JString,
) -> jstring {
    let typed = jstr(&mut env, &typed);
    let own = jstr(&mut env, &own_name);
    let Some(norm) = HostNames::normalize_alias(typed.as_deref(), own.as_deref()) else {
        return std::ptr::null_mut();
    };
    new_jstring(&env, &norm)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_metaAliasSet(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
    alias: JString,
) {
    if let (Some(store), Some(uuid)) = (META_STORES.get(handle), jstr(&mut env, &uuid)) {
        let alias = jstr(&mut env, &alias);
        let _ = store.set_alias(&uuid, alias.as_deref());
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_metaPinnedGet(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
) -> jstring {
    let Some(store) = META_STORES.get(handle) else {
        return std::ptr::null_mut();
    };
    let Some(uuid) = jstr(&mut env, &uuid) else {
        return std::ptr::null_mut();
    };
    let Some(slot) = store.get_pinned(&uuid) else {
        return std::ptr::null_mut();
    };
    let json = serde_json::to_string(&slot).unwrap_or_default();
    new_jstring(&env, &json)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_metaPinnedSet(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
    address: JString,
    port: jint,
) {
    let Some(store) = META_STORES.get(handle) else {
        return;
    };
    let Some(uuid) = jstr(&mut env, &uuid) else {
        return;
    };
    let slot = jstr(&mut env, &address)
        .and_then(|addr| EndpointSlot::new(&addr, port.clamp(0, u16::MAX as i32) as u16));
    let _ = store.set_pinned(&uuid, slot);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_metaForget(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
) {
    if let (Some(store), Some(uuid)) = (META_STORES.get(handle), jstr(&mut env, &uuid)) {
        let _ = store.forget(&uuid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_slab_lifecycle_and_alias_normalization() {
        let dir = std::env::temp_dir().join(format!(
            "nightjar-android-meta-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let h = META_STORES.insert(HostMetaStore::open(&dir));
        assert!(h > 0);
        let store = META_STORES.get(h).unwrap();
        assert!(store.set_alias("pc", Some("  Living Room \n")).unwrap());
        assert_eq!(store.get_alias("pc").as_deref(), Some("Living Room"));
        META_STORES.remove(h);
        assert!(META_STORES.get(h).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
