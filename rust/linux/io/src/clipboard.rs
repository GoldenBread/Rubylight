//! The clipboard on Linux: there is no clipboard backend yet. Its change count never moves,
//! so nothing is synchronised; reading text fails, no image is found, and writes fail.

use anyhow::Result;

/// The clipboard's change count, which every new content raises. It stays 0 on Linux.
pub fn sequence() -> u32 {
    0
}

/// The clipboard's text. Fails on Linux.
pub fn read() -> Result<String> {
    Err(crate::unsupported("The clipboard"))
}

/// Put `text` on the clipboard. Fails on Linux.
pub fn write(_text: &str) -> Result<()> {
    Err(crate::unsupported("The clipboard"))
}

/// The clipboard's image as PNG of at most `max_bytes`, or `None` while it holds none. It
/// never holds one on Linux.
pub fn read_png(_max_bytes: usize) -> Result<Option<Vec<u8>>> {
    Ok(None)
}

/// Put the PNG `png` on the clipboard. Fails on Linux.
pub fn write_png(_png: &[u8]) -> Result<()> {
    Err(crate::unsupported("The image clipboard"))
}
