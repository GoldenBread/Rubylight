//! Tests that need a GPU: they encode pictures through the host's [`Encoder`] and check with
//! FFmpeg that the bytes decode. Without a render node (or FFmpeg) they print why and pass. The
//! capture test needs gamescope and is ignored; run it with `cargo test -- --ignored`.
#![allow(clippy::print_stderr)]
use crate::{
    capture::{Capture, CaptureEnv, Image, Pixel},
    encoder::Encoder,
};
use butterpollo_config::config::Config;
use butterpollo_media_policy::rtsp::Negotiated;
use std::{
    path::PathBuf,
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};

/// Warnings the tests do not read.
struct Quiet;
impl butterpollo_media_policy::WarningSink for Quiet {
    fn set(&self, _code: &str, _message: &str) {}
    fn clear(&self, _code: &str) {}
}

fn negotiated(codec: u8, width: u32, height: u32, fps: u32) -> Negotiated {
    Negotiated {
        width,
        height,
        fps,
        bitrate_kbps: 8000,
        codec,
        ..Default::default()
    }
}

/// A picture with a moving gradient and hard edges, so every frame differs from the last.
fn picture(width: u32, height: u32, step: u32) -> Image {
    let mut bytes = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            let moving = (x + step * 8) % 256;
            let block = if (x / 32 + y / 32) % 2 == 0 { 200 } else { 40 };
            bytes.extend_from_slice(&[(y % 256) as u8, moving as u8, block as u8, 255]);
        }
    }
    Image {
        width,
        height,
        stride: width as usize * 4,
        bytes,
        captured: Instant::now(),
        pixel: Pixel::Bgra8,
    }
}

fn skip(reason: &str) {
    eprintln!("skipped: {reason}");
}

fn tool(name: &str) -> bool {
    Command::new(name)
        .arg("-version")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn scratch(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("rubylight-graphics-{}-{name}", std::process::id()))
}

/// The NAL unit types of a byte stream, in order: `type & 0x1f` for H.264, `type >> 1 & 0x3f`
/// for HEVC.
fn nal_types(stream: &[u8], hevc: bool) -> Vec<u8> {
    stream
        .windows(4)
        .enumerate()
        .filter(|(_, w)| w[..3] == [0, 0, 1])
        .map(|(_, w)| if hevc { w[3] >> 1 & 0x3f } else { w[3] & 0x1f })
        .collect()
}

/// The number of pictures FFmpeg decodes from `file` (a raw `format` stream), or its error.
fn decoded_frames(file: &PathBuf, format: &str) -> Result<usize, String> {
    probe_stream(file, format).map(|(_, frames)| frames)
}

/// The size and the number of pictures FFmpeg decodes from `file`.
fn probe_stream(file: &PathBuf, format: &str) -> Result<((u32, u32), usize), String> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-f",
            format,
            "-count_frames",
            "-select_streams",
            "v:0",
        ])
        .args([
            "-show_entries",
            "stream=nb_read_frames,width,height",
            "-of",
            "csv=p=0",
        ])
        .arg(file)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() || !output.stderr.is_empty() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    // "width,height,frames"
    let fields: Vec<u32> = text
        .trim()
        .split(',')
        .filter_map(|field| field.parse().ok())
        .collect();
    match fields[..] {
        [width, height, frames] => Ok(((width, height), frames as usize)),
        _ => Err(format!("unexpected ffprobe output {text:?}")),
    }
}

