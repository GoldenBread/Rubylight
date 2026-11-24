//! Held-DLL removal, interrupted install/update recovery, and NTFS link safety tests for Neural Rendering.
use super::common::*;
use super::common::{install, remove};
use butterpollo_session::neural::{MANIFEST, Manifest, Mode};
use butterpollo_session::state::App;
use butterpollo_stream::neural::folder::{Request, Response, Writer, handle};
use butterpollo_stream::neural::*;

/// A game that runs outside a stream holds its DLLs: turning frame
/// generation off then stops at the first, before anything changed, and
/// says to close the game, so the game still launches with OptiScaler.ini
/// beside it. The same for the provider in the user's own OptiScaler, whose
/// ini keeps the three values while the provider stays.
#[test]
fn a_removal_while_the_game_runs_changes_nothing_and_says_to_close_it() {
    let temp = tempdir();
    let cache = release(temp.path(), "FG-V.B16", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let before = snapshot(folder);
    let Response::Placed { proxy, .. } = install(Some(&exe), None, &cache) else {
        panic!()
    };
    let placed = snapshot(folder);
    let held = hold(&folder.join(&proxy));
    let Response::Error { message } = remove(&exe) else {
        panic!()
    };
    assert!(message.contains("close the game"), "{message}");
    assert_eq!(snapshot(folder), placed);
    assert!(matches!(
        prepare(&exe, None),
        Response::Placed { updated: false, .. }
    ));
    drop(held);
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
    assert_eq!(snapshot(folder), before);

    // The add-on: the provider's DLL is what the game holds.
    std::fs::write(
        folder.join("dxgi.dll"),
        b"MZ OptiScaler ... Custom DLSSG ...",
    )
    .unwrap();
    std::fs::write(
        folder.join("OptiScaler.ini"),
        b"[FrameGen]\r\nFGInput=auto\r\n",
    )
    .unwrap();
    let before = snapshot(folder);
    let Response::Placed {
        mode: Mode::Addon, ..
    } = install(Some(&exe), None, &cache)
    else {
        panic!()
    };
    let placed = snapshot(folder);
    let held = hold(&folder.join("custom/Apex Neural Rendering FG/dlssg/nvngx_dlssg.dll"));
    let Response::Error { message } = remove(&exe) else {
        panic!()
    };
    assert!(message.contains("close the game"), "{message}");
    assert_eq!(snapshot(folder), placed);
    drop(held);
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
    assert_eq!(snapshot(folder), before);
}

/// A DLL the game holds anywhere in the removal stops it before any DLL
/// goes: in the full package one after OptiScaler's proxy, and in add-on
/// mode, with no proxy of ours to go first, one provider DLL of several,
/// so the user's OptiScaler.ini never points at a half-removed provider.
#[test]
fn a_held_dll_stops_the_removal_before_any_dll_goes() {
    let temp = tempdir();
    let (pdll, pini, pak) = (
        format!("full/game/{PDLL}"),
        format!("full/game/{PINI}"),
        format!("full/game/{PAK}"),
    );
    let cache = package(
        temp.path(),
        "FG-V.B16",
        &[
            ("full/game/dxgi.dll", b"OptiScaler proxy"),
            ("full/game/OptiScaler.ini", b"[FrameGen]\nFGInput=nvngxfg\n"),
            ("full/game/OptiScaler/libxess.dll", b"xess"),
            (&pdll, b"provider"),
            (&pini, b"path = auto\n"),
            (&pak, b"model"),
            ("full/LICENSES/OptiScaler-GPL-3.0.txt", b"GPL"),
            (
                "addon/Apex Neural Rendering FG/dlssg/aaa_helper.dll",
                b"addon helper",
            ),
            (
                "addon/Apex Neural Rendering FG/dlssg/nvngx_dlssg.dll",
                b"addon provider",
            ),
            (
                "addon/Apex Neural Rendering FG/dlssg/nr_fg.ini",
                b"path = auto\n",
            ),
        ],
    );
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let Response::Placed { .. } = install(Some(&exe), None, &cache) else {
        panic!()
    };
    let placed = snapshot(folder);
    let held = hold(&folder.join(PDLL));
    let Response::Error { message } = remove(&exe) else {
        panic!()
    };
    assert!(message.contains("close the game"), "{message}");
    assert_eq!(snapshot(folder), placed);
    drop(held);
    assert!(matches!(remove(&exe), Response::Removed { .. }));

    std::fs::write(
        folder.join("dxgi.dll"),
        b"MZ OptiScaler ... Custom DLSSG ...",
    )
    .unwrap();
    std::fs::write(
        folder.join("OptiScaler.ini"),
        b"[FrameGen]\r\nFGInput=auto\r\n",
    )
    .unwrap();
    let before = snapshot(folder);
    let Response::Placed {
        mode: Mode::Addon, ..
    } = install(Some(&exe), None, &cache)
    else {
        panic!()
    };
    let dlssg = folder.join("custom/Apex Neural Rendering FG/dlssg");
    assert!(dlssg.join("aaa_helper.dll").is_file());
    let placed = snapshot(folder);
    let held = hold(&dlssg.join("nvngx_dlssg.dll"));
    let Response::Error { message } = remove(&exe) else {
        panic!()
    };
    assert!(message.contains("close the game"), "{message}");
    assert_eq!(snapshot(folder), placed);
    drop(held);
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
    assert_eq!(snapshot(folder), before);
}

/// An update cut short (a file the game holds open) leaves each file it
/// replaces in one of its two forms, both the host's: the game does not
/// launch on half an update, the next launch with the release cached
/// finishes it, and turning the switch off takes every file, the mod's DLL
/// included.
#[test]
fn an_update_cut_short_leaves_nothing_a_removal_misses() {
    let temp = tempdir();
    let old = release(temp.path(), "Alpha0.3.5.7", b"OptiScaler old");
    let new = package(
        &temp.path().join("new"),
        "Alpha0.3.5.8",
        &[
            ("full/game/dxgi.dll", b"OptiScaler new"),
            ("full/game/OptiScaler.ini", b"[FrameGen]\n"),
            (&format!("full/game/{PDLL}"), b"provider new"),
            (&format!("full/game/{PAK}"), b"model"),
            ("full/game/OptiScaler/libxess.dll", b"xess new"),
        ],
    );
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let before = names(folder);
    let read = |path: &str| std::fs::read(folder.join(path)).unwrap();
    for finish in [false, true] {
        assert!(matches!(
            install(Some(&exe), None, &old),
            Response::Placed { .. }
        ));
        let held = hold(&folder.join("OptiScaler/libxess.dll"));
        let Response::Error { message } = install(Some(&exe), None, &new) else {
            panic!()
        };
        drop(held);
        assert!(message.contains("libxess.dll"), "{message}");
        // The provider is the new release's, OptiScaler still the old
        // one's (it goes last), and no partial copy stayed.
        assert_eq!(read(PDLL), b"provider new");
        assert_eq!(read("winmm.dll"), b"OptiScaler old");
        assert_eq!(read("OptiScaler/libxess.dll"), b"xess");
        assert!(!names(folder).iter().any(|name| name.ends_with("-partial")));
        let Response::Error { message } = prepare(&exe, None) else {
            panic!()
        };
        assert!(message.contains("cut short"), "{message}");
        if finish {
            assert_eq!(
                prepare(&exe, Some(&new)),
                Response::Placed {
                    exe: exe.clone(),
                    release: "Alpha0.3.5.8".into(),
                    proxy: "winmm.dll".into(),
                    updated: true,
                    mode: Mode::Full,
                }
            );
            assert_eq!(read("winmm.dll"), b"OptiScaler new");
            assert_eq!(read("OptiScaler/libxess.dll"), b"xess new");
            let manifest =
                Manifest::parse(&std::fs::read_to_string(folder.join(MANIFEST)).unwrap()).unwrap();
            assert!(manifest.previous.is_empty());
            assert!(matches!(
                prepare(&exe, Some(&new)),
                Response::Placed { updated: false, .. }
            ));
        }
        assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
        assert_eq!(names(folder), before);
    }
}

/// A first install that fails at any step, its last manifest write
/// included, takes back every file and folder it made.
#[test]
fn a_first_install_cut_short_leaves_nothing_behind() {
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    // A folder where the manifest is written first: every write of it fails.
    let blocker = folder.join(format!("{MANIFEST}.rubylight-partial"));
    std::fs::create_dir(&blocker).unwrap();
    let before = names(folder);
    let Response::Error { message } = install(Some(&exe), None, &cache) else {
        panic!()
    };
    assert!(message.contains(MANIFEST), "{message}");
    assert_eq!(names(folder), before);

    // The first manifest is written; the last fails once every file is copied.
    std::fs::remove_dir(&blocker).unwrap();
    let before = names(folder);
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
    assert_eq!(names(folder), before);
}

/// A link in the game's folder, or the folder itself a link, sends neither
/// a copy nor a delete elsewhere.
#[test]
fn nothing_goes_through_a_link_in_the_games_folder() {
    let temp = tempdir();
    let old = release(temp.path(), "Alpha0.3.5.7", b"OptiScaler old");
    let new = release(&temp.path().join("new"), "Alpha0.3.5.8", b"OptiScaler new");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let before = names(folder);
    assert!(matches!(
        install(Some(&exe), None, &old),
        Response::Placed { .. }
    ));
    // The mod's folder swapped for a junction to one with the same file.
    let elsewhere = temp.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("libxess.dll"), "xess").unwrap();
    std::fs::remove_dir_all(folder.join("OptiScaler")).unwrap();
    junction(&folder.join("OptiScaler"), &elsewhere);
    let Response::Refused { reason } = remove(&exe) else {
        panic!()
    };
    assert!(reason.contains("link"), "{reason}");
    let Response::Refused { reason } = install(Some(&exe), None, &new) else {
        panic!()
    };
    assert!(reason.contains("link"), "{reason}");
    assert_eq!(names(&elsewhere), ["libxess.dll"]);
    assert_eq!(
        std::fs::read(elsewhere.join("libxess.dll")).unwrap(),
        b"xess"
    );
    std::fs::remove_dir(folder.join("OptiScaler")).unwrap();
    assert_eq!(remove(&exe), Response::Removed { kept: vec![] });
    assert_eq!(names(folder), before);

    // The game's folder itself a link (moved and linked back with `mklink
    // /J`): the user's helper follows it once and works where it leads, so
    // the switch goes on and off; a SYSTEM host follows none.
    let linked = temp.path().join("library/common/Linked");
    junction(&linked, folder);
    let through = linked.join("Game.exe");
    let Response::Refused { reason } = handle(
        Request::Install {
            exe: Some(through.clone()),
            install: None,
            package: cached_at(&old),
        },
        Writer::System,
        &|_, _| {},
    ) else {
        panic!("a SYSTEM host wrote through a link")
    };
    assert!(reason.contains("link"), "{reason}");
    assert_eq!(names(folder), before);
    let Response::Placed { exe: placed, .. } = install(Some(&through), None, &old) else {
        panic!("the moved game's folder was refused")
    };
    assert_eq!(names(placed.parent().unwrap()), names(folder));
    assert_ne!(names(folder), before);
    assert!(matches!(
        prepare(&through, Some(&new)),
        Response::Placed { .. }
    ));
    assert_eq!(remove(&through), Response::Removed { kept: vec![] });
    std::fs::remove_dir(&linked).unwrap();
    assert_eq!(names(folder), before);
}

