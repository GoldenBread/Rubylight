//! Connecting a stream: the launch on a worker thread, its recovery and route checks, the
//! session configuration, and the decisions behind it, backed by `nightjar-policy` and
//! `nightjar-core::connect::*`.

use super::*;
use nightjar_core::connect::{
    launch as core_launch, lifecycle as core_lifecycle, preflight as core_preflight,
};
use nightjar_policy::connect_text::{self, HttpCauseKey, StageKey, StartCauseKey, TerminationTextKey};

mod attempts;
pub use attempts::Attempts;
mod early;
mod run;
pub use early::env_flag;
pub(super) use early::{
    BEAM_RACE_NOT_WIRED, BEAM_RACE_NO_GPU, BeamRace, EARLY_NO_RECORDS, EarlyState, beam_race,
    early_decode, early_state, flag_value, race_request, race_runs, takes_rfi,
    video_capabilities,
};
#[cfg(test)]
pub(super) use early::{MAX_RACE_SLICES, RaceRequest, beam_race_for, beam_slices, slices_override};
mod worker;
pub use worker::{CONNECT_TIMEOUT, ConnectFailed, Connecting, LEAVE_WAIT, connect_overdue, end_all_before};
#[cfg(test)]
pub use worker::end_before;
#[cfg(test)]
pub(super) use worker::wait_for_previous;
use worker::ConnectError;

#[cfg(test)]
pub(super) use core_preflight::{asks_for_jumbo, facts_packet_size, jumbo_config_size, reset_packet_size, resume_packet_size, JumboPreflight, JumboSkip};
#[cfg(not(test))]
use core_preflight::{facts_packet_size, reset_packet_size, resume_packet_size, JumboPreflight, JumboSkip};

fn to_route_location(location: Location) -> core_preflight::RouteLocation {
    match location {
        Location::Auto => core_preflight::RouteLocation::Auto,
        Location::Local => core_preflight::RouteLocation::Local,
        Location::Remote => core_preflight::RouteLocation::Remote,
    }
}

fn from_route_location(location: core_preflight::RouteLocation) -> Location {
    match location {
        core_preflight::RouteLocation::Auto => Location::Auto,
        core_preflight::RouteLocation::Local => Location::Local,
        core_preflight::RouteLocation::Remote => Location::Remote,
    }
}

fn initial_stage_text() -> &'static str {
    en("Starting the app on the PC")
}

fn reconnecting_stage_text(attempt: usize) -> String {
    trf("Reconnecting · attempt {} of {}", &[&attempt, &nightjar_policy::recovery::MAX_ATTEMPTS])
}

fn stage_text(stage: Stage) -> &'static str {
    match nightjar_core::connect::stage_key(stage) {
        StageKey::FindingPc => en("Finding the PC"),
        StageKey::SettingUpStream => en("Setting up the stream"),
        StageKey::StartingStream => en("Starting the stream"),
        StageKey::StartingVideo => en("Starting video"),
        StageKey::StartingAudio => en("Starting audio"),
        StageKey::ConnectingInput => en("Connecting input"),
        StageKey::StartingApp => en("Starting the app on the PC"),
    }
}

pub fn termination_text(code: i32) -> Option<String> {
    Some(match connect_text::classify_termination(code) {
        TerminationTextKey::Graceful => return None,
        TerminationTextKey::NoVideoTraffic => {
            en("No video arrived from the PC. A firewall may be blocking UDP ports 47998-48000.").into()
        }
        TerminationTextKey::NoVideoFrame => {
            en("Video arrived but no complete picture could be assembled.").into()
        }
        TerminationTextKey::UnexpectedEarly => {
            en("The PC ended the stream before the first picture. The game may have closed; start it again.").into()
        }
        TerminationTextKey::ProtectedContent => en("The PC stopped streaming protected content.").into(),
        TerminationTextKey::FrameConversion => en("The PC could not convert the picture for streaming.").into(),
        TerminationTextKey::HostGone | TerminationTextKey::Disconnected => {
            en("The connection to the PC was lost.").into()
        }
        TerminationTextKey::VulkanRenderer | TerminationTextKey::Decoder | TerminationTextKey::Unexpected(_) => {
            let other = code;
            trf("The stream ended unexpectedly (error {}).", &[&other])
        }
    })
}

pub use nightjar_policy::launch::host_ended_stream;

