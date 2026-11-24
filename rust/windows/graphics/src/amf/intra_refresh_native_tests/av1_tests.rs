//! Opt-in native AV1 intra-refresh loss recovery test and FFmpeg PSNR helpers.

use super::*;

/// Per-frame MD5s of an independent AV1 decode and its error output. A lossy
/// stream is decoded leniently: its missing reference is the point.
fn decode_av1(
    ffmpeg: &std::ffi::OsStr,
    stream: &Path,
    strict: bool,
) -> Result<(Vec<String>, String)> {
    let hashes = stream.with_extension("framemd5");
    let errors = stream.with_extension("decode.log");
    let mut command = Command::new(ffmpeg);
    command.args(["-v", "error"]);
    if strict {
        command.args(["-xerror", "-err_detect", "explode"]);
    }
    let mut child = command
        .args(["-c:v", "libdav1d", "-f", "obu", "-i"])
        .arg(stream)
        .args([
            "-map",
            "0:v:0",
            "-fps_mode",
            "passthrough",
            "-f",
            "framemd5",
            "-",
        ])
        .stdin(Stdio::null())
        .creation_flags(0x08000000)
        .stdout(fs::File::create(&hashes)?)
        .stderr(fs::File::create(&errors)?)
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(120);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("independent decoder timed out: {}", stream.display());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let stderr = fs::read_to_string(errors)?;
    anyhow::ensure!(
        status.success() || !strict,
        "independent decoder failed on {}: {stderr}",
        stream.display()
    );
    let md5s = fs::read_to_string(hashes)?
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            line.rsplit(',')
                .next()
                .unwrap_or_default()
                .trim()
                .to_string()
        })
        .collect();
    Ok((md5s, stderr))
}

/// PSNR of each decoded frame after the loss against the lossless decode of
/// the same source frame.
fn psnr_after_loss(
    ffmpeg: &std::ffi::OsStr,
    full: &Path,
    lossy: &Path,
    lost: usize,
) -> Result<Vec<f64>> {
    let stats = lossy.with_extension("psnr.log");
    // The filter graph cannot quote a drive letter, so name the log relative
    // to FFmpeg's working directory.
    let filter = format!(
        "[0:v]trim=start_frame={},setpts=N[a];[1:v]trim=start_frame={lost},setpts=N[b];[b][a]psnr=stats_file={}",
        lost + 1,
        stats
            .file_name()
            .and_then(|name| name.to_str())
            .context("PSNR log name")?
    );
    let output = Command::new(ffmpeg)
        .current_dir(stats.parent().context("PSNR log directory")?)
        .args(["-v", "error", "-c:v", "libdav1d", "-f", "obu", "-i"])
        .arg(full)
        .args(["-c:v", "libdav1d", "-f", "obu", "-i"])
        .arg(lossy)
        .args(["-lavfi", &filter, "-f", "null", "-"])
        .stdin(Stdio::null())
        .creation_flags(0x08000000)
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "PSNR comparison failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(fs::read_to_string(&stats)?
        .lines()
        .filter_map(|line| {
            line.split_whitespace()
                .find_map(|field| field.strip_prefix("psnr_avg:"))
        })
        .map(|value| value.parse::<f64>().unwrap_or(f64::INFINITY))
        .collect())
}

