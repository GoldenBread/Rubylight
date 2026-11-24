//! Tests for Steam play history, cloud collections, and library change stamps.
use super::super::collections::{COLLECTIONS_FILE, parse_collections};
use super::super::*;
use super::*;

#[test]
fn played_times_count_only_once_a_local_config_was_read() {
    let steam = tempfile::tempdir().unwrap();
    let apps = steam.path().join("steamapps");
    std::fs::create_dir_all(apps.join("common").join("Portal 2")).unwrap();
    std::fs::write(
        apps.join("appmanifest_620.acf"),
        "\"AppState\" { \"appid\" \"620\" \"name\" \"Portal 2\" \"installdir\" \"Portal 2\" }",
    )
    .unwrap();
    let config = steam.path().join("userdata").join("7").join("config");
    std::fs::create_dir_all(&config).unwrap();
    let roots = [steam.path().to_path_buf()];
    // A user folder whose local config can't be read: when is unknown, not never.
    let unread = discover(&roots);
    assert_eq!(unread.len(), 1);
    assert!(!unread[0].history);
    std::fs::write(
        config.join("localconfig.vdf"),
        "\"UserLocalConfigStore\" { \"Software\" { \"Valve\" { \"Steam\" { \"apps\" { \"10\" { \"LastPlayed\" \"5\" } } } } } }",
    )
    .unwrap();
    // Read, without a time for Portal 2: never played.
    let read = discover(&roots);
    assert!(read[0].history);
    assert_eq!(read[0].last_played, 0);
}
#[test]
fn collections_read_steams_real_format() {
    let steam = steam_with_collections("1", CLOUD_STORAGE);
    let read = collections(&[steam.path().to_owned()]);
    assert_eq!(read.problem, None);
    assert_eq!(
        read.apps[&570],
        ["Favoriten", "VR"],
        "in each it is listed in"
    );
    assert_eq!(read.apps[&730], ["Favoriten"]);
    assert_eq!(read.apps[&620], ["VR"]);
    assert!(!read.apps.contains_key(&1000), "taken out again");
    assert!(
        !read.apps.contains_key(&440),
        "Steam's own Hidden is no group"
    );
}

#[test]
fn collections_tolerate_damage_with_one_problem_and_no_panic() {
    for text in [
        "",
        "{ not json",
        "{}",
        "[1, null, []]",
        r#"[["user-collections.a"]]"#,
    ] {
        let read = parse_collections(text);
        assert!(read.apps.is_empty(), "{text}");
    }
    assert!(parse_collections("{ not json").problem.is_some());
    assert!(parse_collections("{}").problem.is_some());
    // One entry changed shape: the others still read, and the line says so.
    let read = parse_collections(
        r#"[["user-collections.a",{"value":"{\"id\":\"a\",\"name\":\"A\",\"added\":[1]}"}],
            ["user-collections.b",{"value":"not json"}],
            ["user-collections.c",{"value":"{\"name\":\"C\",\"added\":[\"x\"]}"}],
            ["user-collections.d",{"value":"{\"name\":\"  \",\"added\":[2]}"}]]"#,
    );
    assert_eq!(read.apps.len(), 1);
    assert_eq!(read.apps[&1], ["A"]);
    assert_eq!(
        read.problem.as_deref(),
        Some("3 collections could not be read")
    );
}

#[test]
fn collections_come_from_the_account_steam_wrote_last() {
    let steam = steam_with_collections(
        "1",
        r#"[["user-collections.a",{"value":"{\"name\":\"Old\",\"added\":[1]}"}]]"#,
    );
    write_collections(
        steam.path(),
        "2",
        r#"[["user-collections.a",{"value":"{\"name\":\"New\",\"added\":[1]}"}]]"#,
    );
    let older = std::fs::File::options()
        .write(true)
        .open(
            steam
                .path()
                .join("userdata/1/config/cloudstorage")
                .join(COLLECTIONS_FILE),
        )
        .unwrap();
    older
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3600))
        .unwrap();
    drop(older);
    assert_eq!(collections(&[steam.path().to_owned()]).apps[&1], ["New"]);
}

