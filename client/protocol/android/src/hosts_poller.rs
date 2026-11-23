//! JNI bridge exports for `NightjarHostsJni` background app-list polling (`ApplistPollerHandle`).

use std::collections::VecDeque;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use jni::JNIEnv;
use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{jint, jlong, jstring};
use nightjar_core::hosts::client_pool::HostClientPool;
use nightjar_core::hosts::applist::{ApplistPollerHandle, ApplistUpdate};
use nightjar_core::http::ClientIdentity;

use crate::http::new_jstring;
use crate::slab::{Slab, lock};

use super::hosts_manager::HOST_MANAGERS;
use super::jstr;

struct PollerBridge {
    handle: ApplistPollerHandle,
    updates: Arc<(Mutex<VecDeque<ApplistUpdate>>, Condvar)>,
    closed: Arc<AtomicBool>,
}

impl PollerBridge {
    fn shutdown(&self) {
        self.closed.store(true, Ordering::Release);
        self.updates.1.notify_all();
        self.handle.stop();
    }
}

static APPLIST_POLLERS: Slab<PollerBridge> = Slab::new();

pub(crate) fn drain_all_pollers() {
    for bridge in APPLIST_POLLERS.drain_all() {
        bridge.shutdown();
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_applistPollerStart(
    mut env: JNIEnv,
    _: JClass,
    host_manager_handle: jlong,
    address: JString,
    http_port: jint,
    https_port: jint,
    cert_der: JByteArray,
) -> jlong {
    let Some(address) = jstr(&mut env, &address) else {
        return 0;
    };
    let cert_bytes = if cert_der.is_null() {
        return 0;
    } else if let Ok(bytes) = env.convert_byte_array(&cert_der) {
        bytes
    } else {
        return 0;
    };
    let pool: Arc<HostClientPool> = match HOST_MANAGERS.get(host_manager_handle) {
        Some(bridge) => Arc::clone(bridge.manager.pool()),
        None => {
            let Ok(id) = ClientIdentity::generate() else {
                return 0;
            };
            Arc::new(HostClientPool::new(Arc::new(id)))
        }
    };
    let updates: Arc<(Mutex<VecDeque<ApplistUpdate>>, Condvar)> =
        Arc::new((Mutex::new(VecDeque::new()), Condvar::new()));
    let closed = Arc::new(AtomicBool::new(false));
    let updates_cb = Arc::clone(&updates);
    let closed_cb = Arc::clone(&closed);
    let handle = ApplistPollerHandle::start(
        pool,
        address,
        http_port.clamp(0, u16::MAX as i32) as u16,
        https_port.clamp(0, u16::MAX as i32) as u16,
        cert_bytes,
        Box::new(move |update| {
            if closed_cb.load(Ordering::Acquire) {
                return;
            }
            let mut q = lock(&updates_cb.0);
            if q.len() >= 16 {
                q.pop_front();
            }
            q.push_back(update);
            updates_cb.1.notify_all();
        }),
    );
    APPLIST_POLLERS.insert(PollerBridge {
        handle,
        updates,
        closed,
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_applistPollerPollNow(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    if let Some(bridge) = APPLIST_POLLERS.get(handle) {
        bridge.handle.poll_now();
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_applistPollerNextUpdate(
    env: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jstring {
    let Some(bridge) = APPLIST_POLLERS.get(handle) else {
        return ptr::null_mut();
    };
    let (lock_q, cvar) = &*bridge.updates;
    let mut q = lock(lock_q);
    while q.is_empty() && !bridge.closed.load(Ordering::Acquire) {
        q = cvar.wait(q).unwrap_or_else(|e| e.into_inner());
    }
    if bridge.closed.load(Ordering::Acquire) {
        return ptr::null_mut();
    }
    let Some(update) = q.pop_front() else {
        return ptr::null_mut();
    };
    new_jstring(&env, &update.raw_xml)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarHostsJni_applistPollerStop(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    if let Some(bridge) = APPLIST_POLLERS.take(handle) {
        bridge.shutdown();
    }
}
