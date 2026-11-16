//! The simple settings layer for the app's `com.limelight.preferences.SimpleProfileModel`: the
//! same mapping desktop Nightjar uses (`nightjar_policy::simple_profile`), so both apps read one
//! device's capabilities the same way. One JSON request in, one JSON answer out; the shared
//! contract file holds both sides of the JNI call.

use jni::objects::{JClass, JString};
use jni::sys::jstring;
use jni::JNIEnv;

use crate::native_api;

pub fn call(request: Option<&str>) -> String {
    match request {
        Some(request) => nightjar_policy::simple_profile::json::call(request),
        None => serde_json::json!({"error": "invalid", "detail": "the request is not a string"}).to_string(),
    }
}

#[no_mangle]
pub extern "system" fn Java_com_limelight_preferences_SimpleProfileModel_call(
    mut env: JNIEnv,
    _: JClass,
    request: JString,
) -> jstring {
    let request = native_api::string(&mut env, &request);
    match env.new_string(call(request.as_deref())) {
        Ok(answer) => answer.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire contract with the app: each request exactly as `SimpleProfileModel.java` sends
    /// it, and the answer this entry gives. The app's `SimpleProfileTest` sends and reads the same
    /// file, so a change on either side of the JNI call fails one of the two.
    #[test]
    fn answers_each_request_of_the_shared_contract_exactly() {
        let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!("simple_profile_contract.json")).unwrap();
        assert!(cases.len() >= 6);
        for case in cases {
            let request = case["request"].to_string();
            let answer: serde_json::Value = serde_json::from_str(&call(Some(&request))).unwrap();
            assert_eq!(answer, case["response"], "{}", case["name"]);
        }
    }

    #[test]
    fn names_a_missing_request() {
        let answer: serde_json::Value = serde_json::from_str(&call(None)).unwrap();
        assert_eq!(answer["error"], "invalid");
    }
}
