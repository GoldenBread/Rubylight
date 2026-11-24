//! Input worker message queues, guest backpressure and transport handles.

use crate::media::ControlSender;
use butterpollo_session::webrtc::guests::Permission;
use butterpollo_stream::state::Session;
use std::{
    collections::{HashMap, VecDeque, hash_map::Entry},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Instant,
};

/// A guest's controllers are numbered above every id an owner's browser uses (the Gamepad API
/// counts from 0): this bit, the guest's tag, then the id its own browser gave.
pub(super) const GUEST_IDS: u32 = 1 << 31;
pub(super) const GUEST_ID_BITS: u32 = 16;
pub(super) const MAX_GUEST_TAG: u16 = 0x7fff;
/// Pads one guest may plug in, and the slots the guests together leave to the owner's.
pub(super) const GUEST_PADS: usize = 4;
pub(super) const OWNER_PADS: usize = 4;
/// A guest's gamepad and input messages waiting for the worker before they give way: past this,
/// motion is dropped and a snapshot replaces the guest's last one still waiting ([`Yields`]).
pub(super) const GUEST_QUEUE: usize = 64;
/// A guest's messages waiting in all, so a page that sends faster than the desktop takes them
/// cannot fill the host's memory. Past this even keys, buttons and pads coming and going are
/// dropped, and the worker lets go of everything the guest holds when it gets there.
pub(super) const GUEST_BACKLOG: usize = 256;

/// The id in the shared table of a guest's controller; None for an id out of range.
pub(super) fn guest_controller(tag: u16, controller_id: u32) -> Option<u32> {
    (controller_id < 1 << GUEST_ID_BITS)
        .then_some(GUEST_IDS | u32::from(tag) << GUEST_ID_BITS | controller_id)
}

/// The guest tag and the guest's own id of a controller in the shared table; None for the
/// owner's.
pub(super) fn guest_of(controller_id: u32) -> Option<(u16, u32)> {
    (controller_id & GUEST_IDS != 0).then_some((
        ((controller_id & !GUEST_IDS) >> GUEST_ID_BITS) as u16,
        controller_id & ((1 << GUEST_ID_BITS) - 1),
    ))
}

/// What the worker is told. Text frames only: the channels carry JSON.
pub(super) enum Message {
    Gamepad(String, Instant),
    Input(String, Instant),
    /// A DataChannel closed.
    GamepadClosed,
    InputClosed,
    Bind(Arc<Session>),
    Control(ControlSender),
    /// The transport is gone.
    Closed,
    /// The guests' queue has messages ([`GuestInbox`]).
    Guests,
}

/// What a guest's transport tells the worker, by guest id.
pub(super) enum GuestMessage {
    /// A guest of the stream joined, with what it may send.
    Joined(u64, Permission),
    Gamepad(u64, String, Instant),
    Input(u64, String, Instant),
    Control(u64, ControlSender),
    GamepadClosed(u64),
    InputClosed(u64),
    /// A guest left: its pads are unplugged and what it held is released.
    Left(u64),
    /// Some of a guest's keys, buttons or pads' comings and goings were dropped
    /// ([`GUEST_BACKLOG`]): what it holds is released and its pads are unplugged, as if it had
    /// let go of everything.
    Lost(u64),
}

impl GuestMessage {
    fn guest(&self) -> u64 {
        match *self {
            Self::Joined(id, _)
            | Self::Gamepad(id, ..)
            | Self::Input(id, ..)
            | Self::Control(id, _)
            | Self::GamepadClosed(id)
            | Self::InputClosed(id)
            | Self::Left(id)
            | Self::Lost(id) => id,
        }
    }

    /// What the page sends, as opposed to the guest's comings and goings, which always queue.
    fn data(&self) -> bool {
        matches!(self, Self::Gamepad(..) | Self::Input(..))
    }
}

