//! Captures a PipeWire video node as DMA-BUFs and encodes each frame with
//! VA-API without the picture ever entering system memory.
//!
//! `pw_capture --target gamescope --format nv12 --fps 120 --frames 1200`
//!
//! `--target` is a node's name or serial; `--node` its id. `--format nv12`
//! takes pictures the producer already converted (gamescope does) and encodes
//! them as they are; `--format bgrx` takes desktop pictures and converts them
//! on the GPU first. `--no-encode` only measures when frames arrive. `--out`
//! keeps the bitstream, `--csv` the per-frame rows.
#[cfg(not(target_os = "linux"))]
fn main() {}

#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    linux::run()
}

#[cfg(target_os = "linux")]
mod linux {
    use anyhow::{Context, Result, anyhow, bail};
    use butterpollo_linux_probes::{
        args::Args,
        hevc::{Encoder, Settings},
        stats::{self, Frame},
        va::{self, Display, Plane, ffi::VASurfaceID},
        vpp::Converter,
    };
    use pipewire as pw;
    use pw::spa::{
        self,
        buffer::{DataType, meta::MetaHeader},
        param::{
            ParamType,
            format::{FormatProperties, MediaSubtype, MediaType},
            video::{VideoFormat, VideoInfoRaw},
        },
        pod::{ChoiceValue, Object, Pod, Property, PropertyFlags, Value, serialize::PodSerializer},
        utils::{Choice, ChoiceEnum, ChoiceFlags, Id, SpaTypes},
    };
    use std::{
        cell::RefCell,
        collections::HashMap,
        io::Write,
        rc::Rc,
        time::{Duration, Instant},
    };

    /// `DRM_FORMAT_MOD_LINEAR` and `DRM_FORMAT_MOD_INVALID` (the producer's
    /// own layout, undeclared).
    const MOD_LINEAR: i64 = 0;
    const MOD_INVALID: i64 = 0x00ff_ffff_ffff_ffff;

    struct Negotiated {
        pixel: u32,
        width: u32,
        height: u32,
        modifier: u64,
    }

    struct State {
        display: &'static Display,
        mainloop: pw::main_loop::MainLoopRc,
        encode: bool,
        fps: u32,
        bitrate: u32,
        wanted: u64,
        format: Option<Negotiated>,
        encoder: Option<Encoder<'static>>,
        converter: Option<Converter<'static>>,
        /// One surface per buffer of the stream's pool, by its descriptor.
        surfaces: HashMap<i32, VASurfaceID>,
        rows: Vec<Frame>,
        /// Time between one frame's arrival and the next.
        intervals: Vec<Duration>,
        /// The producer's timestamp to the frame's arrival here.
        ages: Vec<Duration>,
        imports: Vec<Duration>,
        /// Time until the producer's own GPU work on a frame was done.
        renders: Vec<Duration>,
        last_arrival: Option<Instant>,
        bytes: Vec<u8>,
        out: Option<std::fs::File>,
        skipped: u64,
        error: Option<anyhow::Error>,
    }

