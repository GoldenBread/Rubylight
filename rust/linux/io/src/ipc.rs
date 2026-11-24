//! Private, bounded local IPC for owned helper processes: whole messages over a Unix socket in the abstract namespace, each end checking who the other is. Everything the Windows pipe offers the host is available; any local program can still try to connect, and is refused by its user or process ID.

use anyhow::{Context, Result, bail, ensure};
use serde::{Serialize, de::DeserializeOwned};
use socket2::{Domain, SockAddr, Socket, Type};
use std::{
    io::{self, Read},
    os::{fd::AsRawFd, unix::ffi::OsStrExt},
    sync::OnceLock,
};

pub const MESSAGE_LIMIT: usize = 4096;
/// The longest name of a socket in the abstract namespace, after its leading NUL.
const NAME_LIMIT: usize = 107;

/// One end of a private message channel. The server end listens under a random name and
/// takes one client; neither end ever blocks.
pub struct Pipe {
    /// The server's listening socket; None on the client end.
    listener: Option<Socket>,
    /// The connection: the client's own socket, or the one the server accepted.
    connection: OnceLock<Socket>,
}

/// The address of the abstract socket `name`.
fn address(name: &str) -> Result<SockAddr> {
    ensure!(
        !name.is_empty() && name.len() <= NAME_LIMIT && !name.contains('\0'),
        "invalid helper pipe name"
    );
    let mut path = vec![0u8];
    path.extend_from_slice(name.as_bytes());
    Ok(SockAddr::unix(std::ffi::OsStr::from_bytes(&path))?)
}

/// The process and user at the other end of `socket`, as the kernel recorded them when
/// the connection was made (`SO_PEERCRED`).
fn peer(socket: &Socket) -> io::Result<(u32, u32)> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut size = size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `credentials` and `size` are live locals, and `size` is the structure's size.
    let status = unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut credentials).cast(),
            &mut size,
        )
    };
    if status != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((u32::try_from(credentials.pid).unwrap_or(0), credentials.uid))
}

fn own_user() -> u32 {
    // SAFETY: geteuid takes no arguments and cannot fail.
    unsafe { libc::geteuid() }
}

