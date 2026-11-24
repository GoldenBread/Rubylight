//! A connection to gamescope's Xwayland that injects keyboard and mouse input through the
//! XTEST extension. A headless gamescope reads no evdev devices, so XTEST is how keys,
//! buttons and motion reach the game's X clients.

use super::keymap::KeyTable;
use anyhow::{Context, Result, bail};
use x11rb::{
    connection::{Connection, RequestConnection},
    protocol::{
        xproto::{ConnectionExt as _, Window},
        xtest::ConnectionExt as _,
    },
    rust_connection::RustConnection,
};

// The core event codes XTEST's `FakeInput` takes as its type.
const KEY_PRESS: u8 = 2;
const KEY_RELEASE: u8 = 3;
const BUTTON_PRESS: u8 = 4;
const BUTTON_RELEASE: u8 = 5;
const MOTION_NOTIFY: u8 = 6;

/// One open X connection with the server's keyboard mapping and screen size.
pub(super) struct XServer {
    conn: RustConnection,
    root: Window,
    /// The `DISPLAY` this connects to.
    pub(super) display: String,
    /// The screen's size in pixels.
    pub(super) size: (u32, u32),
    keys: KeyTable,
}

impl XServer {
    /// Connects to `display` (such as ":2") and checks that it has XTEST.
    pub(super) fn connect(display: &str) -> Result<Self> {
        let (conn, screen_number) = RustConnection::connect(Some(display))
            .with_context(|| format!("cannot connect to the X server {display}"))?;
        let screen = &conn.setup().roots[screen_number];
        let (root, size) = (
            screen.root,
            (
                u32::from(screen.width_in_pixels),
                u32::from(screen.height_in_pixels),
            ),
        );
        let (first, last) = (conn.setup().min_keycode, conn.setup().max_keycode);
        if conn
            .extension_information(x11rb::protocol::xtest::X11_EXTENSION_NAME)?
            .is_none()
        {
            bail!(
                "the X server {display} has no XTEST extension, so keyboard and mouse input cannot be injected"
            );
        }
        conn.xtest_get_version(2, 2)?
            .reply()
            .with_context(|| format!("the X server {display} refused XTEST"))?;
        let mapping = conn
            .get_keyboard_mapping(first, last - first + 1)?
            .reply()
            .context("cannot read the X server's keyboard mapping")?;
        Ok(Self {
            conn,
            root,
            display: display.to_owned(),
            size,
            keys: KeyTable::new(
                first,
                usize::from(mapping.keysyms_per_keycode),
                &mapping.keysyms,
            ),
        })
    }
    /// The keycode of `keysym` and whether Shift goes with it, None when the server's
    /// keyboard has no such key.
    pub(super) fn keycode(&self, keysym: u32) -> Option<(u8, bool)> {
        self.keys.find(keysym)
    }
    fn fake(&self, kind: u8, detail: u8, x: i16, y: i16, root: Window) -> Result<()> {
        self.conn
            .xtest_fake_input(kind, detail, 0, root, x, y, 0)?
            .check()
            .context("fake input")?;
        Ok(())
    }
    /// Sends everything queued; the caller's batch ends here.
    pub(super) fn flush(&self) -> Result<()> {
        self.conn.flush().context("the X connection broke")
    }
    pub(super) fn key(&self, keycode: u8, down: bool) -> Result<()> {
        self.fake(if down { KEY_PRESS } else { KEY_RELEASE }, keycode, 0, 0, 0)
    }
    pub(super) fn button(&self, button: u8, down: bool) -> Result<()> {
        self.fake(
            if down { BUTTON_PRESS } else { BUTTON_RELEASE },
            button,
            0,
            0,
            0,
        )
    }
    /// Moves the pointer by `(dx, dy)` pixels.
    pub(super) fn motion_relative(&self, dx: i16, dy: i16) -> Result<()> {
        self.fake(MOTION_NOTIFY, 1, dx, dy, 0)
    }
    /// Puts the pointer at `(x, y)` of the screen.
    pub(super) fn motion_absolute(&self, x: i16, y: i16) -> Result<()> {
        self.fake(MOTION_NOTIFY, 0, x, y, self.root)
    }
}
