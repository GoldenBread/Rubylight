//! Deferred game-folder removals for disconnected drives, dropped library
//! games, and interrupted installs.
use super::{
    PENDING, PENDING_RETRY, Pending, Removed, SETTINGS_FILE, folder,
    game_check::placed_exes,
    install::{Removing, remove_at},
    record::{settings, update_settings},
};
use crate::state::Shared;
use anyhow::{Context, Result};
use butterpollo_session::{neural, state::App};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::PoisonError,
    time::Instant,
};

/// The record's removals, none when it has no list yet. A list that can't
/// be read is an error, never taken for empty and written over.
fn pending_in(record: &Value) -> Result<Vec<Pending>> {
    match &record[PENDING] {
        Value::Null => Ok(Vec::new()),
        list => serde_json::from_value(list.clone())
            .with_context(|| format!("{SETTINGS_FILE}'s pending removals are damaged")),
    }
}

/// [`pending_in`] for a reader, empty and logged when it can't be read.
pub fn pending(h: &Shared) -> Vec<Pending> {
    pending_in(&settings(h)).unwrap_or_else(|error| {
        tracing::warn!(error = %format!("{error:#}"), "Neural Rendering: its pending removals can't be read");
        Vec::new()
    })
}

/// Take the files placed beside `exe` out again (true), or leave their
/// removal pending when that fails (false); Err only when neither worked.
/// Blocks.
pub(super) fn take_back(h: &Shared, uuid: &str, name: &str, exe: &Path) -> Result<bool> {
    let why = match remove_at(name, exe) {
        Ok(Removed::Done(_)) => return Ok(true),
        Ok(Removed::Unreachable(folder)) => format!("{folder} can't be reached"),
        Err(error) => format!("{error:#}"),
    };
    let pending = Pending {
        name: name.into(),
        exe: exe.into(),
        uuid: Some(uuid.into()),
    };
    record_pending(h, std::slice::from_ref(&pending)).with_context(|| {
        format!(
            "frame generation could not be taken out of {} ({why}), and its removal could not be saved",
            exe.parent().unwrap_or(exe).display()
        )
    })?;
    tracing::warn!(app = %name, reason = %why, "Neural Rendering: taking the files out again failed; their removal is pending");
    Ok(false)
}

/// Drop the removal beside `exe` from the record: the files are meant to be
/// there, or nothing was written.
pub fn clear_pending(h: &Shared, exe: &Path) {
    let saved = update_settings(h, |value| {
        let mut list = pending_in(value)?;
        list.retain(|removal| removal.exe != exe);
        value[PENDING] = json!(list);
        Ok(())
    });
    if let Err(error) = saved {
        tracing::warn!(error = %format!("{error:#}"), "Neural Rendering: the pending removals could not be saved");
    }
}

/// Add removals to the record (once each). If an existing entry for the same
/// executable has no UUID or belongs to a non-running app, claim it for the
/// incoming UUID so concurrent installs serialize on the live app's job slot.
pub fn record_pending(h: &Shared, removals: &[Pending]) -> Result<()> {
    update_settings(h, |value| {
        let mut list = pending_in(value)?;
        neural::merge_pending(&mut list, removals);
        for removal in removals {
            if let Some(uuid) = &removal.uuid
                && let Some(entry) = list.iter_mut().find(|old| old.exe == removal.exe)
                && entry.uuid.as_deref().is_none_or(|old| {
                    old != uuid.as_str() && !super::running(&h.neural.lock(), old)
                })
            {
                entry.uuid = Some(uuid.clone());
            }
        }
        value[PENDING] = json!(list);
        Ok(())
    })
}

/// The apps with files placed that a library change left out: an
/// uninstalled game the launcher sync dropped.
pub fn dropped(before: &[App], after: &[App]) -> Vec<App> {
    neural::dropped_apps(before, after)
}

