//! Windows virtual-key codes as X keysyms, and the arithmetic of absolute moves and scroll
//! clicks. Moonlight sends virtual-key codes; the X server wants keycodes, which come from
//! its keyboard mapping by keysym. Pure, so the tables are tested without a server.

use std::collections::HashMap;

/// The X keysym of a Windows virtual key, for a US layout. `keypad` marks the key as the
/// extended Enter of the numeric keypad.
pub(super) fn keysym(vk: u16, keypad: bool) -> Option<u32> {
    Some(match vk {
        0x08 => 0xff08,
        0x09 => 0xff09,
        0x0c => 0xff0b,
        0x0d if keypad => 0xff8d,
        0x0d => 0xff0d,
        0x10 | 0xa0 => 0xffe1,
        0xa1 => 0xffe2,
        0x11 | 0xa2 => 0xffe3,
        0xa3 => 0xffe4,
        0x12 | 0xa4 => 0xffe9,
        0xa5 => 0xffea,
        0x13 => 0xff13,
        0x14 => 0xffe5,
        0x1b => 0xff1b,
        0x20 => 0x20,
        0x21 => 0xff55,
        0x22 => 0xff56,
        0x23 => 0xff57,
        0x24 => 0xff50,
        0x25 => 0xff51,
        0x26 => 0xff52,
        0x27 => 0xff53,
        0x28 => 0xff54,
        0x29 => 0xff60,
        0x2c => 0xff61,
        0x2d => 0xff63,
        0x2e => 0xffff,
        0x30..=0x39 => u32::from(vk),
        0x41..=0x5a => u32::from(vk) + 0x20,
        0x5b => 0xffeb,
        0x5c => 0xffec,
        0x5d => 0xff67,
        0x60..=0x69 => 0xffb0 + u32::from(vk - 0x60),
        0x6a => 0xffaa,
        0x6b => 0xffab,
        0x6c => 0xffac,
        0x6d => 0xffad,
        0x6e => 0xffae,
        0x6f => 0xffaf,
        0x70..=0x87 => 0xffbe + u32::from(vk - 0x70),
        0x90 => 0xff7f,
        0x91 => 0xff14,
        0xad => 0x1008_ff12,
        0xae => 0x1008_ff11,
        0xaf => 0x1008_ff13,
        0xb0 => 0x1008_ff17,
        0xb1 => 0x1008_ff16,
        0xb2 => 0x1008_ff15,
        0xb3 => 0x1008_ff14,
        0xba => 0x3b,
        0xbb => 0x3d,
        0xbc => 0x2c,
        0xbd => 0x2d,
        0xbe => 0x2e,
        0xbf => 0x2f,
        0xc0 => 0x60,
        0xdb => 0x5b,
        0xdc => 0x5c,
        0xdd => 0x5d,
        0xde => 0x27,
        0xe2 => 0x3c,
        _ => return None,
    })
}

/// The keysym a typed character is, as X names it: Latin-1 by code point, the rest as
/// `0x01000000 + code point`.
pub(super) fn char_keysym(c: char) -> u32 {
    match u32::from(c) {
        0x0a | 0x0d => 0xff0d,
        0x09 => 0xff09,
        cp if (0x20..0x7f).contains(&cp) || (0xa0..0x100).contains(&cp) => cp,
        cp => 0x0100_0000 + cp,
    }
}

/// Where each keysym sits on the server's keyboard: the keycode, and whether it takes Shift.
pub(super) struct KeyTable(HashMap<u32, (u8, bool)>);

impl KeyTable {
    /// Reads the server's mapping: `keysyms` holds `per` columns for each keycode from
    /// `first` on. A symbol found in the first column wins over one found later.
    pub(super) fn new(first: u8, per: usize, keysyms: &[u32]) -> Self {
        let mut table: HashMap<u32, (u8, bool)> = HashMap::new();
        if per == 0 {
            return Self(table);
        }
        for (index, row) in keysyms.chunks(per).enumerate() {
            let Ok(code) = u8::try_from(usize::from(first) + index) else {
                break;
            };
            for (column, &sym) in row.iter().enumerate() {
                if sym == 0 {
                    continue;
                }
                let entry = (code, column % 2 == 1);
                if column < 4 && table.get(&sym).is_none_or(|old| old.1 && !entry.1) {
                    table.insert(sym, entry);
                }
            }
        }
        Self(table)
    }
    /// The keycode of `sym` and whether Shift must be held with it.
    pub(super) fn find(&self, sym: u32) -> Option<(u8, bool)> {
        self.0.get(&sym).copied()
    }
}

