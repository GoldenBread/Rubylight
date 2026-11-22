//! Nightjar desktop pure `egui` launcher UI, theme, glyphs, page state machines and overlay
//! widgets. Compiled and tested without `sdl3-sys`, FFmpeg, GPU drivers or `nightjar-core`.

pub use nightjar_desktop_config::VERSION;
pub use nightjar_desktop_config::{
    app_types, config, gamepad_types, hosts_types, latency_bench_state, network_types, pads_view,
    pin_lock, power_types, shortcuts, stream_types, strings, window_place_types, wol_types,
};

pub mod ambient;
pub mod conn_test;
pub mod console;
pub mod coop_page;
pub mod folders_page;
pub mod glyphs;
pub mod gui;
pub mod host_page;
pub mod hosts_model;
pub mod input_source;
pub mod live_glass;
pub mod menu_style;
pub mod nav_repeat;
pub mod network_test;
pub mod neural_page;
pub mod pin_ui;
pub mod platform_hooks;
pub mod radeon;
pub mod session_page;
pub mod session_ui;
pub mod shortcuts_ui;
#[cfg(any(test, feature = "test-support"))]
pub mod shot;
pub mod stream_menu;
pub mod stream_overlay_widgets;
pub mod theme;
pub mod ui;
pub mod update_banner;

pub use hosts_model as hosts;
pub use latency_bench_state as latency_bench;
pub use network_types as network;
pub use power_types as power;
pub use wol_types as wol;