impl Pipe {
    /// A new server end and its name, which starts with `prefix`; the name is what a
    /// client connects to.
    pub fn server(prefix: &str) -> Result<(Self, String)> {
        let mut random = [0u8; 16];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut source| source.read_exact(&mut random))
            .context("naming the helper pipe")?;
        let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let name = format!("{prefix}{suffix}");
        let listener =
            Socket::new(Domain::UNIX, Type::SEQPACKET, None).context("create helper pipe")?;
        listener
            .bind(&address(&name)?)
            .and_then(|()| listener.listen(1))
            .and_then(|()| listener.set_nonblocking(true))
            .context("create helper pipe")?;
        Ok((
            Self {
                listener: Some(listener),
                connection: OnceLock::new(),
            },
            name,
        ))
    }
    /// Connects to the server named `name`, which must be the process `parent` running as
    /// this process's user.
    pub fn client(name: &str, parent: u32, prefix: &str) -> Result<Self> {
        ensure!(name.starts_with(prefix), "invalid helper pipe name");
        let socket =
            Socket::new(Domain::UNIX, Type::SEQPACKET, None).context("open helper pipe")?;
        socket
            .connect(&address(name)?)
            .context("open helper pipe")?;
        let (pid, user) = peer(&socket)?;
        ensure!(
            pid == parent && parent != 0 && user == own_user(),
            "helper pipe server identity mismatch"
        );
        socket.set_nonblocking(true)?;
        Ok(Self {
            listener: None,
            connection: OnceLock::from(socket),
        })
    }
    /// Whether the client has connected; an error unless it is the process
    /// `expected_pid`. A connection from another user is dropped and never counts.
    pub fn connected(&self, expected_pid: u32) -> Result<bool> {
        while self.connection.get().is_none() {
            let Some(listener) = &self.listener else {
                bail!("helper pipe is not connected");
            };
            let accepted = match listener.accept() {
                Ok((accepted, _)) => accepted,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error).context("accepting the helper"),
            };
            if !peer(&accepted).is_ok_and(|(_, user)| user == own_user()) {
                continue;
            }
            accepted.set_nonblocking(true)?;
            // Only this call fills it, and a second caller's connection is dropped.
            let _ = self.connection.set(accepted);
        }
        let Some(connection) = self.connection.get() else {
            return Ok(false);
        };
        let (actual, _) = peer(connection)?;
        ensure!(
            actual == expected_pid && expected_pid != 0,
            "helper pipe client identity mismatch"
        );
        Ok(true)
    }
    fn connection(&self) -> Result<&Socket> {
        self.connection
            .get()
            .context("helper pipe is not connected")
    }
    pub fn send(&self, message: &impl Serialize) -> Result<()> {
        let bytes = serde_json::to_vec(message)?;
        self.send_bytes(&bytes)
    }
    /// Sends one message, whole or not at all: a peer that has stalled or exited is an
    /// error, never a wait.
    pub fn send_bytes(&self, bytes: &[u8]) -> Result<()> {
        ensure!(
            bytes.len() <= MESSAGE_LIMIT,
            "helper message exceeds its bound"
        );
        // An empty message would read as the end of the connection.
        ensure!(!bytes.is_empty(), "empty helper message");
        match self
            .connection()?
            .send_with_flags(bytes, libc::MSG_NOSIGNAL)
        {
            Ok(written) => {
                ensure!(written == bytes.len(), "helper pipe is full");
                Ok(())
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => bail!("helper pipe is full"),
            Err(error) => Err(error).context("writing to the helper pipe"),
        }
    }
    pub fn receive<T: DeserializeOwned>(&self) -> Result<Option<T>> {
        let Some(bytes) = self.receive_bytes()? else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_slice(&bytes)?))
    }
    /// The next message, None when none waits; an error once the other end is gone.
    pub fn receive_bytes(&self) -> Result<Option<Vec<u8>>> {
        let mut connection = self.connection()?;
        // One byte more than a message may have, to tell an oversized one.
        let mut bytes = vec![0; MESSAGE_LIMIT + 1];
        let read = loop {
            match connection.read(&mut bytes) {
                Ok(read) => break read,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(None),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error).context("reading the helper pipe"),
            }
        };
        ensure!(read != 0, "helper pipe closed");
        ensure!(read <= MESSAGE_LIMIT, "oversized helper message");
        bytes.truncate(read);
        Ok(Some(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    const PREFIX: &str = "Butterpollo.IpcTest.";

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    #[serde(tag = "type")]
    enum Request {
        Release { slot: usize },
    }

    #[test]
    fn a_private_pipe_checks_peers_bounds_messages_and_reports_disconnect() -> Result<()> {
        let pid = std::process::id();
        let (server, name) = Pipe::server(PREFIX)?;
        assert!(name.starts_with(PREFIX));
        assert!(!server.connected(pid)?);
        assert!(server.send_bytes(b"early").is_err());
        assert!(Pipe::client(&name, pid.wrapping_add(1), PREFIX).is_err());
        assert!(Pipe::client(&name, pid, "Another.Prefix.").is_err());
        assert!(Pipe::client(&format!("{PREFIX}no-such-server"), pid, PREFIX).is_err());
        // The refused client took the one connection; a fresh server, as a failed start does.
        drop(server);
        let (server, name) = Pipe::server(PREFIX)?;
        let client = Pipe::client(&name, pid, PREFIX)?;
        assert!(server.connected(pid)?);
        assert!(server.connected(pid.wrapping_add(1)).is_err());
        assert!(server.connected(0).is_err());
        assert_eq!(server.receive::<Request>()?, None);
        client.send(&Request::Release { slot: 2 })?;
        assert_eq!(
            server.receive::<Request>()?,
            Some(Request::Release { slot: 2 })
        );
        assert_eq!(server.receive::<Request>()?, None);
        // Messages keep their bounds: two sent are two received.
        server.send_bytes(b"one")?;
        server.send_bytes(b"two")?;
        assert_eq!(client.receive_bytes()?.as_deref(), Some(&b"one"[..]));
        assert_eq!(client.receive_bytes()?.as_deref(), Some(&b"two"[..]));
        assert_eq!(client.receive_bytes()?, None);
        client.send_bytes(&vec![b'x'; MESSAGE_LIMIT])?;
        assert_eq!(
            server.receive_bytes()?.map(|bytes| bytes.len()),
            Some(MESSAGE_LIMIT)
        );
        assert!(client.send_bytes(&vec![b'x'; MESSAGE_LIMIT + 1]).is_err());
        assert!(client.send_bytes(b"").is_err());
        assert_eq!(server.receive::<Request>()?, None);
        drop(client);
        assert!(server.receive::<Request>().is_err());
        assert!(server.send_bytes(b"late").is_err());
        Ok(())
    }

    #[test]
    fn a_name_too_long_for_a_socket_is_refused() {
        assert!(Pipe::server(&"p".repeat(NAME_LIMIT)).is_err());
        assert!(Pipe::client("", 1, "").is_err());
        assert!(Pipe::client("a\0b", 1, "a").is_err());
    }
}
