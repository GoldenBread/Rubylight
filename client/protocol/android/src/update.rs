//! JNI bridge for `com.limelight.update` (`NightjarPolicyJni` update policy/SemVer/release/notes
//! + `NightjarHttpJni` GitHub release lookup and SHA-256-verified APK download).
//!
//! Security boundary: production entry points accept ONLY hardcoded [`UpdateTarget`] enum codes
//! (`0` = Nightjar, `1` = Rubylight), never arbitrary URLs or test trust stores.

use std::path::Path;
use std::sync::Arc;

use jni::JNIEnv;
use jni::objects::{JByteArray, JClass, JObject, JString};
use jni::sys::{JNI_FALSE, jboolean, jint, jlong, jstring};
use nightjar_core::update::net::WebPkiTransport;
use nightjar_core::update::{UpdateError, UpdateTarget, download_android_apk, fetch_latest};
use nightjar_policy::http::TransferCancel;
use nightjar_policy::update::{ChecksumFile, Release, ReleaseNotes, SemVer, UpdatePolicy};
use serde_json::json;

use crate::http::{CANCELS, jbool, new_jstring, ok_json};
use crate::http_folders::call_folder_progress;

const USER_AGENT: &str = "Nightjar (Android)";

fn jstr(env: &mut JNIEnv, value: &JString) -> Option<String> {
    if value.is_null() {
        return None;
    }
    env.get_string(value).ok().map(Into::into)
}

fn parse_release_payload(s: &str) -> Option<Release> {
    serde_json::from_str::<Release>(s)
        .ok()
        .or_else(|| Release::parse(s).ok())
}

fn update_error_json(err: &UpdateError) -> String {
    let kind = match err {
        UpdateError::RateLimited => "rate_limited",
        UpdateError::NoApk => "no_apk",
        UpdateError::NoChecksums => "no_checksums",
        UpdateError::ChecksumEntryMissing => "checksum_entry_missing",
        UpdateError::ChecksumMismatch => "checksum_mismatch",
        UpdateError::SizeMismatch => "size_mismatch",
        UpdateError::Address(_) => "address",
        UpdateError::Network(_) => "network",
        UpdateError::Io(_) => "io",
        UpdateError::Malformed(_) => "malformed",
        UpdateError::Cancelled => "cancelled",
    };
    json!({
        "error": {
            "kind": kind,
            "message": err.to_string(),
        }
    })
    .to_string()
}

