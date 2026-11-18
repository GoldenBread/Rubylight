#[test]
fn a_notice_joins_the_one_still_showing_once_and_replaces_one_that_went() {
    use super::{joined_toast, TOAST_SHOWN};
    use std::time::Duration;
    let planes = "Video planes are unavailable";
    let start = "Upscaling is off for this stream";
    assert_eq!(joined_toast(Some((start, Duration::from_millis(300))), planes), format!("{start}\n{planes}"));
    assert_eq!(joined_toast(Some((start, TOAST_SHOWN + Duration::from_millis(1))), planes), planes, "a notice that went is not kept");
    assert_eq!(joined_toast(None, planes), planes);
    let joined = format!("{start}\n{planes}");
    assert_eq!(joined_toast(Some((&joined, Duration::ZERO)), planes), joined, "the same notice again is not repeated");
}
use super::*;

#[test]
fn the_decode_time_after_streaming_reads_as_androids_toast() {
    assert_eq!(decode_time_text(0, 0, "HEVC"), None, "nothing decoded, nothing said");
    assert_eq!(decode_time_text(2_340 * 60, 60, "HEVC").as_deref(), Some("Average decode time: 2.3 ms [HEVC]"));
    assert_eq!(decode_time_text(12_600 * 10, 10, "AV1 10-bit").as_deref(), Some("Average decode time: 13 ms [AV1 10-bit]"));
    crate::strings::set(crate::strings::Lang::De);
    assert_eq!(decode_time_text(2_340 * 60, 60, "H.264").as_deref(), Some("Mittlere Dekodierzeit: 2,3 ms [H.264]"));
    crate::strings::set(crate::strings::Lang::En);
}

#[test]
fn the_latency_csv_has_the_statistics_header_and_rows() {
    let rows: VecDeque<String> = ["1000,60.000,60.000,60.000,0.000,2.000,1.500,0.200,3.000,9.000,20000\n".to_owned()].into();
    let csv = statistics_csv(&rows);
    let mut lines = csv.lines();
    assert_eq!(lines.next(), Some("elapsed_ms,received_fps,decoded_fps,shown_fps,loss_percent,rtt_ms,decode_ms,render_ms,host_to_network_ms,latency_ms,bitrate_kbps"));
    assert_eq!(lines.next(), Some("1000,60.000,60.000,60.000,0.000,2.000,1.500,0.200,3.000,9.000,20000"));
    assert_eq!(lines.next(), None);
    assert_eq!(LATENCY_CSV, "nightjar-latency.csv");
}

#[test]
fn an_overlay_change_names_only_what_the_user_changed() {
    // A PC's own Debug overlay in the top right, toggled off: the global mode and corner stay.
    let kept = (true, OverlayMode::Debug, OverlayCorner::TopRight);
    assert_eq!(overlay_change(kept, kept), None);
    assert_eq!(overlay_change(kept, (false, OverlayMode::Debug, OverlayCorner::TopRight)), Some((Some(false), None, None)));
    assert_eq!(overlay_change(kept, (true, OverlayMode::Statistics, OverlayCorner::TopLeft)),
        Some((None, Some(OverlayMode::Statistics), Some(OverlayCorner::TopLeft))));
}

/// HDR output gone mid-stream never ends it: the drawing goes to SDR once the loss has held for
/// a second, and back to HDR once HDR has held again; a flicker in between changes nothing.
#[test]
fn an_hdr_streams_output_change_is_followed_once_it_held_for_a_second() {
    let start = Instant::now();
    let at = |ms| start + Duration::from_millis(ms);
    let mut output = HdrOutput::new(true);
    assert_eq!(output.observe(true, at(0)), None);
    // A mode switch: HDR gone for half a second, then back.
    assert_eq!(output.observe(false, at(100)), None);
    assert_eq!(output.observe(false, at(600)), None);
    assert_eq!(output.observe(true, at(700)), None, "a flicker is not followed");
    assert_eq!(output.observe(false, at(1_500)), None, "the held time starts over");
    assert_eq!(output.observe(false, at(2_400)), None);
    assert_eq!(output.observe(false, at(2_500)), Some(false), "the window went to an SDR display");
    assert_eq!(output.observe(false, at(4_000)), None, "said once");
    assert_eq!(output.observe(true, at(4_100)), None);
    assert_eq!(output.observe(true, at(5_100)), Some(true), "and came back");
    assert_eq!(output.observe(true, at(9_000)), None);
}

#[test]
fn the_window_target_line_counts_direct_frames_against_presents() {
    assert_eq!(window_target_line(98, 100), "window target: 98 of 100 presents drawn straight into the swapchain, 2 through the back buffer");
    assert_eq!(window_target_line(0, 5), "window target: 0 of 5 presents drawn straight into the swapchain, 5 through the back buffer", "PQ output");
    assert_eq!(window_target_line(-3, 5), "window target: 0 of 5 presents drawn straight into the swapchain, 5 through the back buffer", "another renderer");
    assert_eq!(window_target_line(9, 5), "window target: 5 of 5 presents drawn straight into the swapchain, 0 through the back buffer", "launcher frames between the counts");
}

#[test]
fn the_window_target_line_counts_only_on_the_gpu_renderer() {
    assert_eq!(window_target_report("gpu", 98, 100), window_target_line(98, 100));
    assert_eq!(window_target_report("gpu", 0, 5), window_target_line(0, 5), "PQ output on the GPU renderer is a real back-buffer pass");
    assert_eq!(window_target_report("opengl", 0, 5), "window target: not counted on the opengl renderer, which draws through its own back buffer (5 presents)",
        "OpenGL has no swapchain image to draw into: no 0 of 5");
}

#[test]
fn the_window_target_count_survives_a_renderer_made_again_mid_stream() {
    let mut count = WindowTargetCount::default();
    // The launcher drew 40 frames on the first renderer before the stream.
    for frames in 40..43 {
        count.before_present(0x1000, frames);
        count.after_present(frames + 1);
    }
    assert_eq!((count.direct(), count.presents), (3, 3));
    // Made again for PQ: its count starts at zero, and PQ frames go through the back buffer.
    for _ in 0..2 {
        count.before_present(0x2000, 0);
        count.after_present(0);
    }
    assert_eq!((count.direct(), count.presents), (3, 5));
    // scRGB: straight into the swapchain again.
    for frames in 0..2 {
        count.before_present(0x3000, frames);
        count.after_present(frames + 1);
    }
    assert_eq!((count.direct(), count.presents), (5, 7));
    // Made again at the old address: a count below the last one is another renderer.
    count.before_present(0x3000, 0);
    count.after_present(1);
    assert_eq!((count.direct(), count.presents), (6, 8));
    assert_eq!(window_target_line(count.direct(), count.presents), "window target: 6 of 8 presents drawn straight into the swapchain, 2 through the back buffer");
}

#[test]
fn the_swapchain_metadata_changes_only_when_it_differs_and_goes_unknown_after_hdr() {
    assert_eq!(swapchain_metadata_change::<u8>(None, None), None, "an SDR stream sends nothing");
    assert_eq!(swapchain_metadata_change(Some(Some(1)), None), Some(Some(1)));
    assert_eq!(swapchain_metadata_change(Some(Some(1)), Some(Some(1))), None, "once");
    assert_eq!(swapchain_metadata_change(Some(Some(2)), Some(Some(1))), Some(Some(2)), "a new peak");
    assert_eq!(swapchain_metadata_change(None, Some(Some(2))), Some(None), "HDR stopped: unknown");
    assert_eq!(swapchain_metadata_change::<u8>(None, Some(None)), None, "unknown once");
    assert_eq!(swapchain_metadata_change(Some(None), Some(Some(2))), Some(None), "zeroed metadata from the host is unknown");
}
use super::input::*;

#[test]
fn a_shortcut_that_cannot_be_sent_says_why() {
    let keys = |list: &[&str]| list.iter().map(|k| k.to_string()).collect::<Vec<_>>();
    assert_eq!(shortcut_refusal(true, false, &keys(&["ALT", "TAB"])), None);
    assert_eq!(shortcut_refusal(false, true, &keys(&["ALT", "TAB"])),
        Some("Shortcut was not sent: the stream isn’t taking input right now."));
    assert!(shortcut_refusal(true, false, &keys(&["WIN", "D"])).unwrap().contains("Send system keys"));
    assert_eq!(shortcut_refusal(true, true, &keys(&["WIN", "D"])), None);
}

/// Desktop never asked for an HDR format, so no host sent it Dolby Vision RPUs and the draw
/// pass's RPU peak was unreachable. It asks as Android does, and stock hosts see nothing new.
#[test]
fn hdr_streams_ask_a_rubylight_host_for_dolby_vision_where_the_rpus_are_read() {
    use rubylight_protocol::hdr::{ALL, DV_101, DV_81, DV_84, HLG, PQ};
    let both = VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10 | VIDEO_FORMAT_AV1_MAIN8 | VIDEO_FORMAT_AV1_MAIN10;
    let hevc = VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10;
    let host = i32::from(ALL);
    // AV1 first, as the handshake takes it, then HEVC; each only where the host offers it.
    assert_eq!(hdr_format(true, host, both, false, true), DV_101);
    assert_eq!(hdr_format(true, host, hevc, false, true), DV_81);
    assert_eq!(hdr_format(true, i32::from(PQ | HLG | DV_81), both, false, true), DV_81);
    // Never profile 8.4 (an HLG base layer) nor HLG: every desktop HDR output takes PQ.
    assert_eq!(hdr_format(true, i32::from(PQ | HLG | DV_84), both, false, true), 0);
    // Stock HDR10 for a stock host (no HdrFormats: -1), an SDR stream, packed full chroma,
    // 8-bit formats only, and where nothing reads the RPUs (Linux, SDL drawing on Windows).
    assert_eq!(hdr_format(true, -1, both, false, true), 0);
    assert_eq!(hdr_format(true, 0, both, false, true), 0);
    assert_eq!(hdr_format(false, host, both, false, true), 0);
    assert_eq!(hdr_format(true, host, both, true, true), 0);
    assert_eq!(hdr_format(true, host, VIDEO_FORMAT_H265 | VIDEO_FORMAT_AV1_MAIN8, false, true), 0);
    assert_eq!(hdr_format(true, host, both, false, false), 0);
    #[cfg(not(windows))]
    assert!(!reads_dolby_vision(), "only the Windows draw pass reads them");
}

#[test]
fn the_doctor_offers_no_trial_while_the_client_hint_steers_the_bitrate() {
    let trial = nightjar_policy::health::BitrateTrial::default();
    let offer = |automatic| trial.offer(true, 20_000, true, automatic, false);
    let hint = rate_change::RateChange::new(20_000, 60_000, true, 0, 0);
    let manual = rate_change::RateChange::new(20_000, 60_000, false, 0, 0);
    assert_eq!(offer(bitrate_automatic(false, QualityMode::Off, Some(&hint))), TrialOffer::None);
    assert_eq!(offer(bitrate_automatic(false, QualityMode::Off, Some(&manual))), TrialOffer::Try);
    assert_eq!(offer(bitrate_automatic(false, QualityMode::Off, None)), TrialOffer::Try);
    assert_eq!(offer(bitrate_automatic(true, QualityMode::Off, None)), TrialOffer::None);
    assert_eq!(offer(bitrate_automatic(false, QualityMode::Motion, None)), TrialOffer::None);
}

