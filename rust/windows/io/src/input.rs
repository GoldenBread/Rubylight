//! Input event dispatch and shared per-session injection state.

use anyhow::{Context, Result, bail};
use butterpollo_input_policy::input::Input as Event;
use butterpollo_input_policy::input_policy::{VHF_AUTO, gamepad_profile};
use std::{
    collections::{BTreeMap, BTreeSet},
    mem::size_of,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Devices::DeviceAndDriverInstallation::*,
        Foundation::*,
        Storage::FileSystem::*,
        System::StationsAndDesktops::*,
        UI::{
            Controls::*,
            Input::{KeyboardAndMouse::*, Pointer::*},
            WindowsAndMessaging::*,
        },
    },
    core::{GUID, PCWSTR},
};

mod desktop;
mod dispatch;
mod display_rect;
mod dualsense_pad;
mod gamepad_backend;
#[path = "input/gamepad_thread.rs"]
mod gamepad_thread;
mod gamepads;
mod keyboard_mouse;
mod steam_deck_pad;
mod switch2_pad;
mod touch_pen;
mod usbip_pad;
mod vigem;
mod virtual_hid;
mod virtual_route;

pub use desktop::{
    desktop_refused, follow_input_desktop, input_desktop_name, keep_on_input_desktop,
    on_input_desktop, secure_desktop_shown,
};
use display_rect::{Lookup, current_rect, rect_interval, reread_rect};
#[cfg(test)]
use display_rect::{RECT_INTERVAL, WINDOW_RECT_INTERVAL, cursor_in_stream, display_rect};
pub use display_rect::{RectHooks, set_rect_hooks};
/// Gated fake pads for other modules' tests.
#[cfg(test)]
pub(crate) use gamepad_thread::tests::fake as fake_pads;
pub use gamepad_thread::{GamepadThread, PadReport};
pub use gamepads::{Gamepads, open_interface};
use keyboard_mouse::{
    EXPLICIT_EXTENDED_KEY, inject, is_modifier, keyboard_identity, mapped_keyboard_identity,
};
use touch_pen::{inject_pointer, refresh_due};
pub use usbip_pad::UsbipDriver;

static HELD: std::sync::Mutex<BTreeMap<(bool, u32), usize>> =
    std::sync::Mutex::new(BTreeMap::new());
/// Separates virtual-mouse button identities from SendInput button identities in [`HELD`]
/// so mixed-mode multi-client streams never suppress or misroute each other's presses.
const VIRTUAL_BUTTON_FLAG: u32 = 1 << 16;
/// Keeps sends in the order of their HELD updates without holding HELD through
/// SendInput, which another injector's thread waited behind: a delayed send holds it
/// from its update to its send, and a pass's send waits for it. Taken before HELD.
static SEND_ORDER: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn capabilities(config: &butterpollo_config::config::Config) -> u32 {
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    let mut flags = 0;
    if config.boolean("mouse", true)
        && config.boolean("native_pen_touch", true)
        // SAFETY: The module and symbol names are terminated static strings; the returned address is only checked for presence.
        && unsafe {
            GetModuleHandleW(windows::core::w!("user32.dll")).is_ok_and(|module| {
                GetProcAddress(module, windows::core::s!("CreateSyntheticPointerDevice")).is_some()
            })
        }
    {
        flags |= 1;
    }
    if config.boolean("controller", true)
        && !matches!(
            config.get("gamepad", "auto"),
            "vhf_xbox" | "vhf_xbox_one" | "vhf_switch" | "usbip_switch2" | "vigem_x360"
        )
    {
        // Moonlight has one general controller-touch feature bit; there is
        // no separate host bit for dual touchpads. The virtual DualShock 4 and
        // DualSense have one touch surface, so a client's second touchpad
        // (a Steam Deck's right trackpad) is placed on its right half; apply()
        // ignores any other touchpad.
        flags |= 2;
    }
    if config.boolean("mouse", true)
        && config.boolean("native_touchpad", true)
        && usbip_pad::usbip_exe().is_some()
    {
        // A tablet's physical touchpad, as Moonlight V+'s and Foundation Sunshine's packets
        // carry it: one contact (LI_FF_TOUCHPAD_EVENTS) or a whole frame
        // (LI_FF_TOUCHPAD_FRAME_EVENTS) each. It replays on the virtual Precision Touchpad,
        // which needs usbip-win2; without it the client keeps its touchpad a mouse.
        flags |= 0x10 | 0x20;
    }
    flags
}

