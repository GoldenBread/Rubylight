//! A launched game's window brought back when it is minimised or behind its store client (issue
//! #131). A game that switches an exclusive-fullscreen mode can be left minimised, or behind the
//! Steam window that started with it, until the user clicks it in the taskbar; for a minute after
//! its window is first seen, the host does that click itself, at most three times (`process.rs`).
//!
//! Off the stream's thread, from the housekeeping loop, and never holding `current_app` across
//! process enumeration, window enumeration, or foreground changes (`runtime.rs`). The raise and the
//! window reading follow the input desktop, so a host running as the service reaches the desktop of
//! the signed-in user.
use crate::process::{FocusSnapshot, store_client};
use crate::state::Shared;
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

/// How often the game's window is looked at; every `launch_curtain::EVERY` while a curtain is up.
pub const EVERY: Duration = Duration::from_secs(2);

/// One look, from the housekeeping loop.
pub fn watch(h: &Shared) {
    look(
        h,
        Instant::now(),
        |snapshot| snapshot.candidate_pids().unwrap_or_default(),
        game_windows,
        is_store_client,
        butterpollo_win_io::lossless::focus_quietly,
    );
}

/// One look at `now`, keeping `current_app` released while enumerating processes, reading
/// windows, resolving process image paths, and bringing the chosen window back.
fn look(
    h: &Shared,
    now: Instant,
    pids_fn: impl FnOnce(&FocusSnapshot) -> BTreeSet<u32>,
    read: impl FnOnce(&BTreeSet<u32>) -> (Vec<(u32, bool)>, Option<u32>),
    store_client: impl Fn(u32) -> bool,
    mut bring_back: impl FnMut(u32) -> bool,
) {
    let snapshot = {
        let Some(current) = h.try_current_app() else {
            return;
        };
        let Some(snapshot) = current.as_ref().and_then(|app| app.focus_snapshot(now)) else {
            return;
        };
        snapshot
    };
    let pids = pids_fn(&snapshot);
    let (windows, foreground) = if pids.is_empty() {
        (Vec::new(), None)
    } else {
        read(&pids)
    };
    let fg_is_store = foreground.is_some_and(&store_client);
    let action = {
        let Some(mut current) = h.try_current_app() else {
            return;
        };
        current.as_mut().and_then(|app| {
            app.bring_back_for(&snapshot.generation, now, windows, foreground, |_| {
                fg_is_store
            })
        })
    };
    if let Some((pid, app_name)) = action {
        let brought = bring_back(pid);
        tracing::info!(
            app = %app_name,
            pid,
            brought,
            "brought the launched game's window back to the front, as a click in the taskbar does"
        );
    }
}

/// The game's unowned, uncloaked top-level windows now on the desktop (`pid`, minimised), and
/// the active non-shell process in front.
pub(crate) fn game_windows(pids: &BTreeSet<u32>) -> (Vec<(u32, bool)>, Option<u32>) {
    let windows = butterpollo_win_io::window_list::visible()
        .into_iter()
        .filter(|window| {
            !window.owned && !window.tool && !window.cloaked && pids.contains(&window.pid)
        })
        .map(|window| (window.pid, window.minimized))
        .collect();
    (
        windows,
        butterpollo_win_graphics::foreground::active_process(),
    )
}

/// Whether a process's program is a store client's or launcher's (Steam, Epic, EA, Ubisoft,
/// Xbox): its window in front of the game's is what the game is brought back over.
pub(crate) fn is_store_client(pid: u32) -> bool {
    butterpollo_win_io::process::image_path(pid).is_some_and(|path| {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(&path);
        store_client(name) || butterpollo_session::launcher_prompt::launcher_of(name).is_some()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::RunningApp;
    use crate::state::test_support::Fixture;
    use butterpollo_session::state::App;
    use std::collections::BTreeMap;

    fn launching_steam_app() -> RunningApp {
        let mut app = RunningApp::with_environment(&App::desktop(), BTreeMap::new()).unwrap();
        app.mark_launching();
        app.track_steam(r"C:\not-a-game");
        app
    }

    #[test]
    fn launch_focus_releases_current_app_during_os_calls() {
        let f = Fixture::new();
        *f.host.current_app.lock().unwrap() = Some(launching_steam_app());
        let mut brought_pid = None;
        look(
            &f.host,
            Instant::now(),
            |_snapshot| {
                assert!(
                    f.host.try_current_app().is_some(),
                    "candidate pid scan held current_app"
                );
                BTreeSet::from([30])
            },
            |pids| {
                assert!(
                    f.host.try_current_app().is_some(),
                    "window read held current_app"
                );
                assert_eq!(pids, &BTreeSet::from([30]));
                (vec![(30, false)], Some(99))
            },
            |pid| {
                assert!(
                    f.host.try_current_app().is_some(),
                    "store_client lookup held current_app"
                );
                pid == 99
            },
            |pid| {
                assert!(
                    f.host.try_current_app().is_some(),
                    "bring_back held current_app"
                );
                brought_pid = Some(pid);
                true
            },
        );
        assert_eq!(brought_pid, Some(30));
    }

    #[test]
    fn launch_focus_for_a_replaced_app_does_not_apply_to_its_successor() {
        let f = Fixture::new();
        *f.host.current_app.lock().unwrap() = Some(launching_steam_app());
        let mut brought = false;
        look(
            &f.host,
            Instant::now(),
            |_| BTreeSet::from([30]),
            |_| {
                *f.host.current_app.lock().unwrap() = Some(launching_steam_app());
                (vec![(30, true)], Some(99))
            },
            |pid| pid == 99,
            |_| {
                brought = true;
                true
            },
        );
        assert!(!brought, "must not bring back a replaced app");
        assert!(
            f.host
                .current_app
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .bring_back_open(Instant::now()),
            "successor keeps a clean focus window"
        );
    }
}
