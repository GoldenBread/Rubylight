//! PyroWave D3D12 device-loss recovery and handle-leak regression tests.

use super::*;

/// Open handles of this process; a rebuilt encoder must give back what it took.
fn handle_count() -> Result<u32> {
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};
    let mut count = 0;
    // SAFETY: The pseudo-handle is always valid and `count` is a live local.
    unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut count)? };
    Ok(count)
}

/// Encode `image` with the PyroWave encoder, decode it and require its Y plane
/// within 40 dB of `reference`.
fn encode_and_decode(
    gpu: &Device,
    decoder: &mut Decoder,
    encoder: &mut Encoder,
    config: &Negotiated,
    image: &GpuImage,
    reference: &[Vec<u16>],
) -> Result<()> {
    let Encoder::Pyrowave(native) = &mut *encoder else {
        unreachable!()
    };
    native.restart_interval();
    let frames = encode_now(encoder, image, config.bitrate_kbps)?;
    ensure!(frames.len() == 1, "encoder dropped the frame");
    let mut wire = VideoPacketizer {
        sequence: 0,
        iv_counter: 0,
        frame: 1,
        packet_size: config.packet_size,
        fec_percent: 0,
        min_fec: 2,
        key: None,
    };
    let packets = wire.encode_pyrowave(
        frames[0].payload()?,
        9000,
        500,
        PyrowaveFec {
            records: false,
            critical_percentage: 20,
            detail_percentage: 0,
            wire_budget: 0,
            ipv6: false,
        },
    )?;
    let decoded = decoder.decode(gpu, &codec_packets(&receive(&packets)?, false)?)?;
    let psnr = compare(&decoded[0], &reference[0], false, config.width as usize)[0].psnr;
    ensure!(psnr >= 40., "decoded luma is {psnr} dB");
    Ok(())
}

/// Where a life of the stream loses this process's D3D12 device.
#[derive(Clone, Copy, Debug)]
enum Loss {
    /// Never: a control for what an ordinary life leaves open.
    Never,
    /// Between two frames, caught by the entry check.
    BetweenFrames,
    /// After the entry check, before the conversion is recorded, so the
    /// conversion itself fails (`conversion_failed`).
    BeforeConversion,
    /// After the frame is submitted, before its completion wait.
    MidFrame,
}

/// Aborts the run if a test hangs, until dropped. Dropping the guard (on any
/// exit, including `?`) disconnects the channel and the thread ends quietly.
struct Watchdog(#[allow(dead_code)] std::sync::mpsc::Sender<()>);
impl Watchdog {
    fn start(limit: std::time::Duration, name: &'static str) -> Self {
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = receiver.recv_timeout(limit) {
                eprintln!("{name} hung for {limit:?}");
                std::process::abort();
            }
        });
        Self(sender)
    }
}

/// One life of a stream: encode, lose this process's D3D12 device under the
/// encoder's compute conversion at `loss`, require the frame and every later one
/// to fail as a device loss with nothing quarantined, then drop the encoder.
/// Returns the time the failing frames took.
fn lose_the_d3d12_device(loss: Loss) -> Result<std::time::Duration> {
    use windows::Win32::Graphics::Direct3D12::ID3D12Device5;
    let gpu = Device::new("")?;
    let config = Negotiated {
        width: 1280,
        height: 720,
        codec: 3,
        csc_mode: 3,
        packet_size: 1392,
        fps: 60,
        bitrate_kbps: 2_000_000,
        ..Default::default()
    };
    let (source, reference) = picture(&config, 0, Scene::Chart);
    let image = GpuImage::upload_settled(&gpu, &source)?;
    let mut decoder = Decoder::new(&gpu, &config)?;
    let mut encoder = Encoder::new_gpu_options(
        &config,
        &image,
        &Config::parse("gpu_compute_conversion = true")?,
    )?;
    encode_and_decode(
        &gpu,
        &mut decoder,
        &mut encoder,
        &config,
        &image,
        &reference,
    )?;
    let Encoder::Pyrowave(native) = &mut encoder else {
        unreachable!()
    };
    let compute = native
        .compute()
        .context("the compute queue did not convert the frame")?
        .clone();
    ensure!(native.device_removed().is_none(), "lost before the removal");
    if matches!(loss, Loss::Never) {
        drop(encoder);
        drop(decoder);
        return Ok(Default::default());
    }
    let remove = {
        let device = compute.device.cast::<ID3D12Device5>()?;
        move || {
            // SAFETY: `device` is this process's live D3D12 device for the
            // adapter. RemoveDevice marks only that device as removed in
            // software: no GPU reset, so other processes, the display and the
            // D3D11 and Vulkan devices here are untouched.
            unsafe { device.RemoveDevice() }
        }
    };
    match loss {
        Loss::Never => unreachable!(),
        Loss::BetweenFrames => {
            remove();
            ensure!(
                crate::device_loss::DeviceLost::d3d12(&compute.device).is_some(),
                "the D3D12 device is not removed"
            );
        }
        Loss::BeforeConversion => native.hooks.before_conversion = Some(Box::new(remove)),
        Loss::MidFrame => native.hooks.after_submit = Some(Box::new(remove)),
    }
    let started = Instant::now();
    for attempt in 0..3 {
        let Encoder::Pyrowave(native) = &mut encoder else {
            unreachable!()
        };
        native.restart_interval();
        // With two frames in flight a mid-frame loss surfaces when the frame
        // is waited for, not always in its own encode call.
        let error = encode_now(&mut encoder, &image, config.bitrate_kbps)
            .err()
            .with_context(|| format!("frame {attempt} encoded on a removed device"))?;
        ensure!(
            crate::device_loss::DeviceLost::from_error(&error).is_some(),
            "{loss:?}: frame {attempt} failed, but not as a device loss: {error:#}"
        );
        ensure!(
            crate::device_loss::DeviceLost::d3d11(&gpu.device).is_none(),
            "the D3D11 device was lost too"
        );
        let Encoder::Pyrowave(native) = &encoder else {
            unreachable!()
        };
        // A lost D3D12 device holds no work, so the lease is free and the
        // adapter stays usable: the next life builds on a new device.
        ensure!(
            native.leases() == 0,
            "{loss:?}: frame {attempt} left the capture lease pinned: {error:#}"
        );
        super::super::quarantined_adapters()
            .check(native.adapter)
            .with_context(|| format!("{loss:?}: frame {attempt} quarantined the adapter"))?;
    }
    let elapsed = started.elapsed();
    drop(encoder);
    drop(decoder);
    Ok(elapsed)
}

