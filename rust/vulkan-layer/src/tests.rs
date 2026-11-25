//! Unit tests for the Vulkan virtual HDR layer negotiation, proc dispatch, and format injection.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[repr(C)]
struct FakeDispatchable {
    dispatch_table: usize,
}

static NEXT_DISPATCH: AtomicUsize = AtomicUsize::new(0x10_000);
static PHYSICAL_DISPATCH: AtomicUsize = AtomicUsize::new(0x20_000);

fn fn_ptr(f: *const ()) -> Function {
    // SAFETY: test helpers expose valid `unsafe extern "system" fn()` pointers.
    Some(unsafe { std::mem::transmute::<*const (), unsafe extern "system" fn()>(f) })
}

unsafe extern "system" fn fake_dummy_proc() {}
unsafe extern "system" fn fake_physical_feature_proc() {}
unsafe extern "system" fn fake_instance_fallback_proc() {}
unsafe extern "system" fn fake_device_fallback_proc() {}
unsafe extern "system" fn fake_destroy_instance(_handle: Handle, _alloc: *const c_void) {}
unsafe extern "system" fn fake_destroy_device(_handle: Handle, _alloc: *const c_void) {}
unsafe extern "system" fn fake_create_surface(
    _handle: Handle,
    _info: *const Win32Surface,
    _alloc: *const c_void,
    out: *mut u64,
) -> i32 {
    // SAFETY: test passes a valid pointer when non-null.
    unsafe {
        if !out.is_null() {
            *out = 0x5555;
        }
    }
    SUCCESS
}
unsafe extern "system" fn fake_destroy_surface(
    _handle: Handle,
    _surface: u64,
    _alloc: *const c_void,
) {
}
unsafe extern "system" fn fake_formats(
    _dev: Handle,
    _surface: u64,
    count: *mut u32,
    _out: *mut Format,
) -> i32 {
    // SAFETY: test passes a valid `count` pointer.
    unsafe {
        *count = 0;
    }
    SUCCESS
}
unsafe extern "system" fn fake_formats2(
    _dev: Handle,
    _info: *const SurfaceInfo,
    count: *mut u32,
    _out: *mut Format2,
) -> i32 {
    // SAFETY: test passes a valid `count` pointer.
    unsafe {
        *count = 0;
    }
    SUCCESS
}

unsafe extern "system" fn fake_create_instance(
    _info: *const CreateInfo,
    _alloc: *const c_void,
    out: *mut Handle,
) -> i32 {
    let inst = Box::new(FakeDispatchable {
        dispatch_table: NEXT_DISPATCH.fetch_add(16, Ordering::Relaxed),
    });
    // SAFETY: test passes a valid `out` handle pointer.
    unsafe {
        *out = Box::into_raw(inst).cast();
    }
    SUCCESS
}

unsafe extern "system" fn fake_create_device(
    _phys: Handle,
    _info: *const c_void,
    _alloc: *const c_void,
    out: *mut Handle,
) -> i32 {
    let dev = Box::new(FakeDispatchable {
        dispatch_table: NEXT_DISPATCH.fetch_add(16, Ordering::Relaxed),
    });
    // SAFETY: test passes a valid `out` handle pointer.
    unsafe {
        *out = Box::into_raw(dev).cast();
    }
    SUCCESS
}

unsafe extern "system" fn fake_next_gipa(_handle: Handle, name: *const c_char) -> Function {
    if name.is_null() {
        return None;
    }
    // SAFETY: callers pass NUL-terminated C literals.
    let bytes = unsafe { CStr::from_ptr(name).to_bytes() };
    match bytes {
        b"vkCreateInstance" => fn_ptr(fake_create_instance as *const ()),
        b"vkDestroyInstance" => fn_ptr(fake_destroy_instance as *const ()),
        b"vkCreateDevice" => fn_ptr(fake_create_device as *const ()),
        b"vkCreateWin32SurfaceKHR" => fn_ptr(fake_create_surface as *const ()),
        b"vkDestroySurfaceKHR" => fn_ptr(fake_destroy_surface as *const ()),
        b"vkGetPhysicalDeviceSurfaceFormatsKHR" => fn_ptr(fake_formats as *const ()),
        b"vkGetPhysicalDeviceSurfaceFormats2KHR" => fn_ptr(fake_formats2 as *const ()),
        b"vkGetPhysicalDeviceProperties" | b"vkCreateSwapchainKHR" | b"vkInstanceOnlyCmd" => {
            fn_ptr(fake_instance_fallback_proc as *const ())
        }
        _ => None,
    }
}

