//! Tests for host-side NVIDIA enhancement selection, caching, and metadata peak reporting.
use super::*;

#[test]
fn captures_map_to_their_sources() {
    assert_eq!(source(Pixel::Bgra8), Source::Sdr);
    assert_eq!(source(Pixel::RgbaF16), Source::ScRgb);
    assert_eq!(source(Pixel::Rgba10Pq), Source::Pq10);
}

#[test]
fn only_a_capture_on_an_nvidia_gpu_runs_the_enhancements() {
    use nvidia_enhance::{ALL, Elsewhere, Enhancement};
    let config = |text: &str| Config::parse(text).unwrap();
    let video = config(
        "rtx_video = true
",
    );
    // Issue #103: the display's Radeon 780M captures beside an RTX 5060 Ti
    // that offers both: nothing runs there, and the warning names the 780M.
    let (runs, elsewhere) = for_capture(&video, ALL, 0x1002, "AMD Radeon 780M Graphics");
    assert_eq!(runs, 0);
    assert_eq!(
        elsewhere,
        Some(Elsewhere {
            gpu: "AMD Radeon 780M Graphics",
            display_bound: false
        })
    );
    assert_eq!(
        nvidia_enhance::select(&video, runs, true, Source::Sdr),
        Enhancement::None
    );
    // On the NVIDIA card everything it offers runs.
    let (runs, elsewhere) = for_capture(&video, ALL, 0x10de, "NVIDIA GeForce RTX 5060 Ti");
    assert_eq!((runs, elsewhere), (ALL, None));
    assert_eq!(
        nvidia_enhance::select(&video, runs, true, Source::Sdr),
        Enhancement::RtxVideo
    );
    // Desktop Duplication and AMD Direct Capture stay on the display's GPU.
    for method in ["ddx", "DXGI", "amd"] {
        let bound = config(&format!(
            "rtx_video = true
capture = {method}
"
        ));
        let (_, elsewhere) = for_capture(&bound, ALL, 0x1002, "AMD Radeon 780M Graphics");
        assert!(elsewhere.unwrap().display_bound, "{method}");
    }
    // Graphics card names the NVIDIA card, and the capture still runs on
    // the 780M: Automatic fell back to Desktop Duplication.
    let chosen = config("rtx_video = true\nadapter_name = NVIDIA GeForce RTX 5060 Ti\n");
    let (_, elsewhere) = for_capture(&chosen, ALL, 0x1002, "AMD Radeon 780M Graphics");
    assert!(elsewhere.unwrap().display_bound);
    // Graphics card names the 780M itself: choosing the NVIDIA card is the advice.
    let igpu = config("rtx_video = true\nadapter_name = AMD Radeon 780M Graphics\n");
    let (_, elsewhere) = for_capture(&igpu, ALL, 0x1002, "AMD Radeon 780M Graphics");
    assert!(!elsewhere.unwrap().display_bound);
    let (_, elsewhere) = for_capture(&video, ALL, 0x8086, "");
    assert_eq!(
        elsewhere.unwrap().gpu,
        "a graphics card that is not NVIDIA's"
    );
}

#[test]
fn the_enhancer_reports_the_generated_peak_only_while_rtx_hdr_runs() {
    let config = Config::parse("hdr_peak_brightness = 900\n").unwrap();
    let mut enhancer = Enhancer::default();
    assert_eq!(enhancer.hdr_peak(&config), None);
    enhancer.inner.selected = Enhancement::RtxHdr;
    assert_eq!(enhancer.hdr_peak(&config), Some(900));
}

#[test]
fn the_choice_is_parsed_again_only_when_its_inputs_change() {
    // RTX HDR asked for and offered, an SDR picture for an HDR stream.
    let mut config = Config::parse("rtx_hdr = true\n").unwrap();
    config
        .values
        .insert(nvidia_enhance::marker("rtx_hdr"), "true".into());
    config
        .values
        .insert(nvidia_enhance::OFFERED_KEY.into(), RTX_HDR.to_string());
    let parsed = parse(
        1,
        0,
        0x10de,
        "NVIDIA GeForce RTX 5060 Ti",
        Pixel::Bgra8,
        true,
        &config,
    );
    assert_eq!(parsed.wanted, Enhancement::RtxHdr);
    assert_eq!(parsed.warning, None);
    // The same revision, capture device, format and HDR state re-use it.
    assert!(parsed.matches(1, 0, Pixel::Bgra8, true));
    // A replaced configuration, a new capture device, another format or a
    // toggled HDR state parses again.
    assert!(!parsed.matches(2, 0, Pixel::Bgra8, true));
    assert!(!parsed.matches(1, 1, Pixel::Bgra8, true));
    assert!(!parsed.matches(1, 0, Pixel::RgbaF16, true));
    assert!(!parsed.matches(1, 0, Pixel::Bgra8, false));
    // What a new revision parses follows the new values, warning
    // included: here RTX HDR is asked for but nothing offers it.
    let mut unoffered = Config::parse("rtx_hdr = true\n").unwrap();
    unoffered
        .values
        .insert(nvidia_enhance::marker("rtx_hdr"), "true".into());
    let reparsed = parse(
        2,
        0,
        0x10de,
        "NVIDIA GeForce RTX 5060 Ti",
        Pixel::Bgra8,
        true,
        &unoffered,
    );
    assert_eq!(reparsed.wanted, Enhancement::None);
    assert!(reparsed.warning.is_some());
}