#[test]
fn each_image_clipboard_refusal_says_its_own_reason() {
    use nightjar_core::http::{Error, ImageClipboardError as E};
    let texts = [
        image_error(E::TooLarge("20 MB".into()), true),
        image_error(E::NotPng("bad header".into()), true),
        image_error(E::NotPng("bad header".into()), false),
        image_error(E::Unsupported, true),
        image_error(E::Unsupported, false),
        image_error(E::Forbidden, false),
        image_error(E::Host(Error::Cancelled), false),
    ];
    assert!(texts[0].contains("too large") && texts[0].contains("20 MB"));
    assert!(texts[1].contains("this device") && texts[1].contains("bad header"));
    assert!(texts[2].contains("The PC sent"));
    assert!(texts[6].contains(&Error::Cancelled.to_string()));
    let distinct: HashSet<&String> = texts.iter().collect();
    assert_eq!(distinct.len(), texts.len(), "{texts:#?}");
}

#[test]
fn the_shortcut_chord_on_the_session_page_leaves_it_for_the_sheet() {
    assert_eq!(sheet_chord(false, None, None), SheetChord::Open(Sheet::FromChord));
    assert_eq!(sheet_chord(true, None, None), SheetChord::Open(Sheet::FromMenu));
    assert_eq!(sheet_chord(true, Some(Sheet::FromMenu), None), SheetChord::Close);
    assert_eq!(sheet_chord(true, Some(Sheet::FromChord), None), SheetChord::Close);
    // The sheet draws only without the Session page: the chord there shows it over the
    // controls, so Escape from it goes back to them.
    assert_eq!(sheet_chord(true, None, Some(false)), SheetChord::Open(Sheet::FromMenu));
    assert_eq!(sheet_chord(true, None, Some(true)), SheetChord::Stay, "a running End or Close game keeps its page");
}

#[test]
fn a_stream_behind_another_window_leaves_the_pointer_and_wheel_alone() {
    // capture_on_focus: captured before it comes to the front.
    assert!(!pointer_goes_out(true, false), "a pointer over the background window stays the local PC's");
    assert!(pointer_goes_out(true, true));
    assert!(!pointer_goes_out(false, true), "released");
}

#[test]
fn ctrl_alt_backspace_goes_to_the_pc_and_only_ctrl_alt_shift_stays_local() {
    // Ctrl+Alt+Shift is Nightjar's own chord; every other one goes to the PC like any key.
    let local_chord = |mods| crate::shortcuts::Chord::nightjar(crate::shortcuts::Key::Tab).held(mods);
    let ctrl_alt = SDL_Keymod(SDL_KMOD_LCTRL.0 | SDL_KMOD_LALT.0);
    assert!(!local_chord(ctrl_alt), "a host's menu chord is not Nightjar's");
    for (system_keys, right_alt_as_meta) in [(false, false), (true, false), (false, true), (true, true)] {
        let sent = key_input(SDL_SCANCODE_BACKSPACE, ctrl_alt, system_keys, right_alt_as_meta).unwrap();
        assert_eq!((sent.code, sent.modifiers), (0x08, modifier::CTRL | modifier::ALT));
        // The modifier keys themselves go to the PC too.
        assert!(key_input(SDL_SCANCODE_LCTRL, ctrl_alt, system_keys, right_alt_as_meta).is_some());
        assert!(key_input(SDL_SCANCODE_LALT, ctrl_alt, system_keys, right_alt_as_meta).is_some());
    }
    assert!(local_chord(SDL_Keymod(ctrl_alt.0 | SDL_KMOD_LSHIFT.0)));
}

#[test]
fn the_wireless_hint_goes_only_to_a_rubylight_host_on_a_wireless_route_when_the_setting_is_on() {
    use nightjar_core::http::Family;
    assert!(tells_host_wireless(true, Family::Rubylight, true));
    assert!(!tells_host_wireless(false, Family::Rubylight, true));
    assert!(!tells_host_wireless(true, Family::Rubylight, false));
    for other in [Family::GameStream, Family::Sunshine, Family::Apollo, Family::Vibepollo, Family::Unknown] {
        assert!(!tells_host_wireless(true, other, true), "{other:?}");
    }
}

#[test]
fn the_link_speed_hint_is_left_out_when_the_setting_is_off_but_beam_race_and_pace_keep_the_real_speed() {
    // On: the host is told the routed speed.
    let on = link_hints(true, 2500, None, 2100, None);
    assert_eq!(on, LinkHints { link_mbps: 2500, sent_link_mbps: 2500, pace_mbps: 2100 });
    // Off: nothing is sent, the beam race still sees a wired link and the pace stays.
    let off = link_hints(false, 2500, None, 2100, None);
    assert_eq!(off, LinkHints { link_mbps: 2500, sent_link_mbps: 0, pace_mbps: 2100 });
    assert_eq!(beam_race_for(None, true, off.link_mbps, true), BeamRace::On);
    // An unknown speed is never sent, on or off.
    assert_eq!(link_hints(true, 0, None, 0, None).sent_link_mbps, 0);
    // A variable naming the speed wins over the setting, and the pace variable over the measurement.
    let forced = link_hints(false, 2500, Some(1000), 2100, Some(900));
    assert_eq!(forced, LinkHints { link_mbps: 1000, sent_link_mbps: 1000, pace_mbps: 900 });
}

#[test]
fn only_a_remote_monitor_session_is_view_only() {
    let settings = Settings::default();
    let config = |role| session_config(&settings, role, StreamConfig::default(), Location::Auto, true);
    let monitor = config(AppRole::RemoteMonitor);
    assert!(monitor.remote_monitor && !monitor.input_only);
    let stream = config(AppRole::Stream);
    assert!(!stream.remote_monitor && !stream.input_only);
    let input = config(AppRole::InputOnly);
    assert!(!input.remote_monitor && input.input_only);
}

#[test]
fn a_lost_or_reset_render_device_ends_the_stream_saying_why() {
    for kind in [SDL_EVENT_RENDER_DEVICE_RESET, SDL_EVENT_RENDER_DEVICE_LOST] {
        let reason = render_device_ended(kind).expect("ends the stream");
        assert!(!Exit::Ended(Some(reason.into())).retryable(true), "{reason}");
    }
    assert_eq!(render_device_ended(SDL_EVENT_RENDER_TARGETS_RESET), None);
}

#[test]
fn controllers_reach_the_host_only_when_on_and_not_from_a_remote_monitor() {
    let on = Settings { controllers: true, ..Settings::default() };
    let off = Settings { controllers: false, ..Settings::default() };
    assert!(sends_controllers(&on, AppRole::Stream));
    assert!(sends_controllers(&on, AppRole::InputOnly));
    assert!(!sends_controllers(&on, AppRole::RemoteMonitor));
    assert!(!sends_controllers(&off, AppRole::Stream));
}

/// A reconnect keeps the bitrate the stream settled on: an automatic cut is where automatic
/// bitrate starts again on an automatic reconnect, below the same ceiling; a bitrate the player
/// set is the plan's own and no cut is carried with it.
#[test]
fn a_reconnect_keeps_the_bitrate_the_stream_settled_on() {
    assert_eq!(reconnect_bitrate_start(true, true, false, 12_000), Some(12_000));
    assert_eq!(reconnect_bitrate_start(false, true, false, 12_000), None, "a Reconnect the player pressed");
    assert_eq!(reconnect_bitrate_start(true, false, false, 12_000), None, "automatic bitrate did not own the rate");
    assert_eq!(reconnect_bitrate_start(true, true, true, 12_000), None, "the player's bitrate is in the settings");
    assert_eq!(reconnect_bitrate_start(true, true, false, 0), None);
    assert_eq!(automatic_bitrate_start(30_000, Some(12_000)), 12_000);
    assert_eq!(automatic_bitrate_start(20_000, Some(30_000)), 20_000, "never above the encoder's rate");
    assert_eq!(automatic_bitrate_start(30_000, None), 30_000);
    assert_eq!(automatic_bitrate_start(30_000, Some(0)), 30_000);
}

#[test]
fn frames_waiting_for_the_decoder_reach_the_engine() {
    let config = |decode_queue_frames| {
        let settings = Settings { decode_queue_frames, ..Default::default() };
        session_config(&settings, AppRole::Stream, StreamConfig::default(), Location::Auto, true).decode_queue_bound
    };
    assert_eq!(config(15), 15);
    assert_eq!(config(5), 5);
}

#[test]
fn the_input_and_priority_settings_reach_the_engine() {
    let on = session_config(&Settings::default(), AppRole::Stream, StreamConfig::default(), Location::Local, true);
    assert!(on.unbatched_input && on.network_priority);
    // The desktop keeps the decode priority on (NIGHTJAR_DECODE_PRIORITY=0 for A/B runs).
    assert!(on.receive_decode_priority);
    assert_eq!((on.location, on.network_grace), (Location::Local, Duration::from_millis(3000)));
    for (unbatched_input, network_priority) in [(false, true), (true, false), (false, false)] {
        let settings = Settings { unbatched_input, network_priority, network_grace_ms: 0, ..Default::default() };
        let config = session_config(&settings, AppRole::Stream, StreamConfig::default(), Location::Auto, true);
        assert_eq!((config.unbatched_input, config.network_priority), (unbatched_input, network_priority));
        assert_eq!(config.network_grace, Duration::ZERO);
    }
}

#[test]
fn roaming_follows_the_settings_and_needs_mobile_data_without_the_watcher() {
    let on = session_config(&Settings::default(), AppRole::Stream, StreamConfig::default(), Location::Auto, true).roaming;
    assert_eq!(on, RoamingConfig { enabled: true, metered: false, platform_hints: true });
    let metered = Settings { roaming_metered: true, ..Default::default() };
    assert_eq!(roaming_config(&metered, true), RoamingConfig { enabled: true, metered: true, platform_hints: true });
    // Without the watcher a metered network looks like any other.
    assert!(!roaming_config(&Settings::default(), false).enabled);
    assert_eq!(roaming_config(&metered, false), RoamingConfig { enabled: true, metered: true, platform_hints: false });
    let off = Settings { client_roaming: false, roaming_metered: true, ..Default::default() };
    assert!(!roaming_config(&off, true).enabled && !roaming_config(&off, false).enabled);
}