unsafe extern "system" fn fake_next_physical_gipa(
    _handle: Handle,
    name: *const c_char,
) -> Function {
    if name.is_null() {
        return None;
    }
    // SAFETY: callers pass NUL-terminated C literals.
    let bytes = unsafe { CStr::from_ptr(name).to_bytes() };
    match bytes {
        b"vkGetPhysicalDeviceFeatures" => fn_ptr(fake_physical_feature_proc as *const ()),
        b"vkEnumeratePhysicalDeviceGroups" => fn_ptr(fake_dummy_proc as *const ()),
        _ => None,
    }
}

unsafe extern "system" fn fake_next_gdpa(_handle: Handle, name: *const c_char) -> Function {
    if name.is_null() {
        return None;
    }
    // SAFETY: callers pass NUL-terminated C literals.
    let bytes = unsafe { CStr::from_ptr(name).to_bytes() };
    match bytes {
        b"vkDestroyDevice" => fn_ptr(fake_destroy_device as *const ()),
        b"vkQueuePresentKHR" => fn_ptr(fake_device_fallback_proc as *const ()),
        _ => None,
    }
}

#[test]
fn formats_are_not_duplicated_and_short_buffers_are_bounded() {
    let sdr = Format {
        format: 44,
        color_space: 0,
    };
    let mut values = vec![sdr];
    extend(&mut values);
    extend(&mut values);
    assert_eq!(values.len(), 3);
    let mut buffer = [Format::default(); 2];
    let mut count = 1;
    assert_eq!(
        // SAFETY: `count` is a local and the closure indexes a two-entry buffer.
        unsafe { copy_formats(&values, &mut count, |i, format| buffer[i] = format) },
        INCOMPLETE
    );
    assert_eq!(count, 1);
    assert_eq!(buffer[0], sdr);
    assert_eq!(buffer[1], Format::default());
}

