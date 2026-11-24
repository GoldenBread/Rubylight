//! Host-side display session policy adapters connecting `Config`, `Launch`, `Client`,
//! `App`, and `Shared` to [`butterpollo_media_policy::display_session_machine`].
use super::{DeviceMode, Ready};
use crate::state::{Launch, Shared};
use anyhow::Result;
use butterpollo_config::config::Config;
use butterpollo_media_policy::{
    display_policy::{Arrangement, HostDisplay},
    display_session_machine as machine,
    framegen::{Policy, Rate},
    rtsp::Negotiated,
};
use butterpollo_session::session::{Role, Transport};
use std::sync::Arc;

/// The device setting naming the app or host layout a learned device layout replaced, and the
/// one marking a layout the user picked (both in `butterpollo_session::state`).
pub use butterpollo_session::state::{LEARNED_OVER, SET_BY_USER};
/// What a device with `remember_display_mode` keeps per screen (a second screen reads its own:
/// `Client::second_screen`).
pub(crate) use butterpollo_session::state::{REMEMBERED_RESOLUTION, REMEMBERED_SCALE};

#[cfg(test)]
pub(super) use machine::DisplayChanges;
pub(super) use machine::{
    JoinedGame, brings_back_game, display_ask, display_changes, game_display_after,
    joined_hdr_mismatch, joined_report, layout_reason, pick_second_monitor, primary_layout,
    resumed_profile,
};

/// Set on a WebRTC launch whose browser runs on this PC.
pub(crate) const BROWSER_ON_THIS_PC: &str = "rlBrowserOnThisPc";
/// Set on a launch that asks for the running game (`/resume`, the prompt's Stream
/// here too, a Resume tile). The host decides this, never the client's query.
pub const RESUMES_GAME: &str = "rlResumesGame";

pub(super) fn stream_mode(stream: &Negotiated) -> (u32, u32, u32, bool, bool) {
    let (width, height) = stream.display_size();
    (
        width,
        height,
        stream.fps_millihz(),
        stream.hdr,
        stream.vrr_low_latency,
    )
}

fn session_role(role: Role) -> machine::SessionRole {
    match role {
        Role::Stream => machine::SessionRole::Stream,
        Role::RemoteMonitor => machine::SessionRole::RemoteMonitor,
        Role::InputOnly => machine::SessionRole::InputOnly,
    }
}

fn session_transport(transport: Transport) -> machine::SessionTransport {
    match transport {
        Transport::WebRtc => machine::SessionTransport::WebRtc,
        _ => machine::SessionTransport::Moonlight,
    }
}

pub(super) fn virtual_display_mode<'a>(
    config: &'a Config,
    client: &'a butterpollo_session::state::Client,
    app: Option<&'a butterpollo_session::state::App>,
    windows_11: bool,
) -> &'a str {
    if butterpollo_session::background::enabled(config) {
        return "per_client";
    }
    client
        .extra
        .get("virtual_display_mode")
        .and_then(serde_json::Value::as_str)
        // The old console stored "global" for the host setting.
        .filter(|s| !s.is_empty() && *s != "global")
        .or_else(|| {
            app.and_then(|app| app.extra.get("virtual-display-mode"))
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty())
        })
        .unwrap_or(config.virtual_display_mode(windows_11))
}

/// What deciding on a join needs of the stream ([`joined_display`]).
pub(super) struct JoinAsk<'a> {
    pub(super) client: &'a str,
    pub(super) role: Role,
    /// The launch was a `/resume` or the prompt's Stream here too ([`RESUMES_GAME`]).
    pub(super) resumes: bool,
    /// The app it launched.
    pub(super) launched: Option<u32>,
    pub(super) virtual_mode: bool,
    /// `virtual_display_mode`, and the identity that gives this stream's display.
    pub(super) display_mode: &'a str,
    pub(super) identity: &'a str,
    /// A press of Continue here is under way: the game's display goes with the
    /// previous owner, and the device that pressed has none yet.
    pub(super) claim_live: bool,
}