#[test]
fn every_tearing_vrr_stream_checks_its_display_not_only_the_beam_race() {
    let checks = |settings: &Settings, tearing: bool, forced: Option<bool>| {
        let (pacing, vrr) = present::pacing(settings, tearing);
        checks_display_for_vrr(vrr, pacing, present::without_vrr(settings), forced)
    };
    // The default settings: VRR on, the swap chain tears, Real VRR.
    assert!(checks(&Settings::default(), true, None), "a fixed refresh display would tear on every present unchecked");
    let host_times = Settings { present_at_host_times: true, ..Default::default() };
    assert!(checks(&host_times, true, None));
    assert!(checks(&Settings { vsync: false, ..host_times.clone() }, true, None), "host times turn immediate");
    // Nothing to check: V-Sync already waits, VRR is off, the chain cannot tear, or V-Sync is
    // off in the settings and the stream is immediate already.
    assert!(!checks(&Settings::default(), false, None));
    let off = Settings { vrr: false, vsync: false, ..Default::default() };
    assert_eq!(present::pacing(&off, true).0, PacingMode::Immediate);
    assert!(!checks(&off, true, None), "tearing asked for without VRR is the user's choice");
    let shown_when_decoded = Settings { vsync: false, present_at_host_times: false, ..Default::default() };
    assert!(!checks(&shown_when_decoded, true, None), "V-Sync and Real VRR off: immediate either way");
    assert!(!checks(&Settings::default(), true, Some(false)), "NIGHTJAR_VRR_CHECK=0");
    assert!(checks(&Settings::default(), true, Some(true)));
}

/// The bug: with V-Sync off a VRR stream on Wayland stayed on MAILBOX on a fixed refresh
/// display, unchecked. The check now runs for its present mode.
#[test]
fn a_vrr_stream_without_v_sync_checks_its_display_for_the_present_mode() {
    // Real VRR off: frames shown when decoded, which needs no pacing check without V-Sync.
    let settings = Settings { vsync: false, present_at_host_times: false, ..Default::default() };
    let (pacing, vrr) = present::pacing(&settings, true);
    let pacing_check = checks_display_for_vrr(vrr, pacing, present::without_vrr(&settings), None);
    assert!(!pacing_check);
    assert!(checks_display_for_present_mode(vrr, pacing, pacing_check, true, None), "Wayland keeps MAILBOX for VRR");
    assert!(!checks_display_for_present_mode(vrr, pacing, pacing_check, false, None), "IMMEDIATE already (X11, gamescope)");
    assert!(!checks_display_for_present_mode(vrr, pacing, pacing_check, true, Some(false)), "NIGHTJAR_VRR_CHECK=0");
    assert!(!checks_display_for_present_mode(vrr, pacing, true, true, None), "the pacing check reads it already");
    assert!(!checks_display_for_present_mode(false, pacing, false, true, None), "no VRR, no MAILBOX");
    assert!(!checks_display_for_present_mode(vrr, PacingMode::Vsync, false, true, None), "V-Sync is FIFO");
    let vrr_pacing = present::VrrPacing::new(false, PacingMode::Immediate, present::VrrSync::Off, PacingMode::Immediate).with_present_check(true);
    assert_eq!(display_probe(false, &vrr_pacing), Some("display check"));
}

#[test]
fn the_scan_line_is_read_for_the_beam_race_the_display_check_and_vrr_sync_scan() {
    use present::{VrrPacing, VrrSync};
    let vrr = |check, sync| VrrPacing::new(check, PacingMode::Vsync, sync, PacingMode::Immediate);
    assert_eq!(display_probe(false, &vrr(true, VrrSync::Off)), Some("display check"), "every VRR stream that tears");
    assert_eq!(display_probe(true, &vrr(true, VrrSync::Off)), Some("beam race"));
    assert_eq!(display_probe(true, &vrr(false, VrrSync::Off)), Some("beam race"));
    // NIGHTJAR_VRR_SYNC=scan waits on the beam, so it reads the scan line with the check off.
    assert_eq!(display_probe(false, &vrr(false, VrrSync::Scan)), Some("display check"));
    assert_eq!(display_probe(false, &vrr(false, VrrSync::Blank)), Some("display check"), "blank reads the scan line too");
    assert_eq!(display_probe(false, &vrr(false, VrrSync::Present1)), None, "present1 reads no beam");
    assert_eq!(display_probe(false, &vrr(false, VrrSync::Off)), None, "a default stream without VRR does none of it");
}

#[test]
fn the_pacing_line_says_why_a_vrr_stream_waits_for_the_refresh() {
    assert_eq!(pacing_label(PacingMode::Vsync, true), "V-Sync: this display does not refresh on present");
    assert_eq!(pacing_label(PacingMode::Vsync, false), "V-Sync", "V-Sync the settings asked for");
    assert_eq!(pacing_label(PacingMode::Immediate, true), "Immediate: this display does not refresh on present", "V-Sync off in the settings");
    assert_eq!(pacing_label(PacingMode::Immediate, false), "Immediate");
}

#[test]
fn the_network_watcher_runs_for_the_stream_and_stops_with_it() {
    use std::sync::mpsc::RecvTimeoutError;
    let shared = Shared::new(Arc::new(|| {}), 8);
    // Kept whether the host roams or not: a lost network's return reconnects every stream.
    let (watch, ended) = crate::network::RoamWatch::idle();
    keep_roam_watch(&shared, Some(watch));
    assert!(lock(&shared.roam_watch).is_some());
    assert_eq!(ended.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty));
    lock(&shared.roam_watch).take();
    assert_eq!(ended.recv_timeout(Duration::from_secs(5)), Err(RecvTimeoutError::Disconnected));
}

#[test]
fn early_decode_without_records_is_reported_not_silent() {
    assert_eq!(early_state(false, true, true), EarlyState::Off);
    assert_eq!(early_state(true, false, false), EarlyState::Off);
    assert_eq!(early_state(true, true, true), EarlyState::On);
    // The 0-frames-decoded-ahead runs: asked for, PyroWave, but the host never offered records.
    assert_eq!(early_state(true, true, false), EarlyState::NoRecords);
}

#[test]
fn beam_race_line_names_a_host_that_sends_no_row_order() {
    assert_eq!(race_line(100, 300, 0), "100 frames in 300 slices, 0 not in row order");
    assert!(race_line(0, 0, 7).contains("not sending the row order"));
    assert!(!race_line(90, 270, 10).contains("not sending"));
}

#[test]
fn beam_race_needs_the_setting_and_a_wired_link() {
    // The environment override is for measurement runs; these cases are the settings.
    if std::env::var_os("NIGHTJAR_BEAM_RACE").is_some() { return; }
    assert_eq!(beam_race(false, 1000, true), BeamRace::Off);
    assert_eq!(beam_race(true, 1000, true), BeamRace::On);
    assert_eq!(beam_race(true, 0, true), BeamRace::NotWired);
}

#[test]
fn beam_race_needs_the_gpu_decoder_and_says_why_it_is_off() {
    // The forced value is the measurement override; the GPU decoder is needed either way.
    assert_eq!(beam_race_for(None, true, 1000, false), BeamRace::NoGpuDecoder);
    assert_eq!(beam_race_for(Some(true), false, 0, false), BeamRace::NoGpuDecoder);
    assert_eq!(beam_race_for(Some(true), false, 0, true), BeamRace::On);
    assert_eq!(beam_race_for(Some(false), true, 1000, true), BeamRace::Off);
    assert_eq!(beam_race_for(None, false, 1000, false), BeamRace::Off, "the setting off says nothing");
    assert!(BeamRace::NoGpuDecoder.reason().unwrap().contains("CPU"));
    assert!(BeamRace::NotWired.reason().unwrap().contains("wired"));
    assert_eq!((BeamRace::On.reason(), BeamRace::Off.reason()), (None, None));
}

#[test]
fn the_environment_switches_take_a_few_spellings_and_refuse_the_rest() {
    for on in ["1", "true", "TRUE", "yes", "On", " 1 "] { assert_eq!(flag_value(on), Some(true), "{on:?}"); }
    for off in ["0", "false", "No", "OFF"] { assert_eq!(flag_value(off), Some(false), "{off:?}"); }
    // Not a switch: said on stderr by `env_flag` and ignored, so the setting decides.
    for junk in ["", "2", "ture", "enable", "1 0"] { assert_eq!(flag_value(junk), None, "{junk:?}"); }
    assert_eq!(slices_override("4"), Some(4));
    assert_eq!(slices_override("7"), Some(MAX_RACE_SLICES));
    for bad in ["0", "8", "16", "four", "-1", ""] { assert_eq!(slices_override(bad), None, "{bad:?}"); }
    assert_eq!(beam_race_for(None, false, 1000, true), BeamRace::Off, "a refused value leaves the setting in charge");
}

#[test]
fn the_beam_race_off_asks_for_nothing() {
    // Whatever turned it off (the setting, `NIGHTJAR_BEAM_RACE=0`, a link or decoder that cannot race):
    // no row order from the host, no slices (so no probe, no slice handoff), early decode as the user set it.
    for beam in [BeamRace::Off, BeamRace::NotWired, BeamRace::NoGpuDecoder] {
        for early in [false, true] {
            assert_eq!(race_request(beam, early, 1080), RaceRequest { row_order: false, early_decode: early, slices: 0 }, "{beam:?}");
        }
    }
    for early in [EarlyState::Off, EarlyState::On, EarlyState::NoRecords] {
        assert!(!race_runs(0, early), "no slices asked for: the presenter never probes or follows a slice ({early:?})");
    }
    assert!(race_runs(4, EarlyState::On));
    assert!(!race_runs(4, EarlyState::NoRecords), "asked for, but the host sends no records to slice");
    let on = race_request(BeamRace::On, false, 1080);
    assert!(on.row_order && on.early_decode && on.slices > 0, "on asks for all of it: {on:?}");
    // The default settings never turn it on, on any link.
    assert!(!crate::config::Settings::default().beam_race);
    assert_eq!(beam_race_for(None, false, 1000, true), BeamRace::Off);
}

#[test]
fn beam_slices_are_four_or_three_at_4k() {
    if std::env::var_os("NIGHTJAR_BEAM_RACE_SLICES").is_some() { return; }
    assert_eq!(beam_slices(1080), 4);
    assert_eq!(beam_slices(2160), 3);
}

fn display() -> Display {
    Display {
        width: 2560,
        height: 1440,
        refresh_hz: 143.9,
        refresh_reported: true,
    }
}

#[test]
fn metered_limit_caps_fixed_auto_and_pyrowave_bitrates() {
    let settings = Settings { codec: CodecPreference::PyroWave, ..Default::default() };
    let display = Display { width: 1920, height: 1080, refresh_hz: 60.0, refresh_reported: true };
    let (_, _, _, bitrate) = stream_mode(&settings, display);
    assert!(bitrate > 5_000);
    assert_eq!(capped_bitrate(bitrate, 5_000), 5_000);
    assert_eq!(capped_bitrate(4_000, 5_000), 4_000);
    assert_eq!(capped_bitrate(bitrate, 0), bitrate);
}

