//! Release pin, checksum list, download verification, and cached archive
//! package tests for Neural Rendering.
use super::*;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf};

#[test]
fn only_the_two_packages_and_the_checksums_may_be_fetched() {
    assert_eq!(asset("anr-fg-V.B16-full.zip"), Some(Asset::Full("V.B16")));
    assert_eq!(asset("anr-fg-V.B16-addon.zip"), Some(Asset::Addon("V.B16")));
    assert_eq!(asset(SUMS), Some(Asset::Sums));
    for name in [
        "anr-fg-V.B16-kit-windows.zip",
        "anr-fg-V.B16-kit-linux.tar.gz",
        "anr-fg-V.B16-full-optiscaler-source.zip",
        "anr-fg--full.zip",
        "anr-fg-V/B16-full.zip",
        "AMDNR-v0.3.5.8R2.zip",
        "AMDNR-lmxxf-0.3.5.8R2.zip",
        "sha256sums.txt",
        "install.ps1",
    ] {
        assert!(!allowed_asset(name), "{name}");
    }
    // The pin names only files the allowlist takes.
    for file in [PINNED.full, PINNED.addon, PINNED.sums] {
        assert!(allowed_asset(file.asset), "{}", file.asset);
        assert_eq!(file.sha256.len(), 64);
    }
}

fn listed(tag: &str, files: &[(&str, &str, u64)]) -> Value {
    json!({
        "tag_name": tag,
        "draft": false,
        "assets": files.iter().map(|(name, sha, size)| json!({
            "name": name,
            "size": size,
            "digest": format!("sha256:{sha}"),
            "browser_download_url": format!("{GITHUB}/{REPOSITORY}/releases/download/{tag}/{name}"),
        })).collect::<Vec<_>>(),
    })
}

fn pinned_list() -> Value {
    listed(
        PINNED.tag,
        &[
            (PINNED.full.asset, PINNED.full.sha256, 10_782_205),
            (PINNED.addon.asset, PINNED.addon.sha256, 3_156_952),
            (PINNED.sums.asset, PINNED.sums.sha256, 500),
            ("anr-fg-V.B16-kit-windows.zip", &"0".repeat(64), 1),
        ],
    )
}

#[test]
fn only_the_pinned_release_with_its_pinned_digests_is_taken() {
    let release = pinned(&pinned_list(), GITHUB, &PINNED).unwrap();
    assert_eq!(release.tag, "FG-V.B16");
    assert_eq!(release.full.asset, PINNED.full.asset);
    assert_eq!(release.full.digest, PINNED.full.sha256);
    assert_eq!(release.addon.size, 3_156_952);
    assert_eq!(
        release.sums.url,
        format!("{GITHUB}/apex-compute/ANR/releases/download/FG-V.B16/SHA256SUMS.txt")
    );
    // Another tag, a draft, a changed digest, a missing file, an odd address.
    let mut other = pinned_list();
    other["tag_name"] = json!("FG-V.B17");
    assert!(pinned(&other, GITHUB, &PINNED).is_err());
    let mut draft = pinned_list();
    draft["draft"] = json!(true);
    assert!(pinned(&draft, GITHUB, &PINNED).is_err());
    let mut changed = pinned_list();
    changed["assets"][1]["digest"] = json!(format!("sha256:{}", "1".repeat(64)));
    let error = pinned(&changed, GITHUB, &PINNED).unwrap_err();
    assert!(error.contains("another SHA-256"), "{error}");
    let mut sums = pinned_list();
    sums["assets"][2]["digest"] = json!(format!("sha256:{}", "2".repeat(64)));
    assert!(pinned(&sums, GITHUB, &PINNED).is_err());
    let mut gone = pinned_list();
    gone["assets"].as_array_mut().unwrap().remove(0);
    assert!(pinned(&gone, GITHUB, &PINNED).is_err());
    let mut moved = pinned_list();
    moved["assets"][0]["browser_download_url"] = json!("https://example.com/x.zip");
    assert!(pinned(&moved, GITHUB, &PINNED).is_err());
    let mut huge = pinned_list();
    huge["assets"][0]["size"] = json!(MAX_ARCHIVE + 1);
    assert!(pinned(&huge, GITHUB, &PINNED).is_err());
}