fn encode_and_decode(codec: u8, format: &str, width: u32, height: u32) {
    if crate::vaapi::node::nodes().is_empty() {
        return skip("no AMD or NVIDIA render node");
    }
    if !tool("ffprobe") {
        return skip("ffprobe is not installed");
    }
    let config = negotiated(codec, width, height, 30);
    let mut encoder = match Encoder::new_options(&config, "", &Config::default()) {
        Ok(encoder) => encoder,
        Err(error) => return skip(&format!("the GPU has no such encoder: {error:#}")),
    };
    assert_eq!(encoder.backend(), "vaapi");
    let mut stream = Vec::new();
    let mut times = Vec::new();
    let frames = 8;
    for step in 0..frames {
        let began = Instant::now();
        let packets = encoder
            .encode(&picture(width, height, step), step == 0, 8000)
            .expect("encode");
        times.push(began.elapsed());
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].idr, step == 0);
        // Parameter sets travel with each IDR picture and only with those.
        let kinds = nal_types(&packets[0].bytes, codec == 1);
        let (sets, idr) = if codec == 1 {
            (&[32, 33, 34][..], 19)
        } else {
            (&[7, 8][..], 5)
        };
        if step == 0 {
            assert!(kinds.starts_with(sets), "{kinds:?}");
            assert!(kinds.contains(&idr), "{kinds:?}");
        } else {
            assert!(sets.iter().all(|set| !kinds.contains(set)), "{kinds:?}");
            assert!(!kinds.contains(&idr), "{kinds:?}");
        }
        stream.extend_from_slice(&packets[0].bytes);
    }
    // An IDR picture on request, and a new bitrate, both part of the stream.
    let packets = encoder
        .encode(&picture(width, height, frames), true, 4000)
        .expect("encode IDR");
    assert!(packets[0].idr);
    stream.extend_from_slice(&packets[0].bytes);
    let file = scratch(&format!("{format}-{width}x{height}.bin"));
    std::fs::write(&file, &stream).unwrap();
    let decoded = probe_stream(&file, format);
    let _ = std::fs::remove_file(&file);
    let mean = times.iter().skip(1).sum::<Duration>() / (times.len() as u32 - 1);
    eprintln!(
        "{format} {width}x{height}: {} bytes, {} frames decoded, mean encode {:.2} ms (first {:.2} ms)",
        stream.len(),
        decoded.as_ref().map_or(0, |d| d.1),
        mean.as_secs_f64() * 1000.0,
        times[0].as_secs_f64() * 1000.0
    );
    let (size, count) = decoded.expect("the stream decodes");
    assert_eq!((size, count), ((width, height), frames as usize + 1));
}

#[test]
fn h264_pictures_decode() {
    encode_and_decode(0, "h264", 640, 480);
}

#[test]
fn hevc_pictures_decode() {
    encode_and_decode(1, "hevc", 640, 480);
}

#[test]
fn pictures_that_are_not_whole_blocks_decode_cropped() {
    encode_and_decode(0, "h264", 1920, 1080);
    encode_and_decode(1, "hevc", 1920, 1080);
}

#[test]
fn unavailable_encodings_fail_visibly() {
    let mut av1 = negotiated(2, 640, 480, 30);
    let error = Encoder::new_options(&av1, "", &Config::default())
        .err()
        .expect("AV1 fails");
    assert!(format!("{error:#}").contains("AV1 encoding is not available"));
    av1.codec = 1;
    av1.hdr = true;
    assert!(Encoder::new_options(&av1, "", &Config::default()).is_err());
    av1.hdr = false;
    av1.yuv444 = true;
    assert!(Encoder::new_options(&av1, "", &Config::default()).is_err());
}

