//! Signed-in user-session WGC worker process loop and helper failure formatting.

use super::*;
use crate::user_helper::{HelperPipe, HelperPipeExt, UserChild, UserLauncher};
use anyhow::{Context, Result, bail, ensure};
use butterpollo_media_policy::{RecordedWarnings, WarningSink};
use windows::core::Interface;
use windows::{
    Graphics::Capture::Direct3D11CaptureFrame,
    Win32::{
        Foundation::HANDLE,
        System::{
            Threading::{GetCurrentProcess, HIGH_PRIORITY_CLASS, SetPriorityClass},
            WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess,
        },
    },
};

pub(super) struct NativeFrame(Direct3D11CaptureFrame);
impl Drop for NativeFrame {
    fn drop(&mut self) {
        let _ = self.0.Close();
    }
}
pub(super) fn frame_for_slot<T>(
    pending: &mut Option<T>,
    newest: Option<T>,
    available: bool,
) -> Option<T> {
    if let Some(frame) = newest {
        *pending = Some(frame);
    }
    if available { pending.take() } else { None }
}
pub(super) fn next_native(capture: &mut Wgc) -> Result<Option<NativeFrame>> {
    capture.check_color_space()?;
    capture
        .next_native_frame()
        .map(|frame| frame.map(NativeFrame))
        .map_err(
            |error| match crate::device_loss::DeviceLost::d3d11(&capture.gpu.device) {
                Some(loss) => error.context(loss),
                None => error,
            },
        )
}
pub(super) fn helper_error(message: String, reason: Option<i32>) -> anyhow::Error {
    let error = anyhow::anyhow!("WGC helper: {message}");
    match reason {
        Some(reason) => error.context(crate::device_loss::DeviceLost(reason)),
        None => error,
    }
}
/// Why the helper's textures did not open: its own error when it ended
/// meanwhile. A game window resized or closed just after the first frame
/// ends the helper and its handles with it, and only "Access is denied" or
/// "The handle is invalid" was left to say so (#56).
pub(super) fn startup_failure(
    error: anyhow::Error,
    pipe: &dyn HelperPipe,
    process: &dyn UserChild,
) -> anyhow::Error {
    let mut replies = Vec::new();
    while replies.len() <= 2 * SLOTS {
        match pipe.receive_json::<Reply>() {
            Ok(Some(reply)) => replies.push(reply),
            _ => break,
        }
    }
    let exited = process.exit_code().ok().flatten().is_some();
    helper_ended(error, replies, exited)
}
pub(super) fn helper_ended(
    error: anyhow::Error,
    replies: Vec<Reply>,
    exited: bool,
) -> anyhow::Error {
    let said = replies.into_iter().find_map(|reply| match reply {
        Reply::Error {
            message,
            device_removed,
        } => Some(helper_error(message, device_removed)),
        _ => None,
    });
    match said {
        Some(said) => said,
        None if exited => error.context("WGC helper exited before its textures were opened"),
        None => error.context("WGC helper textures did not open"),
    }
}
pub(super) fn native_texture(frame: &NativeFrame) -> Result<ID3D11Texture2D> {
    let access: IDirect3DDxgiInterfaceAccess = frame.0.Surface()?.cast()?;
    // SAFETY: `access` is the surface of a frame that `frame` keeps open for this call.
    Ok(unsafe { access.GetInterface()? })
}

