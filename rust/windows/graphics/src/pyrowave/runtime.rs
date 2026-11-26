//! PyroWave FFI loader, Vulkan completion waiting, adapter quarantine and flight queue.

use super::*;

/// `pyrowave_encoder_set_frame_context`: which of the SDK's readbacks the
/// next encode writes and the next readout reads, for two frames in flight.
pub(super) type SetFrameContext =
    unsafe extern "C" fn(p::pyrowave_encoder, std::ffi::c_int) -> p::pyrowave_result;
macro_rules! api {
    ($($name:ident:$ty:ty),* $(,)?) => {
        pub(super) struct Api { $(pub(super) $name:$ty,)* pub(super) set_frame_context: Option<SetFrameContext>, pub(super) _dll:libloading::Library }
        impl Api {
            pub(super) fn load() -> Result<Arc<Self>> {
                let path=std::env::current_exe()?.parent().context("executable directory unavailable")?.join("libpyrowave-shared-1.dll");
                // SAFETY: `dll` moves into the Api, keeping every copied symbol loaded; each
                // matches the 1.0 C header, and the version is checked below.
                unsafe {
                    let dll=libloading::Library::new(&path).with_context(||format!("loading {}",path.display()))?;
                    $(let $name=*dll.get::<$ty>(concat!("pyrowave_",stringify!($name),"\0").as_bytes())?;)*
                    // Optional: without it, one frame is in flight at a time.
                    let set_frame_context=dll.get::<SetFrameContext>(b"pyrowave_encoder_set_frame_context\0").ok().map(|symbol| *symbol);
                    let api=Self { $($name,)* set_frame_context, _dll:dll };
                    let (mut major,mut minor,mut patch)=(0,0,0);
                    (api.get_api_version)(&mut major,&mut minor,&mut patch);
                    if major!=1 { bail!("unsupported PyroWave ABI {major}.{minor}.{patch}; expected 1.x"); }
                    Ok(Arc::new(api))
                }
            }
        }
    };
}
api! {
    get_api_version:unsafe extern "C" fn(*mut u32,*mut u32,*mut u32),
    create_device_by_compat:unsafe extern "C" fn(u32,u32,*const p::pyrowave_uuid,*const p::pyrowave_uuid,*const p::pyrowave_luid,p::VkQueueGlobalPriority,*mut p::pyrowave_device)->p::pyrowave_result,
    device_get_global_priority:unsafe extern "C" fn(p::pyrowave_device)->p::VkQueueGlobalPriority,
    device_set_queue_type:unsafe extern "C" fn(p::pyrowave_device,p::VkQueueFlagBits)->p::pyrowave_result,
    device_confirm_interop_support:unsafe extern "C" fn(p::pyrowave_device)->bool,
    device_get_vk_device_handles:unsafe extern "C" fn(p::pyrowave_device,*mut p::VkInstance,*mut p::VkPhysicalDevice,*mut p::VkDevice),
    device_destroy:unsafe extern "C" fn(p::pyrowave_device),
    encoder_create:unsafe extern "C" fn(*const p::pyrowave_encoder_create_info,*mut p::pyrowave_encoder)->p::pyrowave_result,
    encoder_destroy:unsafe extern "C" fn(p::pyrowave_encoder),
    encoder_encode_gpu:unsafe extern "C" fn(p::pyrowave_encoder,*const p::pyrowave_gpu_sync_operation,*const p::pyrowave_gpu_sync_operation,*const p::pyrowave_gpu_buffers,*const p::pyrowave_rate_control)->p::pyrowave_result,
    encoder_compute_num_packets_with_padding:unsafe extern "C" fn(p::pyrowave_encoder,usize,usize,*mut usize)->p::pyrowave_result,
    encoder_get_mapped_raw_bitstream:unsafe extern "C" fn(p::pyrowave_encoder,*mut *const c_void,*mut usize,*mut *const c_void,*mut usize)->p::pyrowave_result,
    encoder_packetize_with_padding:unsafe extern "C" fn(p::pyrowave_encoder,*mut p::pyrowave_packet,usize,usize,*mut usize,*mut c_void,usize)->p::pyrowave_result,
    image_create:unsafe extern "C" fn(*const p::pyrowave_image_create_info,*mut p::pyrowave_image)->p::pyrowave_result,
    image_get_image_view:unsafe extern "C" fn(p::pyrowave_image,p::VkImageAspectFlagBits,p::VkImageUsageFlagBits,*mut p::pyrowave_image_view)->p::pyrowave_result,
    image_destroy:unsafe extern "C" fn(p::pyrowave_image),
    sync_object_create:unsafe extern "C" fn(*const p::pyrowave_sync_object_create_info,*mut p::pyrowave_sync_object)->p::pyrowave_result,
    sync_object_get_semaphore:unsafe extern "C" fn(p::pyrowave_sync_object)->p::VkSemaphore,
    sync_object_destroy:unsafe extern "C" fn(p::pyrowave_sync_object),
}
pub(super) fn check(code: p::pyrowave_result) -> Result<()> {
    if code == 0 {
        Ok(())
    } else {
        bail!("PyroWave runtime error {code}")
    }
}
/// The runtime's ordinary fence waits discard VkResult. Wait for the external
/// release ourselves, before its unchecked mapping/packetization can read stale
/// memory. This also replaces a D3D11 GPU wait which could wedge capture forever
/// when Vulkan failed to signal the shared fence.
pub(super) struct CompletionWait {
    device: p::VkDevice,
    wait: p::PFN_vkWaitSemaphores,
    _loader: libloading::Library,
}
impl CompletionWait {
    pub(super) fn new(api: &Api, device: p::pyrowave_device) -> Result<Self> {
        // SAFETY: Both entry points use the generated Vulkan ABI. The loader and
        // PyroWave device outlive every call through this function pointer.
        unsafe {
            let loader = crate::telemetry::system_library("vulkan-1.dll")?;
            let get = loader.get::<p::PFN_vkGetDeviceProcAddr>(b"vkGetDeviceProcAddr\0")?;
            let mut vk_device = ptr::null_mut();
            (api.device_get_vk_device_handles)(
                device,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut vk_device,
            );
            anyhow::ensure!(!vk_device.is_null(), "PyroWave Vulkan device unavailable");
            let symbol = get.context("Vulkan device dispatch unavailable")?(
                vk_device,
                c"vkWaitSemaphores".as_ptr(),
            );
            anyhow::ensure!(
                symbol.is_some(),
                "Vulkan timeline semaphore wait unavailable"
            );
            let wait =
                std::mem::transmute::<p::PFN_vkVoidFunction, p::PFN_vkWaitSemaphores>(symbol);
            Ok(Self {
                device: vk_device,
                wait,
                _loader: loader,
            })
        }
    }
    /// Whether Vulkan has signalled `value`, waiting at most `timeout`.
    pub(super) fn reached(
        &self,
        semaphore: p::VkSemaphore,
        value: u64,
        timeout: Duration,
    ) -> Result<bool> {
        let info = p::VkSemaphoreWaitInfo {
            sType: p::VkStructureType_VK_STRUCTURE_TYPE_SEMAPHORE_WAIT_INFO,
            pNext: ptr::null(),
            flags: 0,
            semaphoreCount: 1,
            pSemaphores: &semaphore,
            pValues: &value,
        };
        let timeout = u64::try_from(timeout.as_nanos()).unwrap_or(u64::MAX);
        // SAFETY: The imported semaphore belongs to this device; its owner and
        // the two borrowed locals remain alive for this bounded CPU wait.
        let result = unsafe { self.wait.unwrap()(self.device, &info, timeout) };
        reached(result)
    }
}
pub(super) fn completion_result(result: p::VkResult) -> Result<()> {
    match result {
        p::VkResult_VK_SUCCESS => Ok(()),
        p::VkResult_VK_ERROR_DEVICE_LOST => Err(anyhow::Error::new(
            crate::device_loss::DeviceLost(DXGI_ERROR_DEVICE_REMOVED.0),
        )
        .context("PyroWave Vulkan device lost (VK_ERROR_DEVICE_LOST)")),
        p::VkResult_VK_TIMEOUT => bail!("PyroWave GPU completion timed out after 2 seconds"),
        code => bail!("PyroWave GPU completion failed with Vulkan result {code}"),
    }
}
/// [`completion_result`] of a wait that may end before the value: `false`
/// when it timed out.
pub(super) fn reached(result: p::VkResult) -> Result<bool> {
    if result == p::VkResult_VK_TIMEOUT {
        return Ok(false);
    }
    completion_result(result).map(|()| true)
}
/// Unconfirmed submissions can still access their resources. Do not run their
/// destructors on timeout: retaining them until process exit is safer than a
/// use-after-free or an unbounded runtime/device-idle wait during recovery.
pub(super) fn retain_pending<T>(pending: &mut Option<T>) {
    if let Some(pending) = pending.take() {
        std::mem::forget(pending);
    }
}
/// Adapter identity survives device recreation. A timeout must not make the
/// recovery loop retain another full encoder on every attempt. Existing
/// concurrent encoders also check this before submitting their next frame.
#[derive(Default)]
pub(super) struct QuarantinedAdapters(pub(super) Mutex<HashSet<[u8; 8]>>);
impl QuarantinedAdapters {
    pub(super) fn check(&self, adapter: [u8; 8]) -> Result<()> {
        anyhow::ensure!(
            !self.0.lock().unwrap().contains(&adapter),
            "PyroWave is unavailable on this adapter after an unconfirmed GPU submission. Restart Rubylight to release the retained GPU resources, or use HEVC/AV1."
        );
        Ok(())
    }
    pub(super) fn block(&self, adapter: [u8; 8]) {
        self.0.lock().unwrap().insert(adapter);
    }
}
pub(super) fn quarantined_adapters() -> &'static QuarantinedAdapters {
    static ADAPTERS: OnceLock<QuarantinedAdapters> = OnceLock::new();
    ADAPTERS.get_or_init(Default::default)
}
pub fn available() -> bool {
    Api::load().is_ok()
}
/// A call this slow held the session thread: log which step blocked so a
/// cold driver compile or stuck queue does not hide inside `convert`.
pub(super) const SLOW_CALL: Duration = Duration::from_millis(50);

