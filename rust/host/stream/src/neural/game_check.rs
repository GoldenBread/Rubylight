//! Background per-game eligibility checks, app metadata helpers, and Steam
//! Valve Anti-Cheat lookup for Neural Rendering.
use super::{
    GAME_RECHECK, GameCheck, GameChecked, GameKey, Looked, Lookup, State,
    folder::{self, Request, Response},
    game_key,
    probe::runtime_beside,
    process::run_folder,
    stall::stalls,
};
use crate::state::Shared;
use anyhow::{Context, Result};
use butterpollo_session::{neural, state::App};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::PoisonError,
    time::{Duration, Instant},
};

/// What a status read knows of `app`'s game folder (saved as `uuid`): the
/// Visual C++ runtime beside its program, and with `ask` the game's own
/// check, as turning the switch on asks it before the download: its
/// program, anti-cheat, DLSS frame generation and Valve Anti-Cheat, through
/// the user's helper as the install does. The check is None when it failed:
/// the switch stays usable and the install says why. `system` is what
/// System32 lacks of the runtime; with nothing lacking, the game's folder
/// is not looked into for it.
///
/// Nothing here touches the game's folder, which may be on a share that
/// stalls: one thread per app at a time finds the game's program and looks
/// beside it, then runs the check where it decides something (`ask`, and
/// the runtime is there). A reader waits for it up to `wait`, then answers
/// with what it last found for the app's record, however old, and the
/// thread's answer serves the next read. Both are held for
/// [`GAME_RECHECK`]; a job drops the check's, and one that began before a
/// job is not kept. While `streaming`, no check starts and the last answer
/// is given.
pub(super) fn game_check(
    h: &Shared,
    uuid: &str,
    app: &App,
    system: &[&'static str],
    wait: Duration,
    ask: bool,
    streaming: bool,
) -> Looked {
    let key = game_key(app);
    let fresh = |at: &Instant| at.elapsed() < GAME_RECHECK;
    fn held_lookup<'a>(state: &'a State, uuid: &str, key: &GameKey) -> Option<&'a Lookup> {
        state.lookups.get(uuid).filter(|lookup| lookup.key == *key)
    }
    fn held_check<'a>(state: &'a State, uuid: &str, key: &GameKey) -> Option<&'a GameChecked> {
        state.games.get(uuid).filter(|(_, held, _)| held == key)
    }
    // Whether what is held answers the read: the check only matters while
    // the runtime is there.
    let current = |state: &State| match held_lookup(state, uuid, &key) {
        Some(lookup) if fresh(&lookup.at) => {
            !ask || system.iter().any(|file| !lookup.beside.contains(file))
                || held_check(state, uuid, &key).is_some_and(|(at, ..)| fresh(at))
        }
        _ => false,
    };
    let mut state = h.neural.lock();
    let quiet = streaming
        && (held_lookup(&state, uuid, &key).is_some() || held_check(&state, uuid, &key).is_some());
    if !current(&state) && !quiet {
        if state.checking.insert(uuid.to_owned()) {
            let generation = state.generation.get(uuid).copied().unwrap_or(0);
            let (h, uuid, app, key) = (h.clone(), uuid.to_owned(), app.clone(), key.clone());
            let system = system.to_vec();
            std::thread::spawn(move || {
                /// Ends the check however it ends, a panic too.
                struct Ending<'a>(&'a Shared, &'a str);
                impl Drop for Ending<'_> {
                    fn drop(&mut self) {
                        self.0.neural.lock().checking.remove(self.1);
                        self.0.neural.checked.notify_all();
                    }
                }
                let _ending = Ending(&h, &uuid);
                let command = game_command(&app);
                let beside = match game_folder_with(&app, command.clone()) {
                    Some(folder) if !system.is_empty() => runtime_beside(&folder),
                    _ => Vec::new(),
                };
                let runtime = system.iter().all(|file| beside.contains(file));
                let lookup = Lookup {
                    at: Instant::now(),
                    key: key.clone(),
                    beside,
                };
                h.neural.lock().lookups.insert(uuid.clone(), lookup);
                h.neural.checked.notify_all();
                if !ask || !runtime {
                    return;
                }
                let exe = legacy_exe(&app).or(command);
                let check = run_game_check(&h, &app, exe, install(&app));
                let mut state = h.neural.lock();
                if state.generation.get(&uuid).copied().unwrap_or(0) == generation {
                    state
                        .games
                        .insert(uuid.clone(), (Instant::now(), key, check));
                }
            });
        }
        let deadline = Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if current(&state) || !state.checking.contains(uuid) || left.is_zero() {
                break;
            }
            state = h
                .neural
                .checked
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
    Looked {
        beside: held_lookup(&state, uuid, &key).map(|lookup| lookup.beside.clone()),
        check: held_check(&state, uuid, &key).and_then(|(.., check)| check.clone()),
    }
}

