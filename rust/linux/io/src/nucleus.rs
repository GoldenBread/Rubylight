//! Nucleus Co-op, on Linux: it is a Windows program and is never installed here, so no
//! session starts, no pad is plugged and nothing is written. The data types, the texts the
//! host shows, the input routing rule and reading a Nucleus Co-op folder's handlers and
//! game list are the same as on Windows.
//!
//! The OS-independent parts (the pinned release, handler lookup, the game profile, the
//! co-op session's state) live in `butterpollo_session::nucleus`.
pub mod desktop;
pub mod elevation;
pub mod files;
pub mod install;
pub mod instances;
pub mod layers;
pub mod pads;
pub mod session;
pub mod user_copy;
