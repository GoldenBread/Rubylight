//! Encodes pictures with the PyroWave library on this machine's GPU and decodes them with
//! the same library's decoder (the one Nightjar loads), comparing against the source. The
//! tests skip, saying why, where there is no library (`BUTTERPOLLO_PYROWAVE_LIB`) or GPU.

use super::*;
use std::{
    ffi::c_void,
    fs::OpenOptions,
    os::fd::{FromRawFd, OwnedFd, RawFd},
};

fn picture(width: u32, height: u32) -> Image {
    let (w, h) = (width as usize, height as usize);
    let mut bytes = vec![0u8; w * h * 4];
    let mut seed = 0x2545_f491u32;
    for y in 0..h {
        for x in 0..w {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (seed >> 31) as i32;
            let edge = if (x / 40 + y / 40) % 2 == 0 { 40 } else { 0 };
            let at = (y * w + x) * 4;
            let v = |base: usize| (base as i32 / 4 + edge + noise).clamp(0, 255) as u8;
            bytes[at] = v(x * 255 / w);
            bytes[at + 1] = v(y * 255 / h);
            bytes[at + 2] = v((x + y) * 255 / (w + h));
            bytes[at + 3] = 255;
        }
    }
    Image {
        width,
        height,
        stride: w * 4,
        bytes,
        captured: Instant::now(),
        pixel: Pixel::Bgra8,
    }
}

fn config(width: u32, height: u32, yuv444: bool) -> Negotiated {
    Negotiated {
        width,
        height,
        fps: 60,
        bitrate_kbps: 400_000,
        codec: 3,
        yuv444,
        packet_size: 1392,
        ..Default::default()
    }
}

fn available() -> bool {
    match ffi::Api::load() {
        Ok(_) => true,
        Err(error) => {
            eprintln!("skipped: {error:#}");
            false
        }
    }
}

/// Decodes the packets of the frame `encoder` just wrote, as planes.
fn decode(encoder: &Encoder, frame: &Encoded, config: &Negotiated) -> [Vec<u8>; 3] {
    let lib = ffi::open().unwrap();
    let api = ffi::decode::Api::bind(&lib).unwrap();
    let mut device = ptr::null_mut();
    // SAFETY: the library's documented creation call with null identifiers.
    ffi::check(unsafe {
        (encoder.api.create_device_by_compat)(
            0,
            0,
            ptr::null(),
            ptr::null(),
            ptr::null(),
            vk::QueueGlobalPriorityKHR::MEDIUM,
            &mut device,
        )
    })
    .unwrap();
    let info = ffi::decode::DecoderCreateInfo {
        device,
        width: config.width as i32,
        height: config.height as i32,
        chroma: i32::from(config.yuv444),
        fragment_path: false,
    };
    let mut decoder = ptr::null_mut();
    // SAFETY: live device and info.
    ffi::check(unsafe { (api.create)(&info, &mut decoder) }).unwrap();
    for packet in &encoder.packets[..encoder.packets.len()] {
        let slice = &frame.bytes[packet.offset..packet.offset + packet.size];
        // SAFETY: a slice of the frame's bytes.
        ffi::check(unsafe { (api.push_packet)(decoder, slice.as_ptr().cast(), slice.len()) })
            .unwrap();
    }
    // SAFETY: the decoder is live.
    let ready = unsafe { (api.is_ready)(decoder, false) };
    assert!(ready, "frame incomplete");
    let (w, h) = (config.width as usize, config.height as usize);
    let (cw, ch) = if config.yuv444 {
        (w, h)
    } else {
        (w / 2, h / 2)
    };
    let mut planes = [vec![0u8; w * h], vec![0u8; cw * ch], vec![0u8; cw * ch]];
    let buffer = ffi::CpuBuffer {
        data: planes.each_mut().map(|p| p.as_mut_ptr().cast::<c_void>()),
        row_stride_in_bytes: [w, cw, cw],
        plane_size_in_bytes: [w * h, cw * ch, cw * ch],
        width: w as i32,
        height: h as i32,
        format: if config.yuv444 {
            ffi::CPU_FORMAT_YUV444P
        } else {
            ffi::CPU_FORMAT_YUV420P
        },
    };
    // SAFETY: planes are sized as the buffer says.
    ffi::check(unsafe { (api.decode_cpu)(decoder, &buffer) }).unwrap();
    // SAFETY: nothing uses them afterwards.
    unsafe {
        (api.destroy)(decoder);
        (encoder.api.device_destroy)(device);
    }
    planes
}

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let sum: f64 = a
        .iter()
        .zip(b)
        .map(|(a, b)| (f64::from(*a) - f64::from(*b)).powi(2))
        .sum();
    if sum == 0. {
        return 99.;
    }
    10. * (255f64 * 255. / (sum / a.len() as f64)).log10()
}

