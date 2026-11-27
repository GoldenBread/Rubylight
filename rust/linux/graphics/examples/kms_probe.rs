//! KMS capture probe: follows one output's page flips and, every few seconds, grabs the shown
//! framebuffer, imports it through VA-API as the host's capture does, and writes it as a PNG
//! with the active session in its name. Run it with `CAP_SYS_ADMIN` and switch to the login
//! screen meanwhile; it keeps the card it opened.
//!
//! `kms_probe [--card /dev/dri/cardN] [--output DP-1] [--seconds 60] [--every 3] [--out DIR]`
use anyhow::{Context, Result, bail};
use butterpollo_linux_graphics::{
    capture::kms::{self, Card, Grab},
    vaapi::{
        AR30, BGRA, BGRX, Plane, RGBA, RGBX, Va, XR30, ffi::VA_RT_FORMAT_RGB32, vpp::Converter,
    },
};
use std::{
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

struct Args {
    card: Option<PathBuf>,
    output: Option<String>,
    seconds: u64,
    every: u64,
    out: PathBuf,
}

fn args() -> Result<Args> {
    let mut args = Args {
        card: None,
        output: None,
        seconds: 60,
        every: 3,
        out: std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".cache/rubylight/kms-probe"))
            .unwrap_or_else(|| PathBuf::from("kms-probe")),
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let value = it.next().with_context(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--card" => args.card = Some(value.into()),
            "--output" => args.output = Some(value),
            "--seconds" => args.seconds = value.parse()?,
            "--every" => args.every = value.parse()?,
            "--out" => args.out = value.into(),
            other => bail!("unknown flag {other}"),
        }
    }
    Ok(args)
}

/// The VT shown and the seat's active session with its user and class.
fn session() -> String {
    let vt = std::fs::read_to_string("/sys/class/tty/tty0/active").unwrap_or_default();
    let seat = std::fs::read_to_string("/run/systemd/seats/seat0").unwrap_or_default();
    let active = seat
        .lines()
        .find_map(|l| l.strip_prefix("ACTIVE="))
        .unwrap_or("none")
        .to_string();
    let detail =
        std::fs::read_to_string(format!("/run/systemd/sessions/{active}")).unwrap_or_default();
    let field = |key: &str| {
        detail
            .lines()
            .find_map(|l| l.strip_prefix(key))
            .unwrap_or("?")
            .to_string()
    };
    format!(
        "{}-s{active}-{}-{}",
        vt.trim(),
        field("USER="),
        field("CLASS=")
    )
}

fn va_pixel(fourcc: u32) -> Option<u32> {
    Some(match &fourcc.to_le_bytes() {
        b"XR24" => BGRX,
        b"AR24" => BGRA,
        b"XB24" => RGBX,
        b"AB24" => RGBA,
        b"AR30" => AR30,
        b"XR30" => XR30,
        _ => return None,
    })
}

/// Imports `grab`, copies it into a linear BGRX surface and reads it back as RGB rows.
fn picture(va: &Arc<Va>, grab: &Grab) -> Result<(Vec<u8>, [Duration; 3])> {
    let pixel = va_pixel(grab.fourcc)
        .with_context(|| format!("no VA format for {}", kms::fourcc_text(grab.fourcc)))?;
    let started = Instant::now();
    // The compositor may still be drawing: wait for its fence, bounded.
    let mut ready = libc::pollfd {
        fd: grab.planes[0].fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one open descriptor.
    unsafe { libc::poll(&raw mut ready, 1, 50) };
    let planes: Vec<Plane> = grab
        .planes
        .iter()
        .map(|p| Plane {
            fd: p.fd.as_raw_fd(),
            offset: p.offset,
            pitch: p.pitch,
        })
        .collect();
    let input = va.import_dmabuf(pixel, grab.modifier, grab.width, grab.height, &planes)?;
    let imported = started.elapsed();
    let size = (grab.width, grab.height);
    let mut output = va.create_linear_surfaces(VA_RT_FORMAT_RGB32, BGRX, size.0, size.1, 1)?;
    let converter = Converter::new(va, size.0, size.1)?;
    let copy = converter.scale_rgb(input, size, output[0], size);
    let synced = copy.and_then(|()| va.sync(output[0]));
    let converted = started.elapsed();
    let mut bgra = Vec::new();
    let read = synced.and_then(|()| va.download_bgra(output[0], size.0, size.1, &mut bgra));
    let downloaded = started.elapsed();
    va.destroy_surfaces(&mut [input]);
    va.destroy_surfaces(&mut output);
    read?;
    let rgb = bgra
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[2], p[1], p[0]])
        .collect();
    Ok((rgb, [imported, converted, downloaded]))
}

/// A linear 32-bit plane (the cursor's) read through a CPU mapping, as RGBA.
fn mapped_rgba(grab: &Grab) -> Result<Vec<u8>> {
    if grab.modifier != 0 && grab.modifier != kms::MOD_INVALID {
        bail!("modifier 0x{:x} is not linear", grab.modifier);
    }
    let plane = &grab.planes[0];
    let length = plane.offset as usize + plane.pitch as usize * grab.height as usize;
    // SAFETY: a read-only shared mapping of a DMA-BUF this process holds, unmapped below.
    let base = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            length,
            libc::PROT_READ,
            libc::MAP_SHARED,
            plane.fd.as_raw_fd(),
            0,
        )
    };
    if base == libc::MAP_FAILED {
        return Err(std::io::Error::last_os_error()).context("mapping the plane");
    }
    let mut rgba = Vec::with_capacity(grab.width as usize * grab.height as usize * 4);
    for y in 0..grab.height as usize {
        // SAFETY: the row lies inside the mapping (`length` covers every row).
        let row = unsafe {
            std::slice::from_raw_parts(
                base.cast::<u8>()
                    .add(plane.offset as usize + y * plane.pitch as usize),
                grab.width as usize * 4,
            )
        };
        for p in row.as_chunks::<4>().0 {
            rgba.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
    }
    // SAFETY: the mapping made above.
    unsafe { libc::munmap(base, length) };
    Ok(rgba)
}

