//! Keyboard and mouse injection into the game's gamescope session, or, when the stream's target
//! is the user's own desktop, into the desktop through `/dev/uinput`. The session's Xwayland
//! may start after the stream does, so the connection is made when a session shows up and
//! made again when another replaces it; input meanwhile is dropped with a debug log.

use super::{
    Event,
    evdev_desktop::DesktopInput,
    keymap::{char_keysym, keysym, scale_absolute, scroll_notches, x_button},
    xserver::XServer,
};
use anyhow::{Context, Result};
use butterpollo_input_policy::input_policy::Policy;
use butterpollo_session::session::Warnings;
use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant},
};

/// How often the session's display is looked up again.
const LOOKUP: Duration = Duration::from_secs(1);
const MODIFIER_EXTENDED: u8 = 0x10;
const WARNING: &str = "input_keyboard_mouse";
/// Shift, Ctrl and Alt as a key-down packet's modifier bits and the virtual keys that hold them.
const MODIFIERS: [(u8, [u16; 3], u16); 3] = [
    (0x01, [0x10, 0xa0, 0xa1], 0xa0),
    (0x02, [0x11, 0xa2, 0xa3], 0xa2),
    (0x04, [0x12, 0xa4, 0xa5], 0xa4),
];

#[derive(Default)]
pub(super) struct KeyboardMouse {
    /// The desktop's virtual devices, while the stream's target is the desktop (this replaces
    /// the X connection).
    desktop: Option<DesktopInput>,
    server: Option<XServer>,
    looked_up: Option<Instant>,
    /// The display that failed, so its error is raised once.
    failed: Option<String>,
    /// The stream's size, for absolute input without a size of its own.
    pub(super) stream: Option<(u32, u32)>,
    /// A display to use instead of the newest session's, for tests that run beside others.
    #[cfg(test)]
    pub(super) display: Option<String>,
    /// Host keys held by virtual key, with the keycode pressed.
    keys: BTreeMap<u16, u8>,
    /// Client keys by the host key they hold.
    sources: BTreeMap<u16, u16>,
    buttons: BTreeSet<u8>,
    scroll: [i32; 2],
}

