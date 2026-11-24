//! Tests for installed launcher records, library imports, local folders, and sync.
use super::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

mod index;
mod local;
mod microsoft;
mod stores;
mod sync;

const EPIC_ITEM: &[u8] = include_bytes!("fixtures/epic.item");
const EA_XML: &str = include_str!("fixtures/ea.xml");
const PRODUCT_DB: &[u8] = include_bytes!("fixtures/product.db");
const BATTLENET_JSON: &str = include_str!("fixtures/battlenet.json");
const PRODUCT_FENRIS_DB: &[u8] = include_bytes!("fixtures/product_fenris.db");
const UBISOFT_JSON: &str = include_str!("fixtures/ubisoft.json");
const GOG_INFO: &[u8] = include_bytes!("fixtures/gog.info");

fn selected(game: &Game) -> Selection {
    Selection {
        source: game.source,
        id: game.id.clone(),
    }
}
fn epic_game() -> Game {
    epic(include_bytes!("fixtures/epic.item")).unwrap().unwrap()
}
const MICROSOFT_CONFIG: &str = include_str!("fixtures/microsoft_game.config");
fn gog_registry_values() -> BTreeMap<String, String> {
    serde_json::from_str(include_str!("fixtures/gog_registry.json")).unwrap()
}
/// The game folders in `fixtures/local_folders.json`, as the folder scan lists them.
fn local_folders() -> Vec<(PathBuf, Vec<Program>)> {
    let folders: BTreeMap<String, Vec<(String, u64)>> =
        serde_json::from_str(include_str!("fixtures/local_folders.json")).unwrap();
    folders
        .into_iter()
        .map(|(name, files)| {
            (
                Path::new(r"D:\Games").join(name),
                files
                    .into_iter()
                    .map(|(path, size)| Program {
                        path: path.into(),
                        size,
                    })
                    .collect(),
            )
        })
        .collect()
}
fn local_settings() -> Settings {
    Settings::from_config(
        &butterpollo_config::config::Config::parse(
            r#"library_local_folders = ["D:\\Games", "E:/More games"]
library_local_exclude = ["portal clone", "F:\\Old\\Thing"]
library_gog_enabled = false
"#,
        )
        .unwrap(),
    )
}
/// Celeste, Hades and The Witcher 3, in that order.
fn local_games() -> Vec<Game> {
    let settings = local_settings();
    local_folders()
        .into_iter()
        .filter(|(folder, _)| !settings.excluded(folder))
        .filter_map(|(folder, programs)| local(&folder, &programs))
        .collect()
}
fn sync_settings(extra: &str) -> Settings {
    Settings::from_config(&butterpollo_config::config::Config::parse(extra).unwrap())
}
fn mirage() -> Game {
    ubisoft("13504", Path::new("F:/Games/Mirage"), "Mirage").unwrap()
}
