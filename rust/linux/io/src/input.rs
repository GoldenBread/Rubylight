//! Input injection for streams: keyboard and mouse into the game's gamescope session through
//! XTEST, and virtual gamepads: an Xbox 360 pad on `/dev/uinput`, or a DualSense on `/dev/uhid`
//! that the kernel gives motion sensors and a touchpad, both with rumble back to the client
//! (the DualSense also lights and adaptive triggers). Touch, pen and a tablet's touchpad are
//! not injected yet, so the host offers clients none of them. The event and report types are
//! the same as on Windows.

use anyhow::Result;
use butterpollo_input_policy::input::Input as Event;
use std::{sync::Arc, time::Instant};

mod desktop;
mod evdev_desktop;
mod evdev_keys;
mod feedback;
mod ff;
mod gamepad_thread;
mod hid_pad;
mod keyboard_mouse;
mod keymap;
#[cfg(test)]
mod pad_tests;
#[cfg(test)]
mod tests;
mod uhid;
mod uinput;
mod usbip_pad;
mod xserver;

pub use desktop::{
    desktop_refused, follow_input_desktop, input_desktop_name, keep_on_input_desktop,
    on_input_desktop, secure_desktop_shown,
};
pub use gamepad_thread::{GamepadThread, PadReport};
pub use usbip_pad::UsbipDriver;

/// The input feature flags the host offers a client: native pen and touch (1), controller
/// touch (2), touchpad events (0x10) and touchpad frames (0x20). On Linux only controller
/// touch, which the virtual DualSense takes (a DualSense has one touch surface, a client's
/// second touchpad goes on its right half), when `/dev/uhid` can be written and the `gamepad`
/// setting does not force an Xbox pad. Pen, touch and tablet touchpads cannot be injected yet.
pub fn capabilities(config: &butterpollo_config::config::Config) -> u32 {
    let mut flags = 0;
    if config.boolean("controller", true)
        && !matches!(
            config.get("gamepad", "auto"),
            "vhf_xbox" | "vhf_xbox_one" | "vhf_switch" | "usbip_switch2" | "vigem_x360"
        )
        && hid_pad::available()
    {
        flags |= 2;
    }
    flags
}

/// How absolute input finds where it lands: the desktop rectangle of a streamed window's
/// captured area, as `[left, top, right, bottom]`, and the display name of a display given
/// by its device id.
#[derive(Clone, Copy)]
pub struct RectHooks {
    pub window_rect: fn(&str) -> Option<[i32; 4]>,
    pub display_name_for_id: fn(&str) -> Option<String>,
}
static RECT_HOOKS: std::sync::OnceLock<RectHooks> = std::sync::OnceLock::new();
/// Install the hooks, once. On Linux nothing asks them yet, as no absolute input is
/// injected.
pub fn set_rect_hooks(hooks: RectHooks) {
    let _ = RECT_HOOKS.set(hooks);
}

/// One stream's keyboard, mouse and controller input. Keys and the mouse go into the game's
/// gamescope session through XTEST, connected when a session runs; controllers are virtual
/// pads of `/dev/uinput`. Touch and pen are not injected.
pub struct Injector {
    /// The controllers a controller session lent ([`Self::use_gamepads`]), or the injector's
    /// own once a controller sends input.
    gamepads: Option<Arc<GamepadThread>>,
    policy: butterpollo_input_policy::input_policy::Policy,
    profile: u16,
    warnings: Arc<butterpollo_session::session::Warnings>,
    keyboard_mouse: keyboard_mouse::KeyboardMouse,
    /// Whether the client wants haptics (and so rumble), which its `Haptics` event sets.
    haptics: bool,
    feedback_waker: Option<Arc<std::net::UdpSocket>>,
}

