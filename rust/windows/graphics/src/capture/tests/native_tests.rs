//! Window-target refusal tests and hardware-gated (`#[ignore]`) D3D11/WGC/DDX capture tests.

use super::*;

#[test]
#[ignore = "opens brief WGC sessions; requires Windows MinUpdateInterval support"]
fn wgc_low_rate_explicitly_disables_the_windows_capture_throttle() -> Result<()> {
    enable_dpi_awareness();
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    for (high_rate, ticks) in [(false, 0), (true, 10_000), (false, 0)] {
        let capture = Wgc::new_device(gpu.clone(), false, high_rate, None, false, false)?;
        assert_eq!(capture.session.MinUpdateInterval()?.Duration, ticks);
    }
    Ok(())
}

#[test]
#[ignore = "requires an interactive Desktop Duplication output"]
fn ddx_snapshots_survive_reacquisition_and_duplication_teardown() -> Result<()> {
    enable_dpi_awareness();
    let _display_awake = crate::timing::DisplayAwake::enter()?;
    let _com = ComGuard::new()?;
    let timer = crate::timing::Timer::new()?;
    for _ in 0..3 {
        let mut capture = Duplication::new("")?;
        let deadline = Instant::now() + Duration::from_secs(3);
        let image = loop {
            if let Some(image) = capture.next_gpu(0)? {
                break image;
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "DDX produced no initial snapshot"
            );
            timer.until(Instant::now() + Duration::from_millis(1));
        };
        let original = image.readback(&mut None)?;
        // Keep the original owned snapshot through new acquisitions and a
        // CPU/GPU transition. Nothing is written to disk or the display.
        capture.next(Duration::ZERO)?;
        for _ in 0..24 {
            capture.next_gpu(0)?;
            timer.until(Instant::now() + Duration::from_millis(1));
        }
        drop(capture);
        assert_eq!(image.readback(&mut None)?.bytes, original.bytes);
    }
    Ok(())
}

#[test]
#[ignore = "requires native D3D11 texture copies and readback"]
fn pointer_only_snapshots_reuse_pixels_and_missed_desktop_updates_invalidate_them() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let upload = |value| {
        GpuImage::upload(
            &gpu,
            &Image {
                width: 4,
                height: 4,
                stride: 16,
                bytes: vec![value; 64],
                captured: Instant::now(),
                pixel: Pixel::Bgra8,
            },
        )
    };
    let first_source = upload(32)?;
    let second_source = upload(128)?;
    let third_source = upload(224)?;
    let mut pool = GpuPool::default();
    let mut cached = None;
    let first = pool
        .desktop_snapshot(&gpu, &first_source.texture, &mut cached, true)?
        .unwrap();
    let pointer = pool
        .desktop_snapshot(&gpu, &second_source.texture, &mut cached, false)?
        .unwrap();
    assert!(std::sync::Arc::ptr_eq(&first.texture, &pointer.texture));
    let second = pool
        .desktop_snapshot(&gpu, &second_source.texture, &mut cached, true)?
        .unwrap();
    assert!(!std::sync::Arc::ptr_eq(&first.texture, &second.texture));
    assert_eq!(first.readback(&mut None)?.bytes, vec![32; 64]);
    assert_eq!(second.readback(&mut None)?.bytes, vec![128; 64]);
    let mut held = vec![first, pointer, second];
    for _ in 0..6 {
        held.push(
            pool.desktop_snapshot(&gpu, &second_source.texture, &mut cached, true)?
                .unwrap(),
        );
    }
    assert!(
        pool.desktop_snapshot(&gpu, &third_source.texture, &mut cached, true)?
            .is_none()
    );
    assert!(cached.is_none());
    held.clear();
    let recovered = pool
        .desktop_snapshot(&gpu, &third_source.texture, &mut cached, false)?
        .unwrap();
    assert_eq!(recovered.readback(&mut None)?.bytes, vec![224; 64]);
    Ok(())
}

