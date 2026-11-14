//! The refresh range of the window's display on macOS. `NSScreen` reports, since macOS 12, the
//! shortest and the longest interval between two refreshes; on a fixed-refresh screen the two are
//! equal, on ProMotion and Adaptive-Sync displays they differ. SDL's display mode can report 0 Hz
//! for ProMotion, so it is no measure of this.
//!
//! Ported from Nonary's moonlight-qt (GPL-3.0, commit 19758e5e6), app/streaming/
//! macdisplaytiming.{h,mm}: what is read from the screen and the tests for a valid and for a
//! variable range.
//!
//! And the display's own resolution, which SDL's mode cannot give: SDL reports a mode in points
//! with its backing scale, and a scaled mode's backing store is larger than the panel ("looks like
//! 2560x1440" draws 5120x2880 for a 3840x2160 panel and scales it down). It is found as moonlight-qt
//! finds it (moonlight-stream/moonlight-qt, GPL-3.0, Cameron Gutman and contributors, commit
//! 67dbfddb2, app/streaming/streamutils.cpp `StreamUtils::getNativeDesktopMode`): CoreGraphics'
//! modes of the display, without the duplicate low-resolution ones, and the one IOKit flags native.

/// IOKit's `kDisplayModeNativeFlag` (IOGraphicsTypes.h): the mode at the display's own resolution.
pub const NATIVE_FLAG: u32 = 0x0200_0000;

/// A mode CoreGraphics lists for a display: its size in pixels and its IOKit flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mode {
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub io_flags: u32,
}

/// The display's own resolution in pixels: the largest mode flagged native. None when no mode is,
/// and the caller keeps SDL's size.
pub fn native_pixels(modes: &[Mode]) -> Option<(u32, u32)> {
    modes
        .iter()
        .filter(|mode| mode.io_flags & NATIVE_FLAG != 0 && mode.pixel_width > 0 && mode.pixel_height > 0)
        .map(|mode| (mode.pixel_width, mode.pixel_height))
        .max_by_key(|&(width, height)| u64::from(width) * u64::from(height))
}

#[cfg(target_os = "macos")]
mod cg {
    use std::ffi::c_void;

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct CGPoint {
        pub x: f64,
        pub y: f64,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        pub fn CGGetDisplaysWithPoint(point: CGPoint, max_displays: u32, displays: *mut u32, matching: *mut u32) -> i32;
        pub fn CGDisplayCopyAllDisplayModes(display: u32, options: *const c_void) -> *const c_void;
        pub fn CGDisplayModeGetIOFlags(mode: *const c_void) -> u32;
        pub fn CGDisplayModeGetPixelWidth(mode: *const c_void) -> usize;
        pub fn CGDisplayModeGetPixelHeight(mode: *const c_void) -> usize;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        pub fn CFArrayGetCount(array: *const c_void) -> isize;
        pub fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
        pub fn CFRelease(object: *const c_void);
    }
}

/// The modes CoreGraphics lists for the display holding the global point `(x, y)`, in points as
/// SDL's display bounds give them on macOS (`CGDisplayBounds`). Empty when no display is there.
#[cfg(target_os = "macos")]
pub fn modes_at(x: f64, y: f64) -> Vec<Mode> {
    let (mut display, mut count) = (0u32, 0u32);
    // SAFETY: CoreGraphics queries with valid out-pointers; the mode list is copied for this call,
    // its modes are read while it lives, and it is released once.
    unsafe {
        if cg::CGGetDisplaysWithPoint(cg::CGPoint { x, y }, 1, &mut display, &mut count) != 0 || count == 0 {
            return Vec::new();
        }
        let list = cg::CGDisplayCopyAllDisplayModes(display, std::ptr::null());
        if list.is_null() {
            return Vec::new();
        }
        let modes = (0..cg::CFArrayGetCount(list).max(0))
            .map(|index| cg::CFArrayGetValueAtIndex(list, index))
            .filter(|mode| !mode.is_null())
            .map(|mode| Mode {
                pixel_width: u32::try_from(cg::CGDisplayModeGetPixelWidth(mode)).unwrap_or(0),
                pixel_height: u32::try_from(cg::CGDisplayModeGetPixelHeight(mode)).unwrap_or(0),
                io_flags: cg::CGDisplayModeGetIOFlags(mode),
            })
            .collect();
        cg::CFRelease(list);
        modes
    }
}

/// What `NSScreen` says about a display's refreshes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DisplayTiming {
    pub maximum_frames_per_second: i64,
    /// The shortest interval between two refreshes (s).
    pub minimum_refresh_interval: f64,
    /// The longest interval between two refreshes (s).
    pub maximum_refresh_interval: f64,
    pub display_update_granularity: f64,
}

impl DisplayTiming {
    /// Upstream `hasValidTiming`.
    pub fn valid(&self) -> bool {
        self.maximum_frames_per_second > 0
            && self.minimum_refresh_interval.is_finite() && self.minimum_refresh_interval > 0.0
            && self.maximum_refresh_interval.is_finite() && self.maximum_refresh_interval >= self.minimum_refresh_interval
            && self.display_update_granularity.is_finite() && self.display_update_granularity >= 0.0
    }

    /// Upstream `supportsVariableRefresh`: continuous Adaptive-Sync and discrete ProMotion ranges
    /// both qualify; a fixed-refresh screen reports equal intervals.
    pub fn variable_refresh(&self) -> bool {
        self.valid() && self.maximum_refresh_interval - self.minimum_refresh_interval > 0.000_001
    }

