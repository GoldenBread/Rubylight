//! JNI bridge exports for `NightjarHostsJni` unified `hosts.json` registry (`HostStore`),
//! mapping Android `ComputerDetails` slots and DER certificates onto `nightjar_core::hosts::HostEntry`.

#[path = "host_flows.rs"]
pub mod host_flows;
#[path = "hosts_manager.rs"]
pub mod hosts_manager;
#[path = "hosts_poller.rs"]
pub mod hosts_poller;

use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::{JNI_FALSE, JNI_TRUE, jboolean, jlong, jstring};
use nightjar_core::hosts::{EndpointSlot, HostEntry, HostStore};
use serde::{Deserialize, Serialize};

use crate::http::new_jstring;
use crate::slab::Slab;

static HOST_STORES: Slab<HostStore> = Slab::new();

pub(crate) fn jstr(env: &mut JNIEnv, value: &JString) -> Option<String> {
    if value.is_null() {
        return None;
    }
    env.get_string(value).ok().map(Into::into)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct JniHostDto {
    pub(crate) uuid: String,
    pub(crate) name: String,
    pub(crate) local: Option<EndpointSlot>,
    pub(crate) remote: Option<EndpointSlot>,
    pub(crate) manual: Option<EndpointSlot>,
    pub(crate) ipv6: Option<EndpointSlot>,
    pub(crate) mac: Option<String>,
    pub(crate) server_cert_b64: Option<String>,
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(BASE64[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub(crate) fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0;
    let mut padding = 0;
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => {
                padding += 1;
                continue;
            }
            b' ' | b'\t' | b'\r' | b'\n' => continue,
            _ => return None,
        };
        if padding > 0 {
            return None;
        }
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    if padding > 2 || bits >= 6 {
        return None;
    }
    Some(out)
}

pub(crate) fn entry_to_dto(entry: &HostEntry) -> JniHostDto {
    let has_android_slots = entry.local_endpoint.is_some()
        || entry.remote_endpoint.is_some()
        || entry.manual_endpoint.is_some()
        || entry.ipv6_endpoint.is_some();
    let fallback_port = if entry.shared.http_port == 0 {
        47989
    } else {
        entry.shared.http_port
    };
    let legacy_slot = if !has_android_slots && !entry.shared.address.is_empty() {
        EndpointSlot::new(&entry.shared.address, fallback_port)
    } else {
        None
    };
    let local = if has_android_slots {
        entry.local_endpoint.clone()
    } else if !entry.shared.manual {
        legacy_slot.clone()
    } else {
        None
    };
    let manual = if has_android_slots {
        entry.manual_endpoint.clone()
    } else if entry.shared.manual {
        legacy_slot
    } else {
        None
    };
    let remote = entry.remote_endpoint.clone().or_else(|| {
        entry
            .shared
            .remote_address
            .as_deref()
            .and_then(|addr| EndpointSlot::new(addr, fallback_port))
    });
    let server_cert_b64 = entry
        .server_certificate_der()
        .map(|der| base64_encode(&der));
    JniHostDto {
        uuid: entry.shared.unique_id.clone(),
        name: entry.shared.name.clone(),
        local,
        remote,
        manual,
        ipv6: entry.ipv6_endpoint.clone(),
        mac: entry.shared.mac.clone(),
        server_cert_b64,
    }
}

pub(crate) fn dto_to_entry(dto: JniHostDto) -> HostEntry {
    let mut entry = HostEntry::default();
    entry.shared.unique_id = dto.uuid;
    entry.shared.name = dto.name;
    entry.shared.mac = dto.mac;
    entry.local_endpoint = dto.local.clone();
    entry.remote_endpoint = dto.remote.clone();
    entry.manual_endpoint = dto.manual.clone();
    entry.ipv6_endpoint = dto.ipv6.clone();

    // Populate shared Desktop fields cleanly via policy (`remote` NEVER promotes to `shared.address`).
    entry.sync_shared_from_slots();

    if let Some(b64) = dto.server_cert_b64.as_deref()
        && let Some(der) = base64_decode(b64)
    {
        entry.set_server_certificate_der(Some(&der));
    }
    entry
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostStoreOpen(
    mut env: JNIEnv,
    _: JClass,
    dir_path: JString,
) -> jlong {
    let Some(dir) = jstr(&mut env, &dir_path) else {
        return 0;
    };
    HOST_STORES.insert(HostStore::open(dir))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostStoreDestroy(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    HOST_STORES.remove(handle);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostStoreAll(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jstring {
    let Some(store) = HOST_STORES.get(handle) else {
        return new_jstring(&env, "[]");
    };
    let dtos: Vec<JniHostDto> = store.all().iter().map(entry_to_dto).collect();
    let json = serde_json::to_string(&dtos).unwrap_or_else(|_| "[]".into());
    new_jstring(&env, &json)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostStoreGetByUuid(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
) -> jstring {
    let Some(store) = HOST_STORES.get(handle) else {
        return std::ptr::null_mut();
    };
    let Some(uuid) = jstr(&mut env, &uuid) else {
        return std::ptr::null_mut();
    };
    let Some(entry) = store.get_by_uuid(&uuid) else {
        return std::ptr::null_mut();
    };
    let json = serde_json::to_string(&entry_to_dto(&entry)).unwrap_or_default();
    new_jstring(&env, &json)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostStoreGetByName(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    name: JString,
) -> jstring {
    let Some(store) = HOST_STORES.get(handle) else {
        return std::ptr::null_mut();
    };
    let Some(name) = jstr(&mut env, &name) else {
        return std::ptr::null_mut();
    };
    let Some(entry) = store.get_by_name(&name) else {
        return std::ptr::null_mut();
    };
    let json = serde_json::to_string(&entry_to_dto(&entry)).unwrap_or_default();
    new_jstring(&env, &json)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostStoreUpsert(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    host_json: JString,
) -> jboolean {
    let Some(store) = HOST_STORES.get(handle) else {
        return JNI_FALSE;
    };
    let Some(raw) = jstr(&mut env, &host_json) else {
        return JNI_FALSE;
    };
    let Ok(dto) = serde_json::from_str::<JniHostDto>(&raw) else {
        return JNI_FALSE;
    };
    if store.upsert(dto_to_entry(dto)).unwrap_or(false) {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostStoreDelete(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    uuid: JString,
) -> jboolean {
    let Some(store) = HOST_STORES.get(handle) else {
        return JNI_FALSE;
    };
    let Some(uuid) = jstr(&mut env, &uuid) else {
        return JNI_FALSE;
    };
    if store.delete_by_uuid(&uuid).unwrap_or(false) {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostStoreHasMigrated(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jboolean {
    let Some(store) = HOST_STORES.get(handle) else {
        return JNI_FALSE;
    };
    if store.has_migrated_legacy() {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_hostStoreImportLegacy(
    mut env: JNIEnv,
    _: JClass,
    handle: jlong,
    legacy_array_json: JString,
) -> jboolean {
    let Some(store) = HOST_STORES.get(handle) else {
        return JNI_FALSE;
    };
    let raw = jstr(&mut env, &legacy_array_json).unwrap_or_else(|| "[]".into());
    let Ok(dtos) = serde_json::from_str::<Vec<JniHostDto>>(&raw) else {
        return JNI_FALSE;
    };
    let entries: Vec<HostEntry> = dtos.into_iter().map(dto_to_entry).collect();
    if store.import_legacy_if_needed(entries).unwrap_or(false) {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_and_dto_roundtrip() {
        let raw = b"\x30\x82\x01\x0a\x00\xff";
        let b64 = base64_encode(raw);
        assert_eq!(base64_decode(&b64).as_deref(), Some(raw.as_slice()));

        let dto = JniHostDto {
            uuid: "host-1".into(),
            name: "Rig".into(),
            local: EndpointSlot::new("192.168.1.5", 47989),
            remote: None,
            manual: None,
            ipv6: None,
            mac: Some("aa:bb:cc:dd:ee:ff".into()),
            server_cert_b64: None,
        };
        let entry = dto_to_entry(dto);
        let back = entry_to_dto(&entry);
        assert_eq!(back.uuid, "host-1");
        assert_eq!(back.local.unwrap().address, "192.168.1.5");
    }

    #[test]
    fn entry_to_dto_never_fabricates_local_for_manual_remote_ipv6_or_cleared_hosts() {
        // Manual-only Android host.
        let manual = dto_to_entry(JniHostDto {
            uuid: "manual-only".into(),
            name: "Manual".into(),
            manual: EndpointSlot::new("my-pc.example", 47989),
            ..Default::default()
        });
        let manual_back = entry_to_dto(&manual);
        assert_eq!(manual_back.local, None);
        assert_eq!(manual_back.manual, EndpointSlot::new("my-pc.example", 47989));

        // Remote-only Android host.
        let remote = dto_to_entry(JniHostDto {
            uuid: "remote-only".into(),
            name: "Remote".into(),
            remote: EndpointSlot::new("203.0.113.55", 48989),
            ..Default::default()
        });
        let remote_back = entry_to_dto(&remote);
        assert_eq!(remote_back.local, None);
        assert_eq!(remote_back.remote, EndpointSlot::new("203.0.113.55", 48989));

        // IPv6-only Android host.
        let v6 = dto_to_entry(JniHostDto {
            uuid: "ipv6-only".into(),
            name: "V6".into(),
            ipv6: EndpointSlot::new("2001:db8::1", 47989),
            ..Default::default()
        });
        let v6_back = entry_to_dto(&v6);
        assert_eq!(v6_back.local, None);
        assert_eq!(v6_back.ipv6, EndpointSlot::new("2001:db8::1", 47989));

        // Desktop-only manual entry without Android slot fields maps shared.address -> manual, not local.
        let mut desktop_manual = HostEntry::default();
        desktop_manual.shared.unique_id = "desk-manual".into();
        desktop_manual.shared.address = "desk.example".into();
        desktop_manual.shared.http_port = 47989;
        desktop_manual.shared.manual = true;
        let desk_manual_dto = entry_to_dto(&desktop_manual);
        assert_eq!(desk_manual_dto.local, None);
        assert_eq!(desk_manual_dto.manual, EndpointSlot::new("desk.example", 47989));

        // Desktop-only LAN entry without Android slot fields maps shared.address -> local with actual port.
        let mut desktop_lan = HostEntry::default();
        desktop_lan.shared.unique_id = "desk-lan".into();
        desktop_lan.shared.address = "192.168.1.20".into();
        desktop_lan.shared.http_port = 47980;
        desktop_lan.shared.remote_address = Some("203.0.113.5".into());
        let desk_lan_dto = entry_to_dto(&desktop_lan);
        assert_eq!(desk_lan_dto.local, EndpointSlot::new("192.168.1.20", 47980));
        assert_eq!(desk_lan_dto.remote, EndpointSlot::new("203.0.113.5", 47980));
        assert_eq!(desk_lan_dto.manual, None);
    }

    #[test]
    fn android_upsert_clears_local_detects_noops_and_preserves_desktop_fields() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("nightjar-android-hosts-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        let store = HostStore::open(&dir);

        let mut desktop = HostEntry::default();
        desktop.shared.unique_id = "pc-1".into();
        desktop.shared.name = "DesktopPC".into();
        desktop.shared.alias = Some("Studio".into());
        desktop.shared.address = "192.168.1.10".into();
        desktop.shared.http_port = 47989;
        desktop.shared.https_port = 47984;
        desktop.shared.other_addresses = vec!["10.0.0.50".into()];
        desktop.shared.other_http_ports = vec![48000];
        desktop.shared.pinned_address = Some("10.0.0.50".into());
        desktop.shared.remote_address = Some("203.0.113.99".into());
        desktop.shared.wake_macs = vec!["aa:bb:cc:dd:ee:02".into()];
        desktop.shared.wake_broadcasts = vec!["10.0.0.255".into()];
        desktop.shared.own_wake_macs = vec!["aa:bb:cc:dd:ee:03".into()];
        desktop.shared.quit_on_leave.insert("uuid:g1".into());
        desktop.shared.notices.insert("PYROWAVE_FALLBACK".into());
        desktop.shared.full_chroma.modes = Some(3);
        desktop.shared.internet_play = Some("secret-token".into());
        desktop.pinned_endpoint = EndpointSlot::new("10.0.0.50", 48000);
        desktop.extra.insert("settings".into(), serde_json::json!({"fps": 120}));
        store.save_records(&[desktop.clone()]).unwrap();

        // Android reads, clears local while keeping remote, and upserts.
        let mut dto = entry_to_dto(&store.get_by_uuid("pc-1").unwrap());
        assert_eq!(dto.local, EndpointSlot::new("192.168.1.10", 47989));
        dto.local = None;
        assert!(store.upsert(dto_to_entry(dto.clone())).unwrap());
        assert!(!store.upsert(dto_to_entry(dto)).unwrap(), "identical second upsert returns false");

        let reloaded = store.get_by_uuid("pc-1").unwrap();
        assert!(
            reloaded.shared.address.is_empty(),
            "remote slot must never promote into shared.address"
        );
        let back = entry_to_dto(&reloaded);
        assert_eq!(back.local, None);
        assert_eq!(back.remote, EndpointSlot::new("203.0.113.99", 47989));
        assert_eq!(reloaded.shared.alias.as_deref(), Some("Studio"));
        assert_eq!(reloaded.shared.https_port, 47984);
        assert_eq!(reloaded.shared.other_addresses, vec!["10.0.0.50"]);
        assert_eq!(reloaded.shared.other_http_ports, vec![48000]);
        assert_eq!(reloaded.shared.pinned_address.as_deref(), Some("10.0.0.50"));
        assert_eq!(reloaded.shared.wake_macs, vec!["aa:bb:cc:dd:ee:02"]);
        assert_eq!(reloaded.shared.wake_broadcasts, vec!["10.0.0.255"]);
        assert_eq!(reloaded.shared.own_wake_macs, vec!["aa:bb:cc:dd:ee:03"]);
        assert!(reloaded.shared.quit_on_leave.contains("uuid:g1"));
        assert!(reloaded.shared.notices.contains("PYROWAVE_FALLBACK"));
        assert_eq!(reloaded.shared.full_chroma.modes, Some(3));
        assert_eq!(reloaded.shared.internet_play.as_deref(), Some("secret-token"));
        assert_eq!(reloaded.pinned_endpoint, EndpointSlot::new("10.0.0.50", 48000));
        assert_eq!(reloaded.extra.get("settings"), Some(&serde_json::json!({"fps": 120})));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