#[test]
fn every_archive_must_be_in_the_releases_checksums_with_its_digest() {
    let release = pinned(&pinned_list(), GITHUB, &PINNED).unwrap();
    let text = format!(
        "{}  {}\n{}  {}\n{}  anr-fg-V.B16-kit-windows.zip\n",
        PINNED.full.sha256,
        PINNED.full.asset,
        PINNED.addon.sha256.to_uppercase(),
        PINNED.addon.asset,
        "8".repeat(64)
    );
    let list = sums(&text).unwrap();
    assert_eq!(
        check_archives(&[&release.full, &release.addon], &list),
        Ok(())
    );
    let without = sums(&format!("{}  {}\n", PINNED.full.sha256, PINNED.full.asset)).unwrap();
    assert!(check_archives(&[&release.full, &release.addon], &without).is_err());
    let wrong = sums(&format!("{}  {}\n", "9".repeat(64), PINNED.full.asset)).unwrap();
    assert!(check_archives(&[&release.full], &wrong).is_err());
    assert!(sums("not a checksum").is_err());
    assert!(sums(&format!("{0}  a\n{1}  a\n", "1".repeat(64), "2".repeat(64))).is_err());
}

#[test]
fn the_archives_give_only_optiscaler_the_provider_and_the_licence() {
    for path in [
        "game/dxgi.dll",
        "game/OptiScaler.ini",
        "game/OptiScaler/custom/Apex Neural Rendering FG/dlssg/nvngx_dlssg.dll",
        "LICENSES/OptiScaler-GPL-3.0.txt",
    ] {
        assert!(Archive::Full.takes(path), "{path}");
    }
    for path in [
        "install.ps1",
        "install.cmd",
        "README.txt",
        "nr_fg_package.json",
    ] {
        assert!(!Archive::Full.takes(path), "{path}");
        assert!(!Archive::Addon.takes(path), "{path}");
    }
    assert!(Archive::Addon.takes("Apex Neural Rendering FG/dlssg/nr_fg.ini"));
    assert!(!Archive::Addon.takes("game/dxgi.dll"));

    let mut files: BTreeMap<String, ()> = required().into_iter().map(|p| (p, ())).collect();
    assert_eq!(check_whole(&files), Ok(()));
    files.remove("addon/Apex Neural Rendering FG/dlssg/nvngx_dlssg.dll");
    assert!(check_whole(&files).unwrap_err().contains("addon/"));
}

#[test]
fn switches_have_stable_ids() {
    assert_eq!(Switch::FrameGeneration.id(), "frame-generation");
    assert_eq!(
        Switch::from_id("frame-generation"),
        Some(Switch::FrameGeneration)
    );
    assert_eq!(Switch::from_id("upscaling"), None);
    assert_eq!(
        serde_json::to_value(Switch::FrameGeneration).unwrap(),
        json!("frame-generation")
    );
}

#[test]
fn cached_package_parsing_path_validation_and_pin_matching() {
    let package = Package {
        tag: PINNED.tag.into(),
        archives: [
            (PINNED.full.asset.into(), PINNED.full.sha256.into()),
            (PINNED.addon.asset.into(), PINNED.addon.sha256.into()),
        ]
        .into(),
        files: [(
            "full/game/dxgi.dll".into(),
            Placed {
                sha256: hash("dll"),
                size: 3,
            },
        )]
        .into(),
    };
    let text = serde_json::to_string(&package).unwrap();
    let parsed = Package::parse(&text, "cache/FG-V.B16").unwrap();
    assert_eq!(parsed, package);
    assert!(serves(&parsed, &PINNED));

    let cached = parsed.clone().with(PathBuf::from("cache/FG-V.B16"));
    assert_eq!(cached.folder, PathBuf::from("cache/FG-V.B16"));
    assert_eq!(cached.package, package);

    let mut wrong_hash = package.clone();
    wrong_hash
        .archives
        .insert(PINNED.addon.asset.into(), "0".repeat(64));
    assert!(!serves(&wrong_hash, &PINNED));

    let mut escaped = package;
    escaped.files.insert(
        "../outside.dll".into(),
        Placed {
            sha256: hash("bad"),
            size: 3,
        },
    );
    let err = Package::parse(&serde_json::to_string(&escaped).unwrap(), "cache/bad").unwrap_err();
    assert!(err.contains("names a file outside it"), "{err}");
    assert!(
        Package::parse("{", "cache/bad")
            .unwrap_err()
            .contains("is damaged")
    );
}

#[test]
fn verify_download_enforces_exact_size_and_digest() {
    let download = Download {
        asset: PINNED.full.asset.into(),
        url: "https://example.invalid/full.zip".into(),
        digest: hash("archive"),
        size: 7,
    };
    assert_eq!(verify_download(7, &hash("archive"), &download), Ok(()));
    assert!(
        verify_download(6, &hash("archive"), &download)
            .unwrap_err()
            .contains("6 bytes")
    );
    assert!(
        verify_download(7, &hash("other"), &download)
            .unwrap_err()
            .contains("SHA-256 digest")
    );
}
