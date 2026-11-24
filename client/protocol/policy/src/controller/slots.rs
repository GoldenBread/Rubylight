//! Controller slot reservation, active gamepad bitmask calculation, 3-pass launch
//! preparation classification, phantom-pad suppression, and multi-context input
//! fusion matching `ControllerHandler`.

use crate::controller::MAX_GAMEPADS;
use crate::controller::arrival::apply_steam_controls_input;
use crate::controller::deadzone::{max_by_magnitude_i16, max_by_magnitude_u8};

/// Packed slot assignment decision returned to Java.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotAssignment {
    /// Assign fixed non-reserved slot (e.g., slot 0 or joined built-in pad slot).
    Fixed(u8),
    /// Reserved a unique slot in `current_controllers`.
    Reserved(u8),
    /// Delegate to Java `InputDevice` `id ± 1` associated-joystick lookup.
    LookupAssociated,
    /// All 16 slots are occupied.
    Full,
}

/// Outcome of releasing a controller context's slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReleaseOutcome {
    pub active_mask: u16,
    pub should_send_neutral: bool,
    pub cleared_slot_input: bool,
}

/// One context's mapped controller report queued for multi-context slot fusion.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContextInput {
    pub buttons: u32,
    pub left_trigger: u8,
    pub right_trigger: u8,
    pub left_stick_x: i16,
    pub left_stick_y: i16,
    pub right_stick_x: i16,
    pub right_stick_y: i16,
}

/// Session-scoped controller slot allocator and input-seen tracker.
#[derive(Clone, Debug)]
pub struct SlotAllocator {
    pub current_controllers: u16,
    pub initial_controllers: u16,
    pub slots_with_input: [bool; MAX_GAMEPADS],
    pub onscreen_own_player_session: bool,
    pub touchpad_button_session: bool,
    pub steam_controls_session: bool,
}

impl SlotAllocator {
    pub fn new(
        initial_controllers: u16,
        onscreen_own_player_session: bool,
        touchpad_button_session: bool,
        steam_controls_session: bool,
    ) -> Self {
        Self {
            current_controllers: if onscreen_own_player_session { 1 } else { 0 },
            initial_controllers,
            slots_with_input: [false; MAX_GAMEPADS],
            onscreen_own_player_session,
            touchpad_button_session,
            steam_controls_session,
        }
    }

    pub fn reserve_next_slot(&mut self, min_slot: usize) -> Option<u8> {
        for i in min_slot..MAX_GAMEPADS {
            let bit = 1u16 << i;
            if (self.current_controllers & bit) == 0 {
                self.current_controllers |= bit;
                self.initial_controllers &= !bit;
                return Some(i as u8);
            }
        }
        None
    }

    /// Computes the 16-bit active controller mask sent to the host (`getActiveControllerMask`).
    pub fn active_controller_mask(
        &self,
        send_controllers: bool,
        multi_controller: bool,
        onscreen_controller: bool,
        layout_gamepad_used: bool,
        has_built_in_player_one: bool,
    ) -> u16 {
        if !send_controllers {
            return if onscreen_controller || layout_gamepad_used || self.onscreen_own_player_session
            {
                1
            } else {
                0
            };
        }
        if multi_controller {
            let p1 = if onscreen_controller || layout_gamepad_used || has_built_in_player_one {
                1
            } else {
                0
            };
            self.current_controllers | self.initial_controllers | p1
        } else {
            1
        }
    }

    /// Assigns a slot for an Android `InputDeviceContext`.
    /// `built_in_pad_slot`: `Some(slot)` if `onscreen_own_player_session && !external && !has_joystick_axes`
    /// found an already-assigned built-in pad.
    #[allow(clippy::too_many_arguments)]
    pub fn assign_input_device_slot(
        &mut self,
        external: bool,
        has_joystick_axes: bool,
        has_gamepad_buttons: bool,
        has_adjacent_associated_joystick: bool,
        multi_controller: bool,
        built_in_pad_slot: Option<u8>,
    ) -> SlotAssignment {
        if !external && !self.onscreen_own_player_session {
            return SlotAssignment::Fixed(0);
        }
        if let Some(slot) = built_in_pad_slot {
            return SlotAssignment::Fixed(slot);
        }
        if multi_controller
            && (has_joystick_axes || (has_gamepad_buttons && !has_adjacent_associated_joystick))
        {
            return match self.reserve_next_slot(0) {
                Some(slot) => SlotAssignment::Reserved(slot),
                None => SlotAssignment::Full,
            };
        }
        if !has_joystick_axes {
            return SlotAssignment::LookupAssociated;
        }
        SlotAssignment::Fixed(0)
    }

