//! Fake-driver tests for D3D12 NVENC teardown, fence synchronization and scene cuts.

use super::*;

/// A fake D3D12 output fence that tracks whether its owner dropped or retained it.
struct TrackedFence(std::rc::Rc<std::cell::Cell<bool>>);
impl Fence for TrackedFence {
    fn raw(&self) -> *mut c_void {
        FAKE_FENCE as *mut c_void
    }
    fn completed(&self) -> Result<u64> {
        FakeFence.completed()
    }
    fn notify(&self, value: u64, event: HANDLE) -> Result<()> {
        FakeFence.notify(value, event)
    }
}
impl Drop for TrackedFence {
    fn drop(&mut self) {
        self.0.set(true);
    }
}

#[test]
fn the_d3d12_structures_have_the_api_12_0_layout() {
    // nv-codec-headers n12.0.16.1 and n12.1.14.0 declare the same versioned
    // fence point and resource structures (SDK 11.1 had a fence point of
    // only a fence and a value), so the D3D12 session may open at API 12.0.
    use std::mem::{offset_of, size_of};
    assert_eq!(offset_of!(NV_ENC_FENCE_POINT_D3D12, pFence), 8);
    assert_eq!(offset_of!(NV_ENC_FENCE_POINT_D3D12, waitValue), 16);
    assert_eq!(offset_of!(NV_ENC_FENCE_POINT_D3D12, signalValue), 24);
    assert_eq!(size_of::<NV_ENC_FENCE_POINT_D3D12>(), 64);
    assert_eq!(offset_of!(NV_ENC_INPUT_RESOURCE_D3D12, pInputBuffer), 8);
    assert_eq!(offset_of!(NV_ENC_INPUT_RESOURCE_D3D12, inputFencePoint), 16);
    assert_eq!(size_of::<NV_ENC_INPUT_RESOURCE_D3D12>(), 272);
    assert_eq!(offset_of!(NV_ENC_OUTPUT_RESOURCE_D3D12, pOutputBuffer), 8);
    assert_eq!(
        offset_of!(NV_ENC_OUTPUT_RESOURCE_D3D12, outputFencePoint),
        16
    );
    assert_eq!(size_of::<NV_ENC_OUTPUT_RESOURCE_D3D12>(), 272);
    let mut point = NV_ENC_FENCE_POINT_D3D12::default();
    point.set_bWait(1);
    point.set_bSignal(1);
    // SAFETY: The bit fields follow the three 8-byte fields as one `uint32_t`.
    let bits = unsafe {
        *std::ptr::from_ref(&point)
            .cast::<u8>()
            .add(32)
            .cast::<u32>()
    };
    assert_eq!(bits, 0b11);
}

#[test]
fn a_d3d12_scene_cut_is_encoded_again_after_the_same_conversion() -> Result<()> {
    reset();
    let mut encoder = d3d12_session(&Negotiated::default(), ApiVersion(13, 0), "")?;
    let slot = d3d12_input(&mut encoder, 17)?;
    encoder.submit(slot, converted(1), true, Instant::now())?;
    encoder.poll()?;
    encoder.cut_limit = Some(4);
    state(|d| d.picture = 24);
    encoder.submit(slot, converted(2), false, Instant::now())?;
    let output = encoder.poll()?;
    assert_eq!(output.len(), 1);
    assert!(output[0].reencoded && output[0].idr);
    // The keyframe waits for the same conversion and signals a new value.
    state(|d| {
        assert_eq!(
            d.fence_points[1..],
            [(CONVERSION_FENCE, 2, 2), (CONVERSION_FENCE, 2, 3)]
        )
    });
    drop(encoder);
    assert_clean();
    Ok(())
}

