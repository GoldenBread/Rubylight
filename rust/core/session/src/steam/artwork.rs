//! Local library-cache artwork lookup and Steam CDN store portrait URLs.
use std::path::{Path, PathBuf};

type Artwork = (Option<PathBuf>, Option<PathBuf>, Option<PathBuf>);
/// Portrait, header and icon from Steam's library cache, or the user's
/// custom grid artwork.
pub fn artwork(appid: u32, library: &Path, roots: &[PathBuf]) -> Artwork {
    let mut caches = vec![];
    let mut grids = vec![];
    for root in std::iter::once(library).chain(roots.iter().map(PathBuf::as_path)) {
        caches.push(root.join("appcache").join("librarycache"));
        caches.push(root.join("librarycache"));
        if let Ok(users) = std::fs::read_dir(root.join("userdata")) {
            let mut users: Vec<_> = users.flatten().map(|u| u.path()).collect();
            users.sort();
            grids.extend(users.into_iter().map(|u| u.join("config").join("grid")));
        }
    }
    let id = appid.to_string();
    let flat = |dirs: &[PathBuf], suffixes: &[String]| {
        dirs.iter()
            .flat_map(|dir| suffixes.iter().map(|s| dir.join(format!("{id}{s}"))))
            .find(|path| path.is_file())
    };
    let nested = |names: &[String]| {
        caches
            .iter()
            .flat_map(|dir| names.iter().map(|n| dir.join(&id).join(n)))
            .find(|path| path.is_file())
    };
    // Current clients keep store assets under <appid>/<content hash>/.
    let hashed = |names: &[String]| {
        caches.iter().find_map(|dir| {
            let mut folders: Vec<_> = std::fs::read_dir(dir.join(&id))
                .ok()?
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect();
            folders.sort();
            folders
                .iter()
                .flat_map(|folder| names.iter().map(|n| folder.join(n)))
                .find(|path| path.is_file())
        })
    };
    let images = |stem: &str| ["jpg", "png", "webp"].map(|ext| format!("{stem}.{ext}"));
    let grid = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>();
    let portrait = flat(&caches, &images("_library_600x900_2x"))
        .or_else(|| nested(&images("library_600x900_2x")))
        .or_else(|| flat(&caches, &images("_library_600x900")))
        .or_else(|| nested(&images("library_600x900")))
        .or_else(|| {
            // Older hashed folders hold library_capsule, newer apps library_600x900.
            let mut names = images("library_capsule").to_vec();
            names.extend(images("library_600x900"));
            hashed(&names)
        })
        .or_else(|| flat(&grids, &grid(&["p.png", "p.jpg", "_p.png", "_p.jpg"])));
    let header = flat(&caches, &images("_header"))
        .or_else(|| nested(&images("header")))
        .or_else(|| hashed(&images("library_header")))
        .or_else(|| flat(&grids, &grid(&["_hero.png", "_hero.jpg"])));
    let icon = flat(&caches, &images("_icon"))
        .or_else(|| nested(&images("icon")))
        .or_else(|| flat(&grids, &grid(&["_icon.png", "_icon.jpg", ".png"])));
    (portrait, header, icon)
}

/// Steam's store portrait at its fixed CDN path. Newer apps publish it only
/// under the content-hash folder, see [`hashed_store_portrait_url`].
pub fn store_portrait_url(appid: u32) -> String {
    format!(
        "https://shared.fastly.steamstatic.com/store_item_assets/steam/apps/{appid}/library_600x900_2x.jpg"
    )
}
/// The CDN's 2x portrait in the same content-hash folder as a cached
/// `<appid>/<40 hex>/library_600x900.*` or `library_capsule.*`, as Steam
/// serves it for apps without a fixed-path portrait.
pub fn hashed_store_portrait_url(appid: u32, local: &Path) -> Option<String> {
    let hash = local.parent()?.file_name()?.to_str()?;
    let stem = local.file_stem()?.to_str()?;
    let hex = hash.len() == 40 && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    (hex && matches!(stem, "library_600x900" | "library_capsule")).then(|| {
        format!(
            "https://shared.fastly.steamstatic.com/store_item_assets/steam/apps/{appid}/{hash}/{stem}_2x.jpg"
        )
    })
}