#[test]
#[ignore = "requires native D3D11 textures"]
fn an_inherited_picture_keeps_the_repeat_and_origin_of_its_source() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let upload = |value| {
        GpuImage::upload(
            &gpu,
            &Image {
                width: 4,
                height: 4,
                stride: 16,
                bytes: vec![value; 64],
                captured: Instant::now(),
                pixel: Pixel::Bgra8,
            },
        )
    };
    let mut source = upload(32)?;
    source.repeat = true;
    source.origin = 7;
    let mut converted = upload(128)?;
    converted.inherit(&source);
    assert!(converted.repeat);
    assert_eq!(converted.origin, 7);
    Ok(())
}

#[test]
#[ignore = "requires native D3D11/D3D12 sharing and readback"]
fn unshareable_frame_falls_back_without_waiting_for_another_desktop_update() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let expected = [32, 64, 128, 255].repeat(16);
    let uploaded = GpuImage::upload(
        &gpu,
        &Image {
            width: 4,
            height: 4,
            stride: 16,
            bytes: expected.clone(),
            captured: Instant::now(),
            pixel: Pixel::Bgra8,
        },
    )?;
    // A normal D3D11-only snapshot cannot be opened by the compute queue.
    let source = GpuPool::default()
        .copy(&gpu, &uploaded.texture, None)?
        .unwrap();
    assert!(!crate::compute::shareable(&source.texture));
    let mut pool = GpuPool {
        compute: Some(crate::compute::Handoff::new(
            crate::compute::Compute::for_device(&gpu.device)?,
            &gpu,
        )?),
        ..Default::default()
    };
    let frame = pool
        .copy(&gpu, &source.texture, None)?
        .context("fallback must return this frame, even if the desktop never updates again")?;
    assert!(pool.compute.is_none());
    assert!(!crate::compute::shareable(&frame.texture));
    assert!(frame.ready.is_none());
    drop(pool);
    assert_eq!(frame.readback(&mut None)?.bytes, expected);
    Ok(())
}

#[test]
#[ignore = "requires native D3D11 texture copies and readback"]
fn a_window_crop_copies_only_the_client_area() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    // Each pixel records its source position: (row, column) in blue and green.
    let mut bytes = vec![0u8; 4 * 4 * 4];
    for y in 0..4 {
        for x in 0..4 {
            let at = (y * 4 + x) * 4;
            bytes[at..at + 4].copy_from_slice(&[y as u8, x as u8, 0, 255]);
        }
    }
    let source = GpuImage::upload(
        &gpu,
        &Image {
            width: 4,
            height: 4,
            stride: 16,
            bytes,
            captured: Instant::now(),
            pixel: Pixel::Bgra8,
        },
    )?;
    let crop = client_box(
        windows::Win32::Foundation::RECT {
            left: 0,
            top: 0,
            right: 4,
            bottom: 4,
        },
        windows::Win32::Foundation::POINT { x: 1, y: 1 },
        (2, 2),
        (4, 4),
    );
    let snapshot = GpuPool::default()
        .copy(&gpu, &source.texture, crop)?
        .context("copy dropped")?;
    assert_eq!((snapshot.width, snapshot.height), (2, 2));
    let read = snapshot.readback(&mut None)?;
    assert_eq!((read.width, read.height), (2, 2));
    assert_eq!(&read.bytes[0..4], &[1, 1, 0, 255]);
    assert_eq!(&read.bytes[4..8], &[1, 2, 0, 255]);
    assert_eq!(&read.bytes[8..12], &[2, 1, 0, 255]);
    assert_eq!(&read.bytes[12..16], &[2, 2, 0, 255]);
    Ok(())
}

