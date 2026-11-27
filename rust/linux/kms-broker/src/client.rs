//! The host's side of the broker: connect, list the outputs, and take frames as they change.
use crate::{
    fdpass,
    protocol::{FrameInfo, OutputInfo, Reply, Request},
};
use anyhow::{Context, Result, bail};
use std::{
    os::fd::{AsFd, FromRawFd, OwnedFd},
    path::Path,
    time::Duration,
};

/// A connection to the broker; the broker that serves it ends with it.
pub struct Client {
    socket: OwnedFd,
}

/// A framebuffer as the broker handed it over: its description, its planes' DMA-BUFs and the
/// cursor's. The buffers stay the compositor's, which draws into them again once it has
/// flipped away.
#[derive(Debug)]
pub struct Frame {
    pub info: FrameInfo,
    pub planes: Vec<OwnedFd>,
    pub cursor: Option<OwnedFd>,
}

impl Client {
    /// Connects to the broker at `path` (normally [`crate::protocol::SOCKET`]).
    pub fn connect(path: &Path) -> Result<Self> {
        // SAFETY: plain socket creation; the descriptor is owned below.
        let fd =
            unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error()).context("making a socket");
        }
        // SAFETY: just made.
        let socket = unsafe { OwnedFd::from_raw_fd(fd) };
        let bytes = path.as_os_str().as_encoded_bytes();
        // SAFETY: plain data; zero is a valid start.
        let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        address.sun_family = libc::AF_UNIX as libc::sa_family_t;
        if bytes.len() >= address.sun_path.len() {
            bail!("the socket path {} is too long", path.display());
        }
        for (to, &from) in address.sun_path.iter_mut().zip(bytes) {
            *to = from as libc::c_char;
        }
        // SAFETY: `address` is a valid sockaddr_un for the call.
        let rc = unsafe {
            libc::connect(
                fd,
                (&raw const address).cast(),
                size_of::<libc::sockaddr_un>() as libc::socklen_t,
            )
        };
        if rc != 0 {
            let error = std::io::Error::last_os_error();
            let hint = match error.raw_os_error() {
                Some(libc::ENOENT | libc::ECONNREFUSED) => " (is rubylight-kms.socket enabled?)",
                Some(libc::EACCES) => " (is this user in the rubylight group?)",
                _ => "",
            };
            return Err(error).with_context(|| {
                format!("connecting to the KMS broker at {}{hint}", path.display())
            });
        }
        Ok(Self { socket })
    }

    /// A client over an already connected socket.
    pub fn from_socket(socket: OwnedFd) -> Self {
        Self { socket }
    }

    fn ask(&self, request: &Request) -> Result<(Reply, Vec<OwnedFd>)> {
        fdpass::send(self.socket.as_fd(), &request.encode(), &[])?;
        let (bytes, fds) = fdpass::receive(self.socket.as_fd())
            .context("reading the KMS broker's reply")?
            .context("the KMS broker closed the connection")?;
        let reply = Reply::decode(&bytes).context("the KMS broker's reply is not readable")?;
        if let Reply::Error { message } = &reply {
            bail!("the KMS broker: {message}");
        }
        if fds.len() != reply.fds() {
            bail!(
                "the KMS broker sent {} descriptors for a reply that names {}",
                fds.len(),
                reply.fds()
            );
        }
        Ok((reply, fds))
    }

    /// The lit outputs.
    pub fn outputs(&self) -> Result<Vec<OutputInfo>> {
        match self.ask(&Request::Outputs)?.0 {
            Reply::Outputs { outputs } => Ok(outputs),
            other => bail!("the KMS broker answered {other:?} to a list of outputs"),
        }
    }

    /// What `output` shows: with `fresh`, once it differs from the last frame this client
    /// took (another framebuffer, or the pointer moved), else now; `None` after `wait` without
    /// a change, or while the output is not lit.
    pub fn frame(&self, output: &str, fresh: bool, wait: Duration) -> Result<Option<Frame>> {
        let request = Request::Frame {
            output: output.to_owned(),
            fresh,
            wait_ms: wait.as_millis().min(u128::from(u32::MAX)) as u32,
        };
        let (reply, mut fds) = self.ask(&request)?;
        match reply {
            Reply::Frame(info) => {
                let cursor = info.cursor.is_some().then(|| fds.pop()).flatten();
                Ok(Some(Frame {
                    info,
                    planes: fds,
                    cursor,
                }))
            }
            Reply::Timeout => Ok(None),
            other => bail!("the KMS broker answered {other:?} to a frame request"),
        }
    }
}
