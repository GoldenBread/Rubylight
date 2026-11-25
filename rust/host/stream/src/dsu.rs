//! The DSU (cemuhook) motion server (`enable_dsu_server`, off by default):
//! emulators on the PC read each streamed controller's gyroscope,
//! accelerometer, buttons and touch from UDP `dsu_server_port` (26760). The
//! packets and the client registrations are `butterpollo_input_policy::dsu`'s; the
//! setting names and port are Foundation Sunshine's
//! (AlkaidLab/foundation-sunshine, GPL-3.0). Unlike its server this one
//! listens only on `127.0.0.1` unless `dsu_server_lan` asks for an emulator
//! on another network device.
//!
//! The control thread only updates a slot and wakes the sender; nothing is
//! sent from it, and with the setting off [`observe`] returns at once. The
//! client registrations have a lock of their own, so a flood of spoofed
//! requests cannot stall the input path.
use butterpollo_input_policy::{
    dsu::{self, Clients, Request, SLOTS, Slots},
    input::Input,
};
use butterpollo_session::session::Warnings;
use std::io::ErrorKind;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Console warning code when the port cannot be opened.
pub const WARNING: &str = "dsu_server_port";
/// How long the receiver waits for a packet before it drops stale clients.
const RECEIVE_TIMEOUT: Duration = Duration::from_millis(500);

static SERVER: OnceLock<Arc<Server>> = OnceLock::new();

struct Server {
    socket: UdpSocket,
    /// Random, as every DSU server's: a client sees a restarted server as new.
    id: u32,
    started: Instant,
    /// The slots and their changes, the only state the input path takes.
    state: Mutex<State>,
    /// The clients that asked for pad data: never touched by [`observe`].
    clients: Mutex<Clients>,
    wake: Condvar,
}

#[derive(Default)]
struct State {
    slots: Slots,
    /// Slots whose pad data changed since the sender last ran.
    dirty: [bool; SLOTS],
}

/// The address the server listens on: this PC only unless the user asks for
/// emulators on the network (`dsu_server_lan`).
fn bind_address(port: u16, lan: bool) -> SocketAddr {
    let ip = if lan {
        Ipv4Addr::UNSPECIFIED
    } else {
        Ipv4Addr::LOCALHOST
    };
    (ip, port).into()
}

/// Starts the server when `port` is set (`Config::dsu_server_port`). A port
/// that cannot be opened is a console warning; the stream works without it.
static RUNTIME_WARNING: Mutex<Option<String>> = Mutex::new(None);

pub(crate) fn runtime_warning() -> Option<String> {
    RUNTIME_WARNING.lock().unwrap().clone()
}

fn set_runtime_warning(message: Option<String>) {
    *RUNTIME_WARNING.lock().unwrap() = message;
}

pub fn start(port: Option<u16>, lan: bool, warnings: &Warnings) {
    set_runtime_warning(None);
    let Some(port) = port else { return };
    let address = bind_address(port, lan);
    let socket = match UdpSocket::bind(address) {
        Ok(socket) => socket,
        Err(error) => {
            tracing::warn!(port, %error, "DSU motion server unavailable");
            warnings.set(
                WARNING,
                format!(
                    "Emulators can't read controller motion: UDP port {port} could not be opened ({error}). Choose another DSU server port or close the program using it, then restart the host."
                ),
            );
            return;
        }
    };
    let receiver = socket.try_clone().and_then(|receiver| {
        receiver.set_read_timeout(Some(RECEIVE_TIMEOUT))?;
        Ok(receiver)
    });
    let local = socket.local_addr().map(|local| local.to_string());
    let receiver = match receiver {
        Ok(receiver) => receiver,
        Err(error) => {
            tracing::warn!(port, %error, "DSU motion server unavailable");
            warnings.set(WARNING, format!("Emulators can't read controller motion: the DSU server could not start ({error})."));
            return;
        }
    };
    warnings.clear(WARNING);
    let server = Arc::new(Server {
        socket,
        id: rand::random(),
        started: Instant::now(),
        state: Mutex::new(State::default()),
        clients: Mutex::new(Clients::default()),
        wake: Condvar::new(),
    });
    if SERVER.set(server.clone()).is_err() {
        return;
    }
    tracing::info!(
        port,
        lan,
        address = local.as_deref().unwrap_or("unknown"),
        "DSU motion server listening"
    );
    let receiving = server.clone();
    let spawned = std::thread::Builder::new()
        .name("dsu-receive".into())
        .spawn(move || receiving.receive(&receiver))
        .and_then(|_| {
            std::thread::Builder::new()
                .name("dsu-send".into())
                .spawn(move || server.send())
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "DSU motion server threads could not start");
        warnings.set(
            WARNING,
            format!(
                "Emulators can't read controller motion: the DSU server could not start ({error})."
            ),
        );
    }
}

