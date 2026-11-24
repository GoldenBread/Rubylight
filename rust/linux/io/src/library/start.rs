//! Starting an imported game through its launcher, on Linux.
//!
//! # Where the game runs
//!
//! A game started through a Steam client that is already running starts in that client's
//! own environment (its X or Wayland display, its audio): `steam steam://rungameid/N` only
//! hands the address to the running client over `~/.steam/steam.pipe` and exits. Heroic and
//! Lutris pass their address to a running instance the same way. The host therefore streams
//! the user's desktop for a library game (`display_session::prepare_stream`) and starts its
//! launcher there, with the host's own environment. [`steam_elsewhere`] still refuses,
//! visibly, a Steam that runs on another display than the one the game is meant for.

use anyhow::{Context, Result, bail};
use butterpollo_session::library::Launch;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The program and leading arguments that open an address of `scheme`, when its launcher is
/// installed: the launcher on `PATH`, else its Flatpak.
pub fn command_for(scheme: &str) -> Option<(PathBuf, Vec<OsString>)> {
    let (program, flatpak, extra): (&str, &str, &[&str]) = match scheme {
        // No window in the stream's session for the friends list or the chat, and the main
        // window stays hidden: the game is what the stream shows.
        "steam" => (
            "steam",
            "com.valvesoftware.Steam",
            &["-silent", "-nochatui", "-nofriendsui"],
        ),
        "heroic" => ("heroic", "com.heroicgameslauncher.hgl", &[]),
        "lutris" => ("lutris", "net.lutris.Lutris", &[]),
        _ => return None,
    };
    let arguments = |first: &[&str]| -> Vec<OsString> {
        first.iter().chain(extra).map(OsString::from).collect()
    };
    if let Some(path) = on_path(program) {
        return Some((path, arguments(&[])));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    if home.join(".var/app").join(flatpak).is_dir() {
        let flatpak_program = on_path("flatpak")?;
        // `flatpak run APP ARGS`: the app's arguments follow its id.
        return Some((flatpak_program, arguments(&["run", flatpak])));
    }
    None
}

/// Whether the launcher of `scheme` is installed.
pub fn installed(scheme: &str) -> bool {
    command_for(scheme).is_some()
}

fn on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|folder| folder.join(name))
        .find(|path| path.is_file())
}

/// The scheme of a launch address.
fn scheme(uri: &str) -> Result<&str> {
    let scheme = uri.split_once(':').context("Invalid launcher URI")?.0;
    Ok(scheme)
}

/// Starts the game the launcher address `action` names. The launcher and its game outlive
/// this call (and a stream's end ends them with the session around them).
pub fn launch(
    action: &Launch,
    directory: Option<&Path>,
    elevated: bool,
    environment: &BTreeMap<String, String>,
) -> Result<()> {
    use crate::process::{Process, Target};
    // A folder the game never had (a Lutris game without one) is not a working directory.
    let directory = directory.filter(|folder| folder.is_dir());
    let process = match action {
        Launch::Uri { uri } => {
            let scheme = scheme(uri)?;
            let Some((program, mut arguments)) = command_for(scheme) else {
                bail!(
                    "The {scheme} launcher is unavailable. Install it, or install it as a Flatpak."
                );
            };
            if scheme == "steam"
                && let Some(reason) = steam_elsewhere(environment)
            {
                bail!("{reason}");
            }
            arguments.push(uri.into());
            Process::spawn(
                &program,
                &arguments,
                directory,
                Target::User { elevated },
                environment,
                true,
            )?
        }
        Launch::Program { program, arguments } => Process::shell(
            &format!("{} {arguments}", shell_quote(program)),
            directory,
            elevated,
            environment,
        )?,
    };
    // The launcher may already serve other games, and the game outlives the launcher's
    // own command: only the stream's session ends them.
    process.leave_children_running_or_warn();
    Ok(())
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// Why a Steam game cannot start now, when a Steam client runs outside the session the
/// game would start in: its display differs from the one `environment` (or this host's own)
/// names. `None` when no Steam runs, or when it runs in that session, or when where it runs
/// cannot be told.
pub fn steam_elsewhere(environment: &BTreeMap<String, String>) -> Option<String> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let pid = [
        ".steam/steam.pid",
        ".var/app/com.valvesoftware.Steam/.steam/steam.pid",
    ]
    .iter()
    .find_map(|file| std::fs::read_to_string(home.join(file)).ok())
    .and_then(|text| text.trim().parse::<u32>().ok())?;
    let running = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
    let wanted = |name: &str| {
        environment
            .get(name)
            .cloned()
            .or_else(|| std::env::var(name).ok())
    };
    different_session(&running, wanted("DISPLAY"), wanted("WAYLAND_DISPLAY")).then(|| {
        "Steam already runs outside the stream's session, so the game would start on the desktop. Quit Steam there, then start the game again."
            .into()
    })
}

/// Whether the NUL-separated environment `running` of a Steam client names another display
/// than the wanted ones.
fn different_session(running: &[u8], display: Option<String>, wayland: Option<String>) -> bool {
    let value = |name: &str| {
        running
            .split(|byte| *byte == 0)
            .filter_map(|entry| std::str::from_utf8(entry).ok()?.split_once('='))
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.to_owned())
    };
    let differs = |name: &str, wanted: Option<String>| matches!((value(name), wanted), (Some(running), Some(wanted)) if running != wanted);
    // Steam's own display must match the session's on every display it names.
    differs("DISPLAY", display) || differs("WAYLAND_DISPLAY", wayland)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_steam_on_another_display_is_elsewhere() {
        let running = b"HOME=/h\0DISPLAY=:0\0WAYLAND_DISPLAY=wayland-0\0";
        assert!(different_session(running, Some(":7".into()), None));
        assert!(different_session(running, None, Some("gamescope-0".into())));
        assert!(!different_session(
            running,
            Some(":0".into()),
            Some("wayland-0".into())
        ));
        // Where Steam runs or where the game should is not known: not a refusal.
        assert!(!different_session(b"HOME=/h\0", Some(":7".into()), None));
        assert!(!different_session(running, None, None));
    }

    #[test]
    fn addresses_of_other_schemes_have_no_launcher() {
        assert!(command_for("file").is_none());
        assert!(command_for("javascript").is_none());
        assert_eq!(scheme("lutris:rungameid/3").unwrap(), "lutris");
        assert!(scheme("no scheme").is_err());
    }

    #[test]
    fn shell_quoting_keeps_a_quote_inside_the_word() {
        assert_eq!(shell_quote("a'b c"), "'a'\\''b c'");
    }
}
