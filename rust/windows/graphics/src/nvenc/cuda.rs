//! CUDA driver interop for NVENC's 10-bit planar 4:4:4 input.
//! Only the installed driver is loaded; no CUDA toolkit or CPU readback is used.

use crate::capture::Device;
use anyhow::{Context as _, Result, bail};
use butterpollo_win_abi::cuda::*;
use std::{cell::Cell, ffi::c_void, ptr, rc::Rc, sync::Arc};
use windows::{
    Win32::Graphics::{Direct3D11::ID3D11Texture2D, Dxgi::IDXGIDevice},
    core::Interface,
};

type Init = unsafe extern "C" fn(u32) -> CUresult;
type DeviceFn = unsafe extern "C" fn(*mut CUdevice, *mut c_void) -> CUresult;
type Create = unsafe extern "C" fn(*mut CUcontext, u32, CUdevice) -> CUresult;
type ContextFn = unsafe extern "C" fn(CUcontext) -> CUresult;
type Pop = unsafe extern "C" fn(*mut CUcontext) -> CUresult;
type CreateStream = unsafe extern "C" fn(*mut CUstream, u32) -> CUresult;
type DestroyStream = unsafe extern "C" fn(CUstream) -> CUresult;
type Register = unsafe extern "C" fn(*mut CUgraphicsResource, *mut c_void, u32) -> CUresult;
type ResourceFn = unsafe extern "C" fn(CUgraphicsResource) -> CUresult;
type Map = unsafe extern "C" fn(u32, *mut CUgraphicsResource, CUstream) -> CUresult;
type Array = unsafe extern "C" fn(*mut CUarray, CUgraphicsResource, u32, u32) -> CUresult;
type Allocate = unsafe extern "C" fn(*mut CUdeviceptr, *mut usize, usize, usize, u32) -> CUresult;
type Free = unsafe extern "C" fn(CUdeviceptr) -> CUresult;
type Copy2d = unsafe extern "C" fn(*const CUDA_MEMCPY2D) -> CUresult;
type Copy2dAsync = unsafe extern "C" fn(*const CUDA_MEMCPY2D, CUstream) -> CUresult;

