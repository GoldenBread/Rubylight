//! Unit tests for capture target switching, snapshot backpressure, adapter filtering,
//! timestamp conversion, frame-pool selection, and backend fallback policy.

use super::*;
use butterpollo_media_policy::RecordedWarnings;

#[test]
fn a_target_change_on_the_same_display_keeps_the_capture_device() {
    let display = r"\\.\DISPLAY3";
    // The app's window found, minimised and restored on the stream's display.
    assert!(switches_in_place(
        display, display, true, "wgc", false, false
    ));
    assert!(switches_in_place(
        display, display, false, "wgc", false, false
    ));
    assert!(switches_in_place(
        display,
        r"\\.\display3",
        false,
        "auto",
        false,
        false
    ));
    // A window opens with WGC whatever the display's backend.
    assert!(switches_in_place(
        display, display, true, "ddx", false, false
    ));
    // Everything else gets a capture on a new device.
    assert!(!switches_in_place(
        display,
        r"\\.\DISPLAY4",
        true,
        "wgc",
        false,
        false
    ));
    assert!(!switches_in_place("", "", false, "wgc", false, false));
    assert!(
        !switches_in_place(display, display, true, "wgc", true, false),
        "WGC helper"
    );
    for kind in ["ddx", "dxgi", "amd"] {
        assert!(
            !switches_in_place(display, display, false, kind, false, false),
            "{kind}"
        );
    }
    assert!(
        !switches_in_place(display, display, false, "wgc", false, true),
        "frame push"
    );
}

