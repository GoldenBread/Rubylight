//! Embeds `NIGHTJAR_VERSION` from Android's `versionName`, compiles the Windows icon/manifest
//! resource, and sets the Linux binary rpath to `$ORIGIN/lib`.

#[path = "../desktop-config/src/version_name.rs"]
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
    println!("cargo:rerun-if-changed=../desktop-config/src/version_name.rs");
    let gradle_text = std::fs::read_to_string(&gradle)
        .unwrap_or_else(|e| panic!("read {}: {e}", gradle.display()));
    let version = version_name::parse(&gradle_text)
        .unwrap_or_else(|| panic!("no versionName \"<version>\" line in {}", gradle.display()));
    println!("cargo:rustc-env=NIGHTJAR_VERSION={version}");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" {
        let icon = manifest_dir.join("packaging/icons/nightjar.ico");
        let manifest = manifest_dir.join("packaging/nightjar.exe.manifest");
        println!("cargo:rerun-if-changed=packaging/icons/nightjar.ico");
        println!("cargo:rerun-if-changed=packaging/nightjar.exe.manifest");
        let rc = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("nightjar.rc");
        let path = icon.display().to_string().replace('\\', "/");
        let manifest_path = manifest.display().to_string().replace('\\', "/");
        std::fs::write(&rc, format!("1 ICON \"{path}\"\n1 24 \"{manifest_path}\"\n"))
            .unwrap_or_else(|e| panic!("write {}: {e}", rc.display()));
        embed_resource::compile_for(&rc, ["nightjar"], embed_resource::NONE)
            .manifest_required()
            .unwrap_or_else(|e| {
                panic!("embed the icon and manifest (needs windres or rc.exe on PATH): {e}")
            });
    } else if target_os == "linux" {
        // The Linux package ships FFmpeg in lib/ beside the executable; the run path finds it
        // there, whatever FFmpeg the machine has.
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN/lib");
    }
}
