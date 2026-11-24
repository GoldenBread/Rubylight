//! A game session's clipboard on its X server (Xwayland of gamescope), through the `CLIPBOARD`
//! selection. One window and one event thread per display: the thread serves the data this
//! host copied (`SelectionRequest`), notices when something else took the selection, counts
//! changes (XFixes `SelectionNotify`, which also reports this host's own) and hands the
//! answers of reads to the caller waiting for them. Reads follow the `INCR` protocol for large
//! data; writes go in one property, so one over the server's request size fails.

use anyhow::{Context, Result, bail};
use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use x11rb::{
    COPY_DEPTH_FROM_PARENT, CURRENT_TIME, NONE,
    connection::{Connection, RequestConnection},
    protocol::{
        Event,
        xfixes::{ConnectionExt as _, SelectionEventMask},
        xproto::{
            Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, PropMode, Property,
            SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, SelectionRequestEvent, Window,
            WindowClass,
        },
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

use super::{Backend, pick_text};

/// How long a read waits for the selection's owner.
const TIMEOUT: Duration = Duration::from_secs(5);
/// How long a write waits for its own change to show in the count.
const SETTLE: Duration = Duration::from_millis(300);
/// The predefined `STRING` atom (Latin-1 text).
const STRING: Atom = 31; // AtomEnum::STRING

struct Atoms {
    clipboard: Atom,
    targets: Atom,
    incr: Atom,
    property: Atom,
    utf8: Atom,
    text_utf8: Atom,
    png: Atom,
}

/// What this host copied and offers to others.
enum Owned {
    Text(String),
    Png(Vec<u8>),
}

struct Inner {
    conn: RustConnection,
    window: Window,
    atoms: Atoms,
    changes: AtomicU32,
    dead: AtomicBool,
    owned: Mutex<Option<Owned>>,
    /// Where the event thread sends what a read waits for.
    waiter: Mutex<Option<mpsc::Sender<Event>>>,
    /// One read at a time: they share a property.
    reading: Mutex<()>,
}

pub(super) struct Clipboard {
    inner: Arc<Inner>,
}

impl Clipboard {
    pub(super) fn open(display: &str) -> Result<Self> {
        let (conn, screen) = x11rb::connect(Some(display))
            .with_context(|| format!("connecting to the game's X server {display}"))?;
        let root = conn.setup().roots[screen].root;
        let window = conn.generate_id()?;
        conn.create_window(
            COPY_DEPTH_FROM_PARENT,
            window,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )?;
        let atom = |name: &str| -> Result<Atom> {
            Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom)
        };
        let atoms = Atoms {
            clipboard: atom("CLIPBOARD")?,
            targets: atom("TARGETS")?,
            incr: atom("INCR")?,
            property: atom("RUBYLIGHT_CLIPBOARD")?,
            utf8: atom("UTF8_STRING")?,
            text_utf8: atom("text/plain;charset=utf-8")?,
            png: atom("image/png")?,
        };
        conn.xfixes_query_version(5, 0)?
            .reply()
            .context("the X server has no XFixes")?;
        conn.xfixes_select_selection_input(
            window,
            atoms.clipboard,
            SelectionEventMask::SET_SELECTION_OWNER
                | SelectionEventMask::SELECTION_WINDOW_DESTROY
                | SelectionEventMask::SELECTION_CLIENT_CLOSE,
        )?;
        conn.flush()?;
        let inner = Arc::new(Inner {
            conn,
            window,
            atoms,
            changes: AtomicU32::new(0),
            dead: AtomicBool::new(false),
            owned: Mutex::new(None),
            waiter: Mutex::new(None),
            reading: Mutex::new(()),
        });
        let thread = inner.clone();
        std::thread::Builder::new()
            .name("clipboard-x11".into())
            .spawn(move || thread.events())?;
        Ok(Self { inner })
    }
}

impl Inner {
    /// The event loop, until the connection ends with the game's X server.
    fn events(&self) {
        loop {
            match self.conn.wait_for_event() {
                Ok(event) => {
                    if let Err(error) = self.dispatch(event) {
                        tracing::debug!("clipboard event: {error:#}");
                    }
                }
                Err(_) => {
                    self.dead.store(true, Ordering::Relaxed);
                    // Wakes a reader, which finds the connection gone.
                    self.waiter
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .take();
                    return;
                }
            }
        }
    }

    fn dispatch(&self, event: Event) -> Result<()> {
        match event {
            Event::XfixesSelectionNotify(e) if e.selection == self.atoms.clipboard => {
                self.changes.fetch_add(1, Ordering::Relaxed);
            }
            Event::SelectionClear(e) if e.selection == self.atoms.clipboard => {
                self.owned
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take();
            }
            Event::SelectionRequest(request) => self.serve(&request)?,
            Event::SelectionNotify(_) | Event::PropertyNotify(_) => {
                if let Some(waiter) = &*self.waiter.lock().unwrap_or_else(PoisonError::into_inner) {
                    let _ = waiter.send(event);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Answers another client's request for what this host copied.
    fn serve(&self, request: &SelectionRequestEvent) -> Result<()> {
        let a = &self.atoms;
        let property = if request.property == NONE {
            request.target
        } else {
            request.property
        };
        let served = {
            let owned = self.owned.lock().unwrap_or_else(PoisonError::into_inner);
            match (&*owned, request.selection == a.clipboard) {
                (Some(owned), true) => self.fill(request, property, owned)?,
                _ => false,
            }
        };
        let notify = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: request.time,
            requestor: request.requestor,
            selection: request.selection,
            target: request.target,
            property: if served { property } else { NONE },
        };
        self.conn
            .send_event(false, request.requestor, EventMask::NO_EVENT, notify)?;
        self.conn.flush()?;
        Ok(())
    }

    /// Sets `property` of the requestor to what `target` asks of `owned`.
    fn fill(&self, request: &SelectionRequestEvent, property: Atom, owned: &Owned) -> Result<bool> {
        let a = &self.atoms;
        let (window, target) = (request.requestor, request.target);
        if target == a.targets {
            let formats: Vec<Atom> = match owned {
                Owned::Text(_) => vec![a.targets, a.utf8, a.text_utf8, STRING],
                Owned::Png(_) => vec![a.targets, a.png],
            };
            self.conn.change_property32(
                PropMode::REPLACE,
                window,
                property,
                AtomEnum::ATOM,
                &formats,
            )?;
            return Ok(true);
        }
        let (kind, bytes): (Atom, &[u8]) = match owned {
            Owned::Text(text) if target == a.utf8 || target == a.text_utf8 => {
                (target, text.as_bytes())
            }
            Owned::Text(text) if target == STRING => {
                // Latin-1 is not UTF-8; only text that is both is served as STRING.
                if !text.is_ascii() {
                    return Ok(false);
                }
                (STRING, text.as_bytes())
            }
            Owned::Png(png) if target == a.png => (a.png, png),
            _ => return Ok(false),
        };
        if bytes.len() + 64 > self.conn.maximum_request_bytes() {
            tracing::warn!("the clipboard's data is larger than the X server takes in one request");
            return Ok(false);
        }
        self.conn
            .change_property8(PropMode::REPLACE, window, property, kind, bytes)?;
        Ok(true)
    }

    fn check_alive(&self) -> Result<()> {
        if self.dead.load(Ordering::Relaxed) {
            bail!("the game's X server has ended");
        }
        Ok(())
    }

    fn owner_is_us(&self) -> Result<bool> {
        let owner = self
            .conn
            .get_selection_owner(self.atoms.clipboard)?
            .reply()?
            .owner;
        Ok(owner == self.window)
    }

    /// Asks the selection's owner for `target` and returns its bytes, at most `max`; `None`
    /// when the owner refuses it.
    fn convert(&self, target: Atom, max: usize) -> Result<Option<(Atom, Vec<u8>)>> {
        self.check_alive()?;
        let _reading = self.reading.lock().unwrap_or_else(PoisonError::into_inner);
        let (send, receive) = mpsc::channel();
        *self.waiter.lock().unwrap_or_else(PoisonError::into_inner) = Some(send);
        let result = self.convert_waiting(target, max, &receive);
        self.waiter
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        result
    }

    fn convert_waiting(
        &self,
        target: Atom,
        max: usize,
        events: &mpsc::Receiver<Event>,
    ) -> Result<Option<(Atom, Vec<u8>)>> {
        let a = &self.atoms;
        self.conn.delete_property(self.window, a.property)?;
        self.conn
            .convert_selection(self.window, a.clipboard, target, a.property, CURRENT_TIME)?;
        self.conn.flush()?;
        let deadline = Instant::now() + TIMEOUT;
        let next = |deadline: Instant| -> Result<Event> {
            events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .map_err(|_| {
                    if self.dead.load(Ordering::Relaxed) {
                        anyhow::anyhow!("the game's X server has ended")
                    } else {
                        anyhow::anyhow!("the clipboard's owner did not answer within {TIMEOUT:?}")
                    }
                })
        };
        loop {
            if let Event::SelectionNotify(e) = next(deadline)?
                && e.selection == a.clipboard
                && e.target == target
            {
                if e.property == NONE {
                    return Ok(None);
                }
                break;
            }
        }
        let (kind, mut data) = self.take_property(max)?;
        if kind != a.incr {
            return Ok(Some((kind, data)));
        }
        // INCR: the owner sends chunks; each one is read and deleted, and an empty one ends.
        data.clear();
        let mut kind = AtomEnum::NONE.into();
        loop {
            match next(Instant::now() + TIMEOUT)? {
                Event::PropertyNotify(p)
                    if p.window == self.window
                        && p.atom == a.property
                        && p.state == Property::NEW_VALUE =>
                {
                    let (chunk_kind, chunk) = self.take_property(max)?;
                    if chunk.is_empty() {
                        return Ok(Some((kind, data)));
                    }
                    kind = chunk_kind;
                    if data.len() + chunk.len() > max {
                        bail!("the clipboard holds more than {max} bytes");
                    }
                    data.extend_from_slice(&chunk);
                }
                _ => {}
            }
        }
    }

    /// Reads and deletes our property, failing over `max` bytes.
    fn take_property(&self, max: usize) -> Result<(Atom, Vec<u8>)> {
        let a = &self.atoms;
        let mut data = Vec::new();
        let mut kind = AtomEnum::NONE.into();
        loop {
            let reply = self
                .conn
                .get_property(
                    false,
                    self.window,
                    a.property,
                    AtomEnum::ANY,
                    (data.len() / 4) as u32,
                    1 << 20,
                )?
                .reply()?;
            kind = if reply.type_ == NONE {
                kind
            } else {
                reply.type_
            };
            data.extend_from_slice(&reply.value);
            if data.len() > max {
                self.conn.delete_property(self.window, a.property)?;
                self.conn.flush()?;
                bail!("the clipboard holds more than {max} bytes");
            }
            if reply.bytes_after == 0 {
                break;
            }
        }
        self.conn.delete_property(self.window, a.property)?;
        self.conn.flush()?;
        Ok((kind, data))
    }

    /// The formats the selection's owner offers, by name.
    fn formats(&self) -> Result<Vec<String>> {
        if self.owner_is_us()? {
            return Ok(
                match &*self.owned.lock().unwrap_or_else(PoisonError::into_inner) {
                    Some(Owned::Text(_)) => {
                        vec!["UTF8_STRING".into(), "text/plain;charset=utf-8".into()]
                    }
                    Some(Owned::Png(_)) => vec!["image/png".into()],
                    None => Vec::new(),
                },
            );
        }
        if self
            .conn
            .get_selection_owner(self.atoms.clipboard)?
            .reply()?
            .owner
            == NONE
        {
            return Ok(Vec::new());
        }
        let Some((_, bytes)) = self.convert(self.atoms.targets, 64 * 1024)? else {
            return Ok(Vec::new());
        };
        let mut names = Vec::new();
        for atom in bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_ne_bytes(*c))
        {
            let name = self.conn.get_atom_name(atom)?.reply()?.name;
            names.push(String::from_utf8_lossy(&name).into_owned());
        }
        Ok(names)
    }

    fn own(&self, owned: Owned) -> Result<()> {
        self.check_alive()?;
        let before = self.changes.load(Ordering::Relaxed);
        *self.owned.lock().unwrap_or_else(PoisonError::into_inner) = Some(owned);
        self.conn
            .set_selection_owner(self.window, self.atoms.clipboard, CURRENT_TIME)?;
        self.conn.flush()?;
        if !self.owner_is_us()? {
            bail!("the game's X server did not give this host the clipboard");
        }
        let until = Instant::now() + SETTLE;
        while self.changes.load(Ordering::Relaxed) == before && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }

    fn owned_text(&self) -> Option<String> {
        match &*self.owned.lock().unwrap_or_else(PoisonError::into_inner) {
            Some(Owned::Text(text)) => Some(text.clone()),
            _ => None,
        }
    }
}

impl Backend for Clipboard {
    fn sequence(&self) -> u32 {
        self.inner.changes.load(Ordering::Relaxed)
    }

    fn text(&self, max: usize) -> Result<Option<String>> {
        let inner = &self.inner;
        inner.check_alive()?;
        // Reading our own selection would ask this thread's own event loop, which answers
        // too, but the copy is at hand.
        if inner.owner_is_us()? {
            return Ok(inner.owned_text());
        }
        let formats = inner.formats()?;
        let Some(name) = pick_text(&formats, &["STRING"]) else {
            return Ok(None);
        };
        let target = inner
            .conn
            .intern_atom(false, name.as_bytes())?
            .reply()?
            .atom;
        let Some((_, bytes)) = inner.convert(target, max)? else {
            return Ok(None);
        };
        // STRING is Latin-1; the others are UTF-8.
        let text = if name == "STRING" {
            bytes.iter().map(|b| *b as char).collect()
        } else {
            String::from_utf8(bytes).context("the clipboard's text is not UTF-8")?
        };
        Ok(Some(text.trim_end_matches('\0').to_owned()))
    }

    fn png(&self, max: usize) -> Result<Option<Vec<u8>>> {
        let inner = &self.inner;
        inner.check_alive()?;
        if !inner.formats()?.iter().any(|f| f == "image/png") {
            return Ok(None);
        }
        if inner.owner_is_us()? {
            let owned = inner.owned.lock().unwrap_or_else(PoisonError::into_inner);
            return Ok(match &*owned {
                Some(Owned::Png(png)) => Some(png.clone()),
                _ => None,
            });
        }
        Ok(inner.convert(inner.atoms.png, max)?.map(|(_, bytes)| bytes))
    }

    fn set_text(&self, text: &str) -> Result<()> {
        self.inner.own(Owned::Text(text.to_owned()))
    }

    fn set_png(&self, png: &[u8]) -> Result<()> {
        self.inner.own(Owned::Png(png.to_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    /// Runs a headless gamescope for a few seconds and returns its X display with the process.
    fn gamescope() -> Option<(std::process::Child, String)> {
        let file = std::env::temp_dir().join(format!("rubylight-x11-test-{}", std::process::id()));
        let mut child = Command::new("gamescope")
            .args(["--backend", "headless", "-W", "640", "-H", "480", "--"])
            .args(["sh", "-c"])
            .arg(format!("echo $DISPLAY > {}; sleep 25", file.display()))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        for _ in 0..100 {
            if let Ok(display) = std::fs::read_to_string(&file) {
                let _ = std::fs::remove_file(&file);
                if !display.trim().is_empty() {
                    return Some((child, display.trim().to_owned()));
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = child.kill();
        None
    }

    /// Text and a PNG round-trip through the X clipboard of a headless gamescope, and a
    /// second client (another connection) reads what the first set.
    #[test]
    #[ignore = "starts a headless gamescope"]
    fn text_and_png_round_trip_in_gamescope() {
        let (mut child, display) = gamescope().expect("gamescope did not start");
        let first = Clipboard::open(&display).unwrap();
        assert_eq!(first.text(1 << 20).unwrap(), None);
        let seq = first.sequence();
        first.set_text("héllo rubylight").unwrap();
        assert!(first.sequence() != seq, "the change count did not move");
        assert_eq!(
            first.text(1 << 20).unwrap().as_deref(),
            Some("héllo rubylight")
        );
        // Another client: a second connection of our own, so it asks the owner over the wire.
        let second = Clipboard::open(&display).unwrap();
        assert_eq!(
            second.text(1 << 20).unwrap().as_deref(),
            Some("héllo rubylight")
        );
        let png = {
            let image = butterpollo_session::clipboard_image::Image {
                width: 2,
                height: 2,
                rgba: vec![255; 16],
            };
            butterpollo_session::clipboard_image::encode_png(&image).unwrap()
        };
        second.set_png(&png).unwrap();
        assert_eq!(first.png(1 << 20).unwrap().as_deref(), Some(&png[..]));
        assert_eq!(second.text(1 << 20).unwrap(), None);
        // A big text goes over INCR.
        let big = "x".repeat(400 * 1024);
        first.set_text(&big).unwrap();
        assert_eq!(second.text(1 << 20).unwrap().as_deref(), Some(big.as_str()));
        assert!(
            second.text(1000).is_err(),
            "the size limit was not enforced"
        );
        let _ = child.kill();
        let _ = child.wait();
    }
}
