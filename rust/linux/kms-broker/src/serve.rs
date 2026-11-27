//! The broker's side of one connection (its stdin, from a socket-activated unit): check the
//! caller, then answer its requests until it goes or stays silent for [`IDLE`].
use crate::{
    drm::{Card, Output, cards},
    fdpass,
    protocol::{CursorInfo, FrameInfo, GROUP, MAX_WAIT_MS, OutputInfo, PlaneInfo, Reply, Request},
};
use anyhow::{Context, Result, bail};
use std::{
    os::fd::{AsFd, BorrowedFd},
    time::{Duration, Instant},
};

/// How long a caller may stay silent before the broker ends.
const IDLE: Duration = Duration::from_secs(60);
/// How often a followed output is looked up again: another session may drive it from another
/// controller, or at another mode.
const RESOLVE_EVERY: Duration = Duration::from_millis(500);
/// The wait between looks at a controller without vblank interrupts (a virtual GPU's).
const POLL: Duration = Duration::from_millis(2);

/// The kernel's account of the caller.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Peer {
    pub pid: i32,
    pub uid: u32,
    pub gid: u32,
    pub groups: Vec<u32>,
}

impl Peer {
    pub fn of(socket: BorrowedFd<'_>) -> Result<Self> {
        use std::os::fd::AsRawFd;
        let mut cred = libc::ucred {
            pid: 0,
            uid: u32::MAX,
            gid: u32::MAX,
        };
        let mut size = size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: `cred` and `size` are valid for the SO_PEERCRED read.
        let rc = unsafe {
            libc::getsockopt(
                socket.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&raw mut cred).cast(),
                &mut size,
            )
        };
        if rc != 0 || cred.uid == u32::MAX {
            bail!(
                "the caller's credentials: {}",
                std::io::Error::last_os_error()
            );
        }
        let mut groups = vec![0u32; 256];
        let mut size = (groups.len() * 4) as libc::socklen_t;
        // SAFETY: the buffer holds `size` bytes; the kernel writes at most that many.
        let rc = unsafe {
            libc::getsockopt(
                socket.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERGROUPS,
                groups.as_mut_ptr().cast(),
                &mut size,
            )
        };
        groups.truncate(if rc == 0 { size as usize / 4 } else { 0 });
        Ok(Self {
            pid: cred.pid,
            uid: cred.uid,
            gid: cred.gid,
            groups,
        })
    }

    /// Root, or a member of `group` (its primary group or a supplementary one).
    pub fn may_capture(&self, group: Option<u32>) -> bool {
        self.uid == 0 || group.is_some_and(|g| self.gid == g || self.groups.contains(&g))
    }
}

/// The id of the group named `name`.
fn group_id(name: &str) -> Option<u32> {
    let name = std::ffi::CString::new(name).ok()?;
    let mut group: libc::group = unsafe { std::mem::zeroed() };
    let mut buffer = vec![0 as libc::c_char; 16 * 1024];
    let mut found = std::ptr::null_mut();
    // SAFETY: every pointer is to a live buffer of the stated size; `found` is set to `group`
    // or null.
    let rc = unsafe {
        libc::getgrnam_r(
            name.as_ptr(),
            &raw mut group,
            buffer.as_mut_ptr(),
            buffer.len(),
            &raw mut found,
        )
    };
    (rc == 0 && !found.is_null()).then_some(group.gr_gid)
}

fn info(output: &Output) -> OutputInfo {
    OutputInfo {
        name: output.name.clone(),
        width: output.mode.0,
        height: output.mode.1,
        refresh_millihz: output.mode.2,
    }
}

/// What a frame showed: the output, its framebuffer, and the cursor's image and place.
type Shown = (String, u32, Option<(u32, i32, i32)>);

/// The cards that open, the output being followed, and what the caller was sent last.
struct Screen {
    cards: Vec<Card>,
    followed: Option<(usize, Output, Instant)>,
    sent: Option<Shown>,
}

impl Screen {
    fn open() -> Self {
        Self {
            cards: cards().iter().filter_map(|p| Card::open(p).ok()).collect(),
            followed: None,
            sent: None,
        }
    }

    fn outputs(&self) -> Vec<OutputInfo> {
        self.cards
            .iter()
            .flat_map(|card| card.outputs().unwrap_or_default())
            .map(|o| info(&o))
            .collect()
    }

    /// The output named `name` and its card, looked up again every [`RESOLVE_EVERY`].
    fn find(&mut self, name: &str) -> Option<(usize, Output)> {
        if let Some((card, output, when)) = &self.followed
            && output.name == name
            && when.elapsed() < RESOLVE_EVERY
        {
            return Some((*card, output.clone()));
        }
        let found = self.cards.iter().enumerate().find_map(|(i, card)| {
            card.outputs()
                .ok()?
                .into_iter()
                .find(|o| o.name == name)
                .map(|o| (i, o))
        });
        self.followed = found
            .clone()
            .map(|(card, output)| (card, output, Instant::now()));
        found
    }