    pub fn run() -> Result<()> {
        let args = Args::parse(&["no-encode"])?;
        let display: &'static Display = Box::leak(Box::new(Display::open(
            args.text("render-node").unwrap_or("/dev/dri/renderD128"),
        )?));
        println!("driver: {}", display.vendor());
        let format = match args.text("format").unwrap_or("nv12") {
            "nv12" => VideoFormat::NV12,
            "bgrx" => VideoFormat::BGRx,
            "bgra" => VideoFormat::BGRA,
            "rgbx" => VideoFormat::RGBx,
            "rgba" => VideoFormat::RGBA,
            other => bail!("--format: nv12, bgrx, bgra, rgbx or rgba, not {other:?}"),
        };
        let megabits: f64 = args.get("bitrate", 40.0)?;

        pw::init();
        let mainloop = pw::main_loop::MainLoopRc::new(None)?;
        let context = pw::context::ContextRc::new(&mainloop, None)?;
        let core = context.connect_rc(None)?;
        let mut properties = pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        };
        if let Some(target) = args.text("target") {
            properties.insert("target.object", target);
        }
        let node: Option<u32> = args.text("node").map(str::parse).transpose()?;
        if node.is_none() && args.text("target").is_none() {
            bail!("name the node to capture: --target NAME or --node ID");
        }
        let stream = pw::stream::StreamBox::new(&core, "rubylight-capture-probe", properties)?;

        let state = Rc::new(RefCell::new(State {
            display,
            mainloop: mainloop.clone(),
            encode: !args.flag("no-encode"),
            fps: args.get("fps", 120)?,
            bitrate: (megabits * 1_000_000.0) as u32,
            wanted: args.get("frames", 1200)?,
            format: None,
            encoder: None,
            converter: None,
            surfaces: HashMap::new(),
            rows: Vec::new(),
            intervals: Vec::new(),
            ages: Vec::new(),
            imports: Vec::new(),
            renders: Vec::new(),
            last_arrival: None,
            bytes: Vec::new(),
            out: match args.text("out") {
                Some(path) => {
                    Some(std::fs::File::create(path).with_context(|| format!("create {path}"))?)
                }
                None => None,
            },
            skipped: 0,
            error: None,
        }));

        let listener = stream
            .add_local_listener_with_user_data(state.clone())
            .state_changed(|_, state, old, new| {
                let mut state = state.borrow_mut();
                println!("stream: {old:?} -> {new:?}");
                if let pw::stream::StreamState::Error(text) = new {
                    state.error = Some(anyhow!("stream error: {text}"));
                    state.mainloop.quit();
                }
            })
            .param_changed(|stream, state, id, param| {
                let Some(param) = param else { return };
                if id != ParamType::Format.as_raw() {
                    return;
                }
                let mut state = state.borrow_mut();
                if let Err(e) = format_changed(stream, &mut state, param) {
                    state.error = Some(e);
                    state.mainloop.quit();
                }
            })
            .process(|stream, state| {
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };
                let mut state = state.borrow_mut();
                if let Err(e) = frame(&mut state, &mut buffer) {
                    state.error = Some(e);
                    state.mainloop.quit();
                } else if state.rows.len() as u64 >= state.wanted {
                    state.mainloop.quit();
                }
            })
            .register()?;

        // DMA-BUF only: a modifier marks the offer as one, and it is
        // mandatory so that no memory-copy format can be chosen instead.
        let mut offered = vec![
            property(
                FormatProperties::MediaType.as_raw(),
                id_value(MediaType::Video.as_raw()),
            ),
            property(
                FormatProperties::MediaSubtype.as_raw(),
                id_value(MediaSubtype::Raw.as_raw()),
            ),
            property(
                FormatProperties::VideoFormat.as_raw(),
                id_value(format.as_raw()),
            ),
            Property {
                key: FormatProperties::VideoModifier.as_raw(),
                flags: PropertyFlags::MANDATORY,
                value: Value::Choice(ChoiceValue::Long(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Enum {
                        default: MOD_LINEAR,
                        alternatives: vec![MOD_LINEAR, MOD_INVALID],
                    },
                ))),
            },
        ];
        if format == VideoFormat::NV12 {
            // The producer converts, so it must be told which conversion the
            // stream's headers announce: BT.709, limited range.
            offered.push(property(
                FormatProperties::VideoColorMatrix.as_raw(),
                id_value(spa::sys::SPA_VIDEO_COLOR_MATRIX_BT709),
            ));
            offered.push(property(
                FormatProperties::VideoColorRange.as_raw(),
                id_value(spa::sys::SPA_VIDEO_COLOR_RANGE_16_235),
            ));
        }
        let offer = serialize(Object {
            type_: SpaTypes::ObjectParamFormat.as_raw(),
            id: ParamType::EnumFormat.as_raw(),
            properties: offered,
        })?;
        let mut params = [Pod::from_bytes(&offer).context("format offer")?];
        stream.connect(
            spa::utils::Direction::Input,
            node,
            pw::stream::StreamFlags::AUTOCONNECT,
            &mut params,
        )?;
        mainloop.run();

        // The callbacks hold the other reference to the state.
        drop(listener);
        drop(stream);
        let state = Rc::try_unwrap(state)
            .map_err(|_| anyhow!("the stream still holds the probe's state"))?
            .into_inner();
        report(state, args.text("csv"))
    }

    fn property(key: u32, value: Value) -> Property {
        Property {
            key,
            flags: PropertyFlags::empty(),
            value,
        }
    }

    fn id_value(raw: u32) -> Value {
        Value::Id(Id(raw))
    }

    fn serialize(object: Object) -> Result<Vec<u8>> {
        let (cursor, _) =
            PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(object))
                .map_err(|e| anyhow!("serialize a PipeWire parameter: {e:?}"))?;
        Ok(cursor.into_inner())
    }

    fn format_changed(stream: &pw::stream::Stream, state: &mut State, param: &Pod) -> Result<()> {
        let mut info = VideoInfoRaw::new();
        info.parse(param)
            .map_err(|e| anyhow!("unreadable video format: {e:?}"))?;
        let pixel = match info.format() {
            VideoFormat::NV12 => va::NV12,
            VideoFormat::BGRx => va::BGRX,
            VideoFormat::BGRA => va::BGRA,
            VideoFormat::RGBx => va::RGBX,
            VideoFormat::RGBA => va::RGBA,
            other => bail!("the node chose {other:?}, which this probe does not take"),
        };
        let size = info.size();
        println!(
            "format: {:?} {}x{}, modifier 0x{:x}, {}/{} fps (0 means as frames come), \
             colour matrix {} range {}",
            info.format(),
            size.width,
            size.height,
            info.modifier(),
            info.framerate().num,
            info.framerate().denom,
            info.color_matrix(),
            info.color_range()
        );
        state.format = Some(Negotiated {
            pixel,
            width: size.width,
            height: size.height,
            modifier: info.modifier(),
        });

        // Ask for DMA-BUF buffers and for each frame's header (its timestamp).
        let buffers = serialize(Object {
            type_: SpaTypes::ObjectParamBuffers.as_raw(),
            id: ParamType::Buffers.as_raw(),
            properties: vec![property(
                spa::sys::SPA_PARAM_BUFFERS_dataType,
                Value::Int(1 << spa::sys::SPA_DATA_DmaBuf),
            )],
        })?;
        let meta = serialize(Object {
            type_: SpaTypes::ObjectParamMeta.as_raw(),
            id: ParamType::Meta.as_raw(),
            properties: vec![
                property(
                    spa::sys::SPA_PARAM_META_type,
                    id_value(spa::sys::SPA_META_Header),
                ),
                property(
                    spa::sys::SPA_PARAM_META_size,
                    Value::Int(size_of::<spa::sys::spa_meta_header>() as i32),
                ),
            ],
        })?;
        let mut params = [
            Pod::from_bytes(&buffers).context("buffers parameter")?,
            Pod::from_bytes(&meta).context("meta parameter")?,
        ];
        stream.update_params(&mut params)?;
        Ok(())
    }

    fn frame(state: &mut State, buffer: &mut pw::buffer::Buffer) -> Result<()> {
        let arrived = Instant::now();
        let pts = buffer.find_meta::<MetaHeader>().map(MetaHeader::pts);
        let Some(format) = &state.format else {
            bail!("a frame arrived before its format");
        };
        let mut planes = Vec::with_capacity(2);
        for data in buffer.datas_mut() {
            if data.type_() != DataType::DmaBuf {
                bail!(
                    "the node sent {:?} buffers, not DMA-BUFs: the picture would be copied",
                    data.type_()
                );
            }
            planes.push(Plane {
                fd: data.fd(),
                offset: data.chunk().offset(),
                pitch: data.chunk().stride() as u32,
            });
        }
        if planes.is_empty() {
            state.skipped += 1;
            return Ok(());
        }
        if format.pixel == va::NV12 && planes.len() == 1 {
            // gamescope describes a two-plane picture with its first plane
            // only. The chroma plane follows the luma rows directly: an
            // assumption that holds for linear layouts whose height needs no
            // padding, and that a decoded frame checks.
            let luma = planes[0];
            planes.push(Plane {
                offset: luma.offset + luma.pitch * format.height,
                ..luma
            });
        }
        if state.rows.is_empty() && state.intervals.is_empty() && state.last_arrival.is_none() {
            println!("first buffer: {planes:?}");
        }
        if let Some(last) = state.last_arrival.replace(arrived) {
            state.intervals.push(arrived - last);
        }
        if let Some(age) = pts.and_then(age_of) {
            state.ages.push(age);
        }
        let index = state.rows.len() as u64;
        if !state.encode {
            state.rows.push(Frame {
                index,
                keyframe: false,
                bytes: 0,
                submit: Duration::ZERO,
                total: Duration::ZERO,
                late: Duration::ZERO,
            });
            return Ok(());
        }

        let display = state.display;
        let surface = match state.surfaces.get(&planes[0].fd) {
            Some(&surface) => surface,
            None => {
                let began = Instant::now();
                let surface = display.import_dmabuf(
                    format.pixel,
                    format.modifier,
                    format.width,
                    format.height,
                    &planes,
                )?;
                state.imports.push(began.elapsed());
                state.surfaces.insert(planes[0].fd, surface);
                surface
            }
        };
        if state.encoder.is_none() {
            let settings = Settings {
                width: format.width,
                height: format.height,
                fps: state.fps,
                bitrate: state.bitrate,
                depth: 8,
                quality_level: None,
                vbv_frames: 2.0,
            };
            state.encoder = Some(Encoder::new(display, settings)?);
            if format.pixel != va::NV12 {
                let (rt_format, pixel) = Encoder::input_format(8);
                state.converter = Some(Converter::new(
                    display,
                    format.width,
                    format.height,
                    rt_format,
                    pixel,
                )?);
            }
        }
        // A buffer can arrive before the producer's GPU has finished drawing
        // it. Waiting here keeps that time out of the encode figures.
        let waiting = Instant::now();
        let mut ready = libc::pollfd {
            fd: planes[0].fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one descriptor that stays open while the buffer is held.
        unsafe { libc::poll(&mut ready, 1, 100) };
        state.renders.push(waiting.elapsed());
        let began = Instant::now();
        let input = match &mut state.converter {
            Some(converter) => converter.convert(surface)?,
            None => surface,
        };
        let encoder = state.encoder.as_mut().expect("made above");
        let submitted = encoder.submit(input, false)?;
        let submit = began.elapsed();
        encoder.finish(input, &mut state.bytes)?;
        let total = began.elapsed();
        if let Some(file) = &mut state.out {
            file.write_all(&state.bytes)?;
        }
        state.rows.push(Frame {
            index,
            keyframe: submitted.keyframe,
            bytes: state.bytes.len(),
            submit,
            total,
            late: arrived.elapsed(),
        });
        Ok(())
    }

    /// How old a frame is, if its timestamp is on the monotonic clock: the
    /// time from the producer stamping it to now.
    fn age_of(pts: i64) -> Option<Duration> {
        let mut now = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: a valid clock id and an out pointer.
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
        let now = now.tv_sec * 1_000_000_000 + now.tv_nsec;
        // A stamp from another clock, or none, lands far outside this window.
        (pts > 0 && pts <= now && now - pts < 1_000_000_000)
            .then(|| Duration::from_nanos((now - pts) as u64))
    }

    fn report(mut state: State, csv: Option<&str>) -> Result<()> {
        // The encoder and converter go before the surfaces they read.
        state.encoder = None;
        state.converter = None;
        let mut surfaces: Vec<VASurfaceID> = state.surfaces.values().copied().collect();
        state.display.destroy_surfaces(&mut surfaces);
        if let Some(error) = state.error {
            return Err(error);
        }
        let rows = &state.rows;
        if rows.is_empty() {
            bail!("no frame arrived");
        }
        println!(
            "frames: {} ({} empty buffers skipped)",
            rows.len(),
            state.skipped
        );
        println!("buffers in the stream's pool: {}", surfaces.len());
        // The first second holds the keyframe and the stream settling.
        let skip = (state.fps as usize).min(rows.len() / 2);
        let intervals = &state.intervals[skip.min(state.intervals.len())..];
        let mean = stats::summarize(intervals.iter().copied());
        println!("after the first second:");
        println!("  arrival interval  {}", mean.line());
        if mean.mean > 0.0 {
            println!("  arrival rate      {:.1} fps", 1000.0 / mean.mean);
        }
        if state.ages.is_empty() {
            println!("  frame age at arrival: the producer's timestamps are not on this clock");
        } else {
            let ages = &state.ages[skip.min(state.ages.len())..];
            println!(
                "  age at arrival    {}",
                stats::summarize(ages.iter().copied()).line()
            );
        }
        if state.encode {
            let steady = &rows[skip..];
            println!(
                "  import (once per pool buffer)  {}",
                stats::summarize(state.imports.iter().copied()).line()
            );
            let renders = &state.renders[skip.min(state.renders.len())..];
            println!(
                "  producer's GPU    {}",
                stats::summarize(renders.iter().copied()).line()
            );
            println!(
                "  submit            {}",
                stats::summarize(steady.iter().map(|r| r.submit)).line()
            );
            println!(
                "  encode            {}",
                stats::summarize(steady.iter().map(|r| r.total)).line()
            );
            println!(
                "  arrival to bytes  {}",
                stats::summarize(steady.iter().map(|r| r.late)).line()
            );
            let bytes: usize = steady.iter().map(|r| r.bytes).sum();
            let seconds: f64 = intervals.iter().map(Duration::as_secs_f64).sum();
            if seconds > 0.0 {
                println!(
                    "  bitrate           {:.2} Mbit/s",
                    bytes as f64 * 8.0 / seconds / 1e6
                );
            }
        }
        if let Some(path) = csv {
            std::fs::write(path, stats::csv(rows)).with_context(|| format!("write {path}"))?;
        }
        Ok(())
    }
}