#[test]
fn negotiation_validates_inputs_and_sets_distinct_physical_proc_addr() {
    assert_eq!(
        // SAFETY: passing null exercises the null check in vkNegotiateLoaderLayerInterfaceVersion.
        unsafe { vkNegotiateLoaderLayerInterfaceVersion(ptr::null_mut()) },
        FAILED
    );
    let mut bad_kind = Negotiation {
        kind: 2,
        next: ptr::null_mut(),
        version: 2,
        gipa: None,
        gdpa: None,
        physical_gipa: None,
    };
    assert_eq!(
        // SAFETY: `bad_kind` is a live Negotiation struct.
        unsafe { vkNegotiateLoaderLayerInterfaceVersion(&mut bad_kind) },
        FAILED
    );

    let mut bad_ver = Negotiation {
        kind: 1,
        next: ptr::null_mut(),
        version: 0,
        gipa: None,
        gdpa: None,
        physical_gipa: None,
    };
    assert_eq!(
        // SAFETY: `bad_ver` is a live Negotiation struct.
        unsafe { vkNegotiateLoaderLayerInterfaceVersion(&mut bad_ver) },
        FAILED
    );

    // SAFETY: dummy function pointer retyped for test sentinel verification.
    let sentinel = unsafe {
        std::mem::transmute::<Function, Option<Gipa>>(fn_ptr(fake_dummy_proc as *const ()))
    };
    let expected_gipa: Option<Gipa> = Some(vkGetInstanceProcAddr);
    let expected_gdpa: Option<Gipa> = Some(vkGetDeviceProcAddr);
    let expected_physical: Option<Gipa> = Some(vkGetPhysicalDeviceProcAddr);
    let mut v1 = Negotiation {
        kind: 1,
        next: ptr::null_mut(),
        version: 1,
        gipa: None,
        gdpa: None,
        physical_gipa: sentinel,
    };
    assert_eq!(
        // SAFETY: `v1` is a live Negotiation struct with version 1.
        unsafe { vkNegotiateLoaderLayerInterfaceVersion(&mut v1) },
        SUCCESS
    );
    assert_eq!(v1.version, 1);
    assert_eq!(
        v1.gipa.map(|f| f as usize),
        expected_gipa.map(|f| f as usize)
    );
    assert_eq!(
        v1.gdpa.map(|f| f as usize),
        expected_gdpa.map(|f| f as usize)
    );
    assert_eq!(
        v1.physical_gipa.map(|f| f as usize),
        sentinel.map(|f| f as usize),
        "v1 negotiation must leave physical_gipa untouched"
    );

    let mut v2 = Negotiation {
        kind: 1,
        next: ptr::null_mut(),
        version: 4,
        gipa: None,
        gdpa: None,
        physical_gipa: None,
    };
    assert_eq!(
        // SAFETY: `v2` is a live Negotiation struct with version >= 2.
        unsafe { vkNegotiateLoaderLayerInterfaceVersion(&mut v2) },
        SUCCESS
    );
    assert_eq!(v2.version, 2);
    assert_eq!(
        v2.gipa.map(|f| f as usize),
        expected_gipa.map(|f| f as usize)
    );
    assert_eq!(
        v2.gdpa.map(|f| f as usize),
        expected_gdpa.map(|f| f as usize)
    );
    assert_eq!(
        v2.physical_gipa.map(|f| f as usize),
        expected_physical.map(|f| f as usize)
    );
    assert_ne!(
        v2.physical_gipa.map(|p| p as usize),
        v2.gipa.map(|p| p as usize),
        "physical_gipa must not be aliased to vkGetInstanceProcAddr"
    );
}

