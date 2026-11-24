//! Display modes, refresh rates and scaling on Linux. None of them can be read or set yet:
//! every function here returns an error, except a scale request that asks for no change.
use anyhow::Result;

/// A display's current mode: its size in pixels and its refresh in whole hertz. The field
/// names are the ones the host reads.
#[allow(non_snake_case)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DisplayMode {
    pub dmPelsWidth: u32,
    pub dmPelsHeight: u32,
    pub dmDisplayFrequency: u32,
}

/// The mode the display `name` shows now. Not readable on Linux yet.
pub fn mode(_name: &str) -> Result<DisplayMode> {
    Err(crate::unsupported("Reading a display's mode"))
}

/// The highest refresh the display `output` offers, at `resolution` when given. Not readable
/// on Linux yet.
pub fn highest_refresh(
    _output: &str,
    _resolution: Option<(u32, u32)>,
) -> Result<butterpollo_media_policy::framegen::Rate> {
    Err(crate::unsupported("Reading a display's refresh rates"))
}

/// The largest desktop scale, in percent, the display `output` offers at its current mode.
/// Not readable on Linux yet.
pub fn largest_scale(_output: &str) -> Result<u32> {
    Err(crate::unsupported("Reading a display's scale range"))
}

/// The desktop scale, in percent, the display `output` shows now. Not readable on Linux yet.
pub fn output_scale(_output: &str) -> Result<u32> {
    Err(crate::unsupported("Reading a display's scale"))
}

/// Set the desktop scale `configured` asks of the display `output`: a percent, or a negative
/// value for the scale that suits `width` x `height`. Zero asks for no change and succeeds;
/// any other value fails, as Linux cannot set a display's scale yet.
pub fn virtual_scale(_output: &str, configured: i64, _width: u32, _height: u32) -> Result<()> {
    if configured == 0 {
        return Ok(());
    }
    Err(crate::unsupported("Setting a display's scale"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_scale_that_asks_for_nothing_succeeds() {
        assert!(virtual_scale("DP-1", 0, 1920, 1080).is_ok());
        assert!(virtual_scale("DP-1", 150, 1920, 1080).is_err());
        assert!(virtual_scale("DP-1", -1, 1920, 1080).is_err());
        assert!(mode("DP-1").is_err() && output_scale("DP-1").is_err());
        assert!(largest_scale("DP-1").is_err() && highest_refresh("", None).is_err());
    }
}
