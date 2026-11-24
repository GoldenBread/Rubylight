//! The client of Vibepollo's root broker: one line per connection, one line back.
//!
//! Ported from Vibepollo (Nonary/Vibepollo, GPL-3.0), src/platform/linux/private_display_mode_client.cpp,
//! and libvirtualdisplay (Nonary/libvirtualdisplay, MIT, Copyright (c) 2026 Chase Payne),
//! src/driver/linux_control_client.cpp. The broker owns the kernel module's connectors; it
//! connects and disconnects them, reports their state and admits a mode for a connected one.
//! Production checks that the socket belongs to root with no "other" permission and that the
//! peer is root before it sends anything.
use anyhow::{Context, Result, anyhow, bail};
use std::io::{Read, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Where the broker listens.
pub const SOCKET: &str = "/run/vibeshine/vkms-control.sock";
/// A request, from connecting to the reply, may take this long. It stays above the broker's own
/// five-second runtime limit, so a timed-out change cannot outlive the wait.
const DEADLINE: Duration = Duration::from_secs(8);
const MAX_REPLY: usize = 128;

/// What the client checks about the socket and its peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trust {
    /// The socket is root-owned without "other" bits and the peer's uid is 0.
    Root,
    /// Nothing is checked: a fake server in a test.
    #[cfg(test)]
    Unchecked,
}

/// A connection point to the broker.
#[derive(Clone, Debug)]
pub struct Broker {
    pub socket: PathBuf,
    pub trust: Trust,
}

impl Default for Broker {
    fn default() -> Self {
        Self {
            socket: PathBuf::from(SOCKET),
            trust: Trust::Root,
        }
    }
}

/// Whether `name` is a connector the broker manages: `Virtual-` and a number from 1 up.
pub fn managed_name(name: &str) -> bool {
    name.strip_prefix("Virtual-").is_some_and(|n| {
        !n.is_empty()
            && n.len() <= 9
            && !n.starts_with('0')
            && n.bytes().all(|b| b.is_ascii_digit())
    })
}

/// Whether the broker accepts this mode: 64..8192 pixels and 1..1000 Hz.
pub fn valid_mode(width: u32, height: u32, millihz: u32) -> bool {
    (64..=8192).contains(&width)
        && (64..=8192).contains(&height)
        && (1000..=1_000_000).contains(&millihz)
}

