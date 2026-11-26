//! Live host integration test for running a Nucleus Co-op session on this PC.
use super::*;
use anyhow::{Context, Result, ensure};

/// A real session on this PC (hw-queue.md, "Nucleus Co-op session"):
/// Nucleus Co-op 2.4.2 unpacked at `RUBYLIGHT_NUCLEUS_PATH`, as chosen in the
/// console, or an earlier install below `RUBYLIGHT_NUCLEUS_DATA`, the game of
/// handler `RUBYLIGHT_NUCLEUS_HANDLER` started for
/// `RUBYLIGHT_NUCLEUS_PLAYERS`, held `RUBYLIGHT_NUCLEUS_HOLD` seconds and
/// stopped; `RUBYLIGHT_NUCLEUS_STEAM_APP` is the Steam app a game was
/// found from. The user's compatibility settings must be as before
/// afterwards. Run as SYSTEM, like the service.
#[test]
#[ignore = "requires the co-op helper, a handler with its game, the virtual gamepad driver and SYSTEM"]
fn a_session_on_this_pc() -> Result<()> {
    use butterpollo_input_policy::input::Input;
    use butterpollo_session::nucleus::install::EXECUTABLE;
    use butterpollo_win_io::nucleus::{
        desktop::Win32,
        files, install,
        instances::Desktop,
        layers::{self, Layers},
        session::{Start, start_on_pc},
    };
    fn env<T: std::str::FromStr>(name: &str, default: T) -> Result<T>
    where
        T::Err: std::error::Error + Send + Sync + 'static,
    {
        std::env::var(name).map_or(Ok(default), |v| {
            v.parse().with_context(|| format!("{name}={v}"))
        })
    }
    let data = std::path::PathBuf::from(
        std::env::var("RUBYLIGHT_NUCLEUS_DATA").context("set RUBYLIGHT_NUCLEUS_DATA")?,
    );
    let configured = std::env::var("RUBYLIGHT_NUCLEUS_PATH").unwrap_or_default();
    let folder = install::installed(&data, &configured)?
        .context("The co-op helper is not set up: set RUBYLIGHT_NUCLEUS_PATH")?;
    let steam_app: Option<u32> = std::env::var("RUBYLIGHT_NUCLEUS_STEAM_APP")
        .ok()
        .map(|app| app.parse().context("RUBYLIGHT_NUCLEUS_STEAM_APP"))
        .transpose()?;
    let file = std::env::var("RUBYLIGHT_NUCLEUS_HANDLER")
        .context("set RUBYLIGHT_NUCLEUS_HANDLER to a hub handler's file")?;
    let path = folder.join("handlers").join(&file);
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let handler =
        butterpollo_session::nucleus::handler::parse(&file, &text).map_err(anyhow::Error::msg)?;
    let exe_path = files::catalog(&folder, true)?
        .games
        .into_iter()
        .find(|game| game.guid == handler.guid)
        .with_context(|| format!("add {}'s game in the co-op helper first", handler.name))?
        .exe_path;
    println!("handler {} for {}", handler.file, handler.executable);
    let layers_before = layers::UserHive::signed_in()?.values()?;
    let players = env("RUBYLIGHT_NUCLEUS_PLAYERS", 1usize)?;
    let hold = Duration::from_secs(env("RUBYLIGHT_NUCLEUS_HOLD", 20u64)?);
    let since = butterpollo_win_io::process::now();
    let warnings = Arc::new(butterpollo_session::session::Warnings::default());
    let request = Start {
        id: 1,
        kind: if players == 1 {
            butterpollo_session::nucleus::coop::Kind::Background
        } else {
            butterpollo_session::nucleus::coop::Kind::Coop
        },
        folder: &folder,
        user_copy: None,
        handler: &handler,
        exe_path: &exe_path,
        steam_app,
        title: &handler.name,
        players,
        owner: ("hw-queue", "hw-queue"),
        size: (1280, 720),
        instances: None,
    };
    let policy = butterpollo_input_policy::input_policy::Policy::resolve(&Default::default())?;
    let running = start_on_pc(&request, policy, warnings.clone())?;
    println!(
        "pads {:?}",
        running.with_session(|s| s.slots.iter().map(|s| s.pad).collect::<Vec<_>>())
    );
    let started = Instant::now();
    loop {
        match running.with_session(|s| s.phase.clone()) {
            Phase::Running => break,
            Phase::Ended(why) => anyhow::bail!("{why}"),
            Phase::Starting => std::thread::sleep(Duration::from_millis(250)),
        }
    }
    println!(
        "running after {:?}, windows {:?}",
        started.elapsed(),
        running.windows()
    );
    let display = butterpollo_win_graphics::capture::primary_display().context("no display")?;
    let _com = butterpollo_win_graphics::capture::ComGuard::new()?;
    let env_bridge = crate::win_bridge::capture_env();
    for slot in 0..players {
        let target = running.capture_target(slot, &display)?;
        let mut capture = butterpollo_win_graphics::capture::Capture::open_for_stream_with_env(
            &target,
            "wgc",
            false,
            &Default::default(),
            warnings.clone(),
            &env_bridge,
        )?;
        let deadline = Instant::now() + Duration::from_secs(5);
        let image = loop {
            if let Some(image) = capture.next_gpu(0)? {
                break image;
            }
            ensure!(
                Instant::now() < deadline,
                "Player {}'s window gave no frame within 5 s",
                slot + 1
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        println!(
            "player {} capture {target}: {} {}x{}",
            slot + 1,
            capture.backend(),
            image.width,
            image.height
        );
    }
    for slot in 0..players {
        match running.audio(slot) {
            Ok(audio) => println!("player {} audio pid {}", slot + 1, audio.identity.pid),
            Err(error) => println!("player {} audio: {error:#}", slot + 1),
        }
    }
    for buttons in [0x1000, 0] {
        running
            .input(
                "hw-queue",
                Input::Controller {
                    id: 0,
                    active: 1,
                    buttons,
                    left_trigger: 0,
                    right_trigger: 0,
                    sticks: [0; 4],
                },
            )
            .map_err(|dropped| anyhow::anyhow!("the pad input was dropped: {dropped:?}"))?;
        std::thread::sleep(Duration::from_millis(200));
    }
    println!("pad reports {:?}", running.feedback());
    std::thread::sleep(hold);
    let phase = running.with_session(|s| s.phase.clone());
    ensure!(phase == Phase::Running, "after {hold:?}: {phase:?}");
    let stopping = Instant::now();
    running.stop();
    println!("stopped in {:?}", stopping.elapsed());
    for warning in warnings.snapshot() {
        println!("warning {}: {}", warning.code, warning.message);
    }
    let left: Vec<String> = Win32::default()
        .processes()?
        .into_iter()
        .filter(|p| p.started >= since)
        .filter(|p| {
            p.name.eq_ignore_ascii_case(EXECUTABLE)
                || p.name.eq_ignore_ascii_case(&handler.executable)
        })
        .map(|p| format!("{} {}", p.name, p.pid))
        .collect();
    ensure!(left.is_empty(), "still running after the stop: {left:?}");
    let layers_after = layers::UserHive::signed_in()?.values()?;
    ensure!(
        layers_after == layers_before,
        "the compatibility settings changed: {layers_before:?} before, {layers_after:?} after"
    );
    println!("compatibility settings as before");
    Ok(())
}
