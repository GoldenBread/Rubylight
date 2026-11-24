//! The few operating-system extensions of the standard library the host calls, under one
//! path on every OS it builds for. On Windows these are `std::os::windows` itself. On Linux
//! each has the meaning the host's use of it needs, stated at its definition.
#[cfg(windows)]
pub use std::os::windows::{
    fs::MetadataExt,
    io::{AsRawSocket, OwnedHandle},
    process::CommandExt,
};

#[cfg(not(windows))]
pub use other::*;

/// The file an app's output setting of `null` sends its output to.
pub const NULL_DEVICE: &str = if cfg!(windows) { "NUL" } else { "/dev/null" };

/// The file an app's output setting names: [`NULL_DEVICE`] for `null`.
pub fn output_file(setting: &str) -> &str {
    if setting == "null" {
        NULL_DEVICE
    } else {
        setting
    }
}

/// The shell command that starts an app's own command. On Windows it is the command itself.
/// On Linux the app runs inside a headless gamescope session at the stream's mode, whose
/// picture the stream captures and whose X server takes the keyboard and mouse
/// (`butterpollo_win_io::gamescope`).
pub fn game_command(
    command: String,
    environment: &std::collections::BTreeMap<String, String>,
) -> anyhow::Result<String> {
    #[cfg(target_os = "linux")]
    {
        let (wrapped, folder) = butterpollo_win_io::gamescope::wrap(&command, environment)?;
        tracing::info!(session = %folder.display(), "the app starts inside a gamescope session");
        Ok(wrapped)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = environment;
        Ok(command)
    }
}

#[cfg(not(windows))]
mod other {
    use std::os::fd::{AsRawFd, RawFd};

    /// An owned handle of the OS: a file descriptor.
    pub type OwnedHandle = std::os::fd::OwnedFd;

    /// The Windows attribute bit the host reads: the file is a link to elsewhere.
    const LINK: u32 = 0x400;

    pub trait MetadataExt {
        /// The file's Windows attribute bits as far as the host reads them: 0x400 for a
        /// symbolic link, nothing else. Take the metadata without following links
        /// (`symlink_metadata`), as the host's callers do, or a link reports its target.
        fn file_attributes(&self) -> u32;
    }
    impl MetadataExt for std::fs::Metadata {
        fn file_attributes(&self) -> u32 {
            if self.file_type().is_symlink() {
                LINK
            } else {
                0
            }
        }
    }

    pub trait CommandExt {
        /// Windows process creation flags (no console window, a new process group). A
        /// child process opens no window of its own here, so the flags change nothing.
        fn creation_flags(&mut self, flags: u32) -> &mut std::process::Command;
    }
    impl CommandExt for std::process::Command {
        fn creation_flags(&mut self, _flags: u32) -> &mut std::process::Command {
            self
        }
    }

    pub trait AsRawSocket {
        /// The socket as the platform's wait and send calls take it: its descriptor.
        fn as_raw_socket(&self) -> RawFd;
    }
    impl<T: AsRawFd> AsRawSocket for T {
        fn as_raw_socket(&self) -> RawFd {
            self.as_raw_fd()
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn only_a_link_carries_the_link_attribute() {
            let folder = std::env::temp_dir().join(format!("os-ext-{}", std::process::id()));
            std::fs::create_dir_all(&folder).unwrap();
            let file = folder.join("file");
            let link = folder.join("link");
            std::fs::write(&file, b"x").unwrap();
            std::os::unix::fs::symlink(&file, &link).unwrap();
            let attributes =
                |path: &std::path::Path| std::fs::symlink_metadata(path).unwrap().file_attributes();
            assert_eq!(attributes(&file), 0);
            assert_eq!(attributes(&folder), 0);
            assert_eq!(attributes(&link), LINK);
            std::fs::remove_dir_all(&folder).unwrap();
        }

        #[test]
        fn creation_flags_leave_the_command_as_it_was() {
            let mut command = std::process::Command::new("true");
            command.creation_flags(0x0800_0000);
            assert!(command.status().unwrap().success());
        }
    }
}