fn write_png(path: &Path, width: u32, height: u32, rgb: &[u8]) -> Result<()> {
    let file = std::fs::File::create(path)?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(if rgb.len() == width as usize * height as usize * 4 {
        png::ColorType::Rgba
    } else {
        png::ColorType::Rgb
    });
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(rgb)?;
    Ok(())
}

fn main() -> Result<()> {
    let args = args()?;
    std::fs::create_dir_all(&args.out)?;
    let cards = match &args.card {
        Some(card) => vec![card.clone()],
        None => kms::cards(),
    };
    let mut chosen = None;
    for path in cards {
        let Ok(card) = Card::open(&path) else {
            continue;
        };
        let outputs = card.outputs()?;
        for output in &outputs {
            println!("{} {output:?}", path.display());
        }
        if let Some(output) = outputs
            .into_iter()
            .find(|o| args.output.as_ref().is_none_or(|name| &o.name == name))
            && chosen.is_none()
        {
            chosen = Some((card, output));
        }
    }
    let (card, mut output) = chosen.context("no lit output to follow")?;
    println!(
        "following {} on {}, pictures in {}",
        output.name,
        card.path().display(),
        args.out.display()
    );
    // Without a VA-API driver (a virtual GPU), linear framebuffers are read through a mapping.
    let va = Va::open("/dev/dri/renderD128")
        .inspect_err(|e| println!("no VA-API ({e:#}): reading linear framebuffers directly"))
        .ok()
        .map(Arc::new);
    let start = Instant::now();
    let end = start + Duration::from_secs(args.seconds);
    let mut next_shot = start;
    let mut second = start;
    let (mut last_fb, mut flips, mut distinct) = (0, 0u32, std::collections::BTreeSet::new());
    while Instant::now() < end {
        let fb = card.primary_fb(output.crtc_id).unwrap_or(0);
        if fb != last_fb {
            flips += 1;
            distinct.insert(fb);
            last_fb = fb;
        }
        let now = Instant::now();
        if now >= second + Duration::from_secs(1) {
            println!(
                "{:>4}s {} flips/s {flips} framebuffers {:?}",
                (now - start).as_secs(),
                session(),
                distinct
            );
            flips = 0;
            distinct.clear();
            second = now;
        }
        if now >= next_shot {
            next_shot = now + Duration::from_secs(args.every);
            // Another compositor may drive the monitor from another controller.
            if let Some(found) = card
                .outputs()
                .ok()
                .and_then(|all| all.into_iter().find(|o| o.name == output.name))
                && found != output
            {
                println!("  {} is now {found:?}", output.name);
                output = found;
            }
            let fb = card.primary_fb(output.crtc_id).unwrap_or(0);
            let planes = card.planes(output.crtc_id).unwrap_or_default();
            let grabbed = Instant::now();
            match card.grab(fb) {
                Ok(grab) => {
                    let exported = grabbed.elapsed();
                    let others: Vec<String> = planes
                        .iter()
                        .filter(|p| p.fb_id != 0 && p.fb_id != fb)
                        .filter_map(|p| card.grab(p.fb_id).ok())
                        .map(|g| {
                            let saved = mapped_rgba(&g).and_then(|rgba| {
                                let name = args.out.join(format!(
                                    "{:03}s-plane-fb{}.png",
                                    (Instant::now() - start).as_secs(),
                                    g.fb_id
                                ));
                                write_png(&name, g.width, g.height, &rgba)
                            });
                            format!(
                                "{}x{} {} modifier 0x{:x}{}",
                                g.width,
                                g.height,
                                kms::fourcc_text(g.fourcc),
                                g.modifier,
                                saved.err().map(|e| format!(" ({e:#})")).unwrap_or_default()
                            )
                        })
                        .collect();
                    print!(
                        "  fb {fb}: {}x{} {} modifier 0x{:x} {} plane(s), export {:?}, other planes {others:?}",
                        grab.width,
                        grab.height,
                        kms::fourcc_text(grab.fourcc),
                        grab.modifier,
                        grab.planes.len(),
                        exported
                    );
                    let read = match &va {
                        Some(va) => picture(va, &grab),
                        None => {
                            let started = Instant::now();
                            mapped_rgba(&grab).map(|rgba| {
                                let rgb = rgba
                                    .as_chunks::<4>()
                                    .0
                                    .iter()
                                    .flat_map(|p| [p[0], p[1], p[2]])
                                    .collect();
                                let took = started.elapsed();
                                (rgb, [Duration::ZERO, Duration::ZERO, took])
                            })
                        }
                    };
                    match read {
                        Ok((rgb, [imported, converted, read])) => {
                            let name = args.out.join(format!(
                                "{:03}s-{}.png",
                                (Instant::now() - start).as_secs(),
                                session()
                            ));
                            write_png(&name, grab.width, grab.height, &rgb)?;
                            println!(
                                ", import {imported:?}, copied {converted:?}, read {read:?} -> {}",
                                name.display()
                            );
                        }
                        Err(error) => println!(", reading it: {error:#}"),
                    }
                }
                Err(error) => println!("  fb {fb}: {error:#}"),
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}
