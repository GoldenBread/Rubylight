//! Crate re-exports from split desktop crates so internal `crate::` paths resolve uniformly.

pub use nightjar_desktop_config::{
    app_types, config, gamepad_types, hosts_types, latency_bench_state, network_types, pads_view,
    pin_lock, shortcuts, strings, stream_types, version_name, VERSION,
};
pub use nightjar_desktop_ui::{
    ambient, console, coop_page, folders_page, glyphs, host_page, hosts_model,
    menu_style, nav_repeat, neural_page, pin_ui, platform_hooks, radeon, session_page, session_ui,
    shortcuts_ui, stream_menu, theme, ui, update_banner,
};
#[cfg(test)]
pub use nightjar_desktop_ui::shot;
pub use nightjar_desktop_media::{
    amd_decode, clipboard, controller_audio, controller_mouse, dmabuf, full_chroma, gamepad,
    handheld, keymap, kms, latency_bench, media, mp4, network, render_device, sdl, second_screen,
    shortcuts_runtime, stream, timing_csv, usb,
};
#[cfg(windows)]
pub use nightjar_desktop_media::gpu_adapters;
#[cfg(any(target_os = "linux", test))]
pub use nightjar_desktop_media::{linux_diagnosis, linux_video};
#[cfg(any(target_os = "macos", test))]
pub use nightjar_desktop_media::mac_display;
#[cfg(target_os = "linux")]
pub use nightjar_desktop_media::usb_linux;
#[cfg(any(windows, test))]
pub use nightjar_desktop_media::usb_winusb;
// Linux and macOS (Unix) share Vulkan video decode.
#[cfg(any(unix, test))]
pub use nightjar_desktop_media::vulkan_video;
