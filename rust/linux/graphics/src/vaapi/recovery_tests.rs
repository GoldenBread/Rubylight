//! Loss recovery on the GPU: encode a sequence, report pictures lost, leave them out of the
//! stream and let FFmpeg decode what is left. The pictures after a report refer to one from
//! before the loss, so they decode to exactly what the uninterrupted stream decodes to, and
//! FFmpeg says nothing about missing references. Without a render node or FFmpeg they skip.
#![allow(clippy::print_stderr)]
use super::tests::{negotiated, picture, scratch, skip, tool};
use crate::encoder::Encoder;
use butterpollo_config::config::Config;
use std::process::Command;

const WIDTH: u32 = 640;
const HEIGHT: u32 = 480;

/// FFmpeg's decode of `stream` as raw 4:2:0 pictures, or what it complained about.
fn decode(stream: &[u8], format: &str, name: &str) -> Result<Vec<Vec<u8>>, String> {
    let file = scratch(&format!("{name}.bin"));
    std::fs::write(&file, stream).map_err(|e| e.to_string())?;
    let mut command = Command::new("ffmpeg");
    command.args(["-v", "error"]);
    if format == "obu" {
        command.args(["-c:v", "libdav1d"]);
    }
    let output = command
        .args(["-f", format, "-i"])
        .arg(&file)
        .args(["-f", "rawvideo", "-pix_fmt", "yuv420p", "-"])
        .output();
    let _ = std::fs::remove_file(&file);
    let output = output.map_err(|e| e.to_string())?;
    if !output.status.success() || !output.stderr.is_empty() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    let size = (WIDTH * HEIGHT * 3 / 2) as usize;
    Ok(output.stdout.chunks(size).map(<[u8]>::to_vec).collect())
}

/// The mean difference between two pictures, in levels.
fn difference(a: &[u8], b: &[u8]) -> f64 {
    let sum: u64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| u64::from(x.abs_diff(*y)))
        .sum();
    sum as f64 / a.len() as f64
}

fn recover(codec: u8, format: &str) {
    if crate::vaapi::node::nodes().is_empty() || !tool("ffmpeg") {
        return skip("no render node or no ffmpeg");
    }
    let config = negotiated(codec, WIDTH, HEIGHT, 60);
    let mut encoder = match Encoder::new_options(&config, "", &Config::default()) {
        Ok(encoder) => encoder,
        Err(error) => return skip(&format!("the GPU has no such encoder: {error:#}")),
    };
    assert!(encoder.supports_invalidation());
    // Pictures by wire number; losses are reported once the picture before is out.
    let mut pictures: Vec<Vec<u8>> = Vec::new();
    let mut idr = Vec::new();
    let mut recovered = Vec::new();
    let mut lost: Vec<u64> = Vec::new();
    for step in 0..40u32 {
        let wire = u64::from(step) + 1;
        let mut keyframe = step == 0;
        match wire {
            // Two pictures lost, the reference before them still held.
            11 => assert!(encoder.invalidate_ref_frames(9, 10)),
            // The same report again changes nothing.
            12 => assert!(encoder.invalidate_ref_frames(9, 10)),
            // One picture lost.
            21 => assert!(encoder.invalidate_ref_frames(20, 20)),
            // Longer ago than any picture held: only a keyframe repairs it.
            31 => {
                assert!(!encoder.invalidate_ref_frames(2, 3));
                keyframe = true;
            }
            _ => {}
        }
        let out = encoder
            .encode(&picture(WIDTH, HEIGHT, step), keyframe, 4000)
            .expect("encode");
        assert_eq!(out.len(), 1);
        if out[0].after_invalidation {
            recovered.push(wire);
        }
        if out[0].idr {
            idr.push(wire);
        }
        pictures.push(out[0].bytes.clone());
    }
    assert_eq!(idr, [1, 31]);
    assert_eq!(recovered, [11, 21]);
    lost.extend([9, 10, 20]);

    let all: Vec<u8> = pictures.concat();
    let left: Vec<u8> = pictures
        .iter()
        .enumerate()
        .filter(|(index, _)| !lost.contains(&(*index as u64 + 1)))
        .flat_map(|(_, bytes)| bytes.iter().copied())
        .collect();
    let full = decode(&all, format, &format!("{format}-all")).expect("the whole stream decodes");
    let partial = decode(&left, format, &format!("{format}-left"))
        .expect("the stream without the lost pictures decodes");
    assert_eq!(full.len(), 40);
    assert_eq!(partial.len(), 37, "{format}: one picture for each one sent");
    // Every picture that arrived decodes as it does when nothing is lost.
    let mut worst = 0f64;
    let mut kept = 0;
    for (index, picture) in full.iter().enumerate() {
        if lost.contains(&(index as u64 + 1)) {
            continue;
        }
        worst = worst.max(difference(picture, &partial[kept]));
        kept += 1;
    }
    eprintln!("{format}: worst difference from the uninterrupted decode {worst:.4} levels");
    assert!(
        worst < 0.001,
        "{format}: pictures after the loss differ ({worst})"
    );

    // Control: a picture lost without a report leaves its successors referring to nothing.
    let broken: Vec<u8> = pictures
        .iter()
        .enumerate()
        .filter(|(index, _)| ![15u64].contains(&(*index as u64 + 1)))
        .flat_map(|(_, bytes)| bytes.iter().copied())
        .collect();
    let unreported = decode(&broken, format, &format!("{format}-broken"));
    assert!(
        unreported.is_err()
            || unreported.is_ok_and(|frames| frames
                .iter()
                .zip(&full)
                .any(|(a, b)| difference(a, b) > 0.001)),
        "{format}: the control stream decoded as the whole one"
    );
}

#[test]
fn h264_recovers_from_loss_without_a_keyframe() {
    recover(0, "h264");
}

#[test]
fn hevc_recovers_from_loss_without_a_keyframe() {
    recover(1, "hevc");
}

#[test]
fn av1_recovers_from_loss_without_a_keyframe() {
    recover(2, "obu");
}

#[test]
fn a_report_beyond_the_pictures_held_asks_for_a_keyframe() {
    if crate::vaapi::node::nodes().is_empty() {
        return skip("no render node");
    }
    let config = negotiated(0, WIDTH, HEIGHT, 60);
    let Ok(mut encoder) = Encoder::new_options(&config, "", &Config::default()) else {
        return skip("the GPU has no such encoder");
    };
    for step in 0..12 {
        encoder
            .encode(&picture(WIDTH, HEIGHT, step), step == 0, 4000)
            .expect("encode");
    }
    // Nothing from before picture 9 is held beyond the window, and picture 1 is the IDR picture.
    assert!(!encoder.invalidate_ref_frames(1, 12));
    assert!(!encoder.invalidate_ref_frames(5, 6));
    assert!(!encoder.invalidate_ref_frames(13, 14));
    assert!(encoder.invalidate_ref_frames(11, 12));
}