#[test]
fn only_a_new_stream_at_the_automatic_bitrate_tests_the_link_first() {
    let info = |paired: u8, version: &str| ServerInfo::parse(&format!("<root status_code=\"200\"><uniqueid>pc</uniqueid><state>SERVER_FREE</state><PairStatus>{paired}</PairStatus><BandwidthProbeVersion>{version}</BandwidthProbeVersion></root>"), 47989).unwrap();
    let offered = info(1, "1");
    assert!(!probes_first_bitrate(&Settings::default(), false, &offered), "the link test is off by default");
    let auto = Settings { link_probe: true, ..Default::default() };
    assert!(probes_first_bitrate(&auto, false, &offered));
    assert!(!probes_first_bitrate(&auto, true, &offered), "a resume keeps the session's bitrate");
    assert!(!probes_first_bitrate(&Settings { bitrate_kbps: 30_000, ..auto.clone() }, false, &offered));
    for codec in [CodecPreference::PyroWave, CodecPreference::PyroWave444] {
        assert!(!probes_first_bitrate(&Settings { codec, ..auto.clone() }, false, &offered));
    }
    assert!(!probes_first_bitrate(&auto, false, &info(1, "0")), "a PC that does not offer the test");
    assert!(!probes_first_bitrate(&auto, false, &info(0, "1")), "an unpaired PC");
}

#[test]
fn native_mode_follows_the_display() {
    let (w, h, fps, kbps) = stream_mode(&Settings::default(), display());
    assert_eq!((w, h, fps), (2560, 1440, 144));
    assert_eq!(kbps, config::default_bitrate_kbps(2560, 1440, 144));
    let settings = Settings {
        width: 1920,
        height: 1080,
        fps: 60,
        bitrate_kbps: 30000,
        ..Default::default()
    };
    assert_eq!(stream_mode(&settings, display()), (1920, 1080, 60, 30000));
    // PyroWave chosen: a chosen bitrate stays, below its floor too (the settings warn), and the
    // automatic one is PyroWave's default (what the settings show).
    let mut settings = Settings { codec: CodecPreference::PyroWave444, ..settings };
    assert_eq!(stream_mode(&settings, display()).3, 30000);
    settings.bitrate_kbps = 0;
    assert_eq!(stream_mode(&settings, display()).3, 398_132);
    settings.bitrate_kbps = 250_000;
    assert_eq!(stream_mode(&settings, display()).3, 250_000);
    settings.bitrate_kbps = 2_000_000;
    assert_eq!(stream_mode(&settings, display()).3, 2_000_000);
}

// User report: PyroWave started at the H.264 default (20 Mbps at 1080p60), far below its floor,
// because the automatic bitrate did not know the codec.
#[test]
fn a_pyrowave_stream_at_the_automatic_bitrate_starts_at_pyrowaves_default() {
    let auto = Settings::default();
    // With the link unknown the default stays within 900 Mbps, unless the floor is higher.
    for (w, h, fps, kbps) in [(1920, 1080, 60, 398_132), (2560, 1440, 120, 900_000), (3840, 2160, 60, 900_000), (3840, 2160, 120, 1_492_992)] {
        assert_eq!(start_bitrate(&auto, w, h, fps, true, 0), kbps, "{w}x{h}@{fps}");
        assert_eq!(start_bitrate(&auto, w, h, fps, false, 0), config::default_bitrate_kbps(w, h, fps));
        assert!(kbps >= nightjar_policy::pyrowave::minimum_kbps(w, h, fps));
        // The settings show the same for PyroWave chosen.
        let chosen = Settings { width: w, height: h, fps, codec: CodecPreference::PyroWave, ..Default::default() };
        assert_eq!(stream_mode(&chosen, display()).3, kbps);
    }
    // Chosen bitrates stay as they are, whatever the link, also below the floor (the settings warn).
    let chosen = |kbps| Settings { bitrate_kbps: kbps, ..Default::default() };
    assert_eq!(start_bitrate(&chosen(300_000), 1920, 1080, 60, true, 0), 300_000);
    assert_eq!(start_bitrate(&chosen(900_000), 3840, 2160, 60, true, 0), 900_000);
    assert_eq!(start_bitrate(&chosen(1_500_000), 3840, 2160, 60, true, 1000), 1_500_000);
    assert_eq!(start_bitrate(&chosen(20_000), 1920, 1080, 60, true, 0), 20_000);
    assert_eq!(start_bitrate(&chosen(20_000), 1920, 1080, 60, false, 0), 20_000);
}

// A default over gigabit broke a first PyroWave stream on ordinary Ethernet and Wi-Fi: the
// default stays within 90% of a known wired link, never below the floor.
#[test]
fn pyrowaves_default_stays_within_the_wired_link() {
    let auto = Settings::default();
    assert_eq!(start_bitrate(&auto, 3840, 2160, 60, true, 1000), 900_000);
    assert_eq!(start_bitrate(&auto, 3840, 2160, 60, true, 2500), 1_592_525);
    assert_eq!(start_bitrate(&auto, 2560, 1440, 120, true, 2500), 1_415_578);
    assert_eq!(start_bitrate(&auto, 1920, 1080, 60, true, 100), 186_624);
    assert_eq!(start_bitrate(&auto, 1920, 1080, 60, false, 100), config::default_bitrate_kbps(1920, 1080, 60));
    // 10G gets the full clean target at 4K60, 1G about 900 Mbps at 1440p120, unknown 900.
    assert_eq!(start_bitrate(&auto, 3840, 2160, 60, true, 10_000), 1_592_525);
    assert_eq!(start_bitrate(&auto, 2560, 1440, 120, true, 1000), 900_000);
    assert_eq!(start_bitrate(&auto, 2560, 1440, 120, true, 0), 900_000);
}

// The bitrate follows what the launch offers the PC, not the codec choice alone: a PyroWave
// choice the PC lacks streams HEVC at the conventional default, and any offer the PC can answer
// with PyroWave starts at PyroWave's (the automatic choice included, should it offer PyroWave).
#[test]
fn the_launch_starts_pyrowave_bitrates_only_when_the_offer_streams_pyrowave() {
    let with = SCM_H264 | SCM_HEVC | SCM_PYROWAVE;
    let offered = |codec, scm| offered_formats(&Settings { codec, ..Default::default() }, scm, &decodes_all()).formats;
    assert!(offers_pyrowave(offered(CodecPreference::PyroWave, with), with));
    assert!(offers_pyrowave(offered(CodecPreference::PyroWave444, SCM_H264 | SCM_PYROWAVE_444), SCM_H264 | SCM_PYROWAVE_444));
    assert!(!offers_pyrowave(offered(CodecPreference::PyroWave, SCM_H264 | SCM_HEVC), SCM_H264 | SCM_HEVC), "the PC has no PyroWave");
    assert!(!offers_pyrowave(offered(CodecPreference::PyroWave, 0), 0), "no trusted codec support: the handshake never picks PyroWave");
    assert!(!offers_pyrowave(offered(CodecPreference::Auto, with), with), "automatic offers HEVC and H.264 only");
    assert!(offers_pyrowave(VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_PYROWAVE, with), "an automatic offer that has PyroWave");
    assert_eq!(start_bitrate(&Settings::default(), 1920, 1080, 60, offers_pyrowave(VIDEO_FORMAT_H264 | VIDEO_FORMAT_PYROWAVE, with), 0), 398_132);
}

// A chosen bitrate below PyroWave's minimum is kept; the bitrate setting warns, naming the
// minimum for the mode.
#[test]
fn a_chosen_bitrate_below_pyrowaves_minimum_is_warned_in_the_settings() {
    let low = Settings { codec: CodecPreference::PyroWave, bitrate_kbps: 20_000, ..Default::default() };
    assert_eq!(pyrowave_bitrate_warning(&low, 1920, 1080, 60).as_deref(),
        Some("Below the level where the project's test scenes kept detail for PyroWave: 187 Mbps or more at 1920x1080, 60 fps. Text and textures may lose detail; raise the bitrate or use HEVC or AV1."));
    let german = crate::strings::tests::with_lang(crate::strings::Lang::De, || pyrowave_bitrate_warning(&low, 1920, 1080, 60));
    assert_eq!(german.as_deref(), Some("PyroWave liegt unter dem Niveau, auf dem die Testbilder des Projekts Details behielten: 187 Mbit/s oder mehr bei 1920x1080, 60 fps. Text und Texturen können Details verlieren; erhöhe die Bitrate oder nutze HEVC oder AV1."));
    assert_eq!(pyrowave_bitrate_warning(&Settings { bitrate_kbps: 186_624, ..low.clone() }, 1920, 1080, 60), None);
    assert_eq!(pyrowave_bitrate_warning(&Settings { bitrate_kbps: 0, ..low.clone() }, 1920, 1080, 60), None, "automatic");
    assert_eq!(pyrowave_bitrate_warning(&Settings { codec: CodecPreference::Hevc, ..low.clone() }, 1920, 1080, 60), None);
    assert!(pyrowave_bitrate_warning(&Settings { codec: CodecPreference::PyroWave444, ..low }, 1920, 1080, 60).is_some());
}

// The player hears before the first picture when a PyroWave mode asks for more than the link
// carries; the stream still starts, so the words name the numbers and the way out.
#[test]
fn a_pyrowave_link_notice_names_the_mode_the_rate_and_the_link() {
    let notice = nightjar_policy::pyrowave::link_notice(3840, 2160, 120, 0, 0)
        .expect("4K120 on an unknown (wireless) link");
    let text = connect::pyrowave_link_text(&notice, 3840, 2160, 120);
    assert_eq!(text, "PyroWave at 3840×2160, 120 fps asks for 1493 Mbps; that may be more than this link carries (900 Mbps). Lower the resolution or frame rate, or use HEVC or AV1.");
    let german = crate::strings::tests::with_lang(crate::strings::Lang::De,
        || connect::pyrowave_link_text(&notice, 3840, 2160, 120));
    assert_eq!(german, "PyroWave bei 3840×2160 mit 120 fps verlangt 1493 Mbps; das kann mehr sein, als diese Verbindung trägt (900 Mbps). Senke die Auflösung oder Bildrate oder nutze HEVC oder AV1.");
    // 1080p60 is far under the unknown link's cap, and a chosen rate above it is named too.
    assert_eq!(nightjar_policy::pyrowave::link_notice(1920, 1080, 60, 0, 0), None);
    let chosen = nightjar_policy::pyrowave::link_notice(1920, 1080, 60, 0, 1_500_000).expect("a chosen 1.5 Gbps");
    assert!(connect::pyrowave_link_text(&chosen, 1920, 1080, 60).contains("1500 Mbps"));
}

// The low-bitrate notice follows the bitrate actually sent, after this client's cap and the
// host's, not the setting: a 100 Mbps limit at 1080p60 and the automatic 4K240 default (held at
// the 2 Gbps runtime maximum) both land below the mode's level.
#[test]
fn the_pyrowave_minimum_notice_follows_the_bitrate_actually_sent() {
    let automatic = Settings::default();
    let capped = capped_bitrate(start_bitrate(&automatic, 1920, 1080, 60, true, 0), 100_000);
    assert_eq!(capped, 100_000);
    let notice = pyrowave_minimum_notice(capped, 1920, 1080, 60).expect("100 Mbps is below 187");
    assert!(notice.contains("187 Mbps or more at 1920x1080, 60 fps"), "{notice}");
    let automatic_4k = start_bitrate(&Settings::default(), 3840, 2160, 240, true, 0);
    assert_eq!(automatic_4k, 2_000_000);
    assert!(pyrowave_minimum_notice(automatic_4k, 3840, 2160, 240).is_some());
    // At the level itself there is nothing to warn about, whatever the setting says.
    assert_eq!(pyrowave_minimum_notice(186_624, 1920, 1080, 60), None);
}

