//! JNI exports for `nightjar_policy::controller::slots`.

use jni::JNIEnv;
use jni::objects::{JClass, JIntArray};
use jni::sys::{JNI_FALSE, jboolean, jfloat, jint, jlong, jshort};
use nightjar_policy::controller::slots::{
    ContextInput, SlotAllocator, announces_onscreen_pad, fuse_contexts,
    launch_pass_for_input_device,
};

use super::{SLOT_SLAB, pack_slot_assignment, write_ints};
use crate::policy::jbool;

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_slotStateCreate(
    _: JNIEnv,
    _: JClass,
    initial_controllers: jshort,
    onscreen_own_player_session: jboolean,
    touchpad_button_session: jboolean,
    steam_controls_session: jboolean,
) -> jlong {
    SLOT_SLAB.insert(SlotAllocator::new(
        initial_controllers as u16,
        onscreen_own_player_session != JNI_FALSE,
        touchpad_button_session != JNI_FALSE,
        steam_controls_session != JNI_FALSE,
    ))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_slotStateDestroy(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) {
    let _ = SLOT_SLAB.remove(handle);
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_activeControllerMask(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
    send_controllers: jboolean,
    multi_controller: jboolean,
    onscreen_controller: jboolean,
    layout_gamepad_used: jboolean,
    has_built_in_player_one: jboolean,
) -> jshort {
    SLOT_SLAB
        .with(handle, |s| {
            s.active_controller_mask(
                send_controllers != JNI_FALSE,
                multi_controller != JNI_FALSE,
                onscreen_controller != JNI_FALSE,
                layout_gamepad_used != JNI_FALSE,
                has_built_in_player_one != JNI_FALSE,
            )
        })
        .unwrap_or(0) as jshort
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_assignInputDeviceSlot(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
    external: jboolean,
    has_joystick_axes: jboolean,
    has_gamepad_buttons: jboolean,
    has_adjacent_associated_joystick: jboolean,
    multi_controller: jboolean,
    built_in_pad_slot: jint,
) -> jint {
    let built_in = u8::try_from(built_in_pad_slot).ok();
    SLOT_SLAB
        .with(handle, |s| {
            pack_slot_assignment(s.assign_input_device_slot(
                external != JNI_FALSE,
                has_joystick_axes != JNI_FALSE,
                has_gamepad_buttons != JNI_FALSE,
                has_adjacent_associated_joystick != JNI_FALSE,
                multi_controller != JNI_FALSE,
                built_in,
            ))
        })
        .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_resolveAssociatedFallbackSlot(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
) -> jint {
    SLOT_SLAB
        .with(handle, |s| {
            pack_slot_assignment(s.resolve_associated_fallback())
        })
        .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_assignUsbDeviceSlot(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
    multi_controller: jboolean,
) -> jint {
    SLOT_SLAB
        .with(handle, |s| {
            pack_slot_assignment(s.assign_usb_device_slot(multi_controller != JNI_FALSE))
        })
        .unwrap_or(0)
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_releaseReservedSlot(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
    slot: jshort,
    was_reserved: jboolean,
    assigned: jboolean,
    send_controllers: jboolean,
    multi_controller: jboolean,
    onscreen_controller: jboolean,
    layout_gamepad_used: jboolean,
    has_built_in_player_one: jboolean,
    pending_usb_arrival: jboolean,
) -> jint {
    SLOT_SLAB
        .with(handle, |s| {
            let out = s.release_slot(
                slot,
                was_reserved != JNI_FALSE,
                assigned != JNI_FALSE,
                send_controllers != JNI_FALSE,
                multi_controller != JNI_FALSE,
                onscreen_controller != JNI_FALSE,
                layout_gamepad_used != JNI_FALSE,
                has_built_in_player_one != JNI_FALSE,
                pending_usb_arrival != JNI_FALSE,
            );
            i32::from(out.active_mask)
                | (i32::from(out.should_send_neutral) << 16)
                | (i32::from(out.cleared_slot_input) << 17)
        })
        .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_markSlotHasInput(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
    slot: jshort,
) {
    let _ = SLOT_SLAB.with(handle, |s| s.mark_slot_has_input(slot));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_slotHasInput(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
    slot: jshort,
) -> jboolean {
    jbool(
        SLOT_SLAB
            .with(handle, |s| s.slot_has_input(slot))
            .unwrap_or(false),
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_announcesOnscreenPad(
    _: JNIEnv,
    _: JClass,
    handle: jlong,
    send_controllers: jboolean,
    onscreen_controller: jboolean,
    layout_gamepad_used: jboolean,
    has_slot_zero_device: jboolean,
) -> jboolean {
    let res = SLOT_SLAB
        .with(handle, |s| {
            announces_onscreen_pad(
                s.onscreen_own_player_session,
                s.touchpad_button_session,
                s.steam_controls_session,
                send_controllers != JNI_FALSE,
                onscreen_controller != JNI_FALSE,
                layout_gamepad_used != JNI_FALSE,
                has_slot_zero_device != JNI_FALSE,
            )
        })
        .unwrap_or(false);
    jbool(res)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_launchPassForInputDevice(
    _: JNIEnv,
    _: JClass,
    external: jboolean,
    has_joystick_axes: jboolean,
) -> jint {
    i32::from(launch_pass_for_input_device(
        external != JNI_FALSE,
        has_joystick_axes != JNI_FALSE,
    ))
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarInputJni_fuseControllerInputs(
    mut env: JNIEnv,
    _: JClass,
    packed_contexts: JIntArray,
    count: jint,
    steam_controls_session: jboolean,
    controller_number: jshort,
    steam_pad_touch_x: jfloat,
    mouse_emulation_active: jboolean,
    out: JIntArray,
) {
    let n = count.max(0) as usize;
    let needed = n.saturating_mul(6);
    let mut stack_buf = [0i32; 96];
    let mut heap_buf = Vec::new();
    let available = if needed == 0 || packed_contexts.is_null() {
        0
    } else {
        env.get_array_length(&packed_contexts).unwrap_or(0).max(0) as usize
    };
    let slice: &[i32] = if needed == 0 || available < needed {
        &[]
    } else if needed <= stack_buf.len() {
        if env
            .get_int_array_region(&packed_contexts, 0, &mut stack_buf[..needed])
            .is_ok()
        {
            &stack_buf[..needed]
        } else {
            &[]
        }
    } else {
        heap_buf.resize(needed, 0);
        if env
            .get_int_array_region(&packed_contexts, 0, &mut heap_buf)
            .is_ok()
        {
            &heap_buf
        } else {
            &[]
        }
    };

    let mut stack_ctx = [ContextInput::default(); 16];
    let mut heap_ctx = Vec::new();
    let contexts: &[ContextInput] = if slice.len() < needed {
        &[]
    } else if n <= stack_ctx.len() {
        for (i, dst) in stack_ctx[..n].iter_mut().enumerate() {
            let base = i * 6;
            let tr = slice[base + 1];
            *dst = ContextInput {
                buttons: slice[base] as u32,
                left_trigger: (tr & 0xFF) as u8,
                right_trigger: ((tr >> 8) & 0xFF) as u8,
                left_stick_x: slice[base + 2] as i16,
                left_stick_y: slice[base + 3] as i16,
                right_stick_x: slice[base + 4] as i16,
                right_stick_y: slice[base + 5] as i16,
            };
        }
        &stack_ctx[..n]
    } else {
        heap_ctx.reserve(n);
        for i in 0..n {
            let base = i * 6;
            let tr = slice[base + 1];
            heap_ctx.push(ContextInput {
                buttons: slice[base] as u32,
                left_trigger: (tr & 0xFF) as u8,
                right_trigger: ((tr >> 8) & 0xFF) as u8,
                left_stick_x: slice[base + 2] as i16,
                left_stick_y: slice[base + 3] as i16,
                right_stick_x: slice[base + 4] as i16,
                right_stick_y: slice[base + 5] as i16,
            });
        }
        &heap_ctx
    };

    let fused = fuse_contexts(
        contexts,
        steam_controls_session != JNI_FALSE,
        controller_number,
        steam_pad_touch_x,
        mouse_emulation_active != JNI_FALSE,
    );
    write_ints(
        &mut env,
        &out,
        &[
            fused.buttons as i32,
            i32::from(fused.left_trigger),
            i32::from(fused.right_trigger),
            i32::from(fused.left_stick_x),
            i32::from(fused.left_stick_y),
            i32::from(fused.right_stick_x),
            i32::from(fused.right_stick_y),
        ],
    );
}