#[test]
fn snapshot_backpressure_keeps_final_native_update_until_consumers_release_space() -> Result<()> {
    use std::{collections::VecDeque, sync::Arc};
    // Shared capture supports more consumers than the eight snapshot slots.
    // Model consumers holding distinct earlier frames while their senders lag.
    let textures: Vec<_> = (0..GPU_SNAPSHOT_CAPACITY).map(Arc::new).collect();
    let mut consumers = textures.clone();
    let mut native = VecDeque::from([GPU_SNAPSHOT_CAPACITY]);
    let mut acquisitions = 0;
    for _ in 0..3 {
        let image = acquire_snapshot_frame(snapshot_capacity(&textures), || {
            acquisitions += 1;
            Ok(native.pop_front())
        })?;
        assert_eq!(image, None);
    }
    assert_eq!(acquisitions, 0);
    assert_eq!(native.len(), 1);

    // There is no subsequent desktop event to rescue a discarded final frame.
    consumers.pop();
    let image = acquire_snapshot_frame(snapshot_capacity(&textures), || {
        acquisitions += 1;
        Ok(native.pop_front())
    })?;
    assert_eq!(image, Some(GPU_SNAPSHOT_CAPACITY));
    assert_eq!(acquisitions, 1);
    assert!(native.is_empty());
    assert_eq!(textures.len(), GPU_SNAPSHOT_CAPACITY);
    assert_eq!(
        consumers.iter().map(|image| **image).collect::<Vec<_>>(),
        (0..GPU_SNAPSHOT_CAPACITY - 1).collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn pending_wgc_snapshot_keeps_only_the_newest_update_and_its_timestamp() -> Result<()> {
    use std::sync::Arc;
    let textures: Vec<_> = (0..GPU_SNAPSHOT_CAPACITY).map(Arc::new).collect();
    let mut consumers = textures.clone();
    let first = Instant::now();
    let last = first + Duration::from_millis(8);
    let mut held = Some((1, first));
    assert!(acquire_snapshot_frame(snapshot_capacity(&textures), || Ok(held.take()))?.is_none());
    assert_eq!(held.replace((2, last)), Some((1, first)));
    assert!(acquire_snapshot_frame(snapshot_capacity(&textures), || Ok(held.take()))?.is_none());
    consumers.pop();
    assert_eq!(
        acquire_snapshot_frame(snapshot_capacity(&textures), || Ok(held.take()))?,
        Some((2, last))
    );
    assert!(held.is_none());
    Ok(())
}

#[test]
fn snapshot_capacity_can_reclaim_the_desktop_cache_without_overwriting_consumers() {
    use std::sync::Arc;
    let textures: Vec<_> = (0..GPU_SNAPSHOT_CAPACITY).map(Arc::new).collect();
    let consumers = textures[..GPU_SNAPSHOT_CAPACITY - 1].to_vec();
    let mut cached = Some(textures.last().unwrap().clone());
    assert!(!snapshot_capacity(&textures));
    // DDX may be the only remaining reader of its cached desktop.
    cached.take();
    assert!(snapshot_capacity(&textures));
    assert!(consumers.iter().all(|image| Arc::strong_count(image) == 2));
    assert!(snapshot_capacity(&textures[..GPU_SNAPSHOT_CAPACITY - 1]));
}

#[test]
fn available_snapshot_does_not_swallow_native_acquisition_failures() {
    let error = acquire_snapshot_frame::<()>(true, || bail!("capture access lost")).unwrap_err();
    assert_eq!(error.to_string(), "capture access lost");
}

#[test]
fn snapshot_backpressure_uses_the_normal_poll_instead_of_an_expired_deadline() {
    let now = Instant::now();
    let expired = now - Duration::from_millis(8);
    let poll = now + Duration::from_micros(500);
    // This is the capture worker's deadline selection. Holding the last WGC
    // update must not turn bounded backpressure into a busy loop.
    let blocked = snapshot_publication_deadline(false, Some(expired));
    assert_eq!(blocked.unwrap_or(poll).min(poll), poll);
    assert_eq!(
        snapshot_publication_deadline(true, Some(expired)),
        Some(expired)
    );
    assert_eq!(snapshot_publication_deadline(true, None), None);
}

#[test]
fn basic_render_driver_is_not_hardware_even_without_the_software_flag() {
    for flags in [0, DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32] {
        let basic = DXGI_ADAPTER_DESC1 {
            VendorId: 0x1414,
            DeviceId: 0x8c,
            Flags: flags,
            ..Default::default()
        };
        assert!(!hardware_adapter(&basic));
        let radeon = DXGI_ADAPTER_DESC1 {
            VendorId: 0x1002,
            ..Default::default()
        };
        let adapters = [basic, radeon];
        assert_eq!(adapters.iter().position(hardware_adapter), Some(1));
        assert!(!adapters[..1].iter().any(hardware_adapter));
    }
}

#[test]
fn hardware_adapter_selection_keeps_other_devices_and_rejects_software() {
    for (vendor, device) in [
        (0x1002, 0x7550),
        (0x10de, 0x2684),
        (0x8086, 0x56a0),
        (0x1414, 0x1234),
    ] {
        let mut desc = DXGI_ADAPTER_DESC1 {
            VendorId: vendor,
            DeviceId: device,
            ..Default::default()
        };
        assert!(hardware_adapter(&desc));
        desc.Flags = DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32;
        assert!(!hardware_adapter(&desc));
    }
}

#[test]
#[ignore = "requires an AMD GPU with AMF H.264, HEVC and AV1 encoding"]
fn headless_hardware_device_encodes_without_a_display_output() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::without_display("", "")?;
    // A PC whose bigger card is NVIDIA's encodes there, not with AMF.
    if !encoder_gpu::is_amd(gpu.vendor) {
        eprintln!(
            "skipped: without a display this PC encodes on {}, not an AMD GPU",
            gpu.display.adapter
        );
        return Ok(());
    }
    assert!(gpu.output.is_none());
    assert!(gpu.display.display_name.is_empty());
    let image = Image {
        width: 640,
        height: 480,
        stride: 2560,
        bytes: vec![128; 640 * 480 * 4],
        captured: Instant::now(),
        pixel: Pixel::Bgra8,
    };
    for codec in 0..=2 {
        let config = butterpollo_media_policy::rtsp::Negotiated {
            width: image.width,
            height: image.height,
            fps: 30,
            bitrate_kbps: 2000,
            codec,
            ..Default::default()
        };
        let mut encoder = crate::amf::Encoder::new_device(&config, gpu.clone())?;
        let mut encoded = false;
        for frame in 0..8 {
            if !encoder
                .encode(&image, frame == 0, config.bitrate_kbps)?
                .is_empty()
            {
                encoded = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            encoded,
            "codec {codec} produced no packet without an output"
        );
        eprintln!(
            "{}: codec {codec} encoded without an output",
            gpu.display.adapter
        );
    }
    Ok(())
}

#[test]
fn desktop_duplication_refuses_a_turned_display_rather_than_stream_it_sideways() {
    for upright in [DXGI_MODE_ROTATION_UNSPECIFIED, DXGI_MODE_ROTATION_IDENTITY] {
        duplication_upright(upright, r"\\.\DISPLAY1").unwrap();
    }
    for turned in [
        DXGI_MODE_ROTATION_ROTATE90,
        DXGI_MODE_ROTATION_ROTATE180,
        DXGI_MODE_ROTATION_ROTATE270,
    ] {
        let error = duplication_upright(turned, r"\\.\DISPLAY1")
            .unwrap_err()
            .to_string();
        assert!(error.contains(r"\\.\DISPLAY1 is turned"), "{error}");
        assert!(error.contains("capture = auto or wgc"), "{error}");
    }
}

#[test]
fn a_missing_selected_capture_display_is_an_error_not_the_desktop() {
    let display = |name: &str, primary| Display {
        device_id: format!("id-{name}"),
        display_name: name.into(),
        friendly_name: name.into(),
        width: 2560,
        height: 1440,
        x: 0,
        y: 0,
        primary,
        adapter: String::new(),
        adapter_index: 0,
        output_index: 0,
    };
    let choices = [display("virtual", false), display("physical", true)];
    assert_eq!(
        select_display(&choices, "virtual")
            .unwrap()
            .unwrap()
            .display_name,
        "virtual"
    );
    assert_eq!(
        select_display(&choices, "id-virtual")
            .unwrap()
            .unwrap()
            .display_name,
        "virtual"
    );
    assert!(select_display(&choices[1..], "virtual").is_err());
    assert!(select_display(&[], "virtual").is_err());
    assert_eq!(
        select_display(&choices, "").unwrap().unwrap().display_name,
        "physical"
    );
    assert_eq!(
        select_display(&choices[..1], "")
            .unwrap()
            .unwrap()
            .display_name,
        "virtual"
    );
    assert!(select_display(&[], "").unwrap().is_none());
}

#[test]
fn capture_timestamps_preserve_past_and_future_stamps_without_changing_pacing() {
    let now = Instant::now();
    let frequency = 10_000_000;
    let current = 100_000_000;
    for (offset, expected) in [
        (-10_000, now - Duration::from_millis(1)),
        (0, now),
        (7_900, now + Duration::from_micros(790)),
    ] {
        let (captured, stamp) = qpc_timestamps_at(current + offset, current, frequency, now);
        assert_eq!(stamp, Some(expected));
        assert_eq!(captured, expected.min(now));
    }
}

#[test]
fn capture_timestamps_keep_old_stamp_diagnostics_and_the_two_second_pacing_cutoff() {
    let now = Instant::now();
    for (age, clamped) in [(20_000_000, false), (20_000_001, true), (30_000_000, true)] {
        let expected = now - Duration::from_nanos(age as u64 * 100);
        let (captured, stamp) = qpc_timestamps_at(100_000_000 - age, 100_000_000, 10_000_000, now);
        assert_eq!(stamp, Some(expected));
        assert_eq!(captured, if clamped { now } else { expected });
    }
}

#[test]
fn capture_timestamps_do_not_report_missing_or_invalid_stamps_as_zero_delay() {
    let now = Instant::now();
    for (ticks, current, frequency) in [(0, 100, 10), (-1, 100, 10), (1, 0, 10), (1, 100, 0)] {
        assert_eq!(
            qpc_timestamps_at(ticks, current, frequency, now),
            (now, None)
        );
    }
}

#[test]
fn wgc_ticks_convert_from_100ns_on_other_qpc_frequencies() {
    let now = Instant::now();
    let frequency = 24_000_000;
    assert_eq!(wgc_qpc(10_000_000, frequency), frequency);
    assert_eq!(wgc_qpc(0, frequency), 0);
    assert_eq!(wgc_qpc(-1, frequency), 0);
    assert_eq!(wgc_qpc(i64::MAX, frequency), i64::MAX);
    let stamp = now - Duration::from_micros(125);
    assert_eq!(
        qpc_timestamps_at(wgc_qpc(9_998_750, frequency), frequency, frequency, now),
        (stamp, Some(stamp))
    );
}

#[test]
fn newest_frame_selection_is_bounded_and_preserves_lone_static_updates() -> Result<()> {
    for limit in [1, WGC_FRAME_POOL_CAPACITY] {
        for frames in [vec![], vec![1], vec![1, 2, 3]] {
            let mut pending = std::collections::VecDeque::from(frames.clone());
            let mut acquired = 0;
            let mut checked = Vec::new();
            let mut closed = Vec::new();
            let newest = newest_capture_frame(
                limit,
                || {
                    acquired += 1;
                    Ok(pending.pop_front())
                },
                |frame| {
                    checked.push(*frame);
                    Ok(())
                },
                |frame| {
                    closed.push(frame);
                    Ok(())
                },
            )?;
            let consumed = frames.len().min(limit);
            assert_eq!(acquired, (frames.len() + 1).min(limit));
            assert_eq!(newest, frames[..consumed].last().copied());
            assert_eq!(checked, frames[..consumed]);
            assert_eq!(closed, frames[..consumed.saturating_sub(1)]);
            assert_eq!(pending.into_iter().collect::<Vec<_>>(), frames[consumed..]);
        }
    }
    Ok(())
}

#[test]
fn newest_frame_selection_closes_every_acquired_frame_on_resize() {
    for invalid in [1, 2] {
        let mut acquired = 0;
        let mut closed = Vec::new();
        let error = newest_capture_frame(
            WGC_FRAME_POOL_CAPACITY,
            || {
                acquired += 1;
                Ok(Some(acquired))
            },
            |frame| {
                if *frame == invalid {
                    bail!("capture output dimensions changed");
                }
                Ok(())
            },
            |frame| {
                closed.push(frame);
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "capture output dimensions changed");
        assert_eq!(acquired, invalid);
        closed.sort_unstable();
        assert_eq!(closed, (1..=invalid).collect::<Vec<_>>());
    }
}

#[test]
fn newest_frame_selection_preserves_pool_errors_and_closes_a_selected_frame() {
    let mut acquired = 0;
    let mut closed = Vec::new();
    let error = newest_capture_frame(
        WGC_FRAME_POOL_CAPACITY,
        || {
            acquired += 1;
            if acquired == 2 {
                bail!("capture pool lost access");
            }
            Ok(Some(acquired))
        },
        |_| Ok(()),
        |frame| {
            closed.push(frame);
            bail!("frame also failed to close");
        },
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "capture pool lost access");
    assert_eq!(acquired, 2);
    assert_eq!(closed, [1]);
}

#[test]
fn newest_frame_selection_closes_the_replacement_when_superseded_close_fails() {
    let mut acquired = 0;
    let mut closed = Vec::new();
    let error = newest_capture_frame(
        WGC_FRAME_POOL_CAPACITY,
        || {
            acquired += 1;
            Ok(Some(acquired))
        },
        |_| Ok(()),
        |frame| {
            closed.push(frame);
            if frame == 1 {
                bail!("superseded frame failed to close");
            }
            Ok(())
        },
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "superseded frame failed to close");
    assert_eq!(acquired, 2);
    assert_eq!(closed, [1, 2]);
}

#[test]
fn wgc_startup_failure_uses_the_same_fallback_as_recovery() {
    let policy = butterpollo_media_policy::framegen::Policy::resolve(
        &Default::default(),
        butterpollo_media_policy::framegen::Rate(60000),
        false,
        "none",
        false,
        false,
    )
    .unwrap();
    let mut attempted = Vec::new();
    let warnings = butterpollo_media_policy::RecordedWarnings::default();
    let capture = open_stream_capture(&policy.capture, &warnings, |kind| {
        attempted.push(kind.to_owned());
        match kind {
            "wgc" => bail!("CreateForMonitor: 0x80070424"),
            "ddx" => Ok("working duplication"),
            _ => unreachable!(),
        }
    })
    .unwrap();
    assert_eq!(capture, "working duplication");
    assert_eq!(attempted, ["wgc", "ddx"]);
    let warning = &warnings.snapshot()[0];
    assert_eq!(warning.code, "capture_backend");
    assert!(warning.message.contains("0x80070424"));
    assert!(warning.message.contains("Desktop Duplication"));
    open_stream_capture("wgc", &warnings, |_| Ok(())).unwrap();
    assert!(warnings.snapshot().is_empty());
    open_stream_capture("auto", &warnings, |_| {
        open_stream_capture("wgc", &warnings, |kind| {
            if kind == "wgc" {
                bail!("helper unavailable");
            }
            Ok(())
        })
    })
    .unwrap();
    assert_eq!(warnings.snapshot()[0].code, "capture_backend");

    let error = open_stream_capture::<()>("wgc", &warnings, |kind| match kind {
        "wgc" => bail!("CreateForMonitor: 0x80070424"),
        _ => bail!("DuplicateOutput: access denied"),
    })
    .unwrap_err();
    let detail = format!("{error:#}");
    assert!(detail.contains("0x80070424"));
    assert!(detail.contains("DuplicateOutput: access denied"));
}

#[test]
fn working_wgc_and_explicit_ddx_do_not_open_another_backend() {
    let w = RecordedWarnings::default();
    for kind in ["wgc", "ddx", "dxgi"] {
        let mut attempts = 0;
        open_stream_capture(kind, &w, |backend| {
            attempts += 1;
            assert_eq!(backend, kind);
            Ok(())
        })
        .unwrap();
        assert_eq!(attempts, 1);
    }
    for kind in ["ddx", "dxgi"] {
        let mut attempts = 0;
        let error = open_stream_capture::<()>(kind, &w, |_| {
            attempts += 1;
            bail!("duplication lost access")
        })
        .unwrap_err();
        assert_eq!(attempts, 1);
        assert_eq!(error.to_string(), "duplication lost access");
    }
}

mod gpu_selection_tests;
mod native_tests;
