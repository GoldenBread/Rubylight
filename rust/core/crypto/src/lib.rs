//! Cryptographic primitives, X.509 identity, API token hashing, and Moonlight pairing.
#![warn(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::undocumented_unsafe_blocks
)]

pub mod auth;
pub mod crypto;
pub mod pairing;
