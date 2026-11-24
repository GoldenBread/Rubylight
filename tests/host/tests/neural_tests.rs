//! Neural Rendering unit/integration tests extracted from `butterpollo_stream::neural`.
//! Neural Rendering runs Windows games through the Radeon driver, so these tests run on Windows
//! only.
#![cfg(windows)]

#[path = "neural_tests/common.rs"]
mod common;
#[path = "neural_tests/download_and_addon_tests.rs"]
mod download_and_addon_tests;
#[path = "neural_tests/gpu_and_status_tests.rs"]
mod gpu_and_status_tests;
#[path = "neural_tests/install_and_anti_cheat_tests.rs"]
mod install_and_anti_cheat_tests;
#[path = "neural_tests/job_lifecycle_tests.rs"]
mod job_lifecycle_tests;
#[path = "neural_tests/legacy_amdnr_tests.rs"]
mod legacy_amdnr_tests;
#[path = "neural_tests/locks_and_links_tests.rs"]
mod locks_and_links_tests;
#[path = "neural_tests/permissions_and_cache_tests.rs"]
mod permissions_and_cache_tests;
#[path = "neural_tests/provider_leftovers_tests.rs"]
mod provider_leftovers_tests;
#[path = "neural_tests/steam_and_launchers_tests.rs"]
mod steam_and_launchers_tests;