pub(super) fn start_failure_text(
    error: String,
    stage: Stage,
    reason: impl FnOnce() -> Option<String>,
) -> String {
    let reason = if nightjar_policy::launch::refused_in_handshake(Some(stage.name())) {
        reason().filter(|r| !r.trim().is_empty())
    } else {
        None
    };
    doctor::with_host_reason(error, reason.as_deref())
}

pub fn connect_timeout_text() -> String {
    tr("The graphics driver or a video decoder stopped answering while Nightjar started the stream, so Nightjar stopped waiting. Connect again; if it keeps happening, update or reinstall the graphics driver.").to_owned()
}

fn http_cause(error: &nightjar_core::http::Error) -> Option<&'static str> {
    match nightjar_core::connect::http_cause_key(error)? {
        HttpCauseKey::TimedOut => Some(en("The PC did not answer in time.")),
        HttpCauseKey::ConnectionRefused => Some(en("The PC refused the connection.")),
        HttpCauseKey::ConnectionLost => Some(en("The connection to the PC was lost.")),
    }
}

fn start_cause(stage: Stage) -> Option<&'static str> {
    match nightjar_core::connect::start_cause_key(stage)? {
        StartCauseKey::DidNotFinish => Some(en("The PC did not finish starting the stream.")),
    }
}

fn map_connect_error(e: nightjar_core::connect::ConnectError) -> ConnectError {
    let cause = e.http_cause.map(|c| match c {
        HttpCauseKey::TimedOut => en("The PC did not answer in time."),
        HttpCauseKey::ConnectionRefused => en("The PC refused the connection."),
        HttpCauseKey::ConnectionLost => en("The connection to the PC was lost."),
    });
    let message = if e.local_failure == Some(nightjar_core::connect::LocalLaunchFailure::SessionChanged) {
        APP_GONE.to_owned()
    } else {
        e.message
    };
    ConnectError {
        message,
        retryable: e.retryable,
        asks_replace: e.asks_replace,
        moved_to: e.moved_to,
        packed_refused: e.packed_refused,
        cause,
    }
}

#[cfg(test)]
fn resume_moved_to(
    paired: bool,
    running: bool,
    offered: bool,
    read: impl FnOnce() -> Result<nightjar_core::http::StreamMove, nightjar_core::http::Error>,
) -> Result<Option<String>, ConnectError> {
    core_launch::resume_moved_to(paired, running, offered, read).map_err(map_connect_error)
}

fn check_resume(
    join: bool,
    paired: bool,
    running: bool,
    offered: bool,
    read: impl FnOnce() -> Result<nightjar_core::http::StreamMove, nightjar_core::http::Error>,
) -> Result<(), ConnectError> {
    core_launch::check_resume(join, paired, running, offered, read).map_err(map_connect_error)
}

const APP_GONE: &str = "The original app is no longer running on the PC. Choose an app to start a new stream.";

fn resume_refused(join: bool, code: i32, message: &str) -> Option<String> {
    use nightjar_policy::continue_here::{ResumeRefusal, resume_refusal};
    Some(match resume_refusal(join, code, message)? {
        ResumeRefusal::SessionChanged => APP_GONE.to_string(),
        ResumeRefusal::BrowserStream => {
            tr("Another device is playing this game in a browser, and this stream can’t run beside it. End that stream first.").to_string()
        }
    })
}

const RECOVERY_STOPPED: &str = en("Automatic reconnecting stopped. Your game was not quit; check the connection and try again.");
const RECOVERY_UNREACHABLE: &str = en("The PC could not be reached.");

fn recovery_gave_up(last_failure: Option<&str>) -> String {
    nightjar_policy::recovery::recovery_gave_up(last_failure, tr(RECOVERY_UNREACHABLE), tr(RECOVERY_STOPPED))
}

