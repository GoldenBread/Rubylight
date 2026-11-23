//! Pure host record schemas, address slot parsing, alias normalization, per-host notice/mode/
//! capability policies, and unreadable-host sanitization shared across Nightjar clients.

pub mod address;
pub mod meta;
pub mod record;
pub mod sanitize;

pub use address::{EndpointSlot, PinnedAddress};
pub use meta::{
    ALL_PACKED_MODES, HostCapabilitiesSnapshot, HostNotice, HostNoticesState,
    HostPackedEntry, HostPackedModesState, NOTICE_FRAME_WARP_UNAVAILABLE,
    NOTICE_PACKED_UNAVAILABLE, NOTICE_PYROWAVE_FALLBACK,
};
pub use record::{ALIAS_MAX_CHARS, HostNames, IdentityFile, SharedHostFields};
pub use sanitize::keep_readable_hosts;

#[cfg(test)]
mod tests;