pub struct Injector {
    keys: BTreeSet<u32>,
    key_sources: BTreeMap<u32, u32>,
    buttons: BTreeSet<u8>,
    /// Subset of `buttons` pressed via the virtual mouse, so releases always return
    /// to the exact route that pressed them even if readiness changes or another client
    /// uses SendInput.
    virtual_buttons: BTreeSet<u8>,
    touches: BTreeMap<u32, POINTER_TOUCH_INFO>,
    /// The pass's touch moves not yet injected, by client id: they go out in one frame.
    touch_moves: BTreeMap<u32, POINTER_TOUCH_INFO>,
    /// Touches on their way to mouse input, with a mouse or trackpad touch profile.
    touch_profile: butterpollo_input_policy::touch_profile::TouchProfile,
    touch_device: Option<HSYNTHETICPOINTERDEVICE>,
    pen_device: Option<HSYNTHETICPOINTERDEVICE>,
    pen: POINTER_PEN_INFO,
    /// When touch contacts and the pen were last injected.
    touch_refreshed: std::time::Instant,
    pen_refreshed: std::time::Instant,
    /// The controllers' thread: a controller session's ([`Self::use_gamepads`]), or one of
    /// its own started with the first controller event.
    gamepads: Option<std::sync::Arc<GamepadThread>>,
    /// The `gamepad` profile its own thread opens.
    gamepad_profile: u16,
    /// What wakes the control socket wait, for whichever thread the controllers get.
    feedback_waker: Option<std::sync::Arc<std::net::UdpSocket>>,
    /// The display's desktop rectangle, None until the display exists.
    rect: Option<RECT>,
    policy: butterpollo_input_policy::input_policy::Policy,
    /// The layout that gives keys the client did not normalize their scan codes (None:
    /// the host's), and the one asked for since. It takes over once no key is held, so
    /// a release sends its press's scan code.
    layout: Option<crate::keylayout::Layout>,
    next_layout: Option<crate::keylayout::Layout>,
    key_flags: BTreeMap<u32, u8>,
    /// Repeating key, its flags, the modifiers pressed around it at key-down
    /// and when it repeats.
    repeat: Option<(u32, u8, u8, std::time::Instant)>,
    scroll: [i32; 2],
    haptics: bool,
    /// The display absolute input maps onto, and when its rectangle was read.
    output: String,
    /// The stream's size: clients place absolute input on the whole stream,
    /// including the bars around a display of another shape.
    stream: Option<(u32, u32)>,
    rect_read: std::time::Instant,
    lookup: Option<Lookup>,
    /// When absolute input was last reported ignored for a missing display.
    display_warned: Option<std::time::Instant>,
    /// Whether the client last moved the mouse by absolute position, and a
    /// left-button release held back meanwhile.
    absolute: bool,
    left_release: Option<std::time::Instant>,
    left_release_virtual: std::cell::Cell<bool>,
    forced_route: std::cell::Cell<Option<MouseRoute>>,
    pending: Pending,
    /// SendInput, or a stand-in in tests.
    inject: fn(&[INPUT]) -> usize,
    /// `inject_pointer` for touch frames, or a stand-in in tests.
    inject_touch: unsafe fn(HSYNTHETICPOINTERDEVICE, &[POINTER_TYPE_INFO]) -> Result<()>,
    warnings: std::sync::Arc<butterpollo_session::session::Warnings>,
    /// This stream holds a share of the virtual mouse (`mouse_mode` auto or virtual_mouse):
    /// mouse input goes to it while it is plugged in.
    virtual_mouse: bool,
    /// Whether mouse input last went to the virtual mouse, to log when that changes.
    virtual_mouse_routed: std::sync::atomic::AtomicBool,
    /// This stream holds a share of the virtual touchpad (`native_touchpad`).
    touchpad: bool,
    /// The touchpad's plugging was reported.
    touchpad_reported: bool,
    /// The code of the warning last set for the touchpad, so each packet need not look it up.
    touchpad_warned: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum MouseRoute {
    /// Moves, scrolls, and unowned pairs: virtual mouse when ready, otherwise SendInput.
    #[default]
    Auto,
    /// Pressed via SendInput; must release via SendInput.
    Injection,
    /// Pressed via the virtual mouse; must release via the virtual mouse.
    Virtual,
}

/// Keyboard and mouse input of one pass of the control loop, sent in one
/// SendInput call: a call cost 31-36 us whether it carried one input or
/// eight.
#[derive(Default)]
struct Pending {
    inputs: Vec<INPUT>,
    /// Parallel to `inputs`: the delivery route each input requires.
    routes: Vec<MouseRoute>,
    /// Each event's end in `inputs`, and its press if it is one.
    events: Vec<(usize, Option<Press>)>,
    /// Why events of the pass failed.
    errors: Vec<anyhow::Error>,
}
impl Pending {
    fn sync_routes(&mut self) {
        if self.inputs.is_empty() {
            self.routes.clear();
        }
    }
    fn push_auto(&mut self, input: INPUT) {
        self.sync_routes();
        self.inputs.push(input);
        self.routes.push(MouseRoute::Auto);
    }
    fn push_routed(&mut self, input: INPUT, route: MouseRoute) {
        self.sync_routes();
        self.inputs.push(input);
        self.routes.push(route);
    }
}

/// A key or button press, recorded as held once Windows takes it.
struct Press {
    keyboard: bool,
    id: u32,
    /// Unmapped keyboard identity; unused for mouse presses.
    source: u32,
    flags: u8,
    /// Whether it was counted in HELD, to count it out if Windows refuses it.
    counted: bool,
    /// The modifiers pressed around a key that repeats.
    repeat: Option<u8>,
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
    pub fn new_options_reported(
        output: &str,
        profile: &str,
        config: &butterpollo_config::config::Config,
        warnings: std::sync::Arc<butterpollo_session::session::Warnings>,
    ) -> Result<Self> {
        let profile = gamepad_profile(profile);
        let policy = butterpollo_input_policy::input_policy::Policy::resolve(config)?;
        // Keyboard, relative mouse and controllers must work before the
        // stream's display exists: a controller's arrival is sent only once.
        let mut injector = Self {
            keys: BTreeSet::new(),
            key_sources: BTreeMap::new(),
            buttons: BTreeSet::new(),
            virtual_buttons: BTreeSet::new(),
            touches: BTreeMap::new(),
            touch_moves: BTreeMap::new(),
            touch_profile: Default::default(),
            touch_device: None,
            pen_device: None,
            pen: Default::default(),
            touch_refreshed: std::time::Instant::now(),
            pen_refreshed: std::time::Instant::now(),
            gamepads: None,
            gamepad_profile: profile,
            feedback_waker: None,
            policy,
            layout: None,
            next_layout: None,
            key_flags: BTreeMap::new(),
            repeat: None,
            scroll: [0; 2],
            haptics: true,
            rect: None,
            output: output.to_owned(),
            stream: None,
            rect_read: std::time::Instant::now(),
            lookup: None,
            display_warned: None,
            absolute: false,
            left_release: None,
            left_release_virtual: std::cell::Cell::new(false),
            forced_route: std::cell::Cell::new(None),
            pending: Pending::default(),
            inject,
            inject_touch: inject_pointer,
            warnings,
            virtual_mouse: false,
            virtual_mouse_routed: std::sync::atomic::AtomicBool::new(false),
            touchpad: false,
            touchpad_reported: false,
            touchpad_warned: None,
        };
        injector.look_up(output);
        injector.start_virtual_devices();
        Ok(injector)
    }
    /// The keyboard layout keys the client did not normalize go with (None: the host's),
    /// read by the control loop from the device's report (0x553E) and `keyboard_layout`.
    /// It takes over at [`Self::follow_keyboard_layout`].
    pub fn set_keyboard_layout(&mut self, layout: Option<crate::keylayout::Layout>) {
        self.next_layout = layout;
    }
    /// Each pass: the layout set last takes over once no key is held, so a release sends
    /// its press's scan code.
    pub fn follow_keyboard_layout(&mut self) {
        let klid = |layout: &Option<crate::keylayout::Layout>| layout.as_ref().map(|l| l.klid());
        if self.keys.is_empty() && klid(&self.layout) != klid(&self.next_layout) {
            self.layout = self.next_layout.clone();
            tracing::info!(layout = ?self.layout, "keys follow a keyboard layout");
        }
    }
    pub fn flush_virtual_left_release(&mut self) -> Result<()> {
        if self.left_release_virtual.get()
            && self
                .left_release
                .is_some_and(|due| std::time::Instant::now() >= due)
        {
            return self.release_deferred_left(false);
        }
        Ok(())
    }
    pub fn refresh(&mut self) -> Result<()> {
        // Every step runs even when an earlier one fails; the first error is
        // reported. A failed step is not retried before its next due time.
        let mut first = None;
        let mut keep = |result: Result<()>| {
            if let Err(error) = result {
                first.get_or_insert(error);
            }
        };
        keep(self.flush_virtual_left_release());
        keep(self.due());
        let now = std::time::Instant::now();
        // A game can change its display's resolution or position mid-stream,
        // and the stream's display can appear after input began.
        self.settle_lookup(false);
        if self.rect_read.elapsed() >= rect_interval(&self.output) {
            if self.rect.is_none() {
                self.resolve_rect();
            } else {
                self.rect_read = now;
                self.rect = reread_rect(&self.output, self.rect, current_rect(&self.output));
            }
        }
        if refresh_due(&mut self.touch_refreshed, now) {
            keep(self.inject_touches());
        }
        if refresh_due(&mut self.pen_refreshed, now)
            && self.pen.pointerInfo.pointerFlags != POINTER_FLAG_NONE
            && let Some(device) = self.pen_device
        {
            // SAFETY: The injector still owns the pen device and the borrowed frame is initialized as pen input.
            keep(unsafe {
                inject_pointer(
                    device,
                    &[POINTER_TYPE_INFO {
                        r#type: PT_PEN,
                        Anonymous: POINTER_TYPE_INFO_0 { penInfo: self.pen },
                    }],
                )
            });
        }
        first.map_or(Ok(()), Err)
    }
    fn flush_and_sync(&mut self) {
        self.flush();
        self.pending.sync_routes();
        self.virtual_buttons
            .retain(|button| self.buttons.contains(button));
    }
    fn release_button_on_route(&self, button: u8, was_virtual: bool, teardown: bool) -> Result<()> {
        struct ForcedRouteReset<'a>(&'a std::cell::Cell<Option<MouseRoute>>);
        impl Drop for ForcedRouteReset<'_> {
            fn drop(&mut self) {
                self.0.set(None);
            }
        }
        let held_id = u32::from(button) | if was_virtual { VIRTUAL_BUTTON_FLAG } else { 0 };
        let route = if was_virtual {
            MouseRoute::Virtual
        } else {
            MouseRoute::Injection
        };
        self.forced_route.set(Some(route));
        let _reset = ForcedRouteReset(&self.forced_route);
        if teardown {
            self.release_owned(false, held_id, Self::button(button, false))
        } else {
            self.held(false, held_id, false, true, Self::button(button, false))
        }
    }
    fn release_deferred_left(&mut self, teardown: bool) -> Result<()> {
        if self.left_release.take().is_none() {
            return Ok(());
        }
        let was_virtual = self.left_release_virtual.replace(false);
        self.release_button_on_route(1, was_virtual, teardown)
    }
    /// The controllers' thread, started here when the injector has none yet: making an
    /// injector starts no thread a controller session's would replace at once.
    fn gamepads(&mut self) -> std::io::Result<&std::sync::Arc<GamepadThread>> {
        if self.gamepads.is_none() {
            let gamepads = GamepadThread::new(
                self.gamepad_profile,
                self.policy.clone(),
                self.warnings.clone(),
            )?;
            if let Some(waker) = &self.feedback_waker {
                gamepads.set_feedback_waker(waker.clone());
            }
            if !self.haptics {
                gamepads.set_haptics(false);
            }
            self.gamepads = Some(std::sync::Arc::new(gamepads));
        }
        Ok(self.gamepads.as_ref().expect("started above"))
    }
    /// Feedback and motion-sensor requests from the virtual gamepads for the
    /// client, collected since the last call.
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
    pub fn gamepad_reports(&self) -> impl Iterator<Item = PadReport> + '_ {
        self.gamepads.iter().flat_map(|gamepads| gamepads.reports())
    }
    /// Whether the controllers have their thread: a controller session's, or the injector's own.
    pub fn has_gamepads(&self) -> bool {
        self.gamepads.is_some()
    }
    /// Wake the control socket wait when the driver posts feedback. The socket
    /// must be connected and nonblocking before accepting controller input.
    pub fn set_feedback_waker(&mut self, waker: std::sync::Arc<std::net::UdpSocket>) {
        if let Some(gamepads) = &self.gamepads {
            gamepads.set_feedback_waker(waker.clone());
        }
        self.feedback_waker = Some(waker);
    }
    pub fn feedback_allowed(&self, kind: u16) -> bool {
        // 0x5504: Steam Controller haptics, withheld like rumble; its
        // settings (0x5505) are not feedback a player feels.
        !matches!(kind, 0x010b | 0x5500 | 0x5503 | 0x5504 | 0x5601)
            || (self.policy.forward_rumble && self.haptics)
    }
    /// The driver worker keeps ownership of its slots until unplug completes.
    /// Disconnecting a peer must not wait for that driver on the control thread.
    pub fn use_gamepads(&mut self, gamepads: std::sync::Arc<GamepadThread>) {
        if let Some(own) = self.gamepads.take() {
            drop(own.stop());
        }
        gamepads.activate();
        if let Some(waker) = &self.feedback_waker {
            gamepads.set_feedback_waker(waker.clone());
        }
        if !self.haptics {
            gamepads.set_haptics(false);
        }
        self.gamepads = Some(gamepads);
    }
    pub fn detach_gamepads(&mut self) {
        if let Some(gamepads) = &self.gamepads {
            gamepads.park();
            if std::sync::Arc::strong_count(gamepads) == 1 {
                drop(gamepads.stop());
            }
        }
    }
    /// Lets go of the keys, mouse buttons and touches this injector holds and keeps its
    /// controllers: a guest's stream that lost the keyboard and mouse to another holder
    /// (`butterpollo_session::multiclient::Kbm`).
    pub fn release_keyboard_mouse(&mut self) {
        if let Err(error) = self.release_pointers() {
            tracing::warn!(%error, "keyboard and mouse release incomplete");
        }
    }
    pub fn release_all(&mut self) {
        if let Some(gamepads) = &self.gamepads {
            gamepads.release_all();
        }
        if let Err(error) = self.release_pointers() {
            tracing::warn!(%error, "input cleanup incomplete");
        }
    }
    fn release_pointers(&mut self) -> Result<()> {
        self.flush_and_sync();
        let mut failures: Vec<_> = std::mem::take(&mut self.pending.errors)
            .into_iter()
            .map(|error| format!("pending input: {error:#}"))
            .collect();
        let mut keep = |what: &str, result: Result<()>| {
            if let Err(error) = result {
                failures.push(format!("{what}: {error:#}"));
            }
        };
        self.repeat = None;
        self.scroll = [0; 2];
        // The buttons a touch held are released below with the rest.
        self.touch_profile = Default::default();
        if self.left_release.is_some() {
            keep("deferred left button", self.release_deferred_left(true));
        }
        for key in &self.keys {
            keep(
                &format!("key {key}"),
                self.release_owned(
                    true,
                    *key,
                    self.key_scan(*key, false, self.key_flags.get(key).copied().unwrap_or(0)),
                ),
            );
        }
        for &button in &self.buttons {
            let was_virtual = self.virtual_buttons.contains(&button);
            keep(
                &format!("mouse button {button}"),
                self.release_button_on_route(button, was_virtual, true),
            );
        }
        // Lift contacts that are still down before their devices go away,
        // attempting every release even if an earlier device refused it.
        keep("touch contacts", self.cancel_touches());
        keep("pen", self.cancel_pen());
        if self.touchpad {
            virtual_hid::TOUCHPAD.with(|pad| pad.release_all(Instant::now()));
        }
        self.keys.clear();
        self.key_sources.clear();
        self.buttons.clear();
        self.virtual_buttons.clear();
        self.key_flags.clear();
        if !failures.is_empty() {
            bail!("{}", failures.join("; "));
        }
        Ok(())
    }
}

impl Drop for Injector {
    fn drop(&mut self) {
        if let Err(error) = self.release_pointers() {
            tracing::warn!(%error, "input cleanup incomplete during teardown");
        }
        // SAFETY: These synthetic device handles are owned by the injector and taken so each is destroyed exactly once.
        unsafe {
            if let Some(device) = self.touch_device.take() {
                DestroySyntheticPointerDevice(device);
            }
            if let Some(device) = self.pen_device.take() {
                DestroySyntheticPointerDevice(device);
            }
        }
        // After the buttons above were released through it.
        if self.virtual_mouse {
            virtual_hid::MOUSE.release();
        }
        if self.touchpad {
            virtual_hid::TOUCHPAD.release();
        }
    }
}

#[cfg(test)]
#[path = "input/native_touch_tests.rs"]
mod native_touch_tests;

#[cfg(test)]
mod tests;
