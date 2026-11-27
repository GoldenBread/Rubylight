//! Gamepads from a WebRTC client: the `gamepad-connected`,
//! `gamepad-disconnected` and `gamepad-state` messages mapped to the
//! [`Input`] events the Moonlight path produces, rumble back to the client,
//! and gamepad mouse mode.
//!
//! Everything here is a pure state machine. The caller passes the time of each
//! event and polls [`Gamepads::tick`], so tests need no clock and the host
//! owns the timer.
//!
//! Button order, field names and the mouse-mode constants follow the pinned
//! reference host (Sunshine-Web-RTC `src/webrtc/input_bridge.{h,cpp}`) and the
//! TV app (`tizen/gamepad-input.js`), which is the contract. Where this
//! differs from the reference it says so.
use std::{collections::BTreeMap, time::Instant};

use super::input::MessageError;
use butterpollo_input_policy::input::Input;

mod wire;
#[cfg(test)]
use wire::BUTTON_MAP;
pub use wire::{
    Action, CONTROL_VERSION, Connected, Control, GamepadMessage, KEEPALIVE, MAX_GAMEPADS,
    MOUSE_MODE_HOLD, MOUSE_TICK, Outcome, State, button, map_buttons, map_stick, map_trigger,
    mouse_delta, parse, rumble_magnitude, supported_buttons,
};
use wire::{
    CCAP_ANALOG_TRIGGERS, CCAP_RUMBLE, CCAP_TRIGGER_RUMBLE, CTYPE_XBOX, MOUSE_BUTTON_MASK,
    MOUSE_BUTTONS, WHEEL_NOTCH,
};

/// A snapshot in Moonlight's terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Pad {
    buttons: u32,
    left_trigger: u8,
    right_trigger: u8,
    /// Left X, left Y, right X, right Y; Y up positive.
    sticks: [i16; 4],
}

#[derive(Debug)]
struct Controller {
    /// What the client announced, to tell a repeat of it from a real change.
    announced: Connected,
    slot: u8,
    capabilities: u16,
    last_seq: Option<u64>,
    stale_states: u64,
    sequence_gaps: u64,
    /// The latest accepted snapshot, which mouse-mode ticks read.
    latest: Pad,
    /// The last state sent to the host and the mask it carried.
    last_sent: Option<(u16, Pad)>,
    /// Protocol button mask of the previous accepted snapshot.
    previous_buttons: u32,
    start_pressed_at: Option<Instant>,
    /// Holding Start may turn mouse mode on: not for a guest that may not move the mouse.
    mouse_mode_allowed: bool,
    mouse_mode: bool,
    /// Mouse-mode buttons held when the mode turned on, ignored until released.
    suppressed: u32,
    /// Protocol bits of the mouse buttons this controller holds down.
    simulated: u32,
    /// Strong, weak, left trigger, right trigger.
    rumble: [f64; 4],
}

/// The gamepads of one WebRTC client.
#[derive(Debug, Default)]
pub struct Gamepads {
    controllers: BTreeMap<u32, Controller>,
    slots: [Option<u32>; MAX_GAMEPADS],
    active_mask: u16,
    next_tick: Option<Instant>,
}

impl Gamepads {
    pub fn new() -> Self {
        Self::default()
    }

    /// Moonlight's active gamepad mask.
    pub fn active_mask(&self) -> u16 {
        self.active_mask
    }

    /// The controllers plugged in, by id.
    pub fn controller_ids(&self) -> Vec<u32> {
        self.controllers.keys().copied().collect()
    }

    pub fn slot(&self, controller_id: u32) -> Option<u8> {
        self.controllers.get(&controller_id).map(|c| c.slot)
    }

    pub fn mouse_mode(&self, controller_id: u32) -> bool {
        self.controllers
            .get(&controller_id)
            .is_some_and(|c| c.mouse_mode)
    }

    /// Snapshots dropped as out of order, for the stream summary.
    pub fn stale_states(&self, controller_id: u32) -> u64 {
        self.controllers
            .get(&controller_id)
            .map_or(0, |c| c.stale_states)
    }

    /// Snapshots the client sent that never arrived.
    pub fn sequence_gaps(&self, controller_id: u32) -> u64 {
        self.controllers
            .get(&controller_id)
            .map_or(0, |c| c.sequence_gaps)
    }

