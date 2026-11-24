//! Tests of the desktop capture. They need a desktop session on a GPU, and the first run shows
//! the portal's dialog, so they are ignored: run them with `cargo test -- --ignored`.
#![allow(clippy::print_stderr)]
use super::{Capture, CaptureEnv, desktop};
use crate::encoder::Encoder;
use butterpollo_config::config::Config;
use butterpollo_media_policy::rtsp::Negotiated;
use std::{
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};

struct Quiet;
impl butterpollo_media_policy::WarningSink for Quiet {
    fn set(&self, _code: &str, _message: &str) {}
    fn clear(&self, _code: &str) {}
}

#[test]
fn targets_parse() {
    assert_eq!(desktop::parse("desktop"), Some(None));
    assert_eq!(desktop::parse("desktop:1"), Some(Some(1)));
    assert_eq!(desktop::parse("desktop:x"), None);
    assert_eq!(desktop::parse("gamescope"), None);
    assert_eq!(desktop::parse("pipewire-node:7"), None);
}

/// Opens the desktop capture, takes 60 frames, encodes them as HEVC and checks FFmpeg decodes
/// them. With no restore token the portal's dialog is waiting for someone to share a monitor.
#[test]
#[ignore = "needs a desktop session, a GPU and an answered portal dialog"]
fn desktop_frames_are_captured_and_encoded() {
    if crate::vaapi::node::nodes().is_empty() {
        eprintln!("skipped: no render node");
        return;
    }
    let warnings: Arc<dyn butterpollo_media_policy::WarningSink> = Arc::new(Quiet);
    let target = std::env::var("RUBYLIGHT_DESKTOP_TARGET").unwrap_or_else(|_| "desktop".into());
    let opened = Instant::now();
    let mut capture = Capture::open_for_stream_with_env(
        &target,
        "pipewire",
        false,
        &Config::default(),
        warnings,
        &CaptureEnv::default(),
    )
    .expect("open the desktop capture");
    eprintln!(
        "opened {target} in {:.2} s; buffer path {}",
        opened.elapsed().as_secs_f64(),
        capture.buffer_path()
    );
    assert_eq!(capture.backend(), "pipewire");
    let timer = crate::timing::Timer::new().unwrap();
    let (mut encoder, mut stream, mut count, mut size) = (None, Vec::new(), 0usize, (0, 0));
    let deadline = Instant::now() + Duration::from_secs(60);
    while count < 60 && Instant::now() < deadline {
        capture.frame_signal().unwrap().reset().unwrap();
        let Some(image) = capture.next_gpu(0).expect("capture") else {
            let signal = capture.frame_signal().unwrap();
            timer
                .until_or_signal(Instant::now() + Duration::from_millis(50), signal)
                .unwrap();
            continue;
        };
        size = (image.width, image.height);
        let encoder = encoder.get_or_insert_with(|| {
            let negotiated = Negotiated {
                width: image.width,
                height: image.height,
                fps: 60,
                bitrate_kbps: 20_000,
                codec: 1,
                ..Default::default()
            };
            Encoder::new_gpu(&negotiated, &image).expect("GPU encoder")
        });
        let packets = encoder
            .encode_gpu(&image, count == 0, 20_000)
            .expect("encode");
        stream.extend_from_slice(&packets[0].bytes);
        count += 1;
        if count == 30 {
            let readback = image.readback(&mut None).expect("readback");
            let lit = readback
                .bytes
                .chunks(4)
                .filter(|px| px[..3] != [0, 0, 0])
                .count();
            eprintln!(
                "readback: {lit} of {} pixels are not black",
                size.0 * size.1
            );
            assert!(lit > 1000, "the captured desktop is black");
        }
    }
    eprintln!(
        "{count} frames of {}x{} in {:.1} s via {}; {} bytes",
        size.0,
        size.1,
        opened.elapsed().as_secs_f64(),
        capture.buffer_path(),
        stream.len()
    );
    assert_eq!(count, 60, "only {count} frames arrived");
    let file = std::env::temp_dir().join(format!("rubylight-desktop-{}.hevc", std::process::id()));
    std::fs::write(&file, &stream).unwrap();
    if let Some(keep) = std::env::var_os("RUBYLIGHT_KEEP_STREAM") {
        std::fs::write(keep, &stream).unwrap();
    }
    let probe = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-f",
            "hevc",
            "-count_frames",
            "-select_streams",
            "v:0",
        ])
        .args(["-show_entries", "stream=nb_read_frames", "-of", "csv=p=0"])
        .arg(&file)
        .output();
    let _ = std::fs::remove_file(&file);
    match probe {
        Ok(out) if out.status.success() => {
            let frames: usize = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
            assert_eq!(frames, 60, "FFmpeg decoded {frames} frames");
        }
        _ => eprintln!("ffprobe is not available: the stream was not decoded"),
    }
}

