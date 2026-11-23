//! JNI bridge for `PairingLink` and `InviteLink` (`NightjarJni.uriCall`), delegating parsing,
//! display formatting, and normalization to `rubylight_protocol::{pairing_uri, invite_uri}`.

use std::fmt::Write as _;

use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::jstring;
use rubylight_protocol::invite_uri::InviteUri;
use rubylight_protocol::pairing_uri::PairingUri;
use serde_json::{Value, json};

use crate::native_api;

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

pub fn call(request: Option<&str>) -> String {
    let Some(request) = request else {
        return json!({"error": "invalid", "message": "request is null"}).to_string();
    };
    let req: Value = match serde_json::from_str(request) {
        Ok(value) => value,
        Err(e) => return json!({"error": "json", "message": e.to_string()}).to_string(),
    };
    let text = req.get("text").and_then(Value::as_str).unwrap_or_default();
    match req.get("op").and_then(Value::as_str) {
        Some("pairing_is_link") => json!({"ok": PairingUri::is_link(text)}).to_string(),
        Some("pairing_parse") => {
            let parsed = PairingUri::parse(text).map(|p| {
                json!({
                    "otp": p.otp,
                    "passphrase": p.passphrase,
                    "unique_id": p.unique_id,
                    "name": p.name,
                    "shown_name": p.shown_name(),
                    "host": p.host,
                    "shown_address": p.shown_address(),
                    "port": p.port,
                    "cert_sha256_hex": hex(&p.cert_sha256),
                    "to_uri": p.to_uri(),
                })
            });
            json!({"ok": parsed}).to_string()
        }
        Some("invite_is_link") => json!({"ok": InviteUri::is_link(text)}).to_string(),
        Some("invite_parse") => {
            let parsed = InviteUri::parse(text).map(|inv| {
                let endpoints: Vec<String> =
                    inv.endpoints.iter().map(ToString::to_string).collect();
                let ipv6: Vec<String> = inv.ipv6.iter().map(ToString::to_string).collect();
                json!({
                    "secret_hex": hex(&inv.secret),
                    "unique_id": inv.unique_id,
                    "cert_sha256_hex": hex(&inv.cert_sha256),
                    "name": inv.name,
                    "shown_name": inv.shown_name(),
                    "expires": inv.expires,
                    "permission": inv.permission,
                    "ports": {
                        "http": inv.ports.http,
                        "https": inv.ports.https,
                        "rtsp": inv.ports.rtsp,
                        "video": inv.ports.video,
                        "audio": inv.ports.audio,
                        "control": inv.ports.control,
                        "mic": inv.ports.mic,
                    },
                    "session_hex": inv.session.map(|s| hex(&s)),
                    "endpoints": endpoints,
                    "nat": inv.nat,
                    "ipv6": ipv6,
                    "external_ipv4": inv.external_ipv4.map(|ip| ip.to_string()),
                    "mapped_ports": inv.mapped_ports,
                    "to_uri": inv.to_uri(),
                })
            });
            json!({"ok": parsed}).to_string()
        }
        _ => json!({"error": "invalid", "message": "unknown uri op"}).to_string(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_nativeUriCall(
    mut env: JNIEnv,
    _: JClass,
    request: JString,
) -> jstring {
    let request = native_api::string(&mut env, &request);
    let reply = call(request.as_deref());
    match env.new_string(reply) {
        Ok(s) => s.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_trimmed_pairing_and_invite_uris() {
        let pairing = "  rubylight://pair?v=1&host=192.168.1.10&port=47989&otp=1234&pass=secret&id=my-pc&fp=00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff&name=My%20PC  ";
        let is_link: Value = serde_json::from_str(&call(Some(
            &json!({"op": "pairing_is_link", "text": pairing}).to_string(),
        )))
        .unwrap();
        assert_eq!(is_link["ok"], true);
        let parsed: Value = serde_json::from_str(&call(Some(
            &json!({"op": "pairing_parse", "text": pairing}).to_string(),
        )))
        .unwrap();
        assert_eq!(parsed["ok"]["shown_name"], "My PC");
        assert_eq!(parsed["ok"]["port"], 47989);
    }

    #[test]
    fn percent_nul_in_pairing_and_invite_fields_round_trips_over_json() {
        let fp = "ab".repeat(32);
        let pairing = format!(
            "rubylight://pair?v=1&host=192.168.1.10&port=47989&otp=12%0034&pass=a%00b&id=id%001&fp={fp}&name=My%00PC"
        );
        let parsed_pair: Value = serde_json::from_str(&call(Some(
            &json!({"op": "pairing_parse", "text": pairing}).to_string(),
        )))
        .unwrap();
        assert_eq!(parsed_pair["ok"]["otp"], "12\u{0}34");
        assert_eq!(parsed_pair["ok"]["passphrase"], "a\u{0}b");
        assert_eq!(parsed_pair["ok"]["unique_id"], "id\u{0}1");
        assert_eq!(parsed_pair["ok"]["name"], "My\u{0}PC");
        assert_eq!(parsed_pair["ok"]["shown_name"], "MyPC");

        let invite = format!(
            "rubylight://invite#v=1&k={}&id=pc-1&fp={fp}&name=Room%00PC&exp=1800000000&perm=view&ports=47989,47984,48010,47998,48000,47999,0",
            "01".repeat(32)
        );
        let parsed_inv: Value = serde_json::from_str(&call(Some(
            &json!({"op": "invite_parse", "text": invite}).to_string(),
        )))
        .unwrap();
        assert_eq!(parsed_inv["ok"]["name"], "Room\u{0}PC");
        assert_eq!(parsed_inv["ok"]["shown_name"], "RoomPC");
    }
}
