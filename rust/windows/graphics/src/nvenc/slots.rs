//! NVENC input/output slot registration, D3D12 READBACK buffer binding,
//! async completion event registration, and deferred cleanup retries.

use super::*;

impl Session {
    pub(super) fn slot(&mut self, input: Input) -> Result<usize> {
        // Slots the driver would not release (key 0) wait for the session's
        // destruction and do not count.
        if self.slots.iter().filter(|slot| slot.input.key != 0).count() >= MAX_PENDING {
            bail!("NVENC input registration limit reached");
        }
        if self.d3d12.is_some() != input.d3d12.is_some() {
            bail!("an NVENC input does not match its session's D3D11 or D3D12 device");
        }
        let _guard = self.guard()?;
        self.retry_deferred();
        let deferred = self.slots.iter().filter(|slot| slot.deferred()).count();
        if deferred >= MAX_DEFERRED {
            // A persistent fault: each failed registration would leave one
            // more slot, with its event handle, until the session ends. Fail
            // visibly so the encoder is recreated, which destroys the session.
            tracing::error!(
                deferred,
                "NVENC keeps inputs it will not release; the encoder must be recreated"
            );
            bail!("NVENC input cleanup keeps failing: {deferred} inputs are held by the driver");
        }
        let fresh = Slot {
            input,
            metadata: Metadata::new(self.stream.codec),
            registered: ptr::null_mut(),
            mapped: ptr::null_mut(),
            output: ptr::null_mut(),
            d3d12: None,
            deltas: Vec::new(),
            event: None,
            event_registered: false,
            signaled: false,
        };
        // A slot the driver has let go is reused, so repeated faults that
        // clean up on retry do not lengthen the list.
        let index = match self.slots.iter().position(Slot::released) {
            Some(index) => {
                self.slots[index] = fresh;
                index
            }
            None => {
                self.slots.push(fresh);
                self.slots.len() - 1
            }
        };
        if let Err(error) = self.acquire(index) {
            // A slot that failed to register is released at once, so the
            // texture registers afresh when it comes round again instead of
            // failing at map.
            match cleanup_slot(&self.functions, self.raw, self.api, &mut self.slots[index]) {
                Ok(()) => {
                    self.slots[index].release();
                    self.trim_released();
                }
                Err(cleanup) => {
                    // The driver still holds part of it: `retry_deferred`
                    // tries again on the next registration and
                    // Session::drop releases the rest. No input finds the
                    // slot by its key and it does not count against the
                    // registration limit, but only MAX_DEFERRED may wait.
                    tracing::warn!(error = %cleanup, "NVENC input cleanup deferred");
                    self.slots[index].input.key = 0;
                }
            }
            return Err(error);
        }
        Ok(index)
    }
    /// Try again to release the slots whose cleanup failed; a slot the driver
    /// lets go of gives up its event handle and inputs.
    fn retry_deferred(&mut self) {
        for slot in self.slots.iter_mut().filter(|slot| slot.deferred()) {
            if cleanup_slot(&self.functions, self.raw, self.api, slot).is_ok() {
                slot.release();
            }
        }
        self.trim_released();
    }
    /// Drop released slots from the end of the list. Only slots with a key
    /// can be in flight, and a released one has none, so no pending frame
    /// names them.
    fn trim_released(&mut self) {
        while self.slots.last().is_some_and(Slot::released) {
            self.slots.pop();
        }
    }
    /// Register slot `index`'s input and make its output and completion
    /// event. A failure leaves what was made in the slot for `cleanup_slot`.
    fn acquire(&mut self, index: usize) -> Result<()> {
        let mut input_fence = self.slots[index]
            .input
            .d3d12
            .as_mut()
            .and_then(|input| input.wait.take())
            .map(|(fence, value)| fence_point(self.api, fence, value, false));
        let mut register = NV_ENC_REGISTER_RESOURCE {
            version: self.api.register(),
            resourceType: self.slots[index].input.kind,
            width: self.stream.width,
            height: self.stream.height,
            pitch: self.slots[index].input.pitch,
            resourceToRegister: self.slots[index].input.raw,
            bufferFormat: buffer_format(&self.stream),
            bufferUsage: _NV_ENC_BUFFER_USAGE_NV_ENC_INPUT_IMAGE,
            pInputFencePoint: input_fence
                .as_mut()
                .map_or(ptr::null_mut(), |point| point as *mut _),
            ..Default::default()
        };
        // SAFETY: `validate` checked the entry; `input.raw` is kept alive by the slot's texture
        // clone or CUDA allocation until `cleanup_slot` unregisters it.
        let status =
            unsafe { (self.functions.nvEncRegisterResource.unwrap())(self.raw, &mut register) };
        self.slots[index].registered = register.registeredResource;
        self.check(status, "register GPU input")?;
        if register.registeredResource.is_null() {
            bail!("NVENC returned no registered resource");
        }
        let readback = self.slots[index]
            .input
            .d3d12
            .as_ref()
            .map(|input| (input.output, input.size));
        if let Some((buffer, size)) = readback {
            // A D3D12 session writes each bitstream into the slot's READBACK
            // buffer, registered as a one-row byte buffer.
            self.slots[index].d3d12 = Some(Box::default());
            let mut register = NV_ENC_REGISTER_RESOURCE {
                version: self.api.register(),
                resourceType: _NV_ENC_INPUT_RESOURCE_TYPE_NV_ENC_INPUT_RESOURCE_TYPE_DIRECTX,
                width: size,
                height: 1,
                resourceToRegister: buffer,
                bufferFormat: _NV_ENC_BUFFER_FORMAT_NV_ENC_BUFFER_FORMAT_U8,
                bufferUsage: _NV_ENC_BUFFER_USAGE_NV_ENC_OUTPUT_BITSTREAM,
                ..Default::default()
            };
            // SAFETY: `validate` checked the entry; the buffer is kept alive by the slot's input
            // until `cleanup_slot` unregisters it.
            let status =
                unsafe { (self.functions.nvEncRegisterResource.unwrap())(self.raw, &mut register) };
            let slot = &mut self.slots[index];
            let resources = slot.d3d12.as_mut().unwrap();
            resources.registered = register.registeredResource;
            slot.output = (&mut resources.output as *mut NV_ENC_OUTPUT_RESOURCE_D3D12).cast();
            self.check(status, "register D3D12 output")?;
            if register.registeredResource.is_null() {
                bail!("NVENC returned no registered output");
            }
            return Ok(());
        }
        let mut output = NV_ENC_CREATE_BITSTREAM_BUFFER {
            version: self.api.structure(1, false),
            ..Default::default()
        };
        // SAFETY: `validate` checked the entry and `output` is versioned; the buffer is owned by
        // this slot.
        let status =
            unsafe { (self.functions.nvEncCreateBitstreamBuffer.unwrap())(self.raw, &mut output) };
        self.slots[index].output = output.bitstreamBuffer;
        self.check(status, "create output buffer")?;
        if output.bitstreamBuffer.is_null() {
            bail!("NVENC returned no output buffer");
        }
        if self.initialize.enableEncodeAsync != 0 {
            let signal = crate::timing::Signal::new()?;
            let raw_event = signal.raw().0;
            self.slots[index].event = Some(signal);
            let mut event = NV_ENC_EVENT_PARAMS {
                version: self.api.event(),
                completionEvent: raw_event,
                ..Default::default()
            };
            self.check(
                // SAFETY: `enableEncodeAsync` is set only if nvEncRegisterAsyncEvent exists;
                // `event` is the slot's handle.
                unsafe { (self.functions.nvEncRegisterAsyncEvent.unwrap())(self.raw, &mut event) },
                "register completion event",
            )?;
            self.slots[index].event_registered = true;
        }
        Ok(())
    }
    pub(super) fn texture_slot(&mut self, texture: &ID3D11Texture2D) -> Result<usize> {
        let key = texture.as_raw() as usize;
        if let Some(index) = self.slots.iter().position(|slot| slot.input.key == key) {
            return Ok(index);
        }
        let input = if let Some(context) = &self.cuda {
            let cuda = cuda::Input::new(context, texture, self.stream.width, self.stream.height)?;
            Input {
                key,
                raw: cuda.pointer as usize as *mut c_void,
                kind: _NV_ENC_INPUT_RESOURCE_TYPE_NV_ENC_INPUT_RESOURCE_TYPE_CUDADEVICEPTR,
                pitch: cuda.pitch,
                _texture: None,
                cuda: Some(cuda),
                d3d12: None,
            }
        } else {
            Input {
                key,
                raw: texture.as_raw(),
                kind: _NV_ENC_INPUT_RESOURCE_TYPE_NV_ENC_INPUT_RESOURCE_TYPE_DIRECTX,
                pitch: 0,
                _texture: Some(texture.clone()),
                cuda: None,
                d3d12: None,
            }
        };
        self.slot(input)
    }
    /// The slot of a D3D12 conversion target, registered the first time the
    /// target comes round with the READBACK buffer `output` makes.
    pub(super) fn resource_slot(
        &mut self,
        resource: &ID3D12Resource,
        wait: Option<(*mut c_void, u64)>,
        output: impl FnOnce() -> Result<(ID3D12Resource, u32)>,
    ) -> Result<usize> {
        let key = resource.as_raw() as usize;
        if let Some(index) = self.slots.iter().position(|slot| slot.input.key == key) {
            return Ok(index);
        }
        let (buffer, size) = output()?;
        self.slot(Input {
            key,
            raw: resource.as_raw(),
            kind: _NV_ENC_INPUT_RESOURCE_TYPE_NV_ENC_INPUT_RESOURCE_TYPE_DIRECTX,
            pitch: 0,
            _texture: None,
            cuda: None,
            d3d12: Some(D3d12Input {
                _resource: Some(resource.clone()),
                wait,
                output: buffer.as_raw(),
                size,
                _output: Some(buffer),
            }),
        })
    }
    pub(super) fn slot_pending(&self, slot: usize) -> bool {
        self.pending.iter().any(|pending| pending.slot == slot)
    }
    pub(super) fn slot_has_key(&self, slot: usize, key: usize) -> bool {
        key != 0
            && self
                .slots
                .get(slot)
                .is_some_and(|entry| entry.input.key == key)
    }
    pub(super) fn clear_inputs(&mut self) -> Result<()> {
        self.drain()?;
        let _guard = self.guard()?;
        for slot in &mut self.slots {
            cleanup_slot(&self.functions, self.raw, self.api, slot)?;
        }
        self.slots.clear();
        Ok(())
    }
}
