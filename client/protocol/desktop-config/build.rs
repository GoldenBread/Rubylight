//! Embeds `NIGHTJAR_VERSION` from `client/app/build.gradle` for `nightjar-desktop-config`.

#[path = "src/version_name.rs"]
mod version_name;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CARGO_MANIFEST_DIR");
    // The release version is Android's versionName, so both clients report the same one.
    let manifest_dir = std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"),
    );
    let gradle = manifest_dir.join("../../app/build.gradle");
    println!("cargo:rerun-if-changed=../../app/build.gradle");
    println!("cargo:rerun-if-changed=src/version_name.rs");
    let gradle_text = std::fs::read_to_string(&gradle)
        .unwrap_or_else(|e| panic!("read {}: {e}", gradle.display()));
    let version = version_name::parse(&gradle_text)
        .unwrap_or_else(|| panic!("no versionName \"<version>\" line in {}", gradle.display()));
    println!("cargo:rustc-env=NIGHTJAR_VERSION={version}");
}
