//! Console mod-name redaction and provider leftover, takeover, and settings preservation tests for Neural Rendering.
use super::common::*;
use super::common::{install, remove};
use butterpollo_session::neural::{self, MANIFEST, Manifest, Mode};
use butterpollo_stream::neural::folder::{Request, Response, Writer, handle};
use butterpollo_stream::neural::*;
use butterpollo_stream::state::Shared;
use serde_json::json;
use std::path::Path;

/// Every text the console or a client shows of a failure or of what stayed
/// behind names no mod: the provider's folder shows as its file's name.
#[tokio::test(flavor = "multi_thread")]
async fn no_text_the_console_shows_names_the_mod() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    let temp = tempdir();
    let cache = release(temp.path(), "FG-V.B16", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    assert!(matches!(
        install(Some(&exe), None, &cache),
        Response::Placed { .. }
    ));
    butterpollo_stream::steam::update_library(h, |apps, _| {
        apps.push(json!({
            "name": "Game",
            "uuid": "game",
            KEY: {"exe": exe, "release": "FG-V.B16", "proxy": "winmm.dll", "mode": "full"},
        }));
        true
    })
    .unwrap();
    // A launch's check finds the provider changed, then the removal keeps it.
    std::fs::write(folder.join(PDLL), "someone else's").unwrap();
    let Response::Error { message } = prepare(&exe, Some(&cache)) else {
        panic!()
    };
    assert!(
        message.contains(neural::PROVIDER),
        "the log has the path: {message}"
    );
    fail_job_for_test(h, "game", message);
    disable(h, "game").unwrap();
    let job = ended(h, "game").await;
    assert_eq!(job.phase, "done", "{job:?}");
    assert_eq!(job.kept, ["frame generation's nvngx_dlssg.dll"]);
    // And a refusal over the provider's folder.
    std::fs::remove_dir_all(folder.join("OptiScaler")).unwrap();
    std::fs::write(
        folder.join("dxgi.dll"),
        b"MZ OptiScaler ... Custom DLSSG ...",
    )
    .unwrap();
    std::fs::write(folder.join("OptiScaler.ini"), b"[FrameGen]\n").unwrap();
    // The user's own add-on in it: its files are named, not the mod.
    let dlssg = format!("custom/{}/dlssg", neural::PROVIDER);
    std::fs::create_dir_all(folder.join(&dlssg)).unwrap();
    std::fs::write(folder.join(format!("{dlssg}/nvngx_dlssg.dll")), b"theirs").unwrap();
    std::fs::write(folder.join(format!("{dlssg}/nr_fg.ini")), b"theirs").unwrap();
    let Response::Refused { reason } = install(Some(&exe), None, &cache) else {
        panic!()
    };
    fail_job_for_test(h, "game", reason);
    let shown = serde_json::to_string(&status(h, "game")).unwrap();
    assert!(!shown.contains("Apex") && !shown.contains("ANR"), "{shown}");
    assert!(
        shown.contains("frame generation's nvngx_dlssg.dll is still in the game's folder from an earlier install: move it out first"),
        "{shown}"
    );
    // A file where the provider's folder goes.
    std::fs::remove_dir_all(folder.join("custom")).unwrap();
    std::fs::create_dir(folder.join("custom")).unwrap();
    std::fs::write(
        folder.join(format!("custom/{}", neural::PROVIDER)),
        b"a file",
    )
    .unwrap();
    let Response::Refused { reason } = install(Some(&exe), None, &cache) else {
        panic!()
    };
    fail_job_for_test(h, "game", reason);
    let shown = serde_json::to_string(&status(h, "game")).unwrap();
    assert!(!shown.contains("Apex") && !shown.contains("ANR"), "{shown}");
    assert!(shown.contains("frame generation's folder"), "{shown}");
}

/// A job that failed with `error`, as an install or a launch's check ends.
fn fail_job_for_test(h: &Shared, uuid: &str, error: String) {
    start_job(h, uuid, "update", "place").unwrap();
    fail_job(h, uuid, error, false);
}

/// The provider's folder as the full package places it.
const DLSSG: &str = "OptiScaler/custom/Apex Neural Rendering FG/dlssg";

