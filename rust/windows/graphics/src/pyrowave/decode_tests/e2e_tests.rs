//! End-to-end PyroWave encode/decode, plane rebuild, framing and quality-sweep tests.

use super::*;

pub(super) fn stream_status() -> Result<String> {
    let log =
        match std::fs::read_to_string(r"C:\ProgramData\Butterpollo\config\logs\butterpollo.log") {
            Ok(log) => log,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok("installed host log absent; stream activity unknown".into());
            }
            Err(error) => return Err(error.into()),
        };
    Ok(log
        .lines()
        .rev()
        .find(|line| line.contains("CLIENT CONNECTED") || line.contains("CLIENT DISCONNECTED"))
        .unwrap_or("no client events in current log")
        .to_owned())
}

#[test]
#[ignore = "requires AMD D3D12 compute, Vulkan and the pinned PyroWave DLL; see rust/PERFORMANCE.md"]
fn pyrowave_decoded_end_to_end() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    println!(
        "PYROWAVE_ENV {}",
        serde_json::json!({"adapter":gpu.display.adapter,"bitstream":pyrowave::BITSTREAM_ID,"panel_order":["bars","gradient","text","noise"]})
    );
    for (width, height) in [(1920, 1080), (1280, 720)] {
        for (format, hdr, sdr_10bit) in [
            ("sdr8", false, false),
            ("sdr10", false, true),
            ("hdr10", true, false),
        ] {
            for yuv444 in [false, true] {
                let mut config = Negotiated {
                    width,
                    height,
                    codec: 3,
                    hdr,
                    sdr_10bit,
                    yuv444,
                    csc_mode: 3,
                    packet_size: 1392,
                    fps: 60,
                    ..Default::default()
                };
                let mut decoder = Decoder::new(&gpu, &config)?;
                let pictures = (0..2)
                    .map(|phase| {
                        let (source, reference) = picture(&config, phase, Scene::Chart);
                        Ok((GpuImage::upload_settled(&gpu, &source)?, reference))
                    })
                    .collect::<Result<Vec<_>>>()?;
                // Keep the conversion control near-lossless after either
                // framing mode reserves space for its transport protection.
                for kbps in [2_000_000, 400_000, 125_000, 30_000] {
                    config.bitrate_kbps = kbps;
                    for records in [false, true] {
                        config.pyrowave_records = records;
                        // The two framing modes have different bitrate semantics
                        // and reserve different parity. Compare conversion paths
                        // and repeat frames at the same actual picture budget.
                        let mut baseline: Vec<Option<Vec<Vec<u16>>>> = vec![None, None];
                        let mut encoders = [false, true]
                            .into_iter()
                            .map(|compute| {
                                Encoder::new_gpu_options(
                                    &config,
                                    &pictures[0].0,
                                    &Config::parse(&format!("gpu_compute_conversion = {compute}"))?,
                                )
                            })
                            .collect::<Result<Vec<_>>>()?;
                        for batch in 0..3 {
                            let status = stream_status()?;
                            println!(
                                "PYROWAVE_BATCH {}",
                                serde_json::json!({"width":width,"height":height,"format":format,"chroma":if yuv444 {444} else {420},"kbps":kbps,"records":records,"batch":batch,"stream":status})
                            );
                            for compute in if batch % 2 == 0 {
                                [false, true]
                            } else {
                                [true, false]
                            } {
                                let encoder = &mut encoders[usize::from(compute)];
                                for (phase, (source, reference)) in pictures.iter().enumerate() {
                                    // Use the normal first-frame budget for every sample. CPU reference
                                    // and readback time must not increase the plain-packet bitrate budget.
                                    let Encoder::Pyrowave(native) = encoder else {
                                        unreachable!()
                                    };
                                    native.restart_interval();
                                    let frames = encode_now(encoder, source, kbps)?;
                                    ensure!(frames.len() == 1, "encoder dropped the frame");
                                    let Encoder::Pyrowave(native) = encoder else {
                                        unreachable!()
                                    };
                                    ensure!(
                                        (native.converts_on_compute()
                                            && crate::compute::shareable(&source.texture))
                                            == compute,
                                        "requested conversion path was not used"
                                    );
                                    let mut wire = VideoPacketizer {
                                        sequence: 0,
                                        iv_counter: 0,
                                        frame: 1,
                                        packet_size: config.packet_size,
                                        fec_percent: 0,
                                        min_fec: 2,
                                        key: None,
                                    };
                                    let framed = frames[0].payload()?;
                                    let packets = wire.encode_pyrowave(
                                        framed,
                                        9000,
                                        500,
                                        PyrowaveFec {
                                            records,
                                            critical_percentage: 20,
                                            detail_percentage: 0,
                                            wire_budget: 0,
                                            ipv6: false,
                                        },
                                    )?;
                                    let frame = receive(&packets)?;
                                    ensure!(
                                        frame == framed.to_vec(),
                                        "wire reconstruction changed the frame"
                                    );
                                    let decoded =
                                        decoder.decode(&gpu, &codec_packets(&frame, records)?)?;
                                    if let Some(baseline) = &baseline[phase] {
                                        for plane in 0..3 {
                                            let difference = decoded[plane]
                                                .iter()
                                                .zip(&baseline[plane])
                                                .map(|(a, b)| a.abs_diff(*b))
                                                .max()
                                                .unwrap();
                                            ensure!(
                                                difference == 0,
                                                "decoded pixels differ by {difference} storage units: {format} {width}x{height} 444={yuv444} {kbps} records={records} compute={compute} batch={batch} phase={phase} plane={plane}"
                                            );
                                        }
                                    }
                                    let errors: Vec<_> = decoded
                                        .iter()
                                        .zip(reference)
                                        .enumerate()
                                        .map(|(plane, (a, b))| {
                                            compare(
                                                a,
                                                b,
                                                config.ten_bit(),
                                                width as usize
                                                    / if plane == 0 || yuv444 { 1 } else { 2 },
                                            )
                                        })
                                        .collect();
                                    println!(
                                        "PYROWAVE_RESULT {}",
                                        serde_json::json!({"width":width,"height":height,"format":format,"chroma":if yuv444 {444} else {420},"kbps":kbps,"minimum_kbps":pyrowave::minimum_kbps(width,height,60000),"records":records,"compute":compute,"batch":batch,"phase":phase,"source":format!("{:?}",source.pixel),"bytes":frame.len(),"datagrams":packets.len(),"planes":errors.iter().map(|e| &e[0]).collect::<Vec<_>>(),"panels":errors.iter().map(|e| &e[1..]).collect::<Vec<_>>()})
                                    );
                                    if kbps == 2_000_000 {
                                        ensure!(
                                            errors.iter().all(|e| e[0].psnr >= 40.
                                                && e[1..].iter().all(|panel| panel.psnr >= 35.)),
                                            "high-bitrate control PSNR below 40 dB per plane or 35 dB per panel"
                                        );
                                    } else if kbps == 400_000 {
                                        ensure!(
                                            errors.iter().all(|e| e[0].psnr >= 24.),
                                            "400 Mbps PSNR below 24 dB per plane"
                                        );
                                    }
                                    if baseline[phase].is_none() {
                                        baseline[phase] = Some(decoded);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// A new capture format rebuilds the encoder's planes, and the driver can give
/// the new images the handles of the ones just destroyed. Every picture must
/// still decode to its own reference, including under an SDK that keeps plane
/// views between frames (Punktfunk's patch 0010; see rust/PERFORMANCE.md).
#[test]
#[ignore = "requires AMD D3D12 compute, Vulkan and the pinned PyroWave DLL; see rust/PERFORMANCE.md"]
fn pyrowave_encoder_follows_rebuilt_planes() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    println!("PYROWAVE_REBUILD_STREAM {}", stream_status()?);
    let config = Negotiated {
        width: 1920,
        height: 1080,
        codec: 3,
        hdr: true,
        yuv444: true,
        csc_mode: 3,
        packet_size: 1392,
        fps: 60,
        bitrate_kbps: 2_000_000,
        ..Default::default()
    };
    // HDR pictures alternate RgbaF16 and Rgba10Pq sources, and each change of
    // source format rebuilds the encoder's planes.
    let pictures = (0..2)
        .map(|phase| {
            let (source, reference) = picture(&config, phase, Scene::Chart);
            Ok((GpuImage::upload_settled(&gpu, &source)?, reference))
        })
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        pictures[0].0.pixel != pictures[1].0.pixel,
        "the pictures must use different source formats"
    );
    let mut decoder = Decoder::new(&gpu, &config)?;
    let mut encoder = Encoder::new_gpu_options(&config, &pictures[0].0, &Config::default())?;
    let mut first: Vec<Option<Vec<Vec<u16>>>> = vec![None, None];
    let (mut planes, mut destroyed) = (Vec::<usize>::new(), Vec::new());
    let (mut rebuilds, mut reused, mut identical) = (0, 0, 0);
    let mut previous = None;
    for (frame, phase) in [0, 0, 1, 1, 0, 1, 0, 1, 1, 0, 0, 1].into_iter().enumerate() {
        let (source, reference) = &pictures[phase];
        let Encoder::Pyrowave(native) = &mut encoder else {
            unreachable!()
        };
        native.restart_interval();
        let frames = encode_now(&mut encoder, source, config.bitrate_kbps)?;
        ensure!(frames.len() == 1, "encoder dropped frame {frame}");
        let Encoder::Pyrowave(native) = &encoder else {
            unreachable!()
        };
        let current: Vec<usize> = native.plane_images();
        // A new source format destroys the planes before it creates new ones.
        if previous.is_some_and(|previous| previous != phase) {
            destroyed.append(&mut planes);
            rebuilds += 1;
            reused += current.iter().filter(|h| destroyed.contains(*h)).count();
            // Where a cache keyed on the VkImage handles alone would keep
            // views of the destroyed images.
            identical += usize::from(destroyed.ends_with(&current));
        }
        planes = current;
        previous = Some(phase);
        let mut wire = VideoPacketizer {
            sequence: 0,
            iv_counter: 0,
            frame: frame as u32 + 1,
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
        let decoded = decoder.decode(&gpu, &codec_packets(&receive(&packets)?, false)?)?;
        let psnr: Vec<f64> = decoded
            .iter()
            .zip(reference)
            .map(|(a, b)| compare(a, b, true, config.width as usize)[0].psnr)
            .collect();
        println!(
            "PYROWAVE_REBUILD {}",
            serde_json::json!({"frame":frame,"phase":phase,"source":format!("{:?}",source.pixel),"psnr":psnr})
        );
        ensure!(
            psnr.iter().all(|&p| p >= 40.),
            "frame {frame} (phase {phase}) does not match its picture: {psnr:?} dB"
        );
        match &first[phase] {
            Some(first) => ensure!(
                &decoded == first,
                "frame {frame} (phase {phase}) decoded differently from the first encode of its picture"
            ),
            None => first[phase] = Some(decoded),
        }
    }
    println!(
        "PYROWAVE_REBUILD_HANDLES {}",
        serde_json::json!({"rebuilds":rebuilds,"new_planes":rebuilds * 3,"reused_handles":reused,"rebuilds_with_the_previous_handles":identical})
    );
    Ok(())
}

#[test]
#[ignore = "requires AMD D3D12 compute, Vulkan and the pinned PyroWave DLL"]
fn record_framing_keeps_the_wire_budget_and_decoded_pixels() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let config = Negotiated {
        width: 1280,
        height: 720,
        fps: 60,
        codec: 3,
        packet_size: 1392,
        pyrowave_records: true,
        ..Default::default()
    };
    let period = butterpollo_media_policy::framegen::Rate(config.fps_millihz()).period();
    let mut encoder = super::super::Encoder::new_device(&config, gpu.clone(), &Config::default())?;
    let mut decoder = Decoder::new(&gpu, &config)?;
    for scene in [Scene::Desktop, Scene::Game] {
        let (source, _) = picture(&config, 0, scene);
        let source = GpuImage::upload_settled(&gpu, &source)?;
        for kbps in [30_000, 80_000, 100_000, 200_000, 300_000, 400_000] {
            encoder.restart_interval();
            let frames = encoder.encode_now(&source, true, kbps)?;
            assert_eq!(frames.len(), 1);
            assert!(encoder.converts_on_compute());
            let budget = pyrowave::budget(kbps, period, config.packet_size, true, 20);
            // The codec's record frame, and the frame as it goes on the wire.
            let raw = &frames[0].bytes[..];
            let framed = frames[0].payload()?.to_vec();
            let mut packetizer = VideoPacketizer {
                sequence: 0,
                iv_counter: 0,
                frame: 1,
                packet_size: config.packet_size,
                fec_percent: 0,
                min_fec: 2,
                key: Some([42; 16]),
            };
            let fec = |critical_percentage| PyrowaveFec {
                records: true,
                critical_percentage,
                detail_percentage: 0,
                wire_budget: 0,
                ipv6: true,
            };
            let allowance = (f64::from(kbps) * 125. * period.as_secs_f64()) as usize;
            let aligned = pyrowave::record_frame(
                raw,
                pyrowave::aligned_payload(config.packet_size),
                pyrowave::max_frame_bytes(config.packet_size, true),
            )?;
            let mut aligned_wire = 0;
            for percentage in 0..=20 {
                let packets = packetizer.encode_pyrowave(&framed, 0, 0, fec(percentage))?;
                let wire: usize = packets
                    .iter()
                    .map(|packet| {
                        packet.len() + butterpollo_media_policy::network_pacing::overhead(true)
                    })
                    .sum();
                assert!(
                    wire <= allowance,
                    "record alignment exceeded the wire budget at {scene:?}/{kbps}/{percentage}: {wire} > {allowance}"
                );
                let aligned_packets =
                    packetizer.encode_pyrowave(&aligned, 0, 0, fec(percentage))?;
                let wire: usize = aligned_packets
                    .iter()
                    .map(|packet| {
                        packet.len() + butterpollo_media_policy::network_pacing::overhead(true)
                    })
                    .sum();
                aligned_wire = aligned_wire.max(wire);
            }
            println!(
                "scene={scene:?} kbps={kbps} budget={budget} raw={} framed={} aligned_wire={aligned_wire} allowance={allowance}",
                raw.len(),
                framed.len()
            );
            if aligned_wire <= allowance {
                assert_eq!(framed, aligned);
            } else {
                assert_eq!(framed, raw);
            }
            let expected = decoder.decode(&gpu, &codec_packets(raw, true)?)?;
            let decoded = decoder.decode(&gpu, &codec_packets(&framed, true)?)?;
            assert_eq!(decoded, expected);
        }
    }
    Ok(())
}

fn quality_sweep(width: u32, height: u32, critical_fec: usize) -> Result<()> {
    println!("PYROWAVE_QUALITY_START {}", stream_status()?);
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    println!(
        "PYROWAVE_QUALITY_ENV {}",
        serde_json::json!({"adapter":gpu.display.adapter,"bitstream":pyrowave::BITSTREAM_ID})
    );
    let tuning = Config::parse(&format!(
        "gpu_compute_conversion = true\npyrowave_critical_fec_percentage = {critical_fec}"
    ))?;
    for scene in [Scene::Desktop, Scene::Game, Scene::Dark] {
        for hdr in [false, true] {
            for yuv444 in [false, true] {
                let mut config = Negotiated {
                    width,
                    height,
                    codec: 3,
                    hdr,
                    yuv444,
                    csc_mode: 3,
                    packet_size: 1392,
                    pyrowave_records: true,
                    ..Default::default()
                };
                let pictures = (0..2)
                    .map(|phase| {
                        let (source, reference) = picture(&config, phase, scene);
                        Ok((GpuImage::upload_settled(&gpu, &source)?, reference))
                    })
                    .collect::<Result<Vec<_>>>()?;
                let mut decoder = Decoder::new(&gpu, &config)?;
                for fps in [60, 30, 120] {
                    config.fps = fps;
                    let mut encoder = Encoder::new_gpu_options(&config, &pictures[0].0, &tuning)?;
                    // Alternate small/large budgets, then reverse the order on the repeat.
                    let mut rates = if critical_fec > 0 {
                        vec![0.25, 5., 0.5, 4.5, 0.75, 4., 1., 3.5, 1.5, 3.2, 2., 3., 2.5]
                    } else {
                        // Without recovery packets, the transport cap allows 4K at 4–5 bpp.
                        vec![4., 5., 4.5]
                    };
                    for batch in 0..2 {
                        println!(
                            "PYROWAVE_QUALITY_BATCH {}",
                            serde_json::json!({"width":width,"height":height,"scene":format!("{scene:?}"),"hdr":hdr,"chroma":if yuv444 {444} else {420},"fps":fps,"critical_fec":critical_fec,"batch":batch,"stream":stream_status()?})
                        );
                        for &bpp in &rates {
                            let kbps = (f64::from(width) * f64::from(height) * f64::from(fps) * bpp
                                / 1000.)
                                .ceil() as u32;
                            let budget = pyrowave::budget(
                                kbps,
                                butterpollo_media_policy::framegen::Rate(fps * 1000).period(),
                                config.packet_size,
                                true,
                                critical_fec as u32,
                            );
                            for (phase, (source, reference)) in pictures.iter().enumerate() {
                                let Encoder::Pyrowave(native) = &mut encoder else {
                                    unreachable!()
                                };
                                native.restart_interval();
                                let frames = encode_now(&mut encoder, source, kbps)?;
                                ensure!(frames.len() == 1, "encoder dropped the frame");
                                let Encoder::Pyrowave(native) = &encoder else {
                                    unreachable!()
                                };
                                ensure!(
                                    native.converts_on_compute()
                                        && crate::compute::shareable(&source.texture),
                                    "compute conversion was not used"
                                );
                                let mut wire = VideoPacketizer {
                                    sequence: 0,
                                    iv_counter: 0,
                                    frame: phase as u32 + 1,
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
                                        records: true,
                                        critical_percentage: critical_fec,
                                        detail_percentage: 0,
                                        wire_budget: 0,
                                        ipv6: false,
                                    },
                                )?;
                                let frame = receive(&packets)?;
                                let decoded =
                                    decoder.decode(&gpu, &codec_packets(&frame, true)?)?;
                                let metrics: Vec<_> = decoded.iter().zip(reference).enumerate().map(|(plane, (a, b))| {
                                    let plane_width = width as usize / if plane == 0 || yuv444 { 1 } else { 2 };
                                    let error = &compare(a, b, hdr, plane_width)[0];
                                    serde_json::json!({"psnr":error.psnr,"max":error.max,"ssim":ssim(a,b,hdr,plane_width)})
                                }).collect();
                                println!(
                                    "PYROWAVE_QUALITY {}",
                                    serde_json::json!({"width":width,"height":height,"scene":format!("{scene:?}"),"hdr":hdr,"chroma":if yuv444 {444} else {420},"fps":fps,"critical_fec":critical_fec,"batch":batch,"phase":phase,"bpp":bpp,"kbps":kbps,"budget_bytes":budget,"frame_bytes":frame.len(),"wire_bytes":packets.iter().map(Vec::len).sum::<usize>(),"planes":metrics})
                                );
                            }
                        }
                        rates.reverse();
                    }
                }
            }
        }
    }
    Ok(())
}

#[test]
#[ignore = "requires AMD D3D12 compute, Vulkan and the pinned PyroWave DLL; see rust/PERFORMANCE.md"]
fn pyrowave_quality_720p() -> Result<()> {
    quality_sweep(1280, 720, 20)
}

#[test]
#[ignore = "requires AMD D3D12 compute, Vulkan and the pinned PyroWave DLL; see rust/PERFORMANCE.md"]
fn pyrowave_quality_1080p() -> Result<()> {
    quality_sweep(1920, 1080, 20)
}

#[test]
#[ignore = "requires AMD D3D12 compute, Vulkan and the pinned PyroWave DLL; see rust/PERFORMANCE.md"]
fn pyrowave_quality_2160p() -> Result<()> {
    quality_sweep(3840, 2160, 20)
}

#[test]
#[ignore = "requires AMD D3D12 compute, Vulkan and the pinned PyroWave DLL; see rust/PERFORMANCE.md"]
fn pyrowave_quality_2160p_without_fec() -> Result<()> {
    quality_sweep(3840, 2160, 0)
}