/// Internal entry point. It runs before the host opens credentials, listeners,
/// virtual displays or the tray, and never needs administrator privileges.
pub fn run_worker_with(pipe: &str, parent: u32, launcher: &dyn UserLauncher) -> Result<()> {
    // The host sets this too, but the helper may be started on its own (a
    // test, or a future caller): the window's frame and client rectangle are
    // compared in physical pixels, so the crop does not drift at a scaled DPI.
    enable_dpi_awareness();
    let pipe = launcher.pipe_client(pipe, parent, PIPE_PREFIX)?;
    let result = worker(&*pipe);
    if let Err(error) = &result {
        let _ = pipe.send_json(&Reply::Error {
            message: format!("{error:#}").chars().take(512).collect(),
            device_removed: crate::device_loss::DeviceLost::from_error(error).map(|loss| loss.0),
        });
        // Give the parent a chance to read the failure before closing its last
        // client handle; never wait indefinitely for a vanished host.
        std::thread::sleep(Duration::from_millis(20));
    }
    result
}
fn worker(pipe: &dyn HelperPipe) -> Result<()> {
    ensure!(
        !crate::process_info::is_system(),
        "WGC helper must run as the signed-in user"
    );
    let deadline = Instant::now() + START_TIMEOUT;
    let (name, hdr, config, wake, window) = loop {
        match pipe.receive_json::<Request>()? {
            Some(Request::Start {
                version,
                name,
                hdr,
                config,
                wake,
                window,
            }) => {
                ensure!(version == VERSION, "unsupported WGC protocol version");
                // The host duplicated it into this process; closed on exit.
                let wake = owned(HANDLE(usize::try_from(wake)? as *mut _));
                let window = window.map(usize::try_from).transpose()?;
                break (name, hdr, capture_config(&config), wake, window);
            }
            Some(Request::Stop) => return Ok(()),
            Some(_) => bail!("unexpected WGC startup command"),
            None => {}
        }
        ensure!(
            Instant::now() < deadline,
            "WGC parent did not initialize capture"
        );
        std::thread::sleep(Duration::from_millis(2));
    };
    let _com = ComGuard::new()?;
    let _priority = Priority::new();
    // Beside busy game-like threads, normal class held capture near 40 of 60
    // fps (2.2.9 notes). 2.5.6 dropped this and loaded streams regressed (#42).
    // SAFETY: The current-process pseudo handle needs no closing; only this helper is affected.
    if let Err(error) = unsafe { SetPriorityClass(GetCurrentProcess(), HIGH_PRIORITY_CLASS) } {
        tracing::debug!(%error, "WGC helper high process priority unavailable");
    }
    crate::timing::disable_power_throttling();
    let _streaming = config
        .boolean("wgc_helper_streaming_scope", false)
        .then(crate::timing::StreamingScope::enter);
    let timer = crate::timing::Timer::new()?;
    let (adapter_name, pnp_id) = crate::capture::stream_adapter(
        Some(name.as_str()),
        config.get("adapter_name", ""),
        config.get("adapter_pnp_id", ""),
    );
    let gpu = Device::for_display(&name, &adapter_name, &pnp_id)?;
    ensure!(
        gpu.display.display_name == name,
        "WGC output disappeared before helper startup"
    );
    let _ = crate::gpu_priority::configure(&gpu, &config);
    let client = super::super::window_client(&config);
    let mut capture = Wgc::new_device(
        gpu.clone(),
        hdr,
        config.boolean("wgc_high_rate_capture", false),
        window,
        super::super::window_cursor(&config),
        client,
    )?;
    capture.drain_to_newest = config.boolean("wgc_drain_to_newest", false);
    if let Err(error) = capture.enable_notifications() {
        tracing::debug!(%error, "WGC idle notifications unavailable; retaining capture polling");
    }
    // WGC delivers a frame when the desktop is composed, and a static desktop
    // may not be for seconds: the host then gave up on WGC for the whole
    // session (Desktop Duplication paces worse). A repaint of every window
    // composes one; nothing visible changes.
    let mut repaint = Instant::now() + Duration::from_millis(33);
    let mut first = loop {
        if let Some(frame) = next_native(&mut capture)? {
            break Some(frame);
        }
        if Instant::now() >= repaint {
            repaint = Instant::now() + Duration::from_millis(500);
            // SAFETY: RedrawWindow takes no pointers here and only queues repaints.
            unsafe {
                use windows::Win32::Graphics::Gdi::{
                    RDW_ALLCHILDREN, RDW_INVALIDATE, RedrawWindow,
                };
                let _ = RedrawWindow(None, None, None, RDW_INVALIDATE | RDW_ALLCHILDREN);
            }
        }
        if let Some(command) = pipe.receive_json::<Request>()? {
            ensure!(
                matches!(command, Request::Stop),
                "unexpected WGC command before the first frame"
            );
            return Ok(());
        }
        if let Some(window) = window {
            super::super::window::check_awaited(window)?;
        }
        ensure!(
            Instant::now() < deadline,
            "Windows Graphics Capture produced no initial frame"
        );
        let until = (Instant::now() + Duration::from_millis(10)).min(repaint);
        if let Some((arrived, _)) = &capture.notifications {
            timer.until_or_signal(until, arrived)?;
        } else {
            timer.until(Instant::now() + Duration::from_micros(500));
        }
    };
    // The shared textures below keep the first frame's size: a game window
    // resized after it ends this helper, and the host reopens at the new size.
    capture.follow_resize = false;
    let source = native_texture(first.as_ref().unwrap())?;
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: `source` is a live texture and `desc` a live local.
    unsafe {
        source.GetDesc(&mut desc);
    }
    desc.Usage = D3D11_USAGE_DEFAULT;
    desc.BindFlags = D3D11_BIND_SHADER_RESOURCE.0 as u32;
    desc.CPUAccessFlags = 0;
    desc.MiscFlags =
        (D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0 | D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0) as u32;
    validate_texture(&desc, desc.Width, desc.Height, desc.Format.0)?;
    // `capture_window_frame = client`: crop the window's title bar and
    // borders away in both copies below. The shared textures take the
    // client's size, so a later change ends the helper like a resize and the
    // host reopens it at the new size.
    let crop = if client {
        window.and_then(|window| {
            super::super::app_window::window_client_box(window, (desc.Width, desc.Height))
        })
    } else {
        None
    };
    let cropped = crop.is_some();
    if let Some(crop) = crop {
        desc.Width = crop.width();
        desc.Height = crop.height();
    }
    let compute_warnings = Arc::new(RecordedWarnings::default());
    let compute_sink: Arc<dyn WarningSink> = compute_warnings.clone();
    let mut copy = handoff(&gpu, &config, &compute_sink);
    let mut textures = Vec::with_capacity(SLOTS);
    let mut handles = Vec::with_capacity(SLOTS);
    for _ in 0..SLOTS {
        // SAFETY: `gpu.device` is live and `desc` was validated above; each shared handle is owned
        // by `handles`.
        unsafe {
            let mut texture = None;
            gpu.device
                .CreateTexture2D(&desc, None, Some(&mut texture))?;
            let texture = texture.context("WGC shared texture missing")?;
            let handle = owned(texture.cast::<IDXGIResource1>()?.CreateSharedHandle(
                None,
                DXGI_SHARED_RESOURCE_READ.0 | DXGI_SHARED_RESOURCE_WRITE.0,
                None,
            )?);
            if let Some(compute) = &mut copy
                && compute.import_shared(&texture, raw(&handle)).is_err()
            {
                copy = None;
            }
            textures.push(Texture {
                mutex: texture.cast()?,
                texture: Arc::new(texture),
            });
            handles.push(handle);
        }
    }
    pipe.send_json(&Reply::Ready {
        version: VERSION,
        handles: std::array::from_fn(|index| raw(&handles[index]).0 as u64),
        width: desc.Width,
        height: desc.Height,
        format: desc.Format.0,
        compute: copy.is_some(),
    })?;
    for warning in compute_warnings.snapshot() {
        pipe.send_json(&Reply::ComputeFallback {
            message: warning.message,
        })?;
    }

    let mut slots = Slots::default();
    let mut pending = None;
    let mut last_frame = Instant::now();
    loop {
        for _ in 0..=SLOTS {
            match pipe.receive_json::<Request>()? {
                Some(Request::Stop) => return Ok(()),
                Some(Request::Release { slot }) => slots.release(slot)?,
                Some(Request::Ping) => pipe.send_json(&Reply::Alive)?,
                Some(Request::Start { .. }) => bail!("WGC cannot replace a live session"),
                None => break,
            }
        }
        let frame = if first.is_some() {
            first.take()
        } else {
            next_native(&mut capture)?
        };
        if frame.is_some() {
            last_frame = Instant::now();
        }
        if frame.is_some() || pending.is_some() {
            let mut chosen = None;
            for (slot, texture) in textures.iter().enumerate() {
                if !slots.busy[slot] && acquire(&texture.mutex, 0)? {
                    chosen = Some(slot);
                    break;
                }
            }
            if let Some(frame) = frame_for_slot(&mut pending, frame, chosen.is_some()) {
                let slot = chosen.unwrap();
                let texture = &textures[slot];
                let result = (|| -> Result<()> {
                    let source = native_texture(&frame)?;
                    let mut source_desc = D3D11_TEXTURE2D_DESC::default();
                    // SAFETY: `source` is live and `source_desc` a live local.
                    unsafe {
                        source.GetDesc(&mut source_desc);
                    }
                    let frame_crop = if client {
                        window.and_then(|window| {
                            super::super::app_window::window_client_box(
                                window,
                                (source_desc.Width, source_desc.Height),
                            )
                        })
                    } else {
                        None
                    };
                    // The shared textures keep the first frame's size: a
                    // client area that changes size needs the helper reopened,
                    // as a window resize does.
                    let crop = if cropped {
                        let Some(crop) = frame_crop else {
                            bail!(
                                "the game window's content is unavailable; reopening its capture"
                            );
                        };
                        ensure!(
                            (crop.width(), crop.height()) == (desc.Width, desc.Height),
                            "the game window's content changed size from {}x{} to {}x{}; reopening its capture",
                            desc.Width,
                            desc.Height,
                            crop.width(),
                            crop.height()
                        );
                        Some(crop)
                    } else {
                        if let Some(crop) = frame_crop
                            && (crop.width(), crop.height()) != (desc.Width, desc.Height)
                        {
                            bail!(
                                "the game window's content is now visible; reopening its capture"
                            );
                        }
                        ensure!(
                            (source_desc.Width, source_desc.Height) == (desc.Width, desc.Height),
                            "the captured surface changed size from {}x{} to {}x{}; reopening its capture",
                            desc.Width,
                            desc.Height,
                            source_desc.Width,
                            source_desc.Height
                        );
                        frame_crop
                    };
                    if let Some(compute) = &mut copy
                        && let Err(error) = compute.copy(&texture.texture, &source, crop)
                    {
                        if let Some(loss) = crate::device_loss::DeviceLost::d3d11(&gpu.device)
                            .or_else(|| compute.device_removed())
                            .or_else(|| crate::device_loss::DeviceLost::from_error(&error))
                        {
                            return Err(error.context(loss));
                        }
                        copy = None;
                        pipe.send_json(&Reply::ComputeFallback {
                            message: format!("{error:#}").chars().take(512).collect(),
                        })?;
                    }
                    if copy.is_none() {
                        // SAFETY: Both textures live on `gpu.device`, and this process holds
                        // `texture`'s keyed mutex (key 0).
                        unsafe {
                            if let Some(crop) = crop {
                                let box_ = crop.d3d11();
                                gpu.context.CopySubresourceRegion(
                                    &*texture.texture,
                                    0,
                                    0,
                                    0,
                                    0,
                                    &source,
                                    0,
                                    Some(&box_),
                                );
                            } else {
                                gpu.context.CopyResource(&*texture.texture, &source);
                            }
                        }
                    }
                    Ok(())
                })();
                // Release is queued after the compute handoff's context wait.
                // Thus the host sees complete pixels even under GPU load.
                // SAFETY: This process acquired the keyed mutex with key 0 above, and the context
                // is this device's.
                unsafe {
                    texture.mutex.ReleaseSync(1)?;
                    gpu.context.Flush();
                }
                result?;
                let sequence = slots
                    .sequence
                    .checked_add(1)
                    .context("WGC sequence exhausted")?;
                slots.publish(slot, sequence)?;
                let relative = frame.0.SystemRelativeTime()?.Duration;
                let qpc = wgc_qpc(relative, qpc_frequency());
                pipe.send_json(&Reply::Frame {
                    slot,
                    sequence,
                    qpc,
                    focus: capture.take_focus(),
                })?;
                // SAFETY: `wake` is an event handle this process owns until `worker` returns.
                unsafe {
                    let _ = windows::Win32::System::Threading::SetEvent(raw(&wake));
                }
            }
            // Keep only the newest unsent update so a desktop that becomes
            // static still reaches the host when a slot returns. One retained
            // native frame leaves the other pool entry available for updates.
        }
        let poll = Duration::from_micros(500);
        if let Some((arrived, _)) = &capture.notifications {
            let interval =
                notification_wait_interval(last_frame.elapsed(), pending.is_some(), poll);
            timer.until_or_signal(Instant::now() + interval, arrived)?;
        } else {
            timer.until(Instant::now() + poll);
        }
    }
}
