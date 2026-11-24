//! Pre-launch Neural Rendering integrity checks and interrupted-update
//! completion.
use super::{
    KEY,
    cache::launch_cache,
    folder::{Request, Response},
    game_check::{
        install, legacy_exe, placed_exe, reached_by, set_key, steam_refusal, steam_roots,
    },
    install::{fail_job, start_job, update_job},
    process::run_folder,
};
use crate::state::Shared;
use anyhow::{Context, Result, anyhow, bail};
use butterpollo_session::{neural, state::App};
use serde_json::Value;
use std::path::PathBuf;

/// Before a launch: the files must still be as placed and the game must not
/// have gained anti-cheat; an update cut short is finished. No other
/// release is applied. Rubylight 3.0's AMDNR is checked the same way, with
/// nothing to finish an update from. Fails the launch loudly otherwise.
/// Blocks.
pub fn before_launch(h: &Shared, app: &App) -> Result<()> {
    let (exe, legacy) = match (placed_exe(app), legacy_exe(app)) {
        (Some(exe), _) => (exe, false),
        (None, Some(exe)) => (exe, true),
        (None, None) => return Ok(()),
    };
    let uuid = app
        .extra
        .get("uuid")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    // The launch's check and update hold the switch's slot: a disable or a
    // delete meanwhile would race the update's files and key.
    if start_job(h, &uuid, "update", "place").is_err() {
        bail!(
            "Neural Rendering is busy with {}; launch it again once that is done",
            app.name
        );
    }
    // The error reaches the client as the launch's status message: it names
    // no provider path, which carries the mod's name; the log has the whole.
    let result = launch_check(h, app, &uuid, exe, legacy).map_err(|error| {
        let full = format!("{error:#}");
        tracing::warn!(app = %app.name, error = %full, "Neural Rendering's launch check failed");
        anyhow!(neural::shown(&full))
    });
    match &result {
        Ok(()) => update_job(h, &uuid, |job| job.phase = "done"),
        Err(error) => fail_job(h, &uuid, error.to_string(), false),
    }
    result
}

fn launch_check(h: &Shared, app: &App, uuid: &str, exe: PathBuf, legacy: bool) -> Result<()> {
    let _reading = h.neural.reading();
    // The hold lives until this check returns: the helper copies from the
    // cached release while it runs.
    let (package, _held) = if legacy {
        (None, None)
    } else {
        match launch_cache(h).context(
            "Neural Rendering's download can't be used; turn frame generation off and on again",
        )? {
            Some((package, held)) => (Some(package), Some(held)),
            None => (None, None),
        }
    };
    let paths = reached_by(app, None, &exe);
    let response = match steam_refusal(app, &paths, &steam_roots(h)) {
        Some(reason) => Response::Refused { reason },
        None => run_folder(
            Request::Prepare {
                exe,
                install: install(app),
                package,
            },
            &|_, _| {},
        )?,
    };
    match response {
        Response::Placed {
            exe,
            release,
            proxy,
            updated,
            mode,
        } => {
            if updated && !legacy {
                tracing::info!(app = %app.name, %release, %proxy, "Neural Rendering updated");
                let value = neural::app_key_value(&exe, &release, &proxy, mode);
                if let Err(error) = set_key(h, uuid, KEY, Some(value)) {
                    tracing::warn!(app = %app.name, error = %format!("{error:#}"), "Neural Rendering: the updated release could not be saved with the app");
                }
            }
            Ok(())
        }
        Response::Refused { reason } => {
            tracing::warn!(app = %app.name, %reason, "Neural Rendering refused the launch");
            let what = if legacy {
                "the earlier Neural Rendering upscaling is in"
            } else {
                "frame generation is on for"
            };
            bail!(
                "{what} {}, but {reason}; remove it on the game's page",
                app.name
            )
        }
        Response::Error { message } | Response::ChooseExe { reason: message } => {
            tracing::warn!(app = %app.name, error = %message, "Neural Rendering's check failed before the launch");
            bail!("{message}")
        }
        Response::Removed { .. }
        | Response::Left { .. }
        | Response::Unreachable { .. }
        | Response::NotWritable { .. }
        | Response::Found { .. } => {
            bail!("unexpected helper answer")
        }
    }
}