#[test]
#[ignore = "requires native D3D11 texture copies and readback"]
fn a_source_description_is_read_once_and_keeps_working_across_sizes() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let upload = |width: u32, height: u32| {
        GpuImage::upload(
            &gpu,
            &Image {
                width,
                height,
                stride: (width * 4) as usize,
                bytes: vec![0u8; (width * height * 4) as usize],
                captured: Instant::now(),
                pixel: Pixel::Bgra8,
            },
        )
    };
    let small = upload(4, 4)?;
    let large = upload(8, 8)?;
    let mut pool = GpuPool::default();
    // The same source texture is described once and then copied from the
    // remembered description.
    let first = pool.copy(&gpu, &small.texture, None)?.unwrap();
    assert_eq!((first.width, first.height), (4, 4));
    let second = pool.copy(&gpu, &small.texture, None)?.unwrap();
    assert_eq!((second.width, second.height), (4, 4));
    // A different source size still replaces the pool's textures.
    let resized = pool.copy(&gpu, &large.texture, None)?.unwrap();
    assert_eq!((resized.width, resized.height), (8, 8));
    // And the first source, described before the resize, still copies.
    let again = pool.copy(&gpu, &small.texture, None)?.unwrap();
    assert_eq!((again.width, again.height), (4, 4));
    Ok(())
}

#[test]
#[ignore = "requires a moving desktop, WGC and native AMD D3D12 sharing"]
fn wgc_compute_snapshots_match_d3d11_during_motion() -> Result<()> {
    enable_dpi_awareness();
    let _com = ComGuard::new()?;
    let _awake = crate::timing::DisplayAwake::enter()?;
    let mut capture = Capture::new_options("", "wgc", false, &Default::default())?;
    let Capture::Wgc(wgc) = &mut capture else {
        bail!("expected WGC");
    };
    anyhow::ensure!(wgc.owned.compute.is_some(), "compute copies unavailable");
    let deadline = Instant::now() + Duration::from_secs(8);
    let (mut checked, mut changed) = (0, 0);
    let mut previous = Vec::new();
    let mut held = None;
    let mut reference_staging = None;
    let mut actual_staging = None;
    while checked < 120 && Instant::now() < deadline {
        let Some(frame) = wgc.try_frame()? else {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        };
        let result = (|| -> Result<()> {
            wgc.check_size(&frame)?;
            let surface = frame.Surface()?;
            let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
            // SAFETY: `access` is the surface of `frame`, which is closed only after this closure.
            let source: ID3D11Texture2D = unsafe { access.GetInterface()? };
            let snapshot = wgc
                .owned
                .copy(&wgc.gpu, &source, None)?
                .context("copy dropped")?;
            anyhow::ensure!(snapshot.ready.is_some(), "compute fell back to graphics");
            // Read the candidate first. Reading the source before submitting
            // the copy would hide missing producer/consumer synchronization.
            let actual = snapshot.readback(&mut actual_staging)?;
            let reference = read_texture(&wgc.gpu, &source, &mut reference_staging)?;
            anyhow::ensure!(
                actual.bytes == reference.bytes,
                "WGC compute copy differed on frame {checked}"
            );
            if !previous.is_empty() && previous != reference.bytes {
                changed += 1;
            }
            if held.is_none() {
                held = Some((snapshot, reference.bytes.clone()));
            }
            previous = reference.bytes;
            checked += 1;
            Ok(())
        })();
        frame.Close()?;
        result?;
    }
    eprintln!("WGC compute verification: {checked} exact frames, {changed} content changes");
    anyhow::ensure!(
        checked >= 30 && changed >= 10,
        "moving source required; static frames do not validate synchronization"
    );
    drop(capture);
    let (snapshot, expected) = held.context("no retained snapshot")?;
    assert_eq!(snapshot.readback(&mut None)?.bytes, expected);
    Ok(())
}

