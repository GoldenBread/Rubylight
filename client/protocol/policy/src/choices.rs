//! Settings that show as one choice but are saved as two switches, so earlier versions and every
//! saved layer (a PC's, a game's, a profile's) keep reading them, as "Controllers" is
//! (`controllers`). Each choice reads a saved pair the way the stream used it before the two rows
//! became one, so nothing a user saved changes behaviour. Both clients list the choices in the
//! order of `ALL`; Android mirrors each in a `*Choice` class of its own.

/// "Back button": what a controller's or handheld's Back does. Saved as "Back opens the stream
/// menu" (desktop `back_opens_menu`, Android `checkbox_back_opens_menu`) and "Back button as
/// Guide" (`back_as_guide`, `checkbox_back_as_guide`). With both on the menu won, on both clients.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackButton {
    /// Back goes to the PC as Back.
    Back,
    /// Back opens the stream menu and never reaches the PC.
    StreamMenu,
    /// Back goes to the PC as Guide.
    Guide,
}

impl BackButton {
    pub const ALL: [Self; 3] = [Self::Back, Self::StreamMenu, Self::Guide];

    pub fn from_saved(opens_menu: bool, as_guide: bool) -> Self {
        if opens_menu {
            Self::StreamMenu
        } else if as_guide {
            Self::Guide
        } else {
            Self::Back
        }
    }

    /// The two switches to save: opens the menu, as Guide.
    pub fn to_saved(self) -> (bool, bool) {
        (self == Self::StreamMenu, self == Self::Guide)
    }
}

/// "Send system keys": whether Alt+Tab, the Windows key and similar go to the PC. Saved as "Send
/// system keys" (desktop `capture_system_keys`, Android `checkbox_capture_system_keys`) and
/// "System keys only in full screen" (`system_keys_fullscreen_only`), which only counted while the
/// first was on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemKeys {
    /// They stay with this device.
    Off,
    /// They go to the PC.
    Always,
    /// They go to the PC in full screen and stay with this device in a window.
    FullScreen,
}

impl SystemKeys {
    pub const ALL: [Self; 3] = [Self::Off, Self::Always, Self::FullScreen];

    pub fn from_saved(send: bool, full_screen_only: bool) -> Self {
        match (send, full_screen_only) {
            (false, _) => Self::Off,
            (true, false) => Self::Always,
            (true, true) => Self::FullScreen,
        }
    }

    /// The two switches to save: send, only in full screen.
    pub fn to_saved(self) -> (bool, bool) {
        (self != Self::Off, self == Self::FullScreen)
    }
}

/// "Keep streaming when the network changes": whether a running stream follows this device to a
/// new network, and onto a metered one (mobile data, a hotspot). Saved as client roaming (desktop
/// `client_roaming`, Android `checkbox_client_roaming`) and "Continue on mobile data"
/// (`roaming_metered`), which only counted while roaming was on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Roaming {
    /// The stream waits for its old network.
    Off,
    /// The stream moves to a new network unless it is metered.
    NotMetered,
    /// The stream moves to any network, metered ones too.
    AnyNetwork,
}

impl Roaming {
    pub const ALL: [Self; 3] = [Self::Off, Self::NotMetered, Self::AnyNetwork];

    pub fn from_saved(roaming: bool, metered: bool) -> Self {
        match (roaming, metered) {
            (false, _) => Self::Off,
            (true, false) => Self::NotMetered,
            (true, true) => Self::AnyNetwork,
        }
    }

    /// The two switches to save: roaming, onto metered networks.
    pub fn to_saved(self) -> (bool, bool) {
        (self != Self::Off, self == Self::AnyNetwork)
    }
}

/// "Lower bitrate": whether this stream's bitrate drops while the connection struggles, and from
/// what. Saved as "Lower bitrate when the network is busy" (desktop `adaptive_bitrate`, Android
/// `adaptive_bitrate`) and "Lower bitrate when frames are lost" (`client_hint_bitrate`, Android
/// `checkbox_rate_change_bitrate`), and "Never: keep my bitrate" (`keep_bitrate` on both). The
/// second takes the place of the first on a PC that takes its rate changes; on a PC that doesn't,
/// the first still runs if it is on. While `keep_bitrate` is set the two others count as off for
/// the stream and stay stored as they were.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LowerBitrate {
    /// The PC lowers it when frames are lost that it cannot repair, and raises it again after.
    Off,
    /// This device lowers it as soon as frames go missing, on a PC that takes its rate changes;
    /// elsewhere it is lowered while the connection falls behind.
    WhenFramesLost,
    /// The PC (or this device) lowers it while the connection falls behind.
    WhenBusy,
    /// This stream keeps the bitrate it was set to: the device sends it to the PC, which stops
    /// lowering it, and this device's own lowering is off.
    Keep,
}