/// What the provider writes into its folder in `dlssg` once a game runs
/// it, as FG-V.B16 did in Final Fantasy XVI: its log and its copy of the
/// system's Vulkan loader.
fn provider_ran(folder: &Path, dlssg: &str) {
    std::fs::write(
        folder.join(format!("{dlssg}/anr-vkloader.dll")),
        b"MZ vulkan-1.dll",
    )
    .unwrap();
    std::fs::write(
        folder.join(format!("{dlssg}/nr_fg-20261115-063120.log")),
        b"[info] frame generation feature: 5120x1440",
    )
    .unwrap();
}

fn manifest_of(folder: &Path) -> Manifest {
    Manifest::parse(&std::fs::read_to_string(folder.join(MANIFEST)).unwrap()).unwrap()
}

#[test]
fn what_the_provider_wrote_goes_with_it_and_anything_else_left_is_named() {
    let temp = tempdir();
    let cache = release(temp.path(), "FG-V.B16", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let game_files = snapshot(folder);

    // A game ran it: removal leaves the folder byte for byte as it was.
    assert!(matches!(
        install(Some(&exe), None, &cache),
        Response::Placed { .. }
    ));
    provider_ran(folder, DLSSG);
    std::fs::write(folder.join("OptiScaler.log"), b"").unwrap();
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
    assert_eq!(snapshot(folder), game_files);

    // The same name anywhere but the provider's folder is not the
    // provider's: the game's own beside it stays and is no concern of the
    // removal; one in OptiScaler's folder, another provider's folder (named
    // once, not looked into) and a file of someone's in the provider's
    // folder stay and are named.
    assert!(matches!(
        install(Some(&exe), None, &cache),
        Response::Placed { .. }
    ));
    provider_ran(folder, DLSSG);
    std::fs::write(folder.join("anr-vkloader.dll"), b"the game's").unwrap();
    std::fs::write(folder.join("OptiScaler/anr-vkloader.dll"), b"someone's").unwrap();
    let other = "OptiScaler/custom/Other FG/dlssg";
    std::fs::create_dir_all(folder.join(other)).unwrap();
    std::fs::write(
        folder.join(format!("{other}/anr-vkloader.dll")),
        b"another provider's",
    )
    .unwrap();
    std::fs::write(folder.join(format!("{DLSSG}/notes.txt")), b"mine").unwrap();
    assert_eq!(
        remove(&exe),
        Response::Removed {
            kept: vec![
                "OptiScaler/anr-vkloader.dll".into(),
                format!("{DLSSG}/notes.txt"),
                "OptiScaler/custom/Other FG".into(),
            ]
        }
    );
    let notes = format!("{DLSSG}/notes.txt");
    let others = format!("{other}/anr-vkloader.dll");
    let mut expected: Vec<String> = game_files.iter().map(|(name, _)| name.clone()).collect();
    expected.extend(
        [
            "anr-vkloader.dll",
            "OptiScaler",
            "OptiScaler/anr-vkloader.dll",
            "OptiScaler/custom",
            "OptiScaler/custom/Apex Neural Rendering FG",
            DLSSG,
            notes.as_str(),
            "OptiScaler/custom/Other FG",
            other,
            others.as_str(),
        ]
        .map(|name: &str| name.to_owned()),
    );
    expected.sort();
    assert_eq!(names(folder), expected);
    for (path, bytes) in [
        ("anr-vkloader.dll", &b"the game's"[..]),
        ("OptiScaler/anr-vkloader.dll", b"someone's"),
        (others.as_str(), b"another provider's"),
    ] {
        assert_eq!(std::fs::read(folder.join(path)).unwrap(), bytes, "{path}");
    }
    // Its record is gone with its files: nothing more to remove.
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
}

#[test]
fn turning_on_over_what_a_removal_left_takes_it_over_or_names_what_is_in_the_way() {
    let temp = tempdir();
    let cache = release(temp.path(), "FG-V.B16", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let game_files = snapshot(folder);
    let provider = "OptiScaler/custom/Apex Neural Rendering FG";

    // What a removal before this release left in Final Fantasy XVI: the
    // provider's Vulkan loader copy in the folders the host had made, and no
    // record of them. Turning on takes them over, parents first, and the
    // next removal leaves nothing.
    std::fs::create_dir_all(folder.join(DLSSG)).unwrap();
    std::fs::write(
        folder.join(format!("{DLSSG}/anr-vkloader.dll")),
        b"MZ vulkan-1.dll",
    )
    .unwrap();
    assert!(matches!(
        install(Some(&exe), None, &cache),
        Response::Placed { .. }
    ));
    let folders = manifest_of(folder).folders;
    let at = |relative: &str| {
        folders
            .iter()
            .position(|f| f == relative)
            .unwrap_or_else(|| panic!("{relative} not in {folders:?}"))
    };
    assert!(at("OptiScaler") < at("OptiScaler/custom"));
    assert!(at("OptiScaler/custom") < at(provider) && at(provider) < at(DLSSG));
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
    assert_eq!(snapshot(folder), game_files);

    // A removal kept a file of someone's in the provider's folder: turning
    // on names it and changes nothing.
    assert!(matches!(
        install(Some(&exe), None, &cache),
        Response::Placed { .. }
    ));
    provider_ran(folder, DLSSG);
    std::fs::write(folder.join(format!("{DLSSG}/notes.txt")), b"mine").unwrap();
    assert_eq!(
        remove(&exe),
        Response::Removed {
            kept: vec![format!("{DLSSG}/notes.txt")]
        }
    );
    let left = snapshot(folder);
    assert_eq!(
        install(Some(&exe), None, &cache),
        Response::Refused {
            reason: format!(
                "{DLSSG}/notes.txt is still in the game's folder from an earlier install: move it out first"
            )
        }
    );
    assert_eq!(snapshot(folder), left);
    // So does the provider's file name anywhere but its folder.
    std::fs::write(folder.join("OptiScaler/anr-vkloader.dll"), b"someone's").unwrap();
    let Response::Refused { reason } = install(Some(&exe), None, &cache) else {
        panic!()
    };
    assert_eq!(
        reason,
        format!(
            "OptiScaler/anr-vkloader.dll is in the game's folder and is not Rubylight's; {DLSSG}/notes.txt is still in the game's folder from an earlier install: move them out first"
        )
    );
    assert!(!folder.join(MANIFEST).exists());
    // Once they are out, the empty folders are taken over.
    std::fs::remove_file(folder.join("OptiScaler/anr-vkloader.dll")).unwrap();
    std::fs::remove_file(folder.join(format!("{DLSSG}/notes.txt"))).unwrap();
    assert!(matches!(
        install(Some(&exe), None, &cache),
        Response::Placed { .. }
    ));
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
    assert_eq!(snapshot(folder), game_files);

    // A link in it is never taken over, nor anything done through it.
    let elsewhere = temp.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::fs::create_dir_all(folder.join(provider)).unwrap();
    junction(&folder.join(DLSSG), &elsewhere);
    let Response::Refused { reason } = install(Some(&exe), None, &cache) else {
        panic!()
    };
    assert_eq!(
        reason,
        format!("{DLSSG} is still in the game's folder from an earlier install: move it out first")
    );
    assert_eq!(names(&elsewhere), Vec::<String>::new());
    assert!(!folder.join(MANIFEST).exists());
    std::fs::remove_dir(folder.join(DLSSG)).unwrap();
    std::fs::remove_dir_all(folder.join("OptiScaler")).unwrap();
    assert_eq!(snapshot(folder), game_files);
}

#[test]
fn the_users_optiscaler_with_a_left_provider_folder_gets_it_taken_over() {
    let temp = tempdir();
    let cache = release(temp.path(), "FG-V.B16", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    std::fs::write(
        folder.join("dxgi.dll"),
        b"MZ OptiScaler ... Custom DLSSG ...",
    )
    .unwrap();
    std::fs::write(
        folder.join("OptiScaler.ini"),
        b"[FrameGen]\r\nFGInput=auto\r\nDisableHudless=auto\r\n",
    )
    .unwrap();
    std::fs::create_dir(folder.join("OptiScaler")).unwrap();
    std::fs::write(folder.join("OptiScaler/libxess.dll"), b"the user's").unwrap();
    let users = snapshot(folder);
    // An earlier add-on's folder, with what the provider wrote in it.
    let provider = "OptiScaler/custom/Apex Neural Rendering FG";
    std::fs::create_dir_all(folder.join(DLSSG)).unwrap();
    provider_ran(folder, DLSSG);

    assert_eq!(
        install(Some(&exe), None, &cache),
        Response::Placed {
            exe: exe.clone(),
            release: "FG-V.B16".into(),
            proxy: "dxgi.dll".into(),
            updated: false,
            mode: Mode::Addon,
        }
    );
    let folders = manifest_of(folder).folders;
    assert!(
        folders.iter().any(|f| f == provider) && folders.iter().any(|f| f == DLSSG),
        "{folders:?}"
    );
    // The custom folder above it is the user's OptiScaler's: never taken.
    assert!(
        !folders.iter().any(|f| f == "OptiScaler/custom"),
        "{folders:?}"
    );
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
    // The user's OptiScaler as it was, its ini back byte for byte; only the
    // empty custom folder the earlier add-on made is left.
    let mut expected = users.clone();
    expected.push(("OptiScaler/custom".into(), None));
    expected.sort();
    assert_eq!(snapshot(folder), expected);
}

/// A folder open in a program (read and write sharing, no delete sharing):
/// it can't be removed until dropped.
fn hold_folder(path: &Path) -> std::fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(3)
        // FILE_FLAG_BACKUP_SEMANTICS: a handle on a folder.
        .custom_flags(0x0200_0000)
        .open(path)
        .unwrap()
}

#[test]
fn a_folder_of_the_hosts_that_cannot_go_is_named_and_taken_over_later() {
    let temp = tempdir();
    let cache = release(temp.path(), "FG-V.B16", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let game_files = snapshot(folder);
    assert!(matches!(
        install(Some(&exe), None, &cache),
        Response::Placed { .. }
    ));
    provider_ran(folder, DLSSG);
    // A program holds the provider's folder open: its files go, the empty
    // folder stays, and the removal names it instead of saying nothing.
    let held = hold_folder(&folder.join(DLSSG));
    assert_eq!(
        remove(&exe),
        Response::Removed {
            kept: vec![DLSSG.into()]
        }
    );
    assert_eq!(names(&folder.join(DLSSG)), Vec::<String>::new());
    assert_eq!(
        neural::shown(DLSSG),
        "frame generation's dlssg",
        "the console names no mod"
    );
    drop(held);
    // Turned on again, the empty folders are taken over and go with the
    // next removal.
    assert!(matches!(
        install(Some(&exe), None, &cache),
        Response::Placed { .. }
    ));
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
    assert_eq!(snapshot(folder), game_files);
}

#[test]
fn what_an_earlier_install_left_is_named_as_such_and_its_settings_are_kept() {
    let temp = tempdir();
    let cache = release(temp.path(), "FG-V.B16", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let game_files = snapshot(folder);
    // A removal keeps a placed file changed since (the provider's model)
    // and takes the provider's settings however they changed.
    assert!(matches!(
        install(Some(&exe), None, &cache),
        Response::Placed { .. }
    ));
    std::fs::write(folder.join(PAK), b"retrained").unwrap();
    std::fs::write(folder.join(PINI), b"path = gpu\n").unwrap();
    assert_eq!(
        remove(&exe),
        Response::Removed {
            kept: vec![PAK.into()]
        }
    );
    assert!(!folder.join(PINI).exists());
    // Turning on again says the model is still there from that install,
    // not that it is someone else's, and the console names no mod.
    let Response::Refused { reason } = install(Some(&exe), None, &cache) else {
        panic!()
    };
    assert_eq!(
        reason,
        format!("{PAK} is still in the game's folder from an earlier install: move it out first")
    );
    assert_eq!(
        neural::shown(&reason),
        "frame generation's pack.nrx is still in the game's folder from an earlier install: move it out first"
    );
    assert_eq!(
        std::fs::read(folder.join(PAK)).unwrap(),
        b"retrained",
        "nothing is deleted"
    );
    std::fs::remove_file(folder.join(PAK)).unwrap();

    // The provider's settings in such a folder can only be the user's (made
    // by hand, or left from their own install): taken over and used as they
    // are, named apart from the files Rubylight placed.
    std::fs::write(folder.join(PINI), b"path = gpu\n").unwrap();
    provider_ran(folder, DLSSG);
    let placed = install(Some(&exe), None, &cache);
    assert!(matches!(placed, Response::Placed { .. }), "{placed:?}");
    assert_eq!(std::fs::read(folder.join(PINI)).unwrap(), b"path = gpu\n");
    let manifest = manifest_of(folder);
    assert_eq!(manifest.taken_over, vec![PINI.to_string()]);
    assert!(!manifest.files.contains_key(PINI));
    assert_eq!(std::fs::read(folder.join(PAK)).unwrap(), b"model");
    // A launch's check and an update leave them so.
    assert!(matches!(
        prepare(&exe, Some(&cache)),
        Response::Placed { .. }
    ));
    let newer = release(temp.path(), "FG-V.B17", b"OptiScaler newer");
    let updated = install(Some(&exe), None, &newer);
    assert!(
        matches!(updated, Response::Placed { updated: true, .. }),
        "{updated:?}"
    );
    assert_eq!(std::fs::read(folder.join(PINI)).unwrap(), b"path = gpu\n");
    assert_eq!(manifest_of(folder).taken_over, vec![PINI.to_string()]);
    // A removal takes what Rubylight put there and leaves the user's
    // settings, named, with the folders around them.
    assert_eq!(
        remove(&exe),
        Response::Removed {
            kept: vec![PINI.into()]
        }
    );
    assert_eq!(std::fs::read(folder.join(PINI)).unwrap(), b"path = gpu\n");
    assert_eq!(names(&folder.join(DLSSG)), ["nr_fg.ini"]);
    // Turned on again, they are taken over again. Once they are gone a
    // launch says so, and a removal leaves the folder as it was.
    assert!(matches!(
        install(Some(&exe), None, &cache),
        Response::Placed { .. }
    ));
    assert_eq!(manifest_of(folder).taken_over, vec![PINI.to_string()]);
    std::fs::remove_file(folder.join(PINI)).unwrap();
    let Response::Error { message } = prepare(&exe, Some(&cache)) else {
        panic!()
    };
    assert!(message.contains("nr_fg.ini is missing"), "{message}");
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
    assert_eq!(snapshot(folder), game_files);
}

/// Settings the user left in such a folder, taken over by a first install
/// that is cut short, stay as they were.
#[test]
fn a_first_install_cut_short_leaves_the_settings_it_took_over() {
    let temp = tempdir();
    let cache = release(temp.path(), "FG-V.B16", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let game_files = snapshot(folder);
    let settings = folder.join(PINI);
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    std::fs::write(&settings, b"path = gpu\n").unwrap();
    // The first manifest is written; the last fails once every file is copied.
    let blocker = folder.join(format!("{MANIFEST}.rubylight-partial"));
    let response = handle(
        Request::Install {
            exe: Some(exe.clone()),
            install: None,
            package: cached_at(&cache),
        },
        Writer::User,
        &|done, total| {
            if done == total {
                let _ = std::fs::create_dir(&blocker);
            }
        },
    );
    let Response::Error { message } = response else {
        panic!("{response:?}")
    };
    assert!(message.contains(MANIFEST), "{message}");
    std::fs::remove_dir(&blocker).unwrap();
    assert_eq!(std::fs::read(&settings).unwrap(), b"path = gpu\n");
    assert!(!folder.join(MANIFEST).exists());
    // Turned on again, they are taken over as they are, and stay through
    // the removal.
    assert!(matches!(
        install(Some(&exe), None, &cache),
        Response::Placed { .. }
    ));
    assert_eq!(std::fs::read(&settings).unwrap(), b"path = gpu\n");
    assert_eq!(
        remove(&exe),
        Response::Removed {
            kept: vec![PINI.into()]
        }
    );
    assert_eq!(std::fs::read(&settings).unwrap(), b"path = gpu\n");
    std::fs::remove_dir_all(folder.join("OptiScaler")).unwrap();
    assert_eq!(snapshot(folder), game_files);
}
