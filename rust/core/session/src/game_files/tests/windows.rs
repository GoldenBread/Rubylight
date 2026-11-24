//! Tests of Windows-only behaviour: drive letters, backslashes, `.exe` names.

use super::*;

#[test]
fn files_resolve_only_inside_the_apps_folders() {
    let game = game();
    let folders = folders([game.folder.clone()]);
    assert_eq!(folders, [std::fs::canonicalize(&game.folder).unwrap()]);
    let resolve = |path: &str| resolve(path, Some(&game.folder), &folders);
    assert_eq!(resolve("OptiScaler.ini").unwrap(), game.resolved);
    assert_eq!(
        resolve(&game.file.to_string_lossy()).unwrap(),
        game.resolved
    );
    assert!(matches!(resolve("missing.ini"), Err(Unusable::Missing(_))));
    assert!(matches!(
        self::resolve("OptiScaler.ini", None, &folders),
        Err(Unusable::Missing(_))
    ));
    // A file beside the folder, named with its full path, is outside it.
    let outside = game.folder.parent().unwrap().join("outside.ini");
    std::fs::write(&outside, "[A]\nB=1\n").unwrap();
    assert!(matches!(
        resolve(&outside.to_string_lossy()),
        Err(Unusable::Refused(_))
    ));
    assert!(matches!(
        resolve(r"..\outside.ini"),
        Err(Unusable::Refused(_))
    ));
    // A folder named .ini is not a file.
    std::fs::create_dir(game.folder.join("dir.ini")).unwrap();
    assert!(matches!(resolve("dir.ini"), Err(Unusable::Refused(_))));
    let big = game.folder.join("big.ini");
    std::fs::write(&big, vec![b'a'; MAX_FILE_BYTES as usize + 1]).unwrap();
    assert!(matches!(resolve("big.ini"), Err(Unusable::Refused(_))));
    // A drive's root and the Windows folder are never an app's folder.
    let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    assert!(self::folders([PathBuf::from(r"C:\"), PathBuf::from(&windows)]).is_empty());
    assert!(self::folders([PathBuf::from("relative")]).is_empty());
}
