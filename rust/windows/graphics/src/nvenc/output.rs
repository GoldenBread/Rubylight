//! NVENC bitstream collection, subframe slice readback, async/D3D12 output events,
//! and synchronous drain waits.

use super::*;
use windows::Win32::Foundation::{WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Threading::WaitForSingleObject;

impl Session {
    /// Oldest pending slot's completion event for an async D3D11/CUDA session.
    pub(super) fn async_output_event(&self) -> Option<&crate::timing::Signal> {
        if self.d3d12.is_some() || !self.ready.is_empty() {
            return None;
        }
        let slot = &self.slots[self.pending.front()?.slot];
        (!slot.signaled).then_some(slot.event.as_ref()).flatten()
    }
    /// Arm `event` to be set when the oldest frame in flight of a D3D12
    /// session is written: true when the caller may wait on it (set already
    /// if the frame is written), false to poll as before. The fence is asked
    /// once per frame. A frame collected earlier can only set the event late,
    /// after this frame was armed; a set event whose frame is not written is
    /// therefore reset, so it never ends every wait at once.
    pub(super) fn arm_output(&self, event: &crate::timing::Signal) -> Result<bool> {
        let (Some(d3d12), Some(signal)) = (
            &self.d3d12,
            self.pending.front().and_then(|pending| pending.signal),
        ) else {
            return Ok(false);
        };
        if !self.ready.is_empty() {
            return Ok(false);
        }
        // A removed device fails the read; the next collect reports it.
        let Ok(completed) = d3d12.fence.completed() else {
            return Ok(false);
        };
        if completed >= signal {
            event.set()?;
        } else if self.armed.get() != signal {
            event.reset()?;
            d3d12.fence.notify(signal, event.raw())?;
            self.armed.set(signal);
        } else if event.is_set() {
            event.reset()?;
            // Written meanwhile: its notification may have come before the reset.
            if d3d12.fence.completed().is_ok_and(|value| value >= signal) {
                event.set()?;
            }
        }
        Ok(true)
    }
    pub(super) fn collect(&mut self) -> Result<()> {
        let _guard = self.guard()?;
        while let Some(pending) = self.pending.front() {
            let index = pending.slot;
            // A D3D12 frame is written once its output fence is signalled.
            if let (Some(d3d12), Some(signal)) = (&self.d3d12, pending.signal)
                && d3d12.fence.completed()? < signal
            {
                if pending.submitted.elapsed() > COMPLETION_TIMEOUT {
                    bail!("NVENC completion timed out");
                }
                break;
            }
            let slot = &mut self.slots[index];
            if let Some(signal) = slot.event.as_ref()
                && !slot.signaled
            {
                // SAFETY: `signal` is owned by the slot and stays open until release/drop.
                match unsafe { WaitForSingleObject(signal.raw(), 0) } {
                    WAIT_OBJECT_0 => slot.signaled = true,
                    WAIT_TIMEOUT => {
                        if pending.submitted.elapsed() > COMPLETION_TIMEOUT {
                            bail!("NVENC completion timed out");
                        }
                        break;
                    }
                    _ => bail!("NVENC completion event failed"),
                }
            }
            let mut lock = NV_ENC_LOCK_BITSTREAM {
                version: self.api.lock(),
                outputBitstream: slot.output,
                sliceOffsets: if self.subframe {
                    self.slice_offsets.as_mut_ptr()
                } else {
                    ptr::null_mut()
                },
                ..Default::default()
            };
            // Never stall the stream thread inside nvEncLockBitstream (`nvEncodeAPI.h`);
            // LOCK_BUSY / NEED_MORE_INPUT leave the frame pending for the next poll.
            lock.set_doNotWait(1);
            if pending.measured {
                let (columns, rows) = butterpollo_media_policy::nvenc::delta_grid(
                    self.stream.codec,
                    self.stream.width,
                    self.stream.height,
                );
                let blocks = columns as usize * rows as usize;
                self.stats_buffer.clear();
                self.stats_buffer.resize(
                    blocks,
                    NV_ENC_OUTPUT_STATS_BLOCK {
                        version: self.api.structure(1, false),
                        ..Default::default()
                    },
                );
                lock.outputStatsPtr = self.stats_buffer.as_mut_ptr().cast();
                lock.outputStatsPtrSize =
                    (blocks * std::mem::size_of::<NV_ENC_OUTPUT_STATS_BLOCK>()) as u32;
            }
            // SAFETY: `slot.output` is the pending slot's bitstream buffer on this session,
            // `sliceOffsets` is null or holds an entry per macroblock, and `lock` is versioned.
            let status =
                unsafe { (self.functions.nvEncLockBitstream.unwrap())(self.raw, &mut lock) };
            // The frame is not complete yet (the driver still wants input for it): it stays
            // pending and the next picture's completion is asked for again, instead of ending
            // the stream on a status that says "not yet".
            if status == _NVENCSTATUS_NV_ENC_ERR_LOCK_BUSY
                || status == _NVENCSTATUS_NV_ENC_ERR_NEED_MORE_INPUT
            {
                if pending.submitted.elapsed() > COMPLETION_TIMEOUT {
                    bail!("NVENC output lock timed out");
                }
                break;
            }
            self.check(status, "lock completed output")?;
            let size = lock.bitstreamSizeInBytes as usize;
            let sent = pending.units.map_or(0, |units| units.sent);
            if self.subframe && self.subframe_traced < TRACED_SUBFRAMES {
                tracing::debug!(
                    frame = pending.frame,
                    status = lock.hwEncodeStatus,
                    slices = lock.numSlices,
                    bytes = size,
                    "NVENC subframe lock"
                );
            }
            if self.subframe && !subframe_complete(&lock) {
                // Hand on the slices written since the last poll, if the
                // frame leaves slice by slice and the driver's slice offsets
                // hold together; otherwise wait for all of it. The newest
                // slice waits for the next poll: it may be the last, whose
                // bytes the completed frame's last unit must carry.
                let streamed = pending.streams && (pending.units.is_some() || !self.slices_joined);
                let consistent = slices_consistent(&lock, &self.slice_offsets);
                let ready = if consistent {
                    self.slice_offsets[lock.numSlices as usize - 1] as usize
                } else {
                    0
                };
                let unit = (|| -> Result<Option<Vec<u8>>> {
                    if size > MAX_FRAME_BYTES
                        || size < sent
                        || (size > 0 && lock.bitstreamBufferPtr.is_null())
                    {
                        bail!("invalid NVENC slice output size");
                    }
                    if !streamed || !consistent || ready <= sent {
                        return Ok(None);
                    }
                    // SAFETY: The lock succeeded and the pointer is non-null with `size` bounded
                    // bytes written, `ready` of them before the newest slice; the bytes stay
                    // valid until the unlock below.
                    let written = unsafe {
                        std::slice::from_raw_parts(lock.bitstreamBufferPtr.cast::<u8>(), ready)
                    };
                    let headroom = if sent == 0 { self.dynamic_headroom } else { 0 };
                    Ok(Some(crate::frame_pool::copy_with_headroom(
                        &written[sent..],
                        headroom,
                    )))
                })();
                self.check(
                    // SAFETY: `output` was locked by the successful nvEncLockBitstream above.
                    unsafe {
                        (self.functions.nvEncUnlockBitstream.unwrap())(
                            self.raw,
                            self.slots[index].output,
                        )
                    },
                    "unlock output",
                )?;
                let unit = unit?;
                if streamed && size > sent && !consistent && !self.offsets_warned {
                    self.offsets_warned = true;
                    tracing::warn!(
                        slices = lock.numSlices,
                        bytes = size,
                        "NVENC reported slice offsets that do not fit the bytes written; this frame waits to finish before it leaves. Report this with the host log."
                    );
                }
                let pending = self.pending.front_mut().unwrap();
                if let Some(bytes) = unit {
                    let units = pending.units.get_or_insert_default();
                    let index = units.next;
                    units.next += 1;
                    units.sent = ready;
                    self.ready.push_back(Encoded {
                        framing: None,
                        bytes,
                        unit: crate::encoder::Unit::Slice { index },
                        idr: false,
                        after_invalidation: false,
                        frame_cap_bytes: pending.frame_cap_bytes,
                        reencoded: false,
                        frame_cap_target: pending.frame_cap_target,
                        latency: Some(pending.started.elapsed()),
                        presentation: Some(pending.presentation),
                    });
                }
                if pending.submitted.elapsed() > COMPLETION_TIMEOUT {
                    bail!("NVENC completion timed out");
                }
                break;
            }
            let result = (|| -> Result<(Encoded, bool)> {
                if lock.outputTimeStamp != pending.frame
                    || lock.bitstreamBufferPtr.is_null()
                    || size == 0
                    || size > MAX_FRAME_BYTES
                    || size <= sent
                {
                    bail!("invalid NVENC output timestamp or size");
                }
                let idr = lock.pictureType == _NV_ENC_PIC_TYPE_NV_ENC_PIC_TYPE_IDR;
                // A frame that has begun to leave slice by slice ends with its
                // last unit; any other leaves whole.
                let unit = pending.units.map_or(crate::encoder::Unit::Whole, |units| {
                    crate::encoder::Unit::Last { index: units.next }
                });
                let whole = unit == crate::encoder::Unit::Whole;
                // SAFETY: The lock succeeded and the pointer is non-null with a bounded size;
                // the bytes stay valid until the unlock below.
                let written = unsafe {
                    std::slice::from_raw_parts(lock.bitstreamBufferPtr.cast::<u8>(), size)
                };
                Ok((
                    Encoded {
                        framing: None,
                        bytes: crate::frame_pool::copy_with_headroom(
                            &written[sent..],
                            self.dynamic_headroom,
                        ),
                        unit,
                        idr: whole && idr,
                        after_invalidation: whole && pending.after_invalidation && !idr,
                        frame_cap_bytes: pending.frame_cap_bytes,
                        reencoded: whole && pending.reencoded,
                        frame_cap_target: pending.frame_cap_target,
                        latency: Some(pending.started.elapsed()),
                        presentation: Some(pending.presentation),
                    },
                    idr,
                ))
            })();
            self.check(
                // SAFETY: `output` was locked by the successful nvEncLockBitstream above.
                unsafe {
                    (self.functions.nvEncUnlockBitstream.unwrap())(
                        self.raw,
                        self.slots[index].output,
                    )
                },
                "unlock output",
            )?;
            let (encoded, idr) = result?;
            if self.subframe {
                self.subframe_traced = self.subframe_traced.saturating_add(1);
            }
            if idr && encoded.unit != crate::encoder::Unit::Whole && !self.unit_idr_reported {
                // Only P-frames are sent slice by slice, and NVENC makes a
                // keyframe only when asked: say so, once, if one was.
                self.unit_idr_reported = true;
                tracing::error!(
                    frame = pending.frame,
                    "NVENC returned a keyframe for a frame sent slice by slice as a P-frame. Report this with the host log."
                );
            }
            self.check(
                // SAFETY: `mapped` is the input mapped for this frame in `submit`, and its output
                // has been collected.
                unsafe {
                    (self.functions.nvEncUnmapInputResource.unwrap())(
                        self.raw,
                        self.slots[index].mapped,
                    )
                },
                "release completed input",
            )?;
            self.slots[index].mapped = ptr::null_mut();
            let output = self.slots[index]
                .d3d12
                .as_ref()
                .map_or(ptr::null_mut(), |resources| resources.mapped);
            if !output.is_null() {
                self.check(
                    // SAFETY: The D3D12 output mapped for this frame in `submit`, whose bitstream
                    // has been read and unlocked.
                    unsafe { (self.functions.nvEncUnmapInputResource.unwrap())(self.raw, output) },
                    "release completed output",
                )?;
                self.slots[index].d3d12.as_mut().unwrap().mapped = ptr::null_mut();
            }
            let pending = self.pending.pop_front().unwrap();
            if pending.measured {
                let (width, height) = butterpollo_media_policy::nvenc::delta_grid(
                    self.stream.codec,
                    self.stream.width,
                    self.stream.height,
                );
                let values = self
                    .stats_buffer
                    .iter()
                    .map(|block| u32::from(block.QP))
                    .collect();
                self.qp_grids.push_back((
                    pending.frame,
                    crate::amf::focus::Grid {
                        width,
                        height,
                        values,
                    },
                ));
            }
            if self.reencodes(&pending, &encoded) {
                match self.reencode(&pending) {
                    // The loop collects the keyframe when it is done.
                    Ok(()) => continue,
                    Err(error) => {
                        let error = format!("{error:#}");
                        tracing::error!(%error, frame = pending.frame, bytes = encoded.bytes.len(), "NVENC could not encode an oversize scene cut again as a keyframe; sending the inter frame, which loses its FEC. Report this with the host log.");
                        self.reencode_failure = Some(error);
                    }
                }
            }
            if idr && crate::amf::over_cap_target(size as u64, pending.frame_cap_target) {
                self.keyframes_over_cap += 1;
            }
            if let Some(cap) = pending.frame_cap_bytes
                && size as u64 > cap.saturating_mul(2)
                && self
                    .oversize_warned
                    .is_none_or(|at| at.elapsed() >= OVERSIZE_WARNING_INTERVAL)
            {
                self.oversize_warned = Some(Instant::now());
                tracing::warn!(
                    bytes = size,
                    cap,
                    picture = if idr { "keyframe" } else { "inter" },
                    "NVENC wrote a frame over twice its frame cap; the one-frame VBV targets the cap but the driver does not guarantee it"
                );
            }
            if idr {
                self.last_idr = pending.frame;
            }
            self.references.completed(pending.frame, idr);
            self.ready.push_back(encoded);
        }
        Ok(())
    }
    pub(super) fn drain(&mut self) -> Result<()> {
        let mut waiter = SessionDrain {
            session: self,
            started: Instant::now(),
        };
        drain::drain_pending(&mut waiter, DRAIN_TIMEOUT)
    }
    /// Blocks at most `timeout` on the oldest frame in flight: on its D3D12
    /// output fence, through `drain_event` set when the fence reaches the
    /// frame's value, or on the completion event the driver signals for its
    /// slot. A slot whose event an earlier wait consumed has no handle left.
    pub(super) fn wait_oldest(&mut self, timeout: Duration) -> Result<drain::DrainWait> {
        let Some(pending) = self.pending.front() else {
            return Ok(drain::DrainWait::Unavailable);
        };
        let (index, signal) = (pending.slot, pending.signal);
        let millis = timeout.as_millis().clamp(1, u32::MAX as u128) as u32;
        if let (Some(d3d12), Some(signal)) = (&self.d3d12, signal) {
            // A removed device fails the read; the next collect reports it.
            let Ok(completed) = d3d12.fence.completed() else {
                return Ok(drain::DrainWait::Unavailable);
            };
            if completed >= signal {
                return Ok(drain::DrainWait::Ready);
            }
            if self.drain_event.is_none() {
                self.drain_event = Some(crate::timing::Signal::new()?);
            }
            let Some(event) = self.drain_event.as_ref() else {
                return Ok(drain::DrainWait::Unavailable);
            };
            // The fence is asked once per frame; a set event from an earlier
            // frame is reset first, so it never ends this wait early.
            if self.drain_armed != signal {
                event.reset()?;
                d3d12.fence.notify(signal, event.raw())?;
                self.drain_armed = signal;
            } else if event.is_set() {
                event.reset()?;
                // Written meanwhile: its notification may have come before the reset.
                if d3d12.fence.completed().is_ok_and(|value| value >= signal) {
                    event.set()?;
                    return Ok(drain::DrainWait::Ready);
                }
            }
            // SAFETY: `event` is a live event owned by this session.
            if unsafe { WaitForSingleObject(event.raw(), millis) } == WAIT_FAILED {
                bail!("NVENC drain wait failed");
            }
            return Ok(drain::DrainWait::Waited);
        }
        let slot = &mut self.slots[index];
        let Some(signal) = slot.event.as_ref().filter(|_| !slot.signaled) else {
            return Ok(drain::DrainWait::Unavailable);
        };
        // SAFETY: `signal` is owned by the slot.
        match unsafe { WaitForSingleObject(signal.raw(), millis) } {
            WAIT_OBJECT_0 => slot.signaled = true,
            WAIT_TIMEOUT => {}
            _ => bail!("NVENC completion event failed"),
        }
        Ok(drain::DrainWait::Waited)
    }
    pub(super) fn poll(&mut self) -> Result<Vec<Encoded>> {
        self.collect()?;
        Ok(self.ready.drain(..).collect())
    }
}

/// Whether a subframe lock holds the whole picture: `hwEncodeStatus` 2, the
/// completion NVIDIA's developer forum gives for subframe readback. The slice
/// count alone is not enough, since a driver may report every slice of the
/// mode (`sliceMode` 3) before the last is written. A frame that never
/// completes fails loudly at the completion timeout rather than leave cut
/// short.
pub(super) fn subframe_complete(lock: &NV_ENC_LOCK_BITSTREAM) -> bool {
    lock.hwEncodeStatus == 2
}
/// Whether an unfinished subframe lock's slice offsets fit the bytes
/// written: at least one slice, offsets rising strictly and the last slice
/// starting inside the written size.
pub(super) fn slices_consistent(lock: &NV_ENC_LOCK_BITSTREAM, offsets: &[u32]) -> bool {
    let count = lock.numSlices as usize;
    count > 0
        && count <= offsets.len()
        && offsets[..count].windows(2).all(|pair| pair[0] < pair[1])
        && offsets[count - 1] < lock.bitstreamSizeInBytes
}
