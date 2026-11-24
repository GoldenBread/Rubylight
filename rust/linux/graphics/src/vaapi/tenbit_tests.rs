//! Ten-bit encoding on the GPU: pictures from system memory go to P010 surfaces, the codec
//! probe's HDR tags and the HDR10 metadata come out in the stream, and FFmpeg reads them. Without
//! a render node or FFmpeg they skip.
#![allow(clippy::print_stderr)]
use super::tests::{negotiated, picture, scratch, skip, tool};
use crate::encoder::Encoder;
use butterpollo_config::config::Config;
use butterpollo_media_policy::hdr::Metadata;
use std::process::Command;

const WIDTH: u32 = 640;
const HEIGHT: u32 = 480;

/// What `ffprobe` says of the first picture and the stream, as JSON text.
pub(super) fn describe(stream: &[u8], format: &str, name: &str) -> String {
    let file = scratch(&format!("{name}.bin"));
    std::fs::write(&file, stream).unwrap();
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-f", format, "-read_intervals", "%+#1"])
        .args(["-show_frames", "-show_streams", "-of", "json"])
        .arg(&file)
        .output()
        .expect("ffprobe runs");
    let _ = std::fs::remove_file(&file);
    assert!(
        output.status.success() && output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn encode(codec: u8, hdr: bool, ten_bit: bool, metadata: Option<Metadata>) -> Option<Vec<u8>> {
    if crate::vaapi::node::nodes().is_empty() || !tool("ffprobe") {
        skip("no render node or no ffprobe");
        return None;
    }
    let mut config = negotiated(codec, WIDTH, HEIGHT, 60);
    config.hdr = hdr;
    config.sdr_10bit = ten_bit;
    let open = if hdr {
        Encoder::new_options_hdr
    } else {
        Encoder::new_options
    };
    let mut encoder = match open(&config, "", &Config::default()) {
        Ok(encoder) => encoder,
        Err(error) => {
            skip(&format!("the GPU has no such encoder: {error:#}"));
            return None;
        }
    };
    if let Some(metadata) = metadata {
        encoder.set_hdr_metadata(metadata);
    }
    let mut stream = Vec::new();
    for step in 0..6 {
        let out = encoder
            .encode(&picture(WIDTH, HEIGHT, step), step == 0, 8000)
            .expect("encode");
        stream.extend_from_slice(&out[0].bytes);
    }
    Some(stream)
}

fn metadata() -> Metadata {
    Metadata {
        primaries: [[34000, 16000], [13250, 34500], [7500, 3000]],
        white: [15635, 16450],
        maximum_nits: 1000,
        minimum: 50,
        max_cll: 900,
        max_fall: 400,
        full_frame_nits: 0,
    }
}

#[test]
fn hevc_ten_bit_pictures_decode_as_main_10() {
    let Some(stream) = encode(1, false, true, None) else {
        return;
    };
    let text = describe(&stream, "hevc", "hevc10");
    assert!(text.contains("\"pix_fmt\": \"yuv420p10le\""), "{text}");
    assert!(text.contains("\"profile\": \"Main 10\""), "{text}");
    // Not HDR: no transfer function of PQ, no metadata.
    assert!(
        !text.contains("smpte2084") && !text.contains("Mastering"),
        "{text}"
    );
}

#[test]
fn av1_ten_bit_pictures_decode() {
    let Some(stream) = encode(2, false, true, None) else {
        return;
    };
    let text = describe(&stream, "obu", "av110");
    assert!(text.contains("\"pix_fmt\": \"yuv420p10le\""), "{text}");
    assert!(
        !text.contains("smpte2084") && !text.contains("Mastering"),
        "{text}"
    );
}

fn check_hdr(text: &str) {
    assert!(text.contains("\"pix_fmt\": \"yuv420p10le\""), "{text}");
    assert!(text.contains("\"color_transfer\": \"smpte2084\""), "{text}");
    assert!(text.contains("\"color_primaries\": \"bt2020\""), "{text}");
    assert!(text.contains("Mastering display metadata"), "{text}");
    assert!(text.contains("Content light level metadata"), "{text}");
    assert!(text.contains("\"max_content\": 900"), "{text}");
    assert!(text.contains("\"max_average\": 400"), "{text}");
    // Red is the first primary: 34000/50000 = 0.68, which AV1 carries as 44564/65536.
    let red =
        text.contains("\"red_x\": \"34000/50000\"") || text.contains("\"red_x\": \"44564/65536\"");
    assert!(red, "{text}");
}

#[test]
fn hevc_hdr_tags_and_metadata_come_out() {
    let Some(stream) = encode(1, true, false, Some(metadata())) else {
        return;
    };
    check_hdr(&describe(&stream, "hevc", "hevchdr"));
}

#[test]
fn av1_hdr_tags_and_metadata_come_out() {
    let Some(stream) = encode(2, true, false, Some(metadata())) else {
        return;
    };
    check_hdr(&describe(&stream, "obu", "av1hdr"));
}

#[test]
fn unavailable_ten_bit_modes_fail_visibly() {
    // H.264 has no ten-bit mode.
    let mut config = negotiated(0, WIDTH, HEIGHT, 60);
    config.sdr_10bit = true;
    let error = Encoder::new_options(&config, "", &Config::default())
        .err()
        .expect("H.264 10-bit fails");
    assert!(format!("{error:#}").contains("no 10-bit mode"), "{error:#}");
}