fn connect_with_recovery(
    plan: &mut LaunchPlan,
    shared: Arc<Shared>,
    interrupt: &Interrupt,
) -> Result<(Session, Facts), ConnectError> {
    loop {
        if let Some(recovery) = &mut plan.recovery {
            let delay = recovery
                .next_delay(recovery::now_ms())
                .ok_or_else(|| ConnectError::from(recovery_gave_up(lock(&shared.last_failure).as_deref())))?;
            shared.notify(Notice::Reconnecting(recovery.attempts() as usize));
            let until = Instant::now() + Duration::from_millis(delay);
            while Instant::now() < until {
                if interrupt.is_interrupted() {
                    return Err("Cancelled.".into());
                }
                std::thread::sleep(Duration::from_millis(20).min(until.saturating_duration_since(Instant::now())));
            }
            let port = if plan.record.http_port == 0 {
                nightjar_core::http::DEFAULT_HTTP_PORT
            } else {
                plan.record.http_port
            };
            wait_for_route(&plan.record.address, port, recovery.remaining(recovery::now_ms()), interrupt)?;
        }
        match connect(plan, shared.clone(), interrupt) {
            Err(error) if error.retryable && plan.recovery.is_some() && !interrupt.is_interrupted() => {
                eprintln!("nightjar: recovery: attempt failed: {}", error.message);
                *lock(&shared.last_failure) = error.cause.map(|cause| tr(cause).to_owned());
                lock(&shared.frames).clear();
                shared.next_attempt();
            }
            result => return result,
        }
    }
}

fn wait_for_route(
    address: &str,
    port: u16,
    remaining_ms: u64,
    interrupt: &Interrupt,
) -> Result<(), ConnectError> {
    let outcome = recovery::wait_for_route(
        recovery::now_ms() + remaining_ms,
        &recovery::now_ms,
        &|| recovery::route_problem(address, port),
        &|| interrupt.is_interrupted(),
        &|ms| {
            let until = Instant::now() + Duration::from_millis(ms);
            while Instant::now() < until {
                if interrupt.is_interrupted() {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(20).min(until.saturating_duration_since(Instant::now())));
            }
            true
        },
        &|msg| eprintln!("nightjar: {msg}"),
        &|msg| eprintln!("nightjar: {msg}"),
    );
    match outcome {
        recovery::RouteWaitOutcome::Cancelled => Err("Cancelled.".into()),
        recovery::RouteWaitOutcome::Reachable | recovery::RouteWaitOutcome::Exhausted => Ok(()),
    }
}

pub(super) fn tells_host_wireless(
    setting: bool,
    family: nightjar_core::http::Family,
    wireless: bool,
) -> bool {
    core_preflight::tells_host_wireless(
        setting,
        family == nightjar_core::http::Family::Rubylight,
        wireless,
    )
}

pub(super) fn hint_env(name: &str) -> Option<u32> {
    let text = std::env::var(name).ok()?;
    let parsed = text.trim().parse().ok();
    if parsed.is_none() {
        eprintln!("nightjar: {name}={text:?} is not a number, so it is ignored");
    }
    parsed
}

pub(super) fn hint_override(name: &str, value: u32) -> u32 {
    hint_env(name).unwrap_or(value)
}

pub(super) use core_preflight::LinkHints;

pub(super) fn link_hints(
    tell: bool,
    routed_mbps: u32,
    link_env: Option<u32>,
    measured_pace: u32,
    pace_env: Option<u32>,
) -> LinkHints {
    core_preflight::link_hints(tell, routed_mbps, link_env, measured_pace, pace_env)
}

fn location_override() -> Location {
    match std::env::var("NIGHTJAR_STREAM_LOCATION").as_deref() {
        Ok("local") => Location::Local,
        Ok("remote") => Location::Remote,
        _ => Location::Auto,
    }
}

pub(super) fn location_for_route(location: Location, vpn: bool) -> (Location, i32) {
    let (loc, size) = core_preflight::location_for_route(to_route_location(location), vpn);
    (from_route_location(loc), size)
}

pub(super) fn jumbo_preflight(
    location: Location,
    address: Option<std::net::IpAddr>,
    offered: u16,
    burst_probe: bool,
    packet_size: u32,
    jumbo_frames: bool,
    auto_reconnect: bool,
) -> JumboPreflight {
    core_preflight::jumbo_preflight(
        to_route_location(location),
        address,
        offered,
        burst_probe,
        packet_size,
        jumbo_frames,
        auto_reconnect,
    )
}

pub(super) fn jumbo_fallback_text() -> String {
    tr("Jumbo video packets didn’t pass the test on this network, so the stream uses normal packets.").to_owned()
}

pub(super) fn pyrowave_link_text(
    notice: &nightjar_policy::pyrowave::LinkNotice,
    width: u32,
    height: u32,
    fps: u32,
) -> String {
    trf(
        "PyroWave at {}×{}, {} fps asks for {} Mbps; that may be more than this link carries ({} Mbps). Lower the resolution or frame rate, or use HEVC or AV1.",
        &[&width, &height, &fps, &notice.asked_kbps.div_ceil(1000), &notice.cap_kbps.div_ceil(1000)],
    )
}

