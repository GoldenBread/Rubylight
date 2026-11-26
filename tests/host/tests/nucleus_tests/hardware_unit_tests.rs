//! Unit tests for Nucleus Co-op hardware-case cleanup, token checks, frame liveness, and cold-settings helpers.
use super::hardware_support::*;
use butterpollo_session::nucleus::elevation::Run;
use butterpollo_stream::nucleus::*;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[test]
fn a_hardware_case_streams_the_exe_or_the_steam_link_as_an_app_would() {
    let exe = case_app(Some(r"D:\games\BFME1\lotrbfme.exe"), None).unwrap();
    assert_eq!(exe.name, "lotrbfme");
    assert!(starts_exe(&exe));
    assert_eq!(executable(&exe), Some(r"D:\games\BFME1\lotrbfme.exe"));
    let link = case_app(None, Some(2155180)).unwrap();
    assert!(!starts_exe(&link));
    assert_eq!(
        butterpollo_session::steam::game_of(&serde_json::to_value(&link).unwrap()),
        Some(2155180)
    );
    assert!(case_app(None, None).is_err());
    assert!(case_app(Some("a.exe"), Some(1)).is_err());
}

#[test]
fn a_black_frame_is_told_from_a_picture() {
    let frame = |fill: &dyn Fn(usize) -> [u8; 4]| butterpollo_win_graphics::capture::Image {
        width: 64,
        height: 32,
        stride: 64 * 4 + 16,
        bytes: (0..(64 * 4 + 16) * 32)
            .map(|at| fill(at % (64 * 4 + 16) / 4)[at % 4])
            .collect(),
        captured: Instant::now(),
        pixel: butterpollo_win_graphics::capture::Pixel::Bgra8,
    };
    assert_eq!(lit(&frame(&|_| [8, 8, 8, 255])).unwrap(), 0.0);
    assert_eq!(lit(&frame(&|_| [0, 0, 200, 255])).unwrap(), 1.0);
    // The left half lit.
    let half = lit(&frame(&|x| if x < 32 { [90, 90, 90, 255] } else { [0; 4] })).unwrap();
    assert!((0.4..0.6).contains(&half), "{half}");
    let mut hdr = frame(&|_| [0; 4]);
    hdr.pixel = butterpollo_win_graphics::capture::Pixel::RgbaF16;
    assert!(lit(&hdr).is_err());
}

#[test]
fn a_hardware_case_puts_back_only_its_games_compatibility_settings() {
    use butterpollo_win_io::nucleus::layers::Value;
    let text = |byte: u8| Value {
        kind: 1,
        data: vec![byte, 0, 0, 0],
    };
    let content = [PathBuf::from(r"C:\Nucleus\content")];
    let content = &content[..];
    let before = BTreeMap::from([
        (r"D:\G\Game.exe".to_string(), text(1)),
        (r"D:\Gone\Game.exe".to_string(), text(2)),
        (r"C:\Other.exe".to_string(), text(3)),
        (
            r"C:\Nucleus\content\G\Instance1\Game.exe".to_string(),
            text(4),
        ),
    ]);
    let after = BTreeMap::from([
        (r"d:/g/game.EXE".to_string(), text(9)),
        (
            r"C:\Nucleus\content\G\Instance0\GAME.exe".to_string(),
            text(9),
        ),
        (
            r"C:\Nucleus\content\G\Instance0\Tool.exe".to_string(),
            text(9),
        ),
        (r"C:\Nucleus\contents\Game.exe".to_string(), text(9)),
        (r"E:\Mine\Game.exe".to_string(), text(9)),
        (r"C:\Other.exe".to_string(), text(9)),
    ]);
    assert_eq!(
        layer_changes(&before, &after, r"D:\G\Game.exe", content, "game.exe"),
        [
            (r"C:\Nucleus\content\G\Instance0\GAME.exe".to_string(), None),
            (r"d:/g/game.EXE".to_string(), None),
            (
                r"C:\Nucleus\content\G\Instance1\Game.exe".to_string(),
                Some(text(4))
            ),
            (r"D:\G\Game.exe".to_string(), Some(text(1))),
        ]
    );
    assert!(layer_changes(&before, &before, r"D:\G\Game.exe", content, "game.exe").is_empty());
    // A session from the user's own copy sets its instances below that
    // copy's content folder; the host's still count.
    let user = r"C:\ProgramData\Butterpollo\nucleus-user\S-1-5-21-7\2.4.2\content";
    let both = [PathBuf::from(r"C:\Nucleus\content"), PathBuf::from(user)];
    let after = BTreeMap::from([
        (format!(r"{user}\G\Instance0\Game.exe"), text(9)),
        (
            r"C:\Nucleus\content\G\Instance0\Game.exe".to_string(),
            text(9),
        ),
        (format!(r"{user}\G\Instance0\Tool.exe"), text(9)),
        (format!(r"{user}s\Game.exe"), text(9)),
    ]);
    let empty = BTreeMap::new();
    assert_eq!(
        layer_changes(&empty, &after, r"D:\G\Game.exe", &both, "game.exe"),
        [
            (r"C:\Nucleus\content\G\Instance0\Game.exe".to_string(), None),
            (format!(r"{user}\G\Instance0\Game.exe"), None),
        ]
    );
    assert_eq!(
        layer_changes(&empty, &after, r"D:\G\Game.exe", content, "game.exe"),
        [(r"C:\Nucleus\content\G\Instance0\Game.exe".to_string(), None)]
    );
}

