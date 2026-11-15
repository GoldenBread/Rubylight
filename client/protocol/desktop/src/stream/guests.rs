//! Guests from other devices on the device playing (feature `session.guests`, docs/multiclient.md
//! in the host repo): a device that wants to join this game opens the stream menu on its Guests
//! page, with Let in and Deny, and the stream menu's Guests row lets it in or turns it down while
//! the host waits, changes what each guest may do, hands over the keyboard and mouse, and kicks or
//! blocks a guest. Read every 2 s while this device manages the guests, every 10 s otherwise, and
//! never on a PC that doesn't offer them.

use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::Ui;
use nightjar_core::http::{
    AppRole, Feature, GuestAction, HostClient, SessionGuest, SessionGuests, FEATURE_SESSION_GUESTS, GUESTS_POLL_MS,
    GUEST_CONTROLLER, GUEST_KEYBOARD_MOUSE, GUEST_VIEW,
};

use crate::strings::{tr, trf};

use super::pipeline::{lock, Notice};
use super::Streaming;

/// How often a device that doesn't manage the guests looks again: it may become the device playing.
const IDLE_POLL: Duration = Duration::from_secs(10);

/// What came back from the PC.
pub enum GuestsAnswer {
    /// The PC doesn't offer guests: it is not asked again in this stream.
    NotOffered,
    Read(Result<SessionGuests, String>),
    Changed(Result<SessionGuests, String>),
}

#[derive(Default)]
pub struct Guests {
    /// None until the capabilities were read.
    offered: Option<bool>,
    read: Option<SessionGuests>,
    /// The requests already announced, so each is announced once.
    announced: Vec<u32>,
    /// The name of a new join request to put on screen, until the stream takes it.
    prompt: Option<String>,
    busy: bool,
    /// An action pressed while a read or another action was out: sent once that answers.
    pending: Option<GuestAction>,
    next: Option<Instant>,
}

impl Guests {
    /// Whether the stream menu shows the Guests row: this device manages the guests of its game.
    pub fn shown(&self) -> bool {
        self.read.as_ref().is_some_and(|r| r.manage)
    }

    pub fn read(&self) -> Option<&SessionGuests> {
        self.read.as_ref()
    }

    /// Takes the PC's answer. Returns the toasts to show: a failed change. A new join request is
    /// put on screen through [`Self::take_prompt`].
    pub fn answered(&mut self, answer: GuestsAnswer, now: Instant) -> Vec<String> {
        self.busy = false;
        match answer {
            GuestsAnswer::NotOffered => {
                self.offered = Some(false);
                Vec::new()
            }
            GuestsAnswer::Read(Ok(read)) | GuestsAnswer::Changed(Ok(read)) => {
                self.offered = Some(true);
                self.next = Some(now + poll_after(Some(&read)));
                if let Some(name) = announcements(&read, &mut self.announced).into_iter().next() {
                    self.prompt.get_or_insert(name);
                }
                self.read = Some(read);
                Vec::new()
            }
            GuestsAnswer::Read(Err(error)) => {
                eprintln!("nightjar: guests not read: {error}");
                self.next = Some(now + poll_after(self.read.as_ref()));
                Vec::new()
            }
            GuestsAnswer::Changed(Err(error)) => vec![trf("Could not change the guests: {}", &[&error])],
        }
    }

    /// Whether a read is due at `now`.
    fn due(&self, now: Instant) -> bool {
        self.offered != Some(false) && !self.busy && self.pending.is_none() && self.next.is_none_or(|next| now >= next)
    }

    /// The name of a join request that came in since the last call: the stream opens its menu on
    /// the Guests page for it.
    pub fn take_prompt(&mut self) -> Option<String> {
        self.prompt.take()
    }

    /// `action` to send now, or None when a read or another action is out: then it is kept and
    /// [`Self::take_pending`] hands it over once that answers. A later press replaces it.
    fn queue(&mut self, action: GuestAction) -> Option<GuestAction> {
        if self.busy {
            self.pending = Some(action);
            return None;
        }
        self.busy = true;
        Some(action)
    }

    /// The action kept while the PC was busy, once it answered.
    pub fn take_pending(&mut self) -> Option<GuestAction> {
        if self.busy { None } else { self.pending.take() }
    }
}

/// How long until the next read.
fn poll_after(read: Option<&SessionGuests>) -> Duration {
    if read.is_some_and(|r| r.manage) { Duration::from_millis(GUESTS_POLL_MS) } else { IDLE_POLL }
}