fn round_trip(width: u32, height: u32, yuv444: bool) {
    if !available() {
        return;
    }
    let config = config(width, height, yuv444);
    let image = picture(width, height);
    let mut encoder = Encoder::new(&config, &Options::default()).unwrap();
    let mut best = None;
    let mut times = Vec::new();
    for _ in 0..12 {
        let started = Instant::now();
        let frames = encoder.encode(&image, true, config.bitrate_kbps).unwrap();
        times.push(started.elapsed());
        best = frames.into_iter().next();
    }
    times.sort();
    let frame = best.expect("a frame");
    assert!(frame.framing.is_some(), "container framing");
    let decoded = decode(&encoder, &frame, &config);
    let source = encoder.planes.planes();
    let scores: Vec<f64> = (0..3).map(|i| psnr(source[i], &decoded[i])).collect();
    eprintln!(
        "{width}x{height} 4:4:4={yuv444}: {} bytes, median {:?} per frame, PSNR Y {:.1} Cb {:.1} Cr {:.1} dB",
        frame.bytes.len(),
        times[times.len() / 2],
        scores[0],
        scores[1],
        scores[2]
    );
    assert!(
        scores[0] > 35. && scores[1] > 45. && scores[2] > 45.,
        "{scores:?}"
    );
}

#[test]
fn cpu_picture_420_round_trips() {
    round_trip(1280, 720, false);
}

#[test]
fn cpu_picture_444_round_trips() {
    round_trip(1280, 720, true);
}

#[test]
fn cpu_picture_1080p_timing() {
    round_trip(1920, 1080, false);
}

#[test]
fn probe_offers_both_sdr_modes() {
    if !available() {
        return;
    }
    assert_eq!(probe_flags(&Options::default()).unwrap(), 0x0180_0000);
}

#[test]
fn hdr_is_refused() {
    let mut hdr = config(640, 480, false);
    hdr.hdr = true;
    assert!(check_config(&hdr).is_err());
}

// A dumb buffer of the first DRM card, exported as a linear DMA-BUF.
const fn iowr(nr: u64, size: u64) -> u64 {
    (3 << 30) | (size << 16) | (0x64 << 8) | nr
}

fn dumb_buffer(image: &Image) -> Option<DmaBuf> {
    #[repr(C)]
    #[derive(Default)]
    struct Create {
        height: u32,
        width: u32,
        bpp: u32,
        flags: u32,
        handle: u32,
        pitch: u32,
        size: u64,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Map {
        handle: u32,
        pad: u32,
        offset: u64,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Prime {
        handle: u32,
        flags: u32,
        fd: i32,
    }
    use std::os::fd::AsRawFd;
    let card = (0..4).find_map(|n| {
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(format!("/dev/dri/card{n}"))
            .ok()
    })?;
    let fd: RawFd = card.as_raw_fd();
    let mut create = Create {
        height: image.height,
        width: image.width,
        bpp: 32,
        ..Default::default()
    };
    // SAFETY: plain DRM ioctls with matching structures on an open card.
    unsafe {
        if libc::ioctl(fd, iowr(0xb2, 32) as _, &mut create) != 0 {
            return None;
        }
        let mut map = Map {
            handle: create.handle,
            ..Default::default()
        };
        if libc::ioctl(fd, iowr(0xb3, 16) as _, &mut map) != 0 {
            return None;
        }
        let at = libc::mmap(
            ptr::null_mut(),
            create.size as usize,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd,
            map.offset as _,
        );
        if at == libc::MAP_FAILED {
            return None;
        }
        for row in 0..image.height as usize {
            ptr::copy_nonoverlapping(
                image.bytes[row * image.stride..].as_ptr(),
                at.cast::<u8>().add(row * create.pitch as usize),
                image.width as usize * 4,
            );
        }
        libc::munmap(at, create.size as usize);
        let mut prime = Prime {
            handle: create.handle,
            flags: libc::O_CLOEXEC as u32,
            fd: -1,
        };
        if libc::ioctl(fd, iowr(0x2d, 12) as _, &mut prime) != 0 {
            return None;
        }
        Some(DmaBuf {
            fd: OwnedFd::from_raw_fd(prime.fd),
            width: image.width,
            height: image.height,
            fourcc: dmabuf::DRM_FORMAT_XRGB8888,
            modifier: 0,
            planes: vec![Plane {
                offset: 0,
                pitch: u64::from(create.pitch),
            }],
        })
    }
}

#[test]
fn dmabuf_picture_round_trips() {
    if !available() {
        return;
    }
    let (width, height) = (1920, 1080);
    let config = config(width, height, false);
    let image = picture(width, height);
    let Some(buf) = dumb_buffer(&image) else {
        eprintln!("skipped: no DRM dumb buffer");
        return;
    };
    let mut encoder = Encoder::new(&config, &Options::default()).unwrap();
    // The CPU path's planes of the same picture are the reference.
    encoder.encode(&image, true, config.bitrate_kbps).unwrap();
    let reference: Vec<Vec<u8>> = encoder.planes.planes().iter().map(|p| p.to_vec()).collect();
    let mut times = Vec::new();
    let mut frame = None;
    for _ in 0..12 {
        let started = Instant::now();
        let frames = encoder
            .encode_dmabuf(&buf, Instant::now(), true, config.bitrate_kbps)
            .unwrap();
        times.push(started.elapsed());
        frame = frames.into_iter().next();
    }
    times.sort();
    let frame = frame.expect("a frame");
    let decoded = decode(&encoder, &frame, &config);
    let scores: Vec<f64> = (0..3).map(|i| psnr(&reference[i], &decoded[i])).collect();
    eprintln!(
        "dmabuf 1920x1080: {} bytes, median {:?} per frame, PSNR vs CPU conversion Y {:.1} Cb {:.1} Cr {:.1} dB",
        frame.bytes.len(),
        times[times.len() / 2],
        scores[0],
        scores[1],
        scores[2]
    );
    assert!(
        scores[0] > 30. && scores[1] > 45. && scores[2] > 45.,
        "{scores:?}"
    );
}