    /// Fallback after `LookupAssociated` fails to find an adjacent joystick.
    pub fn resolve_associated_fallback(&mut self) -> SlotAssignment {
        if self.onscreen_own_player_session {
            match self.reserve_next_slot(1) {
                Some(slot) => SlotAssignment::Reserved(slot),
                None => SlotAssignment::Full,
            }
        } else {
            SlotAssignment::Fixed(0)
        }
    }

    /// Assigns a slot for a `UsbDeviceContext`.
    pub fn assign_usb_device_slot(&mut self, multi_controller: bool) -> SlotAssignment {
        if multi_controller {
            match self.reserve_next_slot(0) {
                Some(slot) => SlotAssignment::Reserved(slot),
                None => SlotAssignment::Full,
            }
        } else {
            SlotAssignment::Fixed(0)
        }
    }

    /// Releases a controller slot (`releaseControllerNumber`).
    #[allow(clippy::too_many_arguments)]
    pub fn release_slot(
        &mut self,
        slot: i16,
        was_reserved: bool,
        assigned: bool,
        send_controllers: bool,
        multi_controller: bool,
        onscreen_controller: bool,
        layout_gamepad_used: bool,
        has_built_in_player_one: bool,
        pending_usb_arrival: bool,
    ) -> ReleaseOutcome {
        if was_reserved && (0..MAX_GAMEPADS as i16).contains(&slot) {
            self.current_controllers &= !(1u16 << slot);
        }
        let active_mask = self.active_controller_mask(
            send_controllers,
            multi_controller,
            onscreen_controller,
            layout_gamepad_used,
            has_built_in_player_one,
        );
        let valid = (0..MAX_GAMEPADS as i16).contains(&slot);
        let had_input = valid && self.slots_with_input[slot as usize];
        let slot_inactive = valid && (active_mask & (1u16 << slot)) == 0;
        let should_send_neutral =
            send_controllers && assigned && (had_input || !pending_usb_arrival || slot_inactive);
        let cleared_slot_input = assigned && slot_inactive;
        if cleared_slot_input {
            self.slots_with_input[slot as usize] = false;
        }
        ReleaseOutcome {
            active_mask,
            should_send_neutral,
            cleared_slot_input,
        }
    }

    pub fn mark_slot_has_input(&mut self, slot: i16) {
        if (0..MAX_GAMEPADS as i16).contains(&slot) {
            self.slots_with_input[slot as usize] = true;
        }
    }

    pub fn slot_has_input(&self, slot: i16) -> bool {
        (0..MAX_GAMEPADS as i16).contains(&slot) && self.slots_with_input[slot as usize]
    }
}

/// Whether the on-screen pad announces itself at launch/reconnect (`announcesOnscreenPad`).
pub fn announces_onscreen_pad(
    onscreen_own_player_session: bool,
    touchpad_button_session: bool,
    steam_controls_session: bool,
    send_controllers: bool,
    onscreen_controller: bool,
    layout_gamepad_used: bool,
    has_slot_zero_device: bool,
) -> bool {
    if onscreen_own_player_session {
        return true;
    }
    if !touchpad_button_session && !steam_controls_session {
        return false;
    }
    if !send_controllers {
        return onscreen_controller || layout_gamepad_used;
    }
    !has_slot_zero_device
}

/// Launch preparation pass (`0`: external with sticks, `1`: external buttons-only, `2`: built-in).
pub fn launch_pass_for_input_device(external: bool, has_joystick_axes: bool) -> u8 {
    if external && has_joystick_axes {
        0
    } else if external {
        1
    } else {
        2
    }
}