/// The names of the join requests not announced yet; only the device that answers them sees them.
fn announcements(read: &SessionGuests, announced: &mut Vec<u32>) -> Vec<String> {
    let new: Vec<_> = read.new_requests(announced).map(|r| (r.id, r.name.clone())).collect();
    new.into_iter()
        .map(|(id, name)| {
            announced.push(id);
            name
        })
        .collect()
}

/// What a permission is called.
pub fn permission_label(permission: &str) -> &'static str {
    match permission {
        GUEST_VIEW => tr("View only"),
        GUEST_KEYBOARD_MOUSE => tr("Controller, keyboard and mouse"),
        _ => tr("Controller"),
    }
}

/// One guest's line: its name, what it may do, the keyboard if it holds it, and away.
pub fn guest_line(read: &SessionGuests, guest: &SessionGuest) -> String {
    let mut line = format!("{} · {}", guest.name, permission_label(&guest.permission));
    if read.keyboard_mouse == Some(guest.id) {
        line.push_str(" · ");
        line.push_str(tr("has the keyboard"));
    }
    if !guest.streaming {
        line.push_str(" · ");
        line.push_str(tr("away"));
    }
    line
}

impl Streaming {
    fn guests_client(&self) -> Option<Arc<HostClient>> {
        if self.facts.role != AppRole::Stream || !self.plan.info.capabilities.has(Feature::NativeApi) {
            return None;
        }
        lock(&self.shared.host_client).clone()
    }

    /// Reads the guests when due, on a thread; called with the notices.
    pub fn poll_guests(&mut self) {
        let now = Instant::now();
        if !self.guests.due(now) {
            return;
        }
        let Some(client) = self.guests_client() else { return };
        let (shared, offered) = (self.shared.clone(), self.guests.offered);
        self.guests.busy = true;
        std::thread::spawn(move || {
            if offered.is_none() {
                match client.native_capabilities() {
                    Ok(caps) if !caps.features.iter().any(|f| f == FEATURE_SESSION_GUESTS) => {
                        return shared.notify(Notice::Guests(GuestsAnswer::NotOffered));
                    }
                    Ok(_) => {}
                    Err(e) => return shared.notify(Notice::Guests(GuestsAnswer::Read(Err(e.to_string())))),
                }
            }
            shared.notify(Notice::Guests(GuestsAnswer::Read(client.session_guests().map_err(|e| e.to_string()))));
        });
    }

    /// Sends one action, or keeps it until the read or action out answers; the PC's answer is the
    /// list the menu shows next.
    pub fn guest_action(&mut self, action: GuestAction) {
        let Some(client) = self.guests_client() else { return };
        let Some(action) = self.guests.queue(action) else { return };
        let shared = self.shared.clone();
        std::thread::spawn(move || {
            let answer = client.guest_action(&action).map_err(|e| e.to_string());
            shared.notify(Notice::Guests(GuestsAnswer::Changed(answer)));
        });
    }

