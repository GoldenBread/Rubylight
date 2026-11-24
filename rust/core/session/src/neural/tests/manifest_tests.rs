//! Manifest parsing, placement target, removal planning, pending record,
//! app key transition, and mod-name redaction tests for Neural Rendering.
use super::*;
use crate::state::App;
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[test]
fn each_file_goes_where_its_mode_places_it() {
    let full = |path: &str| target(Mode::Full, path, "winmm.dll", "");
    assert_eq!(full("full/game/dxgi.dll").as_deref(), Some("winmm.dll"));
    assert_eq!(
        full("full/game/OptiScaler.ini").as_deref(),
        Some("OptiScaler.ini")
    );
    assert_eq!(
        full("full/game/OptiScaler/custom/Apex Neural Rendering FG/dlssg/x.nrx").as_deref(),
        Some("OptiScaler/custom/Apex Neural Rendering FG/dlssg/x.nrx")
    );
    assert_eq!(
        full("full/LICENSES/OptiScaler-GPL-3.0.txt").as_deref(),
        Some("OptiScaler/LICENSES/OptiScaler-GPL-3.0.txt")
    );
    assert_eq!(full("addon/Apex Neural Rendering FG/dlssg/x.dll"), None);
    let addon = |dir: &str| {
        target(
            Mode::Addon,
            "addon/Apex Neural Rendering FG/dlssg/nvngx_dlssg.dll",
            "",
            dir,
        )
    };
    assert_eq!(
        addon("").as_deref(),
        Some("custom/Apex Neural Rendering FG/dlssg/nvngx_dlssg.dll")
    );
    assert_eq!(
        addon("OptiScaler").as_deref(),
        Some("OptiScaler/custom/Apex Neural Rendering FG/dlssg/nvngx_dlssg.dll")
    );
    assert_eq!(target(Mode::Addon, "full/game/dxgi.dll", "", ""), None);
    assert!(settings("OptiScaler.ini"));
    assert!(settings(
        "OptiScaler/custom/Apex Neural Rendering FG/dlssg/nr_fg.ini"
    ));
    assert!(!settings(
        "OptiScaler/custom/Apex Neural Rendering FG/dlssg/path_table.json"
    ));
}

#[test]
fn archive_paths_stay_inside_the_game_folder() {
    assert_eq!(
        entry_path("OptiScaler/D3D12_OptiScaler/D3D12Core.dll").as_deref(),
        Some("OptiScaler/D3D12_OptiScaler/D3D12Core.dll")
    );
    assert_eq!(
        entry_path("Licenses\\LICENSE.txt").as_deref(),
        Some("Licenses/LICENSE.txt")
    );
    assert_eq!(entry_path("OptiScaler/").as_deref(), Some("OptiScaler"));
    for bad in [
        "",
        "/etc/x",
        "../x.dll",
        "a/../../x.dll",
        "C:/Windows/x.dll",
        "C:x.dll",
        "a/./b",
        "dxgi.dll:stream",
        "con.txt",
        "a/NUL",
        "COM1.dll",
        "x.dll.",
        "x ",
        "a//b",
        "a\u{0}b",
    ] {
        assert_eq!(entry_path(bad), None, "{bad:?}");
    }
}

fn placed(text: &str) -> Placed {
    Placed {
        sha256: hash(text),
        size: text.len() as u64,
    }
}

const SETTINGS_FG: &str = "OptiScaler/custom/Apex Neural Rendering FG/dlssg/nr_fg.ini";

fn manifest() -> Manifest {
    Manifest {
        by: PLACED_BY.into(),
        release: "FG-V.B16".into(),
        proxy: "dxgi.dll".into(),
        mode: Mode::Full,
        switches: vec![Switch::FrameGeneration],
        files: [
            ("dxgi.dll", placed("dll")),
            ("OptiScaler.ini", placed("ini")),
            (
                "OptiScaler/custom/Apex Neural Rendering FG/dlssg/nr_fg.ini",
                placed("fg ini"),
            ),
            (
                "OptiScaler/custom/Apex Neural Rendering FG/dlssg/nvngx_dlssg.dll",
                placed("fg"),
            ),
        ]
        .into_iter()
        .map(|(path, placed)| (path.to_string(), placed))
        .collect(),
        previous: BTreeMap::new(),
        folders: vec!["OptiScaler".into()],
        taken_over: Vec::new(),
        ini: None,
    }
}

