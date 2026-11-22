//! Rubylight host runtime library: HTTP/RTSP/WebRTC servers, session lifecycle, and streaming workers.
mod auto_lock;
mod child_session;
pub mod cli;
mod console;
mod continue_here;
mod controller_sessions;
mod couch_menu;
mod display_session;
mod dsu;
mod extensions;
mod game_files;
mod game_settings;
mod host_warnings;
mod internet_play;
mod invites;
mod launch_focus;
mod launcher_watch;
mod library;
mod logging;
mod lossless;
mod maintenance;
mod mic;
mod multiclient;
#[cfg(any(test, feature = "test-harness"))]
pub mod native_api;
#[cfg(not(any(test, feature = "test-harness")))]
mod native_api;
mod network;
mod network_path;
mod neural;
mod nucleus;
#[cfg(any(test, feature = "test-harness"))]
pub mod nvhttp;
#[cfg(not(any(test, feature = "test-harness")))]
mod nvhttp;
mod nvidia_component;
mod nvidia_enhance;
mod playnite;
#[cfg(any(test, feature = "test-harness"))]
pub mod power_action;
#[cfg(not(any(test, feature = "test-harness")))]
mod power_action;
#[cfg(any(test, feature = "test-harness"))]
pub mod process;
#[cfg(not(any(test, feature = "test-harness")))]
mod process;
mod pyrowave_probe;
mod pyrowave_send;
mod radeon_profile;
mod radeon_session;
mod remote_display;
mod rtsp_server;
mod runtime;
mod server;
mod service_tasks;
mod session_control;
mod session_end;
mod session_history;
mod session_notes;
mod settings_backup;
#[cfg(any(debug_assertions, test))]
mod soak_fault;
mod stall_watch;
#[cfg(any(test, feature = "test-harness"))]
pub mod state;
#[cfg(not(any(test, feature = "test-harness")))]
mod state;
mod steam;
mod steamgriddb;
#[cfg(any(test, feature = "test-harness"))]
pub mod stream;
#[cfg(not(any(test, feature = "test-harness")))]
mod stream;
mod stream_focus;
mod stream_notice;
mod telemetry;
#[cfg(any(test, feature = "test-harness"))]
mod test_log;
mod timing_record;
mod tls;
mod updater;
#[cfg(any(test, feature = "test-harness"))]
pub mod usb_forward;
#[cfg(not(any(test, feature = "test-harness")))]
mod usb_forward;
mod video_send;
mod wake;
#[cfg(any(test, feature = "test-harness"))]
pub mod web;
#[cfg(not(any(test, feature = "test-harness")))]
mod web;
mod web_sessions;
#[cfg(any(test, feature = "test-harness"))]
pub mod webhooks;
#[cfg(not(any(test, feature = "test-harness")))]
mod webhooks;
mod webrtc;
