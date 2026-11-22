//! Nonary's Moonlight PCM haptics extension (ControllerHaptics.h).
//! Wire constants and payload framing shared by host and client.
use std::time::{Duration, Instant};

pub const FEATURE: u32 = 0x04;
pub const CAPABILITY: u16 = 0x8000;
pub const PACKET_TYPE: u16 = 0x5601;
pub const CHANNEL: u8 = 8;
pub const FRAMES: usize = 240;
pub const MAX_AGE: Duration = Duration::from_millis(20);

#[derive(Debug, PartialEq)]
pub struct Frame {
    pub id: u16,
    pub sequence: u32,
    pub created: Instant,
    pub samples: Vec<u8>,
}

impl Frame {
    pub fn payload(&self) -> Option<Vec<u8>> {
        if self.id >= 16
            || self.samples.is_empty()
            || self.samples.len() > FRAMES * 4
            || !self.samples.len().is_multiple_of(4)
        {
            return None;
        }
        let mut bytes = vec![1, 1];
        bytes.extend_from_slice(&self.id.to_le_bytes());
        bytes.extend_from_slice(&self.sequence.to_le_bytes());
        bytes.extend_from_slice(&((self.samples.len() / 4) as u16).to_le_bytes());
        bytes.extend_from_slice(&[0, 0]);
        bytes.extend_from_slice(&self.samples);
        Some(bytes)
    }
}