#[test]
fn a_manifest_is_trusted_only_as_the_host_wrote_it() {
    let good = manifest();
    let text = serde_json::to_string(&good).unwrap();
    assert_eq!(Manifest::parse(&text), Ok(good.clone()));
    let mut other = good.clone();
    other.by = "Someone".into();
    assert!(Manifest::parse(&serde_json::to_string(&other).unwrap()).is_err());
    let mut outside = good.clone();
    outside
        .files
        .insert("../../Windows/System32/x.dll".into(), placed("x"));
    assert!(Manifest::parse(&serde_json::to_string(&outside).unwrap()).is_err());
    let mut folder = good.clone();
    folder.folders.push("C:/Windows".into());
    assert!(Manifest::parse(&serde_json::to_string(&folder).unwrap()).is_err());
    let mut ini = good.clone();
    ini.ini = Some(IniRecord {
        path: "../OptiScaler.ini".into(),
        before: BTreeMap::new(),
        section_added: false,
        break_added: false,
    });
    assert!(Manifest::parse(&serde_json::to_string(&ini).unwrap()).is_err());
    // Settings taken over: inside the folder, and never also placed.
    let mut found = good.clone();
    found.files.remove(SETTINGS_FG);
    found.taken_over.push(SETTINGS_FG.into());
    let text = serde_json::to_string(&found).unwrap();
    assert!(text.contains("\"taken_over\""), "{text}");
    assert_eq!(Manifest::parse(&text), Ok(found.clone()));
    let mut both = good.clone();
    both.taken_over.push(SETTINGS_FG.into());
    assert!(Manifest::parse(&serde_json::to_string(&both).unwrap()).is_err());
    let mut outside = found.clone();
    outside.taken_over.push("../nr_fg.ini".into());
    assert!(Manifest::parse(&serde_json::to_string(&outside).unwrap()).is_err());
    // Without any, the manifest is written as a release before this list
    // reads it.
    assert!(!serde_json::to_string(&good).unwrap().contains("taken_over"));
    // An update's earlier forms only of files it names.
    let mut updating = good.clone();
    updating
        .previous
        .insert("dxgi.dll".into(), placed("old dll"));
    assert_eq!(
        Manifest::parse(&serde_json::to_string(&updating).unwrap()),
        Ok(updating.clone())
    );
    updating
        .previous
        .insert("../../Windows/System32/x.dll".into(), placed("x"));
    assert!(Manifest::parse(&serde_json::to_string(&updating).unwrap()).is_err());
    let mut name = good;
    name.proxy = "kernel32.dll".into();
    assert!(Manifest::parse(&serde_json::to_string(&name).unwrap()).is_err());
    assert!(Manifest::parse("{").is_err());
}

/// Rubylight 3.0's AMDNR manifest, as it wrote it, still reads: its proxy
/// may be any of the seven names, and it has no mode, switches or ini.
#[test]
fn a_rubylight_3_0_amdnr_manifest_still_reads() {
    let text = format!(
        r#"{{
  "by": "Rubylight",
  "release": "Alpha0.3.5.8R2",
  "proxy": "winhttp.dll",
  "files": {{
    "winhttp.dll": {{"sha256": "{dll}", "size": 3}},
    "OptiScaler.ini": {{"sha256": "{ini}", "size": 3}},
    "LmxxfNrRuntime.pak": {{"sha256": "{pak}", "size": 3}}
  }},
  "folders": ["OptiScaler"]
}}"#,
        dll = hash("dll"),
        ini = hash("ini"),
        pak = hash("pak")
    );
    let legacy = Manifest::parse(&text).unwrap();
    assert_eq!(legacy.proxy, "winhttp.dll");
    assert_eq!(legacy.mode, Mode::Full);
    assert!(legacy.switches.is_empty() && legacy.ini.is_none() && legacy.taken_over.is_empty());
    let removal = removal(&legacy, |path| match path {
        "OptiScaler.ini" => Now::Changed,
        "winhttp.dll" => Now::Changed,
        _ => Now::Same,
    });
    assert_eq!(removal.delete, ["LmxxfNrRuntime.pak", "OptiScaler.ini"]);
    assert_eq!(removal.keep, ["winhttp.dll"]);
}