#[test]
fn a_hardware_case_puts_the_game_list_and_our_handlers_back_exactly() {
    let root = std::env::temp_dir().join(format!("nucleus-case-{}", uuid::Uuid::new_v4()));
    let handlers = root.join("handlers");
    std::fs::create_dir_all(&handlers).unwrap();
    let ours = |name: &str| {
        format!(
            "{}\nGame.GUID='Rubylight-{name}'\n",
            butterpollo_session::nucleus::generic::MARKER
        )
    };
    let (earlier, hub, list) = (
        handlers.join("Rubylight-earlier.js"),
        handlers.join("Hub.js"),
        root.join("userprofile.json"),
    );
    std::fs::write(&earlier, ours("earlier")).unwrap();
    std::fs::write(&hub, "Game.GUID='Hub'\n").unwrap();
    std::fs::write(&list, r#"{"Games":[{"GameGuid":"Rubylight-earlier"}]}"#).unwrap();
    let kept = KeptFiles::take(&[&handlers], &list).unwrap();
    // The case's sweep takes the earlier one and its entry, and writes
    // its own; the user's hub handler changes meanwhile.
    std::fs::remove_file(&earlier).unwrap();
    std::fs::write(handlers.join("Rubylight-case.js"), ours("case")).unwrap();
    std::fs::write(&list, r#"{"Games":[{"GameGuid":"Rubylight-case"}]}"#).unwrap();
    std::fs::write(&hub, "Game.GUID='Hub'\n// edited\n").unwrap();
    assert!(kept.put_back().is_empty());
    assert_eq!(std::fs::read_to_string(&earlier).unwrap(), ours("earlier"));
    assert!(!handlers.join("Rubylight-case.js").exists());
    assert_eq!(
        std::fs::read_to_string(&list).unwrap(),
        r#"{"Games":[{"GameGuid":"Rubylight-earlier"}]}"#
    );
    assert_eq!(
        std::fs::read_to_string(&hub).unwrap(),
        "Game.GUID='Hub'\n// edited\n"
    );
    // A list that was not there goes again.
    std::fs::remove_file(&list).unwrap();
    let kept = KeptFiles::take(&[&handlers], &list).unwrap();
    std::fs::write(&list, "{}").unwrap();
    assert!(kept.put_back().is_empty());
    assert!(!list.exists());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_hardware_case_puts_the_users_copy_handlers_back_too() {
    let root = std::env::temp_dir().join(format!("nucleus-case-{}", uuid::Uuid::new_v4()));
    let (host, copy) = (root.join("host"), root.join("copy").join("handlers"));
    std::fs::create_dir_all(&host).unwrap();
    let list = root.join("userprofile.json");
    let ours = format!(
        "{}\nGame.GUID='Rubylight-case'\n",
        butterpollo_session::nucleus::generic::MARKER
    );
    std::fs::write(host.join("Rubylight-earlier.js"), &ours).unwrap();
    // The user's copy is not there yet: the session makes it.
    let kept = KeptFiles::take(&[&host, &copy], &list).unwrap();
    std::fs::create_dir_all(&copy).unwrap();
    std::fs::write(copy.join("Rubylight-earlier.js"), &ours).unwrap();
    std::fs::write(copy.join("Rubylight-case.js"), &ours).unwrap();
    std::fs::write(copy.join("Hub.js"), "Game.GUID='Hub'\n").unwrap();
    std::fs::remove_file(host.join("Rubylight-earlier.js")).unwrap();
    assert!(kept.put_back().is_empty());
    assert!(host.join("Rubylight-earlier.js").is_file());
    assert_eq!(
        KeptFiles::own_handlers(&copy).unwrap(),
        Vec::<PathBuf>::new()
    );
    // Only ours go: a hub handler mirrored into the copy stays.
    assert!(copy.join("Hub.js").is_file());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_hardware_case_puts_nothing_back_through_a_link() {
    let root = std::env::temp_dir().join(format!("nucleus-case-{}", uuid::Uuid::new_v4()));
    let (copy, elsewhere) = (root.join("copy"), root.join("elsewhere"));
    let handlers = copy.join("handlers");
    std::fs::create_dir_all(&handlers).unwrap();
    std::fs::create_dir_all(&elsewhere).unwrap();
    let ours = format!(
        "{}\nGame.GUID='Rubylight-case'\n",
        butterpollo_session::nucleus::generic::MARKER
    );
    let list = copy.join("content").join("userprofile.json");
    let kept = KeptFiles::take(&[&handlers], &list).unwrap();
    // The user makes their handlers folder and their content folder links
    // to files of the host's.
    std::fs::remove_dir(&handlers).unwrap();
    std::fs::write(elsewhere.join("Rubylight-host.js"), &ours).unwrap();
    std::fs::write(elsewhere.join("userprofile.json"), "host's").unwrap();
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&handlers)
        .arg(&elsewhere)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "mklink /J failed");
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(copy.join("content"))
        .arg(&elsewhere)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "mklink /J failed");
    let failures = kept.put_back();
    assert_eq!(failures.len(), 2, "{failures:?}");
    assert!(
        failures.iter().all(|f| f.contains("is a link")),
        "{failures:?}"
    );
    assert!(elsewhere.join("Rubylight-host.js").is_file());
    assert_eq!(
        std::fs::read_to_string(elsewhere.join("userprofile.json")).unwrap(),
        "host's"
    );
    std::fs::remove_dir(&handlers).unwrap();
    std::fs::remove_dir(copy.join("content")).unwrap();
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_hardware_case_reads_the_run_it_expects() {
    assert_eq!(expected_run(None).unwrap(), None);
    assert_eq!(expected_run(Some(" ")).unwrap(), None);
    assert_eq!(expected_run(Some("user")).unwrap(), Some(Run::User));
    assert_eq!(expected_run(Some("elevated")).unwrap(), Some(Run::Elevated));
    assert!(expected_run(Some("admin")).is_err());
}

#[test]
fn a_hardware_case_tells_the_users_copy_from_the_hosts() {
    let folder = Path::new(r"C:\ProgramData\Butterpollo\nucleus\2.4.2");
    let copy = Path::new(r"C:\ProgramData\Butterpollo\nucleus-user\S-1-5-21-7\2.4.2\");
    let run = |image: &str| ran_from(image, folder, copy);
    assert_eq!(
        run(r"C:\ProgramData\Butterpollo\nucleus-user\S-1-5-21-7\2.4.2\NucleusCoop.exe").unwrap(),
        Run::User
    );
    assert_eq!(
        run(r"\\?\c:\programdata\butterpollo\NUCLEUS-USER\s-1-5-21-7\2.4.2\nucleuscoop.EXE")
            .unwrap(),
        Run::User
    );
    assert_eq!(
        run(r"C:\ProgramData\Butterpollo\nucleus\2.4.2\NucleusCoop.exe").unwrap(),
        Run::Elevated
    );
    // Another user's copy, a folder below the copy, another program.
    for image in [
        r"C:\ProgramData\Butterpollo\nucleus-user\S-1-5-21-8\2.4.2\NucleusCoop.exe",
        r"C:\ProgramData\Butterpollo\nucleus-user\S-1-5-21-7\2.4.2\content\NucleusCoop.exe",
        r"C:\ProgramData\Butterpollo\nucleus-user\S-1-5-21-7\2.4.2\Game.exe",
    ] {
        assert!(run(image).is_err(), "{image}");
    }
}

#[test]
fn a_hardware_case_asks_for_standard_rights_at_medium_integrity() {
    standard_rights("NucleusCoop.exe", 1, false, 0x2000).unwrap();
    let elevated = standard_rights("NucleusCoop.exe", 1, true, 0x3000).unwrap_err();
    assert_eq!(
        elevated.to_string(),
        "NucleusCoop.exe 1 runs elevated at integrity 0x3000, not unelevated at medium (0x2000)"
    );
    // UAC off: high integrity without an elevated token; a low one.
    assert!(standard_rights("Game.exe", 2, false, 0x3000).is_err());
    assert!(standard_rights("Game.exe", 2, false, 0x1000).is_err());
    // This test's own process, read as the case reads the session's.
    let (elevated, integrity) = token_rights(std::process::id()).unwrap();
    assert_eq!(elevated, butterpollo_win_io::process::elevated().unwrap());
    if elevated {
        assert!(integrity >= 0x3000, "0x{integrity:x}");
    } else {
        assert!(integrity >= MEDIUM_INTEGRITY, "0x{integrity:x}");
    }
    assert!(token_rights(0).is_err());
}

#[test]
fn the_cold_case_seeds_the_bad_settings_and_checks_the_forced_ones() {
    let shipped = butterpollo_session::nucleus::profile::DEFAULT_SETTINGS.as_bytes();
    let cold = cold_settings(shipped);
    settings_are(&cold, &COLD).unwrap();
    assert!(settings_are(&cold, &FORCED).is_err());
    // What a session writes passes, and what it puts back is the cold file.
    let settings = butterpollo_session::nucleus::profile::settings_ini(&cold);
    settings_are(&settings.text, &FORCED).unwrap();
    let (back, _) =
        butterpollo_session::nucleus::profile::put_back_settings(&settings.text, &settings.forced);
    settings_are(&back, &COLD).unwrap();
    // Each value is checked and named.
    let filtered = butterpollo_session::nucleus::profile::ini_set(
        &settings.text,
        &[("Misc", "GameSortingOpt", "Show_Type1")],
    );
    let error = settings_are(&filtered, &FORCED).unwrap_err().to_string();
    assert_eq!(
        error,
        "Settings.ini has GameSortingOpt=Show_Type1, not Sort_2"
    );
    let error = settings_are(b"", &FORCED).unwrap_err().to_string();
    assert_eq!(error, "Settings.ini has UseXinputIndex=nothing, not True");
}

#[test]
fn the_click_is_timed_from_nucleus_start() {
    let t0 = Instant::now();
    let line = |s: u64, text: &str| (t0 + Duration::from_secs(s), text.to_string());
    let lines = [
        line(0, "INFO profile written"),
        line(1, "INFO The co-op helper started pid=4"),
        line(
            5,
            "WARN The co-op helper shows \"Warning: x\" on the PC's screen",
        ),
        line(9, "INFO co-op profile Rubylight clicked"),
    ];
    assert_eq!(click_delay(&lines).unwrap(), Duration::from_secs(8));
    assert!(click_delay(&lines[..3]).is_err());
    assert!(click_delay(&lines[2..]).is_err());
}

#[test]
fn a_file_that_is_not_there_reads_as_none() {
    let dir = std::env::temp_dir().join(format!("nucleus-read-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("crashlog.txt");
    assert_eq!(read_if_there(&path).unwrap(), None);
    std::fs::write(&path, "x").unwrap();
    assert_eq!(read_if_there(&path).unwrap(), Some(b"x".to_vec()));
    // A folder in its place is an error, not a missing file.
    assert!(read_if_there(&dir).is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn kept_settings_go_back_and_take_the_sessions_journal_along() {
    let dir = std::env::temp_dir().join(format!("nucleus-kept-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Settings.ini");
    let journal = dir.join("Rubylight-settings.json");
    std::fs::write(&path, "seeded").unwrap();
    std::fs::write(&journal, "[]").unwrap();
    drop(KeptSettings {
        path: path.clone(),
        before: b"users".to_vec(),
        journal: Some(journal.clone()),
        user: None,
    });
    assert_eq!(std::fs::read(&path).unwrap(), b"users");
    assert!(!journal.exists());
    // None left is fine, and a copy has none.
    for journal in [Some(journal.clone()), None] {
        drop(KeptSettings {
            path: path.clone(),
            before: b"users".to_vec(),
            journal,
            user: None,
        });
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