impl KeyboardMouse {
    /// Input for a stream whose capture target is `output`: the desktop's devices for
    /// `desktop` and `desktop:<n>`, the gamescope session's X server otherwise.
    pub(super) fn for_output(output: &str) -> Self {
        let mut keyboard_mouse = Self::default();
        keyboard_mouse.set_output(output);
        keyboard_mouse
    }
    /// Follows a change of the stream's target.
    pub(super) fn set_output(&mut self, output: &str) {
        match (crate::portal::desktop_target(output), &mut self.desktop) {
            (Some(monitor), None) => self.desktop = Some(DesktopInput::new(monitor)),
            (Some(_), Some(_)) => {}
            (None, Some(desktop)) => {
                let _ = desktop.release();
                self.desktop = None;
            }
            (None, None) => {}
        }
    }
    /// The connection to the newest gamescope session, made or remade when due.
    fn server(&mut self, warnings: &Warnings) -> Option<&XServer> {
        let due = self.looked_up.is_none_or(|at| at.elapsed() >= LOOKUP);
        if due {
            self.looked_up = Some(Instant::now());
            let name = crate::gamescope::current().and_then(|session| session.display);
            #[cfg(test)]
            let name = self.display.clone().or(name);
            match name {
                Some(name) if self.server.as_ref().is_none_or(|s| s.display != name) => {
                    // What the old server held went with it.
                    self.keys.clear();
                    self.sources.clear();
                    self.buttons.clear();
                    match XServer::connect(&name) {
                        Ok(server) => {
                            tracing::info!(display = %name, "keyboard and mouse input goes to the gamescope session");
                            self.server = Some(server);
                            self.failed = None;
                            warnings.clear(WARNING);
                        }
                        Err(error) => {
                            self.server = None;
                            if self.failed.as_deref() != Some(&name) {
                                self.failed = Some(name);
                                warnings.set(
                                    WARNING,
                                    format!("{error:#}; keyboard and mouse input is ignored."),
                                );
                            }
                        }
                    }
                }
                None => self.server = None,
                Some(_) => {}
            }
        }
        if self.server.is_none() {
            tracing::debug!("no gamescope session to inject keyboard and mouse input into");
        }
        self.server.as_ref()
    }
    /// Inject one event. None is sent before [`Self::finish`].
    pub(super) fn apply(&mut self, e: &Event, policy: &Policy, warnings: &Warnings) -> Result<()> {
        if let Some(desktop) = &mut self.desktop {
            return desktop.apply(e, policy);
        }
        if self.server(warnings).is_none() {
            return Ok(());
        }
        let result = self.stage(e, policy);
        if result.is_err() {
            // A broken connection is made again at the next lookup.
            self.server = None;
            self.looked_up = None;
        }
        result
    }
    fn stage(&mut self, e: &Event, policy: &Policy) -> Result<()> {
        if let Event::Keyboard {
            key,
            modifiers,
            down,
            ..
        } = e
        {
            return self.key(*key, *modifiers, *down, policy);
        }
        let Some(server) = &self.server else {
            return Ok(());
        };
        match e {
            Event::Relative { x, y } => server.motion_relative(*x, *y)?,
            Event::Absolute {
                x,
                y,
                width,
                height,
            } => {
                let (w, h) = server.size;
                server.motion_absolute(
                    scale_absolute(*x, *width, w),
                    scale_absolute(*y, *height, h),
                )?;
            }
            Event::MouseButton { button, down } => {
                let Some(code) = x_button(*button) else {
                    return Ok(());
                };
                if *down == self.buttons.contains(button) {
                    return Ok(());
                }
                server.button(code, *down)?;
                if *down {
                    self.buttons.insert(*button);
                } else {
                    self.buttons.remove(button);
                }
            }
            Event::Scroll { amount, horizontal } => {
                let notches = scroll_notches(&mut self.scroll[usize::from(*horizontal)], *amount);
                let button = match (horizontal, notches > 0) {
                    (false, true) => 4,
                    (false, false) => 5,
                    (true, true) => 7,
                    (true, false) => 6,
                };
                for _ in 0..notches.unsigned_abs() {
                    server.button(button, true)?;
                    server.button(button, false)?;
                }
            }
            Event::Text(text) => {
                for c in text.chars() {
                    let Some((code, shift)) = server.keycode(char_keysym(c)) else {
                        tracing::debug!(%c, "no key types this character");
                        continue;
                    };
                    let shift_code = server.keycode(0xffe1).map(|(code, _)| code);
                    let shift = shift.then_some(shift_code).flatten();
                    if let Some(shift) = shift {
                        server.key(shift, true)?;
                    }
                    server.key(code, true)?;
                    server.key(code, false)?;
                    if let Some(shift) = shift {
                        server.key(shift, false)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn key(&mut self, client: u16, modifiers: u8, down: bool, policy: &Policy) -> Result<()> {
        let Some(server) = &self.server else {
            return Ok(());
        };
        if !down {
            let Some(key) = self.sources.remove(&client) else {
                return Ok(());
            };
            // Two client keys may map to one host key: it stays down until the last lets go.
            if self.sources.values().any(|held| *held == key) {
                return Ok(());
            }
            if let Some(code) = self.keys.remove(&key) {
                server.key(code, false)?;
            }
            return Ok(());
        }
        if self.sources.contains_key(&client) {
            return Ok(());
        }
        let key = policy.key(client);
        let keypad = key == 0x0d && modifiers & MODIFIER_EXTENDED != 0;
        let Some((code, shifted)) = keysym(key, keypad).and_then(|sym| server.keycode(sym)) else {
            tracing::debug!(
                key = format_args!("{key:#x}"),
                "no key on the X keyboard for this virtual key"
            );
            return Ok(());
        };
        self.sources.insert(client, key);
        if self.keys.contains_key(&key) {
            return Ok(());
        }
        // Shift, Ctrl and Alt the client reported without pressing them go around this key only.
        let mut around = Vec::new();
        if !matches!(key, 0x10..=0x12 | 0xa0..=0xa5 | 0x5b | 0x5c) {
            // A symbol the keyboard has only with Shift (digits on a French layout).
            let shift_held = MODIFIERS[0].1.iter().any(|vk| self.keys.contains_key(vk));
            if shifted
                && !shift_held
                && let Some((shift, _)) = server.keycode(0xffe1)
            {
                around.push(shift);
            }
            for (bit, alternatives, press) in MODIFIERS {
                if modifiers & bit != 0
                    && (bit != 0x01 || !shifted)
                    && !alternatives.iter().any(|vk| self.keys.contains_key(vk))
                    && let Some((modifier, _)) =
                        keysym(press, false).and_then(|s| server.keycode(s))
                {
                    around.push(modifier);
                }
            }
        }
        for modifier in &around {
            server.key(*modifier, true)?;
        }
        server.key(code, true)?;
        for modifier in around.iter().rev() {
            server.key(*modifier, false)?;
        }
        self.keys.insert(key, code);
        Ok(())
    }
    /// Sends what the events of a pass queued.
    pub(super) fn finish(&mut self) -> Result<()> {
        if let Some(desktop) = &mut self.desktop {
            return desktop.finish();
        }
        let Some(server) = &self.server else {
            return Ok(());
        };
        let result = server.flush();
        if result.is_err() {
            self.server = None;
            self.looked_up = None;
        }
        result
    }
    /// Lets go of every held key and button.
    pub(super) fn release(&mut self) -> Result<()> {
        if let Some(desktop) = &mut self.desktop {
            return desktop.release();
        }
        let keys = std::mem::take(&mut self.keys);
        let buttons = std::mem::take(&mut self.buttons);
        self.sources.clear();
        self.scroll = [0; 2];
        let Some(server) = &self.server else {
            return Ok(());
        };
        let mut result = Ok(());
        for code in keys.into_values() {
            result = result.and(server.key(code, false));
        }
        for button in buttons {
            if let Some(code) = x_button(button) {
                result = result.and(server.button(code, false));
            }
        }
        result
            .and_then(|()| server.flush())
            .context("releasing held keys and buttons")
    }
}