impl Broker {
    /// A broker at `socket` that skips the ownership and peer checks, for a fake server.
    #[cfg(test)]
    pub fn unchecked(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
            trust: Trust::Unchecked,
        }
    }

    /// Connect the connector `name`, so the compositor sees a monitor.
    pub fn connect(&self, name: &str) -> Result<()> {
        self.expect(
            name,
            &format!("connect {name}"),
            &format!("OK connected {name}"),
        )
    }

    /// Disconnect the connector `name`.
    pub fn disconnect(&self, name: &str) -> Result<()> {
        self.expect(
            name,
            &format!("disconnect {name}"),
            &format!("OK disconnected {name}"),
        )
    }

    /// Whether the broker reports `name` connected.
    pub fn status(&self, name: &str) -> Result<bool> {
        check_name(name)?;
        let reply = self.transact(&format!("status {name}"))?;
        if reply == format!("STATUS connected {name}") {
            Ok(true)
        } else if reply == format!("STATUS disconnected {name}") {
            Ok(false)
        } else {
            Err(unexpected("status", name, &reply))
        }
    }

    /// Admit a mode for the connected connector `name`; `millihz` is the refresh in millihertz.
    pub fn mode(&self, name: &str, width: u32, height: u32, millihz: u32) -> Result<()> {
        check_name(name)?;
        if !valid_mode(width, height, millihz) {
            bail!("{width}x{height} at {millihz} mHz is outside what the broker admits");
        }
        let payload = format!("{name} {width} {height} {millihz}");
        let reply = self.transact(&format!("mode {payload}"))?;
        if reply == format!("OK mode {payload}") {
            Ok(())
        } else {
            Err(unexpected("mode", name, &reply))
        }
    }

    fn expect(&self, name: &str, request: &str, ok: &str) -> Result<()> {
        check_name(name)?;
        let reply = self.transact(request)?;
        if reply == ok {
            Ok(())
        } else {
            let verb = request.split(' ').next().unwrap_or(request);
            Err(unexpected(verb, name, &reply))
        }
    }

    /// Send one line and return the broker's one-line reply without its newline.
    fn transact(&self, line: &str) -> Result<String> {
        let deadline = Instant::now() + DEADLINE;
        if self.trust == Trust::Root {
            check_socket(&self.socket)?;
        }
        let mut stream = UnixStream::connect(&self.socket).with_context(|| {
            format!(
                "could not connect to the broker at {}",
                self.socket.display()
            )
        })?;
        if self.trust == Trust::Root {
            let uid = peer_uid(&stream)?;
            if uid != 0 {
                bail!("the broker socket is served by uid {uid}, not root");
            }
        }
        let left = |what: &str| {
            deadline
                .checked_duration_since(Instant::now())
                .filter(|d| !d.is_zero())
                .ok_or_else(|| anyhow!("the broker did not {what} in time"))
        };
        stream.set_write_timeout(Some(left("take the request")?))?;
        stream
            .write_all(format!("{line}\n").as_bytes())
            .context("could not send the request to the broker")?;
        let mut reply = Vec::new();
        let mut chunk = [0u8; 64];
        loop {
            stream.set_read_timeout(Some(left("answer")?))?;
            let count = match stream.read(&mut chunk) {
                Ok(0) => bail!("the broker closed the connection without a complete reply"),
                Ok(count) => count,
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    bail!("the broker did not answer in time")
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e).context("could not read the broker's reply"),
            };
            reply.extend_from_slice(&chunk[..count]);
            if reply.len() > MAX_REPLY {
                bail!("the broker's reply is too long");
            }
            if let Some(end) = reply.iter().position(|&b| b == b'\n') {
                if end + 1 != reply.len() || reply.contains(&b'\r') {
                    bail!("the broker's reply is malformed");
                }
                reply.truncate(end);
                return String::from_utf8(reply).context("the broker's reply is not text");
            }
        }
    }
}

fn check_name(name: &str) -> Result<()> {
    if managed_name(name) {
        Ok(())
    } else {
        bail!("{name:?} is not a connector the broker manages")
    }
}

fn unexpected(verb: &str, name: &str, reply: &str) -> anyhow::Error {
    // The reply is the broker's own text; keep it short and printable for a log line.
    let shown: String = reply
        .chars()
        .filter(|c| !c.is_control())
        .take(100)
        .collect();
    anyhow!("the broker did not accept `{verb} {name}`: it answered {shown:?}")
}

/// The socket must be a socket of root's with no permission for "other": anyone else could
/// have put a stand-in there.
pub(super) fn check_socket(path: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(path)
        .with_context(|| format!("the broker socket {} is missing", path.display()))?;
    if !meta.file_type().is_socket() || meta.uid() != 0 || meta.mode() & 0o007 != 0 {
        bail!(
            "{} is not a root-owned socket without access for other users",
            path.display()
        );
    }
    Ok(())
}

fn peer_uid(stream: &UnixStream) -> Result<u32> {
    let mut cred = libc::ucred {
        pid: 0,
        uid: u32::MAX,
        gid: 0,
    };
    let mut size = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: the descriptor is a live socket and `cred` and `size` are valid for the
    // SO_PEERCRED read, which writes at most `size` bytes.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut cred).cast(),
            &mut size,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error())
            .context("could not read the broker's identity");
    }
    Ok(cred.uid)
}

#[cfg(test)]
pub(crate) mod fake {
    //! A broker that runs in a test: answers by a closure, one line per connection.
    use super::*;
    use std::os::unix::net::UnixListener;

    pub struct Fake {
        pub dir: PathBuf,
        pub socket: PathBuf,
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// A fresh directory under the system temp dir.
    pub fn temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "rubylight-vd-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A sysfs-like tree: devices under `dev`, connectors under `drm`.
    pub fn connector(base: &Path, entry: &str, device: &str, status: &str) {
        let dev = base.join("dev").join(device);
        std::fs::create_dir_all(&dev).unwrap();
        let dir = base.join("drm").join(entry);
        std::fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(&dev, dir.join("device")).unwrap();
        std::fs::write(dir.join("status"), format!("{status}\n")).unwrap();
    }

