//! A launch curtain's state: where the start of a game on the PC is, for a client that keeps its
//! own calm screen over the stream until the game's window is in front (`0x5548`,
//! `rubylight_protocol::launch_curtain`). Pure: the host looks at the PC every
//! [`FAST_EVERY`] while a curtain is up and hands each [`Look`] to [`Curtain::look`], which
//! says whether the state changed.
//!
//! The curtain always comes down: the game's window in front for [`READY_LOOKS`] looks in a
//! row lifts it ([`Phase::Ready`]), a device may lift it ([`Phase::Dismissed`]), the app ending
//! fails it, and [`TIMEOUT`] after the launch it lifts whatever the PC shows
//! ([`Phase::TimedOut`]). A launcher prompt or Windows' secure desktop asks for the player
//! ([`Phase::AttentionNeeded`]); the host only reads, it never clicks or types.
use rubylight_native_api::{
    curtain::{CurtainPhase, CurtainPrompt, LaunchCurtainSnapshot},
    session::{LauncherKind, LauncherNotice},
};
use rubylight_protocol::launch_curtain::{
    CurtainState, MAX_LAUNCHER_BYTES, MAX_REASON_BYTES, MAX_TITLE_BYTES, Phase, PromptKind, clip,
    flags,
};
use std::time::Duration;

/// How long after the launch the curtain lifts, whatever the PC shows.
pub const TIMEOUT: Duration = Duration::from_secs(45);
/// How often the host looks at the PC while a curtain is up.
pub const FAST_EVERY: Duration = Duration::from_millis(250);
/// How often the host repeats an unchanged state, so a client that hears nothing for long
/// knows the host stopped following the launch.
pub const HEARTBEAT: Duration = Duration::from_secs(2);
/// Looks in a row with the game's window in front before the curtain lifts: a window that
/// was in front for a quarter of a second has drawn its first pictures.
pub const READY_LOOKS: u8 = 2;
/// The launcher id of Windows' own prompt.
pub const WINDOWS: &str = "windows";

/// What the reasons say, in plain words.
pub const REASON_TIMED_OUT: &str = "The game is still starting.";
pub const REASON_APP_ENDED: &str = "The game closed before its window opened.";

/// Where the game's window is, from one look at the desktop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GameWindow {
    /// No window of the game is on the desktop.
    #[default]
    Missing,
    /// The game's window is in front.
    Front,
    /// The game's window is minimised.
    Minimized,
    /// A store client's window is in front of the game's.
    BehindClient,
    /// Another program's window is in front (a splash, an anti-cheat, the user's own).
    BehindOther,
}

/// One look at the PC.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Look {
    /// Since the launch started.
    pub elapsed: Duration,
    /// Whether the launched app still runs.
    pub app_running: bool,
    /// Whether the game's own process or window has shown up.
    pub game_up: bool,
    pub window: GameWindow,
    /// A launcher's prompt in place of the game (`launcher_prompt::Watch`).
    pub launcher: Option<LauncherNotice>,
    /// Whether Windows shows its secure desktop (a UAC prompt).
    pub secure_desktop: bool,
    /// Now, for when a prompt was first seen.
    pub now_unix_ms: u64,
}

/// One launch's curtain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Curtain {
    state: CurtainState,
    /// When the last frame was made, since the launch.
    sent_at: Duration,
    front_looks: u8,
    /// When Windows' prompt was first seen.
    uac_since: Option<u64>,
    /// When the launcher prompt shown was first seen.
    prompt_since: u64,
}

impl Curtain {
    /// A curtain for a launch; `bypass` for one that starts no game window to wait for (the
    /// desktop, a launcher's own window, a resumed game), which is lifted from the start.
    pub fn new(bypass: bool) -> Self {
        let state = if bypass {
            CurtainState {
                phase: Phase::Ready,
                flags: flags::DESKTOP_OR_LAUNCHER,
                sequence: 1,
                ..Default::default()
            }
        } else {
            CurtainState {
                phase: Phase::StartingApp,
                flags: flags::DISMISSIBLE,
                sequence: 1,
                ..Default::default()
            }
        };
        Self {
            state,
            sent_at: Duration::ZERO,
            front_looks: 0,
            uac_since: None,
            prompt_since: 0,
        }
    }

    pub fn state(&self) -> &CurtainState {
        &self.state
    }

    /// Whether the curtain is still up.
    pub fn active(&self) -> bool {
        !self.state.phase.lifts()
    }

