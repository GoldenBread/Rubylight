//! Battle.net Agent product.db decoding, registry merging, and play history.
use super::{Game, Launch, Source};
use anyhow::{Context, Result};
use prost::Message;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// Agent product.db fields: repeated installations (1), UID (1), product (2),
// installation settings (3), and the settings' absolute installation path (1).
#[derive(Clone, PartialEq, Message)]
pub(super) struct BattleDatabase {
    #[prost(message, repeated, tag = "1")]
    pub(super) games: Vec<BattleInstall>,
}
#[derive(Clone, PartialEq, Message)]
pub(super) struct BattleInstall {
    #[prost(string, tag = "1")]
    pub(super) uid: String,
    #[prost(string, tag = "2")]
    pub(super) product: String,
    #[prost(message, optional, tag = "3")]
    pub(super) settings: Option<BattleSettings>,
}
#[derive(Clone, PartialEq, Message)]
pub(super) struct BattleSettings {
    #[prost(string, tag = "1")]
    pub(super) path: String,
}
/// Each installed product in the Battle.net Agent's product.db: its uid, its product code and
/// its install folder. Either name may be empty.
pub fn battlenet_database(bytes: &[u8]) -> Result<Vec<(String, String, PathBuf)>> {
    Ok(BattleDatabase::decode(bytes)?
        .games
        .into_iter()
        .filter_map(|game| {
            let settings = game.settings?;
            if settings.path.is_empty() {
                return None;
            }
            Some((game.uid, game.product, PathBuf::from(settings.path)))
        })
        .collect())
}
/// The uid and product code (`--uid=`, `--productcode=`) a Battle.net uninstall command
/// names, either possibly empty; none when it names neither.
pub fn battlenet_uninstall(command: &str) -> Option<(String, String)> {
    let lower = command.to_ascii_lowercase();
    let argument = |name: &str| {
        let at = lower.find(name)? + name.len();
        let value = command[at..].trim_start_matches('"');
        value.split(['"', ' ']).next().map(str::to_owned)
    };
    let uid = argument("--uid=").unwrap_or_default();
    let product = argument("--productcode=").unwrap_or_default();
    (!uid.is_empty() || !product.is_empty()).then_some((uid, product))
}
/// The Battle.net products Rubylight can start: the product code `--exec="launch <code>"`
/// takes, the name, and the uids the Agent may record it under besides the code. Current
/// Battle.net writes long uids such as `fenris` beside the code `fen`. All match in any case.
const BATTLENET_PRODUCTS: &[(&str, &str, &[&str])] = &[
    ("WoW", "World of Warcraft", &[]),
    ("D3", "Diablo III", &["diablo3"]),
    ("S2", "StarCraft II", &[]),
    ("S1", "StarCraft", &[]),
    ("WTCG", "Hearthstone", &["hs_beta"]),
    ("Hero", "Heroes of the Storm", &["heroes"]),
    ("Pro", "Overwatch 2", &["prometheus"]),
    ("VIPR", "Call of Duty: Black Ops 4", &["viper"]),
    ("ODIN", "Call of Duty: Modern Warfare", &[]),
    ("W3", "Warcraft III: Reforged", &[]),
    (
        "LAZR",
        "Call of Duty: Modern Warfare 2 Campaign Remastered",
        &["lazarus"],
    ),
    ("ZEUS", "Call of Duty: Black Ops Cold War", &[]),
    ("WLBY", "Crash Bandicoot 4", &[]),
    ("OSI", "Diablo II: Resurrected", &[]),
    ("RTRO", "Blizzard Arcade Collection", &[]),
    ("FORE", "Call of Duty: Vanguard", &[]),
    ("ANBS", "Diablo Immortal", &[]),
    ("AUKS", "Call of Duty: Modern Warfare II", &[]),
    ("Fen", "Diablo IV", &["fenris"]),
    ("D1", "Diablo", &[]),
    ("W1R", "Warcraft: Remastered", &[]),
    ("W2R", "Warcraft II: Remastered", &[]),
    ("W1", "Warcraft: Orcs & Humans", &[]),
    ("W2", "Warcraft II: Battle.net Edition", &[]),
    ("GRY", "Warcraft Rumble", &["gryphon"]),
    ("ARIS", "Doom: The Dark Ages", &[]),
    ("SCOR", "Sea of Thieves", &["scorpio"]),
    ("ARK", "The Outer Worlds 2", &["arkansas"]),
    ("LBRA", "Tony Hawk's Pro Skater 3 + 4", &["libra"]),
    ("PNTA", "Call of Duty: Modern Warfare III", &["pinta"]),
    ("AQUA", "Avowed", &[]),
];
/// The known product a uid and product code name, looked up by the uid first, so a variant
/// with a uid of its own is never renamed by a product code it shares, then by the code: its
/// launch code and name.
fn battlenet_product(uid: &str, product: &str) -> Option<(&'static str, &'static str)> {
    [uid, product]
        .map(|key| key.trim().to_ascii_lowercase())
        .into_iter()
        .filter(|key| !key.is_empty())
        .find_map(|key| {
            BATTLENET_PRODUCTS
                .iter()
                .find(|(code, _, uids)| {
                    code.eq_ignore_ascii_case(&key) || uids.contains(&key.as_str())
                })
                .map(|&(code, name, _)| (code, name))
        })
}
/// The products product.db lists (`database`), then those only the uninstall registry names.
/// A registry entry already listed is dropped: one with the same uid, or one without a uid
/// whose product code, or the known product it names, is already listed. A game is listed
/// once, under the id product.db gives it, rather than again under its product code.
pub fn battlenet_entries(
    database: Vec<(String, String, PathBuf)>,
    registry: Vec<(String, String, PathBuf)>,
) -> Vec<(String, String, PathBuf)> {
    let mut entries = database;
    for entry in registry {
        let (uid, product, _) = &entry;
        let listed = entries.iter().any(|(listed_uid, listed_product, _)| {
            if !uid.trim().is_empty() {
                return uid.trim().eq_ignore_ascii_case(listed_uid.trim());
            }
            (!product.trim().is_empty()
                && product.trim().eq_ignore_ascii_case(listed_product.trim()))
                || battlenet_product("", product).is_some_and(|(code, _)| {
                    battlenet_product(listed_uid, listed_product)
                        .is_some_and(|(listed, _)| listed == code)
                })
        });
        if !listed {
            entries.push(entry);
        }
    }
    entries
}
/// The Battle.net product with this uid and product code (either may be empty), looked up
/// by the uid first, then by the code. Its id is the uid in lower case (the code without
/// one), the key `Battle.net.config` keeps its play history under. A product Rubylight does
/// not know is named after its install folder and listed as unavailable; the Battle.net app
/// itself is skipped.
pub fn battlenet(uid: &str, product: &str, folder: &Path, launcher: Option<&Path>) -> Option<Game> {
    if [uid, product].iter().any(|key| {
        matches!(
            key.trim().to_ascii_lowercase().as_str(),
            "bna" | "agent" | "battle.net"
        )
    }) {
        return None;
    }
    let known = battlenet_product(uid, product);
    let raw = if uid.trim().is_empty() { product } else { uid }.trim();
    let Some((code, name)) = known else {
        let name = folder
            .file_name()
            .map(|name| name.to_string_lossy().trim().to_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| raw.to_owned());
        let mut game = Game::new(Source::Battlenet, raw.into(), name, folder, None);
        game.unavailable_reason =
            Some("This Battle.net product is not recognized. Add it manually.".into());
        return Some(game);
    };
    Some(Game::new(
        Source::Battlenet,
        raw.to_ascii_lowercase(),
        name.into(),
        folder,
        launcher.map(|path| Launch::Program {
            program: path.to_string_lossy().into_owned(),
            arguments: format!("--exec=\"launch {code}\""),
        }),
    ))
}
/// When each Battle.net game was last played, from the user's `Battle.net.config`: JSON whose
/// `Games` object holds each product's `LastPlayed`, Unix seconds as text, by product uid in
/// lower case, as [`battlenet`] names a game. A product never played has none.
pub fn battlenet_last_played(bytes: &[u8]) -> Result<HashMap<String, u64>> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let config: Value = serde_json::from_slice(bytes).context("Battle.net.config is not JSON.")?;
    Ok(config["Games"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(uid, game)| {
            let at = match &game["LastPlayed"] {
                Value::String(text) => text.trim().parse().ok()?,
                Value::Number(number) => number.as_u64()?,
                _ => return None,
            };
            (at > 0).then(|| (uid.to_ascii_lowercase(), at))
        })
        .collect())
}