/// Two ordinary lives, then three that lose the device at `loss`. A lost life
/// must report device loss, quarantine nothing (the next life builds and
/// decodes on the same adapter), and leave no more handles than an ordinary one.
/// Handle counts are process-wide: run with `--test-threads=1`.
fn recover_from_loss(loss: Loss, name: &'static str) -> Result<()> {
    let _com = ComGuard::new()?;
    // A hang fails the test instead of the run.
    let _watchdog = Watchdog::start(std::time::Duration::from_secs(180), name);
    println!("PYROWAVE_LOSS_STREAM {}", stream_status()?);
    // Two lives that keep their device show what an ordinary life leaves
    // behind (the SDK and driver keep a few handles per Vulkan device); the
    // lives that lose it must leave no more.
    let mut handles: Vec<i64> = Vec::new();
    for (life, loss) in [Loss::Never, Loss::Never, loss, loss, loss]
        .into_iter()
        .enumerate()
    {
        let elapsed = lose_the_d3d12_device(loss)?;
        ensure!(
            elapsed < std::time::Duration::from_secs(10),
            "life {life}: three failing frames took {elapsed:?}"
        );
        handles.push(handle_count()?.into());
        println!(
            "PYROWAVE_LOSS {}",
            serde_json::json!({"life":life,"loss":format!("{loss:?}"),"failing_frames_ms":elapsed.as_millis(),"handles":handles.last()})
        );
    }
    // The first life of each kind warms driver and runtime caches.
    let ordinary = handles[1] - handles[0];
    let lost = handles[4] - handles[3];
    ensure!(
        lost <= ordinary,
        "a lost device leaks {lost} handles per life against {ordinary} for an ordinary one: {handles:?}"
    );
    Ok(())
}

/// A real, per-device removal: `ID3D12Device5::RemoveDevice` on the D3D12
/// device the encoder converts on, between two frames. The host classifies the
/// failure as a device loss, and a stream rebuilt on a new device encodes and
/// decodes again, with no quarantine of the adapter and no handles left behind.
#[test]
#[ignore = "requires AMD D3D12 compute, Vulkan and the pinned PyroWave DLL; removes only this process's D3D12 device; run with --test-threads=1"]
fn pyrowave_recovers_from_a_removed_d3d12_device() -> Result<()> {
    recover_from_loss(
        Loss::BetweenFrames,
        "PyroWave device-loss test (between frames)",
    )
}

/// The device is removed after the entry check and before the conversion, so the
/// conversion fails on the dead device and `conversion_failed` must free the lease.
#[test]
#[ignore = "requires AMD D3D12 compute, Vulkan and the pinned PyroWave DLL; removes only this process's D3D12 device; run with --test-threads=1"]
fn pyrowave_recovers_from_a_d3d12_device_lost_before_conversion() -> Result<()> {
    recover_from_loss(
        Loss::BeforeConversion,
        "PyroWave device-loss test (before conversion)",
    )
}