// A PyroWave choice the PC answers with HEVC shows no PyroWave notice, even though the launch
// offered it: the notices follow the negotiated format, not the offer.
#[test]
fn a_fallback_to_hevc_shows_no_pyrowave_notice() {
    let (link, minimum) = pyrowave_notices_if_stream(
        VIDEO_FORMAT_H265, Some("link".into()), Some("minimum".into()));
    assert_eq!((link, minimum), (None, None));
    let (link, minimum) = pyrowave_notices_if_stream(
        VIDEO_FORMAT_PYROWAVE, Some("link".into()), Some("minimum".into()));
    assert_eq!((link.as_deref(), minimum.as_deref()), (Some("link"), Some("minimum")));
}

#[test]
fn formats_respect_the_host() {
    let all = SCM_H264 | SCM_HEVC | SCM_HEVC_MAIN10 | SCM_AV1_MAIN8 | SCM_AV1_MAIN10;
    assert_eq!(
        video_formats(CodecPreference::Auto, false, all),
        VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265
    );
    assert_eq!(
        video_formats(CodecPreference::Av1, true, all),
        VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10 | VIDEO_FORMAT_AV1_MAIN8 | VIDEO_FORMAT_AV1_MAIN10
    );
    // A PC without AV1 streams HEVC, not H.264.
    assert_eq!(video_formats(CodecPreference::Av1, false, SCM_H264 | SCM_HEVC), VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265);
    assert_eq!(video_formats(CodecPreference::Hevc, false, SCM_H264), VIDEO_FORMAT_H264);
    assert_eq!(video_formats(CodecPreference::H264, true, all), VIDEO_FORMAT_H264);
    assert_eq!(video_formats(CodecPreference::PyroWave, false, SCM_H264 | SCM_PYROWAVE), VIDEO_FORMAT_H264 | VIDEO_FORMAT_PYROWAVE);
    assert_eq!(video_formats(CodecPreference::PyroWave444, true, SCM_H264 | SCM_PYROWAVE_MAIN10_444), VIDEO_FORMAT_H264 | VIDEO_FORMAT_PYROWAVE_MAIN10_444);
    assert_eq!(video_formats(CodecPreference::PyroWave444, false, SCM_H264), VIDEO_FORMAT_H264);
}

#[test]
fn the_streamed_pyrowave_names_its_chroma_and_depth() {
    assert_eq!(codec_name(VIDEO_FORMAT_PYROWAVE), "PyroWave");
    assert_eq!(codec_name(VIDEO_FORMAT_PYROWAVE_444), "PyroWave 4:4:4");
    assert_eq!(codec_name(VIDEO_FORMAT_PYROWAVE_MAIN10), "PyroWave 10-bit");
    assert_eq!(codec_name(VIDEO_FORMAT_PYROWAVE_MAIN10_444), "PyroWave 10-bit 4:4:4");
    assert_eq!(codec_name(VIDEO_FORMAT_H265_REXT8_444), "HEVC", "other codecs keep their names");
}

const COLOR_SCM: u32 = SCM_H264 | SCM_HEVC | SCM_AV1_MAIN8 | SCM_HEVC_MAIN10 | SCM_AV1_MAIN10
    | SCM_H264_HIGH8_444 | SCM_HEVC_REXT8_444 | SCM_AV1_HIGH8_444;

fn decodes_all() -> (u32, String) {
    (color::YUV444_FORMATS | color::SDR10_FORMATS | color::YUV444_10_FORMATS, String::new())
}

/// A PC that encodes HEVC RExt 4:4:4 10-bit (NVENC) streams both settings at once to a device
/// that decodes and draws it, and says both; without it 4:4:4 wins as before.
#[test]
fn both_settings_stream_10_bit_4_4_4_where_the_pc_and_the_device_have_it() {
    let both = Settings { codec: CodecPreference::Hevc, prefer_yuv444: true, sdr_10bit: true, ..Default::default() };
    let scm = COLOR_SCM | SCM_HEVC_REXT10_444;
    let offer = offered_formats(&both, scm, &decodes_all());
    assert_eq!(offer.formats, VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_REXT10_444);
    assert!(launches_ten_bit_sdr(&both, &offer), "an SDR launch of a 10-bit format says hdrMode=0");
    assert_eq!(color_notice(&offer, VIDEO_FORMAT_H265_REXT10_444).as_deref(), Some("Streaming in YUV 4:4:4.\nStreaming 10-bit color (SDR)."));
    let without = (color::YUV444_FORMATS | color::SDR10_FORMATS, String::new());
    let offer = offered_formats(&both, scm, &without);
    assert_eq!(offer.formats, VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_REXT8_444);
}

#[test]
fn the_launch_offers_what_the_4_4_4_and_10_bit_settings_add() {
    let hevc = Settings { codec: CodecPreference::Hevc, ..Default::default() };
    let plain = offered_formats(&hevc, COLOR_SCM, &decodes_all());
    assert_eq!(plain.formats, video_formats(CodecPreference::Hevc, false, COLOR_SCM), "both off: the codec choice alone");
    assert_eq!(color_notice(&plain, VIDEO_FORMAT_H265), None);
    let yuv444 = offered_formats(&Settings { prefer_yuv444: true, ..hevc.clone() }, COLOR_SCM, &decodes_all());
    assert_eq!(yuv444.formats, VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_REXT8_444);
    assert_eq!(color_notice(&yuv444, VIDEO_FORMAT_H265_REXT8_444).as_deref(), Some("Streaming in YUV 4:4:4."));
    let sdr10 = offered_formats(&Settings { sdr_10bit: true, ..hevc.clone() }, COLOR_SCM, &decodes_all());
    assert_eq!(sdr10.formats, VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10);
    assert_eq!(color_notice(&sdr10, VIDEO_FORMAT_H265_MAIN10).as_deref(), Some("Streaming 10-bit color (SDR)."));
    assert_eq!(color_notice(&sdr10, VIDEO_FORMAT_H265).as_deref(), Some("This PC didn’t accept 10-bit, so the stream is 8-bit."));
    // The probe's answer decides: what the decoder can't draw is not offered, and its reason is told.
    let none = (0, "HEVC 4:4:4: the decoder failed on the fixture".to_owned());
    let refused = offered_formats(&Settings { prefer_yuv444: true, ..hevc.clone() }, COLOR_SCM, &none);
    assert_eq!(refused.formats, plain.formats);
    assert_eq!(color_notice(&refused, VIDEO_FORMAT_H265).as_deref(),
        Some("Sharper colored text isn’t available for this stream: this device can’t decode or draw it (HEVC 4:4:4: the decoder failed on the fixture)."));
    // HDR takes the stream and needs no word; packed full chroma keeps its own offer.
    let hdr = offered_formats(&Settings { hdr: true, prefer_yuv444: true, sdr_10bit: true, ..hevc.clone() }, COLOR_SCM, &decodes_all());
    assert_eq!(hdr.formats, video_formats(CodecPreference::Hevc, true, COLOR_SCM));
    assert_eq!(color_notice(&hdr, VIDEO_FORMAT_H265_MAIN10), None);
    let packed = offered_formats(&Settings { experimental_packed_444: true, prefer_yuv444: true, ..hevc.clone() }, COLOR_SCM, &decodes_all());
    assert_eq!(packed.formats, video_formats(CodecPreference::Hevc, false, 0));
    assert_eq!(color_notice(&packed, VIDEO_FORMAT_H265).as_deref(),
        Some("Sharper colored text isn’t available for this stream: sharper colored text is on and has its own 4:4:4 and 10-bit handling."));
    // Both on: 4:4:4 wins and 10-bit says why.
    let both = offered_formats(&Settings { prefer_yuv444: true, sdr_10bit: true, ..hevc }, COLOR_SCM, &decodes_all());
    assert_eq!(color_notice(&both, VIDEO_FORMAT_H265_REXT8_444).as_deref(),
        Some("Streaming in YUV 4:4:4.\n10-bit SDR isn’t used, so the stream is 8-bit: sharper colored text is on, and the PC takes 4:4:4 or 10-bit, not both."));
}

#[test]
fn a_ten_bit_sdr_offer_launches_with_hdr_mode_zero() {
    // Hardware lane 4a: without hdrMode=0 the host streamed 10-bit SDR as HDR and the SDR
    // display ended the stream at once, for HEVC Main10 and AV1 alike.
    for codec in [CodecPreference::Hevc, CodecPreference::Av1] {
        let sdr10 = Settings { sdr_10bit: true, codec, ..Default::default() };
        let offer = offered_formats(&sdr10, COLOR_SCM, &decodes_all());
        assert!(launches_ten_bit_sdr(&sdr10, &offer), "{codec:?}");
        let request = LaunchRequest { ten_bit_sdr: launches_ten_bit_sdr(&sdr10, &offer), ..Default::default() };
        assert!(request.query(&nightjar_core::http::HostCapabilities::UNKNOWN, false).contains("&hdrMode=0&"));
        // HDR asks for HDR; plain 8-bit, and 10-bit the PC or the decoder can't do, say nothing.
        let hdr = Settings { hdr: true, ..sdr10.clone() };
        assert!(!launches_ten_bit_sdr(&hdr, &offered_formats(&hdr, COLOR_SCM, &decodes_all())));
        let plain = Settings { codec, ..Default::default() };
        assert!(!launches_ten_bit_sdr(&plain, &offered_formats(&plain, COLOR_SCM, &decodes_all())));
        assert!(!launches_ten_bit_sdr(&sdr10, &offered_formats(&sdr10, SCM_H264 | SCM_HEVC | SCM_AV1_MAIN8, &decodes_all())));
        assert!(!launches_ten_bit_sdr(&sdr10, &offered_formats(&sdr10, COLOR_SCM, &(0, "no".into()))));
    }
}

#[test]
fn the_decoder_is_checked_only_when_a_setting_reaches_it() {
    let on = Settings { prefer_yuv444: true, ..Default::default() };
    assert!(!probes_color(&Settings::default()));
    assert!(probes_color(&on));
    assert!(probes_color(&Settings { sdr_10bit: true, ..Default::default() }));
    assert!(!probes_color(&Settings { hdr: true, ..on.clone() }));
    assert!(!probes_color(&Settings { experimental_packed_444: true, ..on.clone() }));
    assert!(!probes_color(&Settings { codec: CodecPreference::PyroWave, ..on }));
}

