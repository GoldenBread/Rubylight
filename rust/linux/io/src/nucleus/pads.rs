//! The pads of a Nucleus Co-op session, on Linux: no virtual pad is plugged and XInput, a
//! Windows library, cannot be opened. The rule that routes each player's controller to
//! their pad, the limits a stream shows and the count of free XInput slots are as on
//! Windows.
use anyhow::Result;
use butterpollo_input_policy::input::Input;
use butterpollo_session::nucleus::coop::MAX_PLAYERS;
use std::time::{Duration, Instant};

/// How long a pad may take to plug in and to reach XInput.
pub const PLUG_TIMEOUT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(20);
/// How long a set of pads may take to unplug when it is stopped.
pub const RETIRE_TIMEOUT: Duration = Duration::from_secs(3);

/// What the stream shows when a player's keyboard or mouse input arrives.
pub const KEYBOARD_MOUSE_LIMIT: &str = "Nucleus Co-op streams take controller input only: keyboard, mouse, touch and pen cannot reach one player's game instance yet";
/// What the stream shows when a player uses a second controller.
pub const SECOND_CONTROLLER_LIMIT: &str = "Each Nucleus Co-op player has one controller: only this device's first controller reaches its game instance";
/// The stream's warning for a controller on the PC at a lower XInput index than a player's
/// pad.
pub const CONTROLLER_AHEAD: &str = "nucleus_controller_ahead";

/// Which XInput user indices have a controller.
pub trait XInput: Send {
    fn connected(&mut self) -> Result<[bool; MAX_PLAYERS]>;
}

/// XInput on the PC. It cannot be opened on Linux, so no value of this type exists there.
pub struct System {
    _opened: (),
}
impl System {
    /// Open XInput. Always an error on Linux.
    pub fn open() -> Result<Self> {
        Err(crate::unsupported("XInput"))
    }

    /// The buttons XInput reports at user index `index`; None without a controller there.
    /// Always none on Linux.
    pub fn buttons(&self, _index: u8) -> Result<Option<u16>> {
        Ok(None)
    }
}
impl XInput for System {
    fn connected(&mut self) -> Result<[bool; MAX_PLAYERS]> {
        Ok([false; MAX_PLAYERS])
    }
}

/// How many XInput indices the PC's own controllers leave free. No ended session holds a
/// pad on Linux, so every index without a controller counts.
pub fn free(xinput: &mut dyn XInput) -> Result<usize> {
    Ok(free_of(xinput.connected()?, &[]))
}

fn free_of(connected: [bool; MAX_PLAYERS], held: &[u8]) -> usize {
    (0..MAX_PLAYERS)
        .filter(|&i| !connected[i] || held.contains(&(i as u8)))
        .count()
}

/// How long XInput may take to show controllers [`released`] unplugged gone.
const RELEASE_SETTLE: Duration = Duration::from_secs(1);

/// Run `release`, which unplugs other streams' controllers and returns how many pads they
/// had plugged in, then wait until XInput shows that many fewer controllers. Pads that
/// never reached XInput end the wait after a second.
pub fn released(xinput: &mut dyn XInput, release: impl FnOnce() -> Result<u32>) -> Result<()> {
    let count = |connected: [bool; MAX_PLAYERS]| connected.iter().filter(|c| **c).count();
    let before = count(xinput.connected()?);
    let plugged = release()? as usize;
    if plugged == 0 {
        return Ok(());
    }
    let wanted = before.saturating_sub(plugged);
    let deadline = Instant::now() + RELEASE_SETTLE;
    loop {
        let now = count(xinput.connected()?);
        if now <= wanted {
            return Ok(());
        }
        if Instant::now() >= deadline {
            tracing::info!(
                before,
                now,
                plugged,
                "Nucleus Co-op: some released controllers were not XInput controllers"
            );
            return Ok(());
        }
        std::thread::sleep(POLL);
    }
}

/// Why a player's input event did not reach a pad.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dropped {
    /// The slot's pad is fixed for the session: arrivals, sensors, battery and haptics
    /// settings of the client's controller do not change it.
    Ignored,
    /// Keyboard, mouse, touch or pen: [`KEYBOARD_MOUSE_LIMIT`].
    KeyboardMouse,
    /// A controller other than the client's first: [`SECOND_CONTROLLER_LIMIT`].
    SecondController,
}
impl Dropped {
    /// What the stream tells the player, when it tells them anything.
    pub fn limit(self) -> Option<&'static str> {
        match self {
            Self::Ignored => None,
            Self::KeyboardMouse => Some(KEYBOARD_MOUSE_LIMIT),
            Self::SecondController => Some(SECOND_CONTROLLER_LIMIT),
        }
    }
}

/// Slot `slot`'s pad state from a client's controller `event`, in a session of `players`
/// pads. The client's first controller (the lowest it reports connected) plays; the pad
/// keeps every slot's pad plugged.
pub fn route(event: Input, slot: usize, players: usize) -> Result<Input, Dropped> {
    let all = ((1u32 << players) - 1) as u16;
    match event {
        Input::Controller {
            id,
            active,
            buttons,
            left_trigger,
            right_trigger,
            sticks,
        } => {
            // A client whose controllers all left sends one last state: the slot's pad is
            // released, never unplugged.
            let released = active == 0;
            if !released && id != active.trailing_zeros() as u16 {
                return Err(Dropped::SecondController);
            }
            Ok(Input::Controller {
                id: slot as u16,
                active: all,
                buttons: if released { 0 } else { buttons },
                left_trigger: if released { 0 } else { left_trigger },
                right_trigger: if released { 0 } else { right_trigger },
                sticks: if released { [0; 4] } else { sticks },
            })
        }
        Input::Relative { .. }
        | Input::Absolute { .. }
        | Input::MouseButton { .. }
        | Input::Scroll { .. }
        | Input::Keyboard { .. }
        | Input::Text(_)
        | Input::Touch { .. }
        | Input::Pen { .. }
        | Input::Touchpad { .. } => Err(Dropped::KeyboardMouse),
        _ => Err(Dropped::Ignored),
    }
}