#[test]
fn gipa_gdpa_and_physical_gipa_route_commands_correctly() {
    // SAFETY: testing proc-addr exports with null instance/device handles and valid C literals.
    unsafe {
        assert!(vkGetInstanceProcAddr(ptr::null_mut(), ptr::null()).is_none());
        assert!(vkGetPhysicalDeviceProcAddr(ptr::null_mut(), ptr::null()).is_none());
        assert!(vkGetDeviceProcAddr(ptr::null_mut(), ptr::null()).is_none());
        assert_eq!(
            vkGetInstanceProcAddr(ptr::null_mut(), c"vkGetInstanceProcAddr".as_ptr())
                .map(|p| p as usize),
            Some(vkGetInstanceProcAddr as *const () as usize)
        );
        assert_eq!(
            vkGetInstanceProcAddr(ptr::null_mut(), c"vkCreateInstance".as_ptr())
                .map(|p| p as usize),
            Some(create as *const () as usize)
        );
        assert!(vkGetInstanceProcAddr(ptr::null_mut(), c"vkUnknownGlobal".as_ptr()).is_none());
    }

    // Create a fake instance through the layer's vkCreateInstance hook.
    let mut link = Link {
        next: ptr::null_mut(),
        gipa: fake_next_gipa,
        physical_gipa: fn_ptr(fake_next_physical_gipa as *const ()),
    };
    let chain = Chain {
        kind: 47,
        next: ptr::null(),
        function: 0,
        link: &mut link,
    };
    let info = CreateInfo {
        kind: 1,
        next: (&raw const chain).cast(),
        flags: 0,
        app: ptr::null(),
        layers: 0,
        layer_names: ptr::null(),
        extensions: 0,
        extension_names: ptr::null(),
    };
    let mut inst: Handle = ptr::null_mut();
    // SAFETY: `info` points to valid layer-link structs and `inst` receives an owned dispatchable mock.
    assert_eq!(unsafe { create(&info, ptr::null(), &mut inst) }, SUCCESS);
    assert!(!inst.is_null());

    // Physical-device GIPA returns our physical-device hooks and next layer's physical-device
    // commands, NEVER global/instance/device commands via vkGetInstanceProcAddr.
    // SAFETY: `inst` is live until destroyed below and every command name is a valid C string literal.
    unsafe {
        assert_eq!(
            vkGetPhysicalDeviceProcAddr(inst, c"vkGetPhysicalDeviceSurfaceFormatsKHR".as_ptr())
                .map(|p| p as usize),
            Some(formats as *const () as usize)
        );
        assert_eq!(
            vkGetPhysicalDeviceProcAddr(inst, c"vkGetPhysicalDeviceSurfaceFormats2KHR".as_ptr())
                .map(|p| p as usize),
            Some(formats2 as *const () as usize)
        );
        assert_eq!(
            vkGetPhysicalDeviceProcAddr(inst, c"vkGetPhysicalDeviceFeatures".as_ptr())
                .map(|p| p as usize),
            Some(fake_physical_feature_proc as *const () as usize)
        );
        for non_physical in [
            c"vkCreateInstance",
            c"vkDestroyInstance",
            c"vkCreateDevice",
            c"vkGetDeviceProcAddr",
            c"vkGetInstanceProcAddr",
            c"vkCreateWin32SurfaceKHR",
            c"vkDestroySurfaceKHR",
            c"vkInstanceOnlyCmd",
            c"vkCreateSwapchainKHR",
        ] {
            assert!(
                vkGetPhysicalDeviceProcAddr(inst, non_physical.as_ptr()).is_none(),
                "physical_gipa must not return non-physical command {non_physical:?}"
            );
        }

        // GIPA forwards unknown instance commands through `d.next`.
        assert_eq!(
            vkGetInstanceProcAddr(inst, c"vkInstanceOnlyCmd".as_ptr()).map(|p| p as usize),
            Some(fake_instance_fallback_proc as *const () as usize)
        );
    }

    // Create a fake physical device sharing the instance's dispatch key and a VkDevice.
    let mut phys = FakeDispatchable {
        // SAFETY: `inst` points to our FakeDispatchable created above.
        dispatch_table: unsafe { *inst.cast::<usize>() },
    };
    let mut dev_link = DeviceLink {
        next: ptr::null_mut(),
        gipa: fake_next_gipa,
        gdpa: fake_next_gdpa,
    };
    let dev_chain = DeviceChain {
        kind: 48,
        next: ptr::null(),
        function: 0,
        link: &mut dev_link,
    };
    let dev_info = CreateInfo {
        kind: 3,
        next: (&raw const dev_chain).cast(),
        flags: 0,
        app: ptr::null(),
        layers: 0,
        layer_names: ptr::null(),
        extensions: 0,
        extension_names: ptr::null(),
    };
    let mut dev: Handle = ptr::null_mut();
    assert_eq!(
        // SAFETY: `phys` shares `inst`'s dispatch key and `dev_info` carries a valid DeviceChain.
        unsafe {
            create_device(
                (&raw mut phys).cast(),
                (&raw const dev_info).cast(),
                ptr::null(),
                &mut dev,
            )
        },
        SUCCESS
    );
    assert!(!dev.is_null());

    // SAFETY: `dev` and `inst` are owned mock dispatchable handles created above and cleaned up here.
    unsafe {
        assert_eq!(
            vkGetDeviceProcAddr(dev, c"vkGetDeviceProcAddr".as_ptr()).map(|p| p as usize),
            Some(vkGetDeviceProcAddr as *const () as usize)
        );
        assert_eq!(
            vkGetDeviceProcAddr(dev, c"vkDestroyDevice".as_ptr()).map(|p| p as usize),
            Some(destroy_device as *const () as usize)
        );
        assert_eq!(
            vkGetDeviceProcAddr(dev, c"vkQueuePresentKHR".as_ptr()).map(|p| p as usize),
            Some(fake_device_fallback_proc as *const () as usize)
        );
        assert!(vkGetDeviceProcAddr(dev, c"vkUnknownDeviceCmd".as_ptr()).is_none());

        // Tear down device and instance cleanly.
        destroy_device(dev, ptr::null());
        drop(Box::from_raw(dev.cast::<FakeDispatchable>()));
        assert!(
            key(inst).is_some(),
            "instance key remains valid until vkDestroyInstance"
        );
        destroy(inst, ptr::null());
        drop(Box::from_raw(inst.cast::<FakeDispatchable>()));
    }
    let _ = PHYSICAL_DISPATCH.load(Ordering::Relaxed);
}

