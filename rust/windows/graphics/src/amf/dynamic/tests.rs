//! AMF dynamic HDR metadata tests and shared NAL/readout fixtures.

use super::*;
use butterpollo_media_policy::hdr_dynamic::{FrameStats, frame_units, insert_offset};

fn nal(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut unit = vec![0, 0, 0, 1, kind << 1, 0x01];
    unit.extend(body);
    unit
}
fn keyframe() -> Vec<u8> {
    [
        nal(32, &[0x0C]),
        nal(33, &[0x01]),
        nal(34, &[0xC1]),
        nal(19, &[0xAF, 0x12]),
    ]
    .concat()
}
/// A readout as the GPU pass leaves it: ten cells in each bin from `low`
/// to `high`.
fn readout(conversion: u64, low: usize, high: usize) -> HdrStatsReadout {
    let mut histogram = vec![0; crate::compute::HDR_STATS_BINS];
    histogram[low..=high].fill(10);
    let edge = |bin: usize| bin as f32 / crate::compute::HDR_STATS_BINS as f32;
    HdrStatsReadout {
        conversion,
        histogram,
        min_pq: edge(low),
        max_pq: edge(high),
    }
}

#[test]
fn only_hdr_hevc_and_av1_with_the_setting_on_get_dynamic_metadata() {
    let on = butterpollo_config::config::Config::default();
    let off = butterpollo_config::config::Config::parse("hdr_dynamic_metadata = false\n").unwrap();
    let config = |codec, hdr| butterpollo_media_policy::rtsp::Negotiated {
        codec,
        hdr,
        ..Default::default()
    };
    assert_eq!(
        wanted(&config(1, true), &on),
        Some(Wanted::t35(Codec::Hevc))
    );
    assert_eq!(wanted(&config(2, true), &on), Some(Wanted::t35(Codec::Av1)));
    assert_eq!(wanted(&config(1, false), &on), None, "SDR");
    assert_eq!(wanted(&config(0, true), &on), None, "H.264");
    assert_eq!(wanted(&config(3, true), &on), None, "PyroWave");
    for codec in [1, 2] {
        assert_eq!(wanted(&config(codec, true), &off), None, "setting off");
    }
}

#[test]
fn ten_bit_sdr_never_gets_dynamic_metadata() {
    // Hardware lane 4a: a 10-bit SDR launch streamed as HDR switched HDR10+ on.
    let on = butterpollo_config::config::Config::default();
    for codec in [1, 2] {
        // RTSP reads dynamicRangeMode=1 as HDR; hdrMode=0 at launch makes it SDR.
        let mut stream = butterpollo_media_policy::rtsp::Negotiated {
            codec,
            hdr: true,
            ..Default::default()
        };
        assert!(wanted(&stream, &on).is_some(), "an HDR launch keeps it");
        assert!(butterpollo_media_policy::stream_policy::infer_sdr_10bit(
            &mut stream,
            true
        ));
        assert_eq!(wanted(&stream, &on), None, "codec {codec}");
        // Marked SDR but with `hdr` left set: still none.
        stream.hdr = true;
        assert_eq!(wanted(&stream, &on), None, "codec {codec}, hdr left set");
    }
}

#[test]
fn frames_before_the_first_sample_carry_the_static_stand_in() {
    let mut dynamic = Dynamic::new(Codec::Hevc);
    let frame = keyframe();
    let at = insert_offset(Codec::Hevc, &frame).unwrap();
    let with = |metadata: &butterpollo_media_policy::hdr::Metadata| {
        let units = frame_units(Codec::Hevc, &FrameStats::from_static(metadata)).unwrap();
        (
            [&frame[..at], &units, &frame[at..]].concat(),
            units.len() as u64,
        )
    };
    // Frame 0 comes out before its statistics: it carries the stand-in.
    let (expected, added) = with(&Default::default());
    assert_eq!(dynamic.first_unit(frame.clone()), expected);
    assert_eq!(dynamic.cap(Some(5000)), Some(5000 + added));
    // The stream's static metadata rebuilds it.
    let mut metadata = butterpollo_media_policy::hdr::Metadata::display(600., 0.01, 300.);
    metadata.max_cll = 450;
    dynamic.set_metadata(metadata);
    // Nothing finished yet: still the stand-in.
    dynamic.take(Ok(None), Instant::now());
    assert_eq!(dynamic.first_unit(frame.clone()), with(&metadata).0);
    assert_eq!(dynamic.sampled(), 0);
    assert_eq!((dynamic.report.spliced, dynamic.report.stand_in), (2, 2));
    // An invalid first sample keeps it too.
    let mut empty = readout(4, 0, 0);
    empty.histogram.fill(0);
    dynamic.take(Ok(Some(empty)), Instant::now());
    assert_eq!(dynamic.first_unit(frame.clone()), with(&metadata).0);
    // A sample replaces it.
    let sample = readout(5, 200, 600);
    let units = frame_units(Codec::Hevc, &sample.frame_stats().unwrap()).unwrap();
    dynamic.take(Ok(Some(sample)), Instant::now());
    assert_eq!(
        dynamic.first_unit(frame.clone()),
        [&frame[..at], &units, &frame[at..]].concat()
    );
    assert_eq!((dynamic.report.spliced, dynamic.report.stand_in), (4, 3));
}