/// The launch decoded HEVC and AV1 fixtures on the UI thread whatever the stream could use.
#[test]
fn the_base_probe_checks_only_formats_the_launch_can_offer() {
    let with = |codec, hdr| base_probe_formats(&Settings { codec, hdr, ..Default::default() });
    assert_eq!(with(CodecPreference::H264, true), 0, "H.264 is not checked");
    assert_eq!(with(CodecPreference::Hevc, false), VIDEO_FORMAT_H265, "AV1 only when chosen");
    assert_eq!(with(CodecPreference::Auto, true), VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10);
    assert_eq!(with(CodecPreference::Av1, false), VIDEO_FORMAT_H265 | VIDEO_FORMAT_AV1_MAIN8, "HEVC is AV1's fallback");
    assert_eq!(with(CodecPreference::Av1, true), color_probe::BASE_FORMATS);
    assert_eq!(with(CodecPreference::PyroWave, false), VIDEO_FORMAT_H265, "the HEVC PyroWave falls back to");
    let packed = Settings { experimental_packed_444: true, codec: CodecPreference::Av1, ..Default::default() };
    assert_eq!(base_probe_formats(&packed), 0);
    // What is not checked is not offered either: each launch's offer stays within it.
    for codec in [CodecPreference::H264, CodecPreference::Hevc, CodecPreference::Av1, CodecPreference::Auto, CodecPreference::PyroWave] {
        for hdr in [false, true] {
            let settings = Settings { codec, hdr, ..Default::default() };
            let offered = offered_formats(&settings, 0, &decodes_all()).formats;
            assert_eq!(offered & color_probe::BASE_FORMATS & !base_probe_formats(&settings), 0, "{codec:?} hdr {hdr}");
        }
    }
    assert!(color_probe::probe_base(Hardware::None, 0).decodable == color_probe::BASE_FORMATS, "software is never checked");
}

/// A hardware decoder that cannot decode HEVC Main10 or AV1 (`color_probe::probe_base`) is
/// never offered them, so the PC never streams what FFmpeg would decode in software or not at
/// all; H.264 always stays, and packed full chroma checks its own formats.
#[test]
fn plain_formats_the_hardware_decoder_cannot_decode_are_not_offered() {
    let all = SCM_H264 | SCM_HEVC | SCM_HEVC_MAIN10 | SCM_AV1_MAIN8 | SCM_AV1_MAIN10;
    let hdr = |codec| Settings { codec, hdr: true, ..Default::default() };
    let offered = |settings: &Settings, decodable| offered_formats_decoding(settings, all, &decodes_all(), decodable).formats;
    let eight_bit = VIDEO_FORMAT_H265 | VIDEO_FORMAT_AV1_MAIN8;
    assert_eq!(offered(&hdr(CodecPreference::Hevc), color_probe::BASE_FORMATS), VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10);
    assert_eq!(offered(&hdr(CodecPreference::Hevc), eight_bit), VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265, "no Main10 decoder");
    // NUC (Intel Gen 9.5): AV1 with HDR streamed H.264 8-bit though HEVC Main10 decodes.
    let hevc_all = VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10;
    assert_eq!(offered(&hdr(CodecPreference::Av1), hevc_all), VIDEO_FORMAT_H264 | hevc_all, "no AV1 decoder at all");
    assert_eq!(offered(&hdr(CodecPreference::Av1), VIDEO_FORMAT_H265), VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265, "no 10-bit decoder");
    assert_eq!(offered(&hdr(CodecPreference::Av1), hevc_all | VIDEO_FORMAT_AV1_MAIN8), VIDEO_FORMAT_H264 | hevc_all,
        "AV1 8-bit only: HEVC Main10 keeps HDR");
    assert_eq!(offered(&hdr(CodecPreference::Av1), VIDEO_FORMAT_H265 | VIDEO_FORMAT_AV1_MAIN8), VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_AV1_MAIN8);
    assert_eq!(offered(&hdr(CodecPreference::Av1), color_probe::BASE_FORMATS), VIDEO_FORMAT_H264 | color_probe::BASE_FORMATS);
    assert_eq!(offered(&Settings::default(), 0), VIDEO_FORMAT_H264);
    let packed = Settings { experimental_packed_444: true, codec: CodecPreference::Hevc, ..Default::default() };
    assert_eq!(offered(&packed, 0), offered(&packed, color_probe::BASE_FORMATS));
    // `offered_formats` is the offer of a decoder that decodes them all.
    assert_eq!(offered_formats(&hdr(CodecPreference::Hevc), all, &decodes_all()).formats, offered(&hdr(CodecPreference::Hevc), color_probe::BASE_FORMATS));
    // Software decoding decodes them all, by the software decoder.
    let software = color_probe::Base::software();
    assert_eq!(software.decodable, color_probe::BASE_FORMATS);
    assert!(software.backends.iter().all(|&(_, backend)| backend == Hardware::None) && software.backends.len() == 4);
    assert_eq!(color_probe::probe_base(Hardware::None, color_probe::BASE_FORMATS), software, "software decoding is not checked");
}

#[test]
fn a_codec_this_device_does_not_decode_is_told_with_the_one_streamed() {
    let all = SCM_H264 | SCM_HEVC | SCM_HEVC_MAIN10 | SCM_AV1_MAIN8 | SCM_AV1_MAIN10;
    let av1_hdr = Settings { codec: CodecPreference::Av1, hdr: true, ..Default::default() };
    let hevc_all = VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10;
    let (log, notice) = codec_fallback(&av1_hdr, all, hevc_all, 0, VIDEO_FORMAT_H265_MAIN10).expect("told");
    assert_eq!(notice, "This device doesn’t decode AV1 10-bit, so Nightjar uses HEVC Main10.");
    assert!(log.contains("AV1 10-bit was chosen") && log.ends_with("streaming HEVC Main10"), "{log}");
    let av1 = Settings { hdr: false, ..av1_hdr.clone() };
    assert_eq!(codec_fallback(&av1, all, hevc_all, 0, VIDEO_FORMAT_H265).map(|(_, n)| n).as_deref(),
        Some("This device doesn’t decode AV1, so Nightjar uses HEVC."));
    let hevc = Settings { codec: CodecPreference::Hevc, ..Default::default() };
    assert_eq!(codec_fallback(&hevc, all, 0, 0, VIDEO_FORMAT_H264).map(|(_, n)| n).as_deref(),
        Some("This device doesn’t decode HEVC, so Nightjar uses H.264."));
    // Nothing to tell: it decodes, it streams, the PC doesn't encode it, or no codec was chosen.
    assert_eq!(codec_fallback(&av1_hdr, all, color_probe::BASE_FORMATS, 0, VIDEO_FORMAT_AV1_MAIN10), None);
    assert_eq!(codec_fallback(&av1_hdr, SCM_H264 | SCM_HEVC | SCM_HEVC_MAIN10, hevc_all, 0, VIDEO_FORMAT_H265_MAIN10), None);
    assert_eq!(codec_fallback(&Settings::default(), all, 0, 0, VIDEO_FORMAT_H264), None);
    let german = crate::strings::tests::with_lang(crate::strings::Lang::De, || codec_fallback(&hevc, all, 0, 0, VIDEO_FORMAT_H264));
    assert_eq!(german.map(|(_, n)| n).as_deref(), Some("Dieses Gerät decodiert HEVC nicht, daher nutzt Nightjar H.264."));
}

/// Packed full chroma on AV1 offers HEVC too (beecb7aa1), so AV1 the PC or device has no
/// packed mode for streams HEVC packed where it was refused: the player is told.
#[test]
fn packed_full_chroma_on_av1_that_streams_hevc_is_told() {
    let packed = Settings { codec: CodecPreference::Av1, experimental_packed_444: true, ..Default::default() };
    assert_ne!(offered_formats(&packed, 0, &decodes_all()).formats & VIDEO_FORMAT_H265, 0, "HEVC is the packed fallback");
    let (log, notice) = codec_fallback(&packed, 0, 0, 0, VIDEO_FORMAT_H265).expect("told");
    assert_eq!(notice, "Sharper colored text can’t run AV1 with this PC and device, so Nightjar uses HEVC.");
    assert!(log.contains("full chroma") && log.ends_with("streaming HEVC"), "{log}");
    // HDR: HEVC Main10 packed ranks above AV1 8-bit packed, and says so.
    let hdr = Settings { hdr: true, ..packed.clone() };
    assert_eq!(codec_fallback(&hdr, 0, 0, 0, VIDEO_FORMAT_H265_MAIN10).map(|(_, n)| n).as_deref(),
        Some("Sharper colored text can’t run AV1 10-bit with this PC and device, so Nightjar uses HEVC Main10."));
    // AV1 packed at either depth, and HEVC packed chosen, need no word (8-bit has its own notice).
    assert_eq!(codec_fallback(&packed, 0, 0, 0, VIDEO_FORMAT_AV1_MAIN8), None);
    assert_eq!(codec_fallback(&hdr, 0, 0, 0, VIDEO_FORMAT_AV1_MAIN8), None);
    assert_eq!(codec_fallback(&Settings { codec: CodecPreference::Hevc, ..hdr }, 0, 0, 0, VIDEO_FORMAT_H265), None);
    let german = crate::strings::tests::with_lang(crate::strings::Lang::De, || codec_fallback(&packed, 0, 0, 0, VIDEO_FORMAT_H265));
    assert_eq!(german.map(|(_, n)| n).as_deref(), Some("Schärferer farbiger Text kann AV1 mit diesem PC und Gerät nicht nutzen, daher nutzt Nightjar HEVC."));
}

/// AV1 whose 8-bit decodes but whose Main10 or 4:4:4 doesn't streamed 10-bit SDR and 4:4:4
/// refused as AV1 8-bit, though HEVC carried them; HDR already made way (Android's rule).
#[test]
fn ten_bit_sdr_and_4_4_4_that_only_hevc_carries_stream_hevc_and_say_why() {
    let base = VIDEO_FORMAT_H265 | VIDEO_FORMAT_AV1_MAIN8;
    let hevc_color = (color::YUV444_FORMATS | color::SDR10_FORMATS) & !(VIDEO_FORMAT_AV1_MAIN10 | VIDEO_FORMAT_AV1_HIGH8_444);
    let sdr10 = Settings { codec: CodecPreference::Av1, sdr_10bit: true, ..Default::default() };
    let offer = offered_formats_decoding(&sdr10, COLOR_SCM, &(hevc_color, String::new()), base);
    assert_eq!((offer.formats, offer.sdr10.clone()), (VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10, Ok(())));
    assert_eq!(codec_fallback(&sdr10, COLOR_SCM, base, hevc_color, VIDEO_FORMAT_H265_MAIN10).map(|(_, n)| n).as_deref(),
        Some("This device doesn’t decode AV1 10-bit, so Nightjar uses HEVC Main10."));
    let yuv444 = Settings { sdr_10bit: false, prefer_yuv444: true, ..sdr10.clone() };
    let offer = offered_formats_decoding(&yuv444, COLOR_SCM, &(hevc_color, String::new()), base);
    assert_eq!((offer.formats, offer.yuv444.clone()), (VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_REXT8_444, Ok(())));
    assert_eq!(codec_fallback(&yuv444, COLOR_SCM, base, hevc_color, VIDEO_FORMAT_H265_REXT8_444).map(|(_, n)| n).as_deref(),
        Some("This device doesn’t decode AV1 4:4:4, so Nightjar uses HEVC 4:4:4."));
    // A PC without AV1 Main10 makes way too, unsaid as with HDR; AV1 that carries it keeps AV1.
    let no_av1_main10 = COLOR_SCM & !SCM_AV1_MAIN10;
    let offer = offered_formats_decoding(&sdr10, no_av1_main10, &decodes_all(), base);
    assert_eq!(offer.formats & (VIDEO_FORMAT_AV1_MAIN8 | VIDEO_FORMAT_H265_MAIN10), VIDEO_FORMAT_H265_MAIN10);
    assert_eq!(codec_fallback(&sdr10, no_av1_main10, base, color::YUV444_FORMATS | color::SDR10_FORMATS, VIDEO_FORMAT_H265_MAIN10), None);
    let offer = offered_formats_decoding(&sdr10, COLOR_SCM, &decodes_all(), base);
    assert_eq!(offer.formats, VIDEO_FORMAT_H264 | base | VIDEO_FORMAT_AV1_MAIN10);
    assert_eq!(codec_fallback(&sdr10, COLOR_SCM, base, hevc_color, VIDEO_FORMAT_H265), None, "8-bit HEVC is no color fallback");
}