    /// The time the next [`tick`](Self::tick) is due, while any controller is
    /// in mouse mode.
    pub fn next_tick(&self) -> Option<Instant> {
        self.next_tick
    }

    /// Parse and apply one gamepad message.
    pub fn handle_message(&mut self, text: &str, now: Instant) -> Result<Outcome, MessageError> {
        Ok(match parse(text)? {
            GamepadMessage::Connected(connected) => self.connect(&connected),
            GamepadMessage::Disconnected { controller_id } => self.disconnect(controller_id),
            GamepadMessage::State(state) => self.state(&state, now),
        })
    }

    /// A controller appears. It gets the lowest free slot and an arrival
    /// event; a controller that reconnects under the same ID with a different
    /// description is replaced. The same announcement again changes nothing: a
    /// client that announces on both channels must not replug the pad.
    pub fn connect(&mut self, connected: &Connected) -> Outcome {
        self.connect_with(connected, true)
    }

    /// [`connect`](Self::connect), for a controller that holding Start must not turn into a
    /// mouse when `mouse_mode` is false.
    pub fn connect_with(&mut self, connected: &Connected, mouse_mode: bool) -> Outcome {
        if self
            .controllers
            .get(&connected.controller_id)
            .is_some_and(|known| known.announced == *connected)
        {
            return Outcome {
                accepted: true,
                actions: Vec::new(),
            };
        }
        let mut actions = Vec::new();
        self.remove(connected.controller_id, &mut actions);
        let Some(slot) = self.slots.iter().position(Option::is_none) else {
            return Outcome {
                accepted: false,
                actions,
            };
        };
        let mut capabilities = 0;
        if connected.buttons > 7 {
            capabilities |= CCAP_ANALOG_TRIGGERS;
        }
        if connected.dual_rumble {
            capabilities |= CCAP_RUMBLE;
        }
        if connected.trigger_rumble {
            capabilities |= CCAP_TRIGGER_RUMBLE;
        }
        self.slots[slot] = Some(connected.controller_id);
        self.active_mask |= 1 << slot;
        self.controllers.insert(
            connected.controller_id,
            Controller {
                announced: connected.clone(),
                slot: slot as u8,
                capabilities,
                last_seq: None,
                stale_states: 0,
                sequence_gaps: 0,
                latest: Pad::default(),
                last_sent: None,
                previous_buttons: 0,
                start_pressed_at: None,
                mouse_mode_allowed: mouse_mode,
                mouse_mode: false,
                suppressed: 0,
                simulated: 0,
                rumble: [0.; 4],
            },
        );
        actions.push(Action::Input(Input::Arrival {
            id: slot as u8,
            kind: CTYPE_XBOX,
            capabilities,
            buttons: supported_buttons(connected.buttons),
        }));
        Outcome {
            accepted: true,
            actions,
        }
    }

    /// A controller goes away: its mouse buttons are released, its rumble
    /// stops, and the host sees a neutral state and then its bit leave the
    /// active mask.
    pub fn disconnect(&mut self, controller_id: u32) -> Outcome {
        let mut actions = Vec::new();
        let accepted = self.remove(controller_id, &mut actions);
        Outcome { accepted, actions }
    }

    /// The stream ended or the channels closed: every controller goes away.
    pub fn close(&mut self) -> Vec<Action> {
        let mut actions = Vec::new();
        let ids: Vec<u32> = self.controllers.keys().copied().collect();
        for id in ids {
            self.remove(id, &mut actions);
        }
        self.active_mask = 0;
        self.next_tick = None;
        actions
    }

    /// Neutralizes held buttons, triggers, sticks, and simulated mouse buttons on every plugged
    /// controller without unplugging slots (used when Quick Suspend freezes the game).
    pub fn release_all(&mut self) -> Vec<Action> {
        let mask = self.active_mask;
        let mut actions = Vec::new();
        for c in self.controllers.values_mut() {
            release_simulated(c, &mut actions);
            c.latest = Pad::default();
            c.previous_buttons = 0;
            c.start_pressed_at = None;
            if !c.mouse_mode {
                send_pad(c, mask, Pad::default(), &mut actions);
            }
        }
        actions
    }

