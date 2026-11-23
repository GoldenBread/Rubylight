//! Touchpad request mapping onto the VHF driver's touch surface or Steam
//! Controller pads.

use super::*;

// The driver has one touch surface and two contact slots. Keep this mapping
// independent of device IO so unsupported surfaces cannot change slot state.
pub(in crate::input) struct GamepadTouchRequest {
    pub(in crate::input) packet: Vec<u8>,
    pub(super) id: u8,
    pub(super) pointer: u32,
    pub(in crate::input) slot: u8,
    pub(in crate::input) event: u8,
    /// The driver slot and the contact's position in 0..1, which a ViGEmBus
    /// DualShock 4 takes instead of the packet.
    pub(in crate::input) global: u32,
    pub(in crate::input) position: [f32; 2],
}

impl GamepadTouchRequest {
    pub(super) fn submitted(self, pointers: &mut BTreeMap<(u8, u32), u8>) {
        if self.event == 5 {
            pointers.retain(|(pad, _), _| *pad != self.id);
        } else if matches!(self.event, 2 | 4) {
            pointers.remove(&(self.id, self.pointer));
        } else {
            pointers.insert((self.id, self.pointer), self.slot);
        }
    }
}

/// Moonlight's `LI_CCAP_DUAL_TOUCHPAD` controller capability.
pub(super) const CCAP_DUAL_TOUCHPAD: u16 = 0x100;

/// Maps a controller touch onto the driver's single touch surface. With
/// `dual`, touchpad 0 covers the left half of that surface and touchpad 1 the
/// right half, and a finger is told apart by its touchpad as well as its id.
/// The Steam Controller has a pad per contact instead: a client's two
/// touchpads are its two pads, and one touchpad's halves are stretched onto them.
pub(super) fn gamepad_touch_request(
    pointers: &BTreeMap<(u8, u32), u8>,
    profile: u16,
    global: u16,
    input: &Event,
    dual: bool,
) -> Option<GamepadTouchRequest> {
    let Event::ControllerTouch {
        id,
        event,
        touchpad,
        pointer,
        x,
        y,
        pressure,
    } = input
    else {
        return None;
    };
    let steam = profile == VHF_STEAM_CONTROLLER;
    if *touchpad > u8::from(dual) || !(matches!(profile, 5 | 6 | VIGEM_DS4) || steam) {
        return None;
    }
    let event = match event {
        0..=4 => *event,
        6 => 4,
        7 => 5,
        _ => return None,
    };
    // Finger ids are small; the top bit tells the second touchpad's apart.
    let pointer = (*pointer & 0x7fff_ffff) | (u32::from(*touchpad) << 31);
    let key = (*id, pointer);
    let slot = if event == 5 {
        0
    } else if let Some(slot) = pointers.get(&key) {
        *slot
    } else if matches!(event, 0 | 1) {
        if steam {
            // The Steam Controller's contact is its pad: 0 left, 1 right,
            // one finger each; a second finger on a held pad is ignored.
            let pad = steam_pad::touch_pad(*x, *touchpad, dual);
            if pointers
                .iter()
                .any(|((owner, _), used)| owner == id && *used == pad)
            {
                return None;
            }
            pad
        } else {
            (0..2).find(|slot| {
                !pointers
                    .iter()
                    .any(|((pad, _), used)| pad == id && used == slot)
            })?
        }
    } else {
        return None;
    };
    let mut b = request(22, Some(u32::from(global)));
    b.extend_from_slice(&[slot, event]);
    let x = if steam {
        steam_pad::touch_x(*x, slot, dual)
    } else if dual {
        x.clamp(0., 1.) / 2. + f32::from(*touchpad) / 2.
    } else {
        *x
    };
    for f in [x, *y, *pressure] {
        b.extend_from_slice(&((f.clamp(0., 1.) * 65535.) as u16).to_le_bytes());
    }
    b.extend_from_slice(&[0, 0]);
    Some(GamepadTouchRequest {
        packet: b,
        id: *id,
        pointer,
        slot,
        event,
        global: u32::from(global),
        position: [x.clamp(0., 1.), y.clamp(0., 1.)],
    })
}