/// 10-bit launches decoded the 4:4:4 fixtures too with 4:4:4 off (about 12 ms).
#[test]
fn the_color_probe_checks_only_formats_the_settings_can_add() {
    let with = |codec, prefer_yuv444, sdr_10bit| color_probe_formats(&Settings { codec, prefer_yuv444, sdr_10bit, ..Default::default() });
    assert_eq!(with(CodecPreference::Hevc, false, false), 0);
    assert_eq!(with(CodecPreference::Hevc, false, true), VIDEO_FORMAT_H265_MAIN10);
    assert_eq!(with(CodecPreference::Hevc, true, false), VIDEO_FORMAT_H264_HIGH8_444 | VIDEO_FORMAT_H265_REXT8_444);
    assert_eq!(with(CodecPreference::Av1, true, true), color::YUV444_FORMATS | color::SDR10_FORMATS | color::YUV444_10_FORMATS);
    assert_eq!(with(CodecPreference::Hevc, true, true) & color::YUV444_10_FORMATS, VIDEO_FORMAT_H265_REXT10_444, "both on ask for 10-bit 4:4:4");
    assert_eq!(with(CodecPreference::Hevc, true, false) & color::YUV444_10_FORMATS, 0);
    assert_eq!(with(CodecPreference::Hevc, false, true) & color::YUV444_10_FORMATS, 0);
    assert_eq!(with(CodecPreference::H264, true, true), VIDEO_FORMAT_H264_HIGH8_444);
    assert_eq!(color_probe_formats(&Settings { hdr: true, sdr_10bit: true, ..Default::default() }), 0, "HDR takes the stream");
    // Whatever the settings, the offer adds no 4:4:4 or 10-bit format the probe skipped.
    for codec in [CodecPreference::H264, CodecPreference::Hevc, CodecPreference::Av1, CodecPreference::Auto] {
        for (yuv444, sdr10) in [(false, true), (true, false), (true, true)] {
            let settings = Settings { codec, prefer_yuv444: yuv444, sdr_10bit: sdr10, ..Default::default() };
            let decodable = (color_probe_formats(&settings), String::new());
            assert_eq!(offered_formats(&settings, COLOR_SCM, &decodable).formats, offered_formats(&settings, COLOR_SCM, &decodes_all()).formats,
                "{codec:?} {yuv444} {sdr10}");
        }
    }
}

#[test]
fn color_notices_are_german_in_german() {
    let offer = offered_formats(&Settings { sdr_10bit: true, codec: CodecPreference::Hevc, ..Default::default() }, SCM_H264 | SCM_HEVC, &decodes_all());
    let text = crate::strings::tests::with_lang(crate::strings::Lang::De, || color_notice(&offer, VIDEO_FORMAT_H265));
    assert_eq!(text.as_deref(), Some("10-Bit-SDR wird nicht verwendet, der Stream läuft mit 8 Bit: der PC codiert es für diesen Codec nicht."));
}

#[test]
fn a_chosen_pyrowave_that_does_not_stream_is_logged_and_told() {
    let fallback = |codec: CodecPreference, scm: u32, negotiated: u32| {
        let offered = offered_formats(&Settings { codec, ..Default::default() }, scm, &decodes_all()).formats;
        pyrowave_fallback(codec, (scm, scm), offered, negotiated)
    };
    // The PC lists its codecs, and PyroWave is not among them: it was never offered.
    let (log, notice) = fallback(CodecPreference::PyroWave, SCM_H264 | SCM_HEVC, VIDEO_FORMAT_H265).expect("a fallback");
    assert_eq!(log, "nightjar: PyroWave was chosen, but the host does not offer it (codec support 0x101, reported 0x101); streaming HEVC");
    assert_eq!(notice, "This PC doesn’t offer PyroWave, so Nightjar uses HEVC.");
    // Offered, and the PC picked another codec.
    let (log, notice) = fallback(CodecPreference::PyroWave444, 0, VIDEO_FORMAT_H264).expect("a fallback");
    assert_eq!(log, "nightjar: PyroWave was chosen, but the host reports no trusted codec support and chose another codec (codec support 0x0, reported 0x0); streaming H.264");
    assert_eq!(notice, "This PC doesn’t offer PyroWave, so Nightjar uses H.264.");
    let (log, _) = fallback(CodecPreference::PyroWave, SCM_H264 | SCM_HEVC | SCM_PYROWAVE, VIDEO_FORMAT_H265).expect("a fallback");
    assert!(log.contains("but the host chose another codec (codec support"), "{log}");
    // The log keeps what serverinfo said when the trusted value stripped PyroWave.
    let (log, _) = pyrowave_fallback(CodecPreference::PyroWave, (SCM_H264 | SCM_HEVC, SCM_H264 | SCM_HEVC | SCM_PYROWAVE),
        video_formats(CodecPreference::PyroWave, false, SCM_H264 | SCM_HEVC), VIDEO_FORMAT_H265).expect("a fallback");
    assert!(log.contains(&format!("(codec support 0x101, reported {:#x})", SCM_H264 | SCM_HEVC | SCM_PYROWAVE)), "{log}");
    // PyroWave streams, or was not chosen: nothing to say.
    assert_eq!(fallback(CodecPreference::PyroWave, SCM_H264 | SCM_PYROWAVE, VIDEO_FORMAT_PYROWAVE), None);
    assert_eq!(fallback(CodecPreference::PyroWave444, SCM_H264 | SCM_PYROWAVE, VIDEO_FORMAT_PYROWAVE), None);
    assert_eq!(fallback(CodecPreference::Auto, SCM_H264 | SCM_HEVC, VIDEO_FORMAT_H265), None);
    let german = crate::strings::tests::with_lang(crate::strings::Lang::De, || {
        fallback(CodecPreference::PyroWave, SCM_H264 | SCM_HEVC, VIDEO_FORMAT_H265).map(|(_, notice)| notice)
    });
    assert_eq!(german.as_deref(), Some("Dieser PC bietet PyroWave nicht an, daher nutzt Nightjar HEVC."));
}