/// The identity of the display a stream shares because it asked for the running game.
pub(super) fn joined_display(ask: &JoinAsk<'_>, game: Option<JoinedGame<'_>>) -> Option<String> {
    let pure_ask = machine::JoinAsk {
        client: ask.client,
        role: session_role(ask.role),
        resumes: ask.resumes,
        launched: ask.launched,
        virtual_mode: ask.virtual_mode,
        display_mode: ask.display_mode,
        identity: ask.identity,
        claim_live: ask.claim_live,
        second_screen: is_second_screen(ask.client),
    };
    machine::joined_display(&pure_ask, game)
}

/// Whether `client` is a device's second screen (Dual screen mode).
pub(super) fn is_second_screen(client: &str) -> bool {
    butterpollo_session::state::device_uuid(client) != client
}

/// [`pick_second_monitor`] on this PC, for the stream whose first screen shows
/// `configured` (the output setting; empty for the primary monitor, as a physical
/// stream's display is chosen).
pub(super) fn second_monitor(configured: &str) -> Option<String> {
    let monitors = butterpollo_win_display::display::monitors()
        .inspect_err(|error| tracing::warn!(error = %format!("{error:#}"), "the PC's monitors could not be read for the second screen"))
        .ok()?;
    let active: Vec<String> = butterpollo_win_graphics::capture::displays()
        .ok()?
        .into_iter()
        .map(|d| d.display_name)
        .collect();
    let first = if configured.is_empty() {
        monitors.iter().find(|m| m.primary)
    } else {
        monitors.iter().find(|m| m.matches(configured))
    }
    .map(|m| m.display_name.as_str());
    let candidates: Vec<(&str, bool)> = monitors
        .iter()
        .filter(|m| {
            active
                .iter()
                .any(|a| a.eq_ignore_ascii_case(&m.display_name))
        })
        .map(|m| (m.display_name.as_str(), m.is_rubylight()))
        .collect();
    pick_second_monitor(&candidates, first).map(str::to_owned)
}

/// The display this launch would join, by [`joined_display`], whether or not it is
/// still there.
pub(super) fn asked_game_display(
    h: &Shared,
    launch: &Launch,
    app: Option<&butterpollo_session::state::App>,
    virtual_mode: bool,
    display_mode: &str,
    identity: &str,
) -> Option<String> {
    let ask = JoinAsk {
        client: &launch.client.uuid,
        role: launch.role,
        resumes: launch.options.contains_key(RESUMES_GAME),
        launched: app.map(|a| a.id()),
        virtual_mode,
        display_mode,
        identity,
        claim_live: h.claim_live(),
    };
    joined_display(
        &ask,
        h.current_app.lock().unwrap().as_ref().map(|r| JoinedGame {
            id: r.id,
            display: r.display.as_deref(),
            background: r.background,
            desktop: r.is_desktop(),
        }),
    )
}

/// The display a launch resumes, if `serves` says it still does for the stream. A
/// launch that asked for the running game does not take its own empty display over
/// the game's, while that is `live`: only a new setup can join it. Handed back as by
/// [`crate::state::take_retained`], for the caller to drop outside the lock.
pub(crate) fn take_retained_display(
    h: &Shared,
    launch: &Launch,
    config: &Config,
    app: Option<&butterpollo_session::state::App>,
    serves: impl FnOnce(&Ready) -> bool,
    live: impl FnOnce(&str) -> bool,
) -> std::result::Result<Arc<Ready>, Option<Arc<Ready>>> {
    let mode = virtual_display_mode(
        config,
        &launch.client,
        app,
        butterpollo_win_display::display::virtual_display_os(),
    );
    let identity = virtual_display_identity(config, launch, app);
    let game = asked_game_display(h, launch, app, true, mode, &identity).filter(|id| live(id));
    crate::state::take_retained(
        &mut h.app_display.lock().unwrap(),
        &launch.client.uuid,
        |lease| serves(lease) && lease.shows(game.as_deref()),
    )
}

/// Where the running game is once the stream of a press that ended without it came
/// after all (`stream`: that device's display).
pub(crate) fn display_of_new_owner(
    current: Option<&str>,
    stream: Option<&Ready>,
) -> Option<String> {
    game_display_after(
        current,
        true,
        true,
        stream.is_some_and(|stream| stream.joined),
        stream.and_then(|stream| stream.identity.as_deref()),
    )
}

