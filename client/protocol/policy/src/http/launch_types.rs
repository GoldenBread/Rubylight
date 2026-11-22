//! What the PC's displays do during the stream (Rubylight's `hostDisplay`).

use super::apps::AppRole;

/// What the PC's displays do during the stream (Rubylight's `hostDisplay`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum HostDisplay {
    /// The host's own settings decide; nothing is sent.
    #[default]
    HostDefault,
    Exclusive,
    ExtendedPrimary,
    Extended,
    ExtendedIsolated,
    ExtendedPrimaryIsolated,
    Physical,
}

impl HostDisplay {
    pub fn wire_value(self) -> Option<&'static str> {
        match self {
            Self::HostDefault => None,
            Self::Exclusive => Some("exclusive"),
            Self::ExtendedPrimary => Some("extended_primary"),
            Self::Extended => Some("extended"),
            Self::ExtendedIsolated => Some("extended_isolated"),
            Self::ExtendedPrimaryIsolated => Some("extended_primary_isolated"),
            Self::Physical => Some("physical"),
        }
    }

    /// Only a stream chooses; Remote Monitor and Remote Input keep the host's settings.
    pub fn for_role(self, role: AppRole) -> Self {
        if role == AppRole::Stream {
            self
        } else {
            Self::HostDefault
        }
    }

    /// Whether the launch asks for a virtual display; the host default keeps the setting.
    pub fn requests_virtual_display(self, virtual_display_setting: bool) -> bool {
        match self {
            Self::HostDefault => virtual_display_setting,
            Self::Physical => false,
            _ => true,
        }
    }
}
