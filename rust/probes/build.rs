//! Embed Windows version resources for packaged and diagnostic probe binaries.
include!("../assets/version_info.rs");

// The packaged probes ship with release version info; the rest share one diagnostic resource.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    println!("cargo:rerun-if-changed=../assets/version_info.rs");
    for bin in [
        "butterpollo-performance",
        "butterpollo-protocol-performance",
        "butterpollo-video-packet-performance",
    ] {
        let file = format!("{bin}.exe");
        let rc = write_resource(bin, Some(&file), "", "Rubylight diagnostic tool", false);
        embed_resource::compile_for(&rc, [bin], embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
    let rc = write_resource("probes", None, "", "Rubylight diagnostic tool", false);
    embed_resource::compile_for(
        &rc,
        [
            "afmf_capture",
            "afmf_scene",
            "amf_quality",
            "amf_unit_probe",
            "app_route_probe",
            "audio_probe",
            "capture_phase_probe",
            "copy_probe",
            "d3d12_probe",
            "ddx_arrival_probe",
            "ddx_startup_probe",
            "ddx_sync_probe",
            "ddx_wait_probe",
            "display_color_info",
            "display_cost",
            "display_poke",
            "fullscreen_probe",
            "gpu_load",
            "hdr_scope",
            "hdr_snapshot",
            "input_latency_probe",
            "input_path_probe",
            "look_probe",
            "mic_probe",
            "mic_quality",
            "motion_probe",
            "multi_ddx_probe",
            "pace_probe",
            "pacing_link_probe",
            "pad_feedback_probe",
            "pyrowave_compatibility",
            "radeon_probe",
            "radeon_set",
            "restore_probe",
            "rtss_probe",
            "service_shutdown",
            "session_command",
            "session_command_long",
            "steam_deck_probe",
            "system_launch",
            "timer_probe",
            "udp_pacing_probe",
            "udp_shaper",
            "wait_performance",
            "webrtc_tone",
            "wgc_arrival_probe",
            "wgc_cadence_probe",
            "wgc_stamp_probe",
        ],
        embed_resource::NONE,
    )
    .manifest_optional()
    .unwrap();
}
