//! D3D12 compute device, command queue, fence synchronization and shared texture cache.

use super::*;

/// Whether captures and the AMF and NVENC encoders use the compute queue
/// (`gpu_compute_conversion`, on unless turned off).
pub fn enabled(config: &butterpollo_config::config::Config) -> bool {
    config.boolean("gpu_compute_conversion", true)
}
/// Whether the compute queue can open `texture` (an NT-handle shared one).
pub fn shareable(texture: &ID3D11Texture2D) -> bool {
    let mut desc = windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC::default();
    // SAFETY: `texture` is a live texture and `desc` is a live local that GetDesc fills.
    unsafe { texture.GetDesc(&mut desc) };
    desc.MiscFlags
        & windows::Win32::Graphics::Direct3D11::D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0 as u32
        != 0
}

/// A D3D12 device on the capture GPU with one compute queue and its fence.
pub struct Compute {
    pub device: ID3D12Device,
    pub queue: ID3D12CommandQueue,
    pub fence: ID3D12Fence,
    pub(super) next: Mutex<u64>,
    /// D3D11 textures already opened here, by their COM pointer.
    pub(super) opened: Mutex<HashMap<usize, (ID3D11Texture2D, ID3D12Resource)>>,
    /// Textures another process created and shared, opened from its handle
    /// (CreateSharedHandle cannot share them again); kept until forgotten.
    pub(super) foreign: Mutex<HashMap<usize, (ID3D11Texture2D, ID3D12Resource)>>,
    /// Fast path for [`Self::open`]: skip locking `foreign` while empty.
    pub(super) has_foreign: std::sync::atomic::AtomicBool,
    /// Incremented whenever an entry is removed from `opened` or `foreign`,
    /// invalidating cached per-slot source SRV keys against COM address reuse.
    pub(super) cache_epoch: AtomicU64,
    pub priority: i32,
    /// Set when the queue cannot safely be reused after an ordering error or timeout.
    pub(super) stuck: std::sync::atomic::AtomicBool,
}

/// How long a compute queue made after a GPU reset waits for a fresh D3D12 device. D3D12 hands
/// out one device per adapter while anything holds it, so until the reset device's last
/// references are gone (an encoder or capture being released, retained work) it returns the
/// removed one. On the RX 7900 XT a fresh one came about 400 ms after the reset.
pub(super) const FRESH_DEVICE_WAIT: std::time::Duration = std::time::Duration::from_millis(1500);
pub(super) const FRESH_DEVICE_RETRY: std::time::Duration = std::time::Duration::from_millis(50);

