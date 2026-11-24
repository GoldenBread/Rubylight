//! Tests for Microsoft Store and Xbox PC package parsing and AUMID checks.
use super::*;
use anyhow::Result;
use std::path::Path;

const FAMILY: &str = "Contoso.Racing_8wekyb3d8bbwe";
fn manifest(application: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
         xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10">
  <Identity Name="Contoso.Racing" Publisher="CN=Contoso" Version="1.4.0.0" />
  <Applications>{application}</Applications>
</Package>"#
    )
}
fn racing(package_manifest: &str, display_name: &str) -> Result<Option<Game>> {
    microsoft(
        MICROSOFT_CONFIG,
        Path::new(r"C:\XboxGames\Contoso Racing\Content"),
        Some(&StorePackage {
            family: FAMILY,
            display_name,
            manifest: package_manifest,
        }),
    )
}

#[test]
fn a_microsoft_game_starts_through_its_application_id_and_ignores_console_programs() {
    let installed = manifest(
        r#"<Application Id="Console" Executable="Launcher.exe" EntryPoint="Windows.FullTrustApplication" />
           <Application Id="Game" Executable="Racing_Shipping.exe" EntryPoint="Windows.FullTrustApplication" />"#,
    );
    let game = racing(&installed, "ms-resource:AppName").unwrap().unwrap();
    assert_eq!(game.source, Source::Microsoft);
    assert_eq!(game.id, "Contoso.Racing");
    assert_eq!(game.name, "Contoso Racing: Étoile");
    assert_eq!(game.unavailable_reason, None);
    // The PC program's Id, not the Xbox one that comes first in the manifest.
    assert_eq!(
        game.launch.as_ref().unwrap().command(),
        r"shell:AppsFolder\Contoso.Racing_8wekyb3d8bbwe!Game"
    );
    assert_eq!(game.install_dir, r"C:\XboxGames\Contoso Racing\Content");

    // It imports like any other source, keyed by the identity name.
    let mut apps = vec![];
    assert_eq!(
        import(
            &mut apps,
            &[game],
            &[Selection {
                source: Source::Microsoft,
                id: "Contoso.Racing".into()
            }]
        )[0]
        .result,
        "added"
    );
    assert_eq!(apps[0]["launcher-source"], "microsoft");
    assert_eq!(
        apps[0]["cmd"],
        r"shell:AppsFolder\Contoso.Racing_8wekyb3d8bbwe!Game"
    );
}

#[test]
fn the_program_name_finds_the_application_when_the_ids_differ() {
    let installed = manifest(
        r#"<Application Id="App" Executable="bin\Racing_Shipping.exe" EntryPoint="Windows.FullTrustApplication" />"#,
    );
    let game = racing(&installed, "").unwrap().unwrap();
    assert_eq!(
        game.launch.unwrap().command(),
        r"shell:AppsFolder\Contoso.Racing_8wekyb3d8bbwe!App"
    );
}

#[test]
fn a_microsoft_game_nothing_in_the_package_starts_is_an_error_not_a_guess() {
    let installed = manifest(
        r#"<Application Id="Other" Executable="Other.exe" EntryPoint="Windows.FullTrustApplication" />"#,
    );
    let error = racing(&installed, "").err().unwrap().to_string();
    assert!(error.contains("Racing_Shipping.exe"), "{error}");
    // Two launch helpers: neither is known to be the game's.
    let helpers = manifest(
        r#"<Application Id="A" Executable="GameLaunchHelper.exe" />
           <Application Id="B" Executable="GameLaunchHelper.exe" />"#,
    );
    assert!(racing(&helpers, "").is_err());
}

/// j-scrizz's 3.4.0 scan: Xbox PC games and their packages listed as errors.
#[test]
fn an_xbox_pc_game_starts_through_its_launch_helper_and_a_package_without_applications_is_none() {
    let installed = manifest(
        r#"<Application Id="Racing" Executable="GameLaunchHelper.exe" EntryPoint="Windows.FullTrustApplication" />"#,
    );
    let game = racing(&installed, "").unwrap().unwrap();
    assert_eq!(
        game.launch.unwrap().command(),
        r"shell:AppsFolder\Contoso.Racing_8wekyb3d8bbwe!Racing"
    );
    // Nothing to start, as a DLC's package: not a game, not an error.
    assert!(racing(&manifest(""), "").unwrap().is_none());
    let bare = r#"<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"><Identity Name="Contoso.Racing" /></Package>"#;
    assert!(racing(bare, "").unwrap().is_none());
}

