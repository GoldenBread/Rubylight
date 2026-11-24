//! Frame-ring control IOCTL payloads (`IOCTL_SET_FRAME_RING`, `IOCTL_SET_FENCE_RING`,
//! `IOCTL_CLEAR_FRAME_RING_GENERATION`).
//!
//! Part of Rubylight's virtual display control plane, based on punktfunk's `pf-driver-proto`
//! by Enrico Bühler (unom), MIT OR Apache-2.0 (<https://github.com/unom/punktfunk>).

use super::{Reader, WireError, Writer};
use crate::{modes, ring};

/// [`super::IOCTL_CLEAR_FRAME_RING_GENERATION`] input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClearFrameRingRequest {
    pub session_id: u64,
    /// The [`SetFrameRingRequest::generation`] of the ring to drop.
    pub generation: u32,
}

impl ClearFrameRingRequest {
    pub const SIZE: usize = 16;

    #[must_use]
    pub fn encode(&self) -> [u8; Self::SIZE] {
        Writer::new()
            .u64(self.session_id)
            .u32(self.generation)
            .u32(0)
            .finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(bytes, Self::SIZE)?;
        let request = Self {
            session_id: r.u64(),
            generation: r.u32(),
        };
        if request.generation == 0 {
            return Err(WireError::Invalid("generation"));
        }
        Ok(request)
    }
}

/// [`super::IOCTL_SET_FRAME_RING`] input. Every handle is a value in the driver's WUDFHost,
/// already duplicated there by the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetFrameRingRequest {
    pub session_id: u64,
    /// Non-zero and new for every ring of this monitor. It is stamped into each publish
    /// token, so a frame an old ring left behind is never read from the new one.
    pub generation: u32,
    pub slot_count: u32,
    pub width: u32,
    pub height: u32,
    /// A `DXGI_FORMAT`: [`ring::FORMAT_BGRA8`] or [`ring::FORMAT_RGBA16F`].
    pub format: u32,
    /// Bytes the host mapped for the header section, at least [`ring::HEADER_BYTES`].
    pub header_bytes: u32,
    /// The adapter the textures were created on, which must be the one rendering the
    /// monitor.
    pub adapter_luid_low: u32,
    pub adapter_luid_high: i32,
    /// The header section (`CreateFileMapping`), mapped read-write by the driver.
    pub header_section: u64,
    /// The event the driver sets after each publish.
    pub event: u64,
    /// Shared NT handles of `slot_count` keyed-mutex textures; the rest are `0`.
    pub textures: [u64; ring::MAX_SLOTS],
}

impl SetFrameRingRequest {
    pub const SIZE: usize = 128;

    #[must_use]
    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut w = Writer::new()
            .u64(self.session_id)
            .u32(self.generation)
            .u32(self.slot_count)
            .u32(self.width)
            .u32(self.height)
            .u32(self.format)
            .u32(self.header_bytes)
            .u32(self.adapter_luid_low)
            .i32(self.adapter_luid_high)
            .u64(self.header_section)
            .u64(self.event);
        for texture in self.textures {
            w = w.u64(texture);
        }
        w.u64(0).finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(bytes, Self::SIZE)?;
        let mut request = Self {
            session_id: r.u64(),
            generation: r.u32(),
            slot_count: r.u32(),
            width: r.u32(),
            height: r.u32(),
            format: r.u32(),
            header_bytes: r.u32(),
            adapter_luid_low: r.u32(),
            adapter_luid_high: r.i32(),
            header_section: r.u64(),
            event: r.u64(),
            textures: [0; ring::MAX_SLOTS],
        };
        for texture in &mut request.textures {
            *texture = r.u64();
        }
        if r.u64() != 0 {
            return Err(WireError::Invalid("reserved"));
        }
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<(), WireError> {
        if self.session_id == 0 {
            return Err(WireError::Invalid("session id"));
        }
        if self.generation == 0 || self.generation > ring::Token::GENERATION_MASK {
            return Err(WireError::Invalid("generation"));
        }
        let slots = self.slot_count as usize;
        if !(ring::MIN_SLOTS..=ring::MAX_SLOTS).contains(&slots) {
            return Err(WireError::Invalid("slot count"));
        }
        if !(1..=modes::MAX_DIMENSION).contains(&self.width)
            || !(1..=modes::MAX_DIMENSION).contains(&self.height)
        {
            return Err(WireError::Invalid("size"));
        }
        if !ring::supported_format(self.format) {
            return Err(WireError::Invalid("format"));
        }
        if (self.header_bytes as usize) < ring::HEADER_BYTES {
            return Err(WireError::Invalid("header size"));
        }
        if self.header_section == 0 || self.event == 0 {
            return Err(WireError::Invalid("handle"));
        }
        let (used, unused) = self.textures.split_at(slots);
        if used.contains(&0) || unused.iter().any(|&t| t != 0) {
            return Err(WireError::Invalid("textures"));
        }
        Ok(())
    }

    /// Every handle value the request carries, for the host to close remotely when the
    /// driver refused it.
    pub fn handles(&self) -> impl Iterator<Item = u64> + '_ {
        [self.header_section, self.event]
            .into_iter()
            .chain(self.textures.iter().copied())
            .filter(|&h| h != 0)
    }
}

/// [`super::IOCTL_SET_FENCE_RING`] input: a ring of plain shared textures (no keyed mutex) and the
/// two shared fences that order access to them, every handle a value in the driver's WUDFHost.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetFenceRingRequest {
    /// The ring as a keyed-mutex request would describe it.
    pub ring: SetFrameRingRequest,
    /// The driver signals it with a frame's sequence once that frame's copy is done.
    pub written_fence: u64,
    /// The host signals it once its reads of a slot are done, with the value it records in
    /// the slot's state.
    pub released_fence: u64,
}

impl SetFenceRingRequest {
    pub const SIZE: usize = SetFrameRingRequest::SIZE + 24;

    #[must_use]
    pub fn encode(&self) -> [u8; Self::SIZE] {
        Writer::new()
            .bytes(&self.ring.encode())
            .u64(self.written_fence)
            .u64(self.released_fence)
            .u64(0)
            .finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(bytes, Self::SIZE)?;
        let ring = SetFrameRingRequest::decode(&r.take::<{ SetFrameRingRequest::SIZE }>())?;
        let request = Self {
            ring,
            written_fence: r.u64(),
            released_fence: r.u64(),
        };
        if r.u64() != 0 {
            return Err(WireError::Invalid("reserved"));
        }
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<(), WireError> {
        self.ring.validate()?;
        if self.written_fence == 0
            || self.released_fence == 0
            || self.written_fence == self.released_fence
        {
            return Err(WireError::Invalid("fences"));
        }
        if self
            .handles()
            .enumerate()
            .any(|(index, handle)| self.handles().take(index).any(|earlier| earlier == handle))
        {
            return Err(WireError::Invalid("duplicate handles"));
        }
        Ok(())
    }

    /// Every handle value the request carries, for the host to close remotely when the
    /// driver refused it.
    pub fn handles(&self) -> impl Iterator<Item = u64> + '_ {
        self.ring
            .handles()
            .chain([self.written_fence, self.released_fence])
            .filter(|&h| h != 0)
    }
}
