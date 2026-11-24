//! Guest input state and permission-gated gamepad/keyboard/mouse handling inside [`super::Core`].

use super::{
    Core,
    queue::{GUEST_PADS, GuestMessage, MAX_GUEST_TAG, OWNER_PADS, guest_controller, guest_of},
    sink::{Reply, controller_event},
};
use crate::media::ControlSender;
use butterpollo_input_policy::input::required_permission;
use butterpollo_session::webrtc::{
    gamepad::{self, GamepadMessage, MAX_GAMEPADS, Outcome},
    guests::Permission,
    input::InputChannel,
};
use std::time::Instant;

/// A guest sending through the owner's worker.
pub(super) struct Guest {
    /// The tag in its controllers' ids ([`guest_controller`]).
    pub(super) tag: u16,
    permission: Permission,
    /// Its own keys and buttons, released when it leaves.
    pub(super) channel: InputChannel,
    pub(super) control: Option<Reply>,
}

impl Core {
    pub(super) fn guest_joined(&mut self, id: u64, permission: Permission) {
        if self.guests.contains_key(&id) {
            return;
        }
        let Some(tag) = (1..=MAX_GUEST_TAG).find(|t| self.guests.values().all(|g| g.tag != *t))
        else {
            tracing::warn!(guest = id, "WebRTC input: no tag left for a guest");
            return;
        };
        let mode = self.session.stream_mode();
        self.guests.insert(
            id,
            Guest {
                tag,
                permission,
                channel: InputChannel::new(mode.width, mode.height),
                control: None,
            },
        );
    }

    /// One `gamepad` message of a guest, received at `now`. A view-only guest's are dropped
    /// here; a controller-only guest's pads never turn into a mouse.
    pub(super) fn guest_gamepad(&mut self, id: u64, text: &str, now: Instant) {
        self.summary.gamepad_messages += 1;
        let Some(&Guest {
            tag, permission, ..
        }) = self.guests.get(&id)
        else {
            self.summary.dropped += 1;
            return;
        };
        if !permission.gamepads() || !self.allowed(&controller_event()) {
            self.summary.dropped += 1;
            return;
        }
        let message = match gamepad::parse(text) {
            Ok(message) => message,
            Err(error) => return self.invalid("gamepad", &error),
        };
        let remap = |controller_id| guest_controller(tag, controller_id);
        let outcome = match message {
            GamepadMessage::Connected(mut connected) => {
                remap(connected.controller_id).map(|controller_id| {
                    connected.controller_id = controller_id;
                    if self.pads.slot(controller_id).is_none() && !self.guest_pad_free(tag) {
                        tracing::debug!(guest = id, "WebRTC input: a guest's pad over its share");
                        return Outcome {
                            accepted: false,
                            actions: Vec::new(),
                        };
                    }
                    self.pads
                        .connect_with(&connected, permission.keyboard_mouse())
                })
            }
            GamepadMessage::Disconnected { controller_id } => {
                remap(controller_id).map(|controller_id| self.pads.disconnect(controller_id))
            }
            GamepadMessage::State(mut state) => remap(state.controller_id).map(|controller_id| {
                state.controller_id = controller_id;
                self.pads.state(&state, now)
            }),
        };
        let Some(outcome) = outcome else {
            return self.invalid("gamepad", &"a guest's controller id is out of range");
        };
        if !outcome.accepted && outcome.actions.is_empty() {
            self.summary.gamepad_ignored += 1;
        }
        self.received = Some(now);
        self.actions(outcome.actions, now);
        self.received = None;
    }

    /// Whether the guest tagged `tag` may plug in one more pad: [`GUEST_PADS`] each, and the
    /// guests together leave [`OWNER_PADS`] of the slots to the owner.
    fn guest_pad_free(&self, tag: u16) -> bool {
        let guests: Vec<u16> = self
            .pads
            .controller_ids()
            .into_iter()
            .filter_map(|c| guest_of(c).map(|(t, _)| t))
            .collect();
        guests.iter().filter(|&&t| t == tag).count() < GUEST_PADS
            && guests.len() < MAX_GAMEPADS - OWNER_PADS
    }