/// Runs `create` again while it fails with a lost device, until `deadline`: what
/// [`Compute::new`] does after a reset. The result, and how many times it was asked again.
pub(super) fn retry_lost_device<T>(
    deadline: std::time::Instant,
    retry: std::time::Duration,
    mut create: impl FnMut() -> Result<T>,
) -> (Result<T>, u32) {
    let mut again = 0;
    loop {
        match create() {
            Err(error)
                if crate::device_loss::DeviceLost::from_error(&error).is_some()
                    && std::time::Instant::now() + retry < deadline =>
            {
                again += 1;
                std::thread::sleep(retry);
            }
            result => return (result, again),
        }
    }
}
// D3D12 devices, queues and fences are free-threaded; the rest is locked.
// SAFETY: The D3D12 device, queue and fence are free-threaded, the cached D3D11 textures and D3D12
// resources are only touched under `opened`'s and `foreign`'s locks, and `next` is a mutex.
unsafe impl Send for Compute {}
// SAFETY: `&self` methods call only free-threaded D3D12 methods and change state only through the
// mutexes and the atomic.
unsafe impl Sync for Compute {}
/// Submit a slot's recorded list with `execute` once `ordered` says the queue
/// waits for its inputs, and keep the slot's `fence` for its list. When the
/// ordering failed nothing reaches the queue, and the slot keeps the finished
/// fence of its last conversion: `u64::MAX` there would make its next use
/// wait two seconds for a value nothing signals and mark the queue stuck. A
/// failed submission may have reached the queue, so the slot stays in flight.
/// `mark` counts what else the list reads as in flight (the couch menu's
/// texture and upload), by the same rule.
pub(super) fn submit_slot(
    fence: &mut u64,
    ordered: Result<()>,
    mark: impl FnOnce(),
    execute: impl FnOnce() -> Result<u64>,
) -> Result<u64> {
    ordered?;
    mark();
    *fence = u64::MAX;
    *fence = execute()?;
    Ok(*fence)
}
pub(super) fn completed_value(value: u64) -> Result<u64> {
    // D3D12 signals UINT64_MAX on device removal, including shared fences.
    if value == u64::MAX {
        return Err(crate::device_loss::DeviceLost(DXGI_ERROR_DEVICE_REMOVED.0).into());
    }
    Ok(value)
}
/// AMF owns these states while encoding, but the next transition runs on a
/// compute command list. Never pass a video/graphics-only state to that queue.
/// See DirectX-Specs/CPUEfficiency, "State support by command-list type".
pub(super) fn ensure_compute_texture_state(state: D3D12_RESOURCE_STATES) -> Result<()> {
    let read = D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE | D3D12_RESOURCE_STATE_COPY_SOURCE;
    let valid = state.0 & !read.0 == 0
        || state == D3D12_RESOURCE_STATE_UNORDERED_ACCESS
        || state == D3D12_RESOURCE_STATE_COPY_DEST;
    anyhow::ensure!(
        valid,
        "AMF input resource state {:#x} cannot transition on the compute queue",
        state.0 as u32
    );
    Ok(())
}
pub(super) fn ensure_queue_active(retired: &std::sync::atomic::AtomicBool) -> Result<()> {
    anyhow::ensure!(
        !retired.load(Ordering::Acquire),
        "shared compute queue is retired; reopen capture"
    );
    Ok(())
}
/// A compute queue at high priority, or the normal one when Windows refuses
/// it. Never real-time: a real-time queue pre-empts the desktop compositor
/// that every captured frame comes from, and an HDR stream on an RX 7900 XT
/// with it ended in an AMD engine timeout the driver could not reset. High
/// priority still runs ahead of a game's normal work.
pub(super) fn compute_queue(device: &ID3D12Device) -> Result<(ID3D12CommandQueue, i32)> {
    for priority in [
        D3D12_COMMAND_QUEUE_PRIORITY_HIGH.0,
        D3D12_COMMAND_QUEUE_PRIORITY_NORMAL.0,
    ] {
        // SAFETY: `device` is a live D3D12 device and the queue description outlives the call.
        if let Ok(queue) = unsafe {
            device.CreateCommandQueue::<ID3D12CommandQueue>(&D3D12_COMMAND_QUEUE_DESC {
                Type: D3D12_COMMAND_LIST_TYPE_COMPUTE,
                Priority: priority,
                Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
                NodeMask: 0,
            })
        } {
            return Ok((queue, priority));
        }
    }
    bail!("no D3D12 compute queue")
}
impl Compute {
    /// A compute queue on the adapter with `luid`, on a D3D12 device that works: after a GPU
    /// reset it waits up to [`FRESH_DEVICE_WAIT`] for D3D12 to stop handing out the removed
    /// device, so compute conversion returns with the first encoder made after the reset
    /// instead of that encoder (and the capture) converting on the graphics queue.
    pub fn new(luid: LUID) -> Result<Arc<Self>> {
        let started = std::time::Instant::now();
        let (result, again) =
            retry_lost_device(started + FRESH_DEVICE_WAIT, FRESH_DEVICE_RETRY, || {
                Self::create(luid)
            });
        if again > 0 {
            match &result {
                Ok(_) => tracing::info!(
                    waited_ms = started.elapsed().as_millis() as u64,
                    "compute queue on a fresh D3D12 device after the GPU reset"
                ),
                Err(error) => tracing::warn!(
                    error = %format!("{error:#}"),
                    waited_ms = started.elapsed().as_millis() as u64,
                    "D3D12 still hands out the reset device; no compute queue"
                ),
            }
        }
        result
    }
    pub(super) fn create(luid: LUID) -> Result<Arc<Self>> {
        allow_compute_generation(luid)?;
        // SAFETY: `adapter` comes from a live DXGI factory, the device out-parameter is a live
        // local checked for None, and the queue and fence are created on that device.
        unsafe {
            let factory: IDXGIFactory4 = CreateDXGIFactory1()?;
            let adapter: IDXGIAdapter1 = factory.EnumAdapterByLuid(luid)?;
            let mut device: Option<ID3D12Device> = None;
            D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_11_0, &mut device)
                .context("D3D12 device for compute conversion")?;
            let device = device.context("no D3D12 device")?;
            // The reset device, still held elsewhere: asked again until it is let go.
            if let Some(loss) = crate::device_loss::DeviceLost::d3d12(&device) {
                return Err(anyhow::Error::new(loss)
                    .context("D3D12 returned the GPU's removed device for compute conversion"));
            }
            let (queue, priority) = compute_queue(&device)?;
            let fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE)?;
            tracing::info!(priority, "compute queue ready for colour conversion");
            Ok(Arc::new(Self {
                device,
                queue,
                fence,
                next: Mutex::new(0),
                opened: Mutex::new(HashMap::new()),
                foreign: Mutex::new(HashMap::new()),
                has_foreign: std::sync::atomic::AtomicBool::new(false),
                cache_epoch: AtomicU64::new(0),
                priority,
                stuck: std::sync::atomic::AtomicBool::new(false),
            }))
        }
    }
    /// The compute queue for the GPU a D3D11 device runs on, shared by the
    /// capture and the encoders on that GPU.
    pub fn for_device(
        device: &windows::Win32::Graphics::Direct3D11::ID3D11Device,
    ) -> Result<Arc<Self>> {
        type Shared = Vec<((u32, i32), std::sync::Weak<Compute>)>;
        static SHARED: Mutex<Shared> = Mutex::new(Vec::new());
        // SAFETY: `device` is a live D3D11 device, and GetAdapter and GetDesc only return owned
        // values.
        let luid = unsafe {
            let dxgi: IDXGIDevice = device.cast()?;
            dxgi.GetAdapter()?.GetDesc()?.AdapterLuid
        };
        let key = (luid.LowPart, luid.HighPart);
        // Drop any rejected `Compute` outside `SHARED` so `Compute::drop` never
        // drains its queue while holding the adapter cache lock.
        let mut rejected = Vec::new();
        let result = (|| {
            let mut shared = SHARED.lock().unwrap();
            shared.retain(|(_, compute)| compute.strong_count() > 0);
            for (owner, weak) in shared.iter() {
                if *owner != key {
                    continue;
                }
                let Some(compute) = weak.upgrade() else {
                    continue;
                };
                if !compute.stuck.load(std::sync::atomic::Ordering::Acquire) && compute.completed(0)
                {
                    return Ok(compute);
                }
                rejected.push(compute);
            }
            let compute = Self::new(luid)?;
            shared.push((key, Arc::downgrade(&compute)));
            Ok(compute)
        })();
        drop(rejected);
        result
    }
    /// Signal the fence after the work submitted so far; returns its value.
    #[cfg(test)]
    pub(super) fn signal(&self) -> Result<u64> {
        let mut next = self.next.lock().unwrap();
        // Advance only once the signal is queued: a value nothing will ever
        // signal would hold every later wait on it.
        // SAFETY: `queue` and `fence` were created on the same D3D12 device and live as long as
        // `self`.
        unsafe { self.queue.Signal(&self.fence, *next + 1) }.inspect_err(|_| self.retire())?;
        *next += 1;
        Ok(*next)
    }
    /// Run a closed command list; returns the fence value that follows it.
    pub(crate) fn execute(&self, list: &ID3D12GraphicsCommandList) -> Result<u64> {
        let mut next = self.next.lock().unwrap();
        self.ensure_active()?;
        // SAFETY: Callers pass a closed compute command list recorded on `self.device`, the queue's
        // device, and keep what it references alive until the returned fence value.
        unsafe {
            // The base interface by COM inheritance: no QueryInterface per frame.
            self.queue
                .ExecuteCommandLists(&[Some(ID3D12CommandList::clone(list))]);
        }
        // Keep execution and its monotonically increasing completion signal
        // together even when several encoders share this queue.
        // SAFETY: The queue and fence belong to this live device.
        unsafe { self.queue.Signal(&self.fence, *next + 1) }.inspect_err(|_| self.retire())?;
        *next += 1;
        Ok(*next)
    }
    pub fn completed(&self, value: u64) -> bool {
        // SAFETY: `fence` is a live D3D12 fence owned by `self`; GetCompletedValue only reads it.
        match completed_value(unsafe { self.fence.GetCompletedValue() }) {
            Ok(done) => done >= value,
            Err(_) => {
                self.stuck.store(true, std::sync::atomic::Ordering::Release);
                false
            }
        }
    }
    /// A wait or enqueue failed even if the device still reports itself healthy. Captures
    /// must reopen before submitting more work behind a queue with uncertain ordering.
    pub(crate) fn is_stuck(&self) -> bool {
        self.stuck.load(Ordering::Acquire)
    }
    /// Reject new work after an ordering failure, even when old fence values
    /// have completed. Completion alone does not make a retired queue reusable.
    /// This does not cancel calls already in progress; their resource ownership
    /// and completion fences still apply, and retirement must never wait on them.
    pub(super) fn ensure_active(&self) -> Result<()> {
        ensure_queue_active(&self.stuck)
    }
    /// Do not give this queue to a new capture or encoder. Work already submitted may
    /// still wait on an external fence after an enqueue error prevented its completion.
    pub(crate) fn retire(&self) {
        self.stuck.store(true, Ordering::Release);
    }
    /// Wait on the CPU until the queue has passed `value`. A queue that does
    /// not get there is not used again: [`Compute::for_device`] makes a new one.
    pub fn wait(&self, value: u64) -> Result<()> {
        let result = wait_fence(&self.fence, value);
        if result.is_err() {
            self.stuck.store(true, std::sync::atomic::Ordering::Release);
        }
        result
    }
    /// A shared D3D11 texture as a D3D12 resource on this device.
    pub fn open(&self, texture: &ID3D11Texture2D) -> Result<ID3D12Resource> {
        self.ensure_active()?;
        let key = texture.as_raw() as usize;
        if self.has_foreign.load(Ordering::Acquire)
            && let Some((_, resource)) = self.foreign.lock().unwrap().get(&key)
        {
            return Ok(resource.clone());
        }
        let mut opened = self.opened.lock().unwrap();
        if let Some((_, resource)) = opened.get(&key) {
            return Ok(resource.clone());
        }
        // SAFETY: `texture` is a live D3D11 texture on this GPU, the NT handle is closed exactly
        // once after OpenSharedHandle has used it, and `resource` is a live local.
        unsafe {
            let shared: IDXGIResource1 = texture.cast()?;
            let handle = shared
                .CreateSharedHandle(
                    None,
                    DXGI_SHARED_RESOURCE_READ.0 | DXGI_SHARED_RESOURCE_WRITE.0,
                    None,
                )
                .context("sharing a captured texture with the compute queue")?;
            let mut resource: Option<ID3D12Resource> = None;
            let result = self.device.OpenSharedHandle(handle, &mut resource);
            let _ = CloseHandle(handle);
            result?;
            let resource = resource.context("no shared resource")?;
            // Captures recycle a few textures; start over if many came and
            // went. In-flight copies and conversions hold their own references,
            // so this cannot free memory the GPU still uses; the bound keeps
            // stale 4K HDR textures from pinning gigabytes of video memory.
            if opened.len() >= 24 {
                opened.clear();
                self.cache_epoch.fetch_add(1, Ordering::Release);
            }
            opened.insert(key, (texture.clone(), resource.clone()));
            Ok(resource)
        }
    }
}