#[test]
fn formats2_preserves_caller_pnext_and_fills_known_injected_structs() {
    unsafe extern "system" fn fake_sdr_formats2(
        _dev: Handle,
        _info: *const SurfaceInfo,
        count: *mut u32,
        out: *mut Format2,
    ) -> i32 {
        let sdr = Format {
            format: 44,
            color_space: 0,
        };
        // SAFETY: caller passes valid `count` and optionally an `out` buffer of `*count` slots.
        unsafe {
            if out.is_null() {
                *count = 1;
                return SUCCESS;
            }
            let cap = *count as usize;
            let n = cap.min(1);
            if n == 1 {
                (*out).format = sdr;
                if !(*out).next.is_null() {
                    let hdr = &mut *((*out).next as *mut VulkanHeader);
                    if hdr.kind == IMAGE_COMPRESSION_PROPERTIES_EXT {
                        let props = &mut *((*out).next as *mut ImageCompressionProperties);
                        props.flags = 0x11;
                        props.fixed_rate_flags = 0x22;
                    }
                }
            }
            *count = n as u32;
            if cap == 0 { INCOMPLETE } else { SUCCESS }
        }
    }

    unsafe extern "system" fn fake_sdr_gipa(_handle: Handle, name: *const c_char) -> Function {
        if name.is_null() {
            return None;
        }
        // SAFETY: callers pass NUL-terminated C literals.
        match unsafe { CStr::from_ptr(name).to_bytes() } {
            b"vkCreateInstance" => fn_ptr(fake_create_instance as *const ()),
            b"vkDestroyInstance" => fn_ptr(fake_destroy_instance as *const ()),
            b"vkGetPhysicalDeviceSurfaceFormats2KHR" => fn_ptr(fake_sdr_formats2 as *const ()),
            _ => None,
        }
    }

    let mut link = Link {
        next: ptr::null_mut(),
        gipa: fake_sdr_gipa,
        physical_gipa: None,
    };
    let chain = Chain {
        kind: 47,
        next: ptr::null(),
        function: 0,
        link: &mut link,
    };
    let info = CreateInfo {
        kind: 1,
        next: (&raw const chain).cast(),
        flags: 0,
        app: ptr::null(),
        layers: 0,
        layer_names: ptr::null(),
        extensions: 0,
        extension_names: ptr::null(),
    };
    let mut inst: Handle = ptr::null_mut();
    // SAFETY: `info` has valid CreateInfo + LayerLink; `inst` receives owned FakeDispatchable.
    unsafe {
        assert_eq!(create(&info, ptr::null(), &mut inst), SUCCESS);
        let key_val = key(inst).unwrap();
        let fake_surface = 0xDEAD_BEEF_u64;
        state()
            .lock()
            .unwrap()
            .surfaces
            .insert(fake_surface, (key_val, usize::MAX));

        let s_info = SurfaceInfo {
            kind: 1000119001,
            next: ptr::null(),
            surface: fake_surface,
        };
        let mut pnext = [
            ImageCompressionProperties {
                kind: IMAGE_COMPRESSION_PROPERTIES_EXT,
                next: ptr::null_mut(),
                flags: 0xDEAD_BEEF,
                fixed_rate_flags: 0xDEAD_BEEF,
            },
            ImageCompressionProperties {
                kind: IMAGE_COMPRESSION_PROPERTIES_EXT,
                next: ptr::null_mut(),
                flags: 0xDEAD_BEEF,
                fixed_rate_flags: 0xDEAD_BEEF,
            },
            ImageCompressionProperties {
                kind: IMAGE_COMPRESSION_PROPERTIES_EXT,
                next: ptr::null_mut(),
                flags: 0xDEAD_BEEF,
                fixed_rate_flags: 0xDEAD_BEEF,
            },
        ];
        let mut out = [
            Format2 {
                kind: 1000119002,
                next: (&raw mut pnext[0]).cast(),
                format: Format::default(),
            },
            Format2 {
                kind: 1000119002,
                next: (&raw mut pnext[1]).cast(),
                format: Format::default(),
            },
            Format2 {
                kind: 1000119002,
                next: (&raw mut pnext[2]).cast(),
                format: Format::default(),
            },
        ];
        // Extend path: surface registered -> injects 2 HDR formats, bypassing stream_active via usize::MAX?
        // Note `inject()` checks `stream_active()` when window is 0; insert real `inject` conditions or verify extend helper path:
        // Exercise `formats2` directly when next-layer is called; first verify non-injected base pass-through fills pNext,
        // and directly test `fill_known_pnext` on injected chains.
        let mut count = 3u32;
        assert_eq!(
            formats2(inst, &s_info, &mut count, out.as_mut_ptr()),
            SUCCESS
        );
        assert_eq!(count, 1);
        assert_eq!(pnext[0].flags, 0x11);
        assert_eq!(pnext[0].fixed_rate_flags, 0x22);

        fill_known_pnext((&raw mut pnext[1]).cast());
        assert_eq!(pnext[1].flags, 0);
        assert_eq!(pnext[1].fixed_rate_flags, 0);
        assert_eq!(pnext[1].kind, IMAGE_COMPRESSION_PROPERTIES_EXT);

        state().lock().unwrap().surfaces.remove(&fake_surface);
        destroy(inst, ptr::null());
        drop(Box::from_raw(inst.cast::<FakeDispatchable>()));
    }
}