#[test]
#[ignore = "requires an interactive Windows desktop with WGC support"]
fn wgc_reconnect_and_com_teardown_keep_the_runtime_loaded() -> Result<()> {
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    for cycle in 0..16 {
        std::thread::spawn(move || -> Result<()> {
            let com = ComGuard::new()?;
            let mut capture = Wgc::new("")?;
            capture.enable_notifications()?;
            let timer = crate::timing::Timer::new()?;
            let until = Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(frame) = capture.next_gpu(0)? {
                    assert!(frame.width > 0 && frame.height > 0);
                    break;
                }
                anyhow::ensure!(
                    Instant::now() < until,
                    "WGC reconnect {cycle} produced no frame"
                );
                timer.until_or_signal(until, &capture.notifications.as_ref().unwrap().0)?;
            }
            capture.close();
            assert!(
                capture.next_gpu(0).is_err(),
                "a closed pool must trigger GPU capture recovery"
            );
            assert!(
                capture.next_frame().is_err(),
                "a closed pool must trigger CPU capture recovery"
            );
            drop(capture);
            drop(com);
            // Reproduce the unload boundary before a subsequent stream/thread.
            // SAFETY: CoFreeUnusedLibrariesEx takes no pointers, and the name is a static
            // NUL-terminated literal.
            unsafe {
                CoFreeUnusedLibrariesEx(0, None);
                GetModuleHandleW(windows::core::w!("GraphicsCapture.dll"))?;
            }
            Ok(())
        })
        .join()
        .expect("capture worker panicked")?;
    }
    Ok(())
}

/// A top-level window of the predefined STATIC class, destroyed on drop.
struct TestWindow(windows::Win32::Foundation::HWND);
impl TestWindow {
    fn new(style: windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE) -> Result<Self> {
        use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, WINDOW_EX_STYLE};
        // SAFETY: The class and title are static NUL-terminated literals; no other pointers.
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                windows::core::w!("STATIC"),
                windows::core::w!("Rubylight capture test"),
                style,
                100,
                100,
                640,
                360,
                None,
                None,
                None,
                None,
            )?
        };
        Ok(Self(hwnd))
    }
    fn target(&self, display: &str) -> String {
        WindowTarget::new(self.0.0 as u64, display).to_string()
    }
    /// Handles its messages; the test thread owns the window.
    fn pump(&self) {
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
        };
        let mut message = MSG::default();
        // SAFETY: `message` is a live local; the messages are this thread's.
        unsafe {
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
}
impl Drop for TestWindow {
    fn drop(&mut self) {
        // SAFETY: The window was created on this thread; a destroyed one only fails the call.
        let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::DestroyWindow(self.0) };
    }
}

fn open_error(name: &str, kind: &str) -> String {
    let store = std::sync::Arc::new(RecordedWarnings::default());
    let sink: std::sync::Arc<dyn butterpollo_media_policy::WarningSink> = store.clone();
    let error = Capture::open_for_stream_reported(name, kind, false, &Default::default(), sink)
        .err()
        .expect("the window target opened");
    // Nothing else was tried in its place.
    let msgs: Vec<_> = store.snapshot().into_iter().map(|w| w.message).collect();
    assert!(msgs.is_empty(), "{msgs:?}");
    format!("{error:#}")
}

#[test]
fn window_targets_never_fall_back_to_display_capture() -> Result<()> {
    use windows::Win32::UI::WindowsAndMessaging::{WS_MINIMIZE, WS_POPUP};
    // Hidden windows: nothing appears, and no GPU is opened before the refusals.
    let normal = TestWindow::new(WS_POPUP)?;
    let minimised = TestWindow::new(WS_POPUP | WS_MINIMIZE)?;
    let display = r"\\.\DISPLAY1";
    for kind in ["ddx", "dxgi", "idd-push"] {
        let error = open_error(&normal.target(display), kind);
        assert!(
            error.contains("only be captured with Windows Graphics Capture"),
            "{kind}: {error}"
        );
    }
    for kind in ["wgc", "auto"] {
        let error = open_error(&minimised.target(display), kind);
        assert!(
            error.starts_with("the game window is minimised"),
            "{kind}: {error}"
        );
    }
    let closed = normal.target(display);
    drop(normal);
    for kind in ["wgc", "auto"] {
        let error = open_error(&closed, kind);
        assert!(
            error.starts_with("the game window closed"),
            "{kind}: {error}"
        );
    }
    let error = open_error("window:12@", "wgc");
    assert!(error.contains("names no display"), "{error}");
    // Recovery never swaps a window for the primary display: the reopen
    // reports what is wrong with the window target.
    assert!(display_present(&closed));
    assert!(display_present(r"window:12@\\.\NO_SUCH_DISPLAY"));
    assert!(display_present("window:garbage"));
    assert!(!display_present(r"\\.\NO_SUCH_DISPLAY"));
    Ok(())
}

