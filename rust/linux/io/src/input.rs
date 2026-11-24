//! Input injection for streams: keyboard, mouse, touch, pen and virtual gamepads. Linux has
//! no injection backend yet (`uinput` and `uhid` are not driven), so an [`Injector`] cannot
//! be made, the host offers clients no native touch, pen or controller-touch input, and
//! virtual gamepads ignore what they are sent. The event and report types are the same as
//! on Windows.

use anyhow::Result;
use butterpollo_input_policy::input::Input as Event;
use std::{sync::Arc, time::Instant};

mod desktop;
mod gamepad_thread;
mod usbip_pad;

pub use desktop::{
    desktop_refused, follow_input_desktop, input_desktop_name, keep_on_input_desktop,
    on_input_desktop, secure_desktop_shown,
};
pub use gamepad_thread::{GamepadThread, PadReport};
pub use usbip_pad::UsbipDriver;

/// The input feature flags the host offers a client: native pen and touch (1), controller
/// touch (2), touchpad events (0x10) and touchpad frames (0x20). None on Linux, where none
/// of them can be injected yet.
pub fn capabilities(config: &butterpollo_config::config::Config) -> u32 {
    let _ = config;
    0
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

/// One stream's keyboard, mouse, touch, pen and controller input. None can be made on Linux
/// yet: every constructor fails, and the stream reports that its controls are unavailable.
pub struct Injector {
    /// The controllers a controller session lent ([`Self::use_gamepads`]).
    gamepads: Option<Arc<GamepadThread>>,
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
    /// The injector for the display `output` with the `gamepad` profile and the input
    /// settings of `config`. Fails on Linux, after the settings were checked: input cannot
    /// be injected there yet.
    pub fn new_options_reported(
        output: &str,
        profile: &str,
        config: &butterpollo_config::config::Config,
        warnings: Arc<butterpollo_session::session::Warnings>,
    ) -> Result<Self> {
        let _ = (output, profile, warnings);
        butterpollo_input_policy::input_policy::Policy::resolve(config)?;
        Err(crate::unsupported("Input injection"))
    }
    /// The keyboard layout keys the client did not normalize go with (None: the host's).
    /// Unused on Linux, where no key is injected.
    pub fn set_keyboard_layout(&mut self, layout: Option<crate::keylayout::Layout>) {
        let _ = layout;
    }
    /// Each pass: the layout set last takes over once no key is held. Nothing to follow on
    /// Linux.
    pub fn follow_keyboard_layout(&mut self) {}
    /// The stream's size, for placing absolute input. Unused on Linux, where none is
    /// injected.
    pub fn set_stream_size(&mut self, width: u32, height: u32) {
        let _ = (width, height);
    }
    /// The host cursor as fractions of the whole stream. None on Linux, where the cursor's
    /// position is not read.
    pub fn pointer_in_stream(&mut self) -> Option<(f32, f32)> {
        None
    }
    /// Map absolute input onto `output` from now on. Unused on Linux, where none is
    /// injected.
    pub fn set_output(&mut self, output: &str) {
        let _ = output;
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
    /// Inject one event. Fails on Linux, where input cannot be injected yet.
    pub fn apply(&mut self, e: &Event) -> Result<()> {
        let mut errors = self.apply_all(std::slice::from_ref(e));
        errors.pop().map_or(Ok(()), Err)
    }
    /// Apply a pass of events in order; returns why events failed. On Linux every event
    /// fails: input cannot be injected yet.
    pub fn apply_all(&mut self, events: &[Event]) -> Vec<anyhow::Error> {
        events
            .iter()
            .map(|_| crate::unsupported("Input injection"))
            .collect()
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
    /// The socket to wake when the controllers have feedback. Unused on Linux, where they
    /// never have any.
    pub fn set_feedback_waker(&mut self, waker: Arc<std::net::UdpSocket>) {
        drop(waker);
    }
    /// Whether feedback of `kind` may go to the client. Rumble and haptics (0x010b, 0x5500,
    /// 0x5503, 0x5504, 0x5601) are withheld on Linux: no pad there makes any.
    pub fn feedback_allowed(&self, kind: u16) -> bool {
        !matches!(kind, 0x010b | 0x5500 | 0x5503 | 0x5504 | 0x5601)
    }
    /// Use a controller session's controllers from now on.
    pub fn use_gamepads(&mut self, gamepads: Arc<GamepadThread>) {
        gamepads.activate();
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
    /// controllers. Nothing is held on Linux.
    pub fn release_keyboard_mouse(&mut self) {}
    /// Lets go of everything this injector holds. Nothing is held on Linux.
    pub fn release_all(&mut self) {}
}
