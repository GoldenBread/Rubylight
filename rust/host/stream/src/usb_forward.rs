//! USB forwarding: a device plugged into the client, attached to this host through
//! usbip-win2's virtual host controller.
//!
//! The client asks on the control stream (0x553A). The host checks the setting, the device's
//! `allow_usb`, the driver and the device limit, and answers with a token that opens the native
//! API's USB tunnel once, within 30 s, from the certificate that asked. Over the tunnel the
//! client serves the device as a USB/IP server. The host relays it to the driver on a loopback
//! port, checking the framing on the way, so a client can't hand the kernel driver malformed
//! USB/IP. An attachment ends when the client or the console detaches it, when either end of
//! the relay closes, and with its stream.

use anyhow::{Context, Result};
use butterpollo_session::state::Client;
use rubylight_native_api::usb::TOKEN_TTL_SECS;
use rubylight_protocol::{
    usb::{TOKEN_BYTES, UsbMessage, UsbReason, UsbState},
    usbip as wire,
};
use std::{
    net::SocketAddr,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Notify;

mod relay;

/// How long a driver install check holds: it reads the registry and the disk, and the control
/// thread asks on every attach request.
const READY_FOR: Duration = Duration::from_secs(10);

/// The USB/IP driver; tests swap in a fake.
pub trait Vhci: Send + Sync {
    /// Whether the driver is installed.
    fn ready(&self) -> bool;
    /// Whether the connection from `peer` to the relay listening at `relay` is the driver's:
    /// any process on the PC can reach a loopback port.
    fn is_driver(&self, peer: SocketAddr, relay: SocketAddr) -> Result<bool>;
    /// Attaches `busid` from the USB/IP server on 127.0.0.1:`tcp_port` and returns the hub
    /// port. Blocks until the driver imported the device or failed.
    fn attach(&self, tcp_port: u16, busid: &str) -> Result<u32>;
    /// Unplugs hub `port` and stops the driver attaching the device again.
    fn detach(&self, port: u32, tcp_port: u16, busid: &str) -> Result<()>;
    /// Stops the driver attaching the device again when its hub port is unknown.
    fn stop(&self, tcp_port: u16, busid: &str) -> Result<()>;
}

/// usbip-win2, looked up on every use: installing it needs no host restart.
struct Usbip;
impl Vhci for Usbip {
    fn ready(&self) -> bool {
        butterpollo_win_io::input::UsbipDriver::find().is_some()
    }
    fn is_driver(&self, peer: SocketAddr, relay: SocketAddr) -> Result<bool> {
        butterpollo_win_io::input::UsbipDriver::connected(peer, relay)
    }
    fn attach(&self, tcp_port: u16, busid: &str) -> Result<u32> {
        usbip()?.attach(tcp_port, busid)
    }
    fn detach(&self, port: u32, tcp_port: u16, busid: &str) -> Result<()> {
        usbip()?.detach(port, tcp_port, busid)
    }
    fn stop(&self, tcp_port: u16, busid: &str) -> Result<()> {
        usbip()?.stop(tcp_port, busid)
    }
}

/// Unplugs hub `port`, or stops the driver attaching the device again when no port is known.
fn unplug(vhci: &dyn Vhci, port: Option<u32>, tcp_port: u16, busid: &str) -> Result<()> {
    match port {
        Some(port) => vhci.detach(port, tcp_port, busid),
        None => vhci.stop(tcp_port, busid),
    }
}
fn usbip() -> Result<butterpollo_win_io::input::UsbipDriver> {
    butterpollo_win_io::input::UsbipDriver::find().context("usbip-win2 is not installed")
}

/// The devices clients forward to this host.
pub struct UsbForwarding {
    vhci: Mutex<Arc<dyn Vhci>>,
    /// The last driver install check: when it ran and what it found.
    checked: Mutex<Option<(Instant, bool)>>,
    registry: Mutex<Registry>,
    /// Whether a message may wait for some session's client, read without the
    /// lock: the control loop takes the lock only when this says so. Set under
    /// the registry lock wherever a message is queued, and cleared under it by
    /// `take` when nothing waits for anyone, so a message cannot land behind
    /// the clear.
    queued: AtomicBool,
}
impl Default for UsbForwarding {
    fn default() -> Self {
        Self {
            vhci: Mutex::new(Arc::new(Usbip)),
            checked: Mutex::default(),
            registry: Mutex::default(),
            queued: AtomicBool::new(false),
        }
    }
}

#[derive(Default)]
struct Registry {
    next: u64,
    attachments: Vec<Attachment>,
    /// Messages for each session's client, which its control loop sends.
    outbox: std::collections::HashMap<String, Vec<UsbMessage>>,
}
impl Registry {
    fn queue(&mut self, session: &str, messages: impl IntoIterator<Item = UsbMessage>) {
        let mut messages = messages.into_iter().peekable();
        if messages.peek().is_some() {
            self.outbox
                .entry(session.to_owned())
                .or_default()
                .extend(messages);
        }
    }
    fn index(&self, id: u64) -> Option<usize> {
        self.attachments.iter().position(|a| a.id == id)
    }
    /// Ends attachment `id`: at once while it waits for its tunnel, else through its relay.
    /// `notice` goes to the client once it is detached.
    fn end(&mut self, id: u64, notice: Vec<UsbMessage>) -> bool {
        let Some(index) = self.index(id) else {
            return false;
        };
        match self.attachments[index].stage {
            Stage::Pending { .. } => {
                let attachment = self.attachments.remove(index);
                tracing::info!(device = %attachment.device.name, client = %attachment.client, "USB attachment withdrawn before its tunnel opened");
                self.queue(&attachment.session, notice);
            }
            // Its relay is gone; only the console's retry detaches it.
            Stage::Stuck { .. } => {}
            Stage::Opening | Stage::Attached { .. } => {
                let attachment = &mut self.attachments[index];
                if attachment.ending.is_none() {
                    attachment.ending = Some(notice);
                    attachment.stop.notify_one();
                }
            }
        }
        true
    }
}

struct Attachment {
    id: u64,
    session: String,
    /// The certificate of the device that asked: only it opens the tunnel.
    cert: String,
    client: String,
    request_id: u32,
    device: Requested,
    token: [u8; TOKEN_BYTES],
    stage: Stage,
    since: SystemTime,
    /// Wakes the relay to end the attachment.
    stop: Arc<Notify>,
    /// Set once something ended it: what to tell the client when the relay is done.
    ending: Option<Vec<UsbMessage>>,
    /// Why the driver kept it, for the console.
    error: Option<String>,
}

/// The device as the client described it.
#[derive(Clone, Debug)]
struct Requested {
    vid: u16,
    pid: u16,
    class: u8,
    speed: u8,
    busid: String,
    name: String,
}

#[derive(Clone, Copy)]
enum Stage {
    Pending {
        expires: Instant,
    },
    Opening,
    Attached {
        port: u32,
    },
    /// The relay ended but the driver kept the device; the console retries the detach. It
    /// counts against the limit: it holds a hub port.
    Stuck {
        port: Option<u32>,
        tcp_port: u16,
    },
}

/// An attachment as the console lists it.
#[derive(Clone, Debug, serde::Serialize, PartialEq, Eq)]
pub struct Listed {
    pub id: u64,
    pub client: String,
    pub name: String,
    pub vid: u16,
    pub pid: u16,
    pub class: u8,
    pub speed: u8,
    pub busid: String,
    /// `waiting` for the tunnel, `opening`, `attached`, `detaching`, or `detach_failed` when the
    /// driver kept it (detaching it again retries).
    pub state: &'static str,
    /// The driver's hub port once attached.
    pub port: Option<u32>,
    pub since_unix_ms: u64,
    /// Why the detach failed.
    pub error: Option<String>,
}

/// Why a tunnel token opens nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// Unknown, expired, or another device's.
    Unknown,
    /// Already used.
    Used,
}

