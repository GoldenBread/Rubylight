//! Tests for Steam discovery, launch resolution, collections, and sync.
use super::collections::COLLECTIONS_FILE;
use super::*;
use std::path::{Path, PathBuf};

mod collections;
mod launch;
mod sync;

fn binary_entry(appid: u32, kv: &[u8]) -> Vec<u8> {
    let mut entry = appid.to_le_bytes().to_vec();
    entry.extend(((60 + kv.len()) as u32).to_le_bytes());
    entry.extend([0; 60]);
    entry.extend(kv);
    entry
}
/// Binary VDF in format 28: an object with its children, a text.
fn object(key: &str, children: &[Vec<u8>]) -> Vec<u8> {
    let mut out = vec![0x00];
    out.extend(key.as_bytes());
    out.push(0);
    children.iter().for_each(|child| out.extend(child));
    out.push(0x08);
    out
}
fn text(key: &str, value: &str) -> Vec<u8> {
    let mut out = vec![0x01];
    for part in [key, value] {
        out.extend(part.as_bytes());
        out.push(0);
    }
    out
}
fn option(key: &str, executable: &str, kind: &str, config: &[(&str, &str)]) -> Vec<u8> {
    let config: Vec<_> = config.iter().map(|(k, v)| text(k, v)).collect();
    object(
        key,
        &[
            text("executable", executable),
            text("type", kind),
            object("config", &config),
        ],
    )
}
/// A Steam folder with Black Flag (242050) installed and its launch
/// options in the app cache, Steam's order first.
fn steam_with_launch_options(options: &[Vec<u8>]) -> tempfile::TempDir {
    steam_with_appinfo(options, &[])
}
/// The same with `common` holding `common`.
fn steam_with_appinfo(options: &[Vec<u8>], common: &[Vec<u8>]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let apps = root.path().join("steamapps");
    let game = apps.join("common").join("Black Flag");
    std::fs::create_dir_all(game.join("bin")).unwrap();
    std::fs::write(game.join("bin").join("AC4BFSP.exe"), b"").unwrap();
    std::fs::write(
        apps.join("appmanifest_242050.acf"),
        "\"AppState\"\n{\n\t\"appid\"\t\t\"242050\"\n\t\"installdir\"\t\t\"Black Flag\"\n}\n",
    )
    .unwrap();
    let mut kv = object(
        "appinfo",
        &[
            object("common", common),
            object("config", &[object("launch", options)]),
        ],
    );
    kv.extend([0x08, 0x08]);
    let mut data = 0x0756_4428u32.to_le_bytes().to_vec();
    data.extend(1u32.to_le_bytes());
    data.extend(binary_entry(242050, &kv));
    data.extend(0u32.to_le_bytes());
    std::fs::create_dir_all(root.path().join("appcache")).unwrap();
    std::fs::write(root.path().join("appcache").join("appinfo.vdf"), data).unwrap();
    root
}
fn game(appid: u32, name: &str, last_played: u64) -> Game {
    Game {
        appid,
        name: name.into(),
        installed: true,
        last_played,
        install_dir: PathBuf::from(format!("C:/Steam/steamapps/common/{name}")),
        ..Default::default()
    }
}
const CLOUD_STORAGE: &str = include_str!("../../fixtures/steam-cloud-storage-namespace-1.json");

/// A Steam folder with an account whose collections file says `text`.
fn steam_with_collections(account: &str, text: &str) -> tempfile::TempDir {
    let steam = tempfile::tempdir().unwrap();
    write_collections(steam.path(), account, text);
    steam
}

fn write_collections(steam: &Path, account: &str, text: &str) {
    let folder = steam
        .join("userdata")
        .join(account)
        .join("config")
        .join("cloudstorage");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join(COLLECTIONS_FILE), text).unwrap();
}