/// Note where the running game is after this launch's stream was set up.
pub(crate) fn record_game_display(h: &Shared, launch: &Launch, display: &Ready) {
    if launch.role != Role::Stream {
        return;
    }
    let current = h
        .current_app
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|app| app.display.clone());
    // Outside the app's lock, which every server info takes: a display can be
    // busy with Windows.
    let current_live = current
        .as_deref()
        .is_some_and(|id| butterpollo_win_display::display::joinable_mode(id).is_some());
    if let Some(app) = h
        .current_app
        .lock()
        .unwrap()
        .as_mut()
        .filter(|app| app.id == launch.app_id)
    {
        app.display = game_display_after(
            app.display.as_deref(),
            current_live,
            app.owner == launch.client.uuid,
            display.joined,
            display.identity.as_deref(),
        );
    }
}

pub(super) fn virtual_display_identity(
    config: &Config,
    launch: &Launch,
    app: Option<&butterpollo_session::state::App>,
) -> String {
    if !butterpollo_session::background::enabled(config)
        && app.is_some_and(|a| crate::process::app_bool(a, "use-app-identity", false))
    {
        let app_id = app
            .and_then(|a| a.extra.get("uuid"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("application");
        if app.is_some_and(|a| crate::process::app_bool(a, "per-client-app-identity", false)) {
            butterpollo_media_policy::display_policy::app_client_identity(
                app_id,
                launch
                    .options
                    .get("uniqueid")
                    .filter(|value| uuid::Uuid::parse_str(value).is_ok())
                    .unwrap_or(&launch.client.uuid),
            )
        } else {
            app_id.to_owned()
        }
    } else {
        launch.client.uuid.clone()
    }
}

pub(crate) fn virtual_display_layout<'a>(
    config: &'a Config,
    client: &'a butterpollo_session::state::Client,
    app: Option<&'a butterpollo_session::state::App>,
    requested: Option<HostDisplay>,
) -> &'a str {
    // A background stream must not move the PC's windows or let its mouse
    // wander onto the device's display, whatever the device asks for.
    if butterpollo_session::background::enabled(config) {
        return butterpollo_session::background::LAYOUT;
    }
    if let Some(HostDisplay::Virtual(arrangement)) = requested {
        return arrangement.name();
    }
    let inherited = inherited_layout(config, app);
    client
        .extra
        .get("virtual_display_layout")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty() && *s != "global")
        // A layout learned at the end of a stream stands in for the app or host
        // layout it replaced, until that layout changes (issue #40).
        .filter(|_| {
            client
                .extra
                .get(LEARNED_OVER)
                .and_then(serde_json::Value::as_str)
                .is_none_or(|over| over == inherited)
        })
        .unwrap_or(inherited)
}

/// Whether this launch runs [`Arrangement::Primary`] instead of `layout`.
pub(crate) fn browser_keeps_monitors(launch: &Launch, layout: &str, virtual_layout: bool) -> bool {
    machine::browser_keeps_monitors(
        session_transport(launch.transport),
        launch.options.contains_key(BROWSER_ON_THIS_PC),
        layout,
        virtual_layout,
    )
}

/// The device that keeps the layout this stream ends with, if any.
pub(super) fn layout_owner(
    launch: &Launch,
    learnable: bool,
    monitors_kept: bool,
    planned: Option<Arrangement>,
    in_force: Option<Arrangement>,
) -> Option<String> {
    machine::layout_owner(
        session_role(launch.role),
        &launch.client.uuid,
        learnable,
        monitors_kept,
        planned,
        in_force,
    )
}

/// The virtual display layout of this launch.
pub(crate) fn launch_display_layout<'a>(
    config: &'a Config,
    launch: &'a Launch,
    app: Option<&'a butterpollo_session::state::App>,
    requested: Option<HostDisplay>,
) -> &'a str {
    let layout = virtual_display_layout(config, &launch.client, app, requested);
    if browser_keeps_monitors(launch, layout, true) {
        return Arrangement::Primary.name();
    }
    layout
}

/// Whether the device's own virtual display layout is one the user set rather
/// than one learned from a stream.
pub(super) fn has_user_layout(client: &butterpollo_session::state::Client) -> bool {
    client
        .extra
        .get("virtual_display_layout")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|s| !s.is_empty() && s != "global")
        && client
            .extra
            .get(SET_BY_USER)
            .and_then(serde_json::Value::as_bool)
            == Some(true)
}

