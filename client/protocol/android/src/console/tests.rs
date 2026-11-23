//! Host-safe unit tests for `nightjar-android::console` and `native_api` library/add ops.

use super::hidden_jni::find_cached_library_cover_json;
use super::jni::*;
use nightjar_policy::console::HomeSort;

#[test]
fn repo_lifecycle_and_slab_teardown_clean() {
    let dir = std::env::temp_dir().join(format!(
        "nightjar-android-console-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let h = create_repo(dir.to_str().unwrap());
    assert!(h > 0);
    destroy_repo(h);
    // Double destroy is harmless and never panics.
    destroy_repo(h);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(HomeSort::from_wire("last_played"), HomeSort::LastPlayed);
}

#[test]
fn find_cached_library_cover_json_reads_saved_envelope() {
    let dir = std::env::temp_dir().join(format!(
        "nightjar-android-cover-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let lib_dir = dir.join("console-library");
    std::fs::create_dir_all(&lib_dir).unwrap();
    let envelope = serde_json::json!({
        "fetched_ms": 1000,
        "features": [],
        "library": {
            "revision": "r1",
            "games": [{
                "install_id": "inst-1",
                "app_id": 831329605u32,
                "title": "Hollow Knight",
                "running": false,
                "added": true,
                "artwork": { "cover": "/c/hk", "version": "v1" }
            }]
        }
    });
    std::fs::write(
        lib_dir.join("pc-uuid.json"),
        serde_json::to_vec(&envelope).unwrap(),
    )
    .unwrap();

    let dir_str = dir.to_str().unwrap();
    let found = find_cached_library_cover_json(dir_str, "pc-uuid", 831329605, "").unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&found).unwrap();
    assert_eq!(parsed["explicitly_no_cover"], false);
    assert_eq!(parsed["app_id"], 831329605);
    assert_eq!(parsed["title"], "Hollow Knight");
    assert_eq!(parsed["cover_version"], "v1");
    assert!(find_cached_library_cover_json(dir_str, "pc-uuid", -1, "").is_none());

    let _ = std::fs::remove_dir_all(&dir);
}
