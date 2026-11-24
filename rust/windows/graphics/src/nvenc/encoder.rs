//! Public NVENC `Encoder` implementation: creation, D3D12 compute / D3D11 / CUDA
//! conversion dispatch, repeat-frame caching, dynamic HDR splicing, and polling.

use super::*;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST,
};

/// What the session codes for `config`: a packed 4:4:4 stream is a larger
/// full-range BT.709 SDR 4:2:0 picture (`Negotiated::coded_size`), as AMF
/// codes it; the device unpacks it and takes HDR from the control channel.
pub(super) fn coded(config: &Negotiated) -> Negotiated {
    let mut coded = config.clone();
    if config.chroma_packing != 0 {
        (coded.width, coded.height) = config.coded_size();
        coded.sdr_10bit = config.ten_bit();
        coded.hdr = false;
        coded.csc_mode = 3;
        coded.chroma_packing = 0;
    }
    coded
}
/// A native 4:4:4 stream the SDK this host uses cannot configure (AV1, see
/// `nvenc::sdk_444_refusal`) is refused before a session opens; the GPU's
/// `NV_ENC_CAPS_SUPPORT_YUV444_ENCODE` decides the rest in `configure`.
/// Packed 4:4:4 codes 4:2:0 and is unaffected. A refused stream fails with
/// this reason: it is never encoded in 4:2:0 instead.
pub(super) fn refuse_native_444(coded: &Negotiated) -> Result<()> {
    if !coded.yuv444 {
        return Ok(());
    }
    match butterpollo_media_policy::nvenc::sdk_444_refusal(coded.codec) {
        Some(reason) => bail!(
            "NVENC cannot encode {} 4:4:4: {reason}",
            codec_name(coded.codec)
        ),
        None => Ok(()),
    }
}

/// The statistics pass behind the conversion a frame takes: the compute
/// queue's, or the D3D11 conversion's (as `amf::stats`).
fn stats<'a>(
    compute: Option<&'a mut crate::compute::Converter>,
    graphics: Option<&'a mut crate::gpu_color::HdrStats>,
) -> Option<&'a mut dyn crate::amf::dynamic::Stats> {
    match compute {
        Some(converter) => Some(converter),
        None => graphics.map(|stats| stats as &mut dyn crate::amf::dynamic::Stats),
    }
}
/// A D3D12 session's READBACK buffer: twice an uncompressed picture, within
/// the largest frame collected.
pub(super) fn readback_size(stream: &Negotiated) -> u32 {
    let pixels = u64::from(stream.width) * u64::from(stream.height);
    let samples = if stream.yuv444 {
        pixels * 3
    } else {
        pixels * 3 / 2
    };
    let bytes = samples * if stream.ten_bit() { 2 } else { 1 } * 2;
    bytes.min(MAX_FRAME_BYTES as u64) as u32
}
/// Dynamic HDR metadata spliced into each picture or unit
/// ([`crate::amf::dynamic::output`]): before the first slice, the Dolby
/// Vision RPU after the last.
pub(super) fn splice(dynamic: Option<&mut crate::amf::dynamic::Dynamic>, frame: &mut Encoded) {
    let (bytes, cap) = crate::amf::dynamic::output(
        dynamic,
        frame.unit,
        std::mem::take(&mut frame.bytes),
        frame.frame_cap_bytes,
        frame.idr,
    );
    frame.bytes = bytes;
    frame.frame_cap_bytes = cap;
}

