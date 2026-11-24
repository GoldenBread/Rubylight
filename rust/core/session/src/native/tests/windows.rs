//! Tests of Windows-only behaviour: drive letters, backslashes, `.exe` names.

use super::*;

#[test]
fn a_game_folder_the_scan_no_longer_reads_has_an_unknown_installed_state() {
    let folder = |name: &str, id: &str| {
        app(json!({"name": name, "uuid": name, "launcher-source": "local", "launcher-id": id}))
    };
    let settings = crate::library::Settings {
        local_folders: vec!["D:/Games/".into()],
        local_exclude: vec!["Left Out".into()],
        ..Default::default()
    };
    let scan = Scan::new([r"local:d:\games\hades".into()]).reading(settings);
    let apps = [
        folder("Hades", r"d:\games\hades"),
        folder("Removed", r"d:\games\removed"),
        folder("Left out", r"d:\games\left out"),
        folder("Elsewhere", r"e:\old games\celeste"),
    ];
    let library = library(&apps, Some(&scan), None);
    let installed: Vec<_> = library.games.iter().map(|game| game.installed).collect();
    assert_eq!(installed, [Some(true), Some(false), None, None]);
}