#[test]
fn removal_deletes_what_is_still_as_placed_and_keeps_what_changed() {
    let removal = removal(&manifest(), |path| match path {
        // Replaced by someone: no longer provably the mod.
        "dxgi.dll" => Now::Changed,
        // The menu saved its settings, and the user edited the provider's.
        "OptiScaler.ini" => Now::Changed,
        "OptiScaler/custom/Apex Neural Rendering FG/dlssg/nr_fg.ini" => Now::Changed,
        "OptiScaler/custom/Apex Neural Rendering FG/dlssg/nvngx_dlssg.dll" => Now::Same,
        _ => Now::Missing,
    });
    assert_eq!(
        removal.delete,
        [
            "OptiScaler/custom/Apex Neural Rendering FG/dlssg/nvngx_dlssg.dll",
            "OptiScaler.ini",
            "OptiScaler/custom/Apex Neural Rendering FG/dlssg/nr_fg.ini"
        ]
    );
    assert_eq!(removal.keep, ["dxgi.dll"]);
    // The DLLs go first, OptiScaler's proxy before the others: a game that
    // runs holds them, so its removal stops before OptiScaler.ini is gone.
    let all = super::removal(&manifest(), |_| Now::Same).delete;
    assert_eq!(all[0], "dxgi.dll", "{all:?}");
    let dlls = all.iter().take_while(|path| path.ends_with(".dll")).count();
    assert!(dlls >= 2, "{all:?}");
    assert!(
        all[dlls..].iter().all(|path| !path.ends_with(".dll")),
        "{all:?}"
    );
    assert!(
        all[dlls..].iter().any(|path| path == "OptiScaler.ini"),
        "{all:?}"
    );
    // Settings the host found and took over are the user's: never deleted,
    // named unless gone.
    let mut found = manifest();
    found.files.remove(SETTINGS_FG);
    found.taken_over = vec![SETTINGS_FG.into(), "OptiScaler/gone.ini".into()];
    let left = super::removal(&found, |path| match path {
        SETTINGS_FG => Now::Changed,
        "OptiScaler/gone.ini" => Now::Missing,
        _ => Now::Same,
    });
    assert!(!left.delete.iter().any(|path| path == SETTINGS_FG));
    assert_eq!(left.keep, [SETTINGS_FG]);
    assert!(
        logs("OptiScaler.log")
            && logs("lmxxf_backend.log")
            && logs("OptiScaler.previous-1.Game.exe.log")
    );
    assert!(!logs("Game.log") && !logs("OptiScaler.ini"));
    // What the provider writes in its dlssg folder: its logs and its copy
    // of the Vulkan loader; its settings and DLL are placed files.
    assert!(
        provider_files("nr_fg-20261114-101500.log")
            && provider_files("NR_FG-20261115-063120.LOG")
            && provider_files("anr-vkloader.dll")
            && provider_files("ANR-VKLoader.DLL")
    );
    assert!(
        !provider_files("nr_fg.ini")
            && !provider_files("nvngx_dlssg.dll")
            && !provider_files("vulkan-1.dll")
            && !provider_files("anr-vkloader.dll.bak")
            && !provider_files("user.txt")
    );
    assert!(
        provider_dlssg("OptiScaler/custom/Apex Neural Rendering FG/dlssg")
            && provider_dlssg("custom/apex neural rendering fg/DLSSG")
            && provider_dlssg("Apex Neural Rendering FG/dlssg")
    );
    assert!(
        !provider_dlssg("OptiScaler/custom/Apex Neural Rendering FG")
            && !provider_dlssg("OptiScaler/custom/Other FG/dlssg")
            && !provider_dlssg("Not Apex Neural Rendering FG/dlssg")
            && !provider_dlssg("dlssg")
    );
}