struct Api {
    _library: Option<libloading::Library>,
    push: ContextFn,
    pop: Pop,
    destroy: ContextFn,
    create_stream: Option<CreateStream>,
    destroy_stream: Option<DestroyStream>,
    register: Register,
    unregister: ResourceFn,
    map: Map,
    unmap: Map,
    array: Array,
    allocate: Allocate,
    free: Free,
    copy: Copy2d,
    copy_async: Option<Copy2dAsync>,
}
fn check(status: CUresult, operation: &str) -> Result<()> {
    if status != 0 {
        bail!("CUDA {operation} failed ({status})");
    }
    Ok(())
}
pub(crate) struct Context {
    api: Arc<Api>,
    raw: CUcontext,
    /// Non-blocking CUDA stream created alongside `raw` when `cuStreamCreate`,
    /// `cuStreamDestroy_v2` and `cuMemcpy2DAsync_v2` are available. Kept inside
    /// `Rc<Context>` so `&self.stream` keeps a stable `NV_ENC_CUSTREAM_PTR`
    /// (`CUstream*`, nvEncodeAPI.h:116) for the session's lifetime.
    stream: CUstream,
    /// True once NVENC accepted `stream` via `nvEncSetIOCudaStreams`; while
    /// false, interop stays on the default stream (`NULL`) so NVENC's default
    /// stream reads never race with a non-blocking stream.
    stream_bound: Cell<bool>,
}
pub(crate) struct Guard {
    context: Rc<Context>,
}
impl Drop for Guard {
    fn drop(&mut self) {
        let mut popped = ptr::null_mut();
        if let Err(error) = check(
            // SAFETY: A Guard exists only after a successful push of this context, and `popped` is
            // a live local.
            unsafe { (self.context.api.pop)(&mut popped) },
            "pop context",
        ) {
            tracing::error!(%error, "CUDA context restoration failed");
        }
    }
}
impl Context {
    pub fn new(device: &Device) -> Result<Rc<Self>> {
        // SAFETY: `library` is moved into `Api`, so every copied symbol stays loaded; each is
        // declared with the CUDA driver API's C signature, and all out-pointers are live locals.
        unsafe {
            let library = crate::nvenc::system_library("nvcuda.dll")?;
            let init: Init = *library.get(b"cuInit\0")?;
            check(init(0), "initialize driver")?;
            let get_device: DeviceFn = *library.get(b"cuD3D11GetDevice\0")?;
            let adapter = device.device.cast::<IDXGIDevice>()?.GetAdapter()?;
            let mut cu_device = 0;
            check(
                get_device(&mut cu_device, adapter.as_raw()),
                "resolve D3D11 adapter",
            )?;
            let create: Create = *library.get(b"cuCtxCreate_v2\0")?;
            let create_stream = library
                .get::<CreateStream>(b"cuStreamCreate\0")
                .ok()
                .map(|s| *s);
            let destroy_stream = library
                .get::<DestroyStream>(b"cuStreamDestroy_v2\0")
                .ok()
                .map(|s| *s);
            let copy_async = library
                .get::<Copy2dAsync>(b"cuMemcpy2DAsync_v2\0")
                .ok()
                .map(|s| *s);
            let api = Arc::new(Api {
                push: *library.get(b"cuCtxPushCurrent_v2\0")?,
                pop: *library.get(b"cuCtxPopCurrent_v2\0")?,
                destroy: *library.get(b"cuCtxDestroy_v2\0")?,
                create_stream,
                destroy_stream,
                register: *library.get(b"cuGraphicsD3D11RegisterResource\0")?,
                unregister: *library.get(b"cuGraphicsUnregisterResource\0")?,
                map: *library.get(b"cuGraphicsMapResources\0")?,
                unmap: *library.get(b"cuGraphicsUnmapResources\0")?,
                array: *library.get(b"cuGraphicsSubResourceGetMappedArray\0")?,
                allocate: *library.get(b"cuMemAllocPitch_v2\0")?,
                free: *library.get(b"cuMemFree_v2\0")?,
                copy: *library.get(b"cuMemcpy2D_v2\0")?,
                copy_async,
                _library: Some(library),
            });
            let mut raw = ptr::null_mut();
            let status = create(&mut raw, CU_CTX_SCHED_BLOCKING_SYNC, cu_device);
            if status != 0 {
                if !raw.is_null() {
                    let _ = (api.destroy)(raw);
                }
                check(status, "create context")?;
            }
            if raw.is_null() {
                bail!("CUDA returned no context");
            }
            // cuCtxCreate pushes its context; create the non-blocking stream while it is current.
            let mut stream = ptr::null_mut();
            if let (Some(create_stream), Some(_), Some(_)) =
                (api.create_stream, api.destroy_stream, api.copy_async)
            {
                let status = create_stream(&mut stream, CU_STREAM_NON_BLOCKING);
                if status != 0 {
                    tracing::debug!(
                        status,
                        "CUDA non-blocking stream unavailable; falling back to default stream"
                    );
                    stream = ptr::null_mut();
                }
            }
            let context = Rc::new(Self {
                api,
                raw,
                stream,
                stream_bound: Cell::new(false),
            });
            let mut popped = ptr::null_mut();
            check(
                (context.api.pop)(&mut popped),
                "restore context after creation",
            )?;
            if popped != raw {
                bail!("CUDA context stack changed during creation");
            }
            Ok(context)
        }
    }
    pub fn raw(&self) -> *mut c_void {
        self.raw.cast()
    }
    /// `NV_ENC_CUSTREAM_PTR` (`CUstream*`, nvEncodeAPI.h:116) pointing at this
    /// context's non-blocking stream, or `None` when falling back to the
    /// default stream.
    pub fn stream_ptr(&self) -> Option<*mut c_void> {
        (!self.stream.is_null()).then_some((&self.stream as *const CUstream) as *mut c_void)
    }
    /// Record that NVENC accepted [`Self::stream_ptr`] via `nvEncSetIOCudaStreams`.
    pub fn mark_stream_bound(&self) {
        self.stream_bound.set(true);
    }
    fn io_stream(&self) -> CUstream {
        if self.stream_bound.get() {
            self.stream
        } else {
            ptr::null_mut()
        }
    }
    pub fn enter(self: &Rc<Self>) -> Result<Guard> {
        // SAFETY: `push` comes from the loaded driver and `self.raw` is the live context destroyed
        // only in Drop.
        check(unsafe { (self.api.push)(self.raw) }, "push context")?;
        Ok(Guard {
            context: self.clone(),
        })
    }
}
impl Drop for Context {
    fn drop(&mut self) {
        if !self.stream.is_null()
            && let Some(destroy_stream) = self.api.destroy_stream
        {
            // SAFETY: `self.raw` is still alive; every Guard, Input and NVENC session has already
            // dropped (or been forgotten on failed destroy). Push `self.raw` so `cuStreamDestroy_v2`
            // runs in the owning context, then restore the caller's context.
            unsafe {
                if (self.api.push)(self.raw) == 0 {
                    let _ = check(destroy_stream(self.stream), "destroy stream");
                    let mut popped = ptr::null_mut();
                    let _ = check(
                        (self.api.pop)(&mut popped),
                        "pop context after stream destroy",
                    );
                }
            }
        }
        // SAFETY: `self.raw` was created in `new`; every Guard and Input holds an Rc, so none can
        // still use it.
        let _ = check(unsafe { (self.api.destroy)(self.raw) }, "destroy context");
    }
}

