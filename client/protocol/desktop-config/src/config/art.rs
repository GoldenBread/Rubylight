//! Cached directory index of game cover art files to prune superseded versions.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The cover files of the art folders, by app. A folder is read once, when the first cover is
/// saved into it, and kept up from then on: filling a library saves a cover per game, and reading
/// the folder for each of them reads every cover saved so far again.
#[derive(Default)]
pub struct ArtFiles {
    folders: HashMap<PathBuf, HashMap<i32, Vec<PathBuf>>>,
    #[cfg(test)]
    pub(super) folders_read: usize,
}

impl ArtFiles {
    /// After a cover is saved at `keep` (`<app>.img` or `<app>-<version>.img`), removes the other
    /// files of the same app in that folder: older versions of the cover are never asked for again.
    pub fn saved(&mut self, keep: &Path, app_id: i32) {
        let Some(dir) = keep.parent() else { return };
        if !self.folders.contains_key(dir) {
            self.folders.insert(dir.to_owned(), art_files(dir));
            #[cfg(test)]
            {
                self.folders_read += 1;
            }
        }
        let Some(apps) = self.folders.get_mut(dir) else { return };
        let files = apps.entry(app_id).or_default();
        for old in files.iter().filter(|old| *old != keep) {
            let _ = std::fs::remove_file(old);
        }
        *files = vec![keep.to_owned()];
    }
}

/// The cover files in `dir`, by the app each is of.
fn art_files(dir: &Path) -> HashMap<i32, Vec<PathBuf>> {
    let mut apps: HashMap<i32, Vec<PathBuf>> = HashMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return apps };
    for entry in entries.flatten() {
        let path = entry.path();
        if let Some(app_id) = path.file_name().and_then(|n| n.to_str()).and_then(art_app) {
            apps.entry(app_id).or_default().push(path);
        }
    }
    apps
}

/// The app a cover file is of, by its name: `<app>.img` or `<app>-<version>.img` ([`super::Store::art_path`]).
pub(super) fn art_app(name: &str) -> Option<i32> {
    let stem = name.strip_suffix(".img")?;
    stem.parse().ok().or_else(|| stem.rsplit_once('-')?.0.parse().ok())
}