    /// One look at the PC. Returns whether the state changed or is due again, with a new
    /// sequence; a lifted curtain never changes.
    pub fn look(&mut self, look: &Look) -> bool {
        if !self.active() {
            return false;
        }
        let mut next = CurtainState {
            flags: self.state.flags & (flags::FIRST_FRAME_SEEN | flags::DISMISSIBLE),
            ..Default::default()
        };
        if look.window != GameWindow::Missing {
            next.flags |= flags::FIRST_FRAME_SEEN;
        }
        if !look.secure_desktop {
            self.uac_since = None;
        }
        self.front_looks = if look.window == GameWindow::Front && !look.secure_desktop {
            self.front_looks.saturating_add(1)
        } else {
            0
        };
        if !look.app_running {
            next.phase = Phase::Failed;
            next.reason = REASON_APP_ENDED.into();
        } else if look.elapsed >= TIMEOUT {
            next.phase = Phase::TimedOut;
            next.reason = REASON_TIMED_OUT.into();
        } else if look.secure_desktop {
            let since = *self.uac_since.get_or_insert(look.now_unix_ms);
            self.prompt_since = since;
            next.phase = Phase::AttentionNeeded;
            next.prompt = PromptKind::Uac;
            next.flags |= flags::UAC_SECURE_DESKTOP;
            next.launcher = WINDOWS.into();
        } else if self.front_looks >= READY_LOOKS {
            next.phase = Phase::Ready;
        } else if look.window == GameWindow::Front {
            // In front, not yet for long enough: one look more.
            next.phase = match self.state.phase {
                Phase::FocusingWindow => Phase::FocusingWindow,
                _ => Phase::WaitingWindow,
            };
        } else if let Some(notice) = &look.launcher {
            self.prompt_since = notice.since_unix_ms;
            next.phase = Phase::AttentionNeeded;
            next.prompt = prompt_kind(notice.kind);
            next.launcher = clip(&notice.launcher, MAX_LAUNCHER_BYTES).into();
            next.title = clip(&notice.window_title, MAX_TITLE_BYTES).into();
        } else if matches!(
            look.window,
            GameWindow::Minimized | GameWindow::BehindClient
        ) {
            next.phase = Phase::FocusingWindow;
        } else if look.game_up || look.window != GameWindow::Missing {
            next.phase = Phase::WaitingWindow;
        } else {
            next.phase = Phase::StartingApp;
        }
        if next.phase.lifts() {
            next.flags &= !flags::DISMISSIBLE;
        }
        self.commit(next, look.elapsed)
    }

    /// A device lifted the curtain. Returns whether it was up.
    pub fn dismiss(&mut self, elapsed: Duration) -> bool {
        if !self.active() {
            return false;
        }
        let next = CurtainState {
            phase: Phase::Dismissed,
            flags: self.state.flags & flags::FIRST_FRAME_SEEN,
            ..Default::default()
        };
        self.commit(next, elapsed)
    }

    /// Takes `next` with a new sequence when it differs from the state, or when the state is
    /// due again ([`HEARTBEAT`]).
    fn commit(&mut self, mut next: CurtainState, elapsed: Duration) -> bool {
        next.reason = clip(&next.reason, MAX_REASON_BYTES).into();
        next.sequence = self.state.sequence;
        next.elapsed_ms = self.state.elapsed_ms;
        let changed = next != self.state;
        if !changed && elapsed.saturating_sub(self.sent_at) < HEARTBEAT {
            return false;
        }
        next.sequence = self.state.sequence.wrapping_add(1).max(1);
        next.elapsed_ms = elapsed.as_millis().min(u32::MAX as u128) as u32;
        self.state = next;
        self.sent_at = elapsed;
        true
    }

    /// The native API's view; `owner` for the device that owns the session, which alone sees
    /// a prompt's title and may lift the curtain.
    pub fn snapshot(&self, revision: u64, owner: bool) -> LaunchCurtainSnapshot {
        let state = &self.state;
        let prompt = (state.phase == Phase::AttentionNeeded).then(|| CurtainPrompt {
            launcher: state.launcher.clone(),
            kind: launcher_kind(state.prompt),
            window_title: if owner {
                state.title.clone()
            } else {
                String::new()
            },
            since_unix_ms: self.prompt_since,
        });
        LaunchCurtainSnapshot {
            revision,
            active: self.active(),
            phase: phase_name(state.phase),
            dismissible: owner && self.active(),
            elapsed_ms: state.elapsed_ms.into(),
            prompt,
            reason: (!state.reason.is_empty()).then(|| state.reason.clone()),
        }
    }
}

/// A launcher notice's kind on the wire.
pub fn prompt_kind(kind: LauncherKind) -> PromptKind {
    match kind {
        LauncherKind::Update => PromptKind::Update,
        LauncherKind::SignIn => PromptKind::SignIn,
        LauncherKind::Uac => PromptKind::Uac,
        LauncherKind::Prompt | LauncherKind::Unknown => PromptKind::Prompt,
    }
}

fn launcher_kind(kind: PromptKind) -> LauncherKind {
    match kind {
        PromptKind::Update => LauncherKind::Update,
        PromptKind::SignIn => LauncherKind::SignIn,
        PromptKind::Uac => LauncherKind::Uac,
        PromptKind::Prompt | PromptKind::None => LauncherKind::Prompt,
    }
}

fn phase_name(phase: Phase) -> CurtainPhase {
    match phase {
        Phase::Preparing => CurtainPhase::Preparing,
        Phase::StartingApp => CurtainPhase::StartingApp,
        Phase::WaitingWindow => CurtainPhase::WaitingWindow,
        Phase::FocusingWindow => CurtainPhase::FocusingWindow,
        Phase::AttentionNeeded => CurtainPhase::AttentionNeeded,
        Phase::Ready => CurtainPhase::Ready,
        Phase::Dismissed => CurtainPhase::Dismissed,
        Phase::TimedOut => CurtainPhase::TimedOut,
        Phase::Failed => CurtainPhase::Failed,
    }
}

#[cfg(test)]
mod tests;