pub(crate) struct Input {
    context: Rc<Context>,
    _texture: Option<ID3D11Texture2D>,
    resource: CUgraphicsResource,
    pub pointer: CUdeviceptr,
    pub pitch: u32,
    width: u32,
    rows: u32,
    mapped: bool,
}
impl Input {
    pub fn new(
        context: &Rc<Context>,
        texture: &ID3D11Texture2D,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        Self::new_raw(
            context,
            Some(texture.clone()),
            texture.as_raw(),
            width,
            height,
        )
    }
    fn new_raw(
        context: &Rc<Context>,
        texture: Option<ID3D11Texture2D>,
        raw: *mut c_void,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let _guard = context.enter()?;
        let mut input = Self {
            context: context.clone(),
            _texture: texture,
            resource: ptr::null_mut(),
            pointer: 0,
            pitch: 0,
            width,
            rows: height.checked_mul(3).context("4:4:4 height overflow")?,
            mapped: false,
        };
        // SAFETY: The context is current (guard above), `raw` is kept alive in `_texture`, and
        // Drop releases whatever this block acquired before an error.
        unsafe {
            check(
                (context.api.register)(
                    &mut input.resource,
                    raw,
                    CUgraphicsRegisterFlags_enum_CU_GRAPHICS_REGISTER_FLAGS_READ_ONLY,
                ),
                "register D3D11 texture",
            )?;
            if input.resource.is_null() {
                bail!("CUDA returned no registered texture");
            }
            let mut pitch = 0;
            check(
                (context.api.allocate)(
                    &mut input.pointer,
                    &mut pitch,
                    width as usize * 2,
                    input.rows as usize,
                    16,
                ),
                "allocate pitched 4:4:4 input",
            )?;
            input.pitch = u32::try_from(pitch).context("CUDA pitch exceeds the NVENC ABI")?;
            if input.pointer == 0 || pitch < width as usize * 2 {
                bail!("invalid CUDA pitched allocation");
            }
        }
        Ok(input)
    }
    /// Copy the registered D3D11 stacked-plane texture into the pitched CUDA
    /// device buffer while `guard` already holds this context current.
    pub fn copy(&mut self, guard: &Guard) -> Result<()> {
        debug_assert!(Rc::ptr_eq(&self.context, &guard.context));
        let stream = self.context.io_stream();
        // SAFETY: `guard` keeps `self.context` current; `resource` is the registered texture and
        // the copy writes `width * 2` bytes by `rows` rows within the `pitch` allocation made in
        // `new`, ordered on `stream` (which matches NVENC's IO stream).
        unsafe {
            check(
                (self.context.api.map)(1, &mut self.resource, stream),
                "map D3D11 input",
            )?;
            self.mapped = true;
            let result = (|| {
                let mut array = ptr::null_mut();
                check(
                    (self.context.api.array)(&mut array, self.resource, 0, 0),
                    "resolve mapped texture",
                )?;
                if array.is_null() {
                    bail!("CUDA returned no mapped array");
                }
                let descriptor = CUDA_MEMCPY2D {
                    srcMemoryType: CUmemorytype_enum_CU_MEMORYTYPE_ARRAY,
                    srcArray: array,
                    dstMemoryType: CUmemorytype_enum_CU_MEMORYTYPE_DEVICE,
                    dstDevice: self.pointer,
                    dstPitch: self.pitch as usize,
                    WidthInBytes: self.width as usize * 2,
                    Height: self.rows as usize,
                    ..Default::default()
                };
                if !stream.is_null()
                    && let Some(copy_async) = self.context.api.copy_async
                {
                    check(
                        copy_async(&descriptor, stream),
                        "copy GPU planar input async",
                    )
                } else {
                    check(
                        (self.context.api.copy)(&descriptor),
                        "copy GPU planar input",
                    )
                }
            })();
            let unmapped = check(
                (self.context.api.unmap)(1, &mut self.resource, stream),
                "unmap D3D11 input",
            );
            if unmapped.is_ok() {
                self.mapped = false;
            }
            result.and(unmapped)
        }
    }
}
impl Drop for Input {
    fn drop(&mut self) {
        let Ok(_guard) = self.context.enter() else {
            return;
        };
        let stream = self.context.io_stream();
        // SAFETY: The context is current; each resource is released only if acquired, before the
        // texture drops.
        unsafe {
            if self.mapped {
                let _ = (self.context.api.unmap)(1, &mut self.resource, stream);
            }
            if !self.resource.is_null() {
                let _ = (self.context.api.unregister)(self.resource);
            }
            if self.pointer != 0 {
                let _ = (self.context.api.free)(self.pointer);
            }
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    pub(crate) struct FakeCuda {
        pub(crate) pushes: usize,
        pub(crate) pops: usize,
        pub(crate) register_flags: Vec<u32>,
        pub(crate) stream_flags: Vec<u32>,
        pub(crate) map_streams: Vec<usize>,
        pub(crate) unmap_streams: Vec<usize>,
        pub(crate) async_copy_streams: Vec<usize>,
        pub(crate) sync_copies: usize,
        pub(crate) events: Vec<&'static str>,
    }

    thread_local! {
        static FAKE: RefCell<FakeCuda> = RefCell::default();
    }
    const FAKE_CTX: usize = 0x1001;
    pub(crate) const FAKE_STREAM: usize = 0x2002;
    const FAKE_RES: usize = 0x3003;
    const FAKE_ARR: usize = 0x4004;

    pub(crate) fn reset() {
        FAKE.with(|f| *f.borrow_mut() = FakeCuda::default());
    }
    pub(crate) fn with_fake<T>(f: impl FnOnce(&mut FakeCuda) -> T) -> T {
        FAKE.with(|c| f(&mut c.borrow_mut()))
    }
    pub(crate) fn fake_input(ctx: &Rc<Context>, key: usize) -> Result<Input> {
        Input::new_raw(ctx, None, key as *mut c_void, 1920, 1080)
    }
    unsafe extern "C" fn push(_: CUcontext) -> CUresult {
        with_fake(|f| f.pushes += 1);
        0
    }
    unsafe extern "C" fn pop(out: *mut CUcontext) -> CUresult {
        // SAFETY: Caller passes a live local pointer.
        unsafe { *out = FAKE_CTX as CUcontext };
        with_fake(|f| f.pops += 1);
        0
    }
    unsafe extern "C" fn destroy(_: CUcontext) -> CUresult {
        with_fake(|f| f.events.push("destroy_ctx"));
        0
    }
    unsafe extern "C" fn create_stream(out: *mut CUstream, flags: u32) -> CUresult {
        // SAFETY: Caller passes a live local pointer.
        unsafe { *out = FAKE_STREAM as CUstream };
        with_fake(|f| f.stream_flags.push(flags));
        0
    }
    unsafe extern "C" fn destroy_stream(stream: CUstream) -> CUresult {
        assert_eq!(stream as usize, FAKE_STREAM);
        with_fake(|f| f.events.push("destroy_stream"));
        0
    }
    unsafe extern "C" fn register(
        out: *mut CUgraphicsResource,
        _: *mut c_void,
        flags: u32,
    ) -> CUresult {
        // SAFETY: Caller passes a live local pointer.
        unsafe { *out = FAKE_RES as CUgraphicsResource };
        with_fake(|f| f.register_flags.push(flags));
        0
    }
    unsafe extern "C" fn unregister(_: CUgraphicsResource) -> CUresult {
        with_fake(|f| f.events.push("unregister"));
        0
    }
    unsafe extern "C" fn map(_: u32, _: *mut CUgraphicsResource, stream: CUstream) -> CUresult {
        with_fake(|f| f.map_streams.push(stream as usize));
        0
    }
    unsafe extern "C" fn unmap(_: u32, _: *mut CUgraphicsResource, stream: CUstream) -> CUresult {
        with_fake(|f| f.unmap_streams.push(stream as usize));
        0
    }
    unsafe extern "C" fn array(
        out: *mut CUarray,
        _: CUgraphicsResource,
        _: u32,
        _: u32,
    ) -> CUresult {
        // SAFETY: Caller passes a live local pointer.
        unsafe { *out = FAKE_ARR as CUarray };
        0
    }
    unsafe extern "C" fn allocate(
        ptr_out: *mut CUdeviceptr,
        pitch: *mut usize,
        width_bytes: usize,
        _: usize,
        _: u32,
    ) -> CUresult {
        // SAFETY: Caller passes live local pointers.
        unsafe {
            *ptr_out = 0x5005;
            *pitch = width_bytes.next_multiple_of(512);
        }
        0
    }
    unsafe extern "C" fn free(_: CUdeviceptr) -> CUresult {
        with_fake(|f| f.events.push("free"));
        0
    }
    unsafe extern "C" fn copy_sync(_: *const CUDA_MEMCPY2D) -> CUresult {
        with_fake(|f| f.sync_copies += 1);
        0
    }
    unsafe extern "C" fn copy_async(_: *const CUDA_MEMCPY2D, stream: CUstream) -> CUresult {
        with_fake(|f| f.async_copy_streams.push(stream as usize));
        0
    }

    pub(crate) fn fake_context(with_async_stream: bool) -> Rc<Context> {
        let mut stream = ptr::null_mut();
        if with_async_stream {
            // SAFETY: `create_stream` is our fake above and `stream` is a live local.
            unsafe {
                let _ = create_stream(&mut stream, CU_STREAM_NON_BLOCKING);
            }
        }
        let api = Arc::new(Api {
            _library: None,
            push,
            pop,
            destroy,
            create_stream: with_async_stream.then_some(create_stream as CreateStream),
            destroy_stream: with_async_stream.then_some(destroy_stream as DestroyStream),
            register,
            unregister,
            map,
            unmap,
            array,
            allocate,
            free,
            copy: copy_sync,
            copy_async: with_async_stream.then_some(copy_async as Copy2dAsync),
        });
        Rc::new(Context {
            api,
            raw: FAKE_CTX as CUcontext,
            stream,
            stream_bound: Cell::new(false),
        })
    }

    #[test]
    fn cuda_registers_read_only_uses_bound_async_stream_and_avoids_nested_push() -> Result<()> {
        reset();
        let ctx = fake_context(true);
        // `stream_ptr` must be a pointer to `CUstream` (`NV_ENC_CUSTREAM_PTR`, nvEncodeAPI.h:116).
        let ptr = ctx.stream_ptr().expect("non-blocking stream");
        // SAFETY: `ptr` points at `ctx.stream` while `ctx` lives.
        assert_eq!(unsafe { *ptr.cast::<CUstream>() } as usize, FAKE_STREAM);

        let mut input = Input::new_raw(&ctx, None, 0x9999 as *mut c_void, 1920, 1080)?;
        with_fake(|f| {
            assert_eq!(
                f.register_flags,
                [CUgraphicsRegisterFlags_enum_CU_GRAPHICS_REGISTER_FLAGS_READ_ONLY]
            );
            assert_eq!(f.stream_flags, [CU_STREAM_NON_BLOCKING]);
        });

        // While stream is unbound, interop stays on NULL default stream and sync copy.
        {
            let guard = ctx.enter()?;
            let pushes_before = with_fake(|f| f.pushes);
            input.copy(&guard)?;
            // Zero nested cuCtxPushCurrent_v2 calls inside Input::copy!
            assert_eq!(with_fake(|f| f.pushes), pushes_before);
        }
        with_fake(|f| {
            assert_eq!(f.map_streams, [0]);
            assert_eq!(f.unmap_streams, [0]);
            assert_eq!(f.sync_copies, 1);
            assert!(f.async_copy_streams.is_empty());
        });

        // Once bound via nvEncSetIOCudaStreams, map/copy/unmap run on the non-blocking stream.
        ctx.mark_stream_bound();
        {
            let guard = ctx.enter()?;
            let pushes_before = with_fake(|f| f.pushes);
            input.copy(&guard)?;
            assert_eq!(with_fake(|f| f.pushes), pushes_before);
        }
        with_fake(|f| {
            assert_eq!(f.map_streams, [0, FAKE_STREAM]);
            assert_eq!(f.unmap_streams, [0, FAKE_STREAM]);
            assert_eq!(f.sync_copies, 1);
            assert_eq!(f.async_copy_streams, [FAKE_STREAM]);
        });

        drop(input);
        drop(ctx);
        with_fake(|f| {
            let destroy_stream = f
                .events
                .iter()
                .position(|e| *e == "destroy_stream")
                .unwrap();
            let destroy_ctx = f.events.iter().position(|e| *e == "destroy_ctx").unwrap();
            assert!(
                destroy_stream < destroy_ctx,
                "stream destroyed before context"
            );
            assert_eq!(f.pushes, f.pops, "every push matched by pop");
        });
        Ok(())
    }
}
