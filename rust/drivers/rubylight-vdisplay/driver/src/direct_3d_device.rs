//! The render-side D3D11 device bound to IddCx swap chains, and the device the frame ring's
//! textures open on.
//!
//! From punktfunk's pf-vdisplay `direct_3d_device.rs` by Enrico Bühler (unom), MIT OR
//! Apache-2.0.
//!
//! One device per render LUID, pooled across every swap-chain assignment ([`pooled_device`]):
//! creating one per assignment leaked a vendor driver thread set and VRAM on every reconnect.
//! Because two monitors' workers share it, it is created without `SINGLETHREADED` and its
//! immediate context is multithread-protected.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::{
    Win32::{
        Foundation::{BOOL, E_FAIL, LUID},
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_UNKNOWN,
            Direct3D11::{
                D3D11_ASYNC_GETDATA_DONOTFLUSH, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                D3D11_CREATE_DEVICE_FLAG,
                D3D11_CREATE_DEVICE_PREVENT_ALTERING_LAYER_SETTINGS_FROM_REGISTRY,
                D3D11_QUERY_DESC, D3D11_QUERY_EVENT, D3D11_SDK_VERSION, D3D11CreateDevice,
                ID3D11Device, ID3D11DeviceContext, ID3D11Multithread, ID3D11Query,
            },
            Dxgi::{CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, IDXGIAdapter1, IDXGIFactory5},
        },
    },
    core::{Error, Interface},
};

/// Bounded wait for GPU completion before releasing shared frame-ring resources. Shorter than
/// `Monitor`'s ring-clear wait so a slow GPU drain finishes and drops resources before the
/// control-plane IOCTL times out.
const GPU_DRAIN_BUDGET: Duration = Duration::from_millis(1500);

pub struct Direct3DDevice {
    _dxgi_factory: IDXGIFactory5,
    _adapter: IDXGIAdapter1,
    pub device: ID3D11Device,
    pub device_context: ID3D11DeviceContext,
    /// The adapter this device was created on.
    pub luid: LUID,
    /// Minted per successful `init`, so a TDR recreate on the same LUID is a new epoch.
    epoch: u32,
    /// Set once any worker saw this device removed; the pool then recreates.
    removed: AtomicBool,
}

impl Direct3DDevice {
    pub fn epoch(&self) -> u32 {
        self.epoch
    }

    pub fn is_removed(&self) -> bool {
        self.removed.load(Ordering::Acquire)
    }

    pub fn mark_removed(&self) {
        if !self.removed.swap(true, Ordering::AcqRel) {
            dbglog!(
                "[rlvd] D3D device epoch {} removed; workers stop, the pool recreates",
                self.epoch
            );
        }
    }

