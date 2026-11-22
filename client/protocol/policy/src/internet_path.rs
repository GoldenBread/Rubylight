//! How a device reaches a host: local network, IPv6, forwarded IPv4 port, or punched direct path.

/// Which path reaches a host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InternetPath {
    Lan,
    Ipv6,
    MappedIpv4,
    Punched,
}

impl InternetPath {
    /// The path's name as the client shows it.
    pub fn label(self) -> &'static str {
        match self {
            InternetPath::Lan => "Local network",
            InternetPath::Ipv6 => "Internet (IPv6)",
            InternetPath::MappedIpv4 => "Internet (forwarded port)",
            InternetPath::Punched => "Internet (direct path)",
        }
    }

    /// A stable key for settings, logs and the JNI bridge.
    pub fn key(self) -> &'static str {
        match self {
            InternetPath::Lan => "lan",
            InternetPath::Ipv6 => "ipv6",
            InternetPath::MappedIpv4 => "mapped",
            InternetPath::Punched => "punched",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        [
            InternetPath::Lan,
            InternetPath::Ipv6,
            InternetPath::MappedIpv4,
            InternetPath::Punched,
        ]
        .into_iter()
        .find(|path| path.key() == key)
    }
}
