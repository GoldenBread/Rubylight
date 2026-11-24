//! Unit tests for Nucleus Co-op game list safety, `Settings.ini` journal restore, and game profiles.

use super::*;

#[test]
fn a_game_list_the_host_cannot_edit_is_refused_before_anything_changes() {
    let nucleus = tempfile::tempdir().unwrap();
    let setup = Setup::new(r"D:\Puzzle\Game.exe", "Puzzle", None);
    // No list yet: a new one is written.
    check_list(nucleus.path(), Some(&setup), &[]).unwrap();
    let list = list(nucleus.path());
    std::fs::create_dir_all(list.parent().unwrap()).unwrap();
    std::fs::write(
        &list,
        r#"{"Games":[{"GameGuid":"A","ExePath":"C:\\A.exe"}]}"#,
    )
    .unwrap();
    check_list(nucleus.path(), Some(&setup), &["Rubylight-x"]).unwrap();
    // A game list the catalog reads but no edit could keep whole.
    for broken in ["{", r#"{"Games":7}"#, "[1]"] {
        std::fs::write(&list, broken).unwrap();
        let error = format!(
            "{:#}",
            check_list(nucleus.path(), Some(&setup), &[]).unwrap_err()
        );
        assert!(error.contains("userprofile.json"), "{broken}: {error}");
    }
    std::fs::write(&list, r#"{"Games":[{"GameGuid":}]}"#).unwrap();
    let error = format!(
        "{:#}",
        check_list(nucleus.path(), None, &["Rubylight-x"]).unwrap_err()
    );
    assert!(error.contains("userprofile.json"), "{error}");
    assert_eq!(
        std::fs::read_to_string(&list).unwrap(),
        r#"{"Games":[{"GameGuid":}]}"#
    );
}

#[test]
fn only_a_game_list_file_on_the_disk_of_sane_size_is_read_never_a_link() {
    let nucleus = tempfile::tempdir().unwrap();
    let appdata = tempfile::tempdir().unwrap();
    let setup = Setup::new(r"D:\Puzzle\Game.exe", "Puzzle", None);
    let list = list(nucleus.path());
    let shown = list.display().to_string();
    std::fs::create_dir_all(list.parent().unwrap()).unwrap();
    let theirs = r#"{"Games":[{"GameGuid":"A","ExePath":"C:\\A.exe"}]}"#;
    // A list as Nucleus writes it still reads.
    std::fs::write(&list, theirs).unwrap();
    let games = catalog(nucleus.path(), true).unwrap().games;
    assert_eq!(games.len(), 1);
    check_list(nucleus.path(), Some(&setup), &[]).unwrap();
    // Every read of the list refuses it, naming it, and nothing is written.
    let refused = |why: &str| {
        let errors = [
            catalog(nucleus.path(), true).map(|_| ()),
            check_list(nucleus.path(), Some(&setup), &[]),
            write_own(nucleus.path(), &setup).map(|_| ()),
            remove_game(nucleus.path(), "A"),
        ];
        for error in errors {
            let error = format!("{:#}", error.unwrap_err());
            assert!(error.contains(&shown) && error.contains(why), "{error}");
        }
    };

    // Larger than any list: never read whole.
    let big = std::fs::File::create(&list).unwrap();
    big.set_len(LIST_CAP + 1).unwrap();
    drop(big);
    refused("larger than 16 MB");
    assert_eq!(std::fs::metadata(&list).unwrap().len(), LIST_CAP + 1);
    // A folder where the list should be.
    std::fs::remove_file(&list).unwrap();
    std::fs::create_dir(&list).unwrap();
    refused("is a folder, a link or a device");
    assert!(list.is_dir());
    std::fs::remove_dir(&list).unwrap();
    // A junction to a folder elsewhere.
    let elsewhere = appdata.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    let junction = |link: &Path| {
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(&elsewhere)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "mklink /J failed");
    };
    junction(&list);
    refused("is a folder, a link or a device");
    assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
    std::fs::remove_dir(&list).unwrap();
    // A link to a file elsewhere, which takes Developer Mode or
    // administrator rights to make.
    let target = appdata.path().join("target.json");
    std::fs::write(&target, theirs).unwrap();
    match std::os::windows::fs::symlink_file(&target, &list) {
        Ok(()) => {
            refused("is a folder, a link or a device");
            assert_eq!(std::fs::read_to_string(&target).unwrap(), theirs);
            std::fs::remove_file(&list).unwrap();
        }
        Err(error) => eprintln!(
            "the symbolic link case is skipped: making one needs Developer Mode or administrator rights ({error})"
        ),
    }

    // The list is replaced through neither a link at its name nor one at
    // the temporary file's.
    std::fs::write(&list, theirs).unwrap();
    let temporary = list.with_extension("json.rubylight");
    junction(&temporary);
    let error = format!("{:#}", write_own(nucleus.path(), &setup).unwrap_err());
    assert!(
        error.contains(&temporary.display().to_string()) && error.contains("is a link"),
        "{error}"
    );
    assert_eq!(std::fs::read_to_string(&list).unwrap(), theirs);
    std::fs::remove_dir(&temporary).unwrap();
    std::fs::remove_file(&list).unwrap();
    junction(&list);
    let error = format!("{:#}", replace(nucleus.path(), &list, theirs).unwrap_err());
    assert!(
        error.contains(&shown) && error.contains("is a link"),
        "{error}"
    );
    assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
    std::fs::remove_dir(&list).unwrap();
    // And with none, it is.
    write_own(nucleus.path(), &setup).unwrap();
    let games = catalog(nucleus.path(), true).unwrap().games;
    assert_eq!(games.len(), 1);
    assert_eq!(games[0].guid, setup.guid());
}

