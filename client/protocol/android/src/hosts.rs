//! JNI bridge exports for `NightjarHostsJni` unified `hosts.json` registry (`HostStore`),
//! mapping Android `ComputerDetails` slots and DER certificates onto `nightjar_core::hosts::HostEntry`.

use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::{JNI_FALSE, JNI_TRUE, jboolean, jlong, jstring};
use nightjar_core::hosts::{EndpointSlot, HostEntry, HostStore};
use serde::{Deserialize, Serialize};

use crate::http::new_jstring;
use crate::slab::Slab;

static HOST_STORES: Slab<HostStore> = Slab::new();

fn jstr(env: &mut JNIEnv, value: &JString) -> Option<String> {
    if value.is_null() {
        return None;
    }
    env.get_string(value).ok().map(Into::into)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct JniHostDto {
    uuid: String,
    name: String,
    local: Option<EndpointSlot>,
    remote: Option<EndpointSlot>,
    manual: Option<EndpointSlot>,
    ipv6: Option<EndpointSlot>,
    mac: Option<String>,
    server_cert_b64: Option<String>,
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

fn entry_to_dto(entry: &HostEntry) -> JniHostDto {
    let local = entry.local_endpoint.clone().or_else(|| {
        if !entry.shared.address.is_empty() && entry.shared.http_port > 0 {
            EndpointSlot::new(&entry.shared.address, entry.shared.http_port)
        } else {
            None
        }
    });
    let remote = entry.remote_endpoint.clone().or_else(|| {
        entry
            .shared
            .remote_address
            .as_deref()
            .and_then(|addr| EndpointSlot::new(addr, entry.shared.http_port.max(47989)))
    });
    let server_cert_b64 = entry
        .server_certificate_der()
        .map(|der| base64_encode(&der));
    JniHostDto {
        uuid: entry.shared.unique_id.clone(),
        name: entry.shared.name.clone(),
        local,
        remote,
        manual: entry.manual_endpoint.clone(),
        ipv6: entry.ipv6_endpoint.clone(),
        mac: entry.shared.mac.clone(),
        server_cert_b64,
    }
}

fn dto_to_entry(dto: JniHostDto) -> HostEntry {
    let mut entry = HostEntry::default();
    entry.shared.unique_id = dto.uuid;
    entry.shared.name = dto.name;
    entry.shared.mac = dto.mac;
    entry.local_endpoint = dto.local.clone();
    entry.remote_endpoint = dto.remote.clone();
    entry.manual_endpoint = dto.manual.clone();
    entry.ipv6_endpoint = dto.ipv6.clone();

    // Populate shared Desktop fields cleanly from primary endpoints.
    let primary = dto
        .local
        .as_ref()
        .or(dto.manual.as_ref())
        .or(dto.remote.as_ref())
        .or(dto.ipv6.as_ref());
    if let Some(p) = primary {
        entry.shared.address = p.address.clone();
        entry.shared.http_port = p.port;
    }
    entry.shared.remote_address = dto.remote.as_ref().map(|r| r.address.clone());
    entry.shared.manual = dto.manual.is_some() && dto.local.is_none();

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
    if store.upsert(dto_to_entry(dto)).is_ok() {
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
}