/// How a guest's message gives way once its guest has [`GUEST_QUEUE`] waiting.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Yields {
    /// Motion, scrolling and what the worker could not read anyway: dropped, which leaves
    /// nothing held.
    Drop,
    /// A whole state (a pad's buttons and sticks, the pointer's position): replaces the guest's
    /// last one of the same kind still waiting, unless something that must keep its order came
    /// after it.
    Replace(Snapshot),
    /// Keys, buttons, text, pads coming and going, the guest's comings and goings: queued until
    /// [`GUEST_BACKLOG`].
    Keep,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Snapshot {
    Pad(u32),
    Pointer,
}

impl Yields {
    /// What a page's `gamepad` or `input` message is, from its `type` and `controllerId` alone.
    fn of(message: &GuestMessage) -> Self {
        #[derive(serde::Deserialize)]
        struct Peek<'a> {
            #[serde(rename = "type", borrow)]
            kind: std::borrow::Cow<'a, str>,
            #[serde(rename = "controllerId")]
            controller_id: Option<u32>,
        }
        let (text, gamepad) = match message {
            GuestMessage::Gamepad(_, text, _) => (text, true),
            GuestMessage::Input(_, text, _) => (text, false),
            _ => return Self::Keep,
        };
        let Ok(peek) = serde_json::from_str::<Peek>(text) else {
            return Self::Drop;
        };
        match (gamepad, &*peek.kind, peek.controller_id) {
            (true, "gamepad-state", Some(id)) => Self::Replace(Snapshot::Pad(id)),
            (true, "gamepad-connected" | "gamepad-disconnected", _) => Self::Keep,
            (false, "mouse-position", _) => Self::Replace(Snapshot::Pointer),
            (false, "key" | "mouse-button" | "text", _) => Self::Keep,
            _ => Self::Drop,
        }
    }
}

/// The guests' messages, beside the owner's queue: the worker takes one only while the owner's
/// queue is empty, so no guest delays the owner's input. A guest's messages give way past
/// [`GUEST_QUEUE`] ([`Yields`]) and stop at [`GUEST_BACKLOG`].
#[derive(Default)]
pub(super) struct GuestInbox {
    pub(super) state: Mutex<Inbox>,
    /// A [`Message::Guests`] is in the owner's queue and the worker has not taken it yet.
    pub(super) woken: AtomicBool,
}

#[derive(Default)]
pub(super) struct Inbox {
    pub(super) queue: VecDeque<(GuestMessage, Yields)>,
    /// Per guest with data waiting or a [`GuestMessage::Lost`] last in the queue; no entry for
    /// none.
    pub(super) waiting: HashMap<u64, Backlog>,
}

#[derive(Default, Debug)]
pub(super) struct Backlog {
    /// Data messages queued.
    pub(super) data: usize,
    /// The guest's last message queued is a [`GuestMessage::Lost`], which covers what is
    /// dropped until another queues behind it.
    lost: bool,
}

impl Inbox {
    /// The guest's last snapshot of `kind` still waiting, with nothing that keeps its order
    /// queued for the guest after it.
    fn snapshot(&mut self, guest: u64, kind: Snapshot) -> Option<&mut GuestMessage> {
        for (message, yields) in self.queue.iter_mut().rev() {
            if message.guest() != guest {
                continue;
            }
            match *yields {
                Yields::Replace(k) if k == kind => return Some(message),
                Yields::Keep => return None,
                _ => {}
            }
        }
        None
    }

