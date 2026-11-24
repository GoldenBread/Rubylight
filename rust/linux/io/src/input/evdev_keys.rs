//! Windows virtual-key codes as evdev key codes, for a desktop keyboard made on `/dev/uinput`.
//! evdev names physical keys by their place on a US keyboard, and the desktop applies the
//! user's layout on top, which is what Moonlight's normalized virtual keys mean. Pure, so the
//! tables are tested without a device.

/// `KEY_LEFTSHIFT`, `KEY_LEFTCTRL` and `KEY_LEFTALT`, which a client's modifier bits press.
pub(super) const SHIFT: u16 = 42;

/// The evdev key of a virtual key. `keypad` marks the numeric keypad's Enter.
pub(super) fn evdev(vk: u16, keypad: bool) -> Option<u16> {
    /// A to Z on a US keyboard.
    const LETTERS: [u16; 26] = [
        30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17,
        45, 21, 44,
    ];
    Some(match vk {
        0x08 => 14,
        0x09 => 15,
        0x0d if keypad => 96,
        0x0d => 28,
        0x10 | 0xa0 => 42,
        0xa1 => 54,
        0x11 | 0xa2 => 29,
        0xa3 => 97,
        0x12 | 0xa4 => 56,
        0xa5 => 100,
        0x13 => 119,
        0x14 => 58,
        0x1b => 1,
        0x20 => 57,
        0x21 => 104,
        0x22 => 109,
        0x23 => 107,
        0x24 => 102,
        0x25 => 105,
        0x26 => 103,
        0x27 => 106,
        0x28 => 108,
        0x2c => 99,
        0x2d => 110,
        0x2e => 111,
        0x30 => 11,
        0x31..=0x39 => vk - 0x31 + 2,
        0x41..=0x5a => LETTERS[usize::from(vk - 0x41)],
        0x5b => 125,
        0x5c => 126,
        0x5d => 127,
        0x60 => 82,
        0x61..=0x63 => 79 + (vk - 0x61),
        0x64..=0x66 => 75 + (vk - 0x64),
        0x67..=0x69 => 71 + (vk - 0x67),
        0x6a => 55,
        0x6b => 78,
        0x6c => 121,
        0x6d => 74,
        0x6e => 83,
        0x6f => 98,
        0x70..=0x79 => 59 + (vk - 0x70),
        0x7a => 87,
        0x7b => 88,
        0x7c..=0x87 => 183 + (vk - 0x7c),
        0x90 => 69,
        0x91 => 70,
        0xad => 113,
        0xae => 114,
        0xaf => 115,
        0xb0 => 163,
        0xb1 => 165,
        0xb2 => 166,
        0xb3 => 164,
        0xba => 39,
        0xbb => 13,
        0xbc => 51,
        0xbd => 12,
        0xbe => 52,
        0xbf => 53,
        0xc0 => 41,
        0xdb => 26,
        0xdc => 43,
        0xdd => 27,
        0xde => 40,
        0xe2 => 86,
        _ => return None,
    })
}

/// The key and Shift state that type `c` on a US layout; None for what no key types.
pub(super) fn typed(c: char) -> Option<(u16, bool)> {
    let upper = c.is_ascii_uppercase();
    let lower = c.to_ascii_lowercase();
    Some(match c {
        'a'..='z' | 'A'..='Z' => (evdev(lower as u16 - 0x20, false)?, upper),
        '0' => (11, false),
        '1'..='9' => (evdev(c as u16, false)?, false),
        ')' => (11, true),
        '!' | '@' | '#' | '$' | '%' | '^' | '&' | '*' | '(' => {
            let n = b")!@#$%^&*(".iter().position(|&b| b as char == c)? as u16;
            (evdev(0x30 + n, false)?, true)
        }
        ' ' => (57, false),
        '\n' | '\r' => (28, false),
        '\t' => (15, false),
        ';' | ':' => (39, c == ':'),
        '=' | '+' => (13, c == '+'),
        ',' | '<' => (51, c == '<'),
        '-' | '_' => (12, c == '_'),
        '.' | '>' => (52, c == '>'),
        '/' | '?' => (53, c == '?'),
        '`' | '~' => (41, c == '~'),
        '[' | '{' => (26, c == '{'),
        '\\' | '|' => (43, c == '|'),
        ']' | '}' => (27, c == '}'),
        '\'' | '"' => (40, c == '"'),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_digits_and_function_keys() {
        assert_eq!(evdev(0x41, false), Some(30));
        assert_eq!(evdev(0x51, false), Some(16));
        assert_eq!(evdev(0x5a, false), Some(44));
        assert_eq!(evdev(0x31, false), Some(2));
        assert_eq!(evdev(0x39, false), Some(10));
        assert_eq!(evdev(0x30, false), Some(11));
        assert_eq!(evdev(0x70, false), Some(59));
        assert_eq!(evdev(0x79, false), Some(68));
        assert_eq!(evdev(0x7a, false), Some(87));
        assert_eq!(evdev(0x7b, false), Some(88));
        assert_eq!(evdev(0x7c, false), Some(183));
        assert_eq!(evdev(0x87, false), Some(194));
        assert_eq!(evdev(0x88, false), None);
    }

    #[test]
    fn keypad_and_modifiers() {
        assert_eq!(evdev(0x0d, false), Some(28));
        assert_eq!(evdev(0x0d, true), Some(96));
        assert_eq!(evdev(0x60, false), Some(82));
        assert_eq!(evdev(0x61, false), Some(79));
        assert_eq!(evdev(0x65, false), Some(76));
        assert_eq!(evdev(0x69, false), Some(73));
        assert_eq!(evdev(0xa1, false), Some(54));
        assert_eq!(evdev(0xa3, false), Some(97));
    }

    #[test]
    fn typed_characters_on_a_us_layout() {
        assert_eq!(typed('a'), Some((30, false)));
        assert_eq!(typed('A'), Some((30, true)));
        assert_eq!(typed('1'), Some((2, false)));
        assert_eq!(typed('!'), Some((2, true)));
        assert_eq!(typed('@'), Some((3, true)));
        assert_eq!(typed('('), Some((10, true)));
        assert_eq!(typed(')'), Some((11, true)));
        assert_eq!(typed('?'), Some((53, true)));
        assert_eq!(typed('é'), None);
    }
}