#[test]
fn a_sample_is_spliced_into_every_frame_until_the_next() {
    let mut dynamic = Dynamic::new(Codec::Hevc);
    let first = readout(7, 200, 600);
    let stats = first.frame_stats().unwrap();
    dynamic.take(Ok(Some(first)), Instant::now());
    assert_eq!(dynamic.sampled(), 7, "the next readout must be newer");
    let units = frame_units(Codec::Hevc, &stats).unwrap();
    let frame = keyframe();
    let at = insert_offset(Codec::Hevc, &frame).unwrap();
    let expected = [&frame[..at], &units, &frame[at..]].concat();
    for _ in 0..3 {
        assert_eq!(dynamic.first_unit(frame.clone()), expected);
        assert_eq!(dynamic.cap(Some(5000)), Some(5000 + units.len() as u64));
        assert_eq!(dynamic.cap(Some(0)), Some(0), "unrestricted stays so");
        assert_eq!(dynamic.cap(None), None);
    }
    // A frame without an insertion point goes out as it is, its cap too.
    let bare = nal(32, &[0x0C]);
    assert_eq!(dynamic.first_unit(bare.clone()), bare);
    assert_eq!(dynamic.cap(Some(5000)), Some(5000));
    assert_eq!(dynamic.report.no_insertion, 1);
    // An empty output is no frame to report.
    assert_eq!(dynamic.first_unit(Vec::new()), Vec::<u8>::new());
    assert_eq!(dynamic.cap(Some(5000)), Some(5000));
    assert_eq!(dynamic.report.no_insertion, 1);
    // A cut to a dark scene is taken as it is.
    let dark = readout(9, 10, 80);
    let dark_stats = dark.frame_stats().unwrap();
    dynamic.take(Ok(Some(dark)), Instant::now());
    let units = frame_units(Codec::Hevc, &dark_stats).unwrap();
    assert_eq!(
        dynamic.first_unit(frame.clone()),
        [&frame[..at], &units, &frame[at..]].concat()
    );
    assert_eq!(dynamic.report.scene_cuts, 2);
}

#[test]
fn a_large_frame_is_spliced_into_its_own_pooled_buffer_unchanged() {
    let _alone = crate::frame_pool::alone();
    let mut dynamic = Dynamic::new(Codec::Hevc);
    dynamic.take(Ok(Some(readout(7, 200, 600))), Instant::now());
    let mut frame = keyframe();
    let body: Vec<u8> = (0..crate::frame_pool::POOLED * 2)
        .map(|i| (i % 251) as u8 | 1)
        .collect();
    frame.extend(nal(1, &body));
    let at = insert_offset(Codec::Hevc, &frame).unwrap();
    let units = frame_units(Codec::Hevc, &readout(7, 200, 600).frame_stats().unwrap()).unwrap();
    let expected = [&frame[..at], &units, &frame[at..]].concat();
    // The unit arrives in a reused pooled buffer, as the whole-frame path
    // copies it: the splice writes into it without leaving any of the
    // bytes it held, and the buffer goes back for the next frame.
    crate::frame_pool::give(vec![0xEE; frame.len() + units.len()]);
    for _ in 0..2 {
        let owned = crate::frame_pool::copy(&frame);
        let spliced = dynamic.first_unit(owned);
        assert_eq!(spliced, expected);
        crate::frame_pool::give(spliced);
    }
}

#[test]
fn a_failed_readout_keeps_the_previous_units() {
    let mut dynamic = Dynamic::new(Codec::Av1);
    dynamic.take(Ok(Some(readout(3, 100, 900))), Instant::now());
    let frame = [
        vec![0x12, 0x00],
        vec![0x0A, 0x01, 0x00],
        vec![0x32, 0x01, 0x10],
    ]
    .concat();
    let before = dynamic.first_unit(frame.clone());
    dynamic.take(Err(anyhow::anyhow!("inconsistent")), Instant::now());
    let mut empty = readout(4, 0, 0);
    empty.histogram.fill(0);
    dynamic.take(Ok(Some(empty)), Instant::now());
    assert_eq!(dynamic.first_unit(frame), before);
    assert_eq!(
        (dynamic.report.readout_errors, dynamic.report.invalid),
        (1, 1)
    );
    assert_eq!(dynamic.sampled(), 4, "an invalid sample is not read again");
}

