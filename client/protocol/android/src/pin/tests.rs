//! Unit tests for `nightjar-android::pin` (`canonical_dir`, `STORE_CACHE` sharing across
//! directory creation, and dual-key cache eviction).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nightjar_core::pin_lock::{Mode, RingPinCrypto, hasher, import_legacy_json};
use serde_json::json;

use super::{STORE_CACHE, canonical_dir, evict_cache, open_store};
use crate::slab::lock;

fn fresh_root(name: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "nj-android-pin-{}-{}-{name}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn open_store_shares_cache_and_mode_across_uncreated_to_created_dir() {
    let root = fresh_root("cache-share");
    let uncreated = root.join("nested").join("pin_dir");
    assert!(!uncreated.exists());

    let h1 = open_store(&uncreated, true);
    assert!(
        !uncreated.exists(),
        "opening an uncreated directory must not create it eagerly"
    );

    h1.store.set_pin_with(b"1234", 1).unwrap();
    assert!(uncreated.exists());

    let h2 = open_store(&uncreated, true);
    assert!(
        Arc::ptr_eq(&h1, &h2),
        "STORE_CACHE must return the same AndroidStore after dir creation"
    );

    h1.store.set_mode(Mode::App);
    assert_eq!(h2.store.mode(), Mode::App);
    assert_eq!(h2.store.view().mode, Mode::App);

    evict_cache(&uncreated);
    let h3 = open_store(&uncreated, true);
    assert!(
        !Arc::ptr_eq(&h1, &h3),
        "evict_cache must remove the cached entry"
    );

    evict_cache(&uncreated);
    drop((h1, h2, h3));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn canonical_dir_resolves_parent_traversal_and_uncreated_tails() {
    let root = fresh_root("canon-tail");
    let existing = root.join("existing");
    std::fs::create_dir_all(&existing).unwrap();

    let traversal = existing.join("missing_a").join("..").join("missing_b");
    let expected = std::fs::canonicalize(&existing).unwrap().join("missing_b");
    assert_eq!(canonical_dir(&traversal), expected);
    assert!(!existing.join("missing_a").exists());

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn evict_cache_removes_both_raw_and_canonical_keys() {
    let root = fresh_root("evict-dual");
    let target = root.join("sub");
    std::fs::create_dir_all(&target).unwrap();

    let h = open_store(&target, true);
    let canon = canonical_dir(&target);
    let raw_variant = root.join("sub").join("..").join("sub");
    assert_ne!(raw_variant, canon);
    lock(&STORE_CACHE).insert(raw_variant.clone(), h.clone());

    evict_cache(&raw_variant);
    let cache = lock(&STORE_CACHE);
    assert!(!cache.contains_key(&raw_variant));
    assert!(!cache.contains_key(&canon));
    drop(cache);
    drop(h);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn import_legacy_json_updates_cached_store_opened_before_dir_created() {
    let root = fresh_root("legacy-uncreated");
    let uncreated = root.join("legacy").join("dir");

    let h1 = open_store(&uncreated, true);
    assert!(!uncreated.exists());

    let hash = hasher::create(b"4321", 1).unwrap();
    let legacy = json!({
        "pin_hash": hash,
        "pin_length": 4,
        "pin_mode": "streams",
    })
    .to_string();
    let key = canonical_dir(&uncreated);
    assert!(import_legacy_json(&key, &legacy, &RingPinCrypto).unwrap());
    if let Some(existing) = lock(&STORE_CACHE).get(&key).cloned() {
        existing.store.reload_now();
    }

    let h2 = open_store(&uncreated, true);
    assert!(Arc::ptr_eq(&h1, &h2));
    assert!(h2.store.has_pin());
    assert_eq!(h2.store.mode(), Mode::Streams);

    evict_cache(&uncreated);
    drop((h1, h2));
    let _ = std::fs::remove_dir_all(&root);
}
