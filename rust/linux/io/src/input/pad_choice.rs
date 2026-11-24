//! Which virtual pad a client's controller becomes on Linux. The decision is the Windows
//! host's (`Policy::controller_profile` over the pads on offer, the Steam Deck rule of
//! `Gamepads::ensure_deck` and the Switch 2 Pro Controller rule of `ensure_switch2`); what
//! differs is the pads on offer: an Xbox 360 pad on `/dev/uinput` and, with `/dev/uhid`, the pads
//! the kernel has a driver for. Over uhid the kernel binds hid-playstation to a DualSense and a
//! DualShock 4, hid-nintendo to a Switch Pro Controller and hid-steam to a Steam Deck and a
//! Steam Controller (2026). No kernel driver binds a Switch 2 Pro Controller (057e:2069)
//! as a HID device, so that one is a Switch Pro Controller here, with a warning that says why.

use butterpollo_input_policy::input_policy::{
    self, CTYPE_STEAM, DeckPad, Policy, USBIP_DUALSENSE, USBIP_SWITCH2, VHF_DS4, VHF_DUALSENSE,
    VHF_STEAM_CONTROLLER, VHF_SWITCH, VHF_XBOX_ONE, VHF_XBOX_SERIES, VIGEM_DS4, VIGEM_X360,
};

/// A kind of virtual pad.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Want {
    /// An Xbox 360 pad on `/dev/uinput`.
    Xbox,
    DualSense,
    DualShock4,
    Switch,
    SteamController,
    Deck,
}

impl Want {
    /// The warning key of a pad of this kind that could not be made, and what to call it.
    pub(super) fn warning(self) -> (&'static str, &'static str) {
        match self {
            Self::Xbox => ("input_gamepad", "Xbox 360 pad"),
            Self::DualSense => ("input_dualsense", "DualSense"),
            Self::DualShock4 => ("input_ds4", "DualShock 4"),
            Self::Switch => ("input_switch", "Switch Pro Controller"),
            Self::SteamController => ("input_steam_controls", "Steam Controller"),
            Self::Deck => ("input_steam_deck", "Steam Deck controller"),
        }
    }
    fn profile(self) -> u16 {
        match self {
            Self::Xbox => VHF_XBOX_SERIES,
            Self::DualSense => VHF_DUALSENSE,
            Self::DualShock4 => VHF_DS4,
            Self::Switch => VHF_SWITCH,
            Self::SteamController => VHF_STEAM_CONTROLLER,
            Self::Deck => 0,
        }
    }
}

/// Whether Steam is running on this PC: the Steam Deck's automatic choice and the Steam
/// Controller's both wait for it, as on Windows, since games see those pads only through
/// Steam Input.
pub(super) fn steam_running() -> bool {
    crate::process::processes().is_ok_and(|list| list.iter().any(|process| process.name == "steam"))
}

/// What chooses a pad: the `gamepad` setting, the input policy and what the PC offers.
pub(super) struct Chooser<'a> {
    pub(super) profile: u16,
    pub(super) policy: &'a Policy,
    /// Whether `/dev/uhid` can be written.
    pub(super) uhid: bool,
    pub(super) steam_running: &'a dyn Fn() -> bool,
}

