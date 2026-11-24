//! Nucleus Co-op registry, elevation, and stream admission tests extracted from `butterpollo_stream::nucleus`.
//! Nucleus Co-op runs on Windows only, and so do these tests.
#![cfg(windows)]

#[path = "nucleus_tests/background_stream_tests.rs"]
mod background_stream_tests;
#[path = "nucleus_tests/catalog_and_status_tests.rs"]
mod catalog_and_status_tests;
#[path = "nucleus_tests/elevation_and_handler_tests.rs"]
mod elevation_and_handler_tests;
#[path = "nucleus_tests/hardware_e2e_tests.rs"]
mod hardware_e2e_tests;
#[path = "nucleus_tests/hardware_support.rs"]
mod hardware_support;
#[path = "nucleus_tests/hardware_unit_tests.rs"]
mod hardware_unit_tests;
#[path = "nucleus_tests/session_and_player_tests.rs"]
mod session_and_player_tests;
#[path = "nucleus_tests/setup.rs"]
mod setup_helper;
#[path = "nucleus_tests/window_and_admission_tests.rs"]
mod window_and_admission_tests;