#[test]
#[ignore = "shows a test window and captures it with WGC; needs an idle interactive desktop"]
fn wgc_window_capture_follows_resize_and_fails_on_minimise_and_close() -> Result<()> {
    use windows::Win32::UI::WindowsAndMessaging::{
        SW_MINIMIZE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, SetWindowPos,
        ShowWindow, WS_OVERLAPPEDWINDOW,
    };
    // Its log names the restart at the new size, which rc.4 lacked.
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .try_init();
    enable_dpi_awareness();
    let _com = ComGuard::new()?;
    let window = TestWindow::new(WS_OVERLAPPEDWINDOW)?;
    // SAFETY: The window was created on this thread.
    unsafe {
        let _ = ShowWindow(window.0, SW_SHOWNOACTIVATE);
    }
    let display = primary_display().context("no display")?;
    let mut capture = Capture::new(&window.target(&display), "wgc")?;
    assert_eq!(capture.backend(), "wgc");
    // what names the step: rc.4's resize failure read as a missing first frame.
    let next = |capture: &mut Capture,
                what: &str,
                until: &dyn Fn(&GpuImage) -> bool|
     -> Result<std::sync::Arc<GpuImage>> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            window.pump();
            if let Some(image) = capture.next_gpu(0)?
                && until(&image)
            {
                return Ok(image);
            }
            ensure!(Instant::now() < deadline, "no {what} within 5 s");
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    let first = next(&mut capture, "first window frame", &|_| true)?;
    tracing::info!(
        width = first.width,
        height = first.height,
        "first window frame"
    );
    // SAFETY: The window was created on this thread.
    unsafe {
        SetWindowPos(
            window.0,
            None,
            0,
            0,
            960,
            540,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        )?;
    }
    // The STATIC window draws once after the resize and then never again, like
    // a paused game: its new-size frame comes only from the restarted capture.
    let resized = next(&mut capture, "window frame at the new size", &|image| {
        image.width != first.width
    })?;
    assert!(resized.width > first.width && resized.height > first.height);
    // SAFETY: The window was created on this thread.
    unsafe {
        let _ = ShowWindow(window.0, SW_MINIMIZE);
    }
    let Err(error) = next(&mut capture, "refusal of the minimised window", &|_| false) else {
        unreachable!("no frame matches");
    };
    let error = format!("{error:#}");
    assert!(error.starts_with("the game window is minimised"), "{error}");
    let mut capture = {
        // SAFETY: The window was created on this thread.
        unsafe {
            let _ = ShowWindow(window.0, SW_SHOWNOACTIVATE);
        }
        window.pump();
        Capture::new(&window.target(&display), "wgc")?
    };
    next(&mut capture, "window frame after the restore", &|_| true)?;
    drop(window);
    let deadline = Instant::now() + Duration::from_secs(5);
    let error = loop {
        match capture.next_gpu(0) {
            Err(error) => break format!("{error:#}"),
            Ok(_) => {
                ensure!(
                    Instant::now() < deadline,
                    "the closed window kept capturing"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    };
    assert!(error.starts_with("the game window closed"), "{error}");
    Ok(())
}