    /// The Guests row's contents in the stream menu.
    pub fn show_guests(&mut self, ui: &mut Ui) {
        let Some(read) = self.guests.read().cloned() else { return };
        let mut chosen = None;
        // Opened for a join request, nothing has the focus yet: Let in takes it, as on Android.
        let mut focus_first = ui.ctx().memory(|m| m.focused().is_none());
        for request in &read.waiting {
            ui.label(trf("{} wants to join your game", &[&request.name]));
            ui.horizontal(|ui| {
                let let_in = ui.button(tr("Let in"));
                if std::mem::take(&mut focus_first) {
                    let_in.request_focus();
                }
                if let_in.clicked() {
                    chosen = Some(GuestAction::Approve { id: request.id, permission: None });
                }
                if ui.button(tr("Deny")).clicked() {
                    chosen = Some(GuestAction::Deny(request.id));
                }
            });
        }
        if read.guests.is_empty() && read.waiting.is_empty() {
            ui.weak(tr("Nobody has joined your game"));
        }
        for guest in &read.guests {
            ui.separator();
            ui.label(guest_line(&read, guest));
            ui.horizontal_wrapped(|ui| {
                for permission in [GUEST_VIEW, GUEST_CONTROLLER, GUEST_KEYBOARD_MOUSE] {
                    if permission != guest.permission && ui.button(permission_label(permission)).clicked() {
                        chosen = Some(GuestAction::Permission { id: guest.id, permission: permission.to_owned() });
                    }
                }
                if guest.permission == GUEST_KEYBOARD_MOUSE && guest.streaming {
                    if read.keyboard_mouse == Some(guest.id) {
                        if ui.button(tr("Take back keyboard")).clicked() {
                            chosen = Some(GuestAction::KeyboardMouse(None));
                        }
                    } else if ui.button(tr("Give keyboard")).clicked() {
                        chosen = Some(GuestAction::KeyboardMouse(Some(guest.id)));
                    }
                }
                if ui.button(tr("Kick")).clicked() {
                    chosen = Some(GuestAction::Kick(guest.id));
                }
                if ui.button(tr("Block")).clicked() {
                    chosen = Some(GuestAction::Revoke(guest.id));
                }
            });
        }
        if let Some(action) = chosen {
            self.guest_action(action);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nightjar_core::http::GuestRequest;

    fn read(manage: bool, waiting: &[(u32, &str)]) -> SessionGuests {
        SessionGuests {
            manage,
            waiting: waiting
                .iter()
                .map(|(id, name)| GuestRequest { id: *id, name: (*name).into(), seconds_left: 40 })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn each_request_is_put_on_screen_once_and_only_to_the_manager() {
        let mut guests = Guests::default();
        let now = Instant::now();
        assert!(guests.answered(GuestsAnswer::Read(Ok(read(true, &[(3, "Tablet")]))), now).is_empty(), "a prompt, not a toast");
        assert_eq!(guests.take_prompt().as_deref(), Some("Tablet"));
        guests.answered(GuestsAnswer::Read(Ok(read(true, &[(3, "Tablet")]))), now);
        assert_eq!(guests.take_prompt(), None);
        assert!(guests.shown());
        let mut other = Guests::default();
        other.answered(GuestsAnswer::Read(Ok(read(false, &[(3, "Tablet")]))), now);
        assert_eq!(other.take_prompt(), None);
        assert!(!other.shown());
    }

    #[test]
    fn an_answer_pressed_during_a_read_is_sent_once_the_read_answers() {
        let mut guests = Guests::default();
        let now = Instant::now();
        guests.answered(GuestsAnswer::Read(Ok(read(true, &[(3, "Tablet")]))), now);
        // A read is out when Let in is pressed: kept, not dropped, and no read starts meanwhile.
        guests.busy = true;
        assert_eq!(guests.queue(GuestAction::Approve { id: 3, permission: None }), None);
        assert_eq!(guests.take_pending(), None, "still waiting for the read");
        assert!(!guests.due(now + IDLE_POLL * 10));
        guests.answered(GuestsAnswer::Read(Ok(read(true, &[(3, "Tablet")]))), now);
        assert_eq!(guests.take_pending(), Some(GuestAction::Approve { id: 3, permission: None }));
        assert_eq!(guests.take_pending(), None);
        // With nothing out, an action goes at once.
        assert_eq!(guests.queue(GuestAction::Deny(3)), Some(GuestAction::Deny(3)));
        assert!(guests.busy);
    }

    #[test]
    fn a_pc_without_guests_is_not_asked_again_and_the_manager_reads_often() {
        let now = Instant::now();
        let mut guests = Guests::default();
        assert!(guests.due(now));
        guests.answered(GuestsAnswer::NotOffered, now);
        assert!(!guests.due(now + IDLE_POLL * 10));
        let mut manager = Guests::default();
        manager.answered(GuestsAnswer::Read(Ok(read(true, &[]))), now);
        assert!(!manager.due(now));
        assert!(manager.due(now + Duration::from_millis(GUESTS_POLL_MS)));
        let mut watcher = Guests::default();
        watcher.answered(GuestsAnswer::Read(Ok(read(false, &[]))), now);
        assert!(!watcher.due(now + Duration::from_millis(GUESTS_POLL_MS)));
        assert!(watcher.due(now + IDLE_POLL));
    }

    #[test]
    fn a_failed_change_is_shown_and_a_guest_line_names_the_keyboard_and_away() {
        let mut guests = Guests::default();
        let toasts = guests.answered(GuestsAnswer::Changed(Err("gone".into())), Instant::now());
        assert_eq!(toasts, ["Could not change the guests: gone"]);
        let mut list = read(true, &[]);
        list.keyboard_mouse = Some(2);
        let guest = SessionGuest {
            id: 2,
            name: "Phone".into(),
            permission: GUEST_KEYBOARD_MOUSE.into(),
            streaming: false,
            since: 0,
            expires: None,
        };
        assert_eq!(guest_line(&list, &guest), "Phone · Controller, keyboard and mouse · has the keyboard · away");
    }
}
