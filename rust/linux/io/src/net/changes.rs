//! Notice of this PC's interfaces or addresses changing on Linux, from the kernel's routing
//! socket.
use super::netlink;
use std::{
    os::{fd::AsRawFd, unix::net::UnixStream},
    time::{Duration, Instant},
};

/// Linux telling this PC's IP interfaces or addresses changed: an interface came or went, its
/// state moved, or an address was added or removed. A watcher re-reads the interfaces on
/// a change instead of on a short timer, so the reading runs only when there was one.
pub struct InterfaceChanges {
    /// Closing this end wakes the worker, which then ends.
    stop: Option<UnixStream>,
    worker: Option<std::thread::JoinHandle<()>>,
}
/// How long a change holds the window open for the ones that follow it: whatever the watcher
/// did about the first has happened by the end of it, and later changes within it are covered.
const QUIET: Duration = Duration::from_secs(2);
impl InterfaceChanges {
    /// Call `changed` when Linux reports an interface change, and `quiet` once none followed
    /// within a short window. `None` when the kernel refuses the notification: the caller then
    /// falls back to its own timer.
    pub fn register(
        changed: impl Fn() + Send + 'static,
        quiet: impl Fn() + Send + 'static,
    ) -> Option<Self> {
        Self::register_with(false, changed, quiet)
    }
    /// The registration for the test, which also has the kernel list every interface there is
    /// now, so a notification arrives without an interface actually changing.
    #[cfg(test)]
    pub(super) fn register_initial(
        changed: impl Fn() + Send + 'static,
        quiet: impl Fn() + Send + 'static,
    ) -> Option<Self> {
        Self::register_with(true, changed, quiet)
    }
    fn register_with(
        initial: bool,
        changed: impl Fn() + Send + 'static,
        quiet: impl Fn() + Send + 'static,
    ) -> Option<Self> {
        let socket = netlink::changes(initial).ok()?;
        let (stop, stopped) = UnixStream::pair().ok()?;
        let worker = std::thread::Builder::new()
            .name("interface-watch".into())
            .spawn(move || watch_interfaces(&socket, &stopped, changed, quiet))
            .ok()?;
        Some(Self {
            stop: Some(stop),
            worker: Some(worker),
        })
    }
}
/// The worker: hand each change to `changed`, then wait out the window; when it closes with no
/// further change, `quiet` ends what `changed` began. It ends when `stopped`'s other end
/// closes, or when the routing socket breaks and can report nothing more.
fn watch_interfaces(
    socket: &socket2::Socket,
    stopped: &UnixStream,
    changed: impl Fn(),
    quiet: impl Fn(),
) {
    let mut window: Option<Instant> = None;
    loop {
        let timeout = window.map_or(-1, |end| {
            let left = end.saturating_duration_since(Instant::now());
            left.as_millis().min(i32::MAX as u128) as i32 + 1
        });
        let mut poll = [socket.as_raw_fd(), stopped.as_raw_fd()].map(|fd| libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        });
        // SAFETY: `poll` holds two entries and outlives the call, for descriptors the borrows
        // keep open.
        let ready = unsafe { libc::poll(poll.as_mut_ptr(), 2, timeout) };
        if ready < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            tracing::warn!(
                error = %std::io::Error::last_os_error(),
                "interface change notifications ended"
            );
            return;
        }
        if poll[1].revents != 0 {
            return;
        }
        if poll[0].revents != 0 {
            let working = netlink::drain(socket);
            // A change within an open window is covered by what the first one began.
            if window.is_none() {
                changed();
            }
            window = Some(Instant::now() + QUIET);
            if !working {
                tracing::warn!("interface change notifications ended: the routing socket closed");
                // What `changed` began still ends; nothing follows it.
                quiet();
                return;
            }
            continue;
        }
        if window.is_some_and(|end| Instant::now() >= end) {
            window = None;
            quiet();
        }
    }
}
impl Drop for InterfaceChanges {
    fn drop(&mut self) {
        // The worker sees its end of the pair hang up and returns.
        drop(self.stop.take());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
