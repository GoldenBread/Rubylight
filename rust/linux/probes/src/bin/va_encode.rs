//! Times VA-API HEVC encoding of pictures that are already on the GPU.
//!
//! `va_encode --size 2560x1440 --fps 120 --bitrate 40 --frames 1200 --paced`
//!
//! `--paced` submits one picture per frame interval, as a stream does, and
//! reports how long each took from submit to its bytes: the encode latency.
//! Without it, pictures go in back to back and the run reports throughput.
//! `--caps` lists what the driver offers and encodes nothing. `--out` keeps
//! the bitstream, `--csv` the per-frame rows and `--source` the test pictures
//! as raw NV12 or P010 frames, one loop of them.
#[cfg(not(target_os = "linux"))]
fn main() {}

#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    linux::run()
}

#[cfg(target_os = "linux")]
mod linux {
    use anyhow::{Context, Result};
    use butterpollo_linux_probes::{
        args::Args,
        hevc::{Encoder, Settings},
        pattern,
        stats::{self, Frame},
        va::{Display, ffi::*},
    };
    use std::{
        io::Write,
        time::{Duration, Instant},
    };

    pub fn run() -> Result<()> {
        let args = Args::parse(&["paced", "caps"])?;
        let node = args.text("node").unwrap_or("/dev/dri/renderD128");
        let display = Display::open(node)?;
        println!("driver: {}", display.vendor());
        if args.flag("caps") {
            capabilities(&display);
            return Ok(());
        }
        let (width, height) = args.size("size", (2560, 1440))?;
        let fps: u32 = args.get("fps", 120)?;
        let megabits: f64 = args.get("bitrate", 40.0)?;
        let frames: u64 = args.get("frames", 1200)?;
        let depth: u8 = args.get("depth", 8)?;
        let keyframe_every: u64 = args.get("keyframe-every", 0)?;
        let paced = args.flag("paced");
        let settings = Settings {
            width,
            height,
            fps,
            bitrate: (megabits * 1_000_000.0) as u32,
            depth,
            quality_level: args.text("quality-level").map(str::parse).transpose()?,
            vbv_frames: args.get("vbv-frames", 2.0)?,
        };

        // Every test picture is on the GPU before the first submit, so the
        // timings below hold no upload.
        let (rt_format, pixel) = Encoder::input_format(depth);
        let pictures = pattern::generate(width, height, depth);
        let mut inputs =
            display.create_surfaces(rt_format, pixel, width, height, pictures.len())?;
        for (surface, picture) in inputs.iter().zip(&pictures) {
            display.upload_two_plane(
                *surface,
                width,
                height,
                &picture.luma,
                &picture.chroma,
                if depth > 8 { 2 } else { 1 },
            )?;
        }
        if let Some(path) = args.text("source") {
            // The loop once, as raw frames, for comparing a decode against it.
            let mut file = std::fs::File::create(path).with_context(|| format!("create {path}"))?;
            for picture in &pictures {
                file.write_all(&picture.luma)?;
                file.write_all(&picture.chroma)?;
            }
        }
        drop(pictures);

        let mut encoder = Encoder::new(&display, settings)?;
        println!(
            "encode: HEVC {depth}-bit {width}x{height} at {fps} fps, {megabits} Mbit/s, \
             {frames} frames, {}",
            if paced { "paced" } else { "back to back" }
        );
        println!(
            "packed header kinds the driver takes: 0x{:x}",
            encoder.packed_headers
        );

        let mut out = match args.text("out") {
            Some(path) => {
                Some(std::fs::File::create(path).with_context(|| format!("create {path}"))?)
            }
            None => None,
        };
        let interval = Duration::from_secs_f64(1.0 / f64::from(fps));
        let mut rows = Vec::with_capacity(frames as usize);
        let mut bytes = Vec::new();
        let start = Instant::now();
        for index in 0..frames {
            let due = start + interval.mul_f64(index as f64);
            if paced {
                wait_until(due);
            }
            let input = inputs[(index % inputs.len() as u64) as usize];
            let force = keyframe_every != 0 && index != 0 && index % keyframe_every == 0;
            let began = Instant::now();
            let submitted = encoder.submit(input, force)?;
            let submit = began.elapsed();
            encoder.finish(input, &mut bytes)?;
            let total = began.elapsed();
            if let Some(file) = &mut out {
                file.write_all(&bytes)?;
            }
            rows.push(Frame {
                index,
                keyframe: submitted.keyframe,
                bytes: bytes.len(),
                submit,
                total,
                late: if paced {
                    began.saturating_duration_since(due)
                } else {
                    Duration::ZERO
                },
            });
        }
        let elapsed = start.elapsed();
        drop(encoder);
        display.destroy_surfaces(&mut inputs);

        report(&rows, elapsed, fps, paced);
        if let Some(path) = args.text("csv") {
            std::fs::write(path, stats::csv(&rows)).with_context(|| format!("write {path}"))?;
        }
        Ok(())
    }

