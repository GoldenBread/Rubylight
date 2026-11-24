//! Loopback capture tests that need a running PipeWire daemon; run them with `--ignored`.

use super::Loopback;
use std::{
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// A mono 16-bit 48 kHz WAV of a 1 kHz tone, `seconds` long.
fn tone_wav(seconds: u32) -> Vec<u8> {
    let frames = 48_000 * seconds;
    let mut wav = Vec::new();
    wav.extend(b"RIFF");
    wav.extend((36 + frames * 2).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(48_000u32.to_le_bytes());
    wav.extend(96_000u32.to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend((frames * 2).to_le_bytes());
    for n in 0..frames {
        let v = (f64::from(n) * 2. * std::f64::consts::PI * 1000. / 48_000.).sin() * 0.5;
        wav.extend(((v * 32767.) as i16).to_le_bytes());
    }
    wav
}

fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|v| v * v).sum::<f32>() / samples.len().max(1) as f32).sqrt()
}

#[test]
#[ignore = "needs a PipeWire daemon with a default sink"]
fn a_tone_played_to_the_default_sink_is_captured_quickly() {
    let mut capture = Loopback::new(2).expect("open the capture");
    assert!(capture.event_driven());
    assert_eq!(capture.source(), (2, 0));
    let path: PathBuf =
        std::env::temp_dir().join(format!("rubylight-tone-{}.wav", std::process::id()));
    std::fs::write(&path, tone_wav(2)).unwrap();
    let mut player = Command::new("pw-play")
        .arg(&path)
        .stderr(Stdio::null())
        .spawn()
        .expect("pw-play");
    let started = Instant::now();
    let mut out = Vec::new();
    let (mut first, mut loud, mut total) = (None, 0, 0);
    while started.elapsed() < Duration::from_millis(1500) {
        capture.wait(Duration::from_millis(20));
        while capture.read_into(240, &mut out).expect("capture failed") {
            total += 1;
            assert_eq!(out.len(), 480);
            if rms(&out) > 0.1 {
                loud += 1;
                first.get_or_insert(started.elapsed());
            }
        }
    }
    let _ = player.kill();
    let _ = player.wait();
    let _ = std::fs::remove_file(&path);
    eprintln!("first loud packet after {first:?}; {loud} loud of {total} packets");
    assert!(
        first.is_some_and(|at| at < Duration::from_secs(1)),
        "no tone heard"
    );
    assert!(loud > 20, "only {loud} loud packets");
    assert!(capture.take_delivered().is_some());
}

#[test]
#[ignore = "needs a PipeWire daemon"]
fn an_unknown_sink_fails_when_opened() {
    let error = Loopback::new_sink(2, "no-such-sink-rubylight")
        .err()
        .expect("must fail");
    eprintln!("{error:#}");
}

#[test]
#[ignore = "needs a PipeWire daemon"]
fn surround_layouts_open() {
    for channels in [1, 6, 8] {
        let mut capture = Loopback::new(channels).expect("open");
        capture.wait(Duration::from_millis(100));
        let mut out = Vec::new();
        capture.read_into(240, &mut out).expect("read");
    }
}

#[test]
#[ignore = "needs a PipeWire daemon with pactl"]
fn the_default_sink_opens_by_name() {
    let name = Command::new("pactl")
        .arg("get-default-sink")
        .output()
        .expect("pactl");
    let name = String::from_utf8(name.stdout).unwrap();
    let mut capture = Loopback::new_sink(2, name.trim()).expect("open by name");
    capture.wait(Duration::from_millis(50));
    let mut out = Vec::new();
    capture.read_into(240, &mut out).expect("read");
}