/// The virtual display layout without the device's own: the app's, then the host's.
pub(super) fn inherited_layout<'a>(
    config: &'a Config,
    app: Option<&'a butterpollo_session::state::App>,
) -> &'a str {
    app.and_then(|app| app.extra.get("virtual-display-layout"))
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty() && *s != "global")
        .or_else(|| {
            app.filter(|app| crate::process::app_bool(app, "virtual-display-primary", false))
                .map(|_| "extended_primary")
        })
        .unwrap_or(config.get("virtual_display_layout", "exclusive"))
}

/// The virtual display layout the host asks for itself: what a device layout from before the
/// marker existed has to be compared with.
pub(crate) fn legacy_host_layout(config: &Config) -> &str {
    config.get("virtual_display_layout", "exclusive")
}

/// Give a device's unmarked "extended" layout from before the marker existed its meaning back.
pub(crate) fn migrate_legacy_extended_layouts(
    clients: &mut [butterpollo_session::state::Client],
    host_layout: &str,
) -> Vec<String> {
    let mut changed = Vec::new();
    for client in clients {
        let layout = client
            .extra
            .get("virtual_display_layout")
            .and_then(serde_json::Value::as_str);
        let chosen = client
            .extra
            .get(SET_BY_USER)
            .and_then(serde_json::Value::as_bool)
            == Some(true);
        if layout != Some("extended") || client.extra.contains_key(LEARNED_OVER) || chosen {
            continue;
        }
        if primary_layout(host_layout) {
            tracing::info!(
                device = %client.name,
                "the \"extended\" layout learned before the marker existed was Windows moving the primary; the device follows the host layout again"
            );
            client.extra.remove("virtual_display_layout");
        } else {
            tracing::info!(
                device = %client.name,
                learned_over = host_layout,
                "the \"extended\" layout from before the marker existed stays until the host layout changes"
            );
            client
                .extra
                .insert(LEARNED_OVER.into(), serde_json::json!(host_layout));
        }
        changed.push(client.name.clone());
    }
    changed
}

/// The one-time console notice for the devices [`migrate_legacy_extended_layouts`] changed.
pub(crate) fn legacy_layout_notice(devices: &[String], host_layout: &str) -> String {
    let devices = devices.join(", ");
    if primary_layout(host_layout) {
        format!("{devices} follow the host's display layout again.")
    } else {
        format!(
            "{devices} keep Extend, now marked as learned: an app's own layout or a later change of the host's layout replaces it. Pick it again under Devices to keep it everywhere."
        )
    }
}

/// What the client asked the PC's displays to do for this stream (the
/// `hostDisplay` launch parameter), or None to follow the host, app and
/// per-device settings. Only a stream chooses; a remote monitor keeps the
/// remote monitor layout, and Remote Input has no display.
pub fn client_display(launch: &Launch) -> Option<HostDisplay> {
    if launch.role != Role::Stream {
        return None;
    }
    let value = launch.options.get(HostDisplay::PARAMETER);
    let choice = value.and_then(|value| HostDisplay::parse(value));
    if let Some(value) = value
        && choice.is_none()
        && !value.trim().is_empty()
        && value.trim() != "default"
    {
        tracing::info!(value = %value, "unknown hostDisplay launch value; the display settings decide");
    }
    // `virtualDisplay=0` is not a choice: Apollo-compatible clients send it
    // whenever their own virtual display toggle is off, and Apollo reads it as
    // "not forced". Taking it as "physical" (2.5.6 to 2.5.9) overrode the host's
    // virtual display for those clients, which then mirrored the PC's monitor.
    choice
}