fn preflight_then_launch<T>(preflight: impl FnOnce(), launch: impl FnOnce() -> T) -> T {
    preflight();
    launch()
}

pub(super) fn session_config(
    settings: &Settings,
    role: AppRole,
    stream: StreamConfig,
    location: Location,
    watching: bool,
) -> SessionConfig {
    SessionConfig {
        stream,
        location,
        network_grace: Duration::from_millis(u64::from(settings.network_grace_ms)),
        input_only: role == AppRole::InputOnly,
        remote_monitor: role == AppRole::RemoteMonitor,
        unbatched_input: settings.unbatched_input,
        network_priority: settings.network_priority,
        receive_decode_priority: true,
        receive_coalescing: settings.receive_coalescing,
        decode_queue_bound: settings.decode_queue_frames as usize,
        roaming: roaming_config(settings, watching),
        frame_warp: None,
    }
}

pub(super) fn frame_warp_for(
    settings: &Settings,
    role: AppRole,
    capabilities: &nightjar_core::http::HostCapabilities,
) -> Option<Arc<nightjar_core::frame_warp::FrameWarp>> {
    (settings.frame_warp && role == AppRole::Stream && capabilities.has(Feature::FrameWarp))
        .then(|| nightjar_core::frame_warp::FrameWarp::new(Some(0), super::frame_warp::MASK_EDGE))
}

pub(super) fn roaming_config(settings: &Settings, watching: bool) -> RoamingConfig {
    RoamingConfig {
        enabled: settings.client_roaming && (watching || settings.roaming_metered),
        metered: settings.roaming_metered,
        platform_hints: watching,
    }
}

pub(super) const NETWORK_LOST: &str = en("Network lost.");
pub(super) const NETWORK_LOST_RECONNECTING: &str = en("Network lost. Reconnecting as soon as it is back…");

pub(super) fn roam_watch(
    settings: &Settings,
    address: &str,
    shared: &Arc<Shared>,
) -> Option<crate::network::RoamWatch> {
    let unwatched = if !settings.client_roaming || settings.roaming_metered {
        ""
    } else {
        " and roaming is off"
    };
    let Some(remote) = (address, 47989).to_socket_addrs().ok().and_then(|mut found| found.next()) else {
        eprintln!("nightjar: roaming: {address} did not resolve, network changes go unwatched{unwatched}");
        return None;
    };
    let shared = Arc::downgrade(shared);
    let on_change = move |event| {
        let notice = match event {
            crate::network::LinkEvent::Lost => Notice::NetworkLost,
            crate::network::LinkEvent::Changed { metered, after_loss } => {
                Notice::NetworkChanged(NetworkHint { metered }, after_loss)
            }
        };
        shared.upgrade().map(|s| s.notify(notice)).is_some()
    };
    crate::network::RoamWatch::start(remote, on_change)
        .map_err(|e| eprintln!("nightjar: roaming: network changes go unwatched{unwatched}: {e}"))
        .ok()
}

pub(super) fn keep_roam_watch(shared: &Shared, watch: Option<crate::network::RoamWatch>) {
    *lock(&shared.roam_watch) = watch;
}

pub(super) fn rejoin(mut plan: LaunchPlan, role: AppRole) -> Option<LaunchPlan> {
    plan.confirm_replace = false;
    plan.join = false;
    plan.resume = role == AppRole::Stream;
    if plan.resume && plan.app.control != nightjar_core::http::AppControl::None {
        if plan.info.current_game == 0 {
            return None;
        }
        plan.app.id = plan.info.current_game;
        plan.app.uuid = plan.info.current_game_uuid.clone().unwrap_or_default();
        plan.app.control = nightjar_core::http::AppControl::None;
    }
    Some(plan)
}

pub(super) fn reconnects_automatically(auto_reconnect: bool, away: bool, role: AppRole) -> bool {
    core_lifecycle::reconnects_automatically(auto_reconnect, away, role)
}

pub(crate) fn automatic_rejoin(
    plan: &LaunchPlan,
    role: AppRole,
    auto_reconnect: bool,
    away: bool,
    connected_ms: u64,
    retry_now: bool,
    had_video: bool,
) -> Option<LaunchPlan> {
    let on_its_own = if plan.mic_only {
        auto_reconnect
    } else {
        reconnects_automatically(auto_reconnect, away, role)
    };
    if !on_its_own {
        return None;
    }
    let mut plan = rejoin(plan.clone(), role)?;
    plan.video_shown |= had_video;
    let recovery = plan.recovery.get_or_insert_with(Default::default);
    recovery.reset_after(connected_ms);
    if retry_now {
        recovery.retry_now();
    }
    Some(plan)
}