pub(super) fn timed<T>(call: &'static str, run: impl FnOnce() -> T) -> T {
    let started = Instant::now();
    let result = run();
    let elapsed = started.elapsed();
    if elapsed >= SLOW_CALL {
        tracing::warn!(
            call,
            elapsed_ms = elapsed.as_millis() as u64,
            "slow PyroWave call"
        );
    }
    result
}
pub(super) fn interval(config: &Negotiated) -> pyrowave::Interval {
    // A pause must not buy a larger burst when capture resumes, including for
    // legacy clients without record framing or critical FEC.
    pyrowave::Interval::new(
        butterpollo_media_policy::framegen::Rate(config.fps_millihz()).period(),
        true,
    )
}
/// Lets one line through per period, for the numbers of a warning whose text must stay fixed so
/// `Warnings` keeps it to one log line (a text that changes with every frame never dedups).
#[derive(Default)]
pub(super) struct Notice {
    last: Option<Instant>,
}
impl Notice {
    pub(super) fn due(&mut self, now: Instant, period: Duration) -> bool {
        if self.last.is_some_and(|at| now.duration_since(at) < period) {
            return false;
        }
        self.last = Some(now);
        true
    }
}
/// The text of the warning for a frame sent in index order though row order was asked for.
pub(super) fn row_order_dropped_message(packet_size: usize) -> String {
    format!(
        "PyroWave row order was dropped for frames that do not fit the wire budget at a packet size of {packet_size}. Raise the bitrate or turn the client's beam racing off."
    )
}
/// The text of the warning for a frame over the transport limit.
pub(super) fn frame_size_message(limit: usize) -> String {
    format!(
        "PyroWave frames were dropped: over the transport limit of {limit} bytes. Lower bitrate or resolution."
    )
}
/// Frames in flight at most: the SDK keeps a bitstream readback for each of
/// its frame contexts (`PYROWAVE_MAX_FRAME_CONTEXTS` in pyrowave.h).
pub(super) const MAX_DEPTH: usize = 2;
/// How long a frame's GPU work may take before it counts as unconfirmed.
pub(super) const COMPLETION_TIMEOUT: Duration = Duration::from_secs(2);
/// Whether a frame submitted at `submitted` is done, as `reached` reports it
/// waiting at most the given time: at once unless `block`, otherwise for what
/// is left of [`COMPLETION_TIMEOUT`]. A frame not done in that time is an
/// error, as is any failure of the wait.
pub(super) fn settle(
    reached: impl FnOnce(Duration) -> Result<bool>,
    submitted: Instant,
    now: Instant,
    block: bool,
) -> Result<bool> {
    let left = COMPLETION_TIMEOUT.saturating_sub(now.saturating_duration_since(submitted));
    if reached(if block { left } else { Duration::ZERO })? {
        return Ok(true);
    }
    if block || left.is_zero() {
        bail!("PyroWave GPU completion timed out after 2 seconds");
    }
    Ok(false)
}
/// What a frame's readout needs from its submission.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Plan {
    /// The bitstream budget the encode was given.
    pub(super) budget: usize,
    pub(super) kbps: u32,
    pub(super) interval: Duration,
}
/// A frame on the GPU.
pub(super) struct Flight<L> {
    /// Pins the capture and its conversion inputs until Vulkan confirms the
    /// frame complete.
    pub(super) lease: L,
    /// The planes, fence and SDK frame context it uses.
    pub(super) slot: usize,
    /// The slot fence value its encode signals when done.
    pub(super) release: u64,
    /// The encode call's start, before the colour conversion.
    pub(super) begin: Instant,
    pub(super) submitted: Instant,
    pub(super) captured: Instant,
    pub(super) plan: Plan,
    pub(super) seq_offset: u8,
    pub(super) cache_repeat: Option<RepeatKey>,
}
/// A frame Vulkan confirmed complete, its lease returned.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Completed {
    pub(super) slot: usize,
    pub(super) begin: Instant,
    pub(super) captured: Instant,
    pub(super) plan: Plan,
    pub(super) seq_offset: u8,
    pub(super) cache_repeat: Option<RepeatKey>,
}
/// Frames on the GPU, oldest first. Only the oldest completes, so frames
/// leave in the order they were submitted.
pub(super) struct Flights<L> {
    pub(super) queue: VecDeque<Flight<L>>,
    /// A completion could not be confirmed: every lease and resource stays.
    pub(super) unconfirmed: bool,
}
impl<L> Default for Flights<L> {
    fn default() -> Self {
        Self {
            queue: VecDeque::with_capacity(MAX_DEPTH),
            unconfirmed: false,
        }
    }
}
impl<L> Flights<L> {
    pub(super) fn len(&self) -> usize {
        self.queue.len()
    }
    pub(super) fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
    pub(super) fn front(&self) -> Option<&Flight<L>> {
        self.queue.front()
    }
    /// The lowest of `depth` slots no frame in flight uses.
    pub(super) fn free_slot(&self, depth: usize) -> Option<usize> {
        (0..depth).find(|slot| self.queue.iter().all(|flight| flight.slot != *slot))
    }
    pub(super) fn push(&mut self, flight: Flight<L>) {
        self.queue.push_back(flight);
    }
    /// The oldest frame, once `done` confirms it complete; its lease goes
    /// back then. A frame `done` fails stays in flight, unconfirmed.
    pub(super) fn complete(
        &mut self,
        done: impl FnOnce(&Flight<L>) -> Result<bool>,
    ) -> Result<Option<Completed>> {
        let Some(oldest) = self.queue.front() else {
            return Ok(None);
        };
        match done(oldest) {
            Ok(true) => {}
            Ok(false) => return Ok(None),
            Err(error) => {
                self.unconfirmed = true;
                return Err(error);
            }
        }
        let flight = self.queue.pop_front().expect("the oldest frame");
        let completed = Completed {
            slot: flight.slot,
            begin: flight.begin,
            captured: flight.captured,
            plan: flight.plan,
            seq_offset: flight.seq_offset,
            cache_repeat: flight.cache_repeat,
        };
        drop(flight.lease);
        Ok(Some(completed))
    }
    /// Every frame in order, each confirmed by `done`, which waits for it.
    pub(super) fn settle_all(
        &mut self,
        mut done: impl FnMut(&Flight<L>) -> Result<bool>,
    ) -> Result<()> {
        while !self.is_empty() {
            if self.complete(&mut done)?.is_none() {
                self.unconfirmed = true;
                bail!("PyroWave GPU completion was not confirmed");
            }
        }
        Ok(())
    }
    /// The device that converted the frames is lost: its queued work is dead
    /// and the leases are free. Vulkan reads only the slots' own planes.
    pub(super) fn abandon(&mut self) {
        self.queue.clear();
    }
    /// Keeps every lease alive until the process exits.
    pub(super) fn retain(&mut self) {
        for flight in self.queue.drain(..) {
            std::mem::forget(flight);
        }
    }
}
