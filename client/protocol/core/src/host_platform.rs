//! The operating system a Rubylight host runs on.
//!
//! A host names it in the serverinfo element [`SERVERINFO_FIELD`], unpaired too: [`WINDOWS`]
//! or [`LINUX`]. Rubylight's published releases are Windows builds that a Windows host can
//! install; a Linux host is updated by its package manager. A host that sends no platform is
//! a Windows host, as every Rubylight before the Linux host was.

/// Serverinfo element: the host's operating system, [`WINDOWS`] or [`LINUX`].
pub const SERVERINFO_FIELD: &str = "HostPlatform";
pub const WINDOWS: &str = "windows";
pub const LINUX: &str = "linux";

/// The operating system of a host that sent `field` (`None` when it sent none), in lower
/// case.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Platform {
    Windows,
    Linux,
    /// A platform this build does not know.
    Other,
}

impl Platform {
    pub fn parse(field: Option<&str>) -> Self {
        match field.map(str::trim) {
            None | Some("") => Self::Windows,
            Some(value) if value.eq_ignore_ascii_case(WINDOWS) => Self::Windows,
            Some(value) if value.eq_ignore_ascii_case(LINUX) => Self::Linux,
            Some(_) => Self::Other,
        }
    }

    /// Whether Rubylight's published releases update this host: Windows only.
    pub fn updated_by_releases(self) -> bool {
        self == Self::Windows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_without_a_platform_is_windows_and_only_windows_takes_releases() {
        assert_eq!(Platform::parse(None), Platform::Windows);
        assert_eq!(Platform::parse(Some(" ")), Platform::Windows);
        assert_eq!(Platform::parse(Some("windows")), Platform::Windows);
        assert_eq!(Platform::parse(Some("Linux")), Platform::Linux);
        assert_eq!(Platform::parse(Some("freebsd")), Platform::Other);
        assert!(Platform::Windows.updated_by_releases());
        assert!(!Platform::Linux.updated_by_releases());
        assert!(!Platform::Other.updated_by_releases());
    }
}