/// What reaches a console or a client never names the mod: a path through
/// its folder shows as its file's name.
#[test]
fn shown_text_names_no_mod() {
    let dll = format!("OptiScaler/custom/{PROVIDER}/dlssg/nvngx_dlssg.dll");
    assert_eq!(shown(&dll), "frame generation's nvngx_dlssg.dll");
    assert_eq!(
        shown(&format!(
            "{dll} changed since it was placed; nothing was removed."
        )),
        "frame generation's nvngx_dlssg.dll changed since it was placed; nothing was removed."
    );
    assert_eq!(
        shown(&format!(
            "custom/{PROVIDER} is already in the game's folder"
        )),
        "frame generation's folder is already in the game's folder"
    );
    assert_eq!(
        shown(&format!(
            r"cannot write C:\Games\X\OptiScaler\custom\{PROVIDER}\dlssg\nr_fg.ini: denied"
        )),
        "cannot write frame generation's nr_fg.ini: denied"
    );
    // A stop character of more than one byte before the path.
    for stop in ['\u{3000}', '\u{a0}'] {
        assert_eq!(
            shown(&format!(
                "missing{stop}D:\\G\\OptiScaler\\custom\\{PROVIDER}\\dlssg\\x.dll"
            )),
            format!("missing{stop}frame generation's x.dll")
        );
    }
    assert_eq!(shown("the game uses BattlEye"), "the game uses BattlEye");
    // A folder taken over with its name in another case, and text around
    // it that is not ASCII: the name is hidden all the same.
    let lower = PROVIDER.to_ascii_lowercase();
    let upper = PROVIDER.to_ascii_uppercase();
    assert_eq!(
        shown(&format!(
            "OptiScaler/custom/{lower}/dlssg/notes.txt, OptiScaler/custom/{upper}/dlssg ändern"
        )),
        "frame generation's notes.txt, frame generation's dlssg ändern"
    );
    assert_eq!(
        shown(&format!("Spiel\u{a0}custom/{lower} läuft")),
        "Spiel\u{a0}frame generation's folder läuft"
    );
}

#[test]
fn app_keys_pending_records_and_manifest_transitions_stay_pure() {
    let mut app: App = serde_json::from_value(json!({
        "name": "Game",
        "uuid": "g1",
        "steam-install-dir": " C:/Games/G ",
    }))
    .unwrap();
    assert_eq!(app_install_dir(&app), Some(PathBuf::from("C:/Games/G")));
    assert!(!app_has_files(&app));

    let value = app_key_value(
        Path::new("C:/Games/G/Game.exe"),
        "FG-V.B16",
        "dxgi.dll",
        Mode::Full,
    );
    app.extra.insert(KEY.into(), value.clone());
    app.extra.insert(
        LEGACY_KEY.into(),
        json!({"exe": "C:/Games/G/Old.exe", "release": "Alpha"}),
    );
    assert!(app_has_files(&app));
    assert_eq!(
        app_placed_exes(&app),
        vec![
            PathBuf::from("C:/Games/G/Game.exe"),
            PathBuf::from("C:/Games/G/Old.exe"),
        ]
    );

    let mut edited: App = serde_json::from_value(json!({
        "name": "Game",
        "uuid": "g1",
        KEY: {"exe": "C:/Evil.exe"}
    }))
    .unwrap();
    app_keep_key(Some(&app), &mut edited);
    assert_eq!(edited.extra.get(KEY), Some(&value));
    assert!(edited.extra.contains_key(LEGACY_KEY));

    let dropped = dropped_apps(&[app.clone()], &[]);
    assert_eq!(dropped.len(), 1);
    let mut pending = Vec::new();
    let added = Pending::of(&app);
    assert_eq!(added.len(), 2);
    merge_pending(&mut pending, &added);
    merge_pending(&mut pending, &added);
    assert_eq!(pending.len(), 2);
    retain_pending_not_done(&mut pending, &[PathBuf::from("C:/Games/G/Game.exe")]);
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].exe, PathBuf::from("C:/Games/G/Old.exe"));

    let m = manifest();
    let stale = stale_manifest_paths(&m, &[("dxgi.dll".into(), placed("dll"))]);
    assert_eq!(
        stale,
        vec!["OptiScaler/custom/Apex Neural Rendering FG/dlssg/nvngx_dlssg.dll"]
    );
    let compact = retained_manifest(m, &["dxgi.dll".into()], |f| f == "OptiScaler");
    assert_eq!(compact.files.len(), 1);
    assert!(compact.files.contains_key("dxgi.dll"));
    assert_eq!(compact.folders, vec!["OptiScaler"]);

    assert_eq!(
        prepare_file_check("OptiScaler.ini", Now::Changed, "G"),
        Ok(())
    );
    assert!(
        prepare_file_check("dxgi.dll", Now::Missing, "G")
            .unwrap_err()
            .contains("is missing")
    );
    assert!(
        in_the_way(&[format!("OptiScaler/custom/{PROVIDER}/dlssg/user.txt")])
            .contains("from an earlier install")
    );
    assert!(
        kept_text(
            &[format!(
                "OptiScaler/custom/{PROVIDER}/dlssg/nvngx_dlssg.dll"
            )],
            true
        )
        .contains("frame generation's nvngx_dlssg.dll")
    );
}