fn ini(folder: &Path, section: &str, key: &str) -> Option<Vec<u8>> {
    let text = std::fs::read(folder.join("Settings.ini")).unwrap();
    profile::ini_value(&text, section, key)
}

/// A user's file in the PC's code page (Nucleus writes it through
/// kernel32's ANSI calls: ü is 0xFC), with the filters that hid the
/// game (#59).
const USERS: &[u8] = b"\xef\xbb\xbf[Misc]\r\nSettingsLocation=C:\\M\xfcnchen\r\nGameSortingOpt=Sort_2,Show_Type2\r\n[Dev]\r\nShowFavoriteOnly=True\r\nUseXinputIndex=False\r\n";

#[test]
fn settings_are_completed_forced_and_the_users_put_back_after_the_session() {
    let nucleus = tempfile::tempdir().unwrap();
    let folder = nucleus.path();
    let path = folder.join("Settings.ini");
    std::fs::write(&path, USERS).unwrap();
    let mut guard = write_settings(folder).unwrap();
    let text = std::fs::read(&path).unwrap();
    // Edited as bytes: the BOM, the code page's ü and CRLF stay.
    assert!(
        text.starts_with(b"\xef\xbb\xbf[Misc]\r\nSettingsLocation=C:\\M\xfcnchen\r\n"),
        "{}",
        String::from_utf8_lossy(&text)
    );
    assert_eq!(ini(folder, "Dev", "UseXinputIndex").unwrap(), b"True");
    assert_eq!(ini(folder, "Dev", "ShowFavoriteOnly").unwrap(), b"False");
    assert_eq!(ini(folder, "Dev", "DisablePathCheck").unwrap(), b"True");
    assert_eq!(ini(folder, "Misc", "GameSortingOpt").unwrap(), b"Sort_2");
    // Completed from Nucleus' defaults.
    assert_eq!(ini(folder, "Dev", "Blur").unwrap(), b"4");
    assert!(settings_journal(folder).is_file());
    // Nucleus changed the list's order meanwhile: that stays.
    let changed = profile::ini_set(
        &std::fs::read(&path).unwrap(),
        &[("Misc", "GameSortingOpt", "Sort_3")],
    );
    std::fs::write(&path, changed).unwrap();
    guard.restore();
    assert_eq!(ini(folder, "Dev", "ShowFavoriteOnly").unwrap(), b"True");
    assert_eq!(ini(folder, "Dev", "DisablePathCheck").unwrap(), b"False");
    assert_eq!(ini(folder, "Misc", "GameSortingOpt").unwrap(), b"Sort_3");
    // Pads by index and the completed keys stay.
    assert_eq!(ini(folder, "Dev", "UseXinputIndex").unwrap(), b"True");
    assert_eq!(ini(folder, "Dev", "Blur").unwrap(), b"4");
    assert_eq!(
        ini(folder, "Misc", "SettingsLocation").unwrap(),
        b"C:\\M\xfcnchen"
    );
    assert!(!settings_journal(folder).exists());
    // Once only.
    std::fs::write(&path, b"[Dev]\r\nShowFavoriteOnly=False\r\n").unwrap();
    drop(guard);
    assert_eq!(ini(folder, "Dev", "ShowFavoriteOnly").unwrap(), b"False");
}