    /// Apply a `gamepad-state` snapshot received at `now`.
    pub fn state(&mut self, state: &State, now: Instant) -> Outcome {
        let mask = self.active_mask;
        let Some(c) = self.controllers.get_mut(&state.controller_id) else {
            return Outcome::default();
        };
        // The channel is unordered and unreliable: an older snapshot must not
        // undo a newer one.
        if c.last_seq.is_some_and(|last| state.seq <= last) {
            c.stale_states += 1;
            return Outcome::default();
        }
        if let Some(last) = c.last_seq {
            c.sequence_gaps += state.seq - last - 1;
        }
        c.last_seq = Some(state.seq);

        let buttons = state.buttons as u32 & button::ALL;
        let pad = Pad {
            buttons: map_buttons(buttons),
            left_trigger: map_trigger(state.left_trigger),
            right_trigger: map_trigger(state.right_trigger),
            sticks: [
                map_stick(state.left_stick_x, false),
                map_stick(state.left_stick_y, true),
                map_stick(state.right_stick_x, false),
                map_stick(state.right_stick_y, true),
            ],
        };
        c.latest = pad;
        let mut actions = Vec::new();

        let start = buttons & button::START != 0;
        let was_start = c.previous_buttons & button::START != 0;
        if start && !was_start {
            c.start_pressed_at = Some(now);
        }
        let mut toggled = false;
        if !start
            && was_start
            && c.mouse_mode_allowed
            && c.start_pressed_at
                .is_some_and(|at| now.saturating_duration_since(at) >= MOUSE_MODE_HOLD)
        {
            toggled = true;
            if c.mouse_mode {
                release_simulated(c, &mut actions);
                c.mouse_mode = false;
                c.suppressed = 0;
            } else {
                // The game sees a neutral pad from now on, not the buttons
                // that happened to be held when the mode turned on.
                send_pad(c, mask, Pad::default(), &mut actions);
                c.mouse_mode = true;
                c.suppressed = buttons & MOUSE_BUTTON_MASK;
                self.next_tick.get_or_insert(now + MOUSE_TICK);
            }
            actions.push(Action::Control(Control::MouseMode {
                controller_id: state.controller_id,
                slot: c.slot,
                active: c.mouse_mode,
            }));
            if !c.mouse_mode && !self.controllers_in_mouse_mode() {
                self.next_tick = None;
            }
        }
        // `c` ended its borrow above; look it up again after touching `self`.
        let c = self
            .controllers
            .get_mut(&state.controller_id)
            .expect("the controller was just found");
        if !start {
            c.start_pressed_at = None;
        }

        if c.mouse_mode {
            let effective = buttons & !c.suppressed;
            let previous = c.previous_buttons & !c.suppressed;
            if !toggled {
                for (bit, mouse_button) in MOUSE_BUTTONS {
                    let pressed = effective & bit != 0;
                    if pressed != (previous & bit != 0) {
                        actions.push(Action::Input(Input::MouseButton {
                            button: mouse_button,
                            down: pressed,
                        }));
                    }
                }
                let rising = |bit: u32| buttons & bit != 0 && c.previous_buttons & bit == 0;
                for (bit, amount, horizontal) in [
                    (button::DPAD_UP, WHEEL_NOTCH, false),
                    (button::DPAD_DOWN, -WHEEL_NOTCH, false),
                    (button::DPAD_RIGHT, WHEEL_NOTCH, true),
                    (button::DPAD_LEFT, -WHEEL_NOTCH, true),
                ] {
                    if rising(bit) {
                        actions.push(Action::Input(Input::Scroll { amount, horizontal }));
                    }
                }
            }
            c.suppressed &= buttons;
            c.simulated = effective & MOUSE_BUTTON_MASK;
        } else {
            send_pad(c, mask, pad, &mut actions);
        }
        c.previous_buttons = buttons;
        Outcome {
            accepted: true,
            actions,
        }
    }

    /// Pointer motion for every controller in mouse mode, once per
    /// [`MOUSE_TICK`]. Call it whenever the clock may have reached
    /// [`next_tick`](Self::next_tick); earlier calls do nothing.
    pub fn tick(&mut self, now: Instant) -> Vec<Action> {
        let Some(due) = self.next_tick else {
            return Vec::new();
        };
        if now < due {
            return Vec::new();
        }
        self.next_tick = Some(now + MOUSE_TICK);
        self.controllers
            .values()
            .filter(|c| c.mouse_mode)
            .filter_map(|c| {
                let (x, y) = mouse_delta(c.latest.sticks);
                (x != 0 || y != 0).then_some(Action::Input(Input::Relative { x, y }))
            })
            .collect()
    }