/// A statistics pass that fails to submit on call `fail_at`.
#[derive(Default)]
struct FakeStats {
    enables: u32,
    enable_fails: bool,
    submits: u32,
    fail_at: Option<u32>,
    stopped: bool,
    readout: Option<HdrStatsReadout>,
}
impl Stats for FakeStats {
    fn enable(&mut self) -> anyhow::Result<()> {
        self.enables += 1;
        if self.enable_fails {
            anyhow::bail!("root signature rejected");
        }
        Ok(())
    }
    fn submit(&mut self) -> anyhow::Result<()> {
        self.submits += 1;
        if self.fail_at == Some(self.submits) {
            anyhow::bail!("command allocator reset failed");
        }
        Ok(())
    }
    fn latest(&self, newer_than: u64) -> anyhow::Result<Option<HdrStatsReadout>> {
        Ok(self
            .readout
            .clone()
            .filter(|readout| readout.conversion > newer_than))
    }
    fn stop(&mut self) {
        self.stopped = true;
    }
}

#[test]
fn a_failed_statistics_pass_turns_dynamic_metadata_off_loudly() {
    let warnings = butterpollo_media_policy::RecordedWarnings::default();
    let mut stats = FakeStats {
        fail_at: Some(2),
        readout: Some(readout(5, 200, 600)),
        ..Default::default()
    };
    let mut slot = Some(Box::new(Dynamic::new(Codec::Hevc)));
    sample(&mut slot, Some(&mut stats), &warnings);
    assert_eq!(slot.as_deref().map(Dynamic::sampled), Some(5));
    assert!(warnings.snapshot().is_empty());
    // The second pass fails: no error reaches the encode, which keeps
    // its frames; the stream says why it lost its dynamic metadata.
    sample(&mut slot, Some(&mut stats), &warnings);
    assert!(slot.is_none(), "dynamic metadata stayed on");
    assert!(stats.stopped, "the converter kept queueing passes");
    let shown = warnings.snapshot();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].code, "hdr_dynamic_metadata");
    assert!(
        shown[0]
            .message
            .contains("GPU statistics pass failed (command allocator reset failed)"),
        "{}",
        shown[0].message
    );
    // Off for the rest of the stream: no more passes.
    sample(&mut slot, Some(&mut stats), &warnings);
    assert_eq!(stats.submits, 2);
}

#[test]
fn a_stream_without_the_statistics_pass_says_why_it_carries_static_hdr10_only() {
    let shown = |stats: Option<&mut FakeStats>| {
        let warnings = butterpollo_media_policy::RecordedWarnings::default();
        let dynamic = start(Wanted::t35(Codec::Hevc), false, stats, &warnings);
        assert!(dynamic.is_none());
        let shown = warnings.snapshot();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].code, "hdr_dynamic_metadata");
        assert!(
            shown[0]
                .message
                .starts_with("HDR10+ and HDR Vivid are off for this stream: ")
                && shown[0]
                    .message
                    .ends_with(". It carries static HDR10 metadata only"),
            "{}",
            shown[0].message
        );
        shown[0].message.clone()
    };
    assert!(shown(None::<&mut FakeStats>).contains("no GPU statistics pass"));
    let mut failing = FakeStats {
        enable_fails: true,
        ..Default::default()
    };
    assert!(
        shown(Some(&mut failing))
            .contains("statistics pass did not start (root signature rejected)")
    );
    // With the pass, dynamic metadata is on and nothing is shown.
    let mut stats = FakeStats::default();
    for codec in [Codec::Av1, Codec::Hevc] {
        let warnings = butterpollo_media_policy::RecordedWarnings::default();
        let dynamic = start(Wanted::t35(codec), false, Some(&mut stats), &warnings);
        assert!(dynamic.is_some_and(|dynamic| dynamic.measuring));
        assert!(warnings.snapshot().is_empty());
    }
    assert_eq!(stats.enables, 2);
}

#[test]
fn packed_444_runs_no_statistics_pass_for_dynamic_metadata() {
    // Its bitstream says SDR and the device unpacks it in its own
    // shader, with the control channel's HDR10 metadata: no decoder
    // would read HDR10+ or HDR Vivid, so the GPU never measures for it.
    let warnings = butterpollo_media_policy::RecordedWarnings::default();
    let mut stats = FakeStats::default();
    for codec in [Codec::Hevc, Codec::Av1] {
        assert!(start(Wanted::t35(codec), true, Some(&mut stats), &warnings).is_none());
    }
    assert_eq!(stats.enables, 0);
    assert!(warnings.snapshot().is_empty());
}

#[test]
fn packed_444_streams_ask_with_their_own_config_not_the_coded_one() {
    // amf.rs codes a packed stream as SDR; `wanted` gets the stream's
    // config, so an HDR packed stream reaches `start` and says why it
    // carries no dynamic metadata.
    let mut config = butterpollo_media_policy::rtsp::Negotiated {
        codec: 1,
        hdr: true,
        chroma_packing: 1,
        ..Default::default()
    };
    let on = butterpollo_config::config::Config::default();
    assert_eq!(wanted(&config, &on), Some(Wanted::t35(Codec::Hevc)));
    config.hdr = false;
    assert_eq!(wanted(&config, &on), None);
}

mod dolby_vision_tests;