impl LowerBitrate {
    /// The row's order.
    pub const ALL: [Self; 4] = [Self::Off, Self::WhenFramesLost, Self::WhenBusy, Self::Keep];

    pub fn from_saved(when_busy: bool, when_lost: bool, keep: bool) -> Self {
        match (keep, when_busy, when_lost) {
            (true, _, _) => Self::Keep,
            (_, _, true) => Self::WhenFramesLost,
            (_, true, false) => Self::WhenBusy,
            (_, false, false) => Self::Off,
        }
    }

    /// What to save for this choice, as (when busy, when frames are lost, keep), given the two
    /// switches as they are: "Never" leaves them, every other choice writes them and clears the
    /// third. "When frames are lost" keeps the busy controller on, for a PC that takes no rate
    /// changes.
    pub fn to_saved(self, busy: bool, lost: bool) -> (bool, bool, bool) {
        match self {
            Self::Off => (false, false, false),
            Self::WhenBusy => (true, false, false),
            Self::WhenFramesLost => (true, true, false),
            Self::Keep => (busy, lost, true),
        }
    }

    /// The two switches the stream acts on: both off while "Never" is chosen.
    pub fn effective(when_busy: bool, when_lost: bool, keep: bool) -> (bool, bool) {
        if keep { (false, false) } else { (when_busy, when_lost) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_reads_as_the_stream_used_it() {
        assert_eq!(BackButton::from_saved(false, false), BackButton::Back, "both defaults");
        assert_eq!(BackButton::from_saved(true, false), BackButton::StreamMenu);
        assert_eq!(BackButton::from_saved(false, true), BackButton::Guide);
        // The menu won over Guide (gamepad.rs, GameKeyboardInput.handlePhysicalBackMenuKey).
        assert_eq!(BackButton::from_saved(true, true), BackButton::StreamMenu);
    }

    #[test]
    fn system_keys_read_as_the_stream_used_them() {
        assert_eq!(SystemKeys::from_saved(true, false), SystemKeys::Always, "both defaults");
        assert_eq!(SystemKeys::from_saved(true, true), SystemKeys::FullScreen);
        // Full screen only did nothing while the keys were not sent.
        assert_eq!(SystemKeys::from_saved(false, true), SystemKeys::Off);
        assert_eq!(SystemKeys::from_saved(false, false), SystemKeys::Off);
    }

    #[test]
    fn roaming_reads_as_the_stream_used_it() {
        assert_eq!(Roaming::from_saved(true, false), Roaming::NotMetered, "both defaults");
        assert_eq!(Roaming::from_saved(true, true), Roaming::AnyNetwork);
        // Mobile data did nothing while roaming was off (roaming_config).
        assert_eq!(Roaming::from_saved(false, true), Roaming::Off);
        assert_eq!(Roaming::from_saved(false, false), Roaming::Off);
    }

    #[test]
    fn lower_bitrate_reads_as_the_stream_used_it() {
        assert_eq!(LowerBitrate::from_saved(false, false, false), LowerBitrate::Off, "all defaults");
        assert_eq!(LowerBitrate::from_saved(true, false, false), LowerBitrate::WhenBusy);
        // Frame loss took the place of the busy controller wherever the PC took rate changes.
        assert_eq!(LowerBitrate::from_saved(false, true, false), LowerBitrate::WhenFramesLost);
        assert_eq!(LowerBitrate::from_saved(true, true, false), LowerBitrate::WhenFramesLost);
        // "Never" wins over the two switches, whatever they hold.
        for (busy, lost) in [(false, false), (true, false), (false, true), (true, true)] {
            assert_eq!(LowerBitrate::from_saved(busy, lost, true), LowerBitrate::Keep);
            assert_eq!(LowerBitrate::effective(busy, lost, true), (false, false));
            assert_eq!(LowerBitrate::effective(busy, lost, false), (busy, lost));
        }
    }

    #[test]
    fn each_choice_saves_what_reads_back_as_it() {
        for choice in BackButton::ALL {
            let (a, b) = choice.to_saved();
            assert_eq!(BackButton::from_saved(a, b), choice);
        }
        for choice in SystemKeys::ALL {
            let (a, b) = choice.to_saved();
            assert_eq!(SystemKeys::from_saved(a, b), choice);
        }
        for choice in Roaming::ALL {
            let (a, b) = choice.to_saved();
            assert_eq!(Roaming::from_saved(a, b), choice);
        }
        for choice in LowerBitrate::ALL {
            for (busy, lost) in [(false, false), (true, false), (false, true), (true, true)] {
                let (a, b, c) = choice.to_saved(busy, lost);
                assert_eq!(LowerBitrate::from_saved(a, b, c), choice);
            }
        }
        // "Never" leaves the stored pair as it was; the others clear it.
        assert_eq!(LowerBitrate::Keep.to_saved(true, false), (true, false, true));
        assert_eq!(LowerBitrate::Off.to_saved(true, true), (false, false, false));
    }
}