#[test]
fn the_pyrowave_fallback_is_told_once_per_pc_until_pyrowave_streams_again() {
    let text = "This PC doesn’t offer PyroWave, so Nightjar uses HEVC.";
    let mut pc = HostRecord::default();
    let mut other = HostRecord::default();
    assert_eq!(pyrowave_notice(&mut pc, true, Some(text), VIDEO_FORMAT_H265), (Some(text.to_owned()), true), "the first time");
    // A reconnect, automatic or from the menu, and every later stream from this PC: the log only.
    assert_eq!(pyrowave_notice(&mut pc, true, Some(text), VIDEO_FORMAT_H265), (None, false));
    // Kept with the PC's record, so a restart of Nightjar stays quiet too.
    let mut saved: HostRecord = serde_json::from_str(&serde_json::to_string(&pc).unwrap()).unwrap();
    assert_eq!(pyrowave_notice(&mut saved, true, Some(text), VIDEO_FORMAT_H265), (None, false));
    assert_eq!(pyrowave_notice(&mut other, true, Some(text), VIDEO_FORMAT_H265), (Some(text.to_owned()), true), "another PC is told");
    // Streams that say nothing of PyroWave leave it as it is.
    assert_eq!(pyrowave_notice(&mut pc, false, None, VIDEO_FORMAT_H265), (None, false));
    assert_eq!(pyrowave_notice(&mut pc, true, None, VIDEO_FORMAT_H265), (None, false));
    // PyroWave streams from the PC again: losing it later is told again.
    assert_eq!(pyrowave_notice(&mut pc, true, None, VIDEO_FORMAT_PYROWAVE), (None, true));
    assert_eq!(pyrowave_notice(&mut pc, true, None, VIDEO_FORMAT_PYROWAVE), (None, false), "nothing to save then");
    assert_eq!(pyrowave_notice(&mut pc, true, Some(text), VIDEO_FORMAT_H265), (Some(text.to_owned()), true));
    // A record saved before the notices existed reads with none shown.
    let old: HostRecord = serde_json::from_str(r#"{"name":"PC","address":"192.0.2.1"}"#).unwrap();
    assert!(old.notices.is_empty());
}

#[test]
fn rtp_clock_unwraps() {
    let mut c = RtpClock::default();
    let a = c.host_us(u32::MAX - 899);
    let b = c.host_us(600);
    // 1500 ticks of 90 kHz across the wrap, give or take the rounding.
    assert!((b - a - 16_667).abs() <= 1, "{}", b - a);
}

#[test]
fn the_pipeline_line_says_each_stage_in_fps() {
    let report = PipelineRatesReport {
        short_window: false,
        window_ms: 1000,
        source_mhz: 119_880,
        capture_mhz: 240_000,
        submit_mhz: 120_000,
        output_mhz: 120_000,
        repeat_mhz: 0,
    };
    assert_eq!(pipeline_rates_line(&report), "119.9 source, 240.0 captured, 120.0 encoded, 120.0 sent, 0.0 repeated fps");
    let unknown = PipelineRatesReport { source_mhz: UNKNOWN, ..report };
    assert!(pipeline_rates_line(&unknown).starts_with("- source, 240.0 captured"));
    crate::strings::set(crate::strings::Lang::De);
    assert_eq!(pipeline_rates_line(&report), "119.9 Quelle, 240.0 erfasst, 120.0 kodiert, 120.0 gesendet, 0.0 wiederholt fps");
}

#[test]
fn a_host_without_hardware_readings_is_not_asked_again_but_a_failed_request_is() {
    use nightjar_core::http::{Availability, Error};
    let sample = HardwareMetrics::parse(r#"{"version":1,"sampled_at_unix_ms":5,"interval_ms":1000,"cpu":{},"gpus":[]}"#).unwrap();
    assert_eq!(hardware_answer(&Ok(sample)), HardwareAnswer::Sample { sampled_at_unix_ms: 5, interval_ms: 1000 });
    let unavailable = [
        HostApiError::Unavailable { feature: Feature::HardwareMetrics, availability: Availability::Unknown },
        HostApiError::UnsupportedVersion,
        HostApiError::Host(Error::NotFound("api/hardware-metrics".into())),
        HostApiError::Host(Error::Status { code: 403, message: String::new() }),
        HostApiError::Host(Error::Status { code: 409, message: String::new() }),
    ];
    for error in unavailable {
        assert_eq!(hardware_answer(&Err(error)), HardwareAnswer::Unavailable);
    }
    for error in [Error::Status { code: 500, message: String::new() }, Error::Malformed("{".into()), Error::Cancelled] {
        assert_eq!(hardware_answer(&Err(HostApiError::Host(error))), HardwareAnswer::Failed, "tried again later");
    }
}

#[test]
fn overlay_modes_show_their_own_lines() {
    let overlay = Overlay {
        lines: vec![("Decoder".into(), "d3d11va".into())],
        statistics: vec![("Frames".into(), "60".into())],
        ..Default::default()
    };
    assert_eq!(overlay.lines(OverlayMode::Debug)[0].0, "Decoder");
    assert_eq!(overlay.lines(OverlayMode::Statistics)[0].0, "Frames");
    assert!(overlay.lines(OverlayMode::Compact).is_empty(), "Compact draws its one line instead");
}

#[test]
fn the_link_notice_line_says_the_lowered_bitrate() {
    use rubylight_protocol::control::LinkReason;
    let notice = |current_kbps| LinkNotice {
        state: LinkState::Limited,
        reason: LinkReason::Network,
        ceiling_kbps: 150_000,
        current_kbps,
    };
    assert_eq!(link_notice_line(&notice(37_500)), "PC lowered the bitrate to 38 Mbps: network");
    assert_eq!(link_notice_line(&notice(9_900)), "PC lowered the bitrate to 9.9 Mbps: network");
    crate::strings::set(crate::strings::Lang::De);
    assert_eq!(link_notice_line(&notice(4_500)), "PC hat die Bitrate auf 4.5 Mbit/s gesenkt: Netzwerk");
}

#[test]
fn terminations_read_well() {
    assert!(termination_text(0).is_none());
    assert!(termination_text(-100).unwrap().contains("UDP"));
    // A stop before the first picture is usually the game closing, not protected content.
    let early = termination_text(nightjar_core::session::error::UNEXPECTED_EARLY).unwrap();
    assert!(early.contains("The game may have closed"), "{early}");
}

#[test]
fn a_start_the_pc_refused_in_the_handshake_shows_its_reason() {
    let refused = connect::start_failure_text("Could not start the stream: RTSP handshake failed".into(), Stage::RtspHandshake, || {
        Some("No encoder on this PC can stream HEVC".into())
    });
    assert!(refused.starts_with("Could not start the stream: RTSP handshake failed"));
    assert!(refused.ends_with("The PC reported: No encoder on this PC can stream HEVC"), "{refused}");
    // A PC that keeps no reason, as other hosts never do, leaves today's text.
    let plain = connect::start_failure_text("Could not start the stream: x".into(), Stage::RtspHandshake, || None);
    assert_eq!(plain, "Could not start the stream: x");
    assert_eq!(connect::start_failure_text("e".into(), Stage::RtspHandshake, || Some(" ".into())), "e");
    // Another stage does not ask the PC at all.
    let other = connect::start_failure_text("e".into(), Stage::VideoStreamStart, || panic!("asked the PC"));
    assert_eq!(other, "e");
}

#[test]
fn only_terminations_the_pc_sent_ask_it_for_the_reason() {
    use nightjar_core::session::error;
    for code in [
        0x8000_4005_u32 as i32,
        0xc000_0005_u32 as i32,
        error::UNEXPECTED_EARLY,
        error::PROTECTED_CONTENT,
        error::FRAME_CONVERSION,
    ] {
        assert!(host_ended_stream(code), "{code:#x}");
    }
    for code in [error::GRACEFUL, error::DISCONNECTED, error::NO_VIDEO_TRAFFIC, error::NO_VIDEO_FRAME, 110, 10054, 503] {
        assert!(!host_ended_stream(code), "{code}");
    }
    let body = doctor::with_host_reason(
        "The PC could not convert the picture for streaming.".into(),
        Some("HDR is on, but the display is not in HDR"),
    );
    assert_eq!(
        body,
        "The PC could not convert the picture for streaming.\n\nThe PC reported: HDR is on, but the display is not in HDR"
    );
    assert_eq!(doctor::with_host_reason("Ended.".into(), None), "Ended.");
}

#[test]
fn an_unreported_or_unusable_refresh_rate_leaves_pacing_unfloored() {
    assert_eq!(display().refresh_interval_us(), Some(6_949), "1 s / 143.9 Hz, in whole microseconds");
    let unknown = [
        Display { refresh_hz: 60.0, refresh_reported: false, ..display() },
        Display { refresh_hz: f32::NAN, refresh_reported: true, ..display() },
        Display { refresh_hz: f32::INFINITY, refresh_reported: true, ..display() },
        Display { refresh_hz: 0.0, refresh_reported: true, ..display() },
    ];
    for display in unknown {
        assert_eq!(display.refresh_interval_us(), None, "{display:?} sets no floor");
    }
}

#[test]
fn a_reconfigured_rate_is_confirmed_by_the_hosts_frame_timestamps() {
    assert!(shows_rate(1500, 60_000));
    assert!(shows_rate(1499, 60_000), "the host rounds its 90 kHz stamps");
    assert!(shows_rate(750, 120_000));
    assert!(shows_rate(1502, 59_940), "59.94 Hz at 90 kHz");
    assert!(!shows_rate(1500, 120_000), "a rate the host did not apply");
    assert!(!shows_rate(750, 60_000));
    assert!(!shows_rate(0, 60_000));
    assert!(!shows_rate(1500, 0), "no rate to confirm");
}

#[test]
fn teardown_runs_off_the_calling_thread() {
    let caller = std::thread::current().id();
    let (sender, receiver) = std::sync::mpsc::channel();
    let mut teardown = crate::stream::spawn_background(move || {
        let _ = sender.send(std::thread::current().id());
    });
    let ran_on = receiver.recv_timeout(std::time::Duration::from_secs(10)).expect("teardown runs");
    assert_ne!(ran_on, caller, "a session's stop joins its threads, never on the UI thread");
    assert!(teardown.join_until(Instant::now() + Duration::from_secs(10)));
}

// A request the host does not apply puts every field it changed back together: the facts' rate,
// the shared millihertz, the settings (rate and size), the request in flight and, through it, the
// bitrate policy's mode, the menu's rate and the pacer's frame time.
#[test]
fn a_timed_out_reconfigure_restores_the_whole_mode() {
    let before = ModeBefore {
        fps_millihz: 59_940,
        fps: 60,
        settings_fps: 60,
        width: 2560,
        height: 1440,
        requested: Reconfigure { width: 2560, height: 1440, fps_millihz: 59_940 },
    };
    let mut settings = Settings { fps: 120, width: 1920, height: 1080, ..Default::default() };
    let shared = std::sync::atomic::AtomicU32::new(120_000);
    let mut requested = Reconfigure { width: 1920, height: 1080, fps_millihz: 120_000 };
    let mut fps = 120;
    before.restore(&mut fps, &shared, &mut settings, &mut requested);
    assert_eq!((fps, shared.load(Ordering::Relaxed), settings.fps), (60, 59_940, 60));
    assert_eq!((settings.width, settings.height), (2560, 1440));
    assert_eq!(requested, before.requested, "the mode menu and the plan read is the old one");
    assert_eq!(before.fps_millihz.div_ceil(1000), 60, "the pacer gets the old rate");
}

/// The teardown handle is what lets the app hold the renderer until a session's decoder and audio
/// threads have ended, and reports honestly when the deadline passed first.
#[test]
fn a_teardown_is_joined_where_its_owner_must_wait() {
    let (sender, receiver) = std::sync::mpsc::channel();
    let mut teardown = spawn_background(move || {
        let _ = sender.send(());
    });
    assert!(teardown.join_until(Instant::now() + Duration::from_secs(10)));
    assert!(receiver.try_recv().is_ok(), "the stop ran before the join returned");

    let (started, running) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let mut teardown = spawn_background(move || {
        let _ = started.send(());
        let _ = released.recv();
    });
    running.recv().expect("the teardown started");
    assert!(!teardown.join_until(Instant::now() + Duration::from_millis(10)), "a deadline that passed first says the session did not join");
    release.send(()).expect("release the teardown");
}

/// A reconnect joins the old session's teardown before the connect worker's first request, so the
/// PC never sees this device's old control connection and its new one at once.
#[test]
fn a_reconnect_joins_the_old_session_before_its_first_request() {
    let ended = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = ended.clone();
    let previous = spawn_background(move || {
        std::thread::sleep(Duration::from_millis(20));
        flag.store(true, Ordering::Release);
    });
    let shared = pipeline::Shared::new(std::sync::Arc::new(|| {}), 2);
    super::connect::wait_for_previous(Some(previous), &shared);
    assert!(ended.load(Ordering::Acquire), "the old session ended before the launch");
}

/// A renderer switch and the exit prune the teardowns that ended and are busy only while one is
/// still stopping on the device.
#[test]
fn a_teardown_blocks_the_renderer_only_until_it_ends() {
    let mut teardowns = Vec::new();
    teardowns.push(spawn_background(|| {}));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !teardowns[0].finished() {
        assert!(Instant::now() < deadline, "the empty teardown ends");
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(!prune_teardowns(&mut teardowns), "a finished teardown does not block");
    assert!(teardowns.is_empty(), "and is dropped");

    let (release, released) = std::sync::mpsc::channel::<()>();
    teardowns.push(spawn_background(move || {
        let _ = released.recv();
    }));
    assert!(prune_teardowns(&mut teardowns), "a session still stopping blocks");
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while prune_teardowns(&mut teardowns) {
        assert!(Instant::now() < deadline, "the teardown ends");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// A connect attempt that set a session aside to stop hands its teardown to the app, which joins
/// it with the others.
#[test]
fn a_connect_attempt_hands_its_teardown_to_the_app() {
    let shared = pipeline::Shared::new(std::sync::Arc::new(|| {}), 2);
    let (release, released) = std::sync::mpsc::channel::<()>();
    lock(&shared.teardowns).push(spawn_background(move || {
        let _ = released.recv();
    }));
    let mut kept = Vec::new();
    for teardown in std::mem::take(&mut *lock(&shared.teardowns)) {
        kept.push(teardown);
    }
    assert_eq!(kept.len(), 1);
    assert!(prune_teardowns(&mut kept), "still stopping");
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while prune_teardowns(&mut kept) {
        assert!(Instant::now() < deadline, "the teardown ends");
        std::thread::sleep(Duration::from_millis(2));
    }
}