pub(super) fn route_line(
    toward: Result<crate::network::Toward, crate::network::Fallback>,
    wireless: bool,
    link_mbps: u32,
    adapter_mbps: u32,
) -> String {
    let medium = match (toward, wireless) {
        (_, true) => "wireless",
        (Ok(_), false) => "wired",
        (Err(_), false) => "wired or unknown",
    };
    let source = match toward {
        Ok(_) => "from the route".to_string(),
        Err(why) => format!("from the internet connection, {why}"),
    };
    let link = match (wireless, link_mbps, adapter_mbps) {
        (true, _, 0) => "adapter rate unknown, no link speed used on a wireless route".to_string(),
        (true, _, rate) => {
            format!("adapter rate {rate} Mbps, not used as the link speed on a wireless route")
        }
        (false, 0, _) => "link speed unknown".to_string(),
        (false, link, _) => format!("link {link} Mbps"),
    };
    format!("{medium} ({source}), {link}")
}

pub(super) fn pyrowave_start_line(
    bitrate_kbps: u32,
    width: u32,
    height: u32,
    fps: u32,
    link_mbps: u32,
) -> String {
    let link = if link_mbps == 0 {
        "no link speed to stay within".to_string()
    } else {
        format!("a {link_mbps} Mbps link")
    };
    format!("PyroWave: starting at {bitrate_kbps} kbps, its default for {width}x{height} at {fps} fps and {link}")
}

pub(super) fn lost_notice(settings: &Settings, on_its_own: bool) -> &'static str {
    if core_lifecycle::network_lost_promises_reconnect(on_its_own, settings.network_grace_ms) {
        NETWORK_LOST_RECONNECTING
    } else {
        NETWORK_LOST
    }
}

pub(super) fn reconnects_on_return(
    change: NetworkChange,
    metered: bool,
    settings: &Settings,
    on_its_own: bool,
) -> bool {
    core_lifecycle::reconnects_on_network_return(
        change,
        metered,
        settings.roaming_metered,
        settings.auto_reconnect,
        settings.network_grace_ms,
        on_its_own,
    )
}

pub(super) fn asks_replace(
    plan: &LaunchPlan,
    role: AppRole,
    error: &nightjar_core::http::Error,
) -> bool {
    core_lifecycle::asks_replace(plan.resume, plan.confirm_replace, role, error)
}

pub fn plays_714() -> bool {
    #[cfg(windows)]
    return wasapi::plays_714();
    #[cfg(not(windows))]
    false
}

pub fn offers_714() -> bool {
    #[cfg(windows)]
    return wasapi::offered();
    #[cfg(not(windows))]
    false
}

pub(super) fn audio_configuration(
    choice: AudioChannels,
    host_714: bool,
    output_714: bool,
) -> AudioConfiguration {
    let req = match choice {
        AudioChannels::Stereo => core_lifecycle::RequestedAudioChannels::Stereo,
        AudioChannels::Surround51 => core_lifecycle::RequestedAudioChannels::Surround51,
        AudioChannels::Surround71 => core_lifecycle::RequestedAudioChannels::Surround71,
        AudioChannels::Surround714 => core_lifecycle::RequestedAudioChannels::Surround714,
    };
    core_lifecycle::audio_configuration(req, host_714, output_714)
}

pub use super::host_client_for;


pub(super) fn not_paired_text() -> String {
    tr("This PC is not paired.").into()
}

pub(super) fn input_only_small_refused_text(why: &str) -> String {
    format!("{why} {}", tr("Input-only on this PC uses a small ordinary stream, and the PC refused it. Some PCs allow only one stream at a time: end the other stream or allow more in the PC’s settings."))
}

pub(super) fn connect(
    plan: &mut LaunchPlan,
    shared: Arc<Shared>,
    interrupt: &Interrupt,
) -> Result<(Session, Facts), ConnectError> {
    run::connect(plan, shared, interrupt)
}

fn recent_game(plan: &LaunchPlan) -> Option<config::Recent> {
    run::recent_game(plan)
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod replace_tests;
#[cfg(test)]
mod attempts_tests;