    /// Host rumble for a Moonlight slot, as the `rumble` message. `low` and
    /// `high` are the host's low- and high-frequency motors, 0..65535. None
    /// when the slot is empty or its controller announced no rumble.
    pub fn rumble(&mut self, slot: u16, low: u16, high: u16) -> Option<Control> {
        let (id, c) = self.by_slot(slot)?;
        if c.capabilities & CCAP_RUMBLE == 0 {
            return None;
        }
        c.rumble[0] = rumble_magnitude(low);
        c.rumble[1] = rumble_magnitude(high);
        Some(rumble_control(id, c))
    }

    /// Host trigger rumble for a Moonlight slot, as the `rumble` message.
    pub fn trigger_rumble(&mut self, slot: u16, left: u16, right: u16) -> Option<Control> {
        let (id, c) = self.by_slot(slot)?;
        if c.capabilities & CCAP_TRIGGER_RUMBLE == 0 {
            return None;
        }
        c.rumble[2] = rumble_magnitude(left);
        c.rumble[3] = rumble_magnitude(right);
        Some(rumble_control(id, c))
    }

    fn by_slot(&mut self, slot: u16) -> Option<(u32, &mut Controller)> {
        let id = (*self.slots.get(usize::from(slot))?)?;
        Some((id, self.controllers.get_mut(&id)?))
    }

    fn controllers_in_mouse_mode(&self) -> bool {
        self.controllers.values().any(|c| c.mouse_mode)
    }

    /// Release everything a controller holds and drop it. False when there
    /// was no such controller.
    fn remove(&mut self, controller_id: u32, actions: &mut Vec<Action>) -> bool {
        let Some(mut c) = self.controllers.remove(&controller_id) else {
            return false;
        };
        release_simulated(&mut c, actions);
        if c.mouse_mode {
            actions.push(Action::Control(Control::MouseMode {
                controller_id,
                slot: c.slot,
                active: false,
            }));
        }
        if c.rumble != [0.; 4] {
            c.rumble = [0.; 4];
            actions.push(Action::Control(rumble_control(controller_id, &c)));
        }
        // Release everything first, then drop the slot from the mask so the
        // host unplugs the pad.
        actions.push(Action::Input(controller_input(
            c.slot,
            self.active_mask,
            Pad::default(),
        )));
        self.active_mask &= !(1 << c.slot);
        actions.push(Action::Input(controller_input(
            c.slot,
            self.active_mask,
            Pad::default(),
        )));
        self.slots[usize::from(c.slot)] = None;
        if !self.controllers_in_mouse_mode() {
            self.next_tick = None;
        }
        true
    }
}

fn rumble_control(controller_id: u32, c: &Controller) -> Control {
    Control::Rumble {
        controller_id,
        slot: c.slot,
        strong: c.rumble[0],
        weak: c.rumble[1],
        left_trigger: c.rumble[2],
        right_trigger: c.rumble[3],
    }
}

fn controller_input(slot: u8, active: u16, pad: Pad) -> Input {
    Input::Controller {
        id: u16::from(slot),
        active,
        buttons: pad.buttons,
        left_trigger: pad.left_trigger,
        right_trigger: pad.right_trigger,
        sticks: pad.sticks,
    }
}

/// Send `pad` unless the host already has exactly this state (a keepalive).
fn send_pad(c: &mut Controller, active: u16, pad: Pad, actions: &mut Vec<Action>) {
    if c.last_sent != Some((active, pad)) {
        c.last_sent = Some((active, pad));
        actions.push(Action::Input(controller_input(c.slot, active, pad)));
    }
}

fn release_simulated(c: &mut Controller, actions: &mut Vec<Action>) {
    for (bit, mouse_button) in MOUSE_BUTTONS {
        if c.simulated & bit != 0 {
            actions.push(Action::Input(Input::MouseButton {
                button: mouse_button,
                down: false,
            }));
        }
    }
    c.simulated = 0;
    c.suppressed = 0;
}

#[cfg(test)]
mod tests;
