//! NVENC picture submission, D3D12 fence-point wiring, per-frame HDR10 metadata,
//! QP delta map attachment, and oversize scene-cut keyframe re-encoding.

use super::*;

impl Session {
    pub(super) fn submit(
        &mut self,
        slot: usize,
        texture: Option<Kept>,
        idr: bool,
        presentation: Instant,
    ) -> Result<()> {
        self.submit_frame(slot, texture, idr, presentation, None, false)
    }
    pub(super) fn submit_frame(
        &mut self,
        slot: usize,
        texture: Option<Kept>,
        idr: bool,
        presentation: Instant,
        again: Option<Again>,
        skip_cuda_copy: bool,
    ) -> Result<()> {
        let guard = self.guard()?;
        let frame = again.map_or(self.next, |again| again.frame);
        if self.pending.len() >= MAX_PENDING || self.pending.iter().any(|p| p.slot == slot) {
            bail!("NVENC input is still in flight");
        }
        // A D3D12 picture waits on the GPU for its conversion's fence.
        let wait = match (&self.d3d12, &texture) {
            (None, _) => None,
            (Some(_), Some(Kept::D3d12 { fence, value, .. })) => Some((*fence, *value)),
            (Some(_), _) => bail!("a D3D12 NVENC frame needs its conversion's fence"),
        };
        if let Some(signal) = self.slots[slot].event.as_ref() {
            signal.reset()?;
        }
        if !skip_cuda_copy
            && let (Some(input), Some(guard)) =
                (self.slots[slot].input.cuda.as_mut(), guard.as_ref())
        {
            input.copy(guard)?;
        }
        let mut map = NV_ENC_MAP_INPUT_RESOURCE {
            version: self.api.structure(4, false),
            registeredResource: self.slots[slot].registered,
            ..Default::default()
        };
        // SAFETY: `registered` is this slot's live registration and no pending frame has it mapped
        // (checked above).
        let status = unsafe { (self.functions.nvEncMapInputResource.unwrap())(self.raw, &mut map) };
        self.slots[slot].mapped = map.mappedResource;
        self.check(status, "map encoder input")?;
        if map.mappedResource.is_null() || map.mappedBufferFmt != buffer_format(&self.stream) {
            if !self.slots[slot].mapped.is_null() {
                // SAFETY: `mapped` was just mapped above and no picture consumed it.
                let _ = unsafe {
                    (self.functions.nvEncUnmapInputResource.unwrap())(
                        self.raw,
                        self.slots[slot].mapped,
                    )
                };
                self.slots[slot].mapped = ptr::null_mut();
            }
            bail!("NVENC mapped an incompatible input");
        }
        let mut signal = None;
        let mut input = map.mappedResource;
        if let Some((fence, value)) = wait {
            let mut output = NV_ENC_MAP_INPUT_RESOURCE {
                version: self.api.structure(4, false),
                registeredResource: self.slots[slot].d3d12.as_ref().unwrap().registered,
                ..Default::default()
            };
            // SAFETY: The registration is this slot's live D3D12 output, which no pending frame
            // has mapped (checked above).
            let status =
                unsafe { (self.functions.nvEncMapInputResource.unwrap())(self.raw, &mut output) };
            if status != SUCCESS || output.mappedResource.is_null() {
                // SAFETY: `mapped` is the input mapping made above, which no picture took.
                let _ = unsafe {
                    (self.functions.nvEncUnmapInputResource.unwrap())(
                        self.raw,
                        self.slots[slot].mapped,
                    )
                };
                self.slots[slot].mapped = ptr::null_mut();
                self.check(status, "map D3D12 output")?;
                bail!("NVENC mapped no D3D12 output");
            }
            let session = self.d3d12.as_mut().unwrap();
            session.value += 1;
            signal = Some(session.value);
            let resources = self.slots[slot].d3d12.as_mut().unwrap();
            resources.mapped = output.mappedResource;
            resources.input = NV_ENC_INPUT_RESOURCE_D3D12 {
                version: self.api.structure(1, false),
                pInputBuffer: map.mappedResource,
                inputFencePoint: fence_point(self.api, fence, value, false),
                ..Default::default()
            };
            resources.output = NV_ENC_OUTPUT_RESOURCE_D3D12 {
                version: self.api.structure(1, false),
                pOutputBuffer: output.mappedResource,
                outputFencePoint: fence_point(self.api, session.fence.raw(), session.value, true),
                ..Default::default()
            };
            input = (&mut resources.input as *mut NV_ENC_INPUT_RESOURCE_D3D12).cast();
        }
        let metadata = self.encoded_metadata();
        let idr = self.keyframe(idr);
        // The session's picture parameters, zeroed once at open: this resets
        // the fields that change per frame and leaves the rest -- the
        // reserved areas NV_ENC_PIC_PARAMS requires to stay zero included --
        // as they are.
        let (pitch, output, event) = (
            self.slots[slot].input.pitch,
            self.slots[slot].output,
            self.slots[slot]
                .event
                .as_ref()
                .map_or(ptr::null_mut(), |signal| signal.raw().0),
        );
        self.picture.inputPitch = pitch;
        self.picture.frameIdx = frame as u32;
        self.picture.inputTimeStamp = frame;
        self.picture.inputBuffer = input;
        self.picture.outputBitstream = output;
        self.picture.completionEvent = event;
        self.picture.bufferFmt = map.mappedBufferFmt;
        self.picture.encodePicFlags = if idr {
            _NV_ENC_PIC_FLAGS_NV_ENC_PIC_FLAG_FORCEIDR
                | _NV_ENC_PIC_FLAGS_NV_ENC_PIC_FLAG_OUTPUT_SPSPPS
        } else {
            0
        };
        self.picture.qpDeltaMap = ptr::null_mut();
        self.picture.qpDeltaMapSize = 0;
        if self.stream.hdr && self.api >= ApiVersion(13, 0) {
            *self.slots[slot].metadata.mastering = *self.metadata.mastering;
            *self.slots[slot].metadata.light = *self.metadata.light;
            {
                if self.stream.codec == 1 {
                    self.picture.codecPicParams.hevcPicParams.pMasteringDisplay =
                        self.slots[slot].metadata.mastering.as_mut();
                    self.picture.codecPicParams.hevcPicParams.pMaxCll =
                        self.slots[slot].metadata.light.as_mut();
                } else if self.stream.codec == 2 {
                    self.picture.codecPicParams.av1PicParams.pMasteringDisplay =
                        self.slots[slot].metadata.mastering.as_mut();
                    self.picture.codecPicParams.av1PicParams.pMaxCll =
                        self.slots[slot].metadata.light.as_mut();
                }
            }
        }
        // Encoder focus: the map the encoder set for this frame, never on a
        // keyframe or a frame encoded again.
        let map = self.next_map.take();
        let map = if self.qp_map && !idr && again.is_none() {
            map
        } else {
            // A map left over from a frame that cannot carry one keeps its
            // buffer for the next that does.
            if let Some(mut map) = map {
                map.clear();
                self.map_scratch = map;
            }
            None
        };
        if let Some(map) = map {
            // The slot's last frame is complete, so its map's buffer
            // becomes the next map's scratch.
            let mut done = std::mem::replace(&mut self.slots[slot].deltas, map);
            done.clear();
            self.map_scratch = done;
            self.picture.qpDeltaMap = self.slots[slot].deltas.as_mut_ptr();
            self.picture.qpDeltaMapSize = self.slots[slot].deltas.len() as u32;
        }
        let measured =
            std::mem::take(&mut self.measure_next) && self.block_stats && again.is_none();
        let submitted = Instant::now();
        let started = again.map_or(submitted, |again| again.started);
        // SAFETY: `self.picture` points at the slot's mapped input, output buffer, boxed metadata
        // and delta map, which all stay alive while the slot is in `pending`.
        let status =
            unsafe { (self.functions.nvEncEncodePicture.unwrap())(self.raw, &mut self.picture) };
        if status != SUCCESS && status != _NVENCSTATUS_NV_ENC_ERR_NEED_MORE_INPUT {
            self.force_idr |= idr;
            // The input stays registered; only this frame's mapping goes.
            // SAFETY: `mapped` is the mapping made above, which the rejected picture never took.
            let _ = unsafe {
                (self.functions.nvEncUnmapInputResource.unwrap())(self.raw, self.slots[slot].mapped)
            };
            self.slots[slot].mapped = ptr::null_mut();
            if let Some(resources) = self.slots[slot].d3d12.as_mut() {
                // SAFETY: The D3D12 output mapping made above, which the rejected picture never
                // took.
                let _ = unsafe {
                    (self.functions.nvEncUnmapInputResource.unwrap())(self.raw, resources.mapped)
                };
                resources.mapped = ptr::null_mut();
            }
            self.check(status, "submit picture")?;
        }
        if let (Some(d3d12), Some(signal)) = (&mut self.d3d12, signal) {
            d3d12.submitted = d3d12.submitted.max(signal);
        }
        self.force_idr = false;
        self.submitted_metadata = metadata;
        let after_invalidation = self.references.confirmation(idr);
        self.slots[slot].signaled = false;
        self.pending.push_back(Pending {
            slot,
            frame,
            started,
            submitted,
            presentation,
            after_invalidation,
            idr,
            reencoded: again.is_some(),
            frame_cap_bytes: self.frame_cap(),
            frame_cap_target: self.frame_cap_target,
            cut_limit: self.cut_limit,
            streams: self.subframe && !idr && !after_invalidation && self.frame_cap().is_some(),
            units: None,
            signal,
            measured,
            _converted: texture,
        });
        if again.is_none() {
            self.next = self
                .next
                .checked_add(1)
                .context("NVENC frame index overflow")?;
        }
        Ok(())
    }
    /// Whether `encoded`, `pending`'s output, goes back to NVENC as a keyframe
    /// instead of to the client: the rule AMF follows
    /// ([`crate::amf::reencodes_cut`]), for an inter frame past the stream's
    /// full-FEC bound that the VBV did not hold, while nothing submitted after
    /// it can reference it. Not with slice delivery, as on AMF.
    pub(super) fn reencodes(&self, pending: &Pending, encoded: &Encoded) -> bool {
        !self.subframe
            && crate::amf::reencodes_cut(
                encoded.idr || pending.idr || pending.reencoded,
                encoded.bytes.len() as u64,
                pending.cut_limit,
                self.pending.is_empty(),
            )
    }
    /// Submit `cut`'s input again as a keyframe with its frame number. The
    /// keyframe resets the references, so a later repair names only it.
    pub(super) fn reencode(&mut self, cut: &Pending) -> Result<()> {
        self.submit_frame(
            cut.slot,
            cut._converted.clone(),
            true,
            cut.presentation,
            Some(Again {
                frame: cut.frame,
                started: cut.started,
            }),
            true,
        )?;
        self.reencoded_cuts += 1;
        Ok(())
    }
}
