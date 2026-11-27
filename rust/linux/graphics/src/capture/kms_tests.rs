//! Tests of the KMS capture's target names, formats and the cursor's clipping, and a live
//! capture through the broker.
use super::*;

#[test]
fn targets_name_an_output_or_the_first() {
    assert_eq!(parse("kms"), Some(None));
    assert_eq!(parse("kms:DP-1"), Some(Some("DP-1")));
    assert_eq!(parse("kms:"), None);
    assert_eq!(parse("kmsDP-1"), None);
    assert_eq!(parse("kwin-output:DP-1"), None);
}

#[test]
fn scanout_formats_map_onto_va() {
    assert_eq!(va_pixel(u32::from_le_bytes(*b"XR24")), Some(BGRX));
    assert_eq!(va_pixel(u32::from_le_bytes(*b"AR30")), Some(AR30));
    assert_eq!(va_pixel(u32::from_le_bytes(*b"AB30")), Some(AB30));
    assert_eq!(va_pixel(u32::from_le_bytes(*b"NV12")), None);
}

fn rect(x: u32, y: u32, width: u32, height: u32) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

#[test]
fn the_pointer_is_cut_to_the_screen() {
    let frame = (1920, 1080);
    // Whole.
    assert_eq!(
        visible(100, 200, (64, 64), frame),
        Some((rect(0, 0, 64, 64), rect(100, 200, 64, 64)))
    );
    // Hanging off the top-left corner: the hotspot sits inside the image.
    assert_eq!(
        visible(-10, -3, (64, 64), frame),
        Some((rect(10, 3, 54, 61), rect(0, 0, 54, 61)))
    );
    // Hanging off the bottom-right edge.
    assert_eq!(
        visible(1900, 1070, (64, 64), frame),
        Some((rect(0, 0, 20, 10), rect(1900, 1070, 20, 10)))
    );
    // Off the screen.
    assert_eq!(visible(-64, 0, (64, 64), frame), None);
    assert_eq!(visible(1920, 0, (64, 64), frame), None);
}

/// Opens `kms` through the broker as a stream does and saves its third frame as a PNG in
/// `RUBYLIGHT_LIVE_KMS_PNG` (default /tmp/kms-capture.png). Needs rubylight-kms.socket and
/// this user in the rubylight group, and a VA-API GPU.
#[test]
#[ignore = "needs the KMS broker and a VA-API GPU"]
fn live_the_screen_comes_through_the_broker() {
    let va = Arc::new(Va::open("/dev/dri/renderD128").unwrap());
    let picture = Picture {
        full_chroma: true,
        color_matrix: 1,
        full_range: false,
    };
    let capture = KmsCapture::open(None, &va, picture).unwrap();
    eprintln!("{} at {:?}", capture.output, capture.size);
    let mut taken = 0;
    let started = Instant::now();
    let mut last = None;
    while taken < 3 && started.elapsed() < Duration::from_secs(10) {
        if let Some(frame) = capture.take().unwrap() {
            taken += 1;
            eprintln!(
                "frame {taken}: converted {:?} after it came",
                frame.acquired - frame.captured
            );
            last = Some(frame);
        } else {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    let frame = last.expect("no frame within 10 s (move the mouse)");
    let (width, height) = capture.size;
    let mut bgra = Vec::new();
    va.download_bgra(frame.surface.id(), width, height, &mut bgra)
        .unwrap();
    let rgb: Vec<u8> = bgra
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[2], p[1], p[0]])
        .collect();
    let path = std::env::var("RUBYLIGHT_LIVE_KMS_PNG").unwrap_or("/tmp/kms-capture.png".into());
    let file = std::fs::File::create(&path).unwrap();
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&rgb)
        .unwrap();
    eprintln!("saved {path}");
}
