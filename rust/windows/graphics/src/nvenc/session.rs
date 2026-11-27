//! NVENC session opening, API version negotiation, codec/capability queries,
//! per-picture defaults, and drain waiter adapter.

use super::*;

impl Session {
    pub(super) fn set_hdr_metadata(&mut self, metadata: butterpollo_media_policy::hdr::Metadata) {
        self.metadata.update(metadata, self.stream.codec);
    }
    /// Refuse a session a stream has set up ([`Encoder::probe_focus`]
    /// reopens it without that).
    pub(super) fn startup_only(&self) -> Result<()> {
        anyhow::ensure!(
            self.frame_cap_target.is_none()
                && self.cut_limit.is_none()
                && !self.slices_joined
                && !self.qp_map,
            "the NVENC focus probe runs only on the startup check's encoder, before a stream sets it up"
        );
        Ok(())
    }
    /// Open a session on `device`. With `fence` the device is an
    /// `ID3D12Device`: inputs and outputs are D3D12 resources, each picture
    /// waits for its conversion's fence and signals `fence` when written.
    pub(super) fn open(
        runtime: Arc<Runtime>,
        device: *mut c_void,
        kind: NV_ENC_DEVICE_TYPE,
        cuda: Option<Rc<cuda::Context>>,
        fence: Option<Rc<dyn Fence>>,
        stream: &Negotiated,
        tuning: &Tuning,
    ) -> Result<Self> {
        let _guard = cuda.as_ref().map(|ctx| ctx.enter()).transpose()?;
        let mut failures = Vec::new();
        let candidates = ApiVersion::candidates(stream.codec, runtime.maximum)
            .filter(|api| fence.is_none() || api.d3d12())
            .collect::<Vec<_>>();
        if candidates.is_empty() && fence.is_some() {
            bail!(
                "NVENC on the D3D12 conversion needs driver API 12.0 (NVIDIA driver 522.25 or newer); this driver offers {}.{}",
                runtime.maximum.0,
                runtime.maximum.1
            );
        }
        for api in candidates {
            match Self::open_version(
                runtime.clone(),
                device,
                kind,
                cuda.clone(),
                fence.clone(),
                stream,
                tuning,
                api,
            ) {
                Ok(mut session) => {
                    session.api_fallbacks = std::mem::take(&mut failures);
                    return Ok(session);
                }
                Err(error) if retry_version(&error) => {
                    tracing::warn!(error = %format!("{error:#}"), major = api.0, minor = api.1, "NVENC rejected this driver API; retrying a reviewed older API. Update the NVIDIA driver if encoding remains unstable");
                    failures.push(format!("{}.{}: {error}", api.0, api.1))
                }
                Err(error) => return Err(error),
            }
        }
        bail!(
            "no reviewed NVENC driver API supports this stream: {}",
            api_failures(stream.codec, runtime.maximum, &failures)
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn open_version(
        runtime: Arc<Runtime>,
        device: *mut c_void,
        kind: NV_ENC_DEVICE_TYPE,
        cuda: Option<Rc<cuda::Context>>,
        fence: Option<Rc<dyn Fence>>,
        stream: &Negotiated,
        tuning: &Tuning,
        api: ApiVersion,
    ) -> Result<Self> {
        let mut functions = NV_ENCODE_API_FUNCTION_LIST {
            version: api.structure(2, false),
            ..Default::default()
        };
        check_raw(
            // SAFETY: `create` is NvEncodeAPICreateInstance from the still-loaded DLL and
            // `functions` is versioned.
            unsafe { (runtime.create)(&mut functions) },
            "create function table",
            &functions,
            ptr::null_mut(),
        )?;
        validate(&functions)?;
        let mut session = Self {
            raw: ptr::null_mut(),
            functions,
            api,
            api_fallbacks: Vec::new(),
            _runtime: runtime,
            cuda,
            d3d12: fence.map(|fence| D3d12 {
                fence,
                value: 0,
                submitted: 0,
            }),
            qp_map: false,
            block_stats: false,
            next_map: None,
            map_scratch: Vec::new(),
            picture: Self::picture(api, stream),
            measure_next: false,
            stats_buffer: Vec::new(),
            qp_grids: VecDeque::new(),
            config: Box::default(),
            initialize: Box::default(),
            stream: stream.clone(),
            metadata: Metadata::new(stream.codec),
            submitted_metadata: None,
            slots: Vec::new(),
            pending: VecDeque::new(),
            ready: VecDeque::new(),
            references: References::default(),
            retained: 1,
            invalidation: false,
            dynamic_bitrate: false,
            rate_control: RateControl::Cbr,
            budget: None,
            vbv_ceiling: 0,
            vbv_increase: 0,
            cap_requested: None,
            cap_refused: false,
            frame_cap_target: None,
            frame_cap_floor: 0,
            frame_cap_scaled: None,
            keyframe_cap: KeyframeCapScale::default(),
            cap_scale_applied: 1.,
            cut_limit: None,
            reencoded_cuts: 0,
            keyframes_over_cap: 0,
            rfi_fallbacks: 0,
            oversize_warned: None,
            reencode_failure: None,
            armed: std::cell::Cell::new(0),
            drain_event: None,
            drain_armed: 0,
            slice_blocks: 0,
            subframe: false,
            subframe_slices: 0,
            subframe_traced: 0,
            offsets_warned: false,
            slices_joined: false,
            slice_offsets: Vec::new(),
            unit_idr_reported: false,
            next: 1,
            last_idr: 0,
            dynamic_headroom: 0,
            initialized: false,
            force_idr: false,
        };
        let mut open = NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS {
            version: api.structure(1, false),
            deviceType: kind,
            device,
            apiVersion: api.packed(),
            ..Default::default()
        };
        // The owner exists before the driver call, so even a partially opened
        // rejected version is destroyed before the next candidate is tried.
        // SAFETY: `validate` confirmed nvEncOpenEncodeSessionEx exists, `device` is the caller's
        // live D3D11 or D3D12 device or CUDA context, and both out-pointers are live locals.
        let status =
            unsafe { (functions.nvEncOpenEncodeSessionEx.unwrap())(&mut open, &mut session.raw) };
        session.check(status, "open session").map_err(|error| {
            if matches!(
                status,
                _NVENCSTATUS_NV_ENC_ERR_OUT_OF_MEMORY
                    | _NVENCSTATUS_NV_ENC_ERR_INCOMPATIBLE_CLIENT_KEY
            ) {
                error.context(SessionLimit {
                    memory: status == _NVENCSTATUS_NV_ENC_ERR_OUT_OF_MEMORY,
                })
            } else {
                error
            }
        })?;
        if session.raw.is_null() {
            bail!("NVENC returned no session");
        }
        session.configure(tuning)?;
        Ok(session)
    }
    pub(super) fn guard(&self) -> Result<Option<cuda::Guard>> {
        if self.raw.is_null() {
            bail!("the NVENC session is closed");
        }
        self.cuda.as_ref().map(|ctx| ctx.enter()).transpose()
    }
    /// The session's picture parameters, zeroed once with the fields every
    /// picture shares; each submission resets the rest.
    fn picture(api: ApiVersion, stream: &Negotiated) -> NV_ENC_PIC_PARAMS {
        NV_ENC_PIC_PARAMS {
            version: api.picture(),
            inputWidth: stream.width,
            inputHeight: stream.height,
            pictureStruct: _NV_ENC_PIC_STRUCT_NV_ENC_PIC_STRUCT_FRAME,
            ..Default::default()
        }
    }
    pub(super) fn check(&self, status: NVENCSTATUS, operation: &'static str) -> Result<()> {
        check_raw(status, operation, &self.functions, self.raw)
    }
    pub(super) fn cap(&self, cap: NV_ENC_CAPS) -> Result<i32> {
        let mut parameters = NV_ENC_CAPS_PARAM {
            version: self.api.structure(1, false),
            capsToQuery: cap,
            ..Default::default()
        };
        let mut value = 0;
        self.check(
            // SAFETY: `self.raw` is the open session, `validate` checked nvEncGetEncodeCaps, and
            // outputs are locals.
            unsafe {
                (self.functions.nvEncGetEncodeCaps.unwrap())(
                    self.raw,
                    codec_guid(self.stream.codec)?,
                    &mut parameters,
                    &mut value,
                )
            },
            "query capability",
        )?;
        Ok(value)
    }
    pub(super) fn optional_cap(&self, cap: NV_ENC_CAPS) -> bool {
        self.cap(cap).unwrap_or(0) > 0
    }
    /// A GPU whose NVENC lacks the stream's codec is refused by name, before
    /// any capability query fails less clearly. Drivers without the list
    /// leave it to the session.
    pub(super) fn check_codec(&self) -> Result<()> {
        let (Some(count), Some(list)) = (
            self.functions.nvEncGetEncodeGUIDCount,
            self.functions.nvEncGetEncodeGUIDs,
        ) else {
            return Ok(());
        };
        let mut total = 0;
        // SAFETY: `self.raw` is the open session and `total` a live local.
        self.check(unsafe { count(self.raw, &mut total) }, "count codecs")?;
        let mut guids = vec![GUID::default(); total.min(64) as usize];
        let mut written = 0;
        self.check(
            // SAFETY: `guids` holds `guids.len()` writable GUIDs and `written` is a live local.
            unsafe {
                list(
                    self.raw,
                    guids.as_mut_ptr(),
                    guids.len() as u32,
                    &mut written,
                )
            },
            "list codecs",
        )?;
        let wanted = codec_guid(self.stream.codec)?;
        if !guids[..(written as usize).min(guids.len())]
            .iter()
            .any(|guid| same_guid(guid, &wanted))
        {
            bail!(
                "this NVIDIA GPU's NVENC cannot encode {}. Select another codec on the device",
                codec_name(self.stream.codec)
            );
        }
        Ok(())
    }
    /// HDR metadata for the next picture's bitstream (API 13, HEVC and AV1).
    pub(super) fn encoded_metadata(&self) -> Option<EncodedMetadata> {
        (self.stream.hdr && self.stream.codec != 0 && self.api >= ApiVersion(13, 0))
            .then(|| self.metadata.encoded())
    }
    /// Whether the next picture, asked as `idr`, is coded as a keyframe. The
    /// driver emits HDR metadata on keyframes, so a change in the values
    /// actually encoded asks for one; the old snapshot is kept until a
    /// submission is accepted, so a failed attempt cannot consume an update.
    pub(super) fn keyframe(&self, idr: bool) -> bool {
        idr || self.force_idr || self.encoded_metadata() != self.submitted_metadata
    }
}
/// A drain's view of a session: its frames in flight and the time it has
/// taken so far.
pub(super) struct SessionDrain<'a> {
    pub(super) session: &'a mut Session,
    pub(super) started: Instant,
}
impl drain::DrainWaiter for SessionDrain<'_> {
    fn collect_pending(&mut self) -> Result<usize> {
        if self.session.pending.is_empty() {
            return Ok(0);
        }
        self.session.collect()?;
        Ok(self.session.pending.len())
    }
    fn wait_oldest(&mut self, timeout: Duration) -> Result<drain::DrainWait> {
        self.session.wait_oldest(timeout)
    }
    fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
    fn pause(&mut self) {
        std::thread::sleep(Duration::from_millis(1));
    }
}
