//! Virtual display resolution, shared/joined identity selection, and display
//! arrangement acquisition and verification during stream preparation.
use super::policy::{
    REMEMBERED_RESOLUTION, asked_game_display, browser_keeps_monitors, is_second_screen,
    second_monitor, virtual_display_identity, virtual_display_layout, virtual_display_mode,
};
use crate::state::{Launch, Shared};
use anyhow::Result;
use butterpollo_config::config::Config;
use butterpollo_media_policy::display_policy::{Arrangement, HostDisplay};
use butterpollo_session::session::Role;
use butterpollo_win_display::{display, display_arrangement};

/// Resolved virtual-display availability and target physical output for a launch.
pub(super) struct VirtualSelection<'a> {
    pub(super) mode: &'a str,
    pub(super) client_virtual: Option<bool>,
    pub(super) virtual_mode: bool,
    pub(super) output: String,
}

/// Decide whether this launch uses a virtual display, warn when fallback occurs,
/// and resolve a second screen's alternate physical monitor when streaming physically.
#[allow(clippy::too_many_arguments)]
pub(super) fn select_virtual_mode<'a>(
    launch: &'a Launch,
    app: Option<&'a butterpollo_session::state::App>,
    config: &'a Config,
    client_display: Option<HostDisplay>,
    background: bool,
    output_override: Option<&'a str>,
    output: &str,
    physical_only: bool,
) -> Result<VirtualSelection<'a>> {
    let mode = virtual_display_mode(config, &launch.client, app, display::virtual_display_os());
    let client_virtual = launch
        .options
        .get("virtualDisplay")
        .map(|value| value != "0");
    let mut display_request = butterpollo_media_policy::display_policy::VirtualDisplayRequest {
        client_requested: client_virtual == Some(true)
            || matches!(client_display, Some(HostDisplay::Virtual(_))),
        // Streaming the PC's own monitor would show the device whatever
        // the person at the PC is doing.
        client_physical: client_display == Some(HostDisplay::Physical) && !background,
        client_forced: background
            || launch
                .client
                .extra
                .get("always_use_virtual_display")
                .is_some_and(|v| v == true || v == "true"),
        app_requested: app.is_some_and(|a| {
            crate::process::app_bool(a, "virtual-display", false)
                // Apollo's earlier name, which loading the library also
                // moves (`migrate_app`).
                || crate::process::app_bool(a, "virtual-screen", false)
        }),
        configured: mode != "disabled" || config.boolean("dd_activate_virtual_display", false),
        output_override,
        configured_output: config.get("output_name", ""),
        headless: false,
    };
    if !physical_only && !display_request.requested() {
        display_request.headless =
            butterpollo_win_graphics::capture::displays().is_ok_and(|displays| displays.is_empty());
        if display_request.headless {
            tracing::info!(
                "no display is active; this stream uses a virtual display although the settings or the client turn it off"
            );
        }
    }
    let virtual_mode =
        display_request.uses_virtual(physical_only, display::ensure_virtual_display_available);
    // A device's second screen shows another monitor than its first one, which
    // would otherwise show both screens the same picture.
    let second_screen_monitor =
        (!virtual_mode && launch.role == Role::Stream && is_second_screen(&launch.client.uuid))
            .then(|| second_monitor(output))
            .flatten();
    if !virtual_mode && launch.role == Role::Stream && is_second_screen(&launch.client.uuid) {
        match &second_screen_monitor {
            Some(monitor) => {
                tracing::info!(monitor = %monitor, first = %output, "the second screen streams another monitor");
                launch.warnings.clear("display_second_screen");
            }
            None => launch.warnings.set(
                "display_second_screen",
                "This PC streams its own monitor and has no other one, so the second screen shows the same picture as the first. Turn on the virtual display, or connect a second monitor, for a second picture.",
            ),
        }
    }
    let resolved_output = second_screen_monitor.unwrap_or_else(|| output.to_owned());
    if background && !virtual_mode && launch.role == Role::Stream {
        anyhow::bail!(
            "background streaming needs Rubylight's virtual display, which is unavailable{}; it never streams the PC's own monitor",
            if physical_only {
                " while Windows is locked"
            } else {
                ""
            }
        );
    }
    if physical_only && display_request.requested() {
        launch.warnings.set("display_virtual", "Windows was locked and the virtual display could not be set up there, so this stream shows the physical display. Start the stream again after signing in to use the virtual display.");
    } else if display_request.requested()
        && !virtual_mode
        && let Some(reason) = butterpollo_media_policy::os_gate::missing(
            display::windows_build(),
            butterpollo_media_policy::os_gate::Feature::VirtualDisplay,
        )
    {
        launch.warnings.set("display_virtual", format!("Using the physical display because this Windows cannot load the virtual display driver. {reason} Update to Windows 11 23H2 or later to use a virtual display."));
    } else if display_request.requested() && !virtual_mode {
        launch.warnings.set("display_virtual", "Using a physical display because the virtual display driver is unavailable. The desktop is visible locally and its refresh can limit fresh frames; repair the virtual display driver (run \"butterpollo-setup.exe --repair-drivers --now\" from the Rubylight folder in an administrator shell) or explicitly select the physical display.");
    } else {
        launch.warnings.clear("display_virtual");
    }
    Ok(VirtualSelection {
        mode,
        client_virtual,
        virtual_mode,
        output: resolved_output,
    })
}

