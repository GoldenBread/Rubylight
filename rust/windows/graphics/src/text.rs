//! UTF-16 strings for Win32 calls.

/// `text` as a NUL-terminated UTF-16 buffer for a `PCWSTR` argument.
pub(crate) fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// A NUL-terminated UTF-16 string literal, so a fixed name needs neither a
/// conversion nor a lookup at run time.
macro_rules! wide {
    ($name:literal) => {{
        const UNITS: [u16; $name.len() + 1] = widen($name.as_bytes());
        &UNITS
    }};
}

/// `text`, ASCII, as `N` UTF-16 units with a trailing NUL: the array a
/// [`wide!`] literal holds. The last unit stays 0.
const fn widen<const N: usize>(text: &[u8]) -> [u16; N] {
    let mut units = [0u16; N];
    let mut at = 0;
    while at < N - 1 {
        assert!(text[at] < 0x80, "wide! takes ASCII names");
        units[at] = text[at] as u16;
        at += 1;
    }
    units
}

/// Every fixed name [`static_wide`] is asked for, as one list so the
/// matcher and the test table cannot drift apart.
macro_rules! amf_names {
    ($($name:literal),* $(,)?) => {
        /// The compile-time wide form of a known name.
        fn known(name: &str) -> Option<&'static [u16]> {
            match name {
                $($name => Some(wide!($name)),)*
                _ => None,
            }
        }
        /// The names the match above answers; the test walks them all.
        #[cfg(test)]
        const KNOWN: &[&str] = &[$($name),*];
    };
}
amf_names! {
    "AMFDisplayCapture", "AMFVideoConverter", "AccelerationType", "Av1BlockQIndexFeedback",
    "Av1BlockQIndexMap", "Av1ForceFrameType", "Av1ForceInsertSequenceHeader", "Av1ForceLTRReferenceBitfield",
    "Av1LTRMode", "Av1MarkCurrentWithLTRIndex", "Av1MaxNumLTRFrames", "Av1OutputFrameType",
    "Av1ROIData", "AV1OutputBufferType", "BlockQpFeedback", "BlockQpMap", "CaptureMode",
    "ColorProfile", "DuplicateOutput", "EmbedCursor", "Fill", "FillColor", "FlipTimesamp",
    "ForceLTRReferenceBitfield", "ForcePictureType", "FrameIndex", "FrameRate", "HevcBlockQpFeedback",
    "HevcBlockQpMap", "HevcForceLTRReferenceBitfield", "HevcForcePictureType", "HevcInsertHeader",
    "HevcLTRMode", "HevcMarkCurrentWithLTRIndex", "HevcMaxOfLTRFrames", "HevcOutputBufferType",
    "HevcOutputDataType", "HevcROIData", "HevcROIMap", "InputColorMatrixCoeff", "InputColorPrimaries",
    "InputColorRange", "InputHdrMetadata", "InputTonemapping", "InputTransferChar", "InsertPPS",
    "InsertSPS", "LTRMode", "MarkCurrentWithLTRIndex", "MaxOfLTRFrames", "MemoryType", "MonitorIndex",
    "OutputBufferType", "OutputColorMatrixCoeff", "OutputColorPrimaries", "OutputColorRange",
    "OutputDataType", "OutputFormat", "OutputHdrMetadata", "OutputRect", "OutputSize",
    "OutputTransferChar", "ROIMap", "ROIData", "ScaleType", "UseDecoderHDRMetadata", "VPEAvailability",
}

/// [`to_wide`] of a fixed name, such as an AMF property set on every frame:
/// the known names were converted where this is written, and the rest once,
/// kept for the life of the process. Pass only names from a fixed set,
/// never text built at run time.
pub(crate) fn static_wide(name: &'static str) -> &'static [u16] {
    if let Some(wide) = known(name) {
        return wide;
    }
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static NAMES: OnceLock<Mutex<HashMap<&'static str, &'static [u16]>>> = OnceLock::new();
    let mut names = NAMES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    names
        .entry(name)
        .or_insert_with(|| Vec::leak(to_wide(name)))
}

/// The string in a fixed-size UTF-16 buffer, up to its first NUL.
/// Unpaired surrogates become U+FFFD.
pub(crate) fn from_wide(buffer: &[u16]) -> String {
    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_a_fixed_buffer() {
        let mut buffer = [0u16; 16];
        let wide = to_wide("Radeon");
        assert_eq!(wide.last(), Some(&0));
        buffer[..wide.len()].copy_from_slice(&wide);
        assert_eq!(from_wide(&buffer), "Radeon");
        assert_eq!(from_wide(&wide[..3]), "Rad");
    }

    #[test]
    fn a_fixed_name_is_converted_once() {
        let first = static_wide("HevcOutputDataType");
        assert_eq!(first, &to_wide("HevcOutputDataType")[..]);
        assert!(std::ptr::eq(first, static_wide("HevcOutputDataType")));
        assert_eq!(
            static_wide("OutputDataType"),
            &to_wide("OutputDataType")[..]
        );
    }

    #[test]
    fn every_known_name_is_the_same_nul_terminated_utf16_as_the_conversion() {
        for name in KNOWN {
            let wide = static_wide(name);
            assert_eq!(wide.last(), Some(&0), "{name} lost its NUL");
            assert_eq!(wide, &to_wide(name)[..], "{name}");
        }
    }

    #[test]
    fn a_name_the_table_does_not_know_keeps_the_kept_conversion() {
        let name = "UnlistedName";
        let first = static_wide(name);
        assert_eq!(first, &to_wide(name)[..]);
        assert!(std::ptr::eq(first, static_wide(name)));
    }
}
