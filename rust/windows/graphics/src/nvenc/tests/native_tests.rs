//! Hardware-gated (`#[ignore]`) NVENC native decode and 4:4:4 chroma tests.

use super::hdr_tests::check_decoded_hdr_metadata;
use super::*;

#[test]
#[ignore = "requires explicitly selected NVIDIA hardware and independent FFmpeg decoding"]
fn native_nvenc_loss_recovery_and_444_hdr_decode() -> Result<()> {
    use std::os::windows::process::CommandExt;
    if std::env::var("BUTTERPOLLO_TEST_NVENC").as_deref() != Ok("1") {
        bail!("set BUTTERPOLLO_TEST_NVENC=1 on an NVIDIA test machine");
    }
    let decoder =
        std::env::var_os("BUTTERPOLLO_TEST_FFMPEG").context("set BUTTERPOLLO_TEST_FFMPEG")?;
    let report = std::path::PathBuf::from(
        std::env::var_os("BUTTERPOLLO_TEST_NVENC_REPORT")
            .context("set BUTTERPOLLO_TEST_NVENC_REPORT")?,
    );
    let directory = report
        .parent()
        .context("report requires a parent directory")?;
    std::fs::create_dir_all(directory)?;
    let _com = crate::capture::ComGuard::new()?;
    let gpu = Device::new(&std::env::var("BUTTERPOLLO_TEST_NVENC_DISPLAY").unwrap_or_default())?;
    if !gpu.display.adapter.to_ascii_lowercase().contains("nvidia") {
        bail!("select an NVIDIA-attached output with BUTTERPOLLO_TEST_NVENC_DISPLAY");
    }
    let cases =
        std::env::var("BUTTERPOLLO_TEST_NVENC_CASES").unwrap_or_else(|_| "h264,hevc-hdr".into());
    let mut reports = Vec::new();
    for name in cases.split(',').map(str::trim) {
        let (codec, hdr, yuv444) = match name {
            "h264" => (0, false, false),
            "hevc-hdr" => (1, true, false),
            "av1-hdr" => (2, true, false),
            // Native 4:4:4: AYUV for 8-bit, the CUDA copy for 10-bit. AV1
            // 4:4:4 is refused (`refuse_native_444`).
            "h264-444" => (0, false, true),
            "hevc-444" => (1, false, true),
            "hevc-444-hdr" => (1, true, true),
            _ => bail!("unknown NVIDIA test case {name}"),
        };
        let config = Negotiated {
            width: 640,
            height: 480,
            codec,
            hdr,
            yuv444,
            bitrate_kbps: 20000,
            ..Default::default()
        };
        let mut encoder = Encoder::new_device_options(&config, gpu.clone(), &Config::default())?;
        let metadata_embedded = hdr && encoder.session.api >= ApiVersion(13, 0);
        let first_metadata = butterpollo_media_policy::hdr::Metadata {
            minimum: 20,
            max_cll: 900,
            max_fall: 400,
            ..Default::default()
        };
        let second_metadata = butterpollo_media_policy::hdr::Metadata {
            maximum_nits: 1400,
            minimum: 10,
            max_cll: 1300,
            max_fall: 800,
            ..first_metadata
        };
        encoder.set_hdr_metadata(first_metadata);
        let supported = encoder.supports_invalidation();
        let mut packets = Vec::new();
        for frame in 1..=64 {
            if frame == 40 {
                encoder.set_hdr_metadata(second_metadata);
            }
            if supported && matches!(frame, 7 | 19) {
                assert!(encoder.invalidate_ref_frames(frame - 2, frame - 1));
            }
            let stride = if hdr { 640 * 8 } else { 640 * 4 };
            let mut image = Image {
                width: 640,
                height: 480,
                stride,
                bytes: vec![0; stride * 480],
                pixel: if hdr { Pixel::RgbaF16 } else { Pixel::Bgra8 },
                captured: Instant::now(),
            };
            for (index, pixel) in image
                .bytes
                .chunks_exact_mut(if hdr { 8 } else { 4 })
                .enumerate()
            {
                if hdr {
                    for channel in 0..3 {
                        let value =
                            ((index / 640 + frame as usize * 3 + channel * 11) % 127) as f32 / 10.;
                        pixel[channel * 2..channel * 2 + 2]
                            .copy_from_slice(&half::f16::from_f32(value).to_bits().to_le_bytes());
                    }
                    pixel[6..].copy_from_slice(&half::f16::ONE.to_bits().to_le_bytes());
                } else {
                    pixel.copy_from_slice(&[
                        (index / 640) as u8,
                        (index + frame as usize * 3) as u8,
                        frame as u8,
                        255,
                    ]);
                }
            }
            packets.extend(encoder.encode(
                &image,
                frame == 1,
                if frame >= 33 { 25000 } else { 20000 },
            )?);
            let deadline = Instant::now() + Duration::from_secs(2);
            while encoder.pending() {
                packets.extend(encoder.poll()?);
                if Instant::now() >= deadline {
                    bail!("NVIDIA output fixture timed out");
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            assert_eq!(packets.len(), frame as usize);
        }
        if supported {
            assert!(packets[6].after_invalidation);
            assert!(packets[18].after_invalidation);
        }
        let format = match codec {
            0 => "h264",
            1 => "hevc",
            _ => "obu",
        };
        let path = directory.join(format!("nvenc-{}-{name}.{format}", std::process::id()));
        let bytes: Vec<_> = packets
            .iter()
            .enumerate()
            .filter(|(index, _)| !supported || !matches!(*index, 4 | 5 | 16 | 17))
            .flat_map(|(_, packet)| packet.bytes.iter().copied())
            .collect();
        std::fs::write(&path, bytes)?;
        let ffprobe = std::env::var_os("BUTTERPOLLO_TEST_FFPROBE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(&decoder).with_file_name("ffprobe.exe"));
        let probe = std::process::Command::new(ffprobe)
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_frames",
                "-show_entries",
                "stream=codec_name,width,height,pix_fmt,color_transfer,color_primaries,color_space:frame=key_frame:frame_side_data",
                "-of",
                "json",
            ])
            .arg(&path)
            .creation_flags(0x08000000)
            .output()?;
        assert!(probe.status.success() && probe.stderr.is_empty());
        let info: serde_json::Value = serde_json::from_slice(&probe.stdout)?;
        let stream = &info["streams"][0];
        assert_eq!(stream["width"], 640);
        assert_eq!(stream["height"], 480);
        assert_eq!(
            stream["codec_name"],
            match codec {
                0 => "h264",
                1 => "hevc",
                _ => "av1",
            }
        );
        assert_eq!(
            stream["pix_fmt"],
            match (yuv444, hdr) {
                (false, false) => "yuv420p",
                (false, true) => "yuv420p10le",
                (true, false) => "yuv444p",
                (true, true) => "yuv444p10le",
            }
        );
        if hdr {
            assert_eq!(stream["color_transfer"], "smpte2084");
            assert_eq!(stream["color_primaries"], "bt2020");
            assert_eq!(stream["color_space"], "bt2020nc");
        }
        if metadata_embedded {
            assert!(
                packets[39].idr,
                "metadata change did not request a keyframe"
            );
            let frames = info["frames"]
                .as_array()
                .context("ffprobe returned no decoded frames")?;
            check_decoded_hdr_metadata(
                frames.first().context("no first frame")?,
                first_metadata,
                codec,
            )?;
            check_decoded_hdr_metadata(
                frames.last().context("no last frame")?,
                second_metadata,
                codec,
            )?;
        }
        let output = std::process::Command::new(&decoder)
            .args([
                "-hide_banner",
                "-nostdin",
                "-v",
                "error",
                "-xerror",
                "-err_detect",
                "explode",
                "-f",
                format,
                "-i",
            ])
            .arg(&path)
            .args([
                "-an",
                "-progress",
                "pipe:1",
                "-nostats",
                "-fps_mode",
                "passthrough",
                "-f",
                "null",
                "-",
            ])
            .creation_flags(0x08000000)
            .output()?;
        assert!(
            output.status.success() && output.stderr.is_empty(),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let decoded = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.strip_prefix("frame=")?.trim().parse::<u32>().ok())
            .next_back()
            .context("decoder reported no frames")?;
        assert_eq!(decoded, if supported { 60 } else { 64 });
        reports.push(serde_json::json!({"case":name,"codec":codec,"hdr":hdr,"yuv444":yuv444,"decoded":decoded,
            "native_rfi":supported,"recovery_frames":packets.iter().filter(|p|p.after_invalidation).count(),"bitrate_reconfigured":true,
            "hdr_metadata_before_and_after_change_verified":metadata_embedded}));
    }
    std::fs::write(report, serde_json::to_vec_pretty(&reports)?)?;
    Ok(())
}
/// Native 4:4:4 end to end on an RTX card: a one-pixel red and blue
/// checkerboard, encoded through the real conversion and session and decoded
/// by FFmpeg, keeps its chroma at full resolution. 4:2:0 would average each
/// pair to the same chroma, so the two sites' means would meet.
///
/// Run on an NVIDIA test machine with `BUTTERPOLLO_TEST_NVENC=1`,
/// `BUTTERPOLLO_TEST_FFMPEG` (ffmpeg.exe) and, when NVIDIA does not drive the
/// first output, `BUTTERPOLLO_TEST_NVENC_DISPLAY`. The decoded pictures stay
/// in the temporary folder the failure names.
#[test]
#[ignore = "requires an NVIDIA GPU whose NVENC reports 4:4:4, and FFmpeg"]
fn native_444_keeps_a_one_pixel_chroma_checkerboard() -> Result<()> {
    use std::os::windows::process::CommandExt;
    if std::env::var("BUTTERPOLLO_TEST_NVENC").as_deref() != Ok("1") {
        bail!("set BUTTERPOLLO_TEST_NVENC=1 on an NVIDIA test machine");
    }
    let decoder =
        std::env::var_os("BUTTERPOLLO_TEST_FFMPEG").context("set BUTTERPOLLO_TEST_FFMPEG")?;
    let id = std::process::id();
    let directory = std::env::temp_dir().join(format!("nvenc-444-{id}"));
    std::fs::create_dir_all(&directory)?;
    let _com = crate::capture::ComGuard::new()?;
    let gpu = Device::new(&std::env::var("BUTTERPOLLO_TEST_NVENC_DISPLAY").unwrap_or_default())?;
    if !gpu.display.adapter.to_ascii_lowercase().contains("nvidia") {
        bail!("select an NVIDIA-attached output with BUTTERPOLLO_TEST_NVENC_DISPLAY");
    }
    let (width, height) = (640usize, 480usize);
    for (codec, hdr) in [(0, false), (1, false), (1, true)] {
        let config = Negotiated {
            width: width as u32,
            height: height as u32,
            codec,
            hdr,
            yuv444: true,
            bitrate_kbps: 80_000,
            ..Default::default()
        };
        let mut encoder = Encoder::new_device_options(&config, gpu.clone(), &Config::default())?;
        // Red where x + y is even, blue where it is odd: BGRA8 for SDR, scRGB
        // at 80 nits for HDR.
        let bytes_per_pixel = if hdr { 8 } else { 4 };
        let mut image = Image {
            width: width as u32,
            height: height as u32,
            stride: width * bytes_per_pixel,
            bytes: vec![0; width * height * bytes_per_pixel],
            pixel: if hdr { Pixel::RgbaF16 } else { Pixel::Bgra8 },
            captured: Instant::now(),
        };
        for (index, pixel) in image.bytes.chunks_exact_mut(bytes_per_pixel).enumerate() {
            // 0 red, 1 blue.
            let site = (index % width + index / width) % 2;
            if hdr {
                let rgba: [f32; 4] = [[1., 0., 0., 1.], [0., 0., 1., 1.]][site];
                for (channel, value) in rgba.into_iter().enumerate() {
                    pixel[channel * 2..channel * 2 + 2]
                        .copy_from_slice(&half::f16::from_f32(value).to_bits().to_le_bytes());
                }
            } else {
                pixel.copy_from_slice(&[[0, 0, 255, 255], [255, 0, 0, 255]][site]);
            }
        }
        let mut packets = Vec::new();
        for frame in 0..4 {
            packets.extend(encoder.encode(&image, frame == 0, config.bitrate_kbps)?);
            let deadline = Instant::now() + Duration::from_secs(2);
            while encoder.pending() {
                packets.extend(encoder.poll()?);
                if Instant::now() >= deadline {
                    bail!("NVENC 4:4:4 frame timed out");
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        let format = if codec == 0 { "h264" } else { "hevc" };
        let name = format!("{format}-{}", if hdr { "10bit" } else { "8bit" });
        let stream = directory.join(format!("{name}.{format}"));
        let raw = directory.join(format!("{name}.yuv"));
        let bytes: Vec<u8> = packets.iter().flat_map(|p| p.bytes.clone()).collect();
        std::fs::write(&stream, bytes)?;
        let output = std::process::Command::new(&decoder)
            .args([
                "-hide_banner",
                "-nostdin",
                "-v",
                "error",
                "-y",
                "-f",
                format,
                "-i",
            ])
            .arg(&stream)
            .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt"])
            .arg(if hdr { "yuv444p10le" } else { "yuv444p" })
            .arg(&raw)
            .creation_flags(0x08000000)
            .output()?;
        assert!(
            output.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let decoded = std::fs::read(&raw)?;
        let sample_bytes = if hdr { 2 } else { 1 };
        let plane = width * height;
        assert_eq!(decoded.len(), plane * 3 * sample_bytes, "{name}: not 4:4:4");
        let sample = |plane_index: usize, x: usize, y: usize| -> f64 {
            let at = (plane_index * plane + y * width + x) * sample_bytes;
            if hdr {
                f64::from(u16::from_le_bytes([decoded[at], decoded[at + 1]])) / 4.
            } else {
                f64::from(decoded[at])
            }
        };
        // Mean chroma of each site over the picture, away from its edges, in
        // 8-bit codes.
        let mut sums = [[0f64; 2]; 2];
        let mut counts = [0f64; 2];
        for y in 8..height - 8 {
            for x in 8..width - 8 {
                let site = (x + y) % 2;
                sums[site][0] += sample(1, x, y);
                sums[site][1] += sample(2, x, y);
                counts[site] += 1.;
            }
        }
        let mean = |site: usize| sums[site].map(|sum| sum / counts[site]);
        let (red, blue) = (mean(0), mean(1));
        // Red has the higher Cr and blue the higher Cb, by over 100 codes in
        // SDR and some 30 in PQ at 80 nits. The sites of a 4:2:0 picture
        // share chroma and would be equal.
        let margin = if hdr { 12. } else { 64. };
        assert!(
            red[1] - blue[1] > margin && blue[0] - red[0] > margin,
            "{name}: chroma did not survive at full resolution (Cb red {:.1} blue {:.1}, Cr red {:.1} blue {:.1}); see {}",
            red[0],
            blue[0],
            red[1],
            blue[1],
            directory.display()
        );
    }
    Ok(())
}