impl Encoder {
    pub fn new_device_options(config: &Negotiated, gpu: Device, options: &Config) -> Result<Self> {
        Self::new_device_reported(config, gpu, options, &recorded_warnings())
    }
    pub fn new_device_reported(
        config: &Negotiated,
        gpu: Device,
        options: &Config,
        warnings: &Arc<dyn Warnings>,
    ) -> Result<Self> {
        Self::new_gpu(config, gpu, options, warnings, None)
    }
    /// NVENC on the capture's D3D11 device, or with `compute` on the compute
    /// queue's D3D12 device, encoding what its conversion writes as AMF does
    /// (API 12.0, driver 522.25 and newer).
    pub fn new_gpu(
        config: &Negotiated,
        gpu: Device,
        options: &Config,
        warnings: &Arc<dyn Warnings>,
        compute: Option<Arc<crate::compute::Compute>>,
    ) -> Result<Self> {
        if config.chroma_packing != 0 {
            config.validate()?;
        }
        let coded = coded(config);
        refuse_native_444(&coded)?;
        let runtime = Runtime::load()?;
        let mut tuning = Tuning::new(options, &coded)?;
        // 8-bit native 4:4:4 (AYUV) can convert on the D3D12 compute queue and
        // encode on its D3D12 device (Rubylight #150); 10-bit 4:4:4 stays on
        // D3D11 + CUDA because NVENC's D3D12 interface refuses YUV444_10BIT.
        let compute = compute.filter(|_| supports_compute(&coded));
        let converter = compute
            .map(|compute| -> Result<_> {
                let mut converter = if coded.yuv444 {
                    crate::compute::Converter::new_ayuv(compute, coded.width, coded.height)?
                } else {
                    crate::compute::Converter::new(
                        compute,
                        coded.width,
                        coded.height,
                        config.ten_bit(),
                    )?
                };
                if config.chroma_packing != 0 {
                    converter.pack_chroma()?;
                }
                converter.prepare_yuv()?;
                Ok(Box::new(converter))
            })
            .transpose()?;
        if coded.yuv444 {
            use butterpollo_media_policy::nvenc::Input444;
            let input = match (
                butterpollo_media_policy::nvenc::input_444(coded.ten_bit()),
                converter.is_some(),
            ) {
                (Input444::Ayuv, true) => "AYUV on the D3D12 compute device",
                (Input444::Ayuv, false) => "AYUV on the D3D11 device",
                (Input444::Planar16, _) => "three 16-bit planes copied through CUDA",
            };
            tracing::info!(
                codec = codec_name(coded.codec),
                ten_bit = coded.ten_bit(),
                d3d12 = converter.is_some(),
                input,
                "NVENC native 4:4:4 stream"
            );
        }
        let cuda = if converter.is_none() && coded.yuv444 && coded.ten_bit() {
            Some(cuda::Context::new(&gpu)?)
        } else {
            None
        };
        let (device, kind, fence) = if let Some(context) = &cuda {
            (
                context.raw(),
                _NV_ENC_DEVICE_TYPE_NV_ENC_DEVICE_TYPE_CUDA,
                None,
            )
        } else {
            let (device, fence) = directx_device(converter.as_deref(), &gpu)?;
            (
                device,
                _NV_ENC_DEVICE_TYPE_NV_ENC_DEVICE_TYPE_DIRECTX,
                fence,
            )
        };
        // A stream turns the delta map on with focus (`enable_qp_map`).
        tuning.qp_map = false;
        let session = Session::open(runtime, device, kind, cuda, fence, &coded, &tuning)?;
        let api_warning = (!session.api_fallbacks.is_empty())
            .then(|| api_fallback_warning(&session.api_fallbacks, session.api, &coded, &tuning));
        let metadata = gpu.hdr_metadata();
        let mut encoder = Self {
            session,
            compute: converter,
            gpu,
            config: config.clone(),
            converter: None,
            compute_fallback: false,
            source: None,
            luminance: [100., 1.],
            overlay: None,
            warnings: warnings.clone(),
            dynamic: None,
            stats: None,
            last_submitted: None,
            cap_warned: false,
            tuning,
            focus: Default::default(),
            asked: VecDeque::new(),
            answers: Vec::new(),
            probing: false,
            repeat: false,
            last_converted: None,
            output_event: OutputEvent::Off,
        };
        if options.nvenc_output_event() {
            if encoder.session.d3d12.is_some() {
                match crate::timing::Signal::new() {
                    Ok(event) => encoder.output_event = OutputEvent::D3d12(event),
                    Err(error) => {
                        tracing::warn!(
                            "NVENC output event unavailable ({error:#}); polling for output"
                        )
                    }
                }
            } else if encoder.session.initialize.enableEncodeAsync != 0 {
                encoder.output_event = OutputEvent::Async;
            }
        }
        if let Some(warning) = api_warning {
            // The older API drops HDR10 metadata or split encoding; say so
            // rather than leave it to a log line.
            encoder.warnings.set("nvenc_api", &warning);
        }
        encoder.dynamic = encoder.enable_dynamic(options);
        if encoder.dynamic.is_some() {
            encoder.session.dynamic_headroom = DYNAMIC_HEADROOM;
        }
        encoder.set_hdr_metadata(metadata);
        Ok(encoder)
    }
    /// HDR10+ and HDR Vivid for an HDR stream with `hdr_dynamic_metadata`
    /// on, and Dolby Vision RPUs for a stream that picked it, as AMF sends
    /// them ([`crate::amf::dynamic::start`]). Asks with the negotiated
    /// stream, not the coded one: a packed 4:4:4 stream codes SDR.
    fn enable_dynamic(&mut self, options: &Config) -> Option<Box<crate::amf::dynamic::Dynamic>> {
        let wanted = crate::amf::dynamic::wanted(&self.config, options)?;
        let packed = self.config.chroma_packing != 0;
        if !packed && self.compute.is_none() {
            self.stats = Some(Box::new(crate::gpu_color::HdrStats::new(&self.gpu)));
        }
        crate::amf::dynamic::start(
            wanted,
            packed,
            stats(self.compute.as_deref_mut(), self.stats.as_deref_mut()),
            &self.warnings,
        )
    }
    pub fn set_luminance(&mut self, luminance: [f32; 2]) {
        if self.luminance != luminance {
            self.luminance = luminance;
            self.last_converted = None;
        }
    }
    pub fn set_repeat(&mut self, repeat: bool) {
        self.repeat = repeat;
    }
    /// HDR10 static metadata for the bitstream (API 13 and newer) and as the
    /// stand-in of dynamic metadata.
    pub fn set_hdr_metadata(&mut self, metadata: butterpollo_media_policy::hdr::Metadata) {
        if let Some(dynamic) = self.dynamic.as_deref_mut() {
            dynamic.set_metadata(metadata);
        }
        self.session.set_hdr_metadata(metadata);
    }
    /// The receiver's display peak, for dynamic metadata that names one.
    pub fn set_hdr_target(&mut self, nits: u16) {
        if let Some(dynamic) = self.dynamic.as_deref_mut() {
            dynamic.set_target(nits);
        }
    }
    pub fn set_next_frame(&mut self, frame: u64) {
        debug_assert!(self.session.pending.is_empty() && self.session.ready.is_empty());
        self.session.next = frame.max(1);
    }
    pub fn supports_invalidation(&self) -> bool {
        self.session.invalidation
    }
    pub fn invalidate_ref_frames(&mut self, first: u64, last: u64) -> bool {
        match self.session.invalidate(first, last) {
            Ok(applied) => applied,
            Err(error) => {
                tracing::warn!(%error, "NVENC reference recovery failed; requesting a larger IDR recovery frame. Reduce bitrate on lossy links or update the NVIDIA driver");
                self.session.force_idr = true;
                false
            }
        }
    }
    pub fn pending(&self) -> bool {
        !self.session.pending.is_empty() || !self.session.ready.is_empty()
    }
    /// Frames submitted and not yet encoded.
    pub fn backlog(&self) -> usize {
        self.session.pending.len()
    }
    /// When NVENC accepted the last submitted picture.
    pub fn last_submitted(&self) -> Option<Instant> {
        self.last_submitted
    }
    /// The GPU this encoder opened on.
    pub fn adapter(&self) -> &str {
        &self.gpu.display.adapter
    }
    pub fn device_removed(&self) -> Option<crate::device_loss::DeviceLost> {
        crate::device_loss::DeviceLost::d3d11(&self.gpu.device).or_else(|| {
            self.compute.as_ref().and_then(|converter| {
                crate::device_loss::DeviceLost::d3d12(&converter.compute().device)
            })
        })
    }
    /// AMF's throughput names answered from `NV_ENC_CAPS_MB_PER_SEC_MAX`, in
    /// 16x16 macroblocks per second, for the session's own codec.
    pub fn capability(&self, name: &str) -> Option<i64> {
        if throughput_codec(name)? != self.session.stream.codec {
            return None;
        }
        let _guard = self.session.guard().ok()?;
        self.session
            .cap(_NV_ENC_CAPS_NV_ENC_CAPS_MB_PER_SEC_MAX)
            .ok()
            .filter(|rate| *rate > 0)
            .map(i64::from)
    }
    /// The frame budget guard's verdict on this session's preset.
    pub fn budget(&self) -> Option<Budget> {
        self.session.budget
    }
    /// The host's pace-based frame cap ([`Session::set_frame_cap`]); coalesced
    /// with any bitrate change on the next submission.
    pub fn set_frame_cap(&mut self, bytes: u64) -> Option<u64> {
        self.session.queue_frame_cap(bytes)
    }
    pub fn frame_cap(&self) -> Option<u64> {
        self.session.frame_cap()
    }
    /// The largest inter frame sent as encoded; a larger one is encoded again
    /// as a keyframe, as AMF does.
    pub fn set_cut_limit(&mut self, bytes: u64) {
        self.session.cut_limit = (bytes > 0).then_some(bytes);
    }
    /// Inter frames encoded again as keyframes since the encoder opened.
    pub fn reencoded_cuts(&self) -> u64 {
        self.session.reencoded_cuts
    }
    /// Keyframes more than 1.1 times the cap the host asked for.
    pub fn keyframes_over_cap(&self) -> u64 {
        self.session.keyframes_over_cap
    }
    /// Blocks a P-frame is sent in as NVENC writes its slices; 0 when
    /// `video_slice_delivery` is off, the driver has no subframe readback,
    /// and after [`Self::join_slices`].
    pub fn slice_delivery(&self) -> u8 {
        if self.session.subframe && !self.session.slices_joined {
            self.session.slice_blocks
        } else {
            0
        }
    }
    /// Return every later frame whole, from the next one on. A frame whose
    /// first unit has left is finished as it began.
    pub fn join_slices(&mut self) {
        self.session.slices_joined = true;
    }
    /// Snapshot only at failure/recreation, before dropping the driver state.
    pub fn log_stall(&self) {
        let oldest = self.session.pending.front();
        // SAFETY: the D3D11 device is owned by this encoder and outlives the
        // call; GetDeviceRemovedReason only reads its state.
        let removed = unsafe { self.gpu.device.GetDeviceRemovedReason() };
        tracing::warn!(
            in_flight = self.session.pending.len(),
            ready = self.session.ready.len(),
            oldest_frame = ?oldest.map(|pending| pending.frame),
            oldest_ms = ?oldest.map(|pending| pending.started.elapsed().as_millis()),
            next_frame = self.session.next,
            asynchronous = self.session.initialize.enableEncodeAsync != 0,
            d3d12 = self.compute.is_some(),
            last_submit = ?self.last_submitted.map(|at| at.elapsed().as_millis()),
            d3d11_removed = ?removed,
            "NVENC stall snapshot"
        );
        if let Some(converter) = &self.compute {
            converter.log_stall();
        }
    }
    pub fn poll(&mut self) -> Result<Vec<Encoded>> {
        self.finish_poll(true)
    }
    fn finish_poll(&mut self, collect: bool) -> Result<Vec<Encoded>> {
        if let Some(converter) = self.compute.as_deref_mut() {
            converter.release_completed();
        }
        let mut frames = if collect {
            self.session.poll()?
        } else {
            self.session.ready.drain(..).collect()
        };
        self.focus_feedback();
        if let Some(error) = self.session.reencode_failure.take() {
            self.warnings.event("encoder_reencode", &format!("NVENC could not encode an oversize scene cut again as a keyframe ({error}); the frame goes without full FEC and may freeze the picture briefly. Report this with the host log."), butterpollo_media_policy::EVENT_PERIOD);
        }
        for frame in &mut frames {
            splice(self.dynamic.as_deref_mut(), frame);
        }
        Ok(frames)
    }
    /// An event set once the oldest frame in flight is written, armed by
    /// this call (or the oldest pending slot's completion event on async
    /// D3D11/CUDA sessions); `None` when there is nothing to wait for or
    /// `nvenc_output_event` is off.
    pub fn output_event(&self) -> Option<&crate::timing::Signal> {
        match &self.output_event {
            OutputEvent::Off => None,
            OutputEvent::Async => self.session.async_output_event(),
            OutputEvent::D3d12(event) => match self.session.arm_output(event) {
                Ok(armed) => armed.then_some(event),
                Err(error) => {
                    self.warnings.event(
                        "encoder_output_event",
                        &format!("NVENC could not arm its output event ({error:#}); polling for output instead. Report this with the host log."),
                        butterpollo_media_policy::EVENT_PERIOD,
                    );
                    None
                }
            },
        }
    }
    pub fn accepts_gpu_device(&self, image: &GpuImage) -> bool {
        self.gpu.device.as_raw() == image.gpu.device.as_raw()
            && (self.compute.is_none() || crate::compute::shareable(&image.texture))
    }
    /// Whether this encoder's frames are converted on the D3D12 compute queue.
    pub fn converts_on_compute(&self) -> bool {
        self.compute.is_some()
    }
    /// Record that the caller fell back to the graphics queue because the
    /// D3D12 session failed; later rebuilds must not retry it this stream.
    pub fn mark_compute_fallback(&mut self) {
        self.compute_fallback = true;
    }
    /// Whether this encoder was built after a D3D12 fallback.
    pub fn compute_fell_back(&self) -> bool {
        self.compute_fallback
    }
    /// A CPU image, such as the codec probe's, goes through the same GPU
    /// conversion as a capture.
    pub fn encode(&mut self, image: &Image, idr: bool, bitrate: u32) -> Result<Vec<Encoded>> {
        anyhow::ensure!(
            self.overlay.is_none(),
            "couch menu: needs wgc_direct_encoder_input"
        );
        let gpu = GpuImage::upload(&self.gpu, image)?;
        self.encode_gpu(&gpu, idr, bitrate)
    }
    pub fn encode_gpu(
        &mut self,
        image: &GpuImage,
        idr: bool,
        bitrate: u32,
    ) -> Result<Vec<Encoded>> {
        let repeat = std::mem::take(&mut self.repeat);
        if !self.accepts_gpu_device(image) {
            bail!("NVENC capture device changed; recreate the encoder");
        }
        self.session.collect()?;
        match self.session.apply_bitrate(bitrate, true)? {
            Ok(()) => {
                if !self.session.cap_refused && std::mem::take(&mut self.cap_warned) {
                    self.warnings.clear("encoder_frame_size");
                }
            }
            Err(error) => {
                let bytes = self.session.frame_cap_target.unwrap_or(0);
                self.cap_warned = true;
                tracing::warn!(error = %format!("{error:#}"), bytes, "NVENC could not hold frames to the frame cap; keeping its one-frame VBV");
                self.warnings.set("encoder_frame_size", &format!("NVENC could not lower its frame size limit to {bytes} bytes ({error:#}); large frames may leave without full FEC. Update the NVIDIA driver."));
            }
        }
        if self.session.pending.len() >= MAX_PENDING {
            self.session.drain()?;
        }
        let focus_active = self.focus.settings.is_some() || self.focus.fixed.is_some();
        let cached_hit = if repeat
            && !focus_active
            && let Some(cached) = &self.last_converted
            && cached.matches(image, self.luminance, self.overlay.as_ref())
            && !self.session.slot_pending(cached.slot)
            && self.session.slot_has_key(cached.slot, cached.slot_key)
        {
            Some((cached.slot, cached.kept.clone()))
        } else {
            None
        };
        if let Some((slot, kept)) = cached_hit {
            self.attach_focus(idr, None);
            self.session
                .submit_frame(slot, Some(kept), idr, image.captured, None, true)?;
            self.last_submitted = Some(Instant::now());
            return self.finish_poll(self.session.pending.len() > 1);
        }
        self.last_converted = None;
        let source = (image.width, image.height, image.pixel);
        if let Some(converter) = self.compute.as_deref_mut() {
            let compute = converter.compute().clone();
            let texture = compute.open(&image.texture)?;
            let pointer = image
                .cursor
                .as_ref()
                .map(|cursor| converter.pointer(cursor))
                .transpose()?;
            converter.values = crate::gpu_color::constants(
                &self.config,
                source,
                self.luminance,
                image.cursor.as_ref(),
            );
            converter.set_overlay(self.overlay.as_ref());
            let converted = converter.convert(
                &texture,
                crate::compute::format(image.pixel),
                pointer.as_ref(),
                image,
            )?;
            let size = readback_size(&self.session.stream);
            let slot_key = converted.texture.as_raw() as usize;
            let slot = self.session.resource_slot(
                &converted.texture,
                Some((converted.fence.as_raw(), converted.value)),
                || {
                    let buffer = crate::compute::focus_buffer(
                        &compute.device,
                        D3D12_HEAP_TYPE_READBACK,
                        u64::from(size),
                        D3D12_RESOURCE_STATE_COPY_DEST,
                    )
                    .context("NVENC D3D12 output buffer")?;
                    Ok((buffer, size))
                },
            )?;
            let kept = Kept::D3d12 {
                fence: converted.fence.as_raw(),
                value: converted.value,
                _texture: Some(converted.texture),
                _fence: Some(converted.fence),
            };
            self.attach_focus(idr, Some(converted.conversion));
            self.session
                .submit(slot, Some(kept.clone()), idr, image.captured)?;
            if !focus_active {
                self.last_converted = Some(CachedConversion::new(
                    image,
                    self.luminance,
                    self.overlay.as_ref(),
                    slot,
                    slot_key,
                    kept,
                ));
            }
            self.last_submitted = Some(Instant::now());
            crate::amf::dynamic::sample(
                &mut self.dynamic,
                stats(self.compute.as_deref_mut(), None),
                &self.warnings,
            );
            if self.dynamic.is_none() {
                self.session.dynamic_headroom = 0;
            }
            return self.finish_poll(self.session.pending.len() > 1);
        }
        if self.source != Some(source) {
            self.session.clear_inputs()?;
            self.converter = Some(crate::gpu_color::Converter::new(
                &self.gpu,
                &self.config,
                source,
            )?);
            self.source = Some(source);
        }
        let converter = self.converter.as_mut().unwrap();
        converter.set_luminance(self.luminance);
        converter.set_overlay(self.overlay.as_ref());
        let converted = converter.convert(image)?;
        // Submitted with `dynamic::sample`, once the frame went to NVENC.
        if let (Some(stats), Some(converter)) = (self.stats.as_deref_mut(), &self.converter) {
            stats.queue(image, converter);
        }
        let slot_key = converted.as_raw() as usize;
        let slot = self.session.texture_slot(converted.as_ref())?;
        let kept = Kept::D3d11 {
            _texture: converted,
        };
        self.attach_focus(idr, None);
        self.session
            .submit(slot, Some(kept.clone()), idr, image.captured)?;
        if !focus_active {
            self.last_converted = Some(CachedConversion::new(
                image,
                self.luminance,
                self.overlay.as_ref(),
                slot,
                slot_key,
                kept,
            ));
        }
        self.last_submitted = Some(Instant::now());
        // Before the poll, so the frame it returns carries the newest
        // finished sample.
        crate::amf::dynamic::sample(&mut self.dynamic, self.stats.as_deref_mut(), &self.warnings);
        if self.dynamic.is_none() {
            self.session.dynamic_headroom = 0;
        }
        self.finish_poll(self.session.pending.len() > 1)
    }
}
