//! The target `kwin-output:<output name>`: one KWin output, captured without the desktop
//! portal's dialog. [`screencast`] asks KWin for the stream, [`permission`] makes KWin willing,
//! and [`open`] hands the stream's PipeWire node to the PipeWire capture as a desktop
//! compositor's node.

pub mod permission;
pub mod screencast;

use super::pw_capture::{Options, PipeWireCapture};
use crate::vaapi::Va;
use anyhow::{Context, Result};
use butterpollo_media_policy::capture_policy::Picture;
use std::sync::Arc;

pub use screencast::KwinStream;

/// The target's prefix; the output's name follows.
pub const TARGET: &str = "kwin-output:";

/// The output a target names: `None` for any other target.
pub fn parse(target: &str) -> Option<&str> {
    target.strip_prefix(TARGET)
}

/// Opens the output's stream and captures its node. DMA-BUFs go first; when they cannot be
/// imported the frames come through memory, which is logged. The stream lives as long as the
/// capture does.
pub fn open(output: &str, va: &Arc<Va>, picture: Picture) -> Result<(PipeWireCapture, KwinStream)> {
    let (stream, node) = KwinStream::open(output, true)?;
    tracing::info!(node, output, "desktop capture of a KWin output");
    let desktop = Options {
        desktop: true,
        shm: false,
        picture,
    };
    let capture = match PipeWireCapture::open_with(node, None, va, desktop) {
        Ok(capture) => capture,
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), "DMA-BUF capture of the KWin output failed; trying buffers in memory with a CPU upload");
            let options = Options {
                shm: true,
                ..desktop
            };
            PipeWireCapture::open_with(node, None, va, options)
                .with_context(|| format!("no DMA-BUF either: {error:#}"))?
        }
    };
    Ok((capture, stream))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_target_names_an_output() {
        assert_eq!(super::parse("kwin-output:Virtual-1"), Some("Virtual-1"));
        assert_eq!(super::parse("desktop"), None);
    }
}
