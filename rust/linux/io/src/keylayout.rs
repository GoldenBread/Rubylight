//! Keyboard layouts by Windows layout id (KLID): the scan code of each virtual key. The fixed
//! US table and the per-peer cache are the same as on Windows. Reading a layout is not
//! available on Linux yet: the tables come from Windows' layout files, which a Linux PC does
//! not have, so every layout id fails to load and keys follow the host's own layout.

/// The scan code of each virtual key on a US keyboard, for keys that arrive without one.
pub const SCANCODES: [u8; 256] = [
    0, 0, 0, 70, 0, 0, 0, 0, 14, 15, 0, 0, 76, 28, 0, 0, 42, 29, 56, 0, 58, 0, 0, 0, 0, 0, 0, 1, 0,
    0, 0, 0, 57, 73, 81, 79, 71, 75, 72, 77, 80, 0, 0, 0, 84, 82, 83, 99, 11, 2, 3, 4, 5, 6, 7, 8,
    9, 10, 0, 0, 0, 0, 0, 0, 0, 30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16,
    19, 31, 20, 22, 47, 17, 45, 21, 44, 91, 92, 93, 0, 95, 82, 79, 80, 81, 75, 76, 77, 71, 72, 73,
    55, 78, 0, 74, 83, 53, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 87, 88, 100, 101, 102, 103, 104,
    105, 106, 107, 108, 109, 110, 118, 0, 0, 0, 0, 0, 0, 0, 0, 69, 70, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 42, 54, 29, 29, 56, 56, 106, 105, 103, 104, 101, 102, 50, 32, 46, 48, 25, 16,
    36, 34, 108, 109, 107, 33, 0, 0, 39, 13, 51, 12, 52, 53, 41, 115, 126, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 26, 43, 27, 40, 0, 0, 0, 86, 0, 0, 0, 0, 0, 0,
    113, 92, 123, 0, 111, 90, 0, 0, 91, 0, 95, 0, 94, 0, 0, 0, 93, 0, 98, 0, 0, 0, 0,
];

/// The scan code of each virtual key on one keyboard layout.
#[derive(Clone, PartialEq, Eq)]
pub struct Layout {
    klid: u32,
    scancodes: [u8; 256],
}

impl std::fmt::Debug for Layout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Layout({:08X})", self.klid)
    }
}

impl Layout {
    /// The layout with this id (KLID). Always fails on Linux, which has no layout files to
    /// read the table from.
    pub fn load(klid: u32) -> anyhow::Result<Self> {
        Err(anyhow::anyhow!(
            "keyboard layout {klid:08X} is unknown to this PC ({:#})",
            crate::unsupported("Reading a keyboard layout")
        ))
    }

    pub fn klid(&self) -> u32 {
        self.klid
    }

    /// The scan code of a virtual key, or None if the layout has no such key outside the
    /// E0 and E1 ones (AltGr, the right Ctrl, the keys between the main block and the
    /// numpad), which every layout puts in the same place.
    pub fn scancode(&self, vk: u16) -> Option<u8> {
        self.scancodes
            .get(usize::from(vk))
            .copied()
            .filter(|&scan| scan != 0)
    }
}

/// The most layout ids one [`Layouts`] reads.
pub const MAX_LAYOUTS: usize = 16;

/// The layouts read for one peer by id, read or failed: each id's file is loaded at most
/// once however often a device reports it, and at most [`MAX_LAYOUTS`] ids are.
pub struct Layouts {
    load: fn(u32) -> anyhow::Result<Layout>,
    read: std::collections::HashMap<u32, Result<Layout, String>>,
    refused: bool,
}

impl Default for Layouts {
    fn default() -> Self {
        Self::with_loader(Layout::load)
    }
}

impl Layouts {
    /// Reads layouts with `load` instead of [`Layout::load`], for tests.
    pub fn with_loader(load: fn(u32) -> anyhow::Result<Layout>) -> Self {
        Self {
            load,
            read: Default::default(),
            refused: false,
        }
    }

    /// The layout with this id, or why it cannot be read, and whether that is new: the
    /// first time this id was asked for, or the first refusal past [`MAX_LAYOUTS`].
    pub fn get(&mut self, klid: u32) -> (Result<Layout, String>, bool) {
        if let Some(read) = self.read.get(&klid) {
            return (read.clone(), false);
        }
        if self.read.len() >= MAX_LAYOUTS {
            let first = !std::mem::replace(&mut self.refused, true);
            return (
                Err(format!(
                    "keyboard layout {klid:08X} is one more than the {MAX_LAYOUTS} this stream reads"
                )),
                first,
            );
        }
        let read = (self.load)(klid).map_err(|error| format!("{error:#}"));
        self.read.insert(klid, read.clone());
        (read, true)
    }
}