#[test]
fn the_parsed_choice_answers_what_the_per_picture_reads_answered() {
    // What `Enhancer::apply` read for every picture before: the same
    // helpers over the same values.
    let mut config = Config::parse("rtx_video = true\nrtx_hdr = true\n").unwrap();
    config
        .values
        .insert(nvidia_enhance::marker("rtx_hdr"), "true".into());
    config.values.insert(
        nvidia_enhance::OFFERED_KEY.into(),
        (RTX_HDR | RTX_VIDEO).to_string(),
    );
    let vendor = 0x10de;
    let adapter = "NVIDIA GeForce RTX 5060 Ti";
    let parsed = parse(7, 3, vendor, adapter, Pixel::Bgra8, true, &config);
    let source = Source::Sdr;
    let offered = nvidia_enhance::offered(&config);
    let (runs, elsewhere) = for_capture(&config, offered, vendor, adapter);
    assert_eq!(
        parsed.wanted,
        nvidia_enhance::select(&config, runs, true, source)
    );
    assert_eq!(
        parsed.warning,
        nvidia_enhance::unavailable(&config, offered, true, source, elsewhere)
    );
    assert_eq!(parsed.luminance, nvidia_enhance::rtx_luminance(&config));
    assert_eq!(parsed.parameters, nvidia_enhance::rtx_parameters(&config));
}

#[test]
fn rtx_hdrs_luminance_is_parsed_with_the_choice_and_follows_the_revision() {
    let mut config = Config::parse("rtx_hdr = true\nrtx_hdr_sdr_brightness = 20\n").unwrap();
    config
        .values
        .insert(nvidia_enhance::marker("rtx_hdr"), "true".into());
    config
        .values
        .insert(nvidia_enhance::OFFERED_KEY.into(), RTX_HDR.to_string());
    let mut enhancer = Enhancer::default();
    enhancer.inner.selected = Enhancement::RtxHdr;
    // Nothing parsed yet, so nothing answers.
    assert_eq!(enhancer.luminance(), None);
    // The choice is parsed for revision 1: its luminance answers while
    // it is current, with the values that revision's configuration had.
    enhancer.inner.parsed = Some(parse(
        1,
        0,
        0x10de,
        "NVIDIA GeForce RTX 5060 Ti",
        Pixel::Bgra8,
        true,
        &config,
    ));
    assert_eq!(enhancer.luminance(), Some((120., 1.)));
    // A later revision changed the SDR white; the next parse carries it.
    config
        .values
        .insert("rtx_hdr_sdr_brightness".into(), "40".into());
    enhancer.inner.parsed = Some(parse(
        2,
        0,
        0x10de,
        "NVIDIA GeForce RTX 5060 Ti",
        Pixel::Bgra8,
        true,
        &config,
    ));
    assert_eq!(enhancer.luminance(), Some((140., 1.)));
}

/// RTX HDR converts an SDR picture for an HDR stream. The stream then
/// describes the peak RTX HDR makes, not the SDR monitor's.
#[test]
fn rtx_hdr_describes_the_peak_it_generates() {
    let mut config = Config::parse("rtx_hdr = true\nhdr_peak_brightness = 1200\n").unwrap();
    config
        .values
        .insert(nvidia_enhance::marker("rtx_hdr"), "true".into());
    config.values.insert(
        nvidia_enhance::OFFERED_KEY.into(),
        nvidia_enhance::ALL.to_string(),
    );
    assert_eq!(
        generated_peak(&config, true, Pixel::Bgra8),
        Some(1200),
        "an SDR picture for an HDR stream is generated at the chosen peak"
    );
    assert_eq!(
        generated_peak(&config, false, Pixel::Bgra8),
        None,
        "an SDR stream runs no RTX HDR"
    );
    assert_eq!(
        generated_peak(&config, true, Pixel::Rgba10Pq),
        None,
        "a 10-bit HDR picture is not converted"
    );
    let unasked = Config::parse("rtx_hdr = true\n").unwrap();
    assert_eq!(generated_peak(&unasked, true, Pixel::Bgra8), None);
}

#[test]
fn a_host_without_the_component_reports_nothing_offered() {
    let f = crate::state::test_support::Fixture::new();
    let h = &f.host;
    h.nvidia_enhancement_errors
        .lock()
        .unwrap()
        .push(("rtx_video", "the GPU does not support it".into()));
    let status = status(h);
    assert_eq!(status["component"]["state"], "missing");
    assert_eq!(status["rtx_hdr"], false);
    assert_eq!(status["rtx_video"], false);
    assert_eq!(status["errors"]["rtx_video"], "the GPU does not support it");
    h.nvidia_enhancements.store(RTX_VIDEO, Ordering::Release);
    assert!(status_of(h, "rtx_video"));
}

fn status_of(h: &Shared, key: &str) -> bool {
    status(h)[key].as_bool().unwrap()
}
