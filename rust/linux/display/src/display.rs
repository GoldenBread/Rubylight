//! Displays as the host sees them on Linux: descriptors, saved layouts and stream display leases.
//!
//! No display backend exists on Linux yet. Listings are empty, a virtual display cannot be
//! made, and anything that would read or change a display's mode, scale, rotation, HDR or
//! position returns an error that says so. Saved layout files are still read and checked.

mod modes;
mod monitor;
mod recovery;
pub mod rubylight_vdd;
pub mod self_test;
mod topology;
mod virtual_display;

pub use modes::{DisplayMode, highest_refresh, largest_scale, mode, output_scale, virtual_scale};
pub use recovery::{
    DisplayDiagnostics, DisplayStatus, Guard, Retained, Snapshot, apply_layout, baseline_nodes,
    leased_displays, restore_positions, stream_display_rotation,
};
pub use topology::{
    Monitor, Topology, active_device_ids, active_displays, edid_refresh, monitors, shown_primary,
};
pub use virtual_display::{
    RenderAdapter, VirtualDisplay, VirtualOptions, configured_render_adapter_with,
    ensure_virtual_display_available, joinable_mode, virtual_display_available,
    virtual_display_installed, virtual_display_os, virtual_display_status, windows_build,
};