    /// Serve connections until the `Fake` drops. `answer` gets each request line (without the
    /// newline) and returns the raw bytes to send back.
    pub fn serve(answer: impl Fn(&str) -> Vec<u8> + Send + 'static) -> (Fake, Broker) {
        let dir = temp_dir("broker");
        let socket = dir.join("vkms-control.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut line = Vec::new();
                let mut byte = [0u8; 1];
                while stream.read(&mut byte).is_ok_and(|n| n == 1) && byte[0] != b'\n' {
                    line.push(byte[0]);
                }
                let _ = stream.write_all(&answer(&String::from_utf8_lossy(&line)));
            }
        });
        let broker = Broker::unchecked(&socket);
        (Fake { dir, socket }, broker)
    }
}

#[cfg(test)]
mod tests {
    use super::fake::serve;
    use super::*;

    fn ok_for(request: &str) -> Vec<u8> {
        let mut parts = request.split(' ');
        let line = match (parts.next(), parts.next()) {
            (Some("connect"), Some(n)) => format!("OK connected {n}"),
            (Some("disconnect"), Some(n)) => format!("OK disconnected {n}"),
            (Some("status"), Some(n)) => format!("STATUS connected {n}"),
            (Some("mode"), _) => format!("OK {request}"),
            _ => "ERROR what".into(),
        };
        format!("{line}\n").into_bytes()
    }

    #[test]
    fn each_verb_sends_its_line_and_needs_the_exact_reply() {
        let (_fake, broker) = serve(ok_for);
        broker.connect("Virtual-1").unwrap();
        broker.disconnect("Virtual-2").unwrap();
        assert!(broker.status("Virtual-3").unwrap());
        broker.mode("Virtual-1", 1920, 1080, 60_000).unwrap();
    }

    #[test]
    fn a_refusal_is_reported_with_the_broker_s_words() {
        let (_fake, broker) = serve(|_| b"ERROR not owned by caller\n".to_vec());
        let error = format!("{:#}", broker.connect("Virtual-1").unwrap_err());
        assert!(error.contains("ERROR not owned by caller"), "{error}");
        assert!(error.contains("connect Virtual-1"), "{error}");
    }

    #[test]
    fn a_reply_for_another_connector_or_a_malformed_one_fails() {
        let (_f1, wrong) = serve(|_| b"OK connected Virtual-2\n".to_vec());
        assert!(wrong.connect("Virtual-1").is_err());
        let (_f2, crlf) = serve(|_| b"OK connected Virtual-1\r\n".to_vec());
        assert!(crlf.connect("Virtual-1").is_err());
        let (_f3, early) = serve(|_| Vec::new());
        assert!(format!("{:#}", early.connect("Virtual-1").unwrap_err()).contains("closed"));
        let (_f4, long) = serve(|_| vec![b'x'; 300]);
        assert!(long.status("Virtual-1").is_err());
    }

    #[test]
    fn names_and_modes_are_checked_before_anything_is_sent() {
        let (_fake, broker) = serve(ok_for);
        assert!(broker.connect("DP-1").is_err());
        assert!(broker.connect("Virtual-0").is_err());
        assert!(broker.connect("Virtual-1\n").is_err());
        assert!(broker.mode("Virtual-1", 63, 1080, 60_000).is_err());
        assert!(broker.mode("Virtual-1", 1920, 1080, 999).is_err());
        assert!(managed_name("Virtual-12") && !managed_name("Virtual-"));
    }

    #[test]
    fn production_trust_refuses_a_socket_that_root_does_not_own() {
        let (fake, _) = serve(ok_for);
        let broker = Broker {
            socket: fake.socket.clone(),
            trust: Trust::Root,
        };
        let error = format!("{:#}", broker.status("Virtual-1").unwrap_err());
        assert!(error.contains("root-owned"), "{error}");
        let missing = Broker {
            socket: fake.dir.join("none.sock"),
            trust: Trust::Root,
        };
        assert!(missing.status("Virtual-1").is_err());
    }
}