/// The X pixel of an absolute client position: `x` of `width` onto `0..size`, with the far
/// edge on the last pixel, as the Windows host does.
pub(super) fn scale_absolute(x: i16, width: i16, size: u32) -> i16 {
    let width = i64::from(width).max(1);
    let span = i64::from(size.max(1) - 1);
    (i64::from(x).clamp(0, width) * span / width) as i16
}

/// Adds scroll `amount` (120ths of a notch) to `rest` and returns the whole notches now due,
/// positive for up or right. The remainder stays in `rest`.
pub(super) fn scroll_notches(rest: &mut i32, amount: i16) -> i32 {
    *rest += i32::from(amount);
    let notches = *rest / 120;
    *rest -= notches * 120;
    notches
}

/// The X button of a Moonlight mouse button: left, middle, right, then the side buttons the
/// X server numbers 8 and 9.
pub(super) fn x_button(button: u8) -> Option<u8> {
    match button {
        1..=3 => Some(button),
        4 => Some(8),
        5 => Some(9),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_digits_and_function_keys() {
        assert_eq!(keysym(0x41, false), Some(0x61));
        assert_eq!(keysym(0x5a, false), Some(0x7a));
        assert_eq!(keysym(0x31, false), Some(0x31));
        assert_eq!(keysym(0x70, false), Some(0xffbe));
        assert_eq!(keysym(0x87, false), Some(0xffd5));
        assert_eq!(keysym(0x88, false), None);
    }

    #[test]
    fn modifiers_keypad_and_punctuation() {
        assert_eq!(keysym(0xa0, false), Some(0xffe1));
        assert_eq!(keysym(0xa1, false), Some(0xffe2));
        assert_eq!(keysym(0xa3, false), Some(0xffe4));
        assert_eq!(keysym(0xa5, false), Some(0xffea));
        assert_eq!(keysym(0x0d, false), Some(0xff0d));
        assert_eq!(keysym(0x0d, true), Some(0xff8d));
        assert_eq!(keysym(0x65, false), Some(0xffb5));
        assert_eq!(keysym(0xba, false), Some(u32::from(b';')));
        assert_eq!(keysym(0xde, false), Some(u32::from(b'\'')));
    }

    #[test]
    fn typed_characters() {
        assert_eq!(char_keysym('a'), 0x61);
        assert_eq!(char_keysym('é'), 0xe9);
        assert_eq!(char_keysym('€'), 0x0100_20ac);
        assert_eq!(char_keysym('\n'), 0xff0d);
    }

    #[test]
    fn table_prefers_unshifted_symbols() {
        // keycode 10: 'a' 'A'; keycode 11: 'A' alone in the shifted column.
        let table = KeyTable::new(10, 2, &[0x61, 0x41, 0, 0x41]);
        assert_eq!(table.find(0x61), Some((10, false)));
        assert_eq!(table.find(0x41), Some((10, true)));
        assert_eq!(table.find(0x62), None);
    }

    #[test]
    fn absolute_moves_reach_the_last_pixel() {
        assert_eq!(scale_absolute(0, 1920, 1280), 0);
        assert_eq!(scale_absolute(960, 1920, 1281), 640);
        assert_eq!(scale_absolute(1920, 1920, 1280), 1279);
        assert_eq!(scale_absolute(-5, 1920, 1280), 0);
        assert_eq!(scale_absolute(4000, 1920, 1280), 1279);
        assert_eq!(scale_absolute(10, 0, 100), 99);
    }

    #[test]
    fn scroll_accumulates_in_120ths() {
        let mut rest = 0;
        assert_eq!(scroll_notches(&mut rest, 60), 0);
        assert_eq!(scroll_notches(&mut rest, 60), 1);
        assert_eq!(scroll_notches(&mut rest, -240), -2);
        assert_eq!(scroll_notches(&mut rest, -30), 0);
        assert_eq!(scroll_notches(&mut rest, -90), -1);
        assert_eq!(rest, 0);
    }

    #[test]
    fn buttons() {
        assert_eq!(x_button(1), Some(1));
        assert_eq!(x_button(3), Some(3));
        assert_eq!(x_button(4), Some(8));
        assert_eq!(x_button(5), Some(9));
        assert_eq!(x_button(6), None);
    }
}