#[test]
fn a_missing_or_partial_settings_file_is_completed_from_nucleus_defaults() {
    // The host wrote the two-key file where none was; Nucleus then
    // stopped at its start on the first key missing (#59).
    for old in [
        None,
        Some(&b"[Dev]\r\nUseXinputIndex=True\r\n[Misc]\r\nDisableGameProfiles=False\r\n"[..]),
    ] {
        let nucleus = tempfile::tempdir().unwrap();
        let folder = nucleus.path();
        if let Some(old) = old {
            std::fs::write(folder.join("Settings.ini"), old).unwrap();
        }
        drop(write_settings(folder).unwrap());
        let text = std::fs::read(folder.join("Settings.ini")).unwrap();
        for (section, key) in [
            ("Hotkeys", "Close"),
            ("Dev", "ShowFavoriteOnly"),
            ("Dev", "ProfileAssignGamepadByButonPress"),
            ("Misc", "VGMOnly"),
            ("CustomLayout", "MaxPlayers"),
        ] {
            assert!(
                profile::ini_value(&text, section, key).is_some_and(|v| !v.is_empty()),
                "{section}.{key}"
            );
        }
        assert_eq!(ini(folder, "Dev", "UseXinputIndex").unwrap(), b"True");
        // The completed DisablePathCheck=False came back after the session.
        assert_eq!(ini(folder, "Dev", "DisablePathCheck").unwrap(), b"False");
        assert!(!settings_journal(folder).exists());
    }
}

/// A host that stopped mid-session: its guard never ran, and Windows
/// closed its lock.
fn stopped(mut guard: SettingsGuard) {
    guard._lock = None;
    std::mem::forget(guard);
}

#[test]
fn what_a_session_the_host_never_ended_forced_is_put_back_first() {
    let nucleus = tempfile::tempdir().unwrap();
    let folder = nucleus.path();
    std::fs::write(folder.join("Settings.ini"), USERS).unwrap();
    stopped(write_settings(folder).unwrap());
    assert_eq!(ini(folder, "Dev", "ShowFavoriteOnly").unwrap(), b"False");
    // The next session keeps the user's value, not the last session's.
    stopped(write_settings(folder).unwrap());
    let journal: Vec<profile::Forced> =
        serde_json::from_slice(&std::fs::read(settings_journal(folder)).unwrap()).unwrap();
    let favourites = journal
        .iter()
        .find(|f| f.key == "ShowFavoriteOnly")
        .unwrap();
    assert_eq!(favourites.before, b"True");
    // As the host starts.
    assert_eq!(restore_settings_left(folder).unwrap(), Some(3));
    assert_eq!(ini(folder, "Dev", "ShowFavoriteOnly").unwrap(), b"True");
    assert!(!settings_journal(folder).exists());
    assert_eq!(restore_settings_left(folder).unwrap(), Some(0));
    // An empty journal goes; one that can't be read stops the session
    // and names itself.
    std::fs::write(settings_journal(folder), b"").unwrap();
    assert_eq!(restore_settings_left(folder).unwrap(), Some(0));
    assert!(!settings_journal(folder).exists());
    std::fs::write(settings_journal(folder), b"{").unwrap();
    let error = format!("{:#}", write_settings(folder).err().unwrap());
    assert!(
        error.contains("Rubylight-settings.json can't be read") && error.contains("delete it"),
        "{error}"
    );
}

