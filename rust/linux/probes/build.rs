//! Generates the libva bindings from the system headers, so the encode
//! parameter structures match the installed libva exactly.
fn main() {
    println!("cargo:rerun-if-changed=wrapper.h");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    let mut includes = Vec::new();
    for lib in ["libva", "libva-drm"] {
        let found = pkg_config::Config::new()
            .probe(lib)
            .unwrap_or_else(|e| panic!("{lib} development files not found: {e}"));
        includes.extend(found.include_paths);
    }
    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_args(includes.iter().map(|p| format!("-I{}", p.display())))
        .allowlist_function("va[A-Z].*")
        .allowlist_type("VA.*")
        .allowlist_var("VA_.*")
        .prepend_enum_name(false)
        .layout_tests(false)
        .generate()
        .expect("libva bindings");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("va.rs");
    bindings.write_to_file(out).expect("write libva bindings");
}
