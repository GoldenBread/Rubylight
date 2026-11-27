//! ENet control socket adapter, roaming commit handling, unattended-session sweeps, and
//! client termination notices.

use super::super::*;
use super::peer::ControlPeer;

/// Expire controller sessions once a second (`last` is the last time). Input
/// never waits behind housekeeping: a held app is looked at on the next pass.
pub(in crate::stream) fn expire_controllers(h: &crate::state::Host, last: &mut Instant) {
    if last.elapsed() >= Duration::from_secs(1)
        && let Some(app_running) = h.try_current_app().map(|current| current.is_some())
    {
        h.controller_sessions.expire(app_running);
        *last = Instant::now();
    }
}

/// Cached control-loop settings read from `Host::config`, refreshed only when
/// `config_version` moves or during post-input housekeeping so input wakes never block
/// on `RwLock::read` before draining the UDP socket.
pub(in crate::stream) struct ControlLoopConfig {
    version: u64,
    pub(in crate::stream) ping_timeout: Duration,
    pub(in crate::stream) client_roaming: bool,
}

impl ControlLoopConfig {
    pub(in crate::stream) fn load(h: &Shared) -> Self {
        let version = h.config_version.load(Ordering::Acquire);
        let c = h.config.read().unwrap();
        Self {
            version,
            ping_timeout: crate::network::ping_timeout(&c),
            client_roaming: c.client_roaming(),
        }
    }

    /// Refreshes settings before `host.service()` only when `config_version` changed and
    /// the config lock is uncontended, keeping input wakes lock-free.
    pub(in crate::stream) fn refresh_on_version_change(&mut self, h: &Shared) -> bool {
        let version = h.config_version.load(Ordering::Acquire);
        if version == self.version {
            return false;
        }
        let Ok(c) = h.config.try_read() else {
            return false;
        };
        self.version = version;
        self.ping_timeout = crate::network::ping_timeout(&c);
        self.client_roaming = c.client_roaming();
        true
    }

    /// Non-blocking refresh run in post-input housekeeping so direct config writes without
    /// a `config_version` bump are picked up promptly without delaying input injection.
    pub(in crate::stream) fn refresh_housekeeping(&mut self, h: &Shared) -> bool {
        let Ok(c) = h.config.try_read() else {
            return false;
        };
        self.version = h.config_version.load(Ordering::Acquire);
        let ping_timeout = crate::network::ping_timeout(&c);
        let client_roaming = c.client_roaming();
        let changed = ping_timeout != self.ping_timeout || client_roaming != self.client_roaming;
        self.ping_timeout = ping_timeout;
        self.client_roaming = client_roaming;
        changed
    }
}

pub(in crate::stream) const CONTROL_EVENT_LIMIT: usize = 512;

pub(in crate::stream) fn control_wait_timeout(remaining: Duration, events: usize) -> i32 {
    if events == CONTROL_EVENT_LIMIT {
        0
    } else {
        remaining.as_micros().div_ceil(1000) as i32
    }
}

/// Whether the ENet control worker should see a peer for `session`. A WebRTC
/// session carries control and input on its DataChannels, so it never has one.
pub(in crate::stream) fn expects_enet_control(session: &Session) -> bool {
    session.launch.transport != butterpollo_session::session::Transport::WebRtc
}

/// How often the control worker looks for sessions without a control peer when no peer
/// came or went. The ping timeout it enforces is at least a second, and a worker
/// without peers already woke this often.
pub(in crate::stream) const UNATTENDED_SWEEP: Duration = Duration::from_millis(250);

/// How long a host shutdown waits for the clients it told to acknowledge the termination
/// before it ends their peers. A shutdown gets a second for this.
pub(super) const NOTICE_ACK_WAIT: Duration = Duration::from_millis(200);

/// When the control worker looks for sessions without a control peer: when a peer
/// came or went, and otherwise every [`UNATTENDED_SWEEP`], not on every input pass.
pub(in crate::stream) struct UnattendedSweep {
    at: Instant,
}
impl UnattendedSweep {
    /// The first pass sweeps.
    pub(in crate::stream) fn new(now: Instant) -> Self {
        Self { at: now }
    }
    /// Whether to sweep in the pass at `now`; when it does, the next timed sweep is
    /// [`UNATTENDED_SWEEP`] after it.
    pub(in crate::stream) fn due(&mut self, now: Instant, peers_changed: bool) -> bool {
        if !peers_changed && now < self.at {
            return false;
        }
        self.at = now + UNATTENDED_SWEEP;
        true
    }
    /// When the worker must wake for the next timed sweep.
    pub(in crate::stream) fn next(&self) -> Instant {
        self.at
    }
}

