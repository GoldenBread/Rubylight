//! Test-owned fake mouse-look game with a known input response, for measuring Frame Warp
//! (docs/frame-warp.md). Not packaged. Requires an explicitly selected display and never
//! changes its resolution, refresh rate or HDR.
//!
//! A borderless window covers the display with a procedural outdoor world (sky and clouds,
//! ground, poles and towers) that turns with the mouse and the right stick, under a HUD that
//! never moves (crosshair, health bar, ammo counter, minimap) drawn in exact constant colours.
//! The response is known exactly. The frame drawn at time t shows the camera for the input as
//! of t - lag:
//!
//! ```text
//! camera = gain * mouse_sum(t - lag) + stick_gain * integral of stick_curve(stick) up to t - lag
//! ```
//!
//! t is the frame's render time, taken just before it is drawn and presented. The mouse sums
//! are raw input's relative moves, injected (SendInput, as the host's) or, unless
//! `--injected-only`, not; the window gets them unfocused (RIDEV_INPUTSINK) and ignores
//! absolute packets. The stick is XInput pad `--pad`'s right stick, polled once a frame and
//! held until the next poll, through the same stick_curve the host fits (XInput's axes: y is
//! up, so pushing up moves the camera down, as inverted look does). The content moves against
//! the camera, so a host that fits this game learns mouse_gain = -gain and stick_gain =
//! -stick_gain on both axes (picture x right, y down; scaled by the stream's width over the
//! display's), and a lag of `--lag-ms` plus the time from drawing a frame to its capture.
//! Camera y stops at half the screen height either way and drops the turn past it, as a game's
//! pitch does; x wraps around a world four screen widths wide.
//!
//! With `--fov DEGREES` the world is drawn through a perspective camera with that horizontal
//! field of view instead of slid across the screen, as a 3D game draws: it lies on a cylinder
//! around the camera, whose focal length is `f = (width / 2) / tan(fov / 2)` pixels, and the
//! camera above is its turn in pixels at the screen's centre, so it looks `camera_x / f` radians
//! to the side (about the upright axis) and `camera_y / f` down. The world point a pixel `(x, y)`
//! from the screen's centre shows is, with `y' = y cos(pitch) + f sin(pitch)` and
//! `z' = f cos(pitch) - y sin(pitch)`, at `f * (yaw + atan2(x, z'))` along the cylinder and
//! `f * y' / hypot(x, z')` below the horizon. A turn then moves the screen's edges farther than
//! its centre, and a turn to the side while looking up or down also rolls the picture, as in a
//! game. The world wraps once around the cylinder (`2 pi f`). A host that fits this game learns
//! the same gains, for the screen's centre, and the focal length (scaled like them).
//!
//! With `--injected-only` the camera turns only for SendInput's moves (no device, as the
//! host's are), so a real mouse moved during a run does not disturb it. OUT.mouse.csv still
//! lists every move with its injected flag; its running sums, like frames.csv's, are the ones
//! the camera used, to which a device move then adds nothing.
//!
//! Over everything, the HUD included, the top left carries motion_probe's barcode, drawn as it
//! draws it, so nightjar-cli's reader (top left at scale 1, its defaults) decodes each picture:
//! four rows 24 pixels tall from y = 8, a black reference at x 0-23 and a white one at x 24-47,
//! and bit n of each row's word at x = 64 + 16n (32 bits, 1 white). Row 0 is the frame number,
//! the frames.csv `frame` of the present that showed it (1 for the first, never 0); rows 1 and
//! 2 are the low and high words of the QueryPerformanceCounter ticks read just before that
//! frame is drawn (its render_qpc_us is the same reading in microseconds); row 3 is 0xB17E2212.
//! It changes every frame, so the host never takes it for HUD. `--barcode off` leaves it out.
//!
//! ```text
//! look_probe \\.\DISPLAYn OUT.json [--seconds N] [--fps current|HZ] [--gain PX_PER_COUNT]
//!     [--lag-ms MS] [--stick-gain PX_PER_SECOND] [--stick-deadzone F]
//!     [--stick-exponent 1|1.5|2|2.5|3] [--hud on|off] [--barcode on|off] [--pad 0-3]
//!     [--injected-only] [--fov DEGREES]
//! ```
//!
//! Defaults: 60 seconds; presents at the display's refresh (a rate in Hz paces them as
//! motion_probe does); gain 1 pixel per count on x and y (negative inverts); lag 50 ms; stick
//! gain 1200 pixels per second of full deflection; dead zone 0.2; exponent 1.5; HUD on;
//! barcode on; pad 0; every relative move counts; no perspective (`--fov` takes 20 to 140). The
//! run ends after `--seconds`, or on Escape
//! from any keyboard (raw input, so also when the window is not focused).
//!
//! Written at the end: OUT.json (configuration, counts, the model a host should learn) and,
//! beside it with the extension replaced, OUT.frames.csv (a row per present: frame,
//! present_qpc_us, input_time_us, mouse_sum_x, mouse_sum_y, stick_integral_x,
//! stick_integral_y, camera_x, camera_y, render_qpc_us), OUT.mouse.csv (every relative move)
//! and OUT.stick.csv (every stick change). Times are QueryPerformanceCounter microseconds,
//! input_time_us is render_qpc_us less the lag, camera_x is unwrapped (the picture shows it
//! modulo the world width), and stick integrals are seconds of full deflection.

