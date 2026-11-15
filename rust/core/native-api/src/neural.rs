//! `GET /neural?app=<id>` and `POST /neural/settings`: a game's Neural
//! Rendering switches, which only the host's console could turn before.
//!
//! The answer lists the switches the host offers for the game, so a client
//! draws each by its id and leaves out one it doesn't know. A device never
//! names a file on the PC: when the host can't tell which program is the
//! game, the switch says to choose it in the console.
use crate::radeon::ControlState;
use serde::{Deserialize, Serialize};

/// The frame generation switch.
pub const FRAME_GENERATION: &str = "frame_generation";

/// `GET /neural?app=<id>`, and the answer to a write.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Neural {
    /// The app's id, as `/library` lists it.
    pub app: String,
    /// The one-time notice was accepted on this host. Until it is, a write
    /// that turns a switch on carries `accept_notice`.
    pub accepted: bool,
    /// The GPU the switches run on, when the PC has one: the one the host
    /// streams with, else another of the PC's, used when the game renders
    /// on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu: Option<String>,
    /// When `gpu` is not the GPU the host streams with: that frame
    /// generation runs only when the game renders on it, in the host's words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_note: Option<String>,
    pub switches: Vec<Switch>,
    /// Rubylight 3.0's neural upscaling is still in the game's folder:
    /// turning a switch on removes it first.
    #[serde(default)]
    pub replaces_earlier_upscaling: bool,
    /// What the host is doing for the game now, or how it last ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<Job>,
    /// How frame generation is in the game's folder, once it is on: the
    /// host's own package, or added to an upscaler mod the user installed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
    /// The release in the game's folder, once it is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// The host's own package.
    Full,
    /// Added to an upscaler mod the user installed.
    Addon,
    #[serde(other)]
    Unknown,
}

/// Why a switch can't change now, for a client to say in its own words;
/// `reason` says it in the host's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Why {
    /// The host is downloading, installing or removing for the game.
    Busy,
    /// A stream runs.
    Streaming,
    /// The device lacks Host control.
    NoHostControl,
    /// None of this PC's GPUs can run it.
    Gpu,
    /// The PC lacks the Microsoft Visual C++ 2015-2022 Redistributable (x64)
    /// it needs; a client older than this code shows `reason`.
    Runtime,
    /// The game has an anti-cheat, which treats the mod as a cheat.
    AntiCheat,
    /// The game offers no DLSS frame generation for it to take the place of.
    NoFrameGeneration,
    /// The host can't tell which program is the game: choose it in the console.
    ChooseProgram,
    /// Something else about the game; `reason` says what.
    Game,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Switch {
    /// [`FRAME_GENERATION`]; clients leave out ids they don't know.
    pub id: String,
    /// `configurable`; `unsupported` (this PC's GPU can't run it, or the game
    /// can't have it);
    /// `temporarily_unavailable` (a stream runs, or the host is busy with the
    /// game); `needs_external_setup` (choose the game's program in the
    /// console). `reason` says which in words.
    pub state: ControlState,
    pub on: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Why it can't change, as a code a client says in its own words; absent
    /// when it can, and from a host before these codes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<Why>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Install,
    Remove,
    /// The check a launch runs on a game that has a switch on.
    Update,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Download,
    Place,
    Remove,
    Done,
    Failed,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub action: Action,
    pub phase: Phase,
    /// Bytes so far and in all, while it downloads or copies; 0 when unknown.
    #[serde(default)]
    pub done: u64,
    #[serde(default)]
    pub total: u64,
    /// Why it failed, in words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `POST /neural/settings`: turn one switch of one game on or off.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsWrite {
    pub app: String,
    pub switch: String,
    pub on: bool,
    /// The user accepted the notice on the device just now.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub accept_notice: bool,
}