/// Recovery after one lost AV1 frame with intra refresh forced and with the
/// default policy, which leaves it off: how long the decoder shows a damaged
/// picture, and the keyframes each recovery costs. The lost frame is dropped from a copy of the stream;
/// nothing else differs between the two decodes.
#[test]
#[ignore = "requires an idle AMD GPU, AMF, independent FFmpeg and a task-owned report path"]
fn native_av1_intra_refresh_recovery_after_a_lost_frame() -> Result<()> {
    const WIDTH: usize = 2560;
    const HEIGHT: usize = 1440;
    const LOST: usize = 480;
    const PHASES: usize = 32;
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .try_init();
    let report = std::path::PathBuf::from(
        std::env::var_os("BUTTERPOLLO_TEST_AV1_INTRA_REPORT")
            .context("set BUTTERPOLLO_TEST_AV1_INTRA_REPORT to a task-owned JSON path")?,
    );
    anyhow::ensure!(report.is_absolute(), "report path must be absolute");
    fs::create_dir_all(report.parent().context("report directory")?)?;
    let ffmpeg =
        std::env::var_os("BUTTERPOLLO_TEST_FFMPEG").context("set BUTTERPOLLO_TEST_FFMPEG")?;
    let _com = crate::capture::ComGuard::new()?;
    let device = Device::new("")?;
    let compute = crate::compute::Compute::for_device(&device.device)?;
    let timer = crate::timing::Timer::new()?;
    // Sixteen pixels a frame over a 512-pixel period.
    let pictures = (0..PHASES)
        .map(|phase| scrolled(&device, WIDTH, HEIGHT, phase * 16))
        .collect::<Result<Vec<_>>>()?;
    let mut reports = Vec::new();
    let mut failures = Vec::new();
    // Every arm's client negotiates refresh, as an Xbox does. (name, refresh
    // forced past the policy that turns it off, what the host sends for the loss)
    for (name, refresh, recovery) in [
        ("refresh", true, "nothing"),
        ("refresh_keyframe", true, "keyframe"),
        ("ltr", false, "invalidation"),
        ("keyframe", false, "keyframe"),
    ] {
        let config = butterpollo_media_policy::rtsp::Negotiated {
            width: WIDTH as u32,
            height: HEIGHT as u32,
            fps: 120,
            rate_millihz: 120_000,
            bitrate_kbps: 50_000,
            codec: 2,
            intra_refresh: true,
            ..Default::default()
        };
        intra_refresh::force_av1(refresh);
        let encoder = Encoder::new_gpu(
            &config,
            device.clone(),
            &Default::default(),
            Some(compute.clone()),
        );
        intra_refresh::force_av1(false);
        let mut encoder = encoder?;
        let mut properties = serde_json::Map::new();
        for property in [
            intra_refresh::AV1_MODE,
            intra_refresh::AV1_STRIPES,
            "Av1MaxNumLTRFrames",
            "Av1MaxNumRefFrames",
            "Av1GOPSize",
        ] {
            properties.insert(property.into(), serde_json::json!(encoder.read(property)));
        }
        let established = intra_refresh::AV1_REFRESH
            .iter()
            .all(|&(property, value)| encoder.read(property) == Some(value));
        anyhow::ensure!(
            established == refresh,
            "{name}: refresh {} established: {properties:?}",
            if refresh { "not" } else { "unexpectedly" }
        );
        let full_path = report.with_extension(format!("{name}.obu"));
        let lossy_path = report.with_extension(format!("{name}.lossy.obu"));
        let mut full = std::io::BufWriter::new(fs::File::create(&full_path)?);
        let mut lossy = std::io::BufWriter::new(fs::File::create(&lossy_path)?);
        let start = Instant::now();
        let period = Duration::from_secs_f64(1. / 120.);
        let mut frames = Vec::new();
        let mut invalidated = None;
        for index in 0..FRAMES {
            timer.until(start + period * index as u32);
            let mut image = pictures[index % PHASES].clone();
            image.captured = Instant::now();
            let deadline = image.captured + Duration::from_secs(2);
            if index == LOST + 1 && recovery == "invalidation" {
                // The client names the lost frame; frame numbers start at one.
                invalidated = Some(encoder.invalidate_ref_frames(LOST as u64 + 1, LOST as u64 + 1));
            }
            let idr = index == 0 || (index == LOST + 1 && recovery == "keyframe");
            let mut output = encoder.encode_gpu(&image, idr, config.bitrate_kbps)?;
            while encoder.pending() {
                anyhow::ensure!(
                    Instant::now() < deadline,
                    "AMF stalled at {name} frame {index}"
                );
                output.extend(encoder.poll()?);
            }
            anyhow::ensure!(output.len() == 1, "missing/late {name} output {index}");
            let frame = output.pop().unwrap();
            anyhow::ensure!(!idr || frame.idr, "forced IDR absent at {name} {index}");
            full.write_all(&frame.bytes)?;
            if index != LOST {
                lossy.write_all(&frame.bytes)?;
            }
            frames.push((frame.idr, frame.bytes.len()));
        }
        full.flush()?;
        lossy.flush()?;
        drop(full);
        drop(lossy);
        drop(encoder);
        let (clean, clean_errors) = decode_av1(&ffmpeg, &full_path, true)?;
        let (damaged, damaged_errors) = decode_av1(&ffmpeg, &lossy_path, false)?;
        // Decoded frame j of the lossy stream is source frame j + 1 after the loss.
        let matches: Vec<bool> = (LOST + 1..FRAMES)
            .map(|source| damaged.get(source - 1) == clean.get(source))
            .collect();
        let identical_from = matches
            .iter()
            .rposition(|matched| !matched)
            .map_or(0, |last| last + 1);
        let psnr = psnr_after_loss(&ffmpeg, &full_path, &lossy_path, LOST)?;
        let finite = |db: f64| db.is_finite().then_some(db);
        let keyframes: Vec<usize> = frames
            .iter()
            .filter(|frame| frame.0)
            .map(|frame| frame.1)
            .collect();
        let inter: Vec<usize> = frames
            .iter()
            .filter(|frame| !frame.0)
            .map(|frame| frame.1)
            .collect();
        let result = serde_json::json!({
            "variant": name, "negotiated_intra_refresh": config.intra_refresh,
            "intra_refresh": refresh, "recovery": recovery,
            "adapter": device.display.adapter, "width": WIDTH, "height": HEIGHT,
            "fps": 120, "bitrate_kbps": config.bitrate_kbps, "frames": FRAMES, "lost_frame": LOST,
            "source": "scrolling synthetic texture, 16 px a frame; no capture/display/network",
            "properties": properties, "invalidation_accepted": invalidated,
            "keyframes": keyframes.len(), "keyframe_bytes": keyframes,
            "recovery_frame_bytes": frames[LOST + 1].1, "recovery_frame_idr": frames[LOST + 1].0,
            "max_frame_bytes_after_first": frames.iter().skip(1).map(|frame| frame.1).max(),
            "mean_inter_frame_bytes": inter.iter().sum::<usize>() / inter.len().max(1),
            "clean_frames_decoded": clean.len(), "clean_decoder_errors": clean_errors,
            "lossy_frames_decoded": damaged.len(),
            "lossy_decoder_error_lines": damaged_errors.lines().count(),
            "frames_after_loss_until_identical":
                (identical_from < matches.len()).then_some(identical_from + 1),
            "frames_after_loss_identical": matches.iter().filter(|matched| **matched).count(),
            "frames_after_loss_compared": matches.len(),
            "frames_after_loss_below_40db": psnr.iter().filter(|&&db| db < 40.).count(),
            "last_frame_after_loss_below_40db": psnr.iter().rposition(|&db| db < 40.).map(|n| n + 1),
            "min_psnr_after_loss_db": psnr.iter().copied().filter_map(finite).reduce(f64::min),
            "psnr_first_frames_after_loss_db":
                psnr.iter().take(8).map(|&db| finite(db)).collect::<Vec<_>>(),
        });
        println!("AV1 {name}: {result}");
        // Refresh alone is recorded, not required: on 32.0.32015.7029 dav1d
        // rejects every frame after the loss until a keyframe arrives.
        let recovers = recovery == "nothing" || damaged.len() == FRAMES - 1;
        // The default path answers the loss with the invalidated frame, not a
        // keyframe.
        let invalidates = recovery != "invalidation"
            || (invalidated == Some(true) && !frames[LOST + 1].0 && identical_from == 0);
        if clean.len() != FRAMES || !clean_errors.is_empty() || !recovers || !invalidates {
            failures.push(format!(
                "{name}: decoded {} clean and {} lossy frames; invalidation {invalidated:?}, frame {} keyframe {}, identical from {}",
                clean.len(),
                damaged.len(),
                LOST + 1,
                frames[LOST + 1].0,
                LOST + 1 + identical_from
            ));
        }
        reports.push(result);
        fs::write(&report, serde_json::to_vec_pretty(&reports)?)?;
    }
    anyhow::ensure!(failures.is_empty(), "{failures:?}");
    Ok(())
}