/// Resolved virtual display identity and whether this stream shares or joins an
/// existing display.
pub(super) struct VirtualIdentity {
    pub(super) shared_id: String,
    pub(super) joined_mode: Option<(u32, u32, u32)>,
    pub(super) shares: bool,
}

/// Resolve the stable virtual display identity, creating a shared GUID if needed,
/// and check whether a joinable running game display should be joined.
pub(super) fn resolve_virtual_identity(
    h: &Shared,
    launch: &Launch,
    app: Option<&butterpollo_session::state::App>,
    config: &Config,
    virtual_mode: bool,
    mode: &str,
) -> Result<VirtualIdentity> {
    let id = virtual_display_identity(config, launch, app);
    let shared_id = if mode == "shared" {
        let mut paired = h.paired.write().unwrap();
        let existing = paired.document["root"]["shared_virtual_display_guid"]
            .as_str()
            .filter(|value| uuid::Uuid::parse_str(value).is_ok())
            .map(str::to_owned);
        if let Some(id) = existing {
            id
        } else {
            let id = uuid::Uuid::new_v4().to_string();
            paired.document["root"]["shared_virtual_display_guid"] = id.clone().into();
            paired.save(&h.paired_path)?;
            id
        }
    } else {
        id
    };
    // Stream here too: the game is on the display it is on. A display of its
    // own would show this device an empty desktop.
    let joined_id = asked_game_display(h, launch, app, virtual_mode, mode, &shared_id);
    // Gone with its last stream: this stream makes its own, as usual.
    let joined = joined_id.and_then(|id| display::joinable_mode(&id).map(|mode| (id, mode)));
    let joined_mode = joined.as_ref().map(|(_, mode)| *mode);
    let shares = mode == "shared" || joined.is_some();
    let shared_id = joined.map_or(shared_id, |(id, _)| id);
    Ok(VirtualIdentity {
        shared_id,
        joined_mode,
        shares,
    })
}

/// Whether this stream remembers a resolution/scale chosen in Windows and any
/// previously saved resolution.
pub(super) fn remembered_resolution(
    launch: &Launch,
    virtual_mode: bool,
    shares: bool,
    has_device_mode: bool,
) -> (bool, Option<(u32, u32)>) {
    let remember = launch.role == Role::Stream
        && virtual_mode
        && !shares
        && !has_device_mode
        && launch
            .client
            .extra
            .get("remember_display_mode")
            .is_some_and(|v| v == true || v == "true");
    let resolution = if remember {
        launch
            .client
            .extra
            .get(REMEMBERED_RESOLUTION)
            .and_then(serde_json::Value::as_str)
            .and_then(butterpollo_media_policy::display_policy::parse_resolution)
    } else {
        None
    };
    (remember, resolution)
}

/// Acquired display arrangement and verification results for a prepared launch.
pub(super) struct LayoutResult<'a> {
    pub(super) configured: &'a str,
    pub(super) selection: &'a str,
    pub(super) monitors_kept: bool,
    pub(super) parsed: Option<Arrangement>,
    pub(super) arrangement: Option<display_arrangement::Lease>,
    pub(super) layout_took: bool,
    pub(super) start_primary_mismatch: bool,
}