#[test]
fn a_journal_left_beside_no_settings_file_goes_and_the_file_is_written_whole() {
    let nucleus = tempfile::tempdir().unwrap();
    let folder = nucleus.path();
    let path = folder.join("Settings.ini");
    std::fs::write(&path, USERS).unwrap();
    stopped(write_settings(folder).unwrap());
    // The user deleted the file meanwhile.
    std::fs::remove_file(&path).unwrap();
    let guard = write_settings(folder).unwrap();
    assert_eq!(ini(folder, "Dev", "DisablePathCheck").unwrap(), b"True");
    assert!(ini(folder, "Hotkeys", "Close").is_some_and(|v| !v.is_empty()));
    // The journal is the new session's, put back when it ends.
    drop(guard);
    assert!(!settings_journal(folder).exists());
    assert_eq!(ini(folder, "Dev", "DisablePathCheck").unwrap(), b"False");
    // Nor does the service fail on it.
    stopped(write_settings(folder).unwrap());
    std::fs::remove_file(&path).unwrap();
    assert_eq!(restore_settings_left(folder).unwrap(), Some(0));
    assert!(!settings_journal(folder).exists());
}

#[test]
fn the_service_puts_nothing_back_under_a_session_or_another_restore() {
    let nucleus = tempfile::tempdir().unwrap();
    let folder = nucleus.path();
    std::fs::write(folder.join("Settings.ini"), USERS).unwrap();
    // A session starting or running: the user's values wait for it.
    let guard = write_settings(folder).unwrap();
    assert_eq!(restore_settings_left(folder).unwrap(), None);
    assert_eq!(ini(folder, "Dev", "ShowFavoriteOnly").unwrap(), b"False");
    assert!(settings_journal(folder).is_file());
    drop(guard);
    assert_eq!(ini(folder, "Dev", "ShowFavoriteOnly").unwrap(), b"True");
    // A restore elsewhere holds it.
    stopped(write_settings(folder).unwrap());
    let other = Lock::try_take(&settings_journal(folder)).unwrap().unwrap();
    assert_eq!(restore_settings_left(folder).unwrap(), None);
    assert_eq!(ini(folder, "Dev", "ShowFavoriteOnly").unwrap(), b"False");
    drop(other);
    assert_eq!(restore_settings_left(folder).unwrap(), Some(3));
    assert_eq!(ini(folder, "Dev", "ShowFavoriteOnly").unwrap(), b"True");
}

