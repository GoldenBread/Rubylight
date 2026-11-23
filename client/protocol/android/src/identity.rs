//! JNI bridge exports for `NightjarHostsJni` client pairing identity persistence
//! (`nightjar_core::hosts::IdentityStore`).

use jni::JNIEnv;
use jni::objects::{JByteArray, JClass, JString};
use jni::sys::jstring;
use nightjar_core::hosts::IdentityStore;
use serde::Serialize;

use crate::hosts::base64_encode;
use crate::http::new_jstring;

fn jstr(env: &mut JNIEnv, value: &JString) -> Option<String> {
    if value.is_null() {
        return None;
    }
    env.get_string(value).ok().map(Into::into)
}

#[derive(Serialize)]
struct IdentityDto {
    unique_id: String,
    cert_der_b64: String,
    key_pkcs8_b64: String,
    cert_pem: String,
    persisted: bool,
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_identityLoadOrCreate(
    mut env: JNIEnv,
    _: JClass,
    dir_path: JString,
    legacy_unique_id: JString,
    legacy_cert_pem: JString,
    legacy_key_pkcs8: JByteArray,
) -> jstring {
    let Some(dir) = jstr(&mut env, &dir_path) else {
        return new_jstring(
            &env,
            &serde_json::json!({ "error": "missing identity directory" }).to_string(),
        );
    };
    let legacy_uid = jstr(&mut env, &legacy_unique_id);
    let legacy_cert = jstr(&mut env, &legacy_cert_pem);
    let legacy_key = if legacy_key_pkcs8.is_null() {
        None
    } else {
        env.convert_byte_array(&legacy_key_pkcs8).ok()
    };

    let store = IdentityStore::open(dir);
    let reply = match store.load_or_create(
        legacy_uid.as_deref(),
        legacy_cert.as_deref(),
        legacy_key.as_deref(),
    ) {
        Ok(loaded) => serde_json::json!({
            "ok": IdentityDto {
                unique_id: loaded.unique_id,
                cert_der_b64: base64_encode(&loaded.certificate_der),
                key_pkcs8_b64: base64_encode(&loaded.private_key_pkcs8),
                cert_pem: loaded.certificate_pem,
                persisted: loaded.persisted,
            }
        })
        .to_string(),
        Err(err) => serde_json::json!({ "error": err }).to_string(),
    };
    new_jstring(&env, &reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_dto_serializes_base64_and_persists() {
        let dir = std::env::temp_dir().join(format!(
            "nightjar-android-id-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let loaded = IdentityStore::open(&dir).load_or_create(None, None, None).unwrap();
        assert!(loaded.persisted);
        let dto = IdentityDto {
            unique_id: loaded.unique_id.clone(),
            cert_der_b64: base64_encode(&loaded.certificate_der),
            key_pkcs8_b64: base64_encode(&loaded.private_key_pkcs8),
            cert_pem: loaded.certificate_pem.clone(),
            persisted: loaded.persisted,
        };
        assert_eq!(dto.unique_id.len(), 16);
        assert!(!dto.cert_der_b64.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