fn error_kind_json(kind: &str, message: &str) -> String {
    json!({
        "error": {
            "kind": kind,
            "message": message,
        }
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// NightjarPolicyJni — SemVer, ReleaseInfo, ChecksumFile, ReleaseNotes, Policy
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_semVerParse(
    mut env: JNIEnv,
    _: JClass,
    text: JString,
) -> jstring {
    let Some(text) = jstr(&mut env, &text) else {
        return std::ptr::null_mut();
    };
    let Some(parsed) = SemVer::parse(&text) else {
        return std::ptr::null_mut();
    };
    match serde_json::to_string(&parsed) {
        Ok(json) => new_jstring(&env, &json),
        Err(_) => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_semVerCompare(
    mut env: JNIEnv,
    _: JClass,
    a: JString,
    b: JString,
) -> jint {
    let (Some(a), Some(b)) = (jstr(&mut env, &a), jstr(&mut env, &b)) else {
        return 0;
    };
    let (Some(va), Some(vb)) = (SemVer::parse(&a), SemVer::parse(&b)) else {
        return 0;
    };
    match va.cmp(&vb) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_semVerIsNewer(
    mut env: JNIEnv,
    _: JClass,
    candidate: JString,
    current: JString,
) -> jboolean {
    let (Some(cand), Some(cur)) = (jstr(&mut env, &candidate), jstr(&mut env, &current)) else {
        return JNI_FALSE;
    };
    jbool(SemVer::is_newer(&cand, &cur))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_semVerNormalize(
    mut env: JNIEnv,
    _: JClass,
    text: JString,
) -> jstring {
    let text = jstr(&mut env, &text).unwrap_or_default();
    new_jstring(&env, &SemVer::normalize(&text))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_releaseInfoParse(
    mut env: JNIEnv,
    _: JClass,
    json: JString,
) -> jstring {
    let Some(raw) = jstr(&mut env, &json) else {
        return std::ptr::null_mut();
    };
    let Ok(release) = Release::parse(&raw) else {
        return std::ptr::null_mut();
    };
    match serde_json::to_string(&release) {
        Ok(out) => new_jstring(&env, &out),
        Err(_) => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_releaseInfoIsUpdateFor(
    mut env: JNIEnv,
    _: JClass,
    release_json: JString,
    current_version: JString,
) -> jboolean {
    let (Some(raw), Some(cur)) = (
        jstr(&mut env, &release_json),
        jstr(&mut env, &current_version),
    ) else {
        return JNI_FALSE;
    };
    let Some(release) = parse_release_payload(&raw) else {
        return JNI_FALSE;
    };
    jbool(release.is_update_for(&cur))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_releaseInfoApkAsset(
    mut env: JNIEnv,
    _: JClass,
    release_json: JString,
    root: jboolean,
) -> jstring {
    let Some(raw) = jstr(&mut env, &release_json) else {
        return std::ptr::null_mut();
    };
    let Some(release) = parse_release_payload(&raw) else {
        return std::ptr::null_mut();
    };
    let Some(asset) = release.apk_asset(root != JNI_FALSE) else {
        return std::ptr::null_mut();
    };
    match serde_json::to_string(&asset) {
        Ok(out) => new_jstring(&env, &out),
        Err(_) => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_releaseInfoChecksumAsset(
    mut env: JNIEnv,
    _: JClass,
    release_json: JString,
) -> jstring {
    let Some(raw) = jstr(&mut env, &release_json) else {
        return std::ptr::null_mut();
    };
    let Some(release) = parse_release_payload(&raw) else {
        return std::ptr::null_mut();
    };
    let Some(asset) = release.checksum_asset() else {
        return std::ptr::null_mut();
    };
    match serde_json::to_string(&asset) {
        Ok(out) => new_jstring(&env, &out),
        Err(_) => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_checksumFileFind(
    mut env: JNIEnv,
    _: JClass,
    sums: JString,
    file_name: JString,
) -> jstring {
    let (Some(sums), Some(name)) = (jstr(&mut env, &sums), jstr(&mut env, &file_name)) else {
        return std::ptr::null_mut();
    };
    match ChecksumFile::find(&sums, &name) {
        Some(hex) => new_jstring(&env, &hex),
        None => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_checksumFileHex(
    env: JNIEnv,
    _: JClass,
    bytes: JByteArray,
) -> jstring {
    if bytes.is_null() {
        return std::ptr::null_mut();
    }
    let Ok(raw) = env.convert_byte_array(&bytes) else {
        return std::ptr::null_mut();
    };
    new_jstring(&env, &ChecksumFile::hex(&raw))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_releaseNotesPlain(
    mut env: JNIEnv,
    _: JClass,
    markdown: JString,
    max_chars: jint,
) -> jstring {
    let Some(md) = jstr(&mut env, &markdown) else {
        return new_jstring(&env, "");
    };
    let limit = usize::try_from(max_chars).unwrap_or(0);
    new_jstring(&env, &ReleaseNotes::plain(&md, limit))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_updatePolicyIsCheckDue(
    _: JNIEnv,
    _: JClass,
    auto_check: jboolean,
    last_check_ms: jlong,
    now_ms: jlong,
) -> jboolean {
    jbool(UpdatePolicy::is_check_due(
        auto_check != JNI_FALSE,
        last_check_ms,
        now_ms,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_updatePolicyShouldOffer(
    mut env: JNIEnv,
    _: JClass,
    release_json: JString,
    current_version: JString,
    skipped_version: JString,
) -> jboolean {
    let Some(cur) = jstr(&mut env, &current_version) else {
        return JNI_FALSE;
    };
    let release = jstr(&mut env, &release_json).and_then(|s| parse_release_payload(&s));
    let skipped = jstr(&mut env, &skipped_version);
    jbool(UpdatePolicy::should_offer(
        release.as_ref(),
        &cur,
        skipped.as_deref(),
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_updatePolicyIsManagedByStore(
    mut env: JNIEnv,
    _: JClass,
    installer_package: JString,
) -> jboolean {
    let pkg = jstr(&mut env, &installer_package);
    jbool(UpdatePolicy::is_managed_by_store(pkg.as_deref()))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarPolicyJni_updatePolicyIsSelfUpdateAvailable(
    mut env: JNIEnv,
    _: JClass,
    installer_package: JString,
    github_distribution: jboolean,
) -> jboolean {
    let pkg = jstr(&mut env, &installer_package);
    jbool(UpdatePolicy::is_self_update_available(
        pkg.as_deref(),
        github_distribution != JNI_FALSE,
    ))
}

// ---------------------------------------------------------------------------
// NightjarHttpJni — Release fetch & SHA-256-verified APK download
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_updateFetchLatest(
    env: JNIEnv,
    _: JClass,
    target: jint,
) -> jstring {
    let Some(target) = UpdateTarget::from_code(target) else {
        return new_jstring(&env, &error_kind_json("malformed", "Unknown update target"));
    };
    let net = WebPkiTransport::new(USER_AGENT.to_owned());
    let json_out = match fetch_latest(&net, target) {
        Ok(release) => ok_json(release),
        Err(err) => update_error_json(&err),
    };
    new_jstring(&env, &json_out)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHttpJni_updateDownloadApk(
    mut env: JNIEnv,
    _: JClass,
    release_json: JString,
    root: jboolean,
    dir_path: JString,
    progress: JObject,
    cancel_handle: jlong,
) -> jstring {
    let Some(raw_release) = jstr(&mut env, &release_json) else {
        return new_jstring(&env, &error_kind_json("malformed", "Missing release"));
    };
    let Some(release) = parse_release_payload(&raw_release) else {
        return new_jstring(&env, &error_kind_json("malformed", "Unreadable release"));
    };
    let Some(dir) = jstr(&mut env, &dir_path) else {
        return new_jstring(&env, &error_kind_json("io", "Missing cache directory"));
    };
    let cancel: Option<Arc<TransferCancel>> = if cancel_handle > 0 {
        CANCELS.get(cancel_handle)
    } else {
        None
    };
    let mut net = WebPkiTransport::new(USER_AGENT.to_owned());
    if let Some(c) = &cancel {
        net = net.with_cancel(c.clone());
    }
    let out = match download_android_apk(
        &net,
        &release,
        root != JNI_FALSE,
        Path::new(&dir),
        cancel.as_deref(),
        &mut |done, total| {
            call_folder_progress(&mut env, &progress, done, (total > 0).then_some(total));
        },
    ) {
        Ok(path) => ok_json(path.to_string_lossy().as_ref()),
        Err(err) => update_error_json(&err),
    };
    new_jstring(&env, &out)
}
