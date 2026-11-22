//! Test-harness doubles for streaming session gate and in-memory clipboard access.
use anyhow::{Result, bail};

thread_local! {
    pub static SESSION_ERROR: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}

pub fn ensure_streaming_session() -> Result<()> {
    match SESSION_ERROR.get() {
        Some(message) => bail!(message),
        None => Ok(()),
    }
}

/// The clipboard `/actions/clipboard` reaches in tests: in memory, with the
/// real one's checks and conversions. Only
/// `clipboard_images_travel_as_png_within_the_limit` and Nightjar's interop
/// test put an image there, each holding [`TURN`] while it does.
pub mod clipboard {
    use anyhow::Result;
    use butterpollo_core::clipboard_image::{self, ImageError};
    use std::sync::Mutex;
    pub static PNG: Mutex<Option<Vec<u8>>> = Mutex::new(None);
    /// One test at a time on the clipboard.
    pub static TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    pub fn read() -> Result<String> {
        Ok(String::new())
    }
    pub fn write(_text: &str) -> Result<()> {
        Ok(())
    }
    pub fn read_png(max_bytes: usize) -> Result<Option<Vec<u8>>> {
        match PNG.lock().unwrap().clone() {
            Some(png) if png.len() > max_bytes => Err(ImageError::TooManyBytes {
                bytes: png.len(),
                max: max_bytes,
            }
            .into()),
            png => Ok(png),
        }
    }
    pub fn write_png(png: &[u8]) -> Result<()> {
        let image = clipboard_image::decode_png(png)?;
        *PNG.lock().unwrap() = Some(clipboard_image::encode_png(&image)?);
        Ok(())
    }
}
