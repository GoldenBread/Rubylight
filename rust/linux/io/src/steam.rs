//! Where Steam is installed on this PC, on Linux: the folders Steam's own installer, the
//! distribution packages, Flatpak and Snap use, below the home folder of the user the host
//! runs as. No registry is read; a Steam folder elsewhere is not found yet.

use std::path::{Path, PathBuf};

/// The folders below `home` that hold a Steam installation, by how Steam was installed:
/// natively (with its two usual links), as a Flatpak, and as a Snap.
const BELOW_HOME: [&str; 6] = [
    ".local/share/Steam",
    ".steam/steam",
    ".steam/root",
    ".var/app/com.valvesoftware.Steam/.local/share/Steam",
    ".var/app/com.valvesoftware.Steam/data/Steam",
    "snap/steam/common/.local/share/Steam",
];

/// Steam installations of the user the host runs as: `$XDG_DATA_HOME/Steam`, then the usual
/// folders below `$HOME`. Each is listed once, by its real path, so the links Steam keeps
/// (`~/.steam/steam`) do not repeat the folder they point to.
pub fn roots() -> Vec<PathBuf> {
    let mut candidates = vec![];
    if let Some(data) = std::env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
        candidates.push(PathBuf::from(data).join("Steam"));
    }
    if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        candidates.extend(below_home(Path::new(&home)));
    }
    installations(candidates)
}

/// The folders below `home` where a Steam installation may be.
fn below_home(home: &Path) -> Vec<PathBuf> {
    BELOW_HOME.iter().map(|folder| home.join(folder)).collect()
}

/// The `candidates` that are Steam installations, each once, by its real path.
fn installations(candidates: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = vec![];
    for candidate in candidates {
        if !candidate.is_absolute() {
            continue;
        }
        let Ok(path) = std::fs::canonicalize(&candidate) else {
            continue;
        };
        // A folder Steam left behind holds neither its libraries nor its start script.
        let installation = path.join("steamapps").is_dir() || path.join("steam.sh").is_file();
        if installation && !roots.contains(&path) {
            roots.push(path);
        }
    }
    roots
}