/// The HDR profile the device chose (`hdr_profile`), if any.
pub(super) fn profile_choice(launch: &Launch) -> Option<String> {
    launch
        .client
        .extra
        .get("hdr_profile")
        .and_then(serde_json::Value::as_str)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

/// Puts `wanted` in `slot` through `change`, which gets the lease held (to switch in place,
/// Ok(None)) or none (to acquire, Ok(Some)). The held lease is never let go first, so an error
/// leaves the slot as it was. No profile wanted takes the lease off.
pub(super) fn take_profile<L>(
    slot: &mut Option<L>,
    wanted: Option<&str>,
    change: impl FnOnce(Option<&mut L>, &str) -> Result<Option<L>>,
) -> Result<()> {
    let Some(wanted) = wanted else {
        slot.take();
        return Ok(());
    };
    if let Some(acquired) = change(slot.as_mut(), wanted)? {
        *slot = Some(acquired);
    }
    Ok(())
}

/// The frame limiter and the display refresh for `stream`: the stream's rate,
/// the app's frame generation, the device's own display mode, VRR and
/// Lossless Scaling. Also returns that display mode.
pub(super) fn frame_policy(
    app: Option<&butterpollo_session::state::App>,
    launch: &Launch,
    stream: &Negotiated,
    config: &Config,
    virtual_mode: bool,
) -> Result<(Policy, Option<DeviceMode>)> {
    let option = |key: &str| {
        app.and_then(|a| a.extra.get(key))
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
    };
    let generation = option("frame-generation-mode")
        .or_else(|| option("frame-generation-provider"))
        .unwrap_or("none");
    let generation_enabled = if option("frame-generation-mode").is_some() {
        butterpollo_media_policy::framegen::generation_provider(generation) != "none"
    } else {
        app.as_ref().is_some_and(|a| {
            [
                "frame-generation-enabled",
                "gen1-framegen-fix",
                "gen2-framegen-fix",
                "frame-generation-capture-fix",
            ]
            .iter()
            .any(|key| crate::process::app_bool(a, key, false))
        })
    };
    let device_mode = launch
        .client
        .extra
        .get("display_mode")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .and_then(|text| {
            let mode = butterpollo_media_policy::display_policy::parse_display_mode(text);
            if mode.is_none() {
                tracing::warn!(
                    display_mode = text,
                    "ignoring a device display mode that is not WIDTHxHEIGHTxREFRESH"
                );
            }
            mode
        });
    let mut framegen = Policy::resolve(
        config,
        Rate(stream.fps_millihz()),
        virtual_mode,
        generation,
        generation_enabled,
        launch
            .client
            .extra
            .get("config_overrides")
            .and_then(serde_json::Value::as_object)
            .is_some_and(|o| o.contains_key("rtss_frame_limit_type"))
            || app
                .as_ref()
                .and_then(|a| a.extra.get("config-overrides"))
                .and_then(serde_json::Value::as_object)
                .is_some_and(|o| o.contains_key("rtss_frame_limit_type")),
    )?;
    if let Some((_, _, rate)) = device_mode {
        framegen.display_rate = butterpollo_media_policy::framegen::display_rate(
            rate,
            virtual_mode,
            butterpollo_media_policy::framegen::virtual_refresh(config),
        );
    }
    framegen = framegen.with_vrr(
        config,
        virtual_mode,
        stream.vrr_low_latency || launch.vrr_requested,
    );
    if let Some(limit) = app
        .as_ref()
        .and_then(|a| serde_json::to_value(a).ok())
        .and_then(|a| {
            butterpollo_media_policy::lossless::options(
                &a,
                config,
                f64::from(stream.fps_millihz()) / 1000.,
            )
        })
        .and_then(|options| options.frame_limit)
    {
        framegen.rate = Rate(limit.saturating_mul(1000).min(1_000_000));
        framegen.enabled = true;
        framegen.follows_display = false;
    }
    let provider = butterpollo_win_io::limiter::provider_usable(config);
    if !provider
        && framegen.follows_display
        && framegen.provider != butterpollo_media_policy::framegen::Provider::Rtss
    {
        butterpollo_win_io::limiter::warn_automatic_cap_needs_rtss_once(config);
    }
    framegen = framegen.with_provider_available(provider);
    if framegen.enabled {
        framegen = framegen.for_gpu(streams_on_nvidia(config));
    }
    Ok((framegen, device_mode))
}

/// Whether the GPU this host streams with is NVIDIA's, for RTSS's Reflex mode.
#[cfg(not(any(test, feature = "test-support")))]
fn streams_on_nvidia(config: &Config) -> bool {
    butterpollo_win_graphics::capture::stream_gpu_vendor(config)
        .is_some_and(butterpollo_media_policy::encoder_gpu::is_nvidia)
}
/// Tests stream on no NVIDIA GPU.
#[cfg(any(test, feature = "test-support"))]
fn streams_on_nvidia(_: &Config) -> bool {
    false
}
