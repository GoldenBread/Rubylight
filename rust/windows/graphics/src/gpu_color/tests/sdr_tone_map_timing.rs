//! GPU cost of the HDR desktop to SDR stream conversion with the tone mapping
//! on and off.

use super::*;
use windows::Win32::Graphics::Direct3D12::*;

/// Median GPU ms of 128 conversions on the compute queue, back to back.
fn time_compute(
    queue: &crate::compute::Compute,
    mut convert: impl FnMut() -> Result<crate::compute::Converted>,
) -> Result<f64> {
    const SAMPLES: u32 = 128;
    // SAFETY: Test-owned objects on the live device. The allocator is never reset, and every
    // list is waited for before the queries are read.
    unsafe {
        let mut queries: Option<ID3D12QueryHeap> = None;
        queue.device.CreateQueryHeap(
            &D3D12_QUERY_HEAP_DESC {
                Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP,
                Count: SAMPLES * 2,
                NodeMask: 0,
            },
            &mut queries,
        )?;
        let queries = queries.context("no timestamp query heap")?;
        let ticks = crate::compute::focus_buffer(
            &queue.device,
            D3D12_HEAP_TYPE_READBACK,
            u64::from(SAMPLES) * 16,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )?;
        let allocator: ID3D12CommandAllocator = queue
            .device
            .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COMPUTE)?;
        let list: ID3D12GraphicsCommandList =
            queue
                .device
                .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_COMPUTE, &allocator, None)?;
        list.Close()?;
        let stamp = |index: u32| -> Result<u64> {
            list.Reset(&allocator, None)?;
            list.EndQuery(&queries, D3D12_QUERY_TYPE_TIMESTAMP, index);
            list.ResolveQueryData(
                &queries,
                D3D12_QUERY_TYPE_TIMESTAMP,
                index,
                1,
                &ticks,
                u64::from(index) * 8,
            );
            list.Close()?;
            queue.execute(&list)
        };
        for _ in 0..32 {
            convert()?.wait()?;
        }
        let mut last = 0;
        for sample in 0..SAMPLES {
            stamp(sample * 2)?;
            convert()?;
            last = stamp(sample * 2 + 1)?;
        }
        queue.wait(last)?;
        let frequency = queue.queue.GetTimestampFrequency()? as f64;
        let mut mapped = std::ptr::null_mut();
        ticks.Map(0, None, Some(&mut mapped))?;
        let mut times: Vec<f64> =
            std::slice::from_raw_parts(mapped.cast::<[u64; 2]>(), SAMPLES as usize)
                .iter()
                .map(|[start, end]| end.saturating_sub(*start) as f64 / frequency * 1000.)
                .collect();
        ticks.Unmap(0, None);
        times.sort_by(f64::total_cmp);
        Ok(times[times.len() / 2])
    }
}

#[test]
#[ignore = "requires D3D11/D3D12 and an idle GPU; prints the conversion's GPU time"]
fn gpu_sdr_tone_map_timing() -> Result<()> {
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let queue = crate::compute::Compute::for_device(&gpu.device)?;
    // An SDR white of 480 nits, the Windows slider's top, is scRGB 6.0.
    let white = 480.;
    let contents: [(&str, [[f32; 3]; 3]); 2] = [
        ("grey", [[0.5; 3], [2.5; 3], [6.; 3]]),
        ("saturated", [[6., 0., 0.], [0., 6., 0.], [0., 0., 6.]]),
    ];
    for (content, colours) in contents {
        let image = make_image(&colours, 1280, 2160);
        let mut source = GpuImage::upload_settled(&gpu, &image)?;
        source.gpu.forget_color();
        source.readback(&mut None)?;
        let shape = (image.width, image.height, image.pixel);
        for (ten_bit, width, height) in [
            (false, 3840, 2160),
            (true, 3840, 2160),
            (false, 1920, 1080),
            (true, 1920, 1080),
        ] {
            let mut times = [[0.; 3]; 2];
            // ABBA: on, off, off, on.
            for on in [true, false, false, true] {
                let config = butterpollo_media_policy::rtsp::Negotiated {
                    width,
                    height,
                    codec: 1,
                    sdr_10bit: ten_bit,
                    sdr_tone_map: on,
                    ..Default::default()
                };
                let arm = &mut times[usize::from(on)];
                let mut compute =
                    crate::compute::Converter::new(queue.clone(), width, height, ten_bit)?;
                compute.values = constants(&config, shape, [white, 1.], None);
                let opened = queue.open(&source.texture)?;
                arm[0] += time_compute(&queue, || {
                    compute.convert(&opened, crate::compute::format(image.pixel), None, &source)
                })? / 2.;
                for (index, pixel_passes) in [(1, false), (2, true)] {
                    let mut graphics = Converter::new(&gpu, &config, shape)?;
                    if pixel_passes {
                        graphics.compute = None;
                    } else {
                        assert!(graphics.compute.is_some());
                    }
                    graphics.set_luminance([white, 1.]);
                    let stats = time_conversion(&gpu, || {
                        graphics.convert(&source)?;
                        Ok(())
                    })?;
                    arm[index] += stats["gpu"]["p50_ms"].as_f64().context("no median")? / 2.;
                }
            }
            for (index, path) in ["D3D12 compute", "D3D11 compute", "D3D11 pixel"]
                .into_iter()
                .enumerate()
            {
                let [off, on] = [times[0][index], times[1][index]];
                println!(
                    "| {content} | {width}x{height} | {} | {path} | {off:.3} | {on:.3} | {:+.3} |",
                    if ten_bit { "10-bit" } else { "8-bit" },
                    on - off
                );
            }
        }
    }
    Ok(())
}