#[test]
fn a_package_or_application_name_with_odd_characters_never_becomes_a_launch() {
    let hostile =
        manifest(r#"<Application Id="Game&amp;calc" Executable="Racing_Shipping.exe" />"#);
    assert!(racing(&hostile, "").is_err());
    for family in [
        "NoUnderscore",
        "Name_Pub lisher",
        r"a\b_c",
        "_pub",
        "name_",
        "a_b_c",
    ] {
        let installed = manifest(r#"<Application Id="Game" Executable="Racing_Shipping.exe" />"#);
        let package = StorePackage {
            family,
            display_name: "",
            manifest: &installed,
        };
        assert!(
            microsoft(MICROSOFT_CONFIG, Path::new("C:/g"), Some(&package)).is_err(),
            "{family}"
        );
    }
}

#[test]
fn a_console_only_or_programless_config_is_not_a_pc_game() {
    let xbox_only =
        MICROSOFT_CONFIG.replace(r#"TargetDeviceFamily="PC""#, r#"TargetDeviceFamily="Xbox""#);
    assert!(
        microsoft(&xbox_only, Path::new("C:/g"), None)
            .unwrap()
            .is_none()
    );
    let empty = r#"<Game><Identity Name="Contoso.Empty" /></Game>"#;
    assert!(microsoft(empty, Path::new("C:/g"), None).unwrap().is_none());
    // A program with no family listed is for PCs.
    let bare = r#"<Game><Identity Name="Contoso.Bare" /><ExecutableList><Executable Name="bare.exe" Id="Game" /></ExecutableList></Game>"#;
    assert_eq!(
        microsoft(bare, Path::new("C:/g"), None)
            .unwrap()
            .unwrap()
            .name,
        "Contoso.Bare"
    );
    assert!(microsoft("<Game><not-closed>", Path::new("C:/g"), None).is_err());
}

#[test]
fn a_game_with_no_registered_package_is_listed_but_cannot_start() {
    let game = microsoft(
        MICROSOFT_CONFIG,
        Path::new("D:/XboxGames/Racing/Content"),
        None,
    )
    .unwrap()
    .unwrap();
    assert!(game.launch.is_none());
    let reason = game.unavailable_reason.as_deref().unwrap();
    assert!(
        reason.contains("not installed for this Windows user"),
        "{reason}"
    );
    assert!(
        import(
            &mut vec![],
            &[game],
            &[Selection {
                source: Source::Microsoft,
                id: "Contoso.Racing".into()
            }]
        )[0]
        .error
        .is_some()
    );
}

#[test]
fn the_cover_is_the_config_logo_when_the_file_is_there() {
    let folder = tempfile::tempdir().unwrap();
    let package = manifest(r#"<Application Id="Game" Executable="Racing_Shipping.exe" />"#);
    let with = |folder: &Path| {
        microsoft(
            MICROSOFT_CONFIG,
            folder,
            Some(&StorePackage {
                family: FAMILY,
                display_name: "",
                manifest: &package,
            }),
        )
        .unwrap()
        .unwrap()
    };
    assert_eq!(with(folder.path()).cover, None);
    std::fs::create_dir(folder.path().join("Assets")).unwrap();
    std::fs::write(folder.path().join("Assets").join("Logo150.png"), b"png").unwrap();
    assert_eq!(
        with(folder.path()).cover,
        Some(folder.path().join(r"Assets\Logo150.png"))
    );
}

#[test]
fn microsoft_is_a_library_source_with_its_own_key() {
    assert_eq!(Source::ALL.len(), 8);
    assert_eq!(Source::Microsoft.key(), "microsoft");
    assert_eq!(
        serde_json::to_value(Source::Microsoft).unwrap(),
        "microsoft"
    );
}

#[test]
fn only_a_plain_application_id_is_a_store_launch_target() {
    assert_eq!(
        store_launch_target(r"shell:AppsFolder\Contoso.Racing_8wekyb3d8bbwe!Game"),
        Some("Contoso.Racing_8wekyb3d8bbwe!Game")
    );
    for uri in [
        "steam://rungameid/570",
        r"shell:AppsFolder\Contoso.Racing_8wekyb3d8bbwe!Game & calc",
        r"shell:AppsFolder\..\Windows\System32\cmd.exe",
        r"shell:AppsFolder\Contoso.Racing_8wekyb3d8bbwe",
        r"shell:AppsFolder\Contoso.Racing_8wekyb3d8bbwe!",
        r"shell:Startup",
        r"shell:AppsFolder\{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\cmd.exe",
    ] {
        assert_eq!(store_launch_target(uri), None, "{uri}");
    }
}
