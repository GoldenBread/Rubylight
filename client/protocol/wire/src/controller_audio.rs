//! Rubylight USB controller audio wire constants and parsers shared by host and client.
use crate::haptics;

pub const SPEAKER: u16 = 0x5533;
pub const MICROPHONE: u16 = 0x5534;
pub const CONTROL: u16 = 0x5535;
pub const PLAYBACK: u8 = 1;
pub const CAPTURE: u8 = 2;
pub const HEADPHONES: u8 = 4;
pub const MUTED: u8 = 8;

/// Stereo S16/48 kHz plus packet/control encryption, ENet and IPv6 overhead.
pub const PCM_WIRE_BPS: u64 = 2_000_000;

pub fn pcm(payload: &[u8]) -> Option<(u16, u32, &[u8])> {
    if payload.len() < 12 || payload[..2] != [1, 1] || payload[10..12] != [0, 0] {
        return None;
    }
    let id = u16::from_le_bytes(payload[2..4].try_into().ok()?);
    let frames = usize::from(u16::from_le_bytes(payload[8..10].try_into().ok()?));
    if id >= 16 || !(1..=haptics::FRAMES).contains(&frames) || payload.len() != 12 + frames * 4 {
        return None;
    }
    Some((
        id,
        u32::from_le_bytes(payload[4..8].try_into().ok()?),
        &payload[12..],
    ))
}

/// [version, operation=1, controller:u16, flags, zero, zero, zero].
pub fn status(payload: &[u8]) -> Option<(u16, u8)> {
    if payload.len() != 8
        || payload[..2] != [1, 1]
        || payload[4] & !15 != 0
        || payload[5..] != [0; 3]
    {
        return None;
    }
    let id = u16::from_le_bytes([payload[2], payload[3]]);
    (id < 16).then_some((id, payload[4]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn protocol_rejects_reserved_bits_lengths_and_controller_overflow() {
        assert_eq!(status(&[1, 1, 15, 0, 15, 0, 0, 0]), Some((15, 15)));
        assert!(status(&[1, 1, 16, 0, 1, 0, 0, 0]).is_none());
        assert!(status(&[1, 1, 0, 0, 16, 0, 0, 0]).is_none());
        let frame = haptics::Frame {
            id: 0,
            sequence: 5,
            created: Instant::now(),
            samples: vec![1; 16],
        };
        let mut bytes = frame.payload().unwrap();
        assert!(pcm(&bytes).is_some());
        bytes.push(0);
        assert!(pcm(&bytes).is_none());
    }
}