/// [`game_check`]'s check itself. Blocks.
fn run_game_check(
    h: &Shared,
    app: &App,
    exe: Option<PathBuf>,
    install: Option<PathBuf>,
) -> Option<GameCheck> {
    #[cfg(any(test, feature = "test-support"))]
    {
        h.neural
            .game_checks
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let gate = h.neural.game_gate.lock().unwrap().take();
        if let Some(gate) = gate {
            let _ = gate.recv();
        }
    }
    match find_game(h, app, exe, install) {
        Ok(Response::Found { .. }) => Some(GameCheck::Fits),
        Ok(Response::Refused { reason }) => Some(GameCheck::Refused {
            why: neural::game_why(&reason),
            reason,
        }),
        Ok(Response::ChooseExe { reason }) => Some(GameCheck::Choose(reason)),
        Ok(other) => {
            tracing::debug!(app = %app.name, answer = ?other, "Neural Rendering: the game's check gave no answer");
            None
        }
        Err(error) => {
            tracing::debug!(app = %app.name, error = %format!("{error:#}"), "Neural Rendering: the game's check failed");
            None
        }
    }
}

/// The folder of the game's program as far as the app tells without the
/// helper: where the files are, else where the earlier upscaling was, else
/// the app's command when it is a program in the install. Blocks on the
/// game's folder.
pub(super) fn game_folder(app: &App) -> Option<PathBuf> {
    game_folder_with(app, None).or_else(|| game_folder_with(app, game_command(app)))
}

/// [`game_folder`], with the app's command as [`game_command`] found it.
fn game_folder_with(app: &App, command: Option<PathBuf>) -> Option<PathBuf> {
    placed_exe(app)
        .or_else(|| legacy_exe(app))
        .or(command)
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
}

pub fn app(h: &Shared, uuid: &str) -> Option<App> {
    h.apps
        .read()
        .unwrap()
        .iter()
        .find(|app| app.extra.get("uuid").and_then(Value::as_str) == Some(uuid))
        .cloned()
}

/// The folder the app's launcher installed the game in, when it says.
pub fn install(app: &App) -> Option<PathBuf> {
    neural::app_install_dir(app)
}

/// Why the game is refused when Steam lists Valve Anti-Cheat for it, which
/// no file in its install shows, or can't say ([`steam::valve_anti_cheat`]
/// with Steam's `roots`): asked for each of `paths` ([`reached_by`]), as a
/// game's folder moved elsewhere and linked back into Steam's library is a
/// plain folder where the link leads.
///
/// [`steam::valve_anti_cheat`]: butterpollo_session::steam::valve_anti_cheat
pub fn steam_refusal(app: &App, paths: &[PathBuf], roots: &[PathBuf]) -> Option<String> {
    let appid = serde_json::to_value(app)
        .ok()
        .and_then(|app| butterpollo_session::steam::game_of(&app));
    // A Steam game's link is asked once: its flag names no file.
    let paths = if appid.is_some() {
        &paths[..paths.len().min(1)]
    } else {
        paths
    };
    paths.iter().find_map(|exe| {
        match butterpollo_session::steam::valve_anti_cheat(appid, exe, "", roots) {
            Ok(false) => None,
            Ok(true) => Some(format!(
                "Steam lists Valve Anti-Cheat for the game, and {}",
                folder::ANTI_CHEAT
            )),
            Err(message) => Some(format!(
                "Rubylight can't tell whether the game uses Valve Anti-Cheat ({message})"
            )),
        }
    })
}

