//! Leftover entry inspection for Neural Rendering game folder cleanup.
use std::path::Path;

/// What is still in the folders the host created (`folders`) once its
/// files are out: each file, link or other folder someone else put there,
/// by its path in the game folder (with `/`), and a folder of the host's
/// that is still there with nothing in it (a program holds it open). It
/// stays, and the removal names it, so the folder it holds is never a
/// surprise.
pub(super) fn left_in(folder: &Path, folders: &[String]) -> Vec<String> {
    let ours = |relative: &str| folders.iter().any(|f| f.eq_ignore_ascii_case(relative));
    let mut left = Vec::new();
    for relative in folders {
        let Ok(entries) = std::fs::read_dir(folder.join(relative)) else {
            continue;
        };
        // Whatever is in it names it: its own entries here, a folder of the
        // host's in it by its own.
        let mut entries = entries.flatten().peekable();
        if entries.peek().is_none() {
            left.push(relative.clone());
        }
        for entry in entries {
            let path = format!("{relative}/{}", entry.file_name().to_string_lossy());
            // A folder the host made is named by its own contents.
            let made = entry
                .file_type()
                .is_ok_and(|kind| kind.is_dir() && !kind.is_symlink())
                && ours(&path);
            if !made {
                left.push(path);
            }
        }
    }
    left
}