    /// Waits until `name` shows something else than the caller was sent last (with `fresh`),
    /// and sends it with its descriptors.
    fn frame(
        &mut self,
        socket: BorrowedFd<'_>,
        name: &str,
        fresh: bool,
        wait_ms: u32,
    ) -> Result<()> {
        let deadline = Instant::now() + Duration::from_millis(u64::from(wait_ms.min(MAX_WAIT_MS)));
        loop {
            if let Some((index, output)) = self.find(name) {
                let card = &self.cards[index];
                let fb = card.primary_fb(output.crtc_id).unwrap_or(0);
                let cursor = card.cursor(output.crtc_id).ok().flatten();
                let shown = (name.to_owned(), fb, cursor.map(|c| (c.fb_id, c.x, c.y)));
                if fb != 0 && (!fresh || self.sent.as_ref() != Some(&shown)) {
                    send_frame(socket, card, &output, fb, cursor)?;
                    self.sent = Some(shown);
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    break;
                }
                if card.wait_vblank(output.pipe).is_err() {
                    std::thread::sleep(POLL);
                }
            } else if Instant::now() >= deadline {
                break;
            } else {
                std::thread::sleep(POLL * 10);
            }
        }
        fdpass::send(socket, &Reply::Timeout.encode(), &[])
    }
}

fn send_frame(
    socket: BorrowedFd<'_>,
    card: &Card,
    output: &Output,
    fb: u32,
    cursor: Option<crate::drm::Cursor>,
) -> Result<()> {
    let grab = card.grab(fb)?;
    let cursor = cursor.and_then(|c| card.grab(c.fb_id).ok().map(|g| (c, g)));
    let info = FrameInfo {
        output: info(output),
        fb_id: grab.fb_id,
        width: grab.width,
        height: grab.height,
        fourcc: grab.fourcc,
        modifier: grab.modifier,
        planes: grab
            .planes
            .iter()
            .map(|p| PlaneInfo {
                pitch: p.pitch,
                offset: p.offset,
            })
            .collect(),
        cursor: cursor.as_ref().map(|(c, g)| CursorInfo {
            fb_id: c.fb_id,
            x: c.x,
            y: c.y,
            // Cursor planes do not scale; a driver that does not say takes the image's size.
            width: if c.width == 0 { g.width } else { c.width },
            height: if c.height == 0 { g.height } else { c.height },
            fourcc: g.fourcc,
            modifier: g.modifier,
            pitch: g.planes[0].pitch,
            offset: g.planes[0].offset,
        }),
    };
    let mut fds: Vec<BorrowedFd<'_>> = grab.planes.iter().map(|p| p.fd.as_fd()).collect();
    if let Some((_, g)) = &cursor {
        fds.push(g.planes[0].fd.as_fd());
    }
    fdpass::send(socket, &Reply::Frame(info).encode(), &fds)
}

/// Serves the connection on stdin.
pub fn serve_stdio() -> Result<()> {
    let socket = std::io::stdin();
    let socket = socket.as_fd();
    let peer = Peer::of(socket)?;
    let group = group_id(GROUP);
    if !peer.may_capture(group) {
        eprintln!(
            "refused pid {} (uid {}): not root or in the {GROUP} group",
            peer.pid, peer.uid
        );
        let message = format!("only root and the {GROUP} group may capture the screen");
        let _ = fdpass::send(socket, &Reply::Error { message }.encode(), &[]);
        return Ok(());
    }
    let exe = std::fs::read_link(format!("/proc/{}/exe", peer.pid))
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "?".into());
    eprintln!(
        "pid {} (uid {}, {exe}) captures the screen",
        peer.pid, peer.uid
    );
    set_idle_timeout(socket)?;
    let mut screen = Screen::open();
    if screen.cards.is_empty() {
        bail!("no DRM card opens");
    }
    loop {
        let Some((bytes, _)) = (match fdpass::receive(socket) {
            Ok(message) => message,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                eprintln!("pid {} was silent for {} s", peer.pid, IDLE.as_secs());
                return Ok(());
            }
            Err(e) => return Err(e).context("reading a request"),
        }) else {
            eprintln!("pid {} is done", peer.pid);
            return Ok(());
        };
        match Request::decode(&bytes) {
            Some(Request::Outputs) => {
                let outputs = screen.outputs();
                fdpass::send(socket, &Reply::Outputs { outputs }.encode(), &[])?;
            }
            Some(Request::Frame {
                output,
                fresh,
                wait_ms,
            }) => {
                if let Err(error) = screen.frame(socket, &output, fresh, wait_ms) {
                    let message = format!("{error:#}");
                    fdpass::send(socket, &Reply::Error { message }.encode(), &[])?;
                }
            }
            None => {
                let message = "the request is not readable".to_string();
                fdpass::send(socket, &Reply::Error { message }.encode(), &[])?;
            }
        }
    }
}

fn set_idle_timeout(socket: BorrowedFd<'_>) -> Result<()> {
    use std::os::fd::AsRawFd;
    let tv = libc::timeval {
        tv_sec: IDLE.as_secs() as libc::time_t,
        tv_usec: 0,
    };
    // SAFETY: `tv` is a valid timeval for SO_RCVTIMEO and lives through the call.
    let rc = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            (&raw const tv).cast(),
            size_of::<libc::timeval>() as libc::socklen_t,
        )
    };
    if rc != 0 {
        bail!(
            "setting the idle timeout: {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "serve_tests.rs"]
mod tests;
