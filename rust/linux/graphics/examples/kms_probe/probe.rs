//! The probe itself, on Linux.
use anyhow::{Context, Result, bail};
use butterpollo_linux_graphics::vaapi::{
    AR30, BGRA, BGRX, Plane, RGBA, RGBX, Va, XR30, ffi::VA_RT_FORMAT_RGB32, vpp::Converter,
};
use rubylight_kms_broker::{
    client::Client,
    drm::{self, Card, Grab, GrabPlane, Output},
    protocol,
};
use std::{
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

struct Args {
    direct: bool,
    card: Option<PathBuf>,
    output: Option<String>,
    seconds: u64,
    every: u64,
    out: PathBuf,
}

fn args() -> Result<Args> {
    let mut args = Args {
        direct: false,
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
        if flag == "--direct" {
            args.direct = true;
            continue;
        }
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
        .with_context(|| format!("no VA format for {}", drm::fourcc_text(grab.fourcc)))?;
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
    if grab.modifier != 0 && grab.modifier != drm::MOD_INVALID {
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

/// Where frames come from: the card itself (needs `CAP_SYS_ADMIN`) or the KMS broker.
enum Source {
    Direct { card: Card, output: Output },
    Broker { client: Client, output: String },
}

/// A framebuffer and the cursor plane over it, at its place.
type Shown = (Grab, Option<(i32, i32, Grab)>);

impl Source {
    fn open(args: &Args) -> Result<Self> {
        if !args.direct {
            let client = Client::connect(Path::new(protocol::SOCKET))?;
            let outputs = client.outputs()?;
            for output in &outputs {
                println!("broker: {output:?}");
            }
            let output = outputs
                .into_iter()
                .find(|o| args.output.as_ref().is_none_or(|name| &o.name == name))
                .context("no lit output to follow")?;
            return Ok(Source::Broker {
                client,
                output: output.name,
            });
        }
        let cards = match &args.card {
            Some(card) => vec![card.clone()],
            None => drm::cards(),
        };
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
            {
                return Ok(Source::Direct { card, output });
            }
        }
        bail!("no lit output to follow")
    }

    fn name(&self) -> &str {
        match self {
            Source::Direct { output, .. } => &output.name,
            Source::Broker { output, .. } => output,
        }
    }

    /// What the output shows once it changed from `after` (its last framebuffer; the broker
    /// also counts a moved pointer), waiting up to 100 ms.
    fn next(&mut self, after: u32) -> Result<Option<Shown>> {
        let wait = Duration::from_millis(100);
        match self {
            Source::Broker { client, output } => {
                let Some(frame) = client.frame(output, after != 0, wait)? else {
                    return Ok(None);
                };
                let info = frame.info;
                let grab = Grab {
                    fb_id: info.fb_id,
                    width: info.width,
                    height: info.height,
                    fourcc: info.fourcc,
                    modifier: info.modifier,
                    planes: frame
                        .planes
                        .into_iter()
                        .zip(&info.planes)
                        .map(|(fd, p)| GrabPlane {
                            fd,
                            pitch: p.pitch,
                            offset: p.offset,
                        })
                        .collect(),
                };
                let cursor = info.cursor.zip(frame.cursor).map(|(c, fd)| {
                    let plane = GrabPlane {
                        fd,
                        pitch: c.pitch,
                        offset: c.offset,
                    };
                    let image = Grab {
                        fb_id: c.fb_id,
                        width: c.width,
                        height: c.height,
                        fourcc: c.fourcc,
                        modifier: c.modifier,
                        planes: vec![plane],
                    };
                    (c.x, c.y, image)
                });
                Ok(Some((grab, cursor)))
            }
            Source::Direct { card, output } => {
                let end = Instant::now() + wait;
                while Instant::now() < end {
                    let fb = card.primary_fb(output.crtc_id).unwrap_or(0);
                    if fb != 0 && fb != after {
                        let grab = card.grab(fb)?;
                        let cursor = card
                            .cursor(output.crtc_id)?
                            .and_then(|c| Some((c.x, c.y, card.grab(c.fb_id).ok()?)));
                        return Ok(Some((grab, cursor)));
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                // Another compositor may drive the monitor from another controller.
                if let Some(found) = card.outputs()?.into_iter().find(|o| o.name == output.name)
                    && found != *output
                {
                    println!("  {} is now {found:?}", output.name);
                    *output = found;
                }
                Ok(None)
            }
        }
    }
}

pub fn main() -> Result<()> {
    let args = args()?;
    std::fs::create_dir_all(&args.out)?;
    let mut source = Source::open(&args)?;
    println!(
        "following {} {}, pictures in {}",
        source.name(),
        if args.direct {
            "on the card itself"
        } else {
            "through the KMS broker"
        },
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
        let asked = Instant::now();
        let shown = source.next(last_fb)?;
        let took = asked.elapsed();
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
        let Some((grab, cursor)) = shown else {
            continue;
        };
        flips += 1;
        distinct.insert(grab.fb_id);
        last_fb = grab.fb_id;
        if now < next_shot {
            continue;
        }
        next_shot = now + Duration::from_secs(args.every);
        let seconds = (now - start).as_secs();
        let pointer = match &cursor {
            Some((x, y, image)) => {
                let saved = mapped_rgba(image).and_then(|rgba| {
                    let name = args
                        .out
                        .join(format!("{seconds:03}s-cursor-at-{x}x{y}.png"));
                    write_png(&name, image.width, image.height, &rgba)
                });
                format!(
                    "cursor {}x{} {} at {x},{y}{}",
                    image.width,
                    image.height,
                    drm::fourcc_text(image.fourcc),
                    saved.err().map(|e| format!(" ({e:#})")).unwrap_or_default()
                )
            }
            None => "no cursor".into(),
        };
        print!(
            "  fb {}: {}x{} {} modifier 0x{:x} {} plane(s), came in {took:?}, {pointer}",
            grab.fb_id,
            grab.width,
            grab.height,
            drm::fourcc_text(grab.fourcc),
            grab.modifier,
            grab.planes.len(),
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
                    (rgb, [Duration::ZERO, Duration::ZERO, started.elapsed()])
                })
            }
        };
        match read {
            Ok((rgb, [imported, converted, read])) => {
                let name = args.out.join(format!("{seconds:03}s-{}.png", session()));
                write_png(&name, grab.width, grab.height, &rgb)?;
                println!(
                    ", import {imported:?}, copied {converted:?}, read {read:?} -> {}",
                    name.display()
                );
            }
            Err(error) => println!(", reading it: {error:#}"),
        }
    }
    Ok(())
}
