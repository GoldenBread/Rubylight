//! Prepare display and game integrations before running application commands.
//! The same leases move from an authorized pending launch into its transport.
use crate::state::Shared;
use butterpollo_media_policy::{
    display_policy::HostDisplay,
    display_session_machine::DisplaySessionMachine,
    framegen::{Policy, Rate},
};
use butterpollo_win_display::{
    display::{Guard, Retained},
    display_arrangement, hdr_profile,
};
use butterpollo_win_graphics::vulkan;
use std::sync::Arc;

mod apply;
mod baseline;
mod lifecycle;
#[cfg(target_os = "linux")]
mod linux_private;
mod mode_report;
mod policy;
mod remembered;
mod restore;
mod scale;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
#[cfg(test)]
pub(crate) mod tests;
mod turn;
mod virtual_dd;
mod window_target;

pub(crate) use apply::hold_off_wake_watch;
pub use apply::{displays_settling, prepare_stream};
use baseline::GoldenLease;
pub use lifecycle::{Ready, StreamPreparation, retime};
use mode_report::ModeReport;
#[cfg(test)]
use policy::joined_report;
pub(crate) use policy::{
    BROWSER_ON_THIS_PC, REMEMBERED_RESOLUTION, REMEMBERED_SCALE, display_of_new_owner,
    launch_display_layout, legacy_host_layout, legacy_layout_notice,
    migrate_legacy_extended_layouts, record_game_display, take_retained_display,
};
pub use policy::{LEARNED_OVER, RESUMES_GAME, SET_BY_USER, client_display};
#[cfg(test)]
pub(crate) use policy::{browser_keeps_monitors, virtual_display_layout};
#[cfg(any(test, feature = "test-support"))]
pub use test_support::DISPLAYS_SETTLING;
use turn::Follow;

/// Resolution, refresh in millihertz and HDR a stream asked its display for.
type RequestedMode = butterpollo_media_policy::display_session_machine::RequestedMode;
/// Resolution, refresh in millihertz and HDR a display shows.
type AppliedMode = (u32, u32, u32, bool);
/// The resolution a stream set on its virtual display, and the one Windows shows.
type SizeChange = ((u32, u32), (u32, u32));
/// A device's own display mode (`display_mode`): width, height and refresh.
type DeviceMode = (u32, u32, Rate);

pub struct Prepared {
    pub display: Option<Guard>,
    pub output: String,
    pub framegen: Policy,
    // Restore topology while the selected monitor still exists, then remove it.
    _arrangement: Option<display_arrangement::Lease>,
    _activation: Option<display_arrangement::Activation>,
    _profile: Option<hdr_profile::Lease>,
    _retained: Option<Arc<Retained>>,
    _vulkan: Option<vulkan::Lease>,
    _golden: Option<GoldenLease>,
    mode: (u32, u32, u32, bool, bool),
    client_display: Option<HostDisplay>,
    virtual_layout: Option<String>,
    /// The device whose stream this is, when it keeps a layout the user chose
    /// during the stream: a stream on its own virtual display whose client did
    /// not choose the layout itself.
    layout_owner: Option<String>,
    /// The app or host layout a layout learned for the device replaces.
    inherited_layout: Option<String>,
    /// The device that keeps the resolution the user chose during the stream
    /// (`remember_display_mode`): a stream on its own virtual display without
    /// a display mode of its own.
    resolution_owner: Option<String>,
    /// The scale the display showed once the stream set up, when the device keeps the scale
    /// the user picks (`remember_display_mode` and no configured scale).
    scale_baseline: Option<u32>,
    /// How the stream's own display follows a stream that turns, when
    /// `dd_rotation_option` is auto.
    follow: Option<Follow>,
    /// The identity of the virtual display it holds, by which streams lease it.
    identity: Option<String>,
    /// That display is a running game's, which this stream joined: it takes the
    /// display as it is, and never changes its mode, HDR or scale.
    joined: bool,
    /// A topology fixture keeps lifecycle tests independent of the real desktop.
    #[cfg(any(test, feature = "test-support"))]
    layout_snapshot: Option<Vec<(String, bool)>>,
    /// The resolution the stream set and the one shown, for lifecycle tests.
    #[cfg(test)]
    size_snapshot: Option<SizeChange>,
    /// The scale the display shows, for lifecycle tests.
    #[cfg(test)]
    scale_snapshot: Option<u32>,
    /// The scales put back on the display, for lifecycle tests.
    #[cfg(test)]
    scales_set: std::sync::Mutex<Vec<i64>>,
    revision: u64,
    /// Pure display session lifecycle state machine (recovery, layout/primary watches,
    /// departure/wake epochs, pause/resume transitions).
    machine: DisplaySessionMachine,
    mode_report: ModeReport,
    recovery_scale: i64,
    recovery_dimensions: (u32, u32),
    recovery_profile: Option<String>,
    /// The stream uses a virtual display, as its frame policy was resolved for.
    virtual_mode: bool,
    /// The HDR profile the device had chosen (`hdr_profile`), which a resume compares.
    profile_choice: Option<String>,
    host: std::sync::Weak<crate::state::Host>,
}

impl Prepared {
    /// Nothing prepared: a stream of `output` that holds no display, layout,
    /// profile or lease.
    fn bare(
        h: &Shared,
        output: String,
        framegen: Policy,
        mode: (u32, u32, u32, bool, bool),
        mode_report: ModeReport,
    ) -> Self {
        Self {
            display: None,
            output,
            framegen,
            _arrangement: None,
            _activation: None,
            _profile: None,
            _retained: None,
            _vulkan: None,
            _golden: None,
            mode,
            client_display: None,
            virtual_layout: None,
            layout_owner: None,
            inherited_layout: None,
            resolution_owner: None,
            scale_baseline: None,
            follow: None,
            identity: None,
            joined: false,
            #[cfg(any(test, feature = "test-support"))]
            layout_snapshot: None,
            #[cfg(test)]
            size_snapshot: None,
            #[cfg(test)]
            scale_snapshot: None,
            #[cfg(test)]
            scales_set: std::sync::Mutex::new(Vec::new()),
            revision: 0,
            // A game window holds no layout to keep while paused.
            machine: DisplaySessionMachine::launch(
                std::time::Instant::now(),
                display_arrangement::departures(),
                display_arrangement::wake_layouts(),
                false,
                false,
            ),
            mode_report,
            recovery_scale: 0,
            recovery_dimensions: (mode.0, mode.1),
            recovery_profile: None,
            virtual_mode: false,
            profile_choice: None,
            host: Arc::downgrade(h),
        }
    }

    fn capture_target(&self) -> (String, u64) {
        self._retained.as_ref().map_or_else(
            || (self.output.clone(), self.revision),
            |display| display.capture_target(),
        )
    }
}
