//! Ignored on-PC SYSTEM hardware case exercising a full background Nucleus Co-op stream end to end.
use super::hardware_support::*;
use super::setup_helper::client;
use anyhow::{Context, Result, anyhow, ensure};
use butterpollo_config::config::Config;
use butterpollo_input_policy::input::Input;
use butterpollo_session::nucleus::coop::Phase;
use butterpollo_session::nucleus::elevation::Run;
use butterpollo_session::nucleus::handler::Catalog;
use butterpollo_session::session::Warnings;
use butterpollo_stream::nucleus::*;
use butterpollo_win_io::nucleus::{files, pads, user_copy};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

/// A background stream of a one-player game without a hub handler on
/// this PC, through the host's own admission (tools/hw/catalog.yaml,
/// nucleus-generic-background): the app's .exe `RUBYLIGHT_NUCLEUS_GAME`
/// or the Steam link of `RUBYLIGHT_NUCLEUS_STEAM_APP` goes through the
/// path, launcher, anti-cheat, argument and Unreal checks, Rubylight's
/// handler and list entry are written, and Nucleus Co-op starts the game.
/// Asserted: the handler file and the entry; a process of the handler's
/// .exe started by the session; the session running with its window;
/// that window captured through WGC with a frame not black; the first
/// player's A press and release at the session's XInput index (where
/// Nucleus' input hooks read the pad; whether the game reacts is not
/// observable here); and the game's compatibility settings as before once
/// it stops. Nucleus Co-op's game list, Rubylight's handlers (an earlier one
/// the session's sweep took included) and the settings at the game's
/// paths are then exactly as before, also on a failure. Run as SYSTEM,
/// like the service, with Nucleus Co-op
/// installed by the console and not running. No message box of Nucleus
/// or the game shows at any point, and Nucleus' crashlog.txt is
/// unchanged in the host's folder and in the user's copy (read as them).
///
/// Where its NucleusCoop.exe runs from says how the admission ran it: as
/// the user from their own copy (`nucleus-user\<SID>\<files>`, the
/// handler written there and the game in its list), or elevated from the
/// host's. `RUBYLIGHT_NUCLEUS_RUN` (user or elevated) asserts which
/// (nucleus-generic-background-user). Run as the user, NucleusCoop.exe
/// and the game must hold tokens that are not elevated, at medium
/// integrity, and Rubylight's handlers and the game list in the copy are
/// as before too; the copy's files are read and put back as the user.
///
/// With `RUBYLIGHT_NUCLEUS_COLD=1` (nucleus-generic-background-cold), the
/// Settings.ini of the host's Nucleus and of the user's copy first get
/// the values that stalled #59 ([`cold_settings`]) and are put back
/// afterwards, also on a failure; the profile must be clicked within
/// 15 s of Nucleus' start, Nucleus must set the game's DPI mode, and the
/// Settings.ini Nucleus ran from must hold the forced values while the
/// session runs, the other one untouched. Afterwards the host's has the
/// user's values back with no journal left; the copy's keeps the
/// session's, with no journal in it.
#[test]
#[ignore = "starts a game through Nucleus Co-op on this PC as SYSTEM"]
fn a_background_stream_without_a_hub_handler_on_this_pc() -> Result<()> {
    use butterpollo_win_io::nucleus::layers::{Layers, UserHive};
    let log = CaseLog::install()?;
    let cold = std::env::var("RUBYLIGHT_NUCLEUS_COLD").is_ok_and(|v| v == "1");
    let game = std::env::var("RUBYLIGHT_NUCLEUS_GAME").ok();
    let steam_app = std::env::var("RUBYLIGHT_NUCLEUS_STEAM_APP")
        .ok()
        .map(|id| id.parse::<u32>().context("RUBYLIGHT_NUCLEUS_STEAM_APP"))
        .transpose()?;
    let hold = Duration::from_secs(
        std::env::var("RUBYLIGHT_NUCLEUS_HOLD")
            .ok()
            .map(|s| s.parse().context("RUBYLIGHT_NUCLEUS_HOLD"))
            .transpose()?
            .unwrap_or(20),
    );
    let expect = expected_run(std::env::var("RUBYLIGHT_NUCLEUS_RUN").ok().as_deref())?;
    let app = case_app(game.as_deref(), steam_app)?;
    let pc = OnPc;
    if let Some(why) = pc.unavailable() {
        anyhow::bail!(why);
    }
    // The folder chosen in the console, else an earlier install.
    let chosen = std::env::var("RUBYLIGHT_NUCLEUS_PATH").unwrap_or_default();
    let folder = pc
        .installed(&chosen)?
        .context("Nucleus Co-op is not set up: set RUBYLIGHT_NUCLEUS_PATH to its folder")?;
    if let Some(pid) = pc.running()? {
        anyhow::bail!("Nucleus Co-op runs (process {pid}): close it first");
    }
    // The signed-in user's own copy, made here as the admission would
    // when the session runs as them: its folder is named by the host's
    // program files, so it is only known once made. What is in it is
    // the user's to change: read and put back as them.
    let user = user_copy::user()?;
    let copy = pc.user_copy(&folder)?;
    // Nucleus' crashlog.txt where either may run from.
    let crashlogs = [folder.join("crashlog.txt"), copy.join("crashlog.txt")];
    let crashes = || -> Result<Vec<Option<Vec<u8>>>> {
        Ok(vec![
            read_if_there(&crashlogs[0])?,
            user_copy::as_user(Some(&user), || read_if_there(&crashlogs[1]))??,
        ])
    };
    let crashes_before = crashes()?;
    let settings = folder.join("Settings.ini");
    let copy_settings = copy.join("Settings.ini");
    // Declared before the cleanup, so they are put back after the session ended.
    let _kept_settings = if cold {
        let mut kept = Vec::new();
        for (path, journal, token) in [
            (
                &settings,
                Some(folder.join("Rubylight-settings.json")),
                None,
            ),
            (&copy_settings, None, Some(user.try_clone()?)),
        ] {
            let before = user_copy::as_user(token.as_ref(), || std::fs::read(path))?
                .with_context(|| path.display().to_string())?;
            let seeded = cold_settings(&before);
            kept.push(KeptSettings {
                path: path.clone(),
                before,
                journal,
                user: token,
            });
            let user = kept.last().and_then(|kept| kept.user.as_ref());
            user_copy::as_user(user, || std::fs::write(path, seeded))?
                .with_context(|| path.display().to_string())?;
            println!(
                "Settings.ini seeded: DisablePathCheck=False, ShowFavoriteOnly=True, GameSortingOpt=Sort_2,Show_Type2 in {}",
                path.display()
            );
        }
        kept
    } else {
        Vec::new()
    };
    let registry = Arc::new(Registry::new(Arc::new(OnPc)));
    // The game lists Nucleus Co-op reads (#59): the host's, the copy's.
    let (list, copy_list) = (files::list(&folder), files::list(&copy));
    let own_in = |handlers: &Path| -> Result<Vec<PathBuf>> {
        let mut own = user_copy::as_user(Some(&user), || KeptFiles::own_handlers(handlers))??;
        own.sort();
        Ok(own)
    };
    let list_of = |list: &Path| user_copy::as_user(Some(&user), || read_if_there(list))?;
    let before = pc.catalog(&folder)?;
    let copy_before = (own_in(&copy.join("handlers"))?, list_of(&copy_list)?);
    let mut cleanup = CaseCleanup {
        registry: registry.clone(),
        kept: KeptFiles::take(&[&folder.join("handlers")], &list)?,
        kept_copy: user_copy::as_user(Some(&user), || {
            KeptFiles::take(&[&copy.join("handlers")], &copy_list)
        })??,
        user: Some(user.try_clone()?),
        layers_before: UserHive::signed_in()?.values()?,
        contents: vec![folder.join("content"), copy.join("content")],
        game: None,
        done: false,
    };
    let owner = client("hw-case", false);
    let config = Config::default();
    let warnings = Arc::new(Warnings::default());
    let mut desktop = butterpollo_win_io::nucleus::desktop::Win32::default();
    let boxes_before = windows_boxes(&mut desktop)?;
    let since = butterpollo_win_io::process::now();
    let seat = registry
        .admit(Admission {
            launch: "hw-case",
            client: &owner,
            game: Game::Background(&app),
            size: (1280, 720),
            config: &config,
            warnings: warnings.clone(),
            release_parked: &|| Ok(()),
            instances: None,
        })
        .map_err(|refused| anyhow!("refused ({}): {}", refused.status, refused.message))?;
    let instance = registry
        .instance(seat.session)
        .context("the admitted session is not listed")?;
    let guid = instance.session(|s| s.game.clone());
    // Where its NucleusCoop.exe runs from says how it was run.
    let looking = Instant::now();
    let nucleus: Vec<(u32, String)> = loop {
        let found = butterpollo_win_io::process::processes()?
            .into_iter()
            .filter(|p| p.started >= since && p.name.eq_ignore_ascii_case("NucleusCoop.exe"))
            .map(|p| {
                let image = butterpollo_win_io::process::image_path(p.pid)
                    .with_context(|| format!("NucleusCoop.exe {}'s program path", p.pid))?;
                Ok((p.pid, image))
            })
            .collect::<Result<Vec<_>>>()?;
        if !found.is_empty() {
            break found;
        }
        ensure!(
            looking.elapsed() < Duration::from_secs(30),
            "no NucleusCoop.exe started with the session"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    let runs = nucleus
        .iter()
        .map(|(_, image)| ran_from(image, &folder, &copy))
        .collect::<Result<Vec<Run>>>()?;
    let run = runs[0];
    ensure!(
        runs.iter().all(|other| *other == run),
        "NucleusCoop.exe runs from both copies: {nucleus:?}"
    );
    if let Some(expect) = expect {
        ensure!(
            run == expect,
            "the session runs {run:?}, not {expect:?}: {nucleus:?}"
        );
    }
    let from = match run {
        Run::User => &copy,
        Run::Elevated => &folder,
    };
    println!("runs {run:?} from {}", from.display());
    let catalog = pc.catalog(&folder)?;
    let handler = catalog
        .handlers
        .iter()
        .find(|h| h.guid == guid)
        .with_context(|| format!("no handler {guid} in Nucleus Co-op's handlers"))?
        .clone();
    ensure!(handler.own, "{} is not Rubylight's handler", handler.file);
    let entry = catalog
        .games
        .iter()
        .find(|g| g.guid == guid)
        .with_context(|| format!("{guid} is not in Nucleus Co-op's game list"))?;
    // As Nucleus Co-op 2.4.2 loads the list of the folder it runs from:
    // shown, so it can be selected. The copy's is read as the user.
    let reader = (run == Run::User).then_some(&user);
    let listed = user_copy::as_user(reader, || files::listed_by_nucleus(from))??;
    ensure!(
        listed.contains(&guid),
        "Nucleus Co-op does not show {guid} from {}",
        files::list(from).display()
    );
    println!("listed in {}", files::list(from).display());
    cleanup.game = Some((entry.exe_path.clone(), handler.executable.clone()));
    println!(
        "handler {} for {} at {}",
        handler.file, handler.executable, entry.exe_path
    );
    let started = Instant::now();
    loop {
        match instance.session(|s| s.phase.clone()) {
            Phase::Running => break,
            Phase::Ended(why) => anyhow::bail!("{why}"),
            Phase::Starting => {
                no_message_box(&mut desktop, &boxes_before, since, &handler.executable)?;
                ensure!(
                    started.elapsed() < Duration::from_secs(180),
                    "not running after {:?}",
                    started.elapsed()
                );
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    println!("running after {:?}", started.elapsed());
    let delay = click_delay(&log.0.lock().unwrap())?;
    println!("profile clicked {delay:?} after Nucleus Co-op started");
    ensure!(
        !cold || delay <= Duration::from_secs(15),
        "the profile was clicked {delay:?} after Nucleus Co-op started, not within 15 s"
    );
    let set = layer_changes(
        &cleanup.layers_before,
        &UserHive::signed_in()?.values()?,
        &entry.exe_path,
        &cleanup.contents,
        &handler.executable,
    );
    println!("compatibility settings Nucleus set: {}", set.len());
    // The Settings.ini Nucleus runs from, and the other one.
    let (ran_settings, other_settings) = match run {
        Run::User => (&copy_settings, &settings),
        Run::Elevated => (&settings, &copy_settings),
    };
    let settings_of = |path: &Path| -> Result<Vec<u8>> {
        let reader = (path == copy_settings).then_some(&user);
        user_copy::as_user(reader, || std::fs::read(path))?
            .with_context(|| path.display().to_string())
    };
    if cold {
        settings_are(&settings_of(ran_settings)?, &FORCED)?;
        settings_are(&settings_of(other_settings)?, &COLD)?;
        println!("Settings.ini has the forced values, the other one is untouched");
    }
    ensure!(
        !cold || !set.is_empty(),
        "Nucleus Co-op set no DPI mode for {}",
        handler.executable
    );
    let game_processes: Vec<u32> = butterpollo_win_io::process::processes()?
        .into_iter()
        .filter(|p| p.started >= since && p.name.eq_ignore_ascii_case(&handler.executable))
        .map(|p| p.pid)
        .collect();
    ensure!(
        !game_processes.is_empty(),
        "no {} started with the session",
        handler.executable
    );
    println!("game process {game_processes:?}");
    if run == Run::User {
        let names = nucleus
            .iter()
            .map(|(pid, _)| ("NucleusCoop.exe", *pid))
            .chain(
                game_processes
                    .iter()
                    .map(|pid| (handler.executable.as_str(), *pid)),
            );
        for (name, pid) in names {
            let (elevated, integrity) = token_rights(pid)?;
            standard_rights(name, pid, elevated, integrity)?;
        }
        println!("rights: NucleusCoop.exe and the game unelevated at medium integrity");
    }
    // The window streams through WGC, on the primary display's GPU.
    let display = butterpollo_win_graphics::capture::primary_display().context("no display")?;
    let _com = butterpollo_win_graphics::capture::ComGuard::new()?;
    let target = instance.capture_target(0, &display)?;
    let mut capture = butterpollo_stream::win_bridge::open_capture(
        &target,
        "wgc",
        false,
        &Default::default(),
        warnings.clone(),
    )?;
    let (mut staging, mut best) = (None, 0.0f64);
    let deadline = Instant::now() + Duration::from_secs(60);
    let image = loop {
        if let Some(frame) = capture.next_gpu(0)? {
            let image = frame.readback(&mut staging)?;
            let share = lit(&image)?;
            best = best.max(share);
            if share >= 0.05 {
                break image;
            }
        }
        ensure!(
            Instant::now() < deadline,
            "the window gave no frame that is not black within 60 s (at most {:.1} % lit)",
            best * 100.0
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    println!(
        "capture {target}: {} {}x{}, {:.0} % lit",
        capture.backend(),
        image.width,
        image.height,
        lit(&image)? * 100.0
    );
    drop(capture);
    // A held, then released, through the player's slot to its XInput index.
    let index = instance.session(|s| s.slots[0].pad);
    let xinput = pads::System::open()?;
    for (buttons, expected) in [(0x1000u32, 0x1000u16), (0, 0)] {
        instance
            .input(
                &owner.uuid,
                Input::Controller {
                    id: 0,
                    active: 1,
                    buttons,
                    left_trigger: 0,
                    right_trigger: 0,
                    sticks: [0; 4],
                },
            )
            .map_err(|dropped| anyhow!("the pad input was dropped: {dropped:?}"))?;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let seen = xinput.buttons(index)?;
            if seen.map(|b| b & 0x1000) == Some(expected) {
                break;
            }
            ensure!(
                Instant::now() < deadline,
                "XInput index {index} shows {seen:?}, not A {}",
                if expected == 0 { "released" } else { "held" }
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    println!("pad A held and released at XInput index {index}");
    let holding = Instant::now();
    while holding.elapsed() < hold {
        no_message_box(&mut desktop, &boxes_before, since, &handler.executable)?;
        std::thread::sleep(Duration::from_millis(250));
    }
    let phase = instance.session(|s| s.phase.clone());
    ensure!(phase == Phase::Running, "after {hold:?}: {phase:?}");
    let stopping = Instant::now();
    ensure!(registry.end(seat.session), "the session was gone");
    println!("stopped in {:?}", stopping.elapsed());
    let left: Vec<String> = butterpollo_win_io::process::processes()?
        .into_iter()
        .filter(|p| p.started >= since)
        .filter(|p| {
            p.name.eq_ignore_ascii_case("NucleusCoop.exe")
                || p.name.eq_ignore_ascii_case(&handler.executable)
        })
        .map(|p| format!("{} {}", p.name, p.pid))
        .collect();
    ensure!(left.is_empty(), "still running after the stop: {left:?}");
    // The session put them back itself; the cleanup only covers a failure.
    let changes = layer_changes(
        &cleanup.layers_before,
        &UserHive::signed_in()?.values()?,
        &entry.exe_path,
        &cleanup.contents,
        &handler.executable,
    );
    ensure!(
        changes.is_empty(),
        "the compatibility settings changed: {changes:?}"
    );
    println!("compatibility settings as before");
    ensure!(
        crashes()? == crashes_before,
        "Nucleus Co-op wrote to {} or {}",
        crashlogs[0].display(),
        crashlogs[1].display()
    );
    println!("crashlog.txt unchanged");
    // No journal of Settings.ini in either: the copy's is never one.
    let journal_in = |folder: &Path| {
        let reader = (folder == copy).then_some(&user);
        user_copy::as_user(reader, || folder.join("Rubylight-settings.json").exists())
    };
    ensure!(
        !journal_in(&copy)? && (run == Run::User || !journal_in(&folder)?),
        "the session left Rubylight-settings.json behind"
    );
    if cold {
        match run {
            // The session put the user's values back once Nucleus closed.
            Run::Elevated => {
                settings_are(&settings_of(&settings)?, &COLD)?;
                println!("Settings.ini has the user's values back");
            }
            // Rubylight's own copy keeps them; the host's was never touched.
            Run::User => {
                settings_are(&settings_of(&copy_settings)?, &FORCED)?;
                settings_are(&settings_of(&settings)?, &COLD)?;
                println!("Settings.ini of the user's copy keeps the session's values");
            }
        }
    }
    for warning in warnings.snapshot() {
        println!("warning {}: {}", warning.code, warning.message);
    }
    cleanup.run()?;
    let after = pc.catalog(&folder)?;
    let own = |catalog: &Catalog| -> Vec<String> {
        let mut own: Vec<String> = catalog
            .handlers
            .iter()
            .filter(|h| h.own)
            .map(|h| h.file.clone())
            .collect();
        own.sort();
        own
    };
    ensure!(
        own(&after) == own(&before) && after.games == before.games,
        "the handlers or game list are not as before: {:?} and {:?}, were {:?} and {:?}",
        own(&after),
        after.games,
        own(&before),
        before.games
    );
    let copy_after = (own_in(&copy.join("handlers"))?, list_of(&copy_list)?);
    ensure!(
        copy_after == copy_before,
        "Rubylight's handlers or the game list in the user's copy are not as before: {:?}, were {:?}",
        copy_after.0,
        copy_before.0
    );
    println!("handlers and game list as before");
    Ok(())
}
