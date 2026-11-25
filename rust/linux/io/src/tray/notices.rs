//! The tray's notifications on Linux: desktop notifications (`org.freedesktop.Notifications`),
//! shown while the tray icon is, as on Windows. Each one has its own id and a click says which
//! one it was on, so none waits for another: a click on a pairing request opens the PIN, and
//! one on a launch error quits that app, until another app runs.
use super::{Action, Command, command};
use anyhow::Result;
use butterpollo_session::tray::{NoticeAction, QueuedNotice};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Instant,
};
use zbus::{blocking::Connection, message::Type, zvariant::Value};

const NOTIFICATIONS: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
/// The action a click on the notification itself invokes.
const DEFAULT: &str = "default";

/// A notification that is up and does something when clicked.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Up {
    action: NoticeAction,
    request: Option<String>,
}
static UP: Mutex<Vec<(u32, Up)>> = Mutex::new(Vec::new());

fn notice(title: &str, text: &str, action: Option<NoticeAction>) -> QueuedNotice {
    QueuedNotice {
        title: title.to_owned(),
        text: text.to_owned(),
        action,
        request: None,
    }
}

/// Show a notification, if the tray icon is shown.
pub fn notify(title: &str, text: &str) {
    let _ = command(Command::Notify(notice(title, text, None)));
}

/// Show pairing request `request`'s notification; clicking it opens the console at the PIN.
pub fn notify_pairing(request: &str, title: &str, text: &str) {
    let notice = QueuedNotice {
        request: Some(request.to_owned()),
        ..notice(title, text, Some(NoticeAction::EnterPin))
    };
    if let Err(error) = command(Command::Notify(notice)) {
        tracing::debug!(%error, "no tray notification for a pairing request");
    }
}

/// Pairing request `request` ended, so its notification goes.
pub fn pairing_request_ended(request: &str) {
    close_where(|up| up.request.as_deref() == Some(request));
}

/// Called from the host's loop. Nothing waits on Linux, where each notification is its own.
pub fn release_overdue_notices() {}

/// When [`release_overdue_notices`] next has something to release: never on Linux.
pub fn notices_due() -> Option<Instant> {
    None
}

/// Show a notification, or say why it cannot be: no tray icon is shown. Whether the desktop
/// showed it is logged, as the tray's thread hears it.
pub fn try_notify(title: &str, text: &str) -> Result<()> {
    command(Command::Notify(notice(title, text, None)))
}

pub(super) fn launch_error_notice(app: &str, code: u32) -> butterpollo_session::tray::Notice {
    (
        "Launch Error",
        format!(
            "Application {app} exited too fast with code {}. Click here to terminate the stream.",
            code as i32
        ),
    )
}

/// Keep the detached stream open until the user chooses to end it.
pub fn launch_error(app: &str, code: u32) {
    let (title, text) = launch_error_notice(app, code);
    if let Err(error) = command(Command::Notify(notice(
        title,
        &text,
        Some(NoticeAction::QuitApp),
    ))) {
        tracing::warn!(%error, "no tray notification for a launch error");
    }
}

/// Another app runs or none does: a click on a launch error can no longer mean the app that
/// quit, so those notifications go.
pub(super) fn app_changed() {
    close_where(|up| up.action == NoticeAction::QuitApp);
}

fn close_where(ended: impl Fn(&Up) -> bool) {
    let mut up = UP.lock().unwrap();
    let mut closed = Vec::new();
    up.retain(|(id, notice)| {
        let end = ended(notice);
        if end {
            closed.push(*id);
        }
        !end
    });
    drop(up);
    for id in closed {
        let _ = command(Command::Close(id));
    }
}

/// The text as the notification shows it: the body may be markup.
pub(super) fn plain(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The button a notification shows for its action, besides the click on it.
fn button(action: NoticeAction) -> &'static str {
    match action {
        NoticeAction::EnterPin => "Enter PIN",
        NoticeAction::QuitApp => "End the stream",
    }
}

/// Put `notice` up. On the tray's thread.
pub(super) fn show(connection: &Connection, notice: &QueuedNotice) {
    let actions: Vec<&str> = notice
        .action
        .map(|action| vec![DEFAULT, button(action)])
        .unwrap_or_default();
    let hints: HashMap<&str, Value> = HashMap::new();
    let shown = connection
        .call_method(
            Some(NOTIFICATIONS),
            PATH,
            Some(NOTIFICATIONS),
            "Notify",
            &(
                "Rubylight",
                0u32,
                "",
                notice.title.as_str(),
                plain(&notice.text),
                actions,
                hints,
                -1i32,
            ),
        )
        .and_then(|reply| reply.body().deserialize::<u32>());
    match shown {
        Ok(id) => {
            if let Some(action) = notice.action {
                let mut up = UP.lock().unwrap();
                // One per request: a request asked again replaces its notification.
                let replaced: Vec<u32> = up
                    .iter()
                    .filter(|(_, up)| notice.request.is_some() && up.request == notice.request)
                    .map(|(id, _)| *id)
                    .collect();
                up.retain(|(id, _)| !replaced.contains(id));
                up.push((
                    id,
                    Up {
                        action,
                        request: notice.request.clone(),
                    },
                ));
                drop(up);
                for old in replaced {
                    close(connection, old);
                }
            }
        }
        Err(error) => {
            tracing::warn!(%error, title = %notice.title, "the desktop did not show the notification");
        }
    }
}

/// Take notification `id` down. On the tray's thread.
pub(super) fn close(connection: &Connection, id: u32) {
    if let Err(error) = connection.call_method(
        Some(NOTIFICATIONS),
        PATH,
        Some(NOTIFICATIONS),
        "CloseNotification",
        &(id,),
    ) {
        tracing::debug!(%error, id, "a notification was not taken down");
    }
}

/// What a click on notification `id` does, once: it goes with the click.
fn clicked(id: u32) -> Option<Action> {
    let mut up = UP.lock().unwrap();
    let at = up.iter().position(|(up, _)| *up == id)?;
    Some(match up.remove(at).1.action {
        NoticeAction::EnterPin => Action::EnterPin,
        NoticeAction::QuitApp => Action::QuitApp,
    })
}

/// Hear clicks on the notifications and their closing, until `stopped` at the next signal.
pub(super) fn listen(
    connection: &Connection,
    stopped: &Arc<AtomicBool>,
    sender: mpsc::Sender<Action>,
) -> Result<()> {
    let rule = zbus::MatchRule::builder()
        .msg_type(Type::Signal)
        .interface(NOTIFICATIONS)?
        .path(PATH)?
        .build();
    let signals = zbus::blocking::MessageIterator::for_match_rule(rule, connection, None)?;
    let stopped = stopped.clone();
    std::thread::Builder::new()
        .name("tray-notices".into())
        .spawn(move || {
            for signal in signals {
                if stopped.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(signal) = signal else { continue };
                let member = signal.header().member().map(|member| member.to_string());
                match member.as_deref() {
                    Some("ActionInvoked") => {
                        let Ok((id, _key)) = signal.body().deserialize::<(u32, String)>() else {
                            continue;
                        };
                        if let Some(action) = clicked(id)
                            && sender.send(action).is_err()
                        {
                            break;
                        }
                    }
                    Some("NotificationClosed") => {
                        if let Ok((id, _reason)) = signal.body().deserialize::<(u32, u32)>() {
                            UP.lock().unwrap().retain(|(up, _)| *up != id);
                        }
                    }
                    _ => {}
                }
            }
        })?;
    Ok(())
}
