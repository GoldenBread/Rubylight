//! Rubylight's root side of the Linux virtual display: a pool of hot-pluggable monitors made
//! with the kernel's own VKMS configfs, and a one-line-per-connection socket service that lets
//! the desktop user plug and unplug them.
//!
//! The broker's protocol and lease rules follow libvirtualdisplay's
//! `linux/packaging/vibeshine-vkms` broker (Nonary/libvirtualdisplay, MIT, Copyright (c) 2026
//! Chase Payne), which Vibepollo installs; the pool uses the kernel's stock VKMS configfs.
//!
//! `pool start [--count N]` and `pool stop` run from a oneshot unit; `serve` runs once per
//! connection of a socket-activated unit (the connection is stdin and stdout). Every path root
//! can be moved with `--configfs`, `--faux`, `--run-dir` and `--sessions-dir`, for tests.
#[cfg(target_os = "linux")]
mod lease;
#[cfg(target_os = "linux")]
mod paths;
#[cfg(target_os = "linux")]
mod peer;
#[cfg(target_os = "linux")]
mod pool;
#[cfg(target_os = "linux")]
mod protocol;
#[cfg(all(target_os = "linux", test))]
mod tests;

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("rubylight-vdisplay-broker: {error:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(target_os = "linux")]
fn run() -> anyhow::Result<()> {
    use anyhow::{Context, bail};
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let paths = paths::Paths::take_from(&mut args)?;
    let mut count = pool::DEFAULT_COUNT;
    if let Some(at) = args.iter().position(|a| a == "--count") {
        let value = args.get(at + 1).context("--count needs a number")?;
        count = value.parse().context("--count is not a number")?;
        args.drain(at..at + 2);
    }
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["pool", "start"] => pool::start(&paths, count),
        ["pool", "stop"] => pool::stop(&paths),
        ["serve"] => protocol::serve_stdio(&paths),
        _ => bail!(
            "usage: rubylight-vdisplay-broker pool start [--count N] | pool stop | serve \
             [--configfs DIR] [--faux DIR] [--run-dir DIR] [--sessions-dir DIR]"
        ),
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("rubylight-vdisplay-broker runs on Linux only");
    std::process::exit(1);
}