/// A game folder moved and linked back whose link leads nowhere now (the
/// drive it went to is gone for good) is unreachable, not refused: the
/// app can still be deleted, its removal pending.
#[test]
fn a_link_that_leads_nowhere_is_unreachable() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    let temp = tempdir();
    let moved = temp.path().join("moved");
    std::fs::create_dir(&moved).unwrap();
    let linked = temp.path().join("library/common/Linked");
    std::fs::create_dir_all(linked.parent().unwrap()).unwrap();
    junction(&linked, &moved);
    std::fs::remove_dir(&moved).unwrap();
    let exe = linked.join("Game.exe");
    assert_eq!(
        remove(&exe),
        Response::Unreachable {
            folder: linked.display().to_string()
        }
    );
    let app: App = serde_json::from_value(switched_on("Game", "game", &exe)).unwrap();
    drop(before_delete(h, &app).unwrap());
    assert_eq!(pending(h), Pending::of(&app));
    std::fs::remove_dir(&linked).unwrap();
}

/// A game folder moved and linked back: the program is found where the link
/// leads, as the install places it, and a removal recorded through the link
/// (an update that failed) leaves the mod its app still has on.
#[test]
fn a_linked_games_stale_removal_leaves_the_mod_its_app_has_on() {
    let f = butterpollo_stream::state::test_support::Fixture::new();
    let h = &f.host;
    let temp = tempdir();
    let cache = release(temp.path(), "Alpha0.3.5.8", b"OptiScaler proxy");
    let exe = game(temp.path());
    let folder = exe.parent().unwrap();
    let before = names(folder);
    let linked = temp.path().join("library/common/Linked");
    junction(&linked, folder);
    let through = linked.join("Game.exe");
    let found = handle(
        Request::Find {
            exe: Some(through.clone()),
            install: Some(linked.clone()),
        },
        Writer::User,
        &|_, _| {},
    );
    let Response::Found { exe: real } = found else {
        panic!("{found:?}")
    };
    assert_eq!(
        butterpollo_stream::neural::folder::real(folder).unwrap(),
        real.parent().unwrap()
    );
    // The install takes the program found, in the linked install.
    let Response::Placed { exe: placed, .. } = install(Some(&real), Some(&linked), &cache) else {
        panic!("the program found was refused")
    };
    assert_eq!(placed, real);
    butterpollo_stream::steam::update_library(h, |apps, _| {
        apps.push(switched_on("Game", "game", &placed));
        true
    })
    .unwrap();
    // A record from before, through the link.
    record_pending(
        h,
        &[Pending {
            name: "Game".into(),
            exe: through.clone(),
            uuid: Some("game".into()),
        }],
    )
    .unwrap();
    remove_pending(h);
    assert_ne!(names(folder), before, "the mod its app has on was removed");
    assert!(pending(h).is_empty());
    assert_eq!(remove(&placed), Response::Removed { kept: vec![] });
    std::fs::remove_dir(&linked).unwrap();
    assert_eq!(names(folder), before);
}
