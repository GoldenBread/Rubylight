//! Child helper execution (`--neural-probe` and `--neural-folder`) and SYSTEM
//! fallback handling for Neural Rendering.
use super::folder::{self, Request, Response, Writer};
use anyhow::{Context, Result, bail};
use std::{
    io::{BufRead, Read, Write},
    sync::mpsc,
    time::{Duration, Instant},
};

/// Run the host exe with `flag`, `input` as its first line, and hand each
/// line it prints to `line`. As the signed-in user when this host is SYSTEM.
#[cfg_attr(
    any(test, feature = "test-support"),
    allow(dead_code, reason = "tests run the folder work in-process")
)]
pub(super) fn run_child(
    flag: &str,
    input: &str,
    timeout: Duration,
    mut line: impl FnMut(&str),
) -> Result<()> {
    use butterpollo_win_io::process::{Process, Target};
    enum Child {
        User(Process),
        Own(std::process::Child),
    }
    let program = std::env::current_exe()?;
    let (mut child, stdin, stdout): (Child, Box<dyn Write>, Box<dyn Read + Send>) =
        if butterpollo_win_io::process::is_system() {
            let mut piped = Process::spawn_piped(
                &program,
                &[flag.into()],
                Target::User { elevated: false },
            )
            .context(
                "cannot start the helper as the signed-in user; is anyone signed in to Windows?",
            )?;
            let stdin = piped.stdin.take().context("no helper input")?;
            let stdout = piped.stdout.take().context("no helper output")?;
            (
                Child::User(piped.process),
                Box::new(stdin),
                Box::new(stdout),
            )
        } else {
            use std::os::windows::process::CommandExt;
            let mut own = std::process::Command::new(&program)
                .arg(flag)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                .spawn()
                .context("cannot start the helper")?;
            let stdin = own.stdin.take().context("no helper input")?;
            let stdout = own.stdout.take().context("no helper output")?;
            (Child::Own(own), Box::new(stdin), Box::new(stdout))
        };
    let mut stdin = stdin;
    // A helper that died early closed its input; its output says why.
    let _ = stdin.write_all(format!("{input}\n").as_bytes());
    drop(stdin);
    let (sender, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for text in std::io::BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
        {
            if sender.send(text).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + timeout;
    loop {
        match lines.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(text) => line(&text),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                match &mut child {
                    Child::User(process) => drop(process.terminate()),
                    Child::Own(own) => drop(own.kill()),
                }
                bail!("the helper did not finish within {} s", timeout.as_secs());
            }
        }
    }
    match &mut child {
        Child::User(process) => drop(process.wait(Duration::from_secs(10))),
        Child::Own(own) => drop(own.wait()),
    }
    Ok(())
}

/// Run one folder request: in a helper with the signed-in user's token when
/// this host is SYSTEM (game folders are the user's), here otherwise. A
/// folder the user may not write goes to [`as_system`].
pub(super) fn run_folder(request: Request, progress: &dyn Fn(u64, u64)) -> Result<Response> {
    let system = butterpollo_win_io::process::is_system();
    let answer = if !system || cfg!(any(test, feature = "test-support")) {
        folder::handle(request.clone(), Writer::User, progress)
    } else {
        let mut answer = None;
        run_child(
            folder::FLAG,
            &serde_json::to_string(&request)?,
            Duration::from_secs(30 * 60),
            |line| {
                if let Some(json) = line.strip_prefix(folder::ANSWER) {
                    answer = serde_json::from_str(json).ok();
                } else if let Some((done, total)) = line
                    .strip_prefix(folder::PROGRESS)
                    .and_then(|rest| rest.split_once(' '))
                {
                    progress(done.parse().unwrap_or(0), total.parse().unwrap_or(0));
                }
            },
        )?;
        answer.context("the Neural Rendering helper gave no answer")?
    };
    Ok(as_system(answer, system, || {
        folder::handle(request, Writer::System, progress)
    }))
}

/// The helper's answer, but for a folder the signed-in user may not write
/// (a game in Program Files) this SYSTEM host's own: it writes the folder
/// only when nobody but administrators may change it, and refuses
/// otherwise ([`Writer::System`]). A host that is not SYSTEM refuses.
pub fn as_system(answer: Response, system: bool, again: impl FnOnce() -> Response) -> Response {
    match answer {
        Response::NotWritable { folder } if system => {
            tracing::info!(folder = %folder.display(), "Neural Rendering: only administrators may write the game's folder; the service checks it and writes it itself");
            again()
        }
        Response::NotWritable { folder } => Response::Refused {
            reason: format!(
                "only administrators may change {}, and this Rubylight doesn't run as its service, which could",
                folder.display()
            ),
        },
        answer => answer,
    }
}
