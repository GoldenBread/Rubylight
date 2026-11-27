//! The broker's messages: one JSON request per packet from the host, one JSON reply per packet
//! from the broker, a frame's DMA-BUFs attached to its reply (SCM_RIGHTS): the framebuffer's
//! planes in order, then the cursor's.
use serde::{Deserialize, Serialize};

/// The broker's socket.
pub const SOCKET: &str = "/run/rubylight/kms.sock";
/// The group whose members may connect, besides root.
pub const GROUP: &str = "rubylight";
/// The largest message either side sends.
pub const MAX_MESSAGE: usize = 4096;
/// The longest a frame request may wait.
pub const MAX_WAIT_MS: u32 = 1000;
/// The most descriptors one reply carries: four framebuffer planes and the cursor.
pub const MAX_FDS: usize = 5;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// The lit outputs of every card.
    Outputs,
    /// What the output shows: with `fresh`, once it differs from the last frame this
    /// connection was sent (another framebuffer, or the cursor moved or changed), else now; or
    /// [`Reply::Timeout`] after `wait_ms` (at most [`MAX_WAIT_MS`]).
    Frame {
        output: String,
        fresh: bool,
        wait_ms: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum Reply {
    Outputs {
        outputs: Vec<OutputInfo>,
    },
    /// Its descriptors come with it.
    Frame(FrameInfo),
    /// Nothing new in time, or the output is not lit now (another session may be taking the
    /// screen): ask again.
    Timeout,
    Error {
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputInfo {
    /// As KWin and sysfs name it, `DP-1`.
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub refresh_millihz: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaneInfo {
    pub pitch: u32,
    pub offset: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameInfo {
    /// The output as it is now; its mode may differ from one session to the next.
    pub output: OutputInfo,
    pub fb_id: u32,
    pub width: u32,
    pub height: u32,
    /// The DRM fourcc, such as `XR24` or `AR30`.
    pub fourcc: u32,
    /// `DRM_FORMAT_MOD_INVALID` when the framebuffer declares none.
    pub modifier: u64,
    pub planes: Vec<PlaneInfo>,
    pub cursor: Option<CursorInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorInfo {
    pub fb_id: u32,
    /// The plane's top-left corner on the output; it may be off the output's edge.
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub fourcc: u32,
    pub modifier: u64,
    pub pitch: u32,
    pub offset: u32,
}

impl Request {
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice(bytes).ok()
    }
}

impl Reply {
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice(bytes).ok()
    }

    /// How many descriptors come with this reply.
    pub fn fds(&self) -> usize {
        match self {
            Reply::Frame(frame) => frame.planes.len() + usize::from(frame.cursor.is_some()),
            _ => 0,
        }
    }
}