    /// Queue `message`, or say why not: false when it was dropped. `wake`: the worker needs
    /// waking for what was queued.
    fn push(&mut self, message: GuestMessage) -> (bool, bool) {
        let guest = message.guest();
        let yields = Yields::of(&message);
        let data = self.waiting.get(&guest).map_or(0, |b| b.data);
        if message.data() && data >= GUEST_QUEUE {
            match yields {
                Yields::Drop => return (false, false),
                // The worker takes the guest's waiting messages before it sleeps: no wake.
                Yields::Replace(kind) => {
                    if let Some(older) = self.snapshot(guest, kind) {
                        *older = message;
                        return (true, false);
                    }
                }
                Yields::Keep => {}
            }
            if data >= GUEST_BACKLOG {
                let backlog = self.waiting.entry(guest).or_default();
                if std::mem::replace(&mut backlog.lost, true) {
                    return (false, false);
                }
                self.queue
                    .push_back((GuestMessage::Lost(guest), Yields::Keep));
                return (false, true);
            }
        }
        let backlog = self.waiting.entry(guest).or_default();
        backlog.lost = false;
        if message.data() {
            backlog.data += 1;
        } else if backlog.data == 0 {
            self.waiting.remove(&guest);
        }
        self.queue.push_back((message, yields));
        (true, true)
    }

    fn pop(&mut self) -> Option<GuestMessage> {
        let (message, _) = self.queue.pop_front()?;
        let guest = message.guest();
        if let Entry::Occupied(mut backlog) = self.waiting.entry(guest) {
            match message {
                GuestMessage::Lost(_) => backlog.get_mut().lost = false,
                _ if message.data() => backlog.get_mut().data -= 1,
                _ => {}
            }
            if backlog.get().data == 0 && !backlog.get().lost {
                backlog.remove();
            }
        }
        Some(message)
    }
}

impl GuestInbox {
    /// Queue `message` and wake the worker. False when it was dropped ([`Yields`]).
    fn push(&self, tx: &mpsc::Sender<Message>, message: GuestMessage) -> bool {
        let (queued, wake) = self.state.lock().unwrap().push(message);
        if wake && !self.woken.swap(true, Ordering::AcqRel) {
            let _ = tx.send(Message::Guests);
        }
        queued
    }

    pub(super) fn pop(&self) -> Option<GuestMessage> {
        self.state.lock().unwrap().pop()
    }
}

/// The worker's end of an [`InputHandle`].
pub(crate) struct Queues {
    pub(super) rx: mpsc::Receiver<Message>,
    pub(super) guests: Arc<GuestInbox>,
}

/// A worker's handle and the queues it reads.
pub(crate) fn channel() -> (InputHandle, Queues) {
    let (tx, rx) = mpsc::channel();
    let guests = Arc::new(GuestInbox::default());
    (InputHandle(tx, guests.clone()), Queues { rx, guests })
}

/// The transport's end of a session's input worker. Every call only queues; none blocks.
#[derive(Clone)]
pub struct InputHandle(mpsc::Sender<Message>, Arc<GuestInbox>);

impl InputHandle {
    /// A handle with no worker behind it, and the texts it was given for the `gamepad` channel.
    #[cfg(test)]
    pub(crate) fn probe() -> (Self, impl FnMut() -> Vec<String>) {
        let (tx, rx) = mpsc::channel();
        let drain = move || {
            rx.try_iter()
                .filter_map(|m| match m {
                    Message::Gamepad(text, _) => Some(text),
                    _ => None,
                })
                .collect()
        };
        (Self(tx, Arc::default()), drain)
    }

    /// A text frame from the `gamepad` channel, received at `now`. A binary or non-UTF-8 frame is
    /// not part of the protocol and is dropped with a warning.
    pub fn gamepad(&self, data: Vec<u8>, binary: bool, now: Instant) {
        if let Some(text) = text(data, binary, "gamepad", None) {
            let _ = self.0.send(Message::Gamepad(text, now));
        }
    }

    /// A text frame from the `input` channel.
    pub fn input(&self, data: Vec<u8>, binary: bool) {
        if let Some(text) = text(data, binary, "input", None) {
            let _ = self.0.send(Message::Input(text, Instant::now()));
        }
    }

    /// A DataChannel closed: `input` releases what the client held and `gamepad` unplugs its
    /// controllers.
    pub fn channel_closed(&self, label: &str) {
        match label {
            "input" => {
                let _ = self.0.send(Message::InputClosed);
            }
            "gamepad" => {
                let _ = self.0.send(Message::GamepadClosed);
            }
            _ => {}
        }
    }