/// The memory path: a GStreamer test pattern offered on PipeWire as a video source arrives in
/// memory buffers, is uploaded and converted, and comes out as pictures the encoder takes.
#[test]
#[ignore = "needs a GPU and gst-launch-1.0 with the PipeWire plugin"]
fn memory_buffers_are_uploaded_and_encoded() {
    use super::pw_capture::{Options, PipeWireCapture};
    let Some(node) = crate::vaapi::node::nodes().first().cloned() else {
        eprintln!("skipped: no render node");
        return;
    };
    let va = Arc::new(crate::vaapi::Va::open(&node.path).expect("VA-API"));
    let mut source = Command::new("gst-launch-1.0")
        .args([
            "-q",
            "videotestsrc",
            "is-live=true",
            "pattern=ball",
            "!",
            "video/x-raw,format=BGRx,width=1280,height=720,framerate=60/1",
            "!",
            "pipewiresink",
            "mode=provide",
            "stream-properties=props,media.class=Video/Source,node.name=rubylight-test-source",
        ])
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("gst-launch-1.0");
    let result = std::panic::catch_unwind(|| {
        let id = (0..50)
            .find_map(|_| {
                std::thread::sleep(Duration::from_millis(100));
                let dump = Command::new("pw-dump").output().ok()?;
                let json: serde_json::Value = serde_json::from_slice(&dump.stdout).ok()?;
                json.as_array()?.iter().find_map(|object| {
                    (object["info"]["props"]["node.name"] == "rubylight-test-source")
                        .then(|| object["id"].as_u64())
                        .flatten()
                })
            })
            .expect("the test source never appeared on PipeWire");
        let options = Options {
            desktop: true,
            shm: true,
            ..Options::default()
        };
        let capture = PipeWireCapture::open_with(id as u32, None, &va, options)
            .expect("open the memory capture");
        assert_eq!(capture.size, (1280, 720));
        let mut count = 0;
        let mut bytes = 0;
        let mut encoder = None;
        let deadline = Instant::now() + Duration::from_secs(10);
        while count < 60 && Instant::now() < deadline {
            let Some(frame) = capture.take().expect("capture") else {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            };
            let image = super::GpuImage {
                width: 1280,
                height: 720,
                pixel: super::Pixel::Bgra8,
                captured: frame.captured,
                wgc_stamp: None,
                acquired: frame.acquired,
                published: frame.published,
                gpu: super::Device::on(va.clone(), &node, "memory-test", (1280, 720)),
                focus: None,
                repeat: false,
                origin: 0,
                surface: Some(frame.surface),
            };
            let image = Arc::new(image);
            let encoder = encoder.get_or_insert_with(|| {
                let negotiated = Negotiated {
                    width: 1280,
                    height: 720,
                    fps: 60,
                    bitrate_kbps: 8000,
                    codec: 1,
                    ..Default::default()
                };
                Encoder::new_gpu(&negotiated, &image).expect("GPU encoder")
            });
            bytes += encoder
                .encode_gpu(&image, count == 0, 8000)
                .expect("encode")[0]
                .bytes
                .len();
            if count == 30 {
                let readback = image.readback(&mut None).expect("readback");
                let lit = readback
                    .bytes
                    .chunks(4)
                    .filter(|px| px[..3] != [0, 0, 0])
                    .count();
                assert!(lit > 1000, "the uploaded picture is black");
            }
            count += 1;
        }
        eprintln!(
            "memory path: {count} frames, {bytes} encoded bytes, path {}",
            capture.path()
        );
        assert_eq!(capture.path(), "shm");
        assert_eq!(count, 60);
    });
    let _ = source.kill();
    let _ = source.wait();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

/// The desktop's DMA-BUF offers (RGB formats only, as a compositor makes them) against a
/// producer that has DMA-BUFs: a headless gamescope session running a test client.
#[test]
#[ignore = "needs gamescope, a client and a GPU"]
fn desktop_offers_take_dmabufs() {
    use super::pw_capture::{Options, PipeWireCapture};
    let Some(node) = crate::vaapi::node::nodes().first().cloned() else {
        eprintln!("skipped: no render node");
        return;
    };
    let va = Arc::new(crate::vaapi::Va::open(&node.path).expect("VA-API"));
    let environment = [
        ("SUNSHINE_CLIENT_WIDTH".to_string(), "1280".to_string()),
        ("SUNSHINE_CLIENT_HEIGHT".to_string(), "720".to_string()),
        ("SUNSHINE_CLIENT_FPS".to_string(), "60".to_string()),
    ]
    .into_iter()
    .collect();
    let (command, folder) =
        butterpollo_linux_io::gamescope::wrap("glxgears", &environment).expect("gamescope command");
    let mut session = Command::new("/bin/sh")
        .args(["-c", &command])
        .spawn()
        .unwrap();
    let result = std::panic::catch_unwind(|| {
        let session = butterpollo_linux_io::gamescope::wait_for_node(Duration::from_secs(10))
            .expect("the gamescope session");
        let options = Options {
            desktop: true,
            shm: false,
            ..Options::default()
        };
        let capture = PipeWireCapture::open_with(session.node.expect("a node"), None, &va, options)
            .expect("open the DMA-BUF capture");
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut count = 0;
        while count < 30 && Instant::now() < deadline {
            match capture.take().expect("capture") {
                Some(_) => count += 1,
                None => std::thread::sleep(Duration::from_millis(5)),
            }
        }
        eprintln!("DMA-BUF path: {count} frames, path {}", capture.path());
        assert_eq!(capture.path(), "dmabuf");
        assert_eq!(count, 30);
    });
    let _ = session.kill();
    let _ = session.wait();
    if let Some(record) = butterpollo_linux_io::gamescope::read(&folder) {
        // SAFETY: a signal to the gamescope process this test started.
        unsafe { libc::kill(record.pid as i32, libc::SIGTERM) };
    }
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[test]
fn displays_are_numbered_from_the_primary() {
    let displays = desktop::displays();
    for (index, display) in displays.iter().enumerate() {
        assert_eq!(display.display_name, format!("desktop:{index}"));
        assert_eq!(display.primary, index == 0);
        assert!(display.width > 0 && display.height > 0);
        assert!(desktop::present(&display.display_name));
    }
    assert!(!desktop::present("desktop:99"));
    assert!(!desktop::present("gamescope"));
}