impl Chooser<'_> {
    /// The pad for a controller of `kind` with `capabilities` (and `identity`, the USB vendor
    /// and product its client reported), leaving out the pads in `refused`: those that could
    /// not be made for it.
    pub(super) fn want(
        &self,
        (kind, capabilities): (u8, u16),
        identity: Option<(u16, u16)>,
        refused: &[Want],
    ) -> Want {
        if self.deck(kind, refused) {
            return Want::Deck;
        }
        let mut available = 1 << (VHF_XBOX_SERIES - 1);
        if self.uhid {
            for pad in [
                Want::DualSense,
                Want::DualShock4,
                Want::Switch,
                Want::SteamController,
            ] {
                if !refused.contains(&pad) {
                    available |= 1 << (pad.profile() - 1);
                }
            }
        }
        // A Switch 2 Pro Controller has no kernel driver: the Switch Pro Controller is the
        // closest pad that has one.
        if input_policy::wants_switch2(self.profile, identity)
            && available & (1 << (VHF_SWITCH - 1)) != 0
        {
            return Want::Switch;
        }
        let steam = self.profile == 0
            && self.policy.steam_app
            && available & (1 << (VHF_STEAM_CONTROLLER - 1)) != 0
            && input_policy::steam_controls(kind, capabilities)
            && (self.steam_running)();
        let configured = match self.profile {
            USBIP_DUALSENSE => VHF_DUALSENSE,
            USBIP_SWITCH2 => VHF_SWITCH,
            VHF_XBOX_ONE | VIGEM_X360 => VHF_XBOX_SERIES,
            VIGEM_DS4 => VHF_DS4,
            other => other,
        };
        match self
            .policy
            .controller_profile(configured, kind, capabilities, available, steam)
        {
            Some(VHF_DUALSENSE) => Want::DualSense,
            Some(VHF_DS4) => Want::DualShock4,
            Some(VHF_SWITCH) => Want::Switch,
            Some(VHF_STEAM_CONTROLLER) => Want::SteamController,
            _ => Want::Xbox,
        }
    }
    /// Whether a Steam Deck client's controller becomes a Steam Deck's: the
    /// `steam_deck_controller` setting says so, or it is automatic and Steam runs.
    fn deck(&self, kind: u8, refused: &[Want]) -> bool {
        // A pad chosen by name is the one every client gets.
        if matches!(
            self.profile,
            USBIP_DUALSENSE | USBIP_SWITCH2 | VIGEM_X360 | VIGEM_DS4 | VHF_STEAM_CONTROLLER
        ) || kind != CTYPE_STEAM
            || !self.uhid
            || refused.contains(&Want::Deck)
        {
            return false;
        }
        match self.policy.steam_deck {
            DeckPad::VirtualPad => false,
            DeckPad::SteamDeck => true,
            DeckPad::Auto => (self.steam_running)(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn want(
        profile: u16,
        uhid: bool,
        steam: bool,
        arrival: (u8, u16),
        identity: Option<(u16, u16)>,
    ) -> Want {
        let policy = Policy::resolve(&butterpollo_config::config::Config::default()).unwrap();
        let running = move || steam;
        Chooser {
            profile,
            policy: &policy,
            uhid,
            steam_running: &running,
        }
        .want(arrival, identity, &[])
    }

    #[test]
    fn the_pad_follows_the_controller_like_on_windows() {
        let auto = |uhid, arrival| want(0, uhid, false, arrival, None);
        // An Xbox controller with no sensors stays an Xbox pad.
        assert_eq!(auto(true, (1, 0)), Want::Xbox);
        // A PlayStation controller, or one with motion sensors or a touchpad, is a DualSense.
        assert_eq!(auto(true, (2, 0)), Want::DualSense);
        assert_eq!(auto(true, (1, 0x10)), Want::DualSense);
        assert_eq!(auto(true, (0, 8)), Want::DualSense);
        // A Nintendo controller is a Switch Pro Controller, with its gyro.
        assert_eq!(auto(true, (3, 0)), Want::Switch);
        assert_eq!(auto(true, (3, 0x30)), Want::Switch);
        // Without /dev/uhid every controller is an Xbox pad.
        assert_eq!(auto(false, (2, 0x38)), Want::Xbox);
        assert_eq!(auto(false, (3, 0x30)), Want::Xbox);
    }

    #[test]
    fn the_setting_decides_first() {
        for xbox in [VHF_XBOX_ONE, VHF_XBOX_SERIES, VIGEM_X360] {
            assert_eq!(want(xbox, true, false, (2, 0x38), None), Want::Xbox);
        }
        for (setting, pad) in [
            (VHF_DUALSENSE, Want::DualSense),
            (USBIP_DUALSENSE, Want::DualSense),
            (VHF_DS4, Want::DualShock4),
            (VIGEM_DS4, Want::DualShock4),
            (VHF_SWITCH, Want::Switch),
            (VHF_STEAM_CONTROLLER, Want::SteamController),
        ] {
            assert_eq!(want(setting, true, false, (1, 0), None), pad, "{setting}");
        }
    }

    #[test]
    fn a_steam_deck_follows_the_setting_and_steam() {
        let deck = (CTYPE_STEAM, 0x38);
        // Automatic: a Steam Deck when Steam runs, else a DualSense for its sensors.
        assert_eq!(want(0, true, true, deck, None), Want::Deck);
        assert_eq!(want(0, true, false, deck, None), Want::DualSense);
        assert_eq!(want(0, false, true, deck, None), Want::Xbox);
        // A pad chosen by name is every client's.
        assert_eq!(want(VIGEM_DS4, true, true, deck, None), Want::DualShock4);
        assert_eq!(want(VHF_DS4, true, true, deck, None), Want::Deck);
    }

    #[test]
    fn a_switch_2_pro_controller_is_a_switch_pro_controller() {
        let identity = Some((0x057e, 0x2069));
        assert_eq!(want(0, true, false, (3, 0x30), identity), Want::Switch);
        assert_eq!(want(0, true, false, (1, 0), identity), Want::Switch);
        assert_eq!(want(USBIP_SWITCH2, true, false, (1, 0), None), Want::Switch);
        // With no uhid it is whatever the policy gives.
        assert_eq!(want(0, false, false, (1, 0), identity), Want::Xbox);
    }

    #[test]
    fn a_pad_the_kernel_would_not_bind_gives_way() {
        let policy = Policy::resolve(&butterpollo_config::config::Config::default()).unwrap();
        let running = || false;
        let chooser = Chooser {
            profile: 0,
            policy: &policy,
            uhid: true,
            steam_running: &running,
        };
        // A Nintendo controller with a gyro keeps its gyro on a DualSense.
        assert_eq!(
            chooser.want((3, 0x30), None, &[Want::Switch]),
            Want::DualSense
        );
        assert_eq!(
            chooser.want(
                (3, 0x30),
                None,
                &[Want::Switch, Want::DualSense, Want::DualShock4]
            ),
            Want::Xbox
        );
    }
}