    /// A guest's message, in the order its transport sent them.
    pub(super) fn guest(&mut self, message: GuestMessage, now: Instant) {
        match message {
            GuestMessage::Joined(id, permission) => self.guest_joined(id, permission),
            GuestMessage::Gamepad(id, text, received) => self.guest_gamepad(id, &text, received),
            GuestMessage::Input(id, text, received) => self.guest_input(id, &text, received),
            GuestMessage::Control(id, sender) => self.guest_control(id, sender),
            GuestMessage::GamepadClosed(id) => self.guest_unplug(id, now),
            GuestMessage::InputClosed(id) => self.guest_release(id, now),
            GuestMessage::Left(id) => self.guest_left(id, now),
            GuestMessage::Lost(id) => self.guest_lost(id, now),
        }
    }

    /// One `input` message of a guest: only with the keyboard and mouse permission, and only
    /// the kinds of event that covers.
    pub(super) fn guest_input(&mut self, id: u64, text: &str, now: Instant) {
        self.summary.input_messages += 1;
        let Some(guest) = self.guests.get_mut(&id) else {
            self.summary.dropped += 1;
            return;
        };
        if !guest.permission.keyboard_mouse() {
            self.summary.dropped += 1;
            return;
        }
        let mask = guest.permission.input_mask();
        let events = match butterpollo_session::webrtc::input::parse(text) {
            Ok(message) => guest.channel.translate(message),
            Err(error) => return self.invalid("input", &error),
        };
        let mut allowed = Vec::with_capacity(events.len());
        for event in events {
            if required_permission(&event) & mask == 0 || !self.allowed(&event) {
                self.summary.dropped += 1;
                continue;
            }
            if let Some(guest) = self.guests.get_mut(&id) {
                guest.channel.observe(&event);
            }
            allowed.push(event);
        }
        self.received = Some(now);
        self.inject(allowed, now);
        self.received = None;
    }

    fn guest_control(&mut self, id: u64, sender: ControlSender) {
        if let Some(guest) = self.guests.get_mut(&id) {
            guest.control = Some(Box::new(move |data| sender.send(data, false)));
        }
    }

    /// A guest's `gamepad` channel closed, or it left: unplug its controllers.
    pub(super) fn guest_unplug(&mut self, id: u64, now: Instant) {
        let Some(tag) = self.guests.get(&id).map(|g| g.tag) else {
            return;
        };
        let ids = self.pads.controller_ids();
        self.disconnect(
            ids.into_iter()
                .filter(|&c| guest_of(c).is_some_and(|(t, _)| t == tag)),
            now,
        );
    }

    /// A guest's `input` channel closed, or it left: let go of what it holds.
    pub(super) fn guest_release(&mut self, id: u64, now: Instant) {
        let Some(guest) = self.guests.get_mut(&id) else {
            return;
        };
        let events = guest.channel.release_all();
        if !events.is_empty() {
            tracing::info!(
                guest = id,
                count = events.len(),
                "WebRTC input: releasing what a guest held"
            );
            self.inject(events, now);
        }
    }

    pub(super) fn guest_left(&mut self, id: u64, now: Instant) {
        self.guest_unplug(id, now);
        self.guest_release(id, now);
        self.guests.remove(&id);
    }

    /// Some of a guest's keys, buttons or pads' comings and goings were dropped: rather than
    /// guess what it still holds, let go of all of it and unplug its pads (a pad comes back when
    /// the guest plugs it in again or reloads the page). Only a page that floods the host, or a
    /// desktop that stalls for seconds, gets here.
    fn guest_lost(&mut self, id: u64, now: Instant) {
        tracing::debug!(
            guest = id,
            "WebRTC input: a guest's input was dropped; releasing what it holds"
        );
        self.guest_unplug(id, now);
        self.guest_release(id, now);
    }
}