/// Captures a headless gamescope session running a test client, encodes what arrives and checks
/// the stream decodes. Needs gamescope and a client (`glxgears` or `vkcube`).
#[test]
#[ignore = "needs gamescope and a GPU"]
fn gamescope_frames_are_captured_and_encoded() {
    if crate::vaapi::node::nodes().is_empty() || !tool("ffprobe") {
        return skip("no render node or no ffprobe");
    }
    let client = ["glxgears", "vkcube"].into_iter().find(|name| {
        Command::new("which")
            .arg(name)
            .output()
            .is_ok_and(|o| o.status.success())
    });
    let Some(client) = client else {
        return skip("neither glxgears nor vkcube is installed");
    };
    let environment = [
        ("SUNSHINE_CLIENT_WIDTH".to_string(), "1280".to_string()),
        ("SUNSHINE_CLIENT_HEIGHT".to_string(), "720".to_string()),
        ("SUNSHINE_CLIENT_FPS".to_string(), "60".to_string()),
    ]
    .into_iter()
    .collect();
    let (command, folder) = butterpollo_linux_io::gamescope::wrap(client, &environment).unwrap();
    let mut session = Command::new("/bin/sh")
        .args(["-c", &command])
        .spawn()
        .unwrap();

    let result = std::panic::catch_unwind(|| {
        let warnings: Arc<dyn butterpollo_media_policy::WarningSink> = Arc::new(Quiet);
        // Another gamescope session on this PC may be newer than ours: then the session's
        // node is named directly, which is the target a stream gets from `Session::capture_target`.
        let deadline = Instant::now() + Duration::from_secs(10);
        let node = loop {
            if let Some(node) = butterpollo_linux_io::gamescope::read(&folder).and_then(|s| s.node)
            {
                break node;
            }
            assert!(Instant::now() < deadline, "gamescope offered no node");
            std::thread::sleep(Duration::from_millis(50));
        };
        let ours = butterpollo_linux_io::gamescope::current().is_some_and(|s| s.folder == folder);
        let target = if ours {
            "gamescope".to_string()
        } else {
            butterpollo_linux_io::gamescope::capture_target(node)
        };
        eprintln!("capture target: {target}");
        let mut capture = Capture::open_for_stream_with_env(
            &target,
            "pipewire",
            false,
            &Config::default(),
            warnings.clone(),
            &CaptureEnv::default(),
        )
        .expect("open the gamescope capture");
        assert_eq!(capture.backend(), "pipewire");
        let timer = crate::timing::Timer::new().unwrap();
        let (mut encoder, mut stream, mut times, mut count) = (None, Vec::new(), Vec::new(), 0);
        let mut last = None;
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            capture
                .frame_signal()
                .expect("a frame signal")
                .reset()
                .unwrap();
            let Some(image) = capture.next_gpu(0).expect("capture") else {
                let signal = capture.frame_signal().expect("a frame signal");
                timer
                    .until_or_signal(Instant::now() + Duration::from_millis(20), signal)
                    .unwrap();
                continue;
            };
            assert_eq!((image.width, image.height), (1280, 720));
            let encoder = encoder.get_or_insert_with(|| {
                Encoder::new_gpu(&negotiated(1, 1280, 720, 60), &image).expect("GPU encoder")
            });
            let began = Instant::now();
            let packets = encoder
                .encode_gpu(&image, count == 0, 10_000)
                .expect("encode");
            times.push(began.elapsed());
            stream.extend_from_slice(&packets[0].bytes);
            count += 1;
            last = Some(image.clone());
            if count == 30 {
                let readback = image.readback(&mut None).expect("readback");
                assert_eq!(readback.bytes.len(), 1280 * 720 * 4);
                let lit = readback
                    .bytes
                    .chunks(4)
                    .filter(|px| px[..3] != [0, 0, 0])
                    .count();
                eprintln!("readback: {lit} of {} pixels are not black", 1280 * 720);
                assert!(lit > 1000);
            }
        }
        let mean = times.iter().skip(1).sum::<Duration>() / (times.len().max(2) as u32 - 1);
        eprintln!(
            "captured and encoded {count} frames in 3 s; mean encode {:.2} ms; {} bytes",
            mean.as_secs_f64() * 1000.0,
            stream.len()
        );
        assert!(count >= 60, "only {count} frames arrived");
        let file = scratch("gamescope.hevc");
        std::fs::write(&file, &stream).unwrap();
        // For looking at the decoded picture by hand.
        if let Some(keep) = std::env::var_os("RUBYLIGHT_KEEP_STREAM") {
            std::fs::write(keep, &stream).unwrap();
        }
        let decoded = decoded_frames(&file, "hevc");
        let _ = std::fs::remove_file(&file);
        assert_eq!(decoded.expect("the stream decodes"), count);
        // A stream smaller than the capture scales the picture on the GPU.
        let last = last.expect("a frame");
        let mut small =
            Encoder::new_gpu(&negotiated(0, 640, 360, 60), &last).expect("scaling encoder");
        let mut scaled = Vec::new();
        for step in 0..4 {
            scaled.extend_from_slice(
                &small.encode_gpu(&last, step == 0, 4000).expect("scaled")[0].bytes,
            );
        }
        let file = scratch("scaled.h264");
        std::fs::write(&file, &scaled).unwrap();
        let (size, frames) = probe_stream(&file, "h264").expect("the scaled stream decodes");
        let _ = std::fs::remove_file(&file);
        assert_eq!((size, frames), ((640, 360), 4));
        drop(small);
        // When the session ends the capture says so.
        drop(encoder);
        kill_session(&folder);
        let ended = Instant::now() + Duration::from_secs(5);
        loop {
            match capture.next_gpu(0) {
                Err(error) => {
                    assert!(format!("{error:#}").contains("display closed"), "{error:#}");
                    break;
                }
                Ok(_) => assert!(Instant::now() < ended, "the capture never noticed the end"),
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });
    let _ = session.kill();
    let _ = session.wait();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

fn kill_session(folder: &std::path::Path) {
    if let Some(session) = butterpollo_linux_io::gamescope::read(folder) {
        // SAFETY: a signal to the session's gamescope process, which this test started.
        unsafe { libc::kill(session.pid as i32, libc::SIGTERM) };
    }
}