/// Take the files out of the folders of apps the library sync dropped, as a
/// delete does. Each is recorded first; one that fails (a folder on a drive
/// that is not connected) stays recorded for [`remove_pending`]. Blocks.
pub fn remove_dropped(h: &Shared, apps: &[App]) {
    let removals: Vec<Pending> = apps.iter().flat_map(Pending::of).collect();
    if removals.is_empty() {
        return;
    }
    for removal in &removals {
        tracing::info!(app = %removal.name, "Neural Rendering: the library sync dropped the game; removing its files");
    }
    if let Err(error) = record_pending(h, &removals) {
        tracing::warn!(error = %format!("{error:#}"), "Neural Rendering: the pending removals could not be saved");
    }
    try_pending(h, &removals);
}

/// Whether the maintenance loop should try the pending removals again.
pub fn removal_due(h: &Shared) -> bool {
    {
        let mut state = h.neural.lock();
        if state.removal_at.is_some_and(|at| Instant::now() < at) {
            return false;
        }
        state.removal_at = Some(Instant::now() + PENDING_RETRY);
    }
    !pending(h).is_empty()
}

/// When the loop looks for pending removals again, for its idle sleep: the
/// retry cadence `removal_due` keeps, now, when none has been tried yet.
pub fn removal_due_at(h: &Shared) -> Instant {
    h.neural.lock().removal_at.unwrap_or_else(Instant::now)
}

/// Try the recorded removals again. Blocks.
pub fn remove_pending(h: &Shared) {
    try_pending(h, &[]);
}

/// Try the recorded removals and `also` (in case the record could not be
/// saved); keep the ones that are still to do.
pub fn try_pending(h: &Shared, also: &[Pending]) {
    try_pending_inner(h, also, || {});
}

