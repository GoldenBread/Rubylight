//! Web console, LAN/localhost HTTP API, and client-facing native API routes.
pub mod console;
pub mod native_api;
pub mod web;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