/// Steam folder with accounts 1 ("Old", written an hour ago) and 2 ("New"), and a `loginusers.vdf`
/// when `marked` names an account for `MostRecent`.
fn steam_with_two_accounts(marked: Option<u64>) -> tempfile::TempDir {
    let named = |name: &str| {
        format!(r#"[["user-collections.a",{{"value":"{{\"name\":\"{name}\",\"added\":[1]}}"}}]]"#)
    };
    let steam = steam_with_collections("1", &named("Old"));
    write_collections(steam.path(), "2", &named("New"));
    std::fs::File::options()
        .write(true)
        .open(
            steam
                .path()
                .join("userdata/1/config/cloudstorage")
                .join(COLLECTIONS_FILE),
        )
        .unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3600))
        .unwrap();
    if let Some(account) = marked {
        std::fs::create_dir_all(steam.path().join("config")).unwrap();
        let id = 76561197960265728 + account;
        let other = id + 100;
        std::fs::write(
            steam.path().join("config/loginusers.vdf"),
            format!(
                "\"users\"\n{{\n\"{other}\"\n{{\n\"MostRecent\" \"0\"\n}}\n\"{id}\"\n{{\n\"AccountName\" \"a\"\n\"MostRecent\" \"1\"\n}}\n}}\n"
            ),
        )
        .unwrap();
    }
    steam
}

#[test]
fn collections_come_from_the_account_marked_most_recent_else_from_the_file_written_last() {
    let read = |steam: &tempfile::TempDir| collections(&[steam.path().to_owned()]).apps[&1].clone();
    assert_eq!(
        read(&steam_with_two_accounts(Some(1))),
        ["Old"],
        "the mark beats the file's date"
    );
    assert_eq!(
        read(&steam_with_two_accounts(None)),
        ["New"],
        "no mark, as on some PCs"
    );
    assert_eq!(
        read(&steam_with_two_accounts(Some(9))),
        ["New"],
        "the marked account has no file"
    );
}

#[test]
fn a_steam_file_over_the_limit_is_a_problem_and_never_read() {
    let steam = steam_with_collections("1", "[]");
    let file = steam
        .path()
        .join("userdata/1/config/cloudstorage")
        .join(COLLECTIONS_FILE);
    let grow = |file: &Path, size: u64| {
        std::fs::File::options()
            .write(true)
            .create(true)
            .truncate(false)
            .open(file)
            .unwrap()
            .set_len(size)
            .unwrap();
    };
    grow(&file, MAX_STEAM_TEXT + 1);
    let read = collections(&[steam.path().to_owned()]);
    assert!(read.failed && read.apps.is_empty());
    assert!(
        read.problem.as_deref().is_some_and(|p| p.contains("bytes")),
        "{:?}",
        read.problem
    );
    // Play history too: the game stays unknown, as without the file.
    let history = steam.path().join("userdata/1/config/localconfig.vdf");
    grow(&history, MAX_STEAM_TEXT + 1);
    let mut games = BTreeMap::from([(
        570,
        Game {
            appid: 570,
            ..Default::default()
        },
    )]);
    apply_history(steam.path(), &mut games);
    assert!(!games[&570].history);
}

/// A format Steam changed, or a file past the limit, fails every scan: the old names go.
#[test]
fn collections_are_kept_through_a_few_failed_scans_and_no_longer() {
    let good = r#"[["user-collections.a",{"value":"{\"name\":\"VR\",\"added\":[1]}"}]]"#;
    let steam = steam_with_collections("1", good);
    let roots = [steam.path().to_owned()];
    let mut last = LastCollections::default();
    last.keeping(collections(&roots));
    write_collections(steam.path(), "1", "[[\"user-coll");
    for _ in 1..MAX_FAILED_COLLECTION_READS {
        assert_eq!(last.keeping(collections(&roots)).apps[&1], ["VR"]);
    }
    let gone = last.keeping(collections(&roots));
    assert!(gone.failed && gone.apps.is_empty());
    assert!(gone.problem_line().contains("has none"));
    assert!(
        last.keeping(collections(&roots)).apps.is_empty(),
        "stays gone"
    );
    // A good read starts the count over.
    write_collections(steam.path(), "1", good);
    assert_eq!(last.keeping(collections(&roots)).apps[&1], ["VR"]);
    write_collections(steam.path(), "1", "[[\"user-coll");
    assert_eq!(last.keeping(collections(&roots)).apps[&1], ["VR"]);
}