/// A streamed controller's input from `session`: updates its slot.
pub fn observe(session: &str, input: &Input) {
    let Some(server) = SERVER.get() else { return };
    if !matches!(
        input,
        Input::Controller { .. }
            | Input::Arrival { .. }
            | Input::ControllerTouch { .. }
            | Input::Motion { .. }
            | Input::Battery { .. }
    ) {
        return;
    }
    let now = server.started.elapsed().as_micros() as u64;
    let mut state = server.state.lock().unwrap();
    if let Some(slot) = state.slots.apply(session, input, now) {
        state.dirty[usize::from(slot)] = true;
        server.wake.notify_one();
    }
}

/// A session ended: its controllers leave their slots.
pub fn forget(session: &str) {
    let Some(server) = SERVER.get() else { return };
    let mut state = server.state.lock().unwrap();
    for slot in state.slots.forget(session) {
        state.dirty[usize::from(slot)] = true;
    }
    server.wake.notify_one();
}

impl Server {
    /// Answers requests and records subscriptions.
    fn receive(&self, socket: &UdpSocket) {
        self.receive_with_warnings(socket, None);
    }

    fn receive_with_warnings(&self, socket: &UdpSocket, warnings: Option<&Warnings>) {
        let mut buffer = [0u8; 1024];
        let mut oversized_warn = crate::network::Throttle::new();
        while self.receive_one(socket, &mut buffer, warnings, &mut oversized_warn) {}
    }

    fn receive_one(
        &self,
        socket: &UdpSocket,
        buffer: &mut [u8],
        warnings: Option<&Warnings>,
        oversized_warn: &mut crate::network::Throttle,
    ) -> bool {
        let (len, from) = match socket.recv_from(buffer) {
            Ok(received) => received,
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) =>
            {
                self.clients.lock().unwrap().prune(Instant::now());
                return true;
            }
            // Windows reports an ICMP reset or oversized UDP datagram (`WSAEMSGSIZE`, 10040)
            // on receive; keep serving other requests instead of tearing down the DSU server.
            Err(error) if butterpollo_win_io::net::datagram_lost(&error) => {
                if error.raw_os_error() == Some(10040)
                    && let Some(skipped) = oversized_warn.allow()
                {
                    tracing::warn!(
                        %error,
                        skipped,
                        "DSU motion server dropped an oversized datagram"
                    );
                }
                return true;
            }
            Err(error) => {
                tracing::warn!(%error, "DSU motion server stopped receiving");
                let message = format!(
                    "Emulators can't read controller motion: the DSU server stopped ({error})."
                );
                if let Some(warnings) = warnings {
                    warnings.set(WARNING, message);
                } else {
                    set_runtime_warning(Some(message));
                }
                return false;
            }
        };
        let Some((_, request)) = dsu::parse(&buffer[..len]) else {
            return true;
        };
        match request {
            Request::Version => self.reply(&dsu::version_reply(self.id), from),
            Request::Ports { count, slots } => {
                let replies: Vec<_> = {
                    let state = self.state.lock().unwrap();
                    slots[..usize::from(count)]
                        .iter()
                        .map(|&slot| dsu::port_info(self.id, slot, state.slots.pad(slot)))
                        .collect()
                };
                for reply in replies {
                    self.reply(&reply, from);
                }
            }
            Request::PadData { flags, slot, mac } => {
                // The clients have their own lock: registering never
                // delays [`observe`] on the control thread.
                self.clients
                    .lock()
                    .unwrap()
                    .register(from, flags, slot, mac, Instant::now());
                // A new subscriber gets the current state now, not at the
                // controller's next change.
                let mut state = self.state.lock().unwrap();
                for slot in 0..SLOTS {
                    state.dirty[slot] |= state.slots.pad(slot as u8).is_some();
                }
                self.wake.notify_one();
            }
        }
        true
    }

    /// Sends the slots that changed to the clients that want them. The
    /// changed slots are taken under the input path's lock and the clients
    /// are read after it is released: walking the client map never holds up
    /// a controller packet.
    fn send(&self) {
        let mut state = self.state.lock().unwrap();
        loop {
            while !state.dirty.contains(&true) {
                state = self.wake.wait(state).unwrap();
            }
            let now = Instant::now();
            let mut changed = Vec::new();
            for slot in 0..SLOTS as u8 {
                if !std::mem::take(&mut state.dirty[usize::from(slot)]) {
                    continue;
                }
                let pad = state.slots.pad(slot).cloned();
                changed.push((slot, pad, state.slots.next_number(slot)));
            }
            drop(state);
            let packets = build_packets(self.id, &self.clients, changed, now);
            for (packet, subscribers) in &packets {
                for to in subscribers {
                    self.reply(packet, *to);
                }
            }
            state = self.state.lock().unwrap();
        }
    }

    fn reply(&self, packet: &[u8], to: SocketAddr) {
        if let Err(error) = self.socket.send_to(packet, to) {
            tracing::debug!(%error, %to, "DSU packet not sent");
        }
    }
}