impl Injector {
    pub fn new(output: &str, profile: &str) -> Result<Self> {
        Self::new_options(
            output,
            profile,
            &butterpollo_config::config::Config::default(),
        )
    }
    pub fn new_options(
        output: &str,
        profile: &str,
        config: &butterpollo_config::config::Config,
    ) -> Result<Self> {
        Self::new_options_reported(output, profile, config, Default::default())
    }
    /// The injector with the `gamepad` profile and the input settings of `config`. The display
    /// `output` is the stream's capture target: `desktop` and `desktop:<n>` send keyboard and mouse to
    /// the user's desktop through `/dev/uinput`; any other goes to the newest gamescope session, whenever one runs.
    /// Fails only for settings that cannot be resolved.
    pub fn new_options_reported(
        output: &str,
        profile: &str,
        config: &butterpollo_config::config::Config,
        warnings: Arc<butterpollo_session::session::Warnings>,
    ) -> Result<Self> {
        Ok(Self {
            gamepads: None,
            policy: butterpollo_input_policy::input_policy::Policy::resolve(config)?,
            profile: butterpollo_input_policy::input_policy::gamepad_profile(profile),
            warnings,
            keyboard_mouse: keyboard_mouse::KeyboardMouse::for_output(output),
            haptics: true,
            feedback_waker: None,
        })
    }
    /// The keyboard layout keys the client did not normalize go with (None: the host's).
    /// Unused on Linux, where no key is injected.
    pub fn set_keyboard_layout(&mut self, layout: Option<crate::keylayout::Layout>) {
        let _ = layout;
    }
    /// Each pass: the layout set last takes over once no key is held. Nothing to follow on
    /// Linux.
    pub fn follow_keyboard_layout(&mut self) {}
    /// The stream's size, for placing absolute input that carries no size of its own.
    pub fn set_stream_size(&mut self, width: u32, height: u32) {
        self.keyboard_mouse.stream = Some((width, height));
    }
    /// The host cursor as fractions of the whole stream. None on Linux, where the cursor's
    /// position is not read.
    pub fn pointer_in_stream(&mut self) -> Option<(f32, f32)> {
        None
    }
    /// Map absolute input onto `output` from now on. Unused on Linux, where none is
    /// injected.
    pub fn set_output(&mut self, output: &str) {
        self.keyboard_mouse.set_output(output);
    }
    /// Release a left button whose release was held back, once it is due. None is ever held
    /// on Linux.
    pub fn flush_virtual_left_release(&mut self) -> Result<()> {
        Ok(())
    }
    /// Keep held touches and the pen alive and read the display's rectangle again. Nothing
    /// is held on Linux.
    pub fn refresh(&mut self) -> Result<()> {
        Ok(())
    }
    /// When [`Self::due`] next has a delayed release or a key repeat to send: never on Linux.
    pub fn next_due(&self) -> Option<Instant> {
        None
    }
    /// Send the delayed releases and key repeats that are due. None ever is on Linux.
    pub fn due(&mut self) -> Result<()> {
        Ok(())
    }
    /// Inject one event.
    pub fn apply(&mut self, e: &Event) -> Result<()> {
        let mut errors = self.apply_all(std::slice::from_ref(e));
        errors.pop().map_or(Ok(()), Err)
    }
    /// Apply a pass of events in order; returns why events failed. Keyboard and mouse input
    /// with no gamescope session to take it is dropped, not failed; touch, pen and controller
    /// touchpads fail, as they cannot be injected.
    pub fn apply_all(&mut self, events: &[Event]) -> Vec<anyhow::Error> {
        let mut errors = Vec::new();
        for e in events {
            if !self.policy.allows(e) {
                continue;
            }
            let result = match e {
                Event::Relative { .. }
                | Event::Absolute { .. }
                | Event::MouseButton { .. }
                | Event::Scroll { .. }
                | Event::Keyboard { .. }
                | Event::Text(_) => self.keyboard_mouse.apply(e, &self.policy, &self.warnings),
                Event::Touch { .. } | Event::Pen { .. } | Event::Touchpad { .. } => {
                    Err(crate::unsupported("Touch, pen and touchpad input"))
                }
                Event::Haptics(enabled) => {
                    self.haptics = *enabled;
                    Ok(())
                }
                _ => self.send_to_gamepads(e),
            };
            errors.extend(result.err());
        }
        errors.extend(self.keyboard_mouse.finish().err());
        errors
    }
    /// Controller input goes to the pads, which the injector starts if it has none yet.
    fn send_to_gamepads(&mut self, e: &Event) -> Result<()> {
        if self.gamepads.is_none() {
            let pads =
                GamepadThread::new(self.profile, self.policy.clone(), self.warnings.clone())?;
            if let Some(waker) = &self.feedback_waker {
                pads.set_feedback_waker(waker.clone());
            }
            self.gamepads = Some(Arc::new(pads));
        }
        if let Some(pads) = &self.gamepads {
            pads.send(e.clone());
        }
        Ok(())
    }
    /// Audio for a controller's speaker or haptics, handed to the controllers.
    pub fn controller_audio(&self, id: u16, kind: u16, payload: &[u8]) {
        if let Some(gamepads) = &self.gamepads {
            gamepads.audio(id, kind, payload);
        }
    }
    pub fn controller_audio_ready(&self, id: u16) -> bool {
        self.gamepads
            .as_ref()
            .is_some_and(|gamepads| gamepads.controller_audio_ready(id))
    }
    /// Feedback and motion-sensor requests from the virtual gamepads for the client,
    /// collected since the last call.
    pub fn gamepad_reports(&self) -> impl Iterator<Item = PadReport> + '_ {
        self.gamepads.iter().flat_map(|gamepads| gamepads.reports())
    }
    /// Whether the controllers were handed over ([`Self::use_gamepads`]).
    pub fn has_gamepads(&self) -> bool {
        self.gamepads.is_some()
    }
    /// The socket to wake when the controllers have feedback.
    pub fn set_feedback_waker(&mut self, waker: Arc<std::net::UdpSocket>) {
        if let Some(gamepads) = &self.gamepads {
            gamepads.set_feedback_waker(waker.clone());
        }
        self.feedback_waker = Some(waker);
    }
    /// Whether feedback of `kind` may go to the client. Rumble and haptics (0x010b, 0x5500,
    /// 0x5503, 0x5504, 0x5601) go when the host forwards rumble and the client wants haptics,
    /// as on Windows.
    pub fn feedback_allowed(&self, kind: u16) -> bool {
        !matches!(kind, 0x010b | 0x5500 | 0x5503 | 0x5504 | 0x5601)
            || (self.policy.forward_rumble && self.haptics)
    }
    /// Use a controller session's controllers from now on.
    pub fn use_gamepads(&mut self, gamepads: Arc<GamepadThread>) {
        gamepads.activate();
        if let Some(waker) = &self.feedback_waker {
            gamepads.set_feedback_waker(waker.clone());
        }
        self.gamepads = Some(gamepads);
    }
    /// Leave the controllers to their session, parked for the client's return.
    pub fn detach_gamepads(&mut self) {
        if let Some(gamepads) = &self.gamepads {
            gamepads.park();
            if Arc::strong_count(gamepads) == 1 {
                drop(gamepads.stop());
            }
        }
    }
    /// Lets go of the keys, mouse buttons and touches this injector holds and keeps its
    /// controllers.
    pub fn release_keyboard_mouse(&mut self) {
        if let Err(error) = self.keyboard_mouse.release() {
            tracing::debug!(%error, "keyboard and mouse release failed");
        }
    }
    /// Lets go of everything this injector holds: its keys and buttons and its controllers'
    /// controls.
    pub fn release_all(&mut self) {
        self.release_keyboard_mouse();
        if let Some(gamepads) = &self.gamepads {
            gamepads.park();
        }
    }
}
