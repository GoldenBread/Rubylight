//! Native C/ObjC dependency discovery and FFmpeg/Opus bindgen for nightjar-desktop-media.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CARGO_MANIFEST_DIR");
    println!("cargo:rerun-if-env-changed=NIGHTJAR_FFMPEG_DIR");
    let manifest_dir = std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"),
    );
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let metalfx_m = manifest_dir.join("src/stream/metalfx.m");
        println!("cargo:rerun-if-changed={}", metalfx_m.display());
        let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
        let arch = if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("aarch64") { "arm64" } else { "x86_64" };
        let status = std::process::Command::new("clang")
            .args(["-fobjc-arc", "-arch", arch, "-mmacosx-version-min=13.0", "-c"])
            .arg(&metalfx_m)
            .arg("-o")
            .arg(out.join("metalfx.o"))
            .status()
            .expect("compile MetalFX bridge");
        assert!(status.success(), "compile MetalFX bridge");
        assert!(
            std::process::Command::new("ar")
                .arg("crs")
                .arg(out.join("libnightjar_metalfx.a"))
                .arg(out.join("metalfx.o"))
                .status()
                .unwrap()
                .success()
        );
        println!("cargo:rustc-link-search=native={}", out.display());
        println!("cargo:rustc-link-lib=static=nightjar_metalfx");
        for framework in ["Foundation", "Metal", "MetalFX", "QuartzCore", "CoreVideo", "IOSurface"] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
    }
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    // MSYS2's UCRT64 on x86_64 PCs; ARM64 Windows has CLANGARM64 (clang, libc++, the UCRT).
    let msys = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => r"C:\msys64\clangarm64",
        _ => r"C:\msys64\ucrt64",
    };
    let prefix = std::env::var("NIGHTJAR_FFMPEG_DIR")
        .ok()
        .or_else(|| windows.then(|| msys.to_owned()));
    let mut bindings = bindgen::Builder::default()
        .header_contents(
            "nightjar-media.h",
            if windows {
                "#include <libavcodec/avcodec.h>\n#include <libavutil/hwcontext.h>\n#include <libavutil/pixdesc.h>\n#include <libavutil/hwcontext_d3d11va.h>\n#include <libavutil/hdr_dynamic_metadata.h>\n#include <libavutil/dovi_meta.h>\n"
            } else {
                "#include <libavcodec/avcodec.h>\n#include <libavutil/hwcontext.h>\n#include <libavutil/pixdesc.h>\n"
            },
        )
        .allowlist_type("AV.*|RcOverride|UINT")
        .allowlist_function("avcodec_.*|av_packet_.*|av_frame_.*|av_hwdevice_.*|av_hwframe_map|av_hwframe_transfer_data|av_pix_fmt_desc_get|av_get_pix_fmt_name|av_strerror|av_buffer_unref|avutil_version|av_buffer_pool_.*|av_dict_set|av_dict_free|av_dovi_find_level")
        .allowlist_var("AV_.*|FF_THREAD_SLICE")
        .allowlist_recursively(false)
        .blocklist_type("ID3D.*")
        .prepend_enum_name(false)
        .layout_tests(false)
        .generate_comments(false)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()));
    // clang has no "gnullvm" environment, which bindgen passes on when it cross-compiles (an ARM64
    // build from an x86_64 PC): the MinGW triple clang knows stands for the same ABI.
    if let Some(arch) = std::env::var("TARGET")
        .ok()
        .and_then(|t| t.strip_suffix("-pc-windows-gnullvm").map(str::to_owned))
    {
        bindings = bindings.clang_arg(format!("--target={arch}-w64-windows-gnu"));
    }
    if windows {
        for name in [
            "ID3D11Device",
            "ID3D11DeviceContext",
            "ID3D11VideoDevice",
            "ID3D11VideoContext",
            "ID3D11Texture2D",
        ] {
            bindings = bindings.raw_line(format!("pub type {name} = std::ffi::c_void;"));
        }
    }
    match prefix {
        Some(prefix) => {
            bindings = bindings.clang_arg(format!("-I{prefix}/include"));
            println!("cargo:rustc-link-search=native={prefix}/lib");
            // Windows packages bring Opus in the same prefix. On Linux Opus is the system's
            // (libopus.so.0 keeps its soname) and the package ships FFmpeg in lib/ beside the
            // executable: the run path finds it there, whatever FFmpeg the machine has.
            let libraries: &[&str] = if windows {
                &["avcodec", "avutil", "opus"]
            } else {
                &["avcodec", "avutil"]
            };
            for library in libraries {
                println!("cargo:rustc-link-lib=dylib={library}");
            }
            if !windows {
                pkg_config::Config::new()
                    .probe("opus")
                    .unwrap_or_else(|e| panic!("opus not found: {e}"));
            }
        }
        None => {
            for library in ["libavcodec", "libavutil", "opus"] {
                let found = pkg_config::Config::new()
                    .probe(library)
                    .unwrap_or_else(|e| panic!("{library} not found: {e}"));
                for path in found.include_paths {
                    bindings = bindings.clang_arg(format!("-I{}", path.display()));
                }
            }
        }
    }
    bindings
        .generate()
        .expect("FFmpeg bindings")
        .write_to_file(
            std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("media.rs"),
        )
        .expect("write FFmpeg bindings");
}