#[test]
fn d3d12_teardown_flushes_eos_even_when_idle_and_passes_registration_fence() -> Result<()> {
    reset();
    let stream = Negotiated {
        codec: 1,
        ..Default::default()
    };
    let mut encoder = d3d12_session(&stream, ApiVersion(13, 0), "")?;
    let slot = d3d12_input_waiting(&mut encoder, 17, Some((CONVERSION_FENCE as *mut c_void, 5)))?;
    state(|d| {
        assert_eq!(
            d.register_fence_points,
            [Some((CONVERSION_FENCE, 5)), None],
            "only the D3D12 NV_ENC_INPUT_IMAGE registration carries pInputFencePoint"
        );
    });
    encoder.submit(slot, converted(5), true, Instant::now())?;
    assert_eq!(encoder.poll()?.len(), 1);
    assert!(encoder.pending.is_empty());
    assert!(encoder.close());
    state(|d| {
        let eos = d
            .calls
            .iter()
            .position(|call| *call == "eos")
            .expect("D3D12 teardown must send NV_ENC_PIC_FLAG_EOS even when idle");
        let first_unregister = d
            .calls
            .iter()
            .position(|call| *call == "unregister input")
            .expect("registered D3D12 resources must be unregistered");
        let destroy = d
            .calls
            .iter()
            .position(|call| *call == "destroy session")
            .expect("session must be destroyed");
        assert!(eos < first_unregister && first_unregister < destroy);
    });
    drop(encoder);
    assert_clean();
    Ok(())
}

#[test]
fn d3d12_teardown_waits_for_output_fence_and_retains_fence_on_timeout_or_destroy_error()
-> Result<()> {
    // 1. A rejected submission must not advance the teardown fence target.
    reset();
    let dropped = std::rc::Rc::new(std::cell::Cell::new(false));
    state(|d| d.d3d12 = true);
    let mut encoder = open_with(
        &Negotiated::default(),
        ApiVersion(13, 0),
        "",
        Some(Rc::new(TrackedFence(dropped.clone()))),
    )?;
    let slot = d3d12_input(&mut encoder, 17)?;
    encoder.submit(slot, converted(1), true, Instant::now())?;
    assert_eq!(encoder.poll()?.len(), 1);
    state(|d| d.reject_submit = true);
    assert!(
        encoder
            .submit(slot, converted(2), false, Instant::now())
            .is_err()
    );
    assert_eq!(encoder.d3d12.as_ref().unwrap().submitted, 1);
    assert!(encoder.close_with_wait(Duration::from_millis(5)));
    drop(encoder);
    assert!(dropped.get(), "clean close drops the D3D12 output fence");
    assert_clean();

    // 2. An unreached output fence skips unregistering and retains the fence.
    reset();
    let dropped = std::rc::Rc::new(std::cell::Cell::new(false));
    state(|d| {
        d.d3d12 = true;
        d.hold_fence = true;
    });
    let mut encoder = open_with(
        &Negotiated::default(),
        ApiVersion(13, 0),
        "",
        Some(Rc::new(TrackedFence(dropped.clone()))),
    )?;
    let slot = d3d12_input(&mut encoder, 17)?;
    encoder.submit(slot, converted(1), true, Instant::now())?;
    // Simulate a session where pending was already emptied or drain timed out
    // while the output fence has not yet reached `submitted`.
    encoder.pending.clear();
    assert!(!encoder.close_with_wait(Duration::from_millis(10)));
    state(|d| {
        assert!(
            d.notified.contains(&1),
            "teardown waited on the output fence"
        );
        assert!(
            !d.calls.contains(&"unregister input"),
            "must not unregister D3D12 resources while the output fence is unfinished"
        );
        assert!(d.calls.contains(&"destroy session"));
    });
    drop(encoder);
    assert!(
        !dropped.get(),
        "timed-out D3D12 teardown must retain the output fence"
    );

    // 3. A failed nvEncDestroyEncoder on D3D12 also retains the output fence.
    reset();
    let dropped = std::rc::Rc::new(std::cell::Cell::new(false));
    state(|d| {
        d.d3d12 = true;
        d.destroy_status = Some(_NVENCSTATUS_NV_ENC_ERR_GENERIC);
    });
    let mut encoder = open_with(
        &Negotiated::default(),
        ApiVersion(13, 0),
        "",
        Some(Rc::new(TrackedFence(dropped.clone()))),
    )?;
    let slot = d3d12_input(&mut encoder, 17)?;
    encoder.submit(slot, converted(1), true, Instant::now())?;
    assert_eq!(encoder.poll()?.len(), 1);
    assert!(!encoder.close_with_wait(Duration::from_millis(5)));
    drop(encoder);
    assert!(
        !dropped.get(),
        "failed nvEncDestroyEncoder must retain the D3D12 output fence"
    );
    Ok(())
}