    fn report(rows: &[Frame], elapsed: Duration, fps: u32, paced: bool) {
        // The first second holds the keyframe and the rate control settling.
        let skip = (fps as usize).min(rows.len() / 2);
        let steady = &rows[skip..];
        let total_bytes: usize = rows.iter().map(|r| r.bytes).sum();
        let seconds = rows.len() as f64 / f64::from(fps);
        println!("frames: {} in {:.2} s", rows.len(), elapsed.as_secs_f64());
        if !paced {
            println!(
                "throughput: {:.1} fps",
                rows.len() as f64 / elapsed.as_secs_f64()
            );
        }
        println!(
            "bitrate: {:.2} Mbit/s over the run's {:.2} s of video",
            total_bytes as f64 * 8.0 / seconds / 1e6,
            seconds
        );
        println!(
            "first frame: {} bytes in {:.2} ms",
            rows[0].bytes,
            rows[0].total.as_secs_f64() * 1000.0
        );
        println!("after the first second ({} frames):", steady.len());
        println!(
            "  submit  {}",
            stats::summarize(steady.iter().map(|r| r.submit)).line()
        );
        println!(
            "  total   {}",
            stats::summarize(steady.iter().map(|r| r.total)).line()
        );
        if paced {
            println!(
                "  late    {}",
                stats::summarize(steady.iter().map(|r| r.late)).line()
            );
            let budget = Duration::from_secs_f64(1.0 / f64::from(fps));
            let over = steady.iter().filter(|r| r.total > budget).count();
            println!("  over one frame interval: {over} of {}", steady.len());
        }
        let largest = steady.iter().map(|r| r.bytes).max().unwrap_or(0);
        println!("  largest frame: {largest} bytes");
    }

    /// Sleeps until shortly before `due`, then spins: a sleep alone wakes up
    /// to a millisecond late.
    fn wait_until(due: Instant) {
        loop {
            let now = Instant::now();
            if now >= due {
                return;
            }
            let left = due - now;
            if left > Duration::from_micros(300) {
                std::thread::sleep(left - Duration::from_micros(200));
            } else {
                std::hint::spin_loop();
            }
        }
    }

    fn capabilities(display: &Display) {
        let profiles = [
            ("H.264 High", VAProfileH264High),
            ("HEVC Main", VAProfileHEVCMain),
            ("HEVC Main 10", VAProfileHEVCMain10),
            ("HEVC Main 4:4:4", VAProfileHEVCMain444),
            ("AV1 Profile 0", VAProfileAV1Profile0),
        ];
        let attributes = [
            ("RTFormat", VAConfigAttribRTFormat),
            ("RateControl", VAConfigAttribRateControl),
            ("EncPackedHeaders", VAConfigAttribEncPackedHeaders),
            ("EncMaxRefFrames", VAConfigAttribEncMaxRefFrames),
            ("EncMaxSlices", VAConfigAttribEncMaxSlices),
            ("EncSliceStructure", VAConfigAttribEncSliceStructure),
            ("EncQualityRange", VAConfigAttribEncQualityRange),
            ("EncIntraRefresh", VAConfigAttribEncIntraRefresh),
            ("EncROI", VAConfigAttribEncROI),
            ("EncRateControlExt", VAConfigAttribEncRateControlExt),
            ("MaxFrameSize", VAConfigAttribMaxFrameSize),
            ("EncTileSupport", VAConfigAttribEncTileSupport),
            ("EncHEVCFeatures", VAConfigAttribEncHEVCFeatures),
            ("EncHEVCBlockSizes", VAConfigAttribEncHEVCBlockSizes),
            ("EncAV1", VAConfigAttribEncAV1),
            ("PredictionDirection", VAConfigAttribPredictionDirection),
            ("EncSkipFrame", VAConfigAttribEncSkipFrame),
            ("EncDirtyRect", VAConfigAttribEncDirtyRect),
            (
                "EncParallelRateControl",
                VAConfigAttribEncParallelRateControl,
            ),
            ("MaxPictureWidth", VAConfigAttribMaxPictureWidth),
            ("MaxPictureHeight", VAConfigAttribMaxPictureHeight),
        ];
        for (name, profile) in profiles {
            let entry = VAEntrypointEncSlice;
            if !display.entrypoints(profile).contains(&entry) {
                println!("{name}: no encoder");
                continue;
            }
            println!("{name}:");
            for (label, kind) in attributes {
                match display.attribute(profile, entry, kind) {
                    Some(value) => println!("  {label:<24} 0x{value:08x} ({value})"),
                    None => println!("  {label:<24} not supported"),
                }
            }
        }
    }
}