/// Fuses all active contexts sharing one controller slot (`sendControllerInputPacket`).
pub fn fuse_contexts(
    contexts: &[ContextInput],
    steam_controls_session: bool,
    controller_number: i16,
    steam_pad_touch_x: f32,
    mouse_emulation_active: bool,
) -> ContextInput {
    let mut fused = ContextInput::default();
    for ctx in contexts {
        fused.buttons |= ctx.buttons;
        fused.left_trigger = max_by_magnitude_u8(fused.left_trigger, ctx.left_trigger);
        fused.right_trigger = max_by_magnitude_u8(fused.right_trigger, ctx.right_trigger);
        fused.left_stick_x = max_by_magnitude_i16(fused.left_stick_x, ctx.left_stick_x);
        fused.left_stick_y = max_by_magnitude_i16(fused.left_stick_y, ctx.left_stick_y);
        fused.right_stick_x = max_by_magnitude_i16(fused.right_stick_x, ctx.right_stick_x);
        fused.right_stick_y = max_by_magnitude_i16(fused.right_stick_y, ctx.right_stick_y);
    }
    if !mouse_emulation_active {
        fused.buttons = apply_steam_controls_input(
            steam_controls_session,
            controller_number,
            fused.buttons,
            fused.left_trigger,
            fused.right_trigger,
            steam_pad_touch_x,
        );
    }
    fused
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamepad_constants::button;
    use rubylight_protocol::steam_controller as sc;

    #[test]
    fn onscreen_own_player_reserves_slot_zero_and_pushes_pads_to_one() {
        let mut alloc = SlotAllocator::new(0b11, true, false, false);
        assert_eq!(alloc.current_controllers, 1);
        assert_eq!(
            alloc.assign_input_device_slot(false, true, true, false, true, None),
            SlotAssignment::Reserved(1)
        );
        assert_eq!(
            alloc.assign_usb_device_slot(true),
            SlotAssignment::Reserved(2)
        );
        // Built-in button-only device without a built-in pad joins slot >= 1 via fallback.
        assert_eq!(
            alloc.assign_input_device_slot(false, false, false, false, true, None),
            SlotAssignment::LookupAssociated
        );
        assert_eq!(
            alloc.resolve_associated_fallback(),
            SlotAssignment::Reserved(3)
        );
        assert_eq!(alloc.current_controllers, 0b1111);
    }

    #[test]
    fn controllers_off_exposes_only_slot_zero_when_onscreen_active() {
        let mut alloc = SlotAllocator::new(0b10, false, false, false);
        assert_eq!(
            alloc.assign_usb_device_slot(true),
            SlotAssignment::Reserved(0)
        );
        assert_eq!(
            alloc.active_controller_mask(false, true, false, false, false),
            0
        );
        assert_eq!(
            alloc.active_controller_mask(false, true, true, false, false),
            1
        );
        assert_eq!(
            alloc.active_controller_mask(true, true, false, false, false),
            0b11
        );
        assert_eq!(
            alloc.active_controller_mask(true, false, false, false, false),
            1
        );
    }

    #[test]
    fn release_suppresses_neutral_when_pending_usb_arrival_on_active_slot() {
        let mut alloc = SlotAllocator::new(0, false, false, false);
        assert_eq!(
            alloc.assign_usb_device_slot(true),
            SlotAssignment::Reserved(0)
        );
        let rel_pending = alloc.release_slot(0, true, true, true, false, false, false, false, true);
        // multi_controller=false keeps bit 0 active in active_mask -> !pending_usb_arrival is false
        assert!(!rel_pending.should_send_neutral);

        alloc.mark_slot_has_input(0);
        let rel_seen = alloc.release_slot(0, true, true, true, true, false, false, false, true);
        assert!(rel_seen.should_send_neutral);
        assert!(rel_seen.cleared_slot_input);
        assert!(!alloc.slot_has_input(0));
    }

    #[test]
    fn fuse_contexts_merges_magnitudes_and_steam_controls() {
        let a = ContextInput {
            buttons: button::A,
            left_trigger: 0xF0,
            right_trigger: 10,
            left_stick_x: -32768,
            left_stick_y: 100,
            right_stick_x: 0,
            right_stick_y: 0,
        };
        let b = ContextInput {
            buttons: button::TOUCHPAD | sc::LEFT_GRIP_TOUCH_FLAG,
            left_trigger: 20,
            right_trigger: 0xEF,
            left_stick_x: 32767,
            left_stick_y: -200,
            right_stick_x: 500,
            right_stick_y: -500,
        };
        let fused = fuse_contexts(&[a, b], true, 0, 0.25, false);
        assert_eq!(
            fused.buttons,
            button::A
                | button::TOUCHPAD
                | sc::LEFT_GRIP_TOUCH_FLAG
                | sc::LEFT_TRIGGER_CLICK_FLAG
                | sc::LEFT_PAD_CLICK_FLAG
        );
        assert_eq!(fused.left_trigger, 0xF0);
        assert_eq!(fused.right_trigger, 0xEF);
        assert_eq!(fused.left_stick_x, -32768);
        assert_eq!(fused.left_stick_y, -200);
        assert_eq!(fused.right_stick_x, 500);
        assert_eq!(fused.right_stick_y, -500);
    }

    #[test]
    fn launch_pass_orders_external_sticks_before_buttons_before_builtin() {
        assert_eq!(launch_pass_for_input_device(true, true), 0);
        assert_eq!(launch_pass_for_input_device(true, false), 1);
        assert_eq!(launch_pass_for_input_device(false, true), 2);
        assert_eq!(launch_pass_for_input_device(false, false), 2);
    }
}