impl Compute {
    /// Open a texture another process shared through `handle`, so that
    /// `open` finds it until `forget`.
    pub(crate) fn import(&self, texture: &ID3D11Texture2D, handle: HANDLE) -> Result<()> {
        self.ensure_active()?;
        let mut resource: Option<ID3D12Resource> = None;
        // SAFETY: `handle` is a live NT handle to `texture` that the caller owns, and `resource` is
        // a live local.
        unsafe {
            self.device.OpenSharedHandle(handle, &mut resource)?;
        }
        {
            let mut foreign = self.foreign.lock().unwrap();
            foreign.insert(
                texture.as_raw() as usize,
                (
                    texture.clone(),
                    resource.context("no imported shared resource")?,
                ),
            );
            self.has_foreign.store(true, Ordering::Release);
        }
        Ok(())
    }
    pub(crate) fn forget(&self, texture: &ID3D11Texture2D) {
        let _ = self.take_forgotten(texture);
    }
    /// Remove `texture` from this device's caches and return any cached D3D11/D3D12
    /// pairs so teardown can retain them until queued GPU work completes.
    pub(crate) fn take_forgotten(
        &self,
        texture: &ID3D11Texture2D,
    ) -> Vec<(ID3D11Texture2D, ID3D12Resource)> {
        let key = texture.as_raw() as usize;
        let mut removed = Vec::new();
        if let Some(pair) = self.opened.lock().unwrap().remove(&key) {
            removed.push(pair);
        }
        if self.has_foreign.load(Ordering::Acquire) {
            let mut foreign = self.foreign.lock().unwrap();
            if let Some(pair) = foreign.remove(&key) {
                removed.push(pair);
            }
            self.has_foreign
                .store(!foreign.is_empty(), Ordering::Release);
        }
        if !removed.is_empty() {
            self.cache_epoch.fetch_add(1, Ordering::Release);
        }
        removed
    }
    /// Drain every command, Wait and Signal queued on `self.queue` so no GPU work
    /// remains in flight before tearing down dependent objects on this device.
    pub(crate) fn wait_idle(&self) {
        if crate::device_loss::DeviceLost::d3d12(&self.device).is_some() {
            return;
        }
        RetainedWork::new(self, &self.queue).finish(self);
    }
}

impl Drop for Compute {
    fn drop(&mut self) {
        if crate::device_loss::DeviceLost::d3d12(&self.device).is_some() {
            return;
        }
        // Drain every queued Wait/Signal and keep cached shared resources alive
        // until the queue finishes, so destroying the last `Compute` never frees
        // the queue, device or imported textures behind in-flight GPU work.
        let mut work = RetainedWork::new(self, &self.queue);
        for (_, (d3d11, d3d12)) in self.opened.get_mut().unwrap().drain() {
            work.keep(&d3d11);
            work.keep(&d3d12);
        }
        for (_, (d3d11, d3d12)) in self.foreign.get_mut().unwrap().drain() {
            work.keep(&d3d11);
            work.keep(&d3d12);
        }
        work.finish(self);
    }
}