#[cfg_attr(not(windows), allow(dead_code))]
#[path = "look_probe/look.rs"]
mod look;
#[cfg(windows)]
#[path = "look_probe/runtime.rs"]
mod runtime;
#[cfg(windows)]
#[path = "look_probe/shader.rs"]
mod shader;

#[cfg(not(windows))]
fn main() {
    eprintln!("This probe requires Windows.");
}

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    probe::run()
}

#[cfg(windows)]
mod probe {
    use crate::look::{Camera, Config, MouseHistory, StickHistory, qpc_us, sibling};
    use crate::runtime::{
        DISPLAY_CHANGED, FrameRow, Handle, Input, Window, XInput, now_ticks, now_us, short,
        wait_pumping, window, write_csv,
    };
    use crate::shader::compile;
    use anyhow::{Context, Result, bail};
    use butterpollo_win_graphics::capture::{ComGuard, displays, enable_dpi_awareness};
    use std::{
        sync::atomic::Ordering,
        time::{Duration, Instant},
    };
    use windows::{
        Win32::{
            Foundation::*,
            Graphics::{
                Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST},
                Direct3D11::*,
                Dxgi::{Common::*, *},
            },
            System::{LibraryLoader::GetModuleHandleW, Performance::*, Threading::*},
            UI::{Input::*, WindowsAndMessaging::*},
        },
        core::{Interface, PCWSTR, w},
    };

    pub fn run() -> Result<()> {
        let config = Config::parse(std::env::args().skip(1))?;
        if let Some(parent) = config.out.parent()
            && !parent.as_os_str().is_empty()
            && !parent.is_dir()
        {
            bail!("output folder {} does not exist", parent.display());
        }
        enable_dpi_awareness();
        let _com = ComGuard::new()?;
        let active = displays()?;
        let names: Vec<_> = active.iter().map(|d| d.display_name.clone()).collect();
        let mut display = active
            .into_iter()
            .find(|d| d.display_name.eq_ignore_ascii_case(&config.display))
            .with_context(|| {
                format!(
                    "{} is not an active display (active: {})",
                    config.display,
                    names.join(", ")
                )
            })?;
        if display.width < 640 || display.height < 360 {
            bail!("the look probe needs a display of at least 640 by 360");
        }
        let source_mode = butterpollo_win_display::display::mode(&display.display_name)?;
        println!("LOOK stage=display name={}", display.display_name);
        let xinput = match XInput::load() {
            Ok(xinput) => Some(xinput),
            Err(error) => {
                println!("LOOK pad=unavailable error={error:#}");
                None
            }
        };
        let (width, height) = (display.width, display.height);
        // A lens's world goes once around the camera.
        let focal = config.fov.map(|fov| crate::look::focal(width, fov));
        let world_width = focal.map_or(f64::from(width) * 4.0, |f| std::f64::consts::TAU * f);
        let y_limit = f64::from(height) / 2.0;
        unsafe {
            let module = GetModuleHandleW(None)?;
            let class = WNDCLASSW {
                lpfnWndProc: Some(window),
                hInstance: HINSTANCE(module.0),
                lpszClassName: w!("ButterpolloLookProbe"),
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 {
                bail!("cannot register test window");
            }
            // Never activated, as motion_probe's: it must not take the focus from anything.
            let window = Window(CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                w!("ButterpolloLookProbe"),
                w!("Rubylight test look"),
                WS_POPUP | WS_VISIBLE,
                display.x,
                display.y,
                width as i32,
                height as i32,
                None,
                None,
                Some(HINSTANCE(module.0)),
                None,
            )?);
            SetWindowPos(
                window.0,
                Some(HWND_TOPMOST),
                display.x,
                display.y,
                width as i32,
                height as i32,
                SWP_NOACTIVATE,
            )?;
            println!("LOOK stage=window");
            let factory: IDXGIFactory2 = CreateDXGIFactory1()?;
            let adapter = factory.EnumAdapters1(display.adapter_index)?;
            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
            let device = device.context("no D3D11 device")?;
            let context = context.context("no D3D11 context")?;
            let swap = factory.CreateSwapChainForHwnd(
                &device,
                window.0,
                &DXGI_SWAP_CHAIN_DESC1 {
                    Width: width,
                    Height: height,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                    BufferCount: 2,
                    SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                    Scaling: DXGI_SCALING_STRETCH,
                    AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                    Flags: DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32,
                    ..Default::default()
                },
                None,
                None,
            )?;
            let swap: IDXGISwapChain3 = swap.cast()?;
            swap.SetMaximumFrameLatency(1)?;
            let latency = Handle(swap.GetFrameLatencyWaitableObject());
            if latency.0.is_invalid() {
                bail!("swap chain has no presentation event");
            }
            println!("LOOK stage=swap-chain");
            let texture: ID3D11Texture2D = swap.GetBuffer(0)?;
            let mut target = None;
            device.CreateRenderTargetView(&texture, None, Some(&mut target))?;
            let mut vertex = None;
            let mut picture = None;
            device.CreateVertexShader(
                &compile(b"vertex\0", b"vs_5_0\0")?,
                None,
                Some(&mut vertex),
            )?;
            device.CreatePixelShader(
                &compile(b"picture\0", b"ps_5_0\0")?,
                None,
                Some(&mut picture),
            )?;
            let mut constants = None;
            device.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: 48,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut constants),
            )?;
            let constants = constants.context("no constant buffer")?;
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            context.VSSetShader(vertex.as_ref(), None);
            context.PSSetShader(picture.as_ref(), None);
            context.PSSetConstantBuffers(0, Some(&[Some(constants.clone())]));
            context.RSSetViewports(Some(&[D3D11_VIEWPORT {
                Width: width as f32,
                Height: height as f32,
                MinDepth: 0.,
                MaxDepth: 1.,
                ..Default::default()
            }]));
            // Paces a fixed rate while still handling input.
            let timer = Handle(
                CreateWaitableTimerExW(
                    None,
                    PCWSTR::null(),
                    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                    TIMER_ALL_ACCESS.0,
                )
                .or_else(|_| CreateWaitableTimerExW(None, PCWSTR::null(), 0, TIMER_ALL_ACCESS.0))?,
            );
            let mut frequency = 0;
            QueryPerformanceFrequency(&mut frequency)?;
            // Mouse and keyboard, whether or not the window has the focus.
            let devices = [2, 6].map(|usage| RAWINPUTDEVICE {
                usUsagePage: 1,
                usUsage: usage,
                dwFlags: RIDEV_INPUTSINK,
                hwndTarget: window.0,
            });
            RegisterRawInputDevices(&devices, size_of::<RAWINPUTDEVICE>() as u32)
                .context("cannot register for raw input")?;
            println!("LOOK stage=render-ready");

            let start = Instant::now();
            let start_us = now_us(frequency);
            let mut input = Input {
                frequency,
                mouse: MouseHistory::new(config.injected_only),
                injected: 0,
                device: 0,
                absolute_ignored: 0,
                errors: 0,
                escape: false,
            };
            let mut stick =
                StickHistory::new(start_us, config.stick_deadzone, config.stick_exponent);
            let mut camera = Camera::new(config.gain, config.stick_gain, y_limit);
            let period = config
                .fps
                .map(|hz| Duration::from_secs_f64(1.0 / f64::from(hz)));
            let duration = Duration::from_secs(config.seconds);
            let mut frames = Vec::new();
            let mut next_frame = start;
            let mut progress_at = start + Duration::from_secs(5);
            let (mut pad_connected, mut pad_present) = (false, false);
            let mut pad_retry_at = start;
            let mut exit = "time";
            while start.elapsed() < duration {
                if !wait_pumping(latency.0, 1000, &mut input)? {
                    bail!("presentation event timed out");
                }
                if let Some(period) = period {
                    let remaining = next_frame.saturating_duration_since(Instant::now());
                    if !remaining.is_zero() {
                        // Relative, in 100 ns units.
                        let due = -(i64::try_from(remaining.as_nanos() / 100)
                            .unwrap_or(i64::MAX)
                            .max(1));
                        SetWaitableTimer(timer.0, &due, 0, None, None, false)?;
                        if !wait_pumping(timer.0, 5000, &mut input)? {
                            bail!("frame timer timed out");
                        }
                    }
                    // Bound catch-up after a stall to one frame, as motion_probe does.
                    next_frame = (next_frame + period).max(Instant::now());
                }
                input.pump()?;
                if input.escape {
                    exit = "escape";
                    break;
                }
                if DISPLAY_CHANGED.swap(false, Ordering::Relaxed)
                    && let Some(current) = displays()?
                        .into_iter()
                        .find(|d| d.display_name == display.display_name)
                {
                    if (current.width, current.height) != (width, height) {
                        bail!("the look probe's display changed size");
                    }
                    // A layout change can move the output without moving this popup.
                    if (current.x, current.y) != (display.x, display.y) {
                        SetWindowPos(
                            window.0,
                            None,
                            current.x,
                            current.y,
                            0,
                            0,
                            SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOSIZE,
                        )?;
                        display = current;
                    }
                }
                // The stick, polled once a frame as a game does. Polling a missing pad is
                // slow, so that is retried once a second.
                if let Some(xinput) = &xinput
                    && (pad_connected || Instant::now() >= pad_retry_at)
                {
                    let polled = xinput.right_stick(config.pad);
                    let at_us = now_us(frequency);
                    if polled.is_some() != pad_connected {
                        let state = if polled.is_some() {
                            "connected"
                        } else {
                            "missing"
                        };
                        println!("LOOK pad={} {state}", config.pad);
                    }
                    pad_connected = polled.is_some();
                    pad_present |= pad_connected;
                    match polled {
                        Some((packet, right)) => stick.push(at_us, Some(packet), right),
                        None => {
                            pad_retry_at = Instant::now() + Duration::from_secs(1);
                            stick.push(at_us, None, (0, 0));
                        }
                    }
                }
                // One reading for both the barcode's ticks and render_qpc_us.
                let render_ticks = now_ticks();
                let render_us = qpc_us(render_ticks, frequency);
                let input_us = render_us - config.lag_us;
                let mouse_sum = input.mouse.at(input_us);
                let integral = stick.integral(input_us);
                camera.update(mouse_sum, integral);
                // The barcode's sequence: this row's `frame` in frames.csv, from 1.
                let frame = frames.len() as u32 + 1;
                let values = [
                    (camera.x.rem_euclid(world_width) as f32).to_bits(),
                    (camera.y as f32).to_bits(),
                    (world_width as f32).to_bits(),
                    (width as f32).to_bits(),
                    (height as f32).to_bits(),
                    u32::from(config.hud),
                    frame,
                    u32::from(config.barcode),
                    render_ticks as u32,
                    (render_ticks as u64 >> 32) as u32,
                    (focal.unwrap_or(0.0) as f32).to_bits(),
                    0,
                ];
                context.UpdateSubresource(&constants, 0, None, values.as_ptr().cast(), 0, 0);
                context.OMSetRenderTargets(Some(&[target.clone()]), None);
                context.Draw(3, 0);
                swap.Present(1, DXGI_PRESENT(0)).ok()?;
                let present_us = now_us(frequency);
                if frame == 1 {
                    println!("LOOK stage=first-present");
                }
                frames.push(FrameRow {
                    render_us,
                    present_us,
                    input_us,
                    mouse: mouse_sum,
                    stick: integral,
                    camera: [camera.x, camera.y],
                });
                if Instant::now() >= progress_at {
                    println!(
                        "LOOK_PROGRESS frames={frame} seconds={:.3} mouse_packets={}",
                        start.elapsed().as_secs_f64(),
                        input.injected + input.device
                    );
                    progress_at = Instant::now() + Duration::from_secs(5);
                }
            }
            let end_us = now_us(frequency);
            let seconds = start.elapsed().as_secs_f64();

            let frames_path = sibling(&config.out, "frames");
            let mouse_path = sibling(&config.out, "mouse");
            let stick_path = sibling(&config.out, "stick");
            write_csv(
                &frames_path,
                "frame,present_qpc_us,input_time_us,mouse_sum_x,mouse_sum_y,stick_integral_x,stick_integral_y,camera_x,camera_y,render_qpc_us",
                |out| {
                    for (i, f) in frames.iter().enumerate() {
                        writeln!(
                            out,
                            "{},{},{},{},{},{:.6},{:.6},{:.4},{:.4},{}",
                            i + 1,
                            f.present_us,
                            f.input_us,
                            f.mouse[0],
                            f.mouse[1],
                            f.stick[0],
                            f.stick[1],
                            f.camera[0],
                            f.camera[1],
                            f.render_us
                        )?;
                    }
                    Ok(())
                },
            )?;
            write_csv(&mouse_path, "qpc_us,dx,dy,sum_x,sum_y,injected", |out| {
                for m in &input.mouse.moves {
                    let injected = u8::from(m.injected);
                    writeln!(
                        out,
                        "{},{},{},{},{},{injected}",
                        m.at_us, m.dx, m.dy, m.sum[0], m.sum[1]
                    )?;
                }
                Ok(())
            })?;
            write_csv(
                &stick_path,
                "qpc_us,connected,packet,right_x,right_y,curve_x,curve_y,integral_x,integral_y",
                |out| {
                    for s in &stick.samples {
                        writeln!(
                            out,
                            "{},{},{},{},{},{:.6},{:.6},{:.6},{:.6}",
                            s.at_us,
                            u8::from(s.packet.is_some()),
                            s.packet.map_or(String::new(), |p| p.to_string()),
                            s.stick.0,
                            s.stick.1,
                            s.curve[0],
                            s.curve[1],
                            s.integral[0],
                            s.integral[1]
                        )?;
                    }
                    Ok(())
                },
            )?;
            let mouse_packets = input.injected + input.device;
            let report = serde_json::json!({
                "probe": "look_probe",
                "scope": "fake mouse-look game: camera = gain * mouse_sum(t - lag) + stick_gain * integral of stick_curve up to t - lag, t the frame's render time",
                "config": {
                    "display": config.display,
                    "seconds": config.seconds,
                    "fps": config.fps.map_or(serde_json::json!("current"), |hz| serde_json::json!(hz)),
                    "gain": config.gain,
                    "lag_ms": config.lag_us as f64 / 1000.0,
                    "stick_gain": config.stick_gain,
                    "stick_deadzone": short(config.stick_deadzone),
                    "stick_exponent": short(config.stick_exponent),
                    "hud": config.hud,
                    "barcode": config.barcode,
                    "pad": config.pad,
                    "injected_only": config.injected_only,
                    "fov": config.fov,
                },
                "display": display,
                "display_refresh_hz": source_mode.dmDisplayFrequency,
                "world_width": world_width,
                "camera_y_limit": y_limit,
                "qpc_frequency": frequency,
                "start_qpc_us": start_us,
                "end_qpc_us": end_us,
                "seconds": seconds,
                "exit": exit,
                "counts": {
                    "frames": frames.len(),
                    "mouse_packets": mouse_packets,
                    "mouse_injected_packets": input.injected,
                    "mouse_device_packets": input.device,
                    "absolute_packets_ignored": input.absolute_ignored,
                    "raw_input_errors": input.errors,
                    "pad_present": pad_present,
                    "stick_samples": stick.samples.len() - 1,
                },
                "expected_host_model": {
                    "mouse_gain": [-config.gain, -config.gain],
                    "stick_gain": [-config.stick_gain, -config.stick_gain],
                    "stick_deadzone": short(config.stick_deadzone),
                    "stick_exponent": short(config.stick_exponent),
                    "lag_us_at_least": config.lag_us,
                    "focal": focal,
                    "note": "content shift per input, display pixels, x right and y down, at the screen's centre; focal is the camera's focal length in display pixels, null for a flat world; scale by stream width over display width; the host's lag adds the render-to-capture delay",
                },
                "files": {
                    "frames": frames_path.display().to_string(),
                    "mouse": mouse_path.display().to_string(),
                    "stick": stick_path.display().to_string(),
                },
            });
            std::fs::write(&config.out, serde_json::to_vec_pretty(&report)?)
                .with_context(|| format!("cannot write {}", config.out.display()))?;
            let last = frames.last().map_or([0.0; 2], |f| f.camera);
            println!(
                "LOOK frames={} seconds={seconds:.3} mouse_packets={mouse_packets} injected={} device={} absolute_ignored={} pad={} exit={exit} camera_x={:.1} camera_y={:.1} out={}",
                frames.len(),
                input.injected,
                input.device,
                input.absolute_ignored,
                if pad_present { "present" } else { "absent" },
                last[0],
                last[1],
                config.out.display()
            );
        }
        Ok(())
    }
}