    /// The slowest and the fastest refresh rate (Hz), of a valid timing.
    pub fn range_hz(&self) -> (f64, f64) {
        (1.0 / self.maximum_refresh_interval, 1.0 / self.minimum_refresh_interval)
    }
}

/// The timing of the screen SDL's Cocoa window is on. `None` before macOS 12, for a window with no
/// screen and for a timing that is not valid.
///
/// # Safety
/// `cocoa_window` is null or SDL's live `NSWindow`, and this is the main thread, where the window
/// and its screen live.
#[cfg(target_os = "macos")]
pub unsafe fn of_window(cocoa_window: *mut std::ffi::c_void) -> Option<DisplayTiming> {
    use objc2::runtime::AnyObject;
    use objc2::{msg_send, sel};
    let window = cocoa_window.cast::<AnyObject>();
    if window.is_null() { return None; }
    let screen: *mut AnyObject = msg_send![window, screen];
    if screen.is_null() { return None; }
    // The four properties came with macOS 12.
    let known: bool = msg_send![screen, respondsToSelector: sel!(minimumRefreshInterval)];
    if !known { return None; }
    let maximum_frames_per_second: isize = msg_send![screen, maximumFramesPerSecond];
    let timing = DisplayTiming {
        maximum_frames_per_second: maximum_frames_per_second as i64,
        minimum_refresh_interval: msg_send![screen, minimumRefreshInterval],
        maximum_refresh_interval: msg_send![screen, maximumRefreshInterval],
        display_update_granularity: msg_send![screen, displayUpdateGranularity],
    };
    timing.valid().then_some(timing)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timing(fps: i64, shortest: f64, longest: f64, granularity: f64) -> DisplayTiming {
        DisplayTiming { maximum_frames_per_second: fps, minimum_refresh_interval: shortest, maximum_refresh_interval: longest, display_update_granularity: granularity }
    }

    #[test]
    fn a_range_is_variable_when_its_intervals_differ() {
        let promotion = timing(120, 1.0 / 120.0, 1.0 / 24.0, 1.0 / 240.0);
        assert!(promotion.valid() && promotion.variable_refresh());
        let (slowest, fastest) = promotion.range_hz();
        assert!((slowest - 24.0).abs() < 1e-6 && (fastest - 120.0).abs() < 1e-6);
        let adaptive = timing(144, 1.0 / 144.0, 1.0 / 48.0, 0.0);
        assert!(adaptive.variable_refresh(), "a granularity of zero is a continuous range");
        let fixed = timing(60, 1.0 / 60.0, 1.0 / 60.0, 1.0 / 60.0);
        assert!(fixed.valid() && !fixed.variable_refresh(), "equal intervals are a fixed refresh");
    }

    fn mode(pixel_width: u32, pixel_height: u32, native: bool) -> Mode {
        // Valid and safe (IOKit's 0x1 and 0x2 flags), as listed modes are.
        Mode { pixel_width, pixel_height, io_flags: 0x3 | if native { NATIVE_FLAG } else { 0 } }
    }

    // nightjar#118: a Dell U2720Q (3840x2160) in "looks like 2560x1440" gave a native 5120x2880.
    #[test]
    fn the_native_size_is_the_flagged_mode_not_a_larger_scaled_one() {
        let dell = [mode(5120, 2880, false), mode(3840, 2160, true), mode(2560, 1440, false), mode(1920, 1080, false)];
        assert_eq!(native_pixels(&dell), Some((3840, 2160)), "a scaled mode's backing store is larger than the panel");
        // A 14" MacBook Pro's panel, at its default and at a scaled mode alike: the list holds both.
        let built_in = [mode(3600, 2338, false), mode(3024, 1964, true), mode(2560, 1664, false), mode(1512, 982, false)];
        assert_eq!(native_pixels(&built_in), Some((3024, 1964)));
        assert_eq!(native_pixels(&[mode(3024, 1964, true), mode(3024, 1964, true)]), Some((3024, 1964)), "a native mode at several rates");
        assert_eq!(native_pixels(&[mode(1920, 1080, true), mode(3840, 2160, true)]), Some((3840, 2160)), "the largest flagged mode");
    }

    #[test]
    fn without_a_flagged_mode_there_is_no_native_size() {
        assert_eq!(native_pixels(&[]), None);
        assert_eq!(native_pixels(&[mode(5120, 2880, false), mode(3840, 2160, false)]), None, "no guess from sizes");
        assert_eq!(native_pixels(&[mode(0, 2160, true)]), None, "a mode without a size");
    }

    #[test]
    fn a_timing_that_is_not_valid_is_no_range() {
        assert!(!DisplayTiming::default().valid(), "macOS before 12 reports nothing");
        assert!(!timing(0, 1.0 / 120.0, 1.0 / 24.0, 0.0).variable_refresh(), "no frame rate");
        assert!(!timing(120, 0.0, 1.0 / 24.0, 0.0).variable_refresh(), "no shortest interval");
        assert!(!timing(120, 1.0 / 24.0, 1.0 / 120.0, 0.0).variable_refresh(), "the longest is shorter than the shortest");
        assert!(!timing(120, 1.0 / 120.0, f64::INFINITY, 0.0).variable_refresh());
        assert!(!timing(120, 1.0 / 120.0, 1.0 / 24.0, -1.0).variable_refresh());
        assert!(!timing(120, f64::NAN, 1.0 / 24.0, 0.0).variable_refresh());
    }
}
