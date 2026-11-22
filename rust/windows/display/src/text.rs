//! UTF-16 conversion helpers for Windows display APIs.

/// The string in a fixed-size UTF-16 buffer, up to its first NUL.
pub(crate) fn from_wide(buffer: &[u16]) -> String {
    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}