#[test]
fn inject_cache_reuses_window_and_stream_probes_within_ttl_and_expires() {
    let stream_probes = AtomicUsize::new(0);
    let hdr_probes = AtomicUsize::new(0);
    let surface_a = 0xAA01u64;
    let surface_b = 0xAA02u64;
    let win_a = 0x1111usize;
    let win_b = 0x2222usize;
    {
        let mut s = state().lock().unwrap();
        s.surfaces.insert(surface_a, (0xF001, win_a));
        s.surfaces.insert(surface_b, (0xF001, win_b));
        s.inject = InjectCache::default();
    }

    let probe_stream = || {
        stream_probes.fetch_add(1, Ordering::Relaxed);
        true
    };
    let probe_hdr = |window: usize| {
        hdr_probes.fetch_add(1, Ordering::Relaxed);
        window == win_a
    };

    let t0 = Instant::now();
    assert!(inject_with(surface_a, t0, probe_stream, probe_hdr));
    assert!(inject_with(
        surface_a,
        t0 + Duration::from_millis(50),
        probe_stream,
        probe_hdr
    ));
    assert_eq!(stream_probes.load(Ordering::Relaxed), 1);
    assert_eq!(hdr_probes.load(Ordering::Relaxed), 1);

    assert!(!inject_with(
        surface_b,
        t0 + Duration::from_millis(100),
        probe_stream,
        probe_hdr
    ));
    assert_eq!(stream_probes.load(Ordering::Relaxed), 1);
    assert_eq!(hdr_probes.load(Ordering::Relaxed), 2);

    assert!(inject_with(
        surface_a,
        t0 + INJECT_CACHE_TTL + Duration::from_millis(1),
        probe_stream,
        probe_hdr
    ));
    assert_eq!(stream_probes.load(Ordering::Relaxed), 2);
    assert_eq!(hdr_probes.load(Ordering::Relaxed), 3);

    let mut s = state().lock().unwrap();
    s.surfaces.remove(&surface_a);
    s.surfaces.remove(&surface_b);
    s.inject = InjectCache::default();
}