#[test]
fn a_failed_read_keeps_the_last_collections_until_the_file_reads_again() {
    let good = |name: &str| {
        format!(r#"[["user-collections.a",{{"value":"{{\"name\":\"{name}\",\"added\":[1]}}"}}]]"#)
    };
    let steam = steam_with_collections("1", &good("VR"));
    let roots = [steam.path().to_owned()];
    let mut last = LastCollections::default();
    assert_eq!(last.keeping(collections(&roots)).apps[&1], ["VR"]);
    // Caught half written: the groups stay, and the line still tells.
    write_collections(steam.path(), "1", "[[\"user-coll");
    let kept = last.keeping(collections(&roots));
    assert_eq!(kept.apps[&1], ["VR"]);
    assert!(kept.failed && kept.problem.is_some());
    assert!(kept.problem_line().contains("keeps the last ones"));
    // Read again, it says what the file says now, also that a game left a collection.
    write_collections(steam.path(), "1", &good("Couch"));
    assert_eq!(last.keeping(collections(&roots)).apps[&1], ["Couch"]);
    write_collections(steam.path(), "1", "[]");
    assert!(last.keeping(collections(&roots)).apps.is_empty());
    // Another account's file that fails gets nothing of this one's.
    write_collections(steam.path(), "1", &good("VR"));
    last.keeping(collections(&roots));
    let other = steam_with_collections("2", "{ not json");
    let read = last.keeping(collections(&[other.path().to_owned()]));
    assert!(read.failed && read.apps.is_empty());
    // No file at all: nothing to keep from.
    last.keeping(collections(&roots));
    assert_eq!(
        last.keeping(collections(&[])),
        Collections::default(),
        "the file is gone"
    );
}

#[test]
fn roots_off_this_pc_are_not_opened_for_collections_or_games() {
    let steam = tempfile::tempdir().unwrap();
    let local = steam.path().to_owned();
    let share = PathBuf::from(r"\\nas\games\Steam");
    assert_eq!(only_on_this_pc(&[share.clone(), local.clone()]), [local]);
    assert_eq!(
        collections(std::slice::from_ref(&share)),
        Collections::default()
    );
    assert!(discover(&[share]).is_empty());
}

#[test]
fn the_library_stamp_moves_when_a_games_folder_comes_or_goes() {
    let steam = tempfile::tempdir().unwrap();
    let roots = [steam.path().to_owned()];
    let common = steam.path().join("steamapps").join("common");
    std::fs::create_dir_all(&common).unwrap();
    let before = library_stamp(&roots);
    // No manifest changes: only the folder's time can tell. Past the clock's tick.
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::create_dir(common.join("Dota 2")).unwrap();
    let added = library_stamp(&roots);
    assert_ne!(added, before);
    assert_eq!(library_stamp(&roots), added, "nothing since");
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::remove_dir(common.join("Dota 2")).unwrap();
    assert_ne!(library_stamp(&roots), added);
}

#[test]
fn no_steam_is_no_problem_and_an_account_without_the_file_is_one() {
    let empty = tempfile::tempdir().unwrap();
    assert_eq!(
        collections(&[empty.path().to_owned()]),
        Collections::default()
    );
    assert_eq!(collections(&[]), Collections::default());
    std::fs::create_dir_all(empty.path().join("userdata").join("1")).unwrap();
    let read = collections(&[empty.path().to_owned()]);
    assert!(read.apps.is_empty());
    assert!(read.problem.is_some());
}

#[test]
fn the_library_stamp_moves_for_installs_and_collections_but_not_for_a_download() {
    let steam = steam_with_collections("1", "[]");
    let roots = [steam.path().to_owned()];
    let apps = steam.path().join("steamapps");
    std::fs::create_dir_all(apps.join("common")).unwrap();
    let first = library_stamp(&roots);
    assert_eq!(library_stamp(&roots), first, "nothing changed");
    let manifest = apps.join("appmanifest_570.acf");
    std::fs::write(&manifest, "\"AppState\" { \"appid\" \"570\" }").unwrap();
    let installed = library_stamp(&roots);
    assert_ne!(installed, first, "a new manifest");
    std::fs::write(&manifest, "\"AppState\" { \"BytesDownloaded\" \"1\" }").unwrap();
    assert_eq!(library_stamp(&roots), installed, "a download's progress");
    write_collections(steam.path(), "1", "[1]");
    let edited = library_stamp(&roots);
    assert_ne!(edited, installed, "the collections file");
    std::fs::remove_file(&manifest).unwrap();
    assert_ne!(library_stamp(&roots), edited, "an uninstall");
    assert_eq!(library_stamp(&[]), library_stamp(&[]), "no Steam at all");
}