#[test]
fn a_settings_file_saved_as_utf16_is_refused_and_left_as_it_is() {
    let nucleus = tempfile::tempdir().unwrap();
    let folder = nucleus.path();
    let path = folder.join("Settings.ini");
    let utf16 = |bom: [u8; 2], big: bool| {
        let mut bytes = bom.to_vec();
        for unit in "[Dev]\r\nShowFavoriteOnly=True\r\n".encode_utf16() {
            let pair = if big {
                unit.to_be_bytes()
            } else {
                unit.to_le_bytes()
            };
            bytes.extend_from_slice(&pair);
        }
        bytes
    };
    for text in [utf16([0xff, 0xfe], false), utf16([0xfe, 0xff], true)] {
        std::fs::write(&path, &text).unwrap();
        let error = format!("{:#}", write_settings(folder).err().unwrap());
        assert!(
            error.contains("Settings.ini") && error.contains("UTF-16"),
            "{error}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), text);
        assert!(!settings_journal(folder).exists());
    }
    // Saved as UTF-16 while a session ran: its journal stays.
    std::fs::write(&path, USERS).unwrap();
    stopped(write_settings(folder).unwrap());
    let text = utf16([0xff, 0xfe], false);
    std::fs::write(&path, &text).unwrap();
    let error = format!("{:#}", restore_settings_left(folder).unwrap_err());
    assert!(error.contains("UTF-16"), "{error}");
    assert!(settings_journal(folder).is_file());
    assert_eq!(std::fs::read(&path).unwrap(), text);
}

#[test]
fn the_profile_takes_its_own_number_or_the_next() {
    let nucleus = tempfile::tempdir().unwrap();
    let profiles = nucleus.path().join("game profiles").join("Borderlands 2");
    std::fs::create_dir_all(&profiles).unwrap();
    std::fs::write(profiles.join("Profile[1].json"), r#"{"Title":"Mine"}"#).unwrap();
    std::fs::write(profiles.join("Profile[2].json"), r#"{"Title":""}"#).unwrap();
    let screens = [SCREEN];
    let number = write_profile(
        nucleus.path(),
        "Borderlands 2",
        &[player(0), player(1)],
        &screens,
        false,
    )
    .unwrap();
    assert_eq!(number, 3);
    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(profiles.join("Profile[3].json")).unwrap()).unwrap();
    assert_eq!(written["Title"], profile::TITLE);
    assert_eq!(written["Player(s)"], 2);
    assert_eq!(written["Data"][1]["GamepadGuid"], profile::gamepad_guid(1));
    assert_eq!(written["Data"][1]["MonitorBounds"]["X"], 6000);
    // The user's profiles are left as they were.
    assert_eq!(
        std::fs::read_to_string(profiles.join("Profile[1].json")).unwrap(),
        r#"{"Title":"Mine"}"#
    );

    // Next time the same file is rewritten, for one player.
    let again = write_profile(
        nucleus.path(),
        "Borderlands 2",
        &[player(2)],
        &screens,
        false,
    )
    .unwrap();
    assert_eq!(again, 3);
    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(profiles.join("Profile[3].json")).unwrap()).unwrap();
    assert_eq!(written["Player(s)"], 1);
    assert_eq!(written["Data"][0]["GamepadGuid"], profile::gamepad_guid(2));
    assert_eq!(std::fs::read_dir(&profiles).unwrap().count(), 3);
}

#[test]
fn a_foreign_profile_in_the_next_place_is_never_overwritten() {
    let nucleus = tempfile::tempdir().unwrap();
    let profiles = nucleus.path().join("game profiles").join("game");
    std::fs::create_dir_all(&profiles).unwrap();
    // Numbers 1 and 3: the list has two, so a new profile would be the 3rd.
    std::fs::write(profiles.join("Profile[1].json"), r#"{"Title":"A"}"#).unwrap();
    std::fs::write(profiles.join("Profile[3].json"), r#"{"Title":"B"}"#).unwrap();
    let error = write_profile(nucleus.path(), "game", &[player(0)], &[SCREEN], false)
        .unwrap_err()
        .to_string();
    assert!(error.contains("Profile[3].json"), "{error}");
    assert_eq!(
        std::fs::read_to_string(profiles.join("Profile[3].json")).unwrap(),
        r#"{"Title":"B"}"#
    );

    // Nucleus lists every file of the folder, so a stray one shifts the
    // places: a new profile there would never load and is taken back.
    let stray = nucleus.path().join("game profiles").join("stray");
    std::fs::create_dir_all(&stray).unwrap();
    std::fs::write(stray.join("Profile[1].json"), r#"{"Title":"A"}"#).unwrap();
    std::fs::write(stray.join("Profile[1].json.bak"), r#"{"Title":"A"}"#).unwrap();
    let error = write_profile(nucleus.path(), "stray", &[player(0)], &[SCREEN], false)
        .unwrap_err()
        .to_string();
    assert!(error.contains("would not load"), "{error}");
    assert!(
        error.contains("Profile[1].json, Profile[3].json, Profile[1].json.bak"),
        "{error}"
    );
    assert!(!stray.join("Profile[3].json").exists());
}

#[test]
fn a_guid_that_names_another_folder_is_refused() {
    let nucleus = tempfile::tempdir().unwrap();
    for guid in ["..", r"..\..\x", "a/b", "C:x", " ", "a\nb"] {
        let error = write_profile(nucleus.path(), guid, &[player(0)], &[SCREEN], false)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("cannot name a profile folder"),
            "{guid:?}: {error}"
        );
    }
    assert!(!nucleus.path().join("game profiles").exists());
}