/// A claimed token: the attachment whose relay the tunnel carries.
#[derive(Debug)]
pub struct Opened {
    id: u64,
    request_id: u32,
    device: Requested,
    client: String,
    stop: Arc<Notify>,
}

fn status(request_id: u32, state: UsbState, reason: UsbReason) -> UsbMessage {
    UsbMessage::Status {
        request_id,
        state,
        reason,
    }
}

/// Compares tokens in constant time.
fn same(a: &[u8; TOKEN_BYTES], b: &[u8; TOKEN_BYTES]) -> bool {
    a.iter().zip(b).fold(0, |differ, (a, b)| differ | (a ^ b)) == 0
}

impl UsbForwarding {
    fn registry(&self) -> MutexGuard<'_, Registry> {
        self.registry.lock().unwrap_or_else(PoisonError::into_inner)
    }
    /// Marks a message as waiting, with release, under the registry lock: a
    /// take that empties the outbox clears it under the same lock, so nothing
    /// queued can hide behind the clear.
    fn mark_queued(&self) {
        self.queued.store(true, Ordering::Release);
    }
    /// Whether the control loop should look for a message.
    pub fn queued(&self) -> bool {
        self.queued.load(Ordering::Acquire)
    }
    fn vhci(&self) -> Arc<dyn Vhci> {
        self.vhci
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_vhci(&self, vhci: Arc<dyn Vhci>) {
        *self.vhci.lock().unwrap() = vhci;
        *self.checked.lock().unwrap() = None;
    }

    /// Whether the USB/IP driver is installed, checked at most every [`READY_FOR`].
    pub fn ready(&self) -> bool {
        let mut checked = self.checked.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        match *checked {
            Some((at, ready)) if now.duration_since(at) < READY_FOR => ready,
            _ => {
                let ready = self.vhci().ready();
                *checked = Some((now, ready));
                ready
            }
        }
    }

    /// A 0x553A message from the client of `session`; `enabled` and `limit` are the host's
    /// `usb_forwarding` and `usb_max_devices`.
    pub fn on_message(
        &self,
        session: &str,
        client: &Client,
        enabled: bool,
        limit: u8,
        message: UsbMessage,
    ) {
        match message {
            UsbMessage::AttachRequest {
                request_id,
                vid,
                pid,
                class,
                speed,
                busid,
                name,
            } => {
                let refusal = if !enabled {
                    Some(UsbReason::Disabled)
                } else if !client.allows_usb() {
                    Some(UsbReason::Permission)
                } else if !wire::valid_busid(&busid) {
                    Some(UsbReason::DeviceError)
                } else if !self.ready() {
                    Some(UsbReason::Driver)
                } else {
                    None
                };
                let mut registry = self.registry();
                if registry
                    .attachments
                    .iter()
                    .any(|a| a.session == session && a.request_id == request_id)
                {
                    tracing::warn!(client = %client.name, request_id, "USB attach request repeats a request id; ignored");
                    return;
                }
                let refusal = refusal.or_else(|| {
                    (registry.attachments.len() >= usize::from(limit)).then_some(UsbReason::Limit)
                });
                if let Some(reason) = refusal {
                    tracing::warn!(client = %client.name, device = %name, vid = format_args!("{vid:04x}"), pid = format_args!("{pid:04x}"), ?reason, "USB attach refused");
                    registry.queue(session, [status(request_id, UsbState::Refused, reason)]);
                    self.mark_queued();
                    return;
                }
                tracing::info!(client = %client.name, device = %name, vid = format_args!("{vid:04x}"), pid = format_args!("{pid:04x}"), %busid, "USB attach requested");
                let token: [u8; TOKEN_BYTES] = rand::random();
                registry.next += 1;
                let id = registry.next;
                registry.attachments.push(Attachment {
                    id,
                    session: session.to_owned(),
                    cert: client.cert.clone(),
                    client: client.name.clone(),
                    request_id,
                    device: Requested {
                        vid,
                        pid,
                        class,
                        speed,
                        busid,
                        name,
                    },
                    token,
                    stage: Stage::Pending {
                        expires: Instant::now() + Duration::from_secs(TOKEN_TTL_SECS),
                    },
                    since: SystemTime::now(),
                    stop: Arc::default(),
                    ending: None,
                    error: None,
                });
                registry.queue(session, [UsbMessage::TunnelReady { request_id, token }]);
                self.mark_queued();
            }
            UsbMessage::Detach { request_id } => {
                let mut registry = self.registry();
                let found = registry
                    .attachments
                    .iter()
                    .find(|a| a.session == session && a.request_id == request_id)
                    .map(|a| a.id);
                match found {
                    Some(id) => {
                        tracing::info!(client = %client.name, request_id, "USB detach requested by the client");
                        registry.end(
                            id,
                            vec![status(request_id, UsbState::Detached, UsbReason::None)],
                        );
                        self.mark_queued();
                    }
                    None => tracing::debug!(request_id, "USB detach for no attachment"),
                }
            }
            other => tracing::debug!(?other, "USB message only a host sends"),
        }
    }

    /// What to send the client of `session` now. Reports tokens that ran out first.
    pub fn take(&self, session: &str, now: Instant) -> Vec<UsbMessage> {
        let mut registry = self.registry();
        if registry.attachments.is_empty() && registry.outbox.is_empty() {
            self.queued.store(false, Ordering::Release);
            return Vec::new();
        }
        let mut expired = Vec::new();
        registry.attachments.retain(|a| {
            let gone = a.session == session
                && matches!(a.stage, Stage::Pending { expires } if expires <= now);
            if gone {
                tracing::warn!(client = %a.client, device = %a.device.name, "USB tunnel not opened in time");
                expired.push(status(
                    a.request_id,
                    UsbState::Failed,
                    UsbReason::TunnelTimeout,
                ));
            }
            !gone
        });
        registry.queue(session, expired);
        let messages = registry.outbox.remove(session).unwrap_or_default();
        // Nothing waits for anyone once every outbox is empty and no tunnel is
        // still to open; a token that is still pending would have to be
        // reported when it runs out.
        if registry.outbox.is_empty()
            && !registry
                .attachments
                .iter()
                .any(|a| matches!(a.stage, Stage::Pending { .. }))
        {
            self.queued.store(false, Ordering::Release);
        }
        messages
    }

    /// `session` ended or disconnected: detaches everything it attached and returns what to
    /// tell its client, if it is still there.
    pub fn end_session(&self, session: &str) -> Vec<UsbMessage> {
        let mut registry = self.registry();
        let mut messages = registry.outbox.remove(session).unwrap_or_default();
        registry.attachments.retain_mut(|a| {
            // A device the driver kept already told its client; the console retries it.
            if a.session != session || matches!(a.stage, Stage::Stuck { .. }) {
                return true;
            }
            tracing::info!(client = %a.client, device = %a.device.name, "USB device detached: the stream ended");
            match a.ending.replace(Vec::new()) {
                Some(notice) => messages.extend(notice),
                None => messages.extend([
                    UsbMessage::Detach {
                        request_id: a.request_id,
                    },
                    status(a.request_id, UsbState::Detached, UsbReason::SessionEnd),
                ]),
            }
            if matches!(a.stage, Stage::Pending { .. }) {
                return false;
            }
            a.stop.notify_one();
            true
        });
        messages
    }

    /// USB forwarding was turned off: ends every attachment now, as a console detach does,
    /// rather than when its stream ends. A device the driver kept stays for the console's
    /// retry. Returns how many it ended.
    pub fn disable(&self) -> usize {
        let mut registry = self.registry();
        let ending: Vec<_> = registry
            .attachments
            .iter()
            .filter(|a| !matches!(a.stage, Stage::Stuck { .. }))
            .map(|a| (a.id, a.request_id))
            .collect();
        for &(id, request_id) in &ending {
            registry.end(
                id,
                vec![
                    UsbMessage::Detach { request_id },
                    status(request_id, UsbState::Detached, UsbReason::Disabled),
                ],
            );
        }
        if !ending.is_empty() {
            tracing::info!(
                devices = ending.len(),
                "USB forwarding turned off; detaching every device"
            );
            self.mark_queued();
        }
        ending.len()
    }

    /// The console's list.
    pub fn list(&self) -> Vec<Listed> {
        self.registry()
            .attachments
            .iter()
            .map(|a| Listed {
                id: a.id,
                client: a.client.clone(),
                name: a.device.name.clone(),
                vid: a.device.vid,
                pid: a.device.pid,
                class: a.device.class,
                speed: a.device.speed,
                busid: a.device.busid.clone(),
                state: match (a.stage, a.ending.is_some()) {
                    (Stage::Stuck { .. }, _) => "detach_failed",
                    (_, true) => "detaching",
                    (Stage::Pending { .. }, _) => "waiting",
                    (Stage::Opening, _) => "opening",
                    (Stage::Attached { .. }, _) => "attached",
                },
                port: match a.stage {
                    Stage::Attached { port } => Some(port),
                    Stage::Stuck { port, .. } => port,
                    _ => None,
                },
                since_unix_ms: a
                    .since
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as u64),
                error: a.error.clone(),
            })
            .collect()
    }

    /// The console detaches attachment `id`; false when there is none. A device the driver
    /// kept is detached again here, blocking for usbip.exe; Err when the driver keeps it again.
    pub fn detach(&self, id: u64) -> Result<bool> {
        let mut registry = self.registry();
        let Some(index) = registry.index(id) else {
            return Ok(false);
        };
        let attachment = &registry.attachments[index];
        let request_id = attachment.request_id;
        if let Stage::Stuck { port, tcp_port } = attachment.stage {
            let busid = attachment.device.busid.clone();
            drop(registry);
            tracing::info!(id, "USB detach retried from the console");
            let unplugged = unplug(&*self.vhci(), port, tcp_port, &busid);
            let mut registry = self.registry();
            let Some(index) = registry.index(id) else {
                return Ok(true);
            };
            return match unplugged {
                Ok(()) => {
                    let attachment = registry.attachments.remove(index);
                    tracing::info!(client = %attachment.client, device = %attachment.device.name, "USB device detached from the console after a failed detach");
                    Ok(true)
                }
                Err(error) => {
                    let error = error.context("the USB/IP driver still holds the device");
                    tracing::error!(id, error = format!("{error:#}"), "USB detach failed again");
                    registry.attachments[index].error = Some(format!("{error:#}"));
                    Err(error)
                }
            };
        }
        tracing::info!(id, "USB device detached from the console");
        let ended = registry.end(
            id,
            vec![
                UsbMessage::Detach { request_id },
                status(request_id, UsbState::Detached, UsbReason::None),
            ],
        );
        if ended {
            self.mark_queued();
        }
        Ok(ended)
    }

    /// Takes the tunnel token of the device with certificate `cert`.
    pub fn claim(
        &self,
        token: &[u8; TOKEN_BYTES],
        cert: &str,
        now: Instant,
    ) -> Result<Opened, Refused> {
        let mut registry = self.registry();
        let attachment = registry
            .attachments
            .iter_mut()
            .find(|a| same(&a.token, token) && a.cert == cert)
            .ok_or(Refused::Unknown)?;
        match attachment.stage {
            Stage::Pending { expires } if expires > now && attachment.ending.is_none() => {}
            Stage::Pending { .. } => return Err(Refused::Unknown),
            Stage::Opening | Stage::Attached { .. } | Stage::Stuck { .. } => {
                return Err(Refused::Used);
            }
        }
        attachment.stage = Stage::Opening;
        Ok(Opened {
            id: attachment.id,
            request_id: attachment.request_id,
            device: attachment.device.clone(),
            client: attachment.client.clone(),
            stop: attachment.stop.clone(),
        })
    }

    /// The upgrade of a claimed tunnel failed.
    pub fn abandon(&self, opened: Opened, error: impl std::fmt::Display) {
        tracing::warn!(client = %opened.client, device = %opened.device.name, %error, "USB tunnel upgrade failed");
        self.finish(
            &opened,
            vec![status(
                opened.request_id,
                UsbState::Failed,
                UsbReason::DeviceError,
            )],
        );
    }

    fn attached(&self, opened: &Opened, port: u32) {
        let mut registry = self.registry();
        let Some(index) = registry.index(opened.id) else {
            return;
        };
        let attachment = &mut registry.attachments[index];
        attachment.stage = Stage::Attached { port };
        if attachment.ending.is_none() {
            tracing::info!(client = %opened.client, device = %opened.device.name, port, "USB device attached");
            let session = attachment.session.clone();
            registry.queue(
                &session,
                [status(
                    opened.request_id,
                    UsbState::Attached,
                    UsbReason::None,
                )],
            );
            self.mark_queued();
        }
    }

    /// Forgets a relayed attachment and tells its client what ended it, unless that was said.
    fn finish(&self, opened: &Opened, natural: Vec<UsbMessage>) {
        let mut registry = self.registry();
        let Some(index) = registry.index(opened.id) else {
            return;
        };
        let attachment = registry.attachments.remove(index);
        let notice = attachment.ending.unwrap_or(natural);
        registry.queue(&attachment.session, notice);
        self.mark_queued();
    }

    /// The relay ended but the driver kept the device: the console keeps listing it until a
    /// retried detach works, and the client hears it failed instead of what ended it.
    fn stuck(
        &self,
        opened: &Opened,
        port: Option<u32>,
        tcp_port: u16,
        error: anyhow::Error,
        natural: Vec<UsbMessage>,
    ) {
        tracing::error!(client = %opened.client, device = %opened.device.name, ?port, error = format!("{error:#}"), "USB detach failed: the USB/IP driver still holds the device; detach it again in the console");
        let mut registry = self.registry();
        let Some(index) = registry.index(opened.id) else {
            return;
        };
        let attachment = &mut registry.attachments[index];
        attachment.stage = Stage::Stuck { port, tcp_port };
        attachment.error = Some(format!("{error:#}"));
        let notice: Vec<_> = attachment
            .ending
            .take()
            .unwrap_or(natural)
            .into_iter()
            .map(|message| match message {
                UsbMessage::Status { request_id, .. } => {
                    status(request_id, UsbState::Failed, UsbReason::Driver)
                }
                other => other,
            })
            .collect();
        let session = attachment.session.clone();
        registry.queue(&session, notice);
        self.mark_queued();
    }
}

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
#[cfg(test)]
pub(crate) mod tests;