/// Select, parse, acquire, apply virtual mode after layout, and verify whether
/// Windows took the requested display arrangement.
#[allow(clippy::too_many_arguments)]
pub(super) fn acquire_and_verify_layout<'a>(
    h: &Shared,
    launch: &'a Launch,
    app: Option<&'a butterpollo_session::state::App>,
    config: &'a Config,
    client_display: Option<HostDisplay>,
    virtual_mode: bool,
    virtual_layout: bool,
    output: &str,
    display: Option<&display::Guard>,
    reapply_mode: bool,
    original: Option<display::Snapshot>,
) -> Result<LayoutResult<'a>> {
    // The client's choice for this stream comes before the device, app
    // and host layouts.
    let configured = if virtual_layout {
        virtual_display_layout(
            config,
            &launch.client,
            app,
            client_display.filter(|_| virtual_mode),
        )
    } else {
        config.get("dd_configuration_option", "verify_only")
    };
    let monitors_kept = browser_keeps_monitors(launch, configured, virtual_layout);
    let selection = if monitors_kept {
        Arrangement::Primary.name()
    } else {
        configured
    };
    // An unknown layout falls back to the default: exclusive for a
    // virtual display, verify only (no arrangement) for a physical one.
    // A remote monitor takes which displays stay on and which is primary
    // from it; the remote monitor layout places it.
    let parsed = if !matches!(launch.role, Role::Stream | Role::RemoteMonitor)
        || matches!(selection, "disabled" | "verify_only")
    {
        None
    } else {
        Arrangement::parse(configured)
            .inspect_err(|_| {
                butterpollo_config::config::invalid(
                    if virtual_layout {
                        "virtual_display_layout"
                    } else {
                        "dd_configuration_option"
                    },
                    configured,
                )
            })
            .ok()
            .or(virtual_layout.then_some(Arrangement::Exclusive))
            .map(|parsed| {
                if monitors_kept {
                    Arrangement::Primary
                } else {
                    parsed
                }
            })
            .and_then(|parsed| {
                if launch.role == Role::RemoteMonitor {
                    parsed.for_remote_monitor()
                } else {
                    Some(parsed)
                }
            })
    };
    let arrangement = match parsed {
        Some(parsed) => {
            let retained = crate::remote_display::outputs(h);
            match display_arrangement::Lease::acquire(
                output,
                parsed,
                &retained,
                virtual_mode && display.is_some(),
                original,
                butterpollo_session::state::device_uuid(&launch.client.uuid),
            ) {
                Ok(lease) => Some(lease),
                // The remote monitor still works where the remote monitor
                // layout put it.
                Err(error) if launch.role == Role::RemoteMonitor => {
                    tracing::warn!(error = %format!("{error:#}"), layout = selection, "remote monitor layout could not be applied");
                    launch.warnings.set("display_layout", format!("The virtual display layout ({selection}) could not be applied to this remote monitor ({error:#}), so the other displays stay as they are. Check Windows display settings and reconnect."));
                    None
                }
                Err(error) => return Err(error),
            }
        }
        _ => None,
    };
    if let Some(display) = display.filter(|_| reapply_mode) {
        // Applying the layout can recall the display's saved mode.
        display.apply_virtual_mode("after layout");
    }
    let in_force = arrangement
        .as_ref()
        .map(display_arrangement::Lease::arrangement);
    let expected = in_force.map_or(selection, |arrangement| arrangement.name());
    // A monitor still on its way back leaves the stream's display alone for now, which
    // says nothing of the layout.
    let mut start_primary_mismatch = false;
    let layout_took = if !virtual_layout
        || arrangement
            .as_ref()
            .is_none_or(display_arrangement::Lease::after_cut_watch)
    {
        true
    } else {
        match display::monitors() {
            Ok(monitors) => {
                let active: Vec<_> = monitors
                    .into_iter()
                    .map(|m| (m.display_name, m.primary))
                    .collect();
                match butterpollo_media_policy::display_policy::layout_mismatch(
                    expected, output, &active,
                ) {
                    None => true,
                    Some(actual) => {
                        // Only the primary is the watch's to put back; the card drops
                        // the warning once it did.
                        start_primary_mismatch =
                            expected == "extended_primary" && actual == "extended";
                        tracing::warn!(
                            output = %output,
                            planned = expected,
                            actual,
                            "Windows did not apply the virtual display layout"
                        );
                        launch.warnings.set(
                            "display_layout",
                            format!("Windows did not apply the {expected} display layout; the stream's display is {actual}. Check that no other display tool rearranges displays, then reconnect."),
                        );
                        false
                    }
                }
            }
            // A layout Windows would not report is not one it took.
            Err(_) => false,
        }
    };
    Ok(LayoutResult {
        configured,
        selection,
        monitors_kept,
        parsed,
        arrangement,
        layout_took,
        start_primary_mismatch,
    })
}