/// The game's .exe `exe` (where a linked game folder leads, as the helper
/// works there) and the paths it was reached by before the link was
/// followed: the chosen program `requested`, the app's command, and `exe`
/// in the app's install as named. Local candidates are also resolved via
/// [`folder::real`] so junctions or non-canonical paths into a Steam library
/// never hide Valve Anti-Cheat.
pub(super) fn reached_by(app: &App, requested: Option<&Path>, exe: &Path) -> Vec<PathBuf> {
    let local_real = |path: &Path| {
        butterpollo_session::nucleus::generic::on_this_pc(&path.to_string_lossy())
            .and_then(|_| folder::real(path))
    };
    let real_exe = local_real(exe);
    let named = install(app).and_then(|install| {
        let relative = local_real(&install)
            .and_then(|real_install| {
                real_exe
                    .as_deref()
                    .and_then(|re| re.strip_prefix(&real_install).ok())
                    .or_else(|| exe.strip_prefix(&real_install).ok())
                    .map(Path::to_path_buf)
            })
            .or_else(|| exe.strip_prefix(&install).ok().map(Path::to_path_buf))
            .or_else(|| exe.file_name().map(PathBuf::from))?;
        Some(install.join(relative))
    });
    let cmd = command_exe(app);
    let mut paths = Vec::new();
    for path in [
        Some(exe.to_path_buf()),
        real_exe,
        requested.map(Path::to_path_buf),
        requested.and_then(local_real),
        cmd.clone(),
        cmd.as_deref().and_then(local_real),
        named.clone(),
        named.as_deref().and_then(local_real),
    ]
    .into_iter()
    .flatten()
    {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

/// Steam's folders on this PC, or a test's.
pub fn steam_roots(h: &Shared) -> Vec<PathBuf> {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(roots) = h.neural.steam_roots.lock().unwrap().clone() {
        return roots;
    }
    let _ = h;
    butterpollo_win_io::steam::roots()
}

/// The game's program as an install places it ([`Request::Find`]), or why
/// not: Steam lists Valve Anti-Cheat for the game at it or at a path it was
/// reached by ([`steam_refusal`]). Blocks.
pub fn find_game(
    h: &Shared,
    app: &App,
    exe: Option<PathBuf>,
    install: Option<PathBuf>,
) -> Result<Response> {
    let requested = exe.clone();
    let exe = match run_folder(Request::Find { exe, install }, &|_, _| {})? {
        Response::Found { exe } => exe,
        other => return Ok(other),
    };
    let paths = reached_by(app, requested.as_deref(), &exe);
    Ok(match steam_refusal(app, &paths, &steam_roots(h)) {
        Some(reason) => Response::Refused { reason },
        None => Response::Found { exe },
    })
}

/// The app's command when it starts a program directly.
pub(super) fn command_exe(app: &App) -> Option<PathBuf> {
    let target = PathBuf::from(butterpollo_win_io::process::command_target(&app.cmd));
    (target.is_absolute()
        && target
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("exe")))
    .then_some(target)
}

/// The app's command when it starts the game itself: in the game's install,
/// which the app must name, and no launcher, interpreter or program of
/// Windows. Otherwise the game's .exe is found in its install, or the user
/// picks it: nothing outside a known install is taken unasked.
pub fn game_command(app: &App) -> Option<PathBuf> {
    let install = install(app)?;
    command_exe(app).filter(|exe| {
        stalls(exe);
        exe.is_file() && folder::inside(exe, &install) && folder::not_the_game(exe).is_none()
    })
}

/// The .exe the files sit beside, once they are in.
pub fn placed_exe(app: &App) -> Option<PathBuf> {
    neural::app_placed_exe(app)
}

/// The .exe Rubylight 3.0 placed AMDNR beside, until it is removed.
pub fn legacy_exe(app: &App) -> Option<PathBuf> {
    neural::app_legacy_exe(app)
}

/// Every folder the host placed files in for the app: this release's and
/// Rubylight 3.0's, each once.
pub fn placed_exes(app: &App) -> Vec<PathBuf> {
    neural::app_placed_exes(app)
}

/// Whether the host placed files for the app, now or in Rubylight 3.0.
pub(super) fn has_files(app: &App) -> bool {
    neural::app_has_files(app)
}

/// Set or clear the app's `key` and save the apps.
pub fn set_key(h: &Shared, uuid: &str, key: &str, value: Option<Value>) -> Result<()> {
    h.reload_apps();
    let mut apps = h.apps.write().unwrap();
    let mut next = apps.clone();
    let app = next
        .iter_mut()
        .find(|app| app.extra.get("uuid").and_then(Value::as_str) == Some(uuid))
        .context("application not found")?;
    match value {
        Some(value) => app.extra.insert(key.into(), value),
        None => app.extra.remove(key),
    };
    h.assign_apps(&mut next)?;
    let mut doc = h.app_document.read().unwrap().clone();
    doc["apps"] = serde_json::to_value(&next)?;
    h.save_apps(&doc)?;
    *apps = next;
    *h.app_document.write().unwrap() = doc;
    h.config_version
        .fetch_add(1, std::sync::atomic::Ordering::Release);
    Ok(())
}

/// A saved app keeps the host's keys: the console never sets them.
pub fn keep_key(stored: Option<&App>, app: &mut App) {
    neural::app_keep_key(stored, app)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::Fixture;
    use serde_json::json;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Temp {
        path: PathBuf,
        junctions: Vec<PathBuf>,
    }

    impl Temp {
        fn new(tag: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "rubylight-neural-game-check-{tag}-{}-{id}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self {
                path,
                junctions: Vec::new(),
            }
        }

        fn path(&self) -> &Path {
            &self.path
        }

        fn junction(&mut self, link: PathBuf, target: &Path) -> PathBuf {
            if let Some(parent) = link.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            let output = std::process::Command::new("cmd")
                .args(["/c", "mklink", "/J"])
                .arg(&link)
                .arg(target)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            self.junctions.push(link.clone());
            link
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            for link in self.junctions.iter().rev() {
                let _ = std::fs::remove_dir(link);
            }
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn vac_steam_root(tag: &str) -> Temp {
        let root = Temp::new(tag);
        let apps = root.path().join("steamapps");
        let bin = apps.join(r"common\CSGO\game\bin\win64");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("cs2.exe"), b"MZ").unwrap();
        std::fs::write(bin.join("nvngx_dlssg.dll"), b"DLSS").unwrap();
        std::fs::write(
            apps.join("appmanifest_730.acf"),
            "\"AppState\" { \"appid\" \"730\" \"installdir\" \"CSGO\" }",
        )
        .unwrap();
        let mut kv = Vec::new();
        for key in ["appinfo", "common", "category"] {
            kv.push(0x00);
            kv.extend(key.as_bytes());
            kv.push(0);
        }
        kv.push(0x01);
        kv.extend(b"category_8\0");
        kv.extend(b"1\0");
        kv.extend([0x08; 5]);
        let mut data = 0x0756_4428u32.to_le_bytes().to_vec();
        data.extend(1u32.to_le_bytes());
        data.extend(730u32.to_le_bytes());
        data.extend(((60 + kv.len()) as u32).to_le_bytes());
        data.extend([0; 60]);
        data.extend(kv);
        data.extend(0u32.to_le_bytes());
        std::fs::create_dir_all(root.path().join("appcache")).unwrap();
        std::fs::write(root.path().join(r"appcache\appinfo.vdf"), data).unwrap();
        root
    }

    #[test]
    fn traversal_verbatim_and_junction_paths_cannot_bypass_steam_vac_check() {
        let f = Fixture::new();
        let h = &f.host;
        let mut steam = vac_steam_root("vac");
        let roots = vec![steam.path().to_path_buf()];
        *h.neural.steam_roots.lock().unwrap() = Some(roots.clone());

        let app: App = serde_json::from_value(json!({"name": "CS2", "uuid": "cs2"})).unwrap();
        let traversal = PathBuf::from(format!(
            r"{}\steamapps\common\CSGO\game\bin\win64\..\win64\cs2.exe",
            steam.path().display()
        ));
        let verbatim = PathBuf::from(format!(
            r"\\?\{}\steamapps\common\CSGO\game\bin\win64\cs2.exe",
            steam.path().display()
        ));

        // Direct `steam_refusal` catches both `..` and `\\?\`.
        for exe in [&traversal, &verbatim] {
            let refusal = steam_refusal(&app, std::slice::from_ref(exe), &roots)
                .expect("VAC game must be refused");
            assert!(
                refusal.starts_with("Steam lists Valve Anti-Cheat"),
                "{}: {refusal}",
                exe.display()
            );
            // `find_game` rejects traversal and verbatim paths upfront too.
            assert!(
                matches!(
                    find_game(h, &app, Some(exe.clone()), None).unwrap(),
                    Response::ChooseExe { .. }
                ),
                "{}",
                exe.display()
            );
        }

        // A junction outside Steam pointing into `steamapps\common\CSGO\game\bin\win64`
        // resolves via `reached_by` (`folder::real`) and is still refused by VAC.
        let alias = steam.junction(
            steam.path().join("AliasBin"),
            &steam.path().join(r"steamapps\common\CSGO\game\bin\win64"),
        );
        let alias_exe = alias.join("cs2.exe");
        let response = find_game(h, &app, Some(alias_exe), None).unwrap();
        match response {
            Response::Refused { reason } => {
                assert!(
                    reason.starts_with("Steam lists Valve Anti-Cheat"),
                    "{reason}"
                );
            }
            other => panic!("expected VAC refusal through junction, got {other:?}"),
        }
    }
}