    /// Flush the immediate context and wait (bounded) for a `D3D11_QUERY_EVENT` query to
    /// complete on the GPU so all prior commands referencing shared ring textures or fences
    /// have finished before those resources are dropped. `cached_query` is owned per worker
    /// thread so two monitors sharing one [`Direct3DDevice`] never collide on a query object.
    pub fn drain_gpu(&self, cached_query: &mut Option<ID3D11Query>) {
        if self.is_removed() {
            *cached_query = None;
            return;
        }
        // SAFETY: a plain status query on the live device.
        if unsafe { self.device.GetDeviceRemovedReason() }.is_err() {
            self.mark_removed();
            *cached_query = None;
            return;
        }
        if cached_query.is_none() {
            let desc = D3D11_QUERY_DESC {
                Query: D3D11_QUERY_EVENT,
                MiscFlags: 0,
            };
            let mut query = None;
            // SAFETY: `desc` and `query` are live locals; `self.device` is live.
            match unsafe { self.device.CreateQuery(&desc, Some(&mut query)) } {
                Ok(()) => *cached_query = query,
                Err(e) => {
                    dbglog!("[rlvd] D3D11 CreateQuery(EVENT) failed: {e:?}");
                    // SAFETY: a plain status query on the live device.
                    if unsafe { self.device.GetDeviceRemovedReason() }.is_err() {
                        self.mark_removed();
                    } else {
                        // SAFETY: plain flush of the multithread-protected immediate context.
                        unsafe { self.device_context.Flush() };
                    }
                    return;
                }
            }
        }
        let Some(query) = cached_query.as_ref() else {
            // SAFETY: plain flush of the multithread-protected immediate context.
            unsafe { self.device_context.Flush() };
            return;
        };
        // SAFETY: `query` belongs to `self.device`, and `self.device_context` is
        // multithread-protected; `End` + `Flush` submits all prior work and the event query.
        unsafe {
            self.device_context.End(query);
            self.device_context.Flush();
        }
        let started = Instant::now();
        let mut spins: u32 = 0;
        loop {
            let mut done = BOOL(0);
            // SAFETY: `query` is a live `D3D11_QUERY_EVENT` on this device; `GetData` writes
            // one `BOOL` into the live local `done` only when it returns `S_OK`.
            let res = unsafe {
                self.device_context.GetData(
                    query,
                    Some((&raw mut done).cast()),
                    size_of::<BOOL>() as u32,
                    D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32,
                )
            };
            match res {
                Ok(()) if done.as_bool() => return,
                Ok(()) => {}
                Err(e) => {
                    dbglog!("[rlvd] D3D11 GetData(EVENT) failed: {e:?}");
                    // SAFETY: a plain status query on the live device.
                    if unsafe { self.device.GetDeviceRemovedReason() }.is_err() {
                        self.mark_removed();
                        *cached_query = None;
                    }
                    return;
                }
            }
            if self.is_removed() {
                *cached_query = None;
                return;
            }
            if started.elapsed() >= GPU_DRAIN_BUDGET {
                dbglog!(
                    "[rlvd] D3D11 GPU drain timed out after {} ms (epoch {})",
                    started.elapsed().as_millis(),
                    self.epoch
                );
                return;
            }
            if spins < 8 {
                spins += 1;
                std::thread::yield_now();
            } else {
                // SAFETY: a plain status query on the live device.
                if unsafe { self.device.GetDeviceRemovedReason() }.is_err() {
                    self.mark_removed();
                    *cached_query = None;
                    return;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    fn init(adapter_luid: LUID) -> Result<Self, Error> {
        // SAFETY: a plain DXGI factory creation.
        let dxgi_factory =
            unsafe { CreateDXGIFactory2::<IDXGIFactory5>(DXGI_CREATE_FACTORY_FLAGS(0))? };
        // SAFETY: `dxgi_factory` is the live factory; the LUID is passed by value.
        let adapter = unsafe { dxgi_factory.EnumAdapterByLuid::<IDXGIAdapter1>(adapter_luid)? };
        let mut device = None;
        let mut device_context = None;
        // The device only copies the swap chain's surfaces into the host's textures.
        let flags = D3D11_CREATE_DEVICE_FLAG(
            D3D11_CREATE_DEVICE_BGRA_SUPPORT.0
                | D3D11_CREATE_DEVICE_PREVENT_ALTERING_LAYER_SETTINGS_FROM_REGISTRY.0,
        );
        // SAFETY: `adapter` is a live adapter; the out-params are live locals checked below;
        // flags and SDK version are valid constants.
        unsafe {
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                None,
                flags,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut device_context),
            )?;
        }
        let device = device.ok_or_else(|| Error::new(E_FAIL, "ID3D11Device not found"))?;
        let device_context =
            device_context.ok_or_else(|| Error::new(E_FAIL, "ID3D11DeviceContext not found"))?;
        match device_context.cast::<ID3D11Multithread>() {
            Ok(mt) => {
                // SAFETY: a plain setter on the live context's multithread interface.
                unsafe {
                    let _ = mt.SetMultithreadProtected(BOOL::from(true));
                }
            }
            Err(e) => dbglog!(
                "[rlvd] ID3D11Multithread unavailable ({e:?}); immediate context unprotected"
            ),
        }
        let epoch = DEVICE_EPOCH.fetch_add(1, Ordering::AcqRel) + 1;
        dbglog!("[rlvd] Direct3DDevice::init OK, epoch {epoch}");
        Ok(Self {
            _dxgi_factory: dxgi_factory,
            _adapter: adapter,
            device,
            device_context,
            luid: adapter_luid,
            epoch,
            removed: AtomicBool::new(false),
        })
    }
}

/// One device per render LUID, bounded so a hybrid PC's two adapters do not evict each other.
static DEVICE_POOL: Mutex<Vec<(i64, Arc<Direct3DDevice>)>> = Mutex::new(Vec::new());
const DEVICE_POOL_CAP: usize = 4;
static DEVICE_EPOCH: AtomicU32 = AtomicU32::new(0);

/// Sleep/stop ends the render-device epoch even when DXGI has not reported removal.
/// Mark devices before releasing the pool: a racing assignment cannot reuse one.
pub fn retire_pool() {
    let retired = {
        let mut pool = crate::registry::lock(&DEVICE_POOL);
        for (_, device) in pool.iter() {
            device.mark_removed();
        }
        core::mem::take(&mut *pool)
    };
    drop(retired);
}

/// A LUID as one comparable number.
pub fn luid_key(luid: LUID) -> i64 {
    (i64::from(luid.HighPart) << 32) | i64::from(luid.LowPart)
}

/// Get or create the pooled device for `luid`, recreating one that was removed.
pub fn pooled_device(luid: LUID) -> Option<Arc<Direct3DDevice>> {
    let key = luid_key(luid);
    let mut pool = crate::registry::lock(&DEVICE_POOL);
    if !crate::adapter::powered() {
        return None;
    }
    if let Some(pos) = pool.iter().position(|(k, _)| *k == key) {
        let dev = &pool[pos].1;
        // SAFETY: a plain status query on the live pooled device.
        let alive = !dev.is_removed() && unsafe { dev.device.GetDeviceRemovedReason() }.is_ok();
        if alive {
            return Some(dev.clone());
        }
        dev.mark_removed();
        pool.remove(pos);
    }
    match Direct3DDevice::init(luid) {
        Ok(d) => {
            let a = Arc::new(d);
            if pool.len() >= DEVICE_POOL_CAP {
                pool.remove(0);
            }
            pool.push((key, a.clone()));
            Some(a)
        }
        Err(e) => {
            dbglog!("[rlvd] pooled Direct3DDevice::init failed: {e:?}");
            None
        }
    }
}