/// Ends the sessions that expect an ENet control peer, have had none for longer than
/// `ping_timeout`, and are not attended now (`attended` says by session id). Drains `sessions`.
/// The log names what `control` received from the client, if anything.
pub(in crate::stream) fn end_unattended_sessions(
    sessions: &mut Vec<Arc<Session>>,
    attended: impl Fn(&str) -> bool,
    unattended: &mut HashMap<String, Instant>,
    ping_timeout: Duration,
    control: &ReplySocket,
) {
    for s in sessions.drain(..) {
        let id = &s.launch.id;
        if s.stopping() || !expects_enet_control(&s) || attended(id) {
            unattended.remove(id);
            continue;
        }
        let since = match unattended.get(id) {
            Some(since) => *since,
            None => {
                let since = Instant::now();
                unattended.insert(id.clone(), since);
                since
            }
        };
        if since.elapsed() > ping_timeout {
            let received: Vec<_> = control
                .sources_from(s.client_ip())
                .iter()
                .map(|(peer, local)| format!("{peer} on {local}"))
                .collect();
            tracing::warn!(
                client = %s.launch.client.name,
                peer = %s.client_ip(),
                control_datagrams = if received.is_empty() { "none".into() } else { received.join(", ") },
                "session has no control connection; ending it"
            );
            s.fail("the client's control stream never connected");
            s.stop();
        }
    }
}

/// ENet's UDP adapter ends the control worker on any error but WouldBlock,
/// and nothing restarted it: a Wi-Fi roam (WSAENETUNREACH), an ICMP reply
/// (WSAENETRESET) or an oversized datagram (WSAEMSGSIZE) left every later
/// session without input or control. These lose one datagram; ENet resends
/// what was reliable.
pub(in crate::stream) struct ControlSocket {
    /// Answers each client from the address its datagrams arrived on, as
    /// moonlight-common-c's ENet does; shared so the worker can log it.
    pub(in crate::stream) socket: Arc<ReplySocket>,
    /// Whether datagrams are held back, and those held, in order: ENet
    /// sends a packet's acknowledgement before it returns the packet, and
    /// that sendto delayed every input by its cost.
    holding: bool,
    held: Vec<(SocketAddr, Vec<u8>)>,
    /// Buffers released from `held`, reused by the next pass instead of allocated.
    spare: Vec<Vec<u8>>,
    /// When this pass read its first datagram for ENet, for the input latency in
    /// `stream timings`; cleared by hold().
    pub(in crate::stream) first_received: Option<Instant>,
    /// Client roaming: what a moved client's datagrams map to.
    pub(in crate::stream) roaming: roaming::Roaming,
}

impl ControlSocket {
    pub(in crate::stream) fn new(socket: Arc<ReplySocket>) -> Self {
        Self {
            socket,
            holding: false,
            held: Vec::new(),
            spare: Vec::new(),
            first_received: None,
            roaming: Default::default(),
        }
    }
    /// Hold back what ENet sends until release(), and start a pass.
    pub(in crate::stream) fn hold(&mut self) {
        self.holding = true;
        self.first_received = None;
    }
    /// Send what was held, in order, and stop holding. A datagram that is
    /// lost is logged; any other error ends the worker, as it would have
    /// inside service().
    pub(in crate::stream) fn release(&mut self) -> std::io::Result<()> {
        self.holding = false;
        for (address, mut buffer) in self.held.drain(..) {
            let address = self.roaming.wire_of(address);
            if let Err(error) = self.socket.send_to(&buffer, address) {
                if !butterpollo_win_io::net::datagram_lost(&error) {
                    return Err(error);
                }
                tracing::debug!(%error, %address, "control datagram dropped");
            }
            if self.spare.len() < 64 {
                buffer.clear();
                self.spare.push(buffer);
            }
        }
        Ok(())
    }
}

