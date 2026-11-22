//! Pure host power action model and platform policy helpers, without OS D-Bus or Win32 calls.

use crate::strings::en;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerAction {
    ShutDown,
    Restart,
    Sleep,
}

impl PowerAction {
    /// In the order the Close Nightjar question offers them.
    pub const ALL: [Self; 3] = [Self::ShutDown, Self::Restart, Self::Sleep];

    pub fn label(self) -> &'static str {
        match self {
            Self::ShutDown => en("Shut down"),
            Self::Restart => en("Restart"),
            Self::Sleep => en("Sleep"),
        }
    }

    /// The question that confirms it.
    pub fn question(self) -> &'static str {
        match self {
            Self::ShutDown => en("Shut down this PC?"),
            Self::Restart => en("Restart this PC?"),
            Self::Sleep => en("Put this PC to sleep?"),
        }
    }

    /// The title of the message when the system refused, the reason under it.
    pub fn failed(self) -> &'static str {
        match self {
            Self::ShutDown => en("Could not shut down this PC"),
            Self::Restart => en("Could not restart this PC"),
            Self::Sleep => en("Could not put this PC to sleep"),
        }
    }

    /// logind's Manager method for it.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn login1_method(self) -> &'static str {
        match self {
            Self::ShutDown => "PowerOff",
            Self::Restart => "Reboot",
            Self::Sleep => "Suspend",
        }
    }

    /// InitiateShutdownW's flags, None for sleep (SetSuspendState). Other apps are asked to
    /// close, not forced, as the Start menu does.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn shutdown_flags(self) -> Option<u32> {
        // SHUTDOWN_POWEROFF and SHUTDOWN_RESTART.
        match self {
            Self::ShutDown => Some(0x0000_0008),
            Self::Restart => Some(0x0000_0004),
            Self::Sleep => None,
        }
    }

    /// logind's Manager method that says whether it may be done.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn login1_can(self) -> &'static str {
        match self {
            Self::ShutDown => "CanPowerOff",
            Self::Restart => "CanReboot",
            Self::Sleep => "CanSuspend",
        }
    }
}

/// An action the system says this session may take. `interactive`: logind wants a password
/// first ("challenge"), so the request lets polkit ask for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Offer {
    pub action: PowerAction,
    pub interactive: bool,
}

/// Windows' offers by its power capabilities: shut down and restart always, Sleep only with a
/// sleep state (S1 to S3). A Modern Standby PC (no S3) gets no Sleep: SetSuspendState cannot put
/// it into standby and may hibernate it instead.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn windows_offers(sleep_state: bool) -> Vec<Offer> {
    PowerAction::ALL
        .into_iter()
        .filter(|a| *a != PowerAction::Sleep || sleep_state)
        .map(|action| Offer { action, interactive: false })
        .collect()
}

/// logind's answer to `Can…` for `action`: "yes" offers it, "challenge" offers it behind a
/// password prompt, "no", "na" or no answer leave it out.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn logind_offer(action: PowerAction, answer: Option<&str>) -> Option<Offer> {
    match answer? {
        "yes" => Some(Offer { action, interactive: false }),
        "challenge" => Some(Offer { action, interactive: true }),
        _ => None,
    }
}