/// The device is removed after the frame is submitted, before its wait. Proves
/// that the check after the wait reports the loss and the lease is released with
/// no quarantine. It does not prove the race where the wait is satisfied before
/// Vulkan has run the encode; that rests on `pyrowave_encoder_destroy` idling the GPU.
#[test]
#[ignore = "requires AMD D3D12 compute, Vulkan and the pinned PyroWave DLL; removes only this process's D3D12 device; run with --test-threads=1"]
fn pyrowave_recovers_from_a_d3d12_device_lost_mid_frame() -> Result<()> {
    recover_from_loss(Loss::MidFrame, "PyroWave device-loss test (mid frame)")
}

/// A life of the encoder alone: build it on a fresh capture device, encode one
/// frame, drop everything.
fn encoder_life(config: &Negotiated, stage: u32, compute: bool) -> Result<()> {
    let gpu = Device::new("")?;
    let (source, _) = picture(config, 0, Scene::Chart);
    let image = GpuImage::upload_settled(&gpu, &source)?;
    if stage == 0 {
        return Ok(());
    }
    let mut encoder = Encoder::new_gpu_options(
        config,
        &image,
        &Config::parse(&format!("gpu_compute_conversion = {compute}"))?,
    )?;
    if stage == 1 {
        return Ok(());
    }
    let Encoder::Pyrowave(native) = &mut encoder else {
        unreachable!()
    };
    native.restart_interval();
    let frames = encode_now(&mut encoder, &image, config.bitrate_kbps)?;
    ensure!(frames.len() == 1, "encoder dropped the frame");
    Ok(())
}

/// A life of the decoder alone: build it, drop it.
fn decoder_life(config: &Negotiated) -> Result<()> {
    let gpu = Device::new("")?;
    drop(Decoder::new(&gpu, config)?);
    Ok(())
}

/// Handles open after each of `lives` lives, and the growth after the first
/// ten, which warm driver and runtime caches.
fn handles_per_life(name: &str, lives: usize, mut life: impl FnMut() -> Result<()>) -> Result<i64> {
    let mut handles = Vec::new();
    for _ in 0..lives {
        life()?;
        handles.push(i64::from(handle_count()?));
    }
    let growth = handles[lives - 1] - handles[9];
    println!(
        "PYROWAVE_HANDLES {}",
        serde_json::json!({"life":name,"lives":lives,"growth_after_warmup":growth,"handles":handles})
    );
    Ok(growth)
}

/// Handles each encoder or decoder life may leave behind. The pinned SDK and
/// the AMD Vulkan driver leave 3 events and 1 thread handle open per
/// encoder or decoder (measured: the same 4 for the encoder and the decoder,
/// none for our D3D devices, imports and conversion); the bound catches any
/// leak of ours on top of that. Handle counts are process-wide: run with
/// `--test-threads=1`.
#[test]
#[ignore = "requires AMD D3D12 compute, Vulkan and the pinned PyroWave DLL; run with --test-threads=1"]
fn pyrowave_lives_do_not_leak_handles() -> Result<()> {
    let _com = ComGuard::new()?;
    let _watchdog = Watchdog::start(
        std::time::Duration::from_secs(300),
        "PyroWave handle leak test",
    );
    let config = Negotiated {
        width: 1280,
        height: 720,
        codec: 3,
        csc_mode: 3,
        packet_size: 1392,
        fps: 60,
        bitrate_kbps: 2_000_000,
        ..Default::default()
    };
    const LIVES: usize = 50;
    const SDK_HANDLES_PER_LIFE: i64 = 4;
    let measured = (LIVES - 10) as i64;
    let mut cases = Vec::new();
    for (name, stage, compute, sdk) in [
        ("device and upload", 0, true, 0),
        ("encoder built", 1, true, SDK_HANDLES_PER_LIFE),
        (
            "encoder, one frame, graphics conversion",
            2,
            false,
            SDK_HANDLES_PER_LIFE,
        ),
        (
            "encoder, one frame, compute conversion",
            2,
            true,
            SDK_HANDLES_PER_LIFE,
        ),
    ] {
        let growth = handles_per_life(name, LIVES, || encoder_life(&config, stage, compute))?;
        cases.push((name, growth, sdk));
    }
    let growth = handles_per_life("decoder", LIVES, || decoder_life(&config))?;
    cases.push(("decoder", growth, SDK_HANDLES_PER_LIFE));
    for (name, growth, sdk) in cases {
        ensure!(
            growth <= sdk * measured + 4,
            "{name}: {growth} handles leaked over {measured} lives, expected at most {} per life",
            sdk
        );
    }
    Ok(())
}