impl rusty_enet::Socket for ControlSocket {
    type Address = SocketAddr;
    type Error = std::io::Error;
    fn init(&mut self, _options: rusty_enet::SocketOptions) -> std::io::Result<()> {
        // What ENet's own UdpSocket adapter sets.
        self.socket.set_nonblocking(true)?;
        self.socket.set_broadcast(true)
    }
    fn send(&mut self, address: SocketAddr, buffer: &[u8]) -> std::io::Result<usize> {
        if self.holding {
            let mut held = self.spare.pop().unwrap_or_default();
            held.clear();
            held.extend_from_slice(buffer);
            self.held.push((address, held));
            return Ok(buffer.len());
        }
        match self.socket.send_to(buffer, self.roaming.wire_of(address)) {
            Err(error) if butterpollo_win_io::net::datagram_lost(&error) => Ok(0),
            result => result,
        }
    }
    fn receive(
        &mut self,
        buffer: &mut [u8; rusty_enet::MTU_MAX],
    ) -> std::io::Result<Option<(SocketAddr, rusty_enet::PacketReceived)>> {
        // A flood of roaming datagrams, or of datagrams from an address a client left,
        // must not keep ENet's service from returning.
        for _ in 0..SWALLOWED_PER_RECEIVE {
            let (size, from, local) = match self.socket.receive(buffer) {
                Ok(received) => received,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(None),
                Err(error) if butterpollo_win_io::net::datagram_lost(&error) => continue,
                Err(error) => return Err(error),
            };
            if self.roaming.intercepts(&buffer[..size]) {
                self.roaming.on_datagram(
                    &self.socket,
                    Instant::now(),
                    from,
                    local,
                    &buffer[..size],
                );
                continue;
            }
            let peer = match self.roaming.peer_of(from) {
                Some(peer) => peer,
                None if self.roaming.reopens(from, &buffer[..size]) => from,
                None => {
                    self.roaming.dropped(Instant::now(), from);
                    continue;
                }
            };
            // ENet answers before it knows the session: learn from every datagram.
            self.socket.answer_from(from, local);
            self.first_received.get_or_insert_with(Instant::now);
            return Ok(Some((peer, rusty_enet::PacketReceived::Complete(size))));
        }
        Ok(None)
    }
}

/// Datagrams one `ControlSocket::receive` takes for roaming or drops before it returns
/// none; the rest wait for ENet's next service.
pub(in crate::stream) const SWALLOWED_PER_RECEIVE: usize = 64;

/// Tells `p`'s client that its stream ends: its forwarded USB devices detach first, then the
/// 0x0109 termination, and the session stops. A normal stop and the host's shutdown both go
/// through here. True when the termination was queued for it.
pub(super) fn tell_client_stopping(
    h: &Shared,
    host: &mut Host<ControlSocket>,
    peer_id: PeerID,
    p: &mut ControlPeer,
    s: &Arc<Session>,
) -> bool {
    host.socket_mut().roaming.set_stopping(&p.id);
    // Its forwarded USB devices detach, and the client hears so before the
    // termination.
    for usb in h.usb.end_session(&p.id) {
        let sent = usb
            .encode()
            .context("the USB message does not encode")
            .and_then(|payload| p.encrypt(s, rubylight_protocol::usb::USB_MESSAGE_TYPE, &payload))
            .and_then(|message| {
                host.peer_mut(peer_id)
                    .send(0, &Packet::new(message, PacketKind::Reliable))
                    .map_err(|e| anyhow::anyhow!("{e}"))
            });
        if let Err(e) = sent {
            tracing::warn!(error = %format!("{e:#}"), ?usb, "USB message not sent");
        }
    }
    // Before the client hears it: it asks `/serverinfo` for the reason then.
    h.record_failure(s);
    let reason = s.termination_reason();
    let told = if let Ok(message) = p.encrypt(s, 0x0109, &reason.to_be_bytes()) {
        let peer = host.peer_mut(peer_id);
        let sent = peer
            .send(0, &Packet::new(message, PacketKind::Reliable))
            .is_ok();
        peer.disconnect_later(0);
        sent
    } else {
        host.peer_mut(peer_id).disconnect_now(0);
        false
    };
    s.stop();
    told
}

/// Carries out a client's validated move (`roaming`): the session's media follow the
/// client to its new address, ENet checks the path at once, and the client hears MIGRATED.
pub(super) fn follow_roam(
    h: &Shared,
    media: &Media,
    host: &mut Host<ControlSocket>,
    peers: &HashMap<PeerID, ControlPeer>,
    socket: &ReplySocket,
    commit: roaming::Commit,
) {
    let mut stale = Vec::new();
    let session = {
        // The ping thread's lock order. Holding the sessions lock until the session knows
        // its new path keeps a new-path ping from landing between the two: its peer would be
        // dropped below after it took the new path, and with it the keyframe.
        let sessions = h.sessions.lock().unwrap();
        let session = sessions.active.get(&commit.id).cloned();
        if let Some(s) = &session {
            // The old path's media addresses are stale; the first pings from the new path
            // replace them.
            stale.extend(media.peers.lock().unwrap().remove(&commit.id));
            s.roamed(commit.new.ip(), Instant::now());
        }
        session
    };
    for (audio, peer) in stale {
        if audio {
            media.audio.forget(peer);
        } else {
            media.video.forget(peer);
        }
    }
    if let Some((peer_id, _)) = peers.iter().find(|(_, p)| p.id == commit.id) {
        host.peer_mut(*peer_id).ping();
    }
    roaming::send(socket, &commit.migrated, commit.new);
    tracing::info!(
        session = %commit.id,
        client = session.as_ref().map_or("", |s| s.launch.client.name.as_str()),
        media_rebound = commit.media_rebound,
        "client roamed {} -> {}",
        roaming::canonical(commit.old),
        roaming::canonical(commit.new)
    );
}
