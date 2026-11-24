//! Local game folder executable heuristics and final library scan deduplication.
use super::{Game, Launch, Source, folder_key, name_key, within};
use std::path::{Component, Path, PathBuf};

/// A program below a game folder: its path relative to the folder, and its size.
pub struct Program {
    pub path: PathBuf,
    pub size: u64,
}
/// Folders below a game folder that never hold the game's own program.
pub const LOCAL_SKIPPED_FOLDERS: [&str; 16] = [
    "_commonredist",
    "commonredist",
    "redist",
    "redistributables",
    "_redist",
    "__installer",
    "installer",
    "installers",
    "directx",
    "dotnet",
    "vcredist",
    "prereqs",
    "support",
    "easyanticheat",
    "battleye",
    "crashreporter",
];
/// Whether a program (by file name) is part of a game's install, not the game: setup,
/// uninstallers, redistributables, crash reporters, updaters and anti-cheat services.
fn helper_program(file: &str) -> bool {
    let stem = file.to_lowercase();
    let stem = stem.strip_suffix(".exe").unwrap_or(&stem);
    [
        "unins",
        "setup",
        "install",
        "redist",
        "directx",
        "dotnet",
        "crashhandler",
        "crashpad",
        "crashsender",
        "crashdump",
        "bugreport",
        "errorreport",
        "reporter",
        "update",
        "patcher",
        "prereq",
        "easyanticheat",
        "battleye",
        "uploader",
        "helper",
        "cefprocess",
        "touchup",
        "cleanup",
        "activation",
        "config",
    ]
    .iter()
    .any(|part| stem.contains(part))
        || matches!(stem, "eac" | "be" | "be_service" | "dxsetup")
}
/// The game in `folder`, a subfolder of one of the configured game folders, started by
/// the program `programs` lists below it (at most two folders deep). Helpers
/// ([`helper_program`]) and anything in [`LOCAL_SKIPPED_FOLDERS`] are never it. The program
/// nearest the top wins, then one named like the folder, then the largest. The game is
/// known by its folder, so a newer program does not make it a new game.
pub fn local(folder: &Path, programs: &[Program]) -> Option<Game> {
    let name = folder.file_name()?.to_string_lossy().trim().to_owned();
    let title = name_key(&name);
    let program = programs
        .iter()
        .filter(|program| {
            let mut parts = program.path.components().rev();
            let file = parts.next().map(|part| part.as_os_str().to_string_lossy());
            file.is_some_and(|file| file.to_lowercase().ends_with(".exe") && !helper_program(&file))
                && program.path.components().count() <= 3
                && parts.all(|part| {
                    matches!(part, Component::Normal(_))
                        && !LOCAL_SKIPPED_FOLDERS
                            .contains(&part.as_os_str().to_string_lossy().to_lowercase().as_str())
                })
        })
        .min_by_key(|program| {
            let stem = name_key(
                &program
                    .path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy(),
            );
            let named = stem.len() >= 3 && (title.contains(&stem) || stem.contains(&title));
            (
                program.path.components().count(),
                !named,
                std::cmp::Reverse(program.size),
                program.path.clone(),
            )
        })?;
    let program = folder.join(program.path.components().collect::<PathBuf>());
    let mut game = Game::new(
        Source::Local,
        folder_key(&folder.to_string_lossy()),
        name,
        folder,
        Some(Launch::Program {
            program: program.to_string_lossy().into_owned(),
            arguments: String::new(),
        }),
    );
    if let Some(parent) = program.parent() {
        game.working_dir = parent.into();
    }
    Some(game)
}

/// The scan's games once every source is read, sorted by name: one per launcher and id,
/// and no game-folder entry for a folder a launcher already lists or one that holds a
/// launcher's games (a library, not a game). A launcher's folder that holds game folders
/// (a record pointing at `D:\Games`) hides none of them.
pub fn finish(mut games: Vec<Game>) -> Vec<Game> {
    let mut seen = std::collections::BTreeSet::new();
    games.retain(|game| seen.insert((game.source, game.id.clone())));
    let launcher_folders: Vec<String> = games
        .iter()
        .filter(|game| game.source != Source::Local)
        .map(|game| game.install_dir.clone())
        .collect();
    games.retain(|game| {
        game.source != Source::Local
            || !launcher_folders
                .iter()
                .any(|folder| within(folder, &game.install_dir))
    });
    games.sort_by_cached_key(|game| (game.name.to_lowercase(), game.source, game.id.clone()));
    games
}
