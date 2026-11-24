//! Client connection hooks, application state commands, stream-menu commands, and env expansion.

use super::RunningApp;
use anyhow::Result;
use butterpollo_session::connection_commands::ConnectionCommand as ClientCommand;
use butterpollo_session::state::{App, PrepCommand};
use butterpollo_win_io::process::{CommandCancellation, Process};
use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};

/// The stream values a device's connection commands read. The size is the one
/// the device asked for, which its display keeps: a host `max_stream_resolution`
/// cap shrinks only the coded picture, so `SUNSHINE_CLIENT_WIDTH/HEIGHT` keep
/// reporting the requested size, as they always did.
pub(super) fn client_stream_environment(
    stream: &butterpollo_media_policy::rtsp::Negotiated,
    fps: String,
) -> BTreeMap<String, String> {
    let (width, height) = stream.display_size();
    BTreeMap::from([
        ("SUNSHINE_CLIENT_WIDTH".to_owned(), width.to_string()),
        ("SUNSHINE_CLIENT_HEIGHT".to_owned(), height.to_string()),
        ("SUNSHINE_CLIENT_FPS".to_owned(), fps),
        ("SUNSHINE_CLIENT_HDR".to_owned(), stream.hdr.to_string()),
    ])
}
/// Administrator-configured hooks run once for each connected transport, with
/// its environment saved for disconnect even if the application exits first.
pub struct ClientCommands {
    undo: Vec<ClientCommand>,
    environment: BTreeMap<String, String>,
}
impl ClientCommands {
    pub fn start(h: &crate::state::Shared, s: &crate::state::Session) -> Result<Self> {
        if s.launch.role == butterpollo_session::session::Role::InputOnly
            || !s.launch.client.allows_commands()
            || h.current_app
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|app| !app.allow_client_commands)
        {
            return Ok(Self {
                undo: vec![],
                environment: BTreeMap::new(),
            });
        }
        let environment = h
            .current_app
            .lock()
            .unwrap()
            .as_ref()
            .filter(|app| app.deferred.is_none())
            .map(|app| app.environment.clone());
        let mut environment = match environment {
            Some(environment) => environment,
            None => match butterpollo_win_io::process::user_environment() {
                Ok(environment) => environment,
                Err(error) => {
                    tracing::info!(%error, "no signed-in user; client commands skipped");
                    return Ok(Self {
                        undo: vec![],
                        environment: BTreeMap::new(),
                    });
                }
            },
        };
        environment.insert(
            "SUNSHINE_CLIENT_NAME".into(),
            butterpollo_crypto::pairing::command_device_name(&s.launch.client.name),
        );
        environment.insert("SUNSHINE_CLIENT_UUID".into(), s.launch.client.uuid.clone());
        let fps = if h
            .config
            .read()
            .unwrap()
            .boolean("envvar_compatibility_mode", false)
        {
            s.config.fps.to_string()
        } else {
            butterpollo_media_policy::framegen::Rate(s.config.fps_millihz()).to_string()
        };
        environment.extend(client_stream_environment(&s.config, fps));
        for (key, value) in environment.clone() {
            if let Some(suffix) = key.strip_prefix("SUNSHINE_") {
                environment.insert(format!("APOLLO_{suffix}"), value);
            }
        }
        // A device whose commands cannot be read streams without them, as in
        // the C++ host, instead of failing every stream.
        Ok(
            Self::with_environment(&s.launch.client.extra, environment).unwrap_or_else(|error| {
                tracing::warn!(
                    client = %s.launch.client.name,
                    error = format!("{error:#}"),
                    "device commands cannot be used; streaming without them"
                );
                Self {
                    undo: vec![],
                    environment: BTreeMap::new(),
                }
            }),
        )
    }
    pub(super) fn with_environment(
        extra: &BTreeMap<String, serde_json::Value>,
        environment: BTreeMap<String, String>,
    ) -> Result<Self> {
        let parse = |key: &str| -> Result<Vec<ClientCommand>> {
            butterpollo_session::connection_commands::parse(extra.get(key), key)
                .map_err(anyhow::Error::msg)
        };
        // Validate both lists before any command is started.
        let commands = parse("do")?;
        let hooks = Self {
            undo: parse("undo")?,
            environment,
        };
        hooks.run(&commands);
        Ok(hooks)
    }
    fn run(&self, commands: &[ClientCommand]) {
        for command in commands.iter().filter(|c| !c.cmd.is_empty()) {
            if let Err(error) = expand(&command.cmd, &self.environment).and_then(|cmd| {
                Process::shell_detached(&cmd, None, command.elevated, &self.environment)
            }) {
                tracing::warn!(%error, "client connection command failed");
            }
        }
    }
}
impl Drop for ClientCommands {
    fn drop(&mut self) {
        self.run(&self.undo);
    }
}
pub(super) struct StateCommands {
    pub(super) sender: Option<std::sync::mpsc::Sender<bool>>,
    pub(super) cancel: Arc<CommandCancellation>,
    pub(super) worker: Option<std::thread::JoinHandle<()>>,
}
impl StateCommands {
    pub(super) fn send(&self, active: bool) {
        if let Some(sender) = &self.sender {
            let _ = sender.send(active);
        }
    }
}
impl Drop for StateCommands {
    fn drop(&mut self) {
        if let Err(error) = self.cancel.cancel() {
            tracing::warn!(%error, "application state command cancellation failed");
        }
        // Wake an idle receiver as well as a running command. The worker
        // checks cancellation before consuming any queued transition.
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// A stream-menu command to start: the command line, as administrator or not,
/// and its environment (empty for host-wide commands, which run as before with
/// the signed-in user's).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuRun {
    pub cmd: String,
    pub elevated: bool,
    pub environment: BTreeMap<String, String>,
    /// The app's working folder; empty for host-wide commands.
    pub working: String,
}
impl MenuRun {
    pub fn start(&self) -> Result<()> {
        Process::shell_detached(
            &self.cmd,
            directory(&self.working),
            self.elevated,
            &self.environment,
        )
    }
}

/// The PC commands a device with the PC commands permission sees, in index
/// order: the host-wide `server_cmd` ones (unless the running app hides them),
/// then the running app's own (`menu-cmd`) while it allows device commands.
/// `config` is the settings the caller answers from (serverinfo's snapshot), so the list and the
/// rest of its reply agree.
pub fn pc_command_names(
    h: &crate::state::Shared,
    config: &butterpollo_config::config::Config,
) -> Vec<String> {
    let global = butterpollo_input_policy::menu_commands::global(config.get("server_cmd", "[]"));
    let app = h.current_app.lock().unwrap();
    // The same rule as running one (`pc_command`): a list of commands that do nothing is none.
    let Some((own, exclude)) = device_menu(app.as_ref()) else {
        return Vec::new();
    };
    butterpollo_input_policy::menu_commands::listed(&global, own, exclude)
        .into_iter()
        .map(|(_, command)| command.name.clone())
        .collect()
}

/// The running app's own menu commands and whether it hides the host-wide ones; None while it
/// turned device commands off (`allow-client-commands = false`), when no PC command is listed or
/// run. Without an app, the host-wide ones.
fn device_menu(
    app: Option<&RunningApp>,
) -> Option<(
    &[butterpollo_input_policy::menu_commands::MenuCommand],
    bool,
)> {
    match app {
        Some(app) if !app.allow_client_commands => None,
        Some(app) => Some((&app.menu_commands[..], app.exclude_global_menu)),
        None => Some((&[], false)),
    }
}

/// PC command `index` of [`pc_command_names`], ready to start; an app command
/// is recorded for its undo. None when the running app turned device commands
/// off, the index is out of range, the command is empty, or `expect` names
/// another command (the list changed since the device read it). The caller
/// checks the device's permission.
pub fn pc_command(h: &crate::state::Shared, index: usize, expect: Option<&str>) -> Option<MenuRun> {
    use butterpollo_input_policy::menu_commands::{self, Source};
    let global = menu_commands::global(h.config.read().unwrap().get("server_cmd", "[]"));
    let mut app = h.current_app.lock().unwrap();
    let source = {
        let (own, exclude) = device_menu(app.as_ref())?;
        let (source, command) = *menu_commands::listed(&global, own, exclude).get(index)?;
        if expect.is_some_and(|name| name != command.name) {
            return None;
        }
        source
    };
    match source {
        Source::Global(index) => {
            let command = &global[index];
            (!command.cmd.is_empty()).then(|| MenuRun {
                cmd: command.cmd.clone(),
                elevated: command.elevated,
                ..Default::default()
            })
        }
        Source::App(index) => app.as_mut()?.menu_command(index),
    }
}

/// The folder of the program an app starts, as Vibepollo uses when the app
/// has no working directory: games often load files relative to it.
pub(super) fn inferred_working_dir(command: &str) -> String {
    let target = butterpollo_win_io::process::command_target(command);
    if target.contains("://") {
        return String::new();
    }
    let path = Path::new(target);
    path.is_absolute()
        .then(|| path.parent())
        .flatten()
        .filter(|parent| parent.is_dir())
        .map(|parent| parent.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub(super) fn directory(s: &str) -> Option<&Path> {
    if s.is_empty() {
        None
    } else {
        Some(Path::new(s))
    }
}

/// How long `app` gets to close before it is ended: its own `exit-timeout`, else the host's
/// `exit_timeout` (issue #71), else 10 s; at most 300 s.
pub fn exit_timeout(app: &App, config: &butterpollo_config::config::Config) -> Duration {
    let seconds = app
        .extra
        .get("exit-timeout")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_else(|| config.integer("exit_timeout", 10).max(0) as u64);
    if seconds > 300 {
        tracing::warn!(app = %app.name, seconds, "exit timeout above 300 s; the app gets 300 s to close");
    }
    Duration::from_secs(seconds.min(300))
}
pub fn app_bool(app: &App, name: &str, default: bool) -> bool {
    app.extra.get(name).map_or(default, |v| match v {
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::String(s) => matches!(s.as_str(), "true" | "1" | "yes"),
        _ => default,
    })
}
/// Starts each pause (undo) or resume (do) command without waiting for it, as Apollo does: its
/// auto-pause `pause.ahk` keeps running until `resume.ahk` signals it, so waiting blocked the
/// resume and the 120 s timeout then killed the paused game's script.
pub(super) fn start_state_commands(
    commands: &[PrepCommand],
    active: bool,
    environment: &BTreeMap<String, String>,
    mut start: impl FnMut(&str, bool) -> Result<()>,
) {
    for command in commands {
        let cmd = if active { &command.r#do } else { &command.undo };
        if cmd.is_empty() {
            continue;
        }
        if let Err(error) = expand(cmd, environment).and_then(|cmd| start(&cmd, command.elevated)) {
            tracing::warn!(%error, "application state command failed to start");
        }
    }
}
pub(super) fn expand(value: &str, environment: &BTreeMap<String, String>) -> Result<String> {
    let mut output = String::new();
    let mut rest = value;
    while let Some(index) = rest.find('$') {
        output.push_str(&rest[..index]);
        rest = &rest[index..];
        if let Some(tail) = rest.strip_prefix("$$") {
            output.push('$');
            rest = tail;
        } else if let Some(tail) = rest.strip_prefix("$(") {
            let end = tail
                .find(')')
                .ok_or_else(|| anyhow::anyhow!("unterminated environment variable"))?;
            output.push_str(
                environment
                    .get(&tail[..end].to_uppercase())
                    .map_or("", String::as_str),
            );
            rest = &tail[end + 1..];
        } else {
            output.push('$');
            rest = &rest[1..];
        }
    }
    output.push_str(rest);
    Ok(output)
}