    /// The session exists; its client's permissions and display apply from here.
    pub fn bind(&self, session: Arc<Session>) {
        let _ = self.0.send(Message::Bind(session));
    }

    /// The `control` channel to answer on.
    pub fn control(&self, sender: ControlSender) {
        let _ = self.0.send(Message::Control(sender));
    }

    /// The transport closed: release everything and end.
    pub fn closed(&self) {
        let _ = self.0.send(Message::Closed);
    }

    /// A guest of this session's stream, sending through this worker with `permission`. None
    /// for a view-only guest: nothing it sends reaches the worker.
    pub fn guest(&self, id: u64, permission: Permission) -> Option<GuestHandle> {
        if !permission.gamepads() {
            return None;
        }
        let guest = GuestHandle {
            tx: self.0.clone(),
            inbox: self.1.clone(),
            id,
            permission,
            warned: AtomicBool::new(false),
            flooded: AtomicBool::new(false),
        };
        guest.push(GuestMessage::Joined(id, permission));
        Some(guest)
    }
}

/// A guest's end of the owner's input worker, for the guest transport's hooks. Every call only
/// queues, and only what `permission` allows. Dropping it is the guest leaving: its pads are
/// unplugged and its keys let go.
pub struct GuestHandle {
    tx: mpsc::Sender<Message>,
    inbox: Arc<GuestInbox>,
    id: u64,
    permission: Permission,
    /// A frame that is not text was logged once; a page sending more must not flood the log.
    warned: AtomicBool,
    /// Dropping messages over [`GUEST_QUEUE`] was logged once.
    flooded: AtomicBool,
}

impl GuestHandle {
    fn push(&self, message: GuestMessage) {
        if !self.inbox.push(&self.tx, message) && !self.flooded.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                guest = self.id,
                "WebRTC input: a guest sends faster than the desktop takes it; dropping the excess"
            );
        }
    }

    pub fn gamepad(&self, data: Vec<u8>, binary: bool, now: Instant) {
        if let Some(text) = text(data, binary, "gamepad", Some(&self.warned)) {
            self.push(GuestMessage::Gamepad(self.id, text, now));
        }
    }

    pub fn input(&self, data: Vec<u8>, binary: bool) {
        if !self.permission.keyboard_mouse() {
            return;
        }
        if let Some(text) = text(data, binary, "input", Some(&self.warned)) {
            self.push(GuestMessage::Input(self.id, text, Instant::now()));
        }
    }

    pub fn channel_closed(&self, label: &str) {
        let message = match label {
            "input" => GuestMessage::InputClosed(self.id),
            "gamepad" => GuestMessage::GamepadClosed(self.id),
            _ => return,
        };
        self.push(message);
    }

    /// The guest's `control` channel, for its pads' rumble and mouse mode.
    pub fn control(&self, sender: ControlSender) {
        self.push(GuestMessage::Control(self.id, sender));
    }
}

impl Drop for GuestHandle {
    fn drop(&mut self) {
        self.push(GuestMessage::Left(self.id));
    }
}

/// The frame's text. A guest's frames that are not text are logged loudly once (`warned`), then
/// quietly.
fn text(data: Vec<u8>, binary: bool, channel: &str, warned: Option<&AtomicBool>) -> Option<String> {
    let why = if binary {
        "WebRTC input: a binary frame is not part of the protocol; dropped"
    } else {
        match String::from_utf8(data) {
            Ok(text) => return Some(text),
            Err(_) => "WebRTC input: a frame that is not UTF-8; dropped",
        }
    };
    if warned.is_some_and(|warned| warned.swap(true, Ordering::Relaxed)) {
        tracing::debug!(channel, "{why}");
    } else {
        tracing::warn!(channel, "{why}");
    }
    None
}