fn build_packets(
    server_id: u32,
    clients: &Mutex<Clients>,
    changed: Vec<(u8, Option<dsu::Pad>, u32)>,
    now: Instant,
) -> Vec<(Vec<u8>, Vec<SocketAddr>)> {
    let mut packets = Vec::new();
    if changed.is_empty() {
        return packets;
    }
    let mut clients = clients.lock().unwrap();
    for (slot, pad, number) in changed {
        let subscribers = clients.subscribers(slot, now);
        if !subscribers.is_empty() {
            let packet = match pad {
                Some(pad) => dsu::pad_data(server_id, slot, &pad, number),
                None => dsu::pad_data_disconnected(server_id, slot, number),
            };
            packets.push((packet, subscribers));
        }
    }
    packets
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The motion server is a passwordless input feed: an emulator on this PC
    /// reaches it at 127.0.0.1, and only `dsu_server_lan` exposes it.
    #[test]
    fn the_motion_server_listens_on_loopback_unless_lan_access_is_on() {
        let local = bind_address(26760, false);
        assert!(local.ip().is_loopback());
        assert_eq!(local.port(), 26760);
        let lan = bind_address(26760, true);
        assert!(lan.ip().is_unspecified());
        assert_eq!(lan.port(), 26760);
    }

    #[test]
    fn disconnect_and_session_forget_emit_disconnected_neutral_pad_data() {
        let mut state = State::default();
        let clients = Mutex::new(Clients::default());
        let sub: SocketAddr = "127.0.0.1:41234".parse().unwrap();
        let now = Instant::now();
        clients.lock().unwrap().register(sub, 0, 0, [0; 6], now);

        // Connect controller 0 with held A button and stick deflection.
        let slot = state
            .slots
            .apply(
                "s1",
                &Input::Controller {
                    id: 0,
                    active: 1,
                    buttons: dsu::button::A,
                    left_trigger: 200,
                    right_trigger: 100,
                    sticks: [20_000, -20_000, 0, 0],
                },
                100,
            )
            .unwrap();
        assert_eq!(slot, 0);

        // Disconnect controller via active = 0: must emit disconnected neutral MSG_PAD_DATA.
        let cleared = state
            .slots
            .apply(
                "s1",
                &Input::Controller {
                    id: 0,
                    active: 0,
                    buttons: 0,
                    left_trigger: 0,
                    right_trigger: 0,
                    sticks: [0; 4],
                },
                200,
            )
            .unwrap();
        assert_eq!(cleared, 0);
        let pad = state.slots.pad(0).cloned();
        assert!(pad.is_none());
        let packets = build_packets(
            42,
            &clients,
            vec![(0, pad, state.slots.next_number(0))],
            now,
        );
        assert_eq!(packets.len(), 1);
        let (pkt, recipients) = &packets[0];
        assert_eq!(recipients, &[sub]);
        assert_eq!(pkt.len(), 100);
        assert_eq!(pkt[21], 0, "slot state must be 0 (not connected)");
        assert_eq!(pkt[31], 0, "connected byte must be 0");
        assert_eq!(pkt[36..40], [0, 0, 0, 0], "buttons cleared");
        assert_eq!(pkt[40..44], [128, 128, 128, 128], "sticks centered");

        // Reconnect and end session via `forget`: also emits disconnected neutral MSG_PAD_DATA.
        state
            .slots
            .apply(
                "s2",
                &Input::Controller {
                    id: 0,
                    active: 1,
                    buttons: dsu::button::B,
                    left_trigger: 255,
                    right_trigger: 0,
                    sticks: [0; 4],
                },
                300,
            )
            .unwrap();
        let freed = state.slots.forget("s2");
        assert_eq!(freed, vec![0]);
        let packets = build_packets(
            42,
            &clients,
            vec![(0, state.slots.pad(0).cloned(), state.slots.next_number(0))],
            now,
        );
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].0[21], 0);
        assert_eq!(packets[0].0[31], 0);
    }

    #[test]
    fn oversized_datagram_is_classified_as_datagram_lost_and_keeps_serving() {
        let server_sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = server_sock.local_addr().unwrap();
        let client_sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        client_sock
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        server_sock
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();

        let server = Arc::new(Server {
            socket: server_sock.try_clone().unwrap(),
            id: 99,
            started: Instant::now(),
            state: Mutex::default(),
            clients: Mutex::default(),
            wake: std::sync::Condvar::new(),
        });
        let warnings = Warnings::default();
        let mut buf = [0u8; 1024];
        let mut throttle = crate::network::Throttle::new();

        // 1. Send an oversized UDP payload (> 1024-byte recv buffer) to trigger WSAEMSGSIZE (10040) on Windows.
        let oversized = vec![0u8; 2048];
        client_sock.send_to(&oversized, addr).unwrap();
        assert!(
            server.receive_one(&server_sock, &mut buf, Some(&warnings), &mut throttle),
            "oversized datagram must not stop DSU server"
        );
        assert!(warnings.snapshot().is_empty());

        // 2. Immediately send a valid DSU version request (`DSUC`); the server must still be alive and answer (`DSUS`).
        let mut req = [0u8; 20];
        req[0..4].copy_from_slice(b"DSUC");
        req[4..6].copy_from_slice(&1001u16.to_le_bytes());
        req[6..8].copy_from_slice(&4u16.to_le_bytes());
        req[16..20].copy_from_slice(&dsu::MSG_VERSION.to_le_bytes());
        let crc = crc32(&req);
        req[8..12].copy_from_slice(&crc.to_le_bytes());
        client_sock.send_to(&req, addr).unwrap();
        assert!(server.receive_one(&server_sock, &mut buf, Some(&warnings), &mut throttle));

        let mut reply = [0u8; 64];
        let (len, _) = client_sock.recv_from(&mut reply).unwrap();
        assert!(len >= 20, "expected DSU version reply, got {len} bytes");
        assert_eq!(&reply[0..4], b"DSUS");

        // 3. Fatal receive error (`WSAEINVAL` on an unbound socket) must stop the loop and record the console warning.
        let raw = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )
        .unwrap();
        let unbound: UdpSocket = raw.into();
        let fatal_warnings = Warnings::default();
        assert!(!server.receive_one(&unbound, &mut buf, Some(&fatal_warnings), &mut throttle));
        let snap = fatal_warnings.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].code, WARNING);
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xedb8_8320 & mask);
            }
        }
        !crc
    }
}
