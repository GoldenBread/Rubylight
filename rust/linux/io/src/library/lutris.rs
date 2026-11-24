//! Lutris' library, read from its `pga.db` (SQLite) without starting Lutris or writing
//! to the file. Lutris' launch address is `lutris:rungameid/<id>`.

use super::sqlite::{self, Row};
use anyhow::{Context, Result};
use butterpollo_session::library::{Game, Launch, Source};
use std::path::{Path, PathBuf};

/// The largest `pga.db` read; a real one is a few hundred kilobytes.
const MAX_BYTES: u64 = 256 << 20;

/// Lutris' data folders on this PC: `$XDG_DATA_HOME/lutris`, `~/.local/share/lutris` and
/// the Flatpak's.
pub fn roots() -> Vec<PathBuf> {
    let mut candidates = vec![];
    if let Some(data) = std::env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
        candidates.push(PathBuf::from(data).join("lutris"));
    }
    if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        let home = PathBuf::from(home);
        candidates.push(home.join(".local/share/lutris"));
        candidates.push(home.join(".var/app/net.lutris.Lutris/data/lutris"));
    }
    let mut roots: Vec<PathBuf> = vec![];
    for candidate in candidates {
        if let Ok(path) = std::fs::canonicalize(&candidate)
            && path.is_absolute()
            && path.join("pga.db").is_file()
            && !roots.contains(&path)
        {
            roots.push(path);
        }
    }
    roots
}

/// The installed games of the Lutris data folder `root`, except the ones that start through
/// Steam (Steam lists those), hidden games and entries that are not installed.
pub fn games(root: &Path) -> Result<Vec<Game>> {
    let path = root.join("pga.db");
    let length = std::fs::metadata(&path)
        .with_context(|| path.display().to_string())?
        .len();
    anyhow::ensure!(length <= MAX_BYTES, "{} is too large", path.display());
    let bytes = std::fs::read(&path).with_context(|| path.display().to_string())?;
    let rows = sqlite::rows(&bytes, "games").with_context(|| path.display().to_string())?;
    Ok(rows.iter().filter_map(|row| game(root, row)).collect())
}

fn text<'a>(row: &'a Row, key: &str) -> &'a str {
    row.get(key)
        .and_then(sqlite::Value::text)
        .map_or("", str::trim)
}

fn game(root: &Path, row: &Row) -> Option<Game> {
    let number = |key: &str| row.get(key).and_then(sqlite::Value::int).unwrap_or(0);
    let id = number("id");
    let (name, slug) = (text(row, "name"), text(row, "slug"));
    if id <= 0
        || name.is_empty()
        || number("installed") != 1
        || number("hidden") == 1
        || text(row, "runner") == "steam"
        || text(row, "service") == "steam"
    {
        return None;
    }
    let directory = Path::new(text(row, "directory"));
    // Some games (emulated ones) have no folder of their own: they get a name below
    // Lutris' data folder that no other game shares and that holds nothing.
    let folder = if directory.is_absolute() && directory.is_dir() {
        directory.to_path_buf()
    } else {
        root.join("games").join(if slug.is_empty() {
            id.to_string()
        } else {
            slug.to_owned()
        })
    };
    let mut game = Game::new(
        Source::Local,
        format!("lutris:{id}"),
        name.to_owned(),
        &folder,
        Some(Launch::Uri {
            uri: format!("lutris:rungameid/{id}"),
        }),
    );
    game.cover = cover(root, slug);
    Some(game)
}

/// Lutris' cover art for a slug: the portrait cover, else the banner, else the icon.
fn cover(root: &Path, slug: &str) -> Option<PathBuf> {
    // The slug names a file below Lutris' folders: nothing that climbs out of them.
    if slug.is_empty() || slug.contains(['/', '\\']) || slug.starts_with('.') {
        return None;
    }
    let candidates = [
        ("coverart", slug.to_owned()),
        ("banners", slug.to_owned()),
        ("icons", format!("lutris_{slug}")),
    ];
    candidates.iter().find_map(|(folder, stem)| {
        ["jpg", "png", "webp"]
            .iter()
            .map(|extension| root.join(folder).join(format!("{stem}.{extension}")))
            .find(|path| path.is_file())
    })
}