fn try_pending_inner(h: &Shared, also: &[Pending], before_take: impl Fn()) {
    let _one = h
        .neural
        .removing
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let mut removals = pending(h);
    neural::merge_pending(&mut removals, also);
    before_take();
    let real = |exe: &Path| folder::real(exe.parent().unwrap_or(exe));
    let mut done = Vec::new();
    for removal in &removals {
        // Acquire the app's switch slot BEFORE inspecting live apps and
        // pending state: an install that finishes before the slot opens writes
        // `KEY` and calls `clear_pending` before transitioning its job to
        // `"done"`, so re-checking under the slot cannot race a fresh install.
        let Ok(slot) = Removing::take(h, removal.uuid.as_deref()) else {
            tracing::info!(app = %removal.name, "Neural Rendering is busy with the game; its removal stays pending");
            continue;
        };
        // The files an app has on stay: such a record is stale. The same folder
        // also when one path goes through a link (a game folder moved and linked
        // back) and the other doesn't.
        let wanted: Vec<PathBuf> = h
            .apps
            .read()
            .unwrap()
            .iter()
            .flat_map(placed_exes)
            .collect();
        let removal_real = real(&removal.exe);
        let stale = wanted.iter().any(|exe| {
            *exe == removal.exe
                || folder::same(exe, &removal.exe)
                || removal_real
                    .as_ref()
                    .is_some_and(|at| real(exe).as_ref() == Some(at))
        });
        if stale {
            clear_pending(h, &removal.exe);
            done.push(removal.exe.clone());
            slot.restore();
            continue;
        }
        let live = pending(h)
            .into_iter()
            .find(|entry| entry.exe == removal.exe);
        let still_owned = live
            .as_ref()
            .is_some_and(|entry| entry.uuid == removal.uuid)
            || (live.is_none() && also.iter().any(|entry| entry.exe == removal.exe));
        if !still_owned {
            slot.restore();
            continue;
        }
        match remove_at(&removal.name, &removal.exe) {
            Ok(Removed::Done(_)) => {
                clear_pending(h, &removal.exe);
                done.push(removal.exe.clone());
            }
            Ok(Removed::Unreachable(folder)) => {
                tracing::info!(app = %removal.name, %folder, "Neural Rendering: the game's folder can't be reached; its removal stays pending");
            }
            Err(error) => {
                tracing::warn!(app = %removal.name, error = %format!("{error:#}"), "Neural Rendering: the removal stays pending");
            }
        }
    }
    let remaining_also: Vec<Pending> = also
        .iter()
        .filter(|removal| !done.contains(&removal.exe))
        .cloned()
        .collect();
    if remaining_also.is_empty() {
        return;
    }
    let saved = update_settings(h, |value| {
        let mut list = pending_in(value)?;
        neural::merge_pending(&mut list, &remaining_also);
        value[PENDING] = json!(list);
        Ok(())
    });
    if let Err(error) = saved {
        tracing::warn!(error = %format!("{error:#}"), "Neural Rendering: the pending removals could not be saved");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        neural::{
            folder::{Response, Writer, handle},
            game_check::placed_exe,
            install::{fail_job, finish_install, start_job},
        },
        state::test_support::Fixture,
    };
    use butterpollo_session::neural::{Cached, MANIFEST, Package, Placed};
    use sha2::{Digest, Sha256};
    use std::{
        collections::BTreeMap,
        sync::{
            Arc, Barrier,
            atomic::{AtomicU64, Ordering},
        },
    };

    struct Temp(PathBuf);
    impl Temp {
        fn new(tag: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "rubylight-neural-pending-{tag}-{}-{id}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn sha(bytes: &[u8]) -> String {
        hex::encode(Sha256::digest(bytes))
    }

    /// Build a minimal valid cached package and install it into a fresh game folder.
    fn setup_installed_game(temp: &Path) -> (PathBuf, Response) {
        let cache = temp.join("cache");
        let files_dir = cache.join("files");
        let provider_rel =
            "full/game/OptiScaler/custom/Apex Neural Rendering FG/dlssg/nvngx_dlssg.dll";
        let entries: &[(&str, &[u8])] = &[
            ("full/game/dxgi.dll", b"OptiScaler proxy"),
            ("full/game/OptiScaler.ini", b"[FrameGen]\nFGInput=nvngxfg\n"),
            (provider_rel, b"provider"),
        ];
        let mut files = BTreeMap::new();
        for (rel, bytes) in entries {
            let path = files_dir.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, bytes).unwrap();
            files.insert(
                (*rel).to_owned(),
                Placed {
                    sha256: sha(bytes),
                    size: bytes.len() as u64,
                },
            );
        }
        let package = Cached {
            folder: cache,
            package: Package {
                tag: "FG-Test".into(),
                archives: BTreeMap::new(),
                files,
            },
        };
        let game_dir = temp.join("Game");
        std::fs::create_dir_all(&game_dir).unwrap();
        let exe = game_dir.join("Game.exe");
        std::fs::write(&exe, b"MZ").unwrap();
        std::fs::write(game_dir.join("nvngx_dlssg.dll"), b"DLSS").unwrap();
        let placed = handle(
            folder::Request::Install {
                exe: Some(exe.clone()),
                install: None,
                package,
            },
            Writer::User,
            &|_, _| {},
        );
        assert!(matches!(placed, Response::Placed { .. }), "{placed:?}");
        (exe, placed)
    }

    /// T1-2 regression: `try_pending` reads a pending entry while an install
    /// is in flight, then the install completes (`set_key` + `clear_pending` +
    /// `done`) before `Removing::take` runs. Re-checking live `h.apps` and
    /// `pending` under `Removing::take` must preserve the freshly installed
    /// files and keep the install job `"done"`.
    #[test]
    fn try_pending_rechecks_under_slot_and_preserves_freshly_finished_install() {
        let f = Fixture::new();
        let h = &f.host;
        let temp = Temp::new("race-finish");
        let (exe, placed) = setup_installed_game(temp.path());
        let folder = exe.parent().unwrap();

        crate::steam::update_library(h, |apps, _| {
            apps.push(json!({"name": "Game", "uuid": "game"}));
            true
        })
        .unwrap();
        let app = crate::neural::app(h, "game").unwrap();

        // Install is active and has recorded its pending entry; `KEY` is not saved yet.
        start_job(h, "game", "install", "place").unwrap();
        record_pending(
            h,
            &[Pending {
                name: "Game".into(),
                exe: exe.clone(),
                uuid: Some("game".into()),
            }],
        )
        .unwrap();

        // Simulate the exact race window: `try_pending` has already snapshotted
        // `removals` before `finish_install` saves `KEY`, clears pending, and
        // marks the job `"done"`.
        try_pending_inner(h, &[], || {
            finish_install(h, "game", &app, Some(&exe), Ok(placed.clone()));
        });

        assert!(
            folder.join(MANIFEST).is_file(),
            "manifest was deleted out from under freshly enabled app"
        );
        assert!(
            folder.join("dxgi.dll").is_file(),
            "proxy DLL was deleted out from under freshly enabled app"
        );
        assert!(pending(h).is_empty());
        let saved_app = crate::neural::app(h, "game").unwrap();
        assert_eq!(placed_exe(&saved_app), Some(exe));
        let job = h.neural.lock().jobs["game"].clone();
        assert_eq!((job.action, job.phase), ("install", "done"));
    }

    /// T1-2 regression: if an entry was cleared from live `pending(h)` before
    /// `Removing::take` opens (even without `KEY` saved on any app),
    /// `try_pending` skips it without calling `remove_at` and restores any
    /// failed job state.
    #[test]
    fn try_pending_skips_entry_already_cleared_before_slot_opened() {
        let f = Fixture::new();
        let h = &f.host;
        let temp = Temp::new("race-cleared");
        let (exe, _) = setup_installed_game(temp.path());
        let folder = exe.parent().unwrap();

        start_job(h, "game", "install", "place").unwrap();
        record_pending(
            h,
            &[Pending {
                name: "Game".into(),
                exe: exe.clone(),
                uuid: Some("game".into()),
            }],
        )
        .unwrap();

        try_pending_inner(h, &[], || {
            clear_pending(h, &exe);
            fail_job(h, "game", "refused".into(), false);
        });

        assert!(folder.join(MANIFEST).is_file());
        assert!(folder.join("dxgi.dll").is_file());
        assert!(pending(h).is_empty());
        let job = h.neural.lock().jobs["game"].clone();
        assert_eq!(job.phase, "failed");
        assert_eq!(job.error.as_deref(), Some("refused"));
    }

    /// T1-2 regression: an older pending entry with `uuid: None` gets its UUID
    /// filled when a new install records pending for the same exe, and a
    /// concurrent `remove_pending` thread never deletes a freshly completed
    /// install.
    #[test]
    fn concurrent_finish_install_and_remove_pending_never_deletes_installed_files() {
        let f = Fixture::new();
        let h = &f.host;
        let temp = Temp::new("race-concurrent");
        let (exe, placed) = setup_installed_game(temp.path());
        let folder = exe.parent().unwrap();

        // Pre-existing `uuid: None` entry gets upgraded to `Some("game")`.
        record_pending(
            h,
            &[Pending {
                name: "Game".into(),
                exe: exe.clone(),
                uuid: None,
            }],
        )
        .unwrap();
        record_pending(
            h,
            &[Pending {
                name: "Game".into(),
                exe: exe.clone(),
                uuid: Some("game".into()),
            }],
        )
        .unwrap();
        assert_eq!(pending(h)[0].uuid.as_deref(), Some("game"));

        crate::steam::update_library(h, |apps, _| {
            apps.push(json!({"name": "Game", "uuid": "game"}));
            true
        })
        .unwrap();
        let app = crate::neural::app(h, "game").unwrap();
        start_job(h, "game", "install", "place").unwrap();

        let barrier = Arc::new(Barrier::new(2));
        let h_finish = h.clone();
        let b_finish = barrier.clone();
        let exe_finish = exe.clone();
        let handle_finish = std::thread::spawn(move || {
            b_finish.wait();
            finish_install(&h_finish, "game", &app, Some(&exe_finish), Ok(placed));
        });

        let h_pending = h.clone();
        let b_pending = barrier;
        let handle_pending = std::thread::spawn(move || {
            b_pending.wait();
            for _ in 0..8 {
                remove_pending(&h_pending);
            }
        });

        handle_finish.join().unwrap();
        handle_pending.join().unwrap();

        assert!(folder.join(MANIFEST).is_file());
        assert!(folder.join("dxgi.dll").is_file());
        assert!(pending(h).is_empty());
        assert_eq!(
            placed_exe(&crate::neural::app(h, "game").unwrap()),
            Some(exe)
        );
    }

    /// T1-2 regression (Case A & Case B): if a legacy `uuid: None` entry is
    /// upgraded to `Some("game")` OR a deleted app's `uuid: Some("old")` entry
    /// is superseded by `Some("new")` during an in-flight install (before
    /// `set_key`), a `try_pending` pass carrying the stale UUID snapshot must
    /// neither bypass the live owner's job slot nor delete the placed files.
    #[test]
    fn try_pending_skips_entries_whose_uuid_was_upgraded_or_superseded_mid_install() {
        // Case A: legacy `uuid: None` upgraded to `Some("game")` during an in-flight install.
        {
            let f = Fixture::new();
            let h = &f.host;
            let temp = Temp::new("race-uuid-upgrade");
            let (exe, _) = setup_installed_game(temp.path());
            let folder = exe.parent().unwrap();

            record_pending(
                h,
                &[Pending {
                    name: "Game".into(),
                    exe: exe.clone(),
                    uuid: None,
                }],
            )
            .unwrap();

            try_pending_inner(h, &[], || {
                start_job(h, "game", "install", "place").unwrap();
                record_pending(
                    h,
                    &[Pending {
                        name: "Game".into(),
                        exe: exe.clone(),
                        uuid: Some("game".into()),
                    }],
                )
                .unwrap();
            });

            assert!(
                folder.join(MANIFEST).is_file(),
                "Case A: manifest was deleted during in-flight install"
            );
            assert!(
                folder.join("dxgi.dll").is_file(),
                "Case A: proxy DLL was deleted during in-flight install"
            );
            assert_eq!(pending(h)[0].uuid.as_deref(), Some("game"));
        }

        // Case B: deleted app's `uuid: Some("old")` superseded by re-added `Some("new")`
        // during an in-flight install (before `set_key`).
        {
            let f = Fixture::new();
            let h = &f.host;
            let temp = Temp::new("race-uuid-supersede");
            let (exe, _) = setup_installed_game(temp.path());
            let folder = exe.parent().unwrap();

            record_pending(
                h,
                &[Pending {
                    name: "Game".into(),
                    exe: exe.clone(),
                    uuid: Some("old".into()),
                }],
            )
            .unwrap();

            try_pending_inner(h, &[], || {
                start_job(h, "new", "install", "place").unwrap();
                record_pending(
                    h,
                    &[Pending {
                        name: "Game".into(),
                        exe: exe.clone(),
                        uuid: Some("new".into()),
                    }],
                )
                .unwrap();
            });

            assert!(
                folder.join(MANIFEST).is_file(),
                "Case B: manifest was deleted during in-flight re-added game install"
            );
            assert!(
                folder.join("dxgi.dll").is_file(),
                "Case B: proxy DLL was deleted during in-flight re-added game install"
            );
            assert_eq!(pending(h)[0].uuid.as_deref(), Some("new"));
            assert!(
                !h.neural.lock().jobs.contains_key("old"),
                "Case B: stale `old` slot must be cleanly restored"
            );
        }
    }
}
