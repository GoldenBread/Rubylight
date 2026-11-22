//! The ignored tests load FFmpeg from `BUTTERPOLLO_TEST_FFMPEG_ROOT` (its `bin` directory);
//! build.ps1 sets it and runs them with `--ignored ffmpeg::`.
use super::*;
use std::path::PathBuf;

type Decoder = Ac3Decoder;

fn root() -> PathBuf {
    std::env::var_os("BUTTERPOLLO_TEST_FFMPEG_ROOT")
        .map(PathBuf::from)
        .expect("set BUTTERPOLLO_TEST_FFMPEG_ROOT to the FFmpeg build's bin directory")
}

/// `frames` frames of interleaved silence with a 0.5 sine on `active`: 1 kHz, or 50 Hz on
/// 5.1's LFE (channel 3), which AC3 low-passes at 120 Hz.
fn tone(channels: usize, active: usize, frames: usize) -> Vec<Vec<f32>> {
    let hz = if channels == 6 && active == 3 {
        50.0
    } else {
        1000.0
    };
    (0..frames)
        .map(|frame| {
            let mut samples = vec![0.0; FRAME_SAMPLES * channels];
            for index in 0..FRAME_SAMPLES {
                let t = (frame * FRAME_SAMPLES + index) as f32 / 48_000.0;
                samples[index * channels + active] = 0.5 * (std::f32::consts::TAU * hz * t).sin();
            }
            samples
        })
        .collect()
}

fn rms(samples: &[f32], channels: usize, channel: usize) -> f32 {
    let values: Vec<f32> = samples
        .iter()
        .skip(channel)
        .step_by(channels)
        .copied()
        .collect();
    (values.iter().map(|value| value * value).sum::<f32>() / values.len() as f32).sqrt()
}

#[test]
fn channel_masks_are_the_captures_layouts() {
    for (channels, mask) in [(2, 0x3u64), (6, 0x3F), (8, 0x63F)] {
        assert_eq!(channel_mask(channels), Some(mask));
        assert_eq!(mask.count_ones(), u32::from(channels));
    }
    assert_eq!(channel_mask(12), None);
    assert_eq!(channel_mask(1), None);
}

#[test]
fn a_missing_ffmpeg_is_refused_by_name() {
    let empty = std::env::temp_dir().join(format!("butterpollo-no-ffmpeg-{}", std::process::id()));
    std::fs::create_dir_all(&empty).unwrap();
    let error = format!("{:#}", probe(&empty).unwrap_err());
    std::fs::remove_dir_all(&empty).unwrap();
    assert!(error.contains(AVUTIL), "{error}");
}

/// `LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR` takes only absolute paths: a relative folder is made one.
#[test]
fn ffmpeg_is_loaded_by_its_absolute_path() {
    let path = beside(Path::new("no-ffmpeg-here"), AVUTIL).unwrap();
    assert!(path.is_absolute(), "{}", path.display());
    assert!(path.ends_with(Path::new("no-ffmpeg-here").join(AVUTIL)));
    let error = format!("{:#}", probe(Path::new("no-ffmpeg-here")).unwrap_err());
    assert!(error.contains(&path.display().to_string()), "{error}");
}

#[test]
#[ignore = "needs BUTTERPOLLO_TEST_FFMPEG_ROOT"]
fn ffmpeg_loads_from_a_folder_named_without_its_drive() {
    // `\src\...` is relative on Windows (no drive), and loads only once made absolute.
    let root = root();
    let relative: PathBuf = root
        .components()
        .filter(|c| !matches!(c, std::path::Component::Prefix(_)))
        .collect();
    assert!(!relative.is_absolute());
    // Windows resolves it on the current directory's drive. CI keeps the SDK
    // on C: and works on D:, where the DLLs aren't: then the error names the
    // path made absolute on that drive.
    fn prefix(path: &Path) -> Option<std::path::Component<'_>> {
        path.components()
            .next()
            .filter(|c| matches!(c, std::path::Component::Prefix(_)))
    }
    let cwd = std::env::current_dir().unwrap();
    if prefix(&root) == prefix(&cwd) {
        probe(&relative).unwrap();
    } else {
        let expected = beside(&relative, AVUTIL).unwrap();
        assert!(
            expected.starts_with(prefix(&cwd).unwrap()),
            "{}",
            expected.display()
        );
        let error = format!("{:#}", probe(&relative).unwrap_err());
        assert!(error.contains(&expected.display().to_string()), "{error}");
    }
}

#[test]
#[ignore = "needs BUTTERPOLLO_TEST_FFMPEG_ROOT"]
fn each_channel_round_trips_at_the_protocols_bitrates() {
    let root = root();
    for (eac3, channels, bitrate) in [
        (false, 2u8, 192_000u32),
        (false, 6, 640_000),
        (true, 2, 256_000),
        (true, 6, 768_000),
    ] {
        let count = usize::from(channels);
        for active in 0..count {
            let mut encoder = Ac3Encoder::new(&root, eac3, channels, bitrate).unwrap();
            let mut decoder = Decoder::new(&root, eac3).unwrap();
            let mut decoded = Vec::new();
            for samples in tone(count, active, 10) {
                let frame = encoder.encode(&samples).unwrap();
                // A constant bitrate: 32 ms of it per frame.
                assert_eq!(
                    frame.len(),
                    bitrate as usize * 32 / 8000,
                    "{eac3} {channels}"
                );
                let (decoded_channels, mut samples) = decoder.decode(&frame).unwrap();
                assert_eq!(decoded_channels, count);
                decoded.append(&mut samples);
            }
            // Past the encoder's delay, every channel decodes where it was sent.
            let settled = &decoded[2 * FRAME_SAMPLES * count..];
            let expected = 0.5 / std::f32::consts::SQRT_2;
            for channel in 0..count {
                let level = rms(settled, count, channel);
                if channel == active {
                    assert!(
                        (level - expected).abs() < 0.1 * expected,
                        "{eac3} {channels} channel {channel}: {level}"
                    );
                } else {
                    assert!(
                        level < 0.05 * expected,
                        "{eac3} {channels} channel {channel} took {level} of channel {active}"
                    );
                }
            }
        }
    }
}

#[test]
#[ignore = "needs BUTTERPOLLO_TEST_FFMPEG_ROOT"]
fn seven_one_is_refused_with_ffmpegs_reason() {
    for (eac3, bitrate) in [(true, 1_024_000), (false, 640_000)] {
        let error = format!(
            "{:#}",
            Ac3Encoder::new(&root(), eac3, 8, bitrate).err().unwrap()
        );
        assert!(error.contains("for 8 channels"), "{error}");
    }
    let error = format!(
        "{:#}",
        Ac3Encoder::new(&root(), false, 12, 640_000).err().unwrap()
    );
    assert!(error.contains("not 12"), "{error}");
}

#[test]
#[ignore = "needs BUTTERPOLLO_TEST_FFMPEG_ROOT"]
fn the_probe_opens_both_encoders_and_frames_must_be_whole() {
    probe(&root()).unwrap();
    let mut encoder = Ac3Encoder::new(&root(), false, 2, 192_000).unwrap();
    assert!(encoder.encode(&[0.0; 2 * 480]).is_err());
    assert_eq!(
        encoder.encode(&[0.0; 2 * FRAME_SAMPLES]).unwrap().len(),
        768
    );
}
