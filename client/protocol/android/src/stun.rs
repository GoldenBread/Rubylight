//! JNI bridge exports for `NightjarDiscoveryJni` RFC 5389 STUN packet encode/decode
//! (`nightjar_core::stun`).

use jni::JNIEnv;
use jni::objects::{JByteArray, JClass};
use jni::sys::{jbyteArray, jstring};
use nightjar_core::stun::{binding_request, mapped_address};

use crate::http::new_jstring;

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_stunBindingRequest(
    env: JNIEnv,
    _: JClass,
    transaction: JByteArray,
) -> jbyteArray {
    if transaction.is_null() {
        return std::ptr::null_mut();
    }
    let Ok(tx_bytes) = env.convert_byte_array(&transaction) else {
        return std::ptr::null_mut();
    };
    let Ok(tx): Result<[u8; 12], _> = tx_bytes.try_into() else {
        return std::ptr::null_mut();
    };
    let req = binding_request(&tx);
    env.byte_array_from_slice(&req)
        .map_or(std::ptr::null_mut(), |arr| arr.into_raw())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarDiscoveryJni_stunMappedAddress(
    env: JNIEnv,
    _: JClass,
    response: JByteArray,
    transaction: JByteArray,
) -> jstring {
    if response.is_null() || transaction.is_null() {
        return std::ptr::null_mut();
    }
    let Ok(resp_bytes) = env.convert_byte_array(&response) else {
        return std::ptr::null_mut();
    };
    let Ok(tx_bytes) = env.convert_byte_array(&transaction) else {
        return std::ptr::null_mut();
    };
    let Ok(tx): Result<[u8; 12], _> = tx_bytes.try_into() else {
        return std::ptr::null_mut();
    };
    let Some(ip) = mapped_address(&resp_bytes, &tx) else {
        return std::ptr::null_mut();
    };
    new_jstring(&env, &ip.to_string())
}
