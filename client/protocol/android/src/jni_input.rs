//! JNI input entry points for `com.limelight.nvstream.jni.NightjarJni` (mouse, keyboard,
//! touch, physical touchpad, pen, and controllers).

use jni::JNIEnv;
use jni::objects::{JByteArray, JClass, JFloatArray, JIntArray};
use jni::sys::{JNI_FALSE, jboolean, jbyte, jfloat, jint, jshort};

use nightjar_core::input::InputQueue;

use crate::{engine, extras, touchpad};

/// Runs an input call on the session: 0 or the C API's error code (-2 without a stream).
fn input(f: impl FnOnce(&mut InputQueue) -> Result<(), nightjar_core::input::InputError>) -> jint {
    engine::with_session(-2, |s| extras::input_result(s.input(f)))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_sendMouseMove(
    _: JNIEnv,
    _: JClass,
    dx: jshort,
    dy: jshort,
) {
    input(|q| q.send_mouse_move(dx, dy));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_sendMousePosition(
    _: JNIEnv,
    _: JClass,
    x: jshort,
    y: jshort,
    width: jshort,
    height: jshort,
) {
    input(|q| q.send_mouse_position(x, y, width, height));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_sendMouseMoveAsMousePosition(
    _: JNIEnv,
    _: JClass,
    dx: jshort,
    dy: jshort,
    width: jshort,
    height: jshort,
) {
    input(|q| q.send_mouse_move_as_position(dx, dy, width, height));
}

/// `BUTTON_ACTION_PRESS` (0x07) or `BUTTON_ACTION_RELEASE` (0x08).
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_sendMouseButton(
    _: JNIEnv,
    _: JClass,
    action: jbyte,
    button: jbyte,
) {
    input(|q| q.send_mouse_button(button as u8, action == 0x07));
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_sendMultiControllerInput(
    _: JNIEnv,
    _: JClass,
    controller: jshort,
    active_mask: jshort,
    buttons: jint,
    left_trigger: jbyte,
    right_trigger: jbyte,
    left_x: jshort,
    left_y: jshort,
    right_x: jshort,
    right_y: jshort,
) {
    input(|q| {
        q.send_controller(
            controller as u16,
            active_mask as u16,
            buttons as u32,
            left_trigger as u8,
            right_trigger as u8,
            (left_x, left_y),
            (right_x, right_y),
        )
    });
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_sendTouchEvent(
    _: JNIEnv,
    _: JClass,
    event: jbyte,
    pointer_id: jint,
    x: jfloat,
    y: jfloat,
    pressure: jfloat,
    contact_major: jfloat,
    contact_minor: jfloat,
    rotation: jshort,
) -> jint {
    input(|q| {
        q.send_touch(
            event as u8,
            pointer_id as u32,
            x,
            y,
            pressure,
            contact_major,
            contact_minor,
            rotation as u16,
        )
    })
}

/// How the host takes a physical touchpad's fingers: `RustBridge.TOUCHPAD_NONE`, `_INDIVIDUAL`
/// or `_FRAME`; none without a session.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_touchpadTransport(
    _: JNIEnv,
    _: JClass,
) -> jint {
    engine::with_session(touchpad::TRANSPORT_NONE, |s| {
        s.input(|q| touchpad::transport_code(q.feature_flags()))
    })
}

/// One motion event of a physical touchpad: the first `count` fingers still on it (ids, and
/// positions on the touchpad's surface, 0-1), its click, and its size in millimetres (0 when
/// unknown). Returns 0, or the input error code.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_touchpadFrame(
    env: JNIEnv,
    _: JClass,
    device: jint,
    ids: JIntArray,
    xs: JFloatArray,
    ys: JFloatArray,
    pressures: JFloatArray,
    count: jint,
    button: jboolean,
    width_mm: jshort,
    height_mm: jshort,
) -> jint {
    let Some(session) = engine::session_id() else {
        return -2;
    };
    let count = count.clamp(0, 16) as usize;
    let mut id = vec![0i32; count];
    let mut x = vec![0f32; count];
    let mut y = vec![0f32; count];
    let mut pressure = vec![0f32; count];
    if count > 0
        && (env.get_int_array_region(&ids, 0, &mut id).is_err()
            || env.get_float_array_region(&xs, 0, &mut x).is_err()
            || env.get_float_array_region(&ys, 0, &mut y).is_err()
            || env
                .get_float_array_region(&pressures, 0, &mut pressure)
                .is_err())
    {
        return -3;
    }
    let contacts = touchpad::contacts(&id, &x, &y, &pressure);
    let outs = touchpad::frame(
        session,
        device,
        contacts,
        button != JNI_FALSE,
        width_mm.max(0) as u16,
        height_mm.max(0) as u16,
    );
    input(|q| touchpad::send(outs, q))
}

/// Lifts the touchpad's fingers on the host (capture or focus lost).
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_touchpadCancel(
    _: JNIEnv,
    _: JClass,
) -> jint {
    let Some(session) = engine::session_id() else {
        return -2;
    };
    let outs = touchpad::track(session, |t| t.cancel());
    input(|q| touchpad::send(outs, q))
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_sendPenEvent(
    _: JNIEnv,
    _: JClass,
    event: jbyte,
    tool: jbyte,
    buttons: jbyte,
    x: jfloat,
    y: jfloat,
    pressure: jfloat,
    contact_major: jfloat,
    contact_minor: jfloat,
    rotation: jshort,
    tilt: jbyte,
) -> jint {
    input(|q| {
        q.send_pen(
            event as u8,
            tool as u8,
            buttons as u8,
            x,
            y,
            pressure,
            contact_major,
            contact_minor,
            rotation as u16,
            tilt as u8,
            // Android's MotionEvent has no barrel roll axis: pens go as the stock packet.
            rubylight_protocol::pen::ROLL_UNKNOWN,
        )
    })
}

/// Which controller `controller` is, by its USB vendor and product: send it before the arrival.
/// The queue drops it for a host without `ControllerIdentityVersion`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_sendControllerIdentityEvent(
    _: JNIEnv,
    _: JClass,
    controller: jbyte,
    vendor: jint,
    product: jint,
) -> jint {
    input(|q| q.send_controller_identity(controller as u8, vendor as u16, product as u16))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_sendControllerArrivalEvent(
    _: JNIEnv,
    _: JClass,
    controller: jbyte,
    active_mask: jshort,
    kind: jbyte,
    supported_buttons: jint,
    capabilities: jshort,
) -> jint {
    input(|q| {
        q.send_controller_arrival(
            controller as u8,
            active_mask as u16,
            kind as u8,
            supported_buttons as u32,
            capabilities as u16,
        )
    })
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_sendControllerTouchEvent(
    _: JNIEnv,
    _: JClass,
    controller: jbyte,
    event: jbyte,
    pointer_id: jint,
    x: jfloat,
    y: jfloat,
    pressure: jfloat,
) -> jint {
    // LiSendControllerTouchEvent is touchpad 0 of LiSendControllerTouchEvent2.
    input(|q| {
        q.send_controller_touch(
            controller as u8,
            event as u8,
            0,
            pointer_id as u32,
            x,
            y,
            pressure,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_sendControllerMotionEvent(
    _: JNIEnv,
    _: JClass,
    controller: jbyte,
    kind: jbyte,
    x: jfloat,
    y: jfloat,
    z: jfloat,
) -> jint {
    input(|q| q.send_controller_motion(controller as u8, kind as u8, x, y, z))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_sendControllerBatteryEvent(
    _: JNIEnv,
    _: JClass,
    controller: jbyte,
    state: jbyte,
    percentage: jbyte,
) -> jint {
    input(|q| q.send_controller_battery(controller as u8, state as u8, percentage as u8))
}

/// `KEY_ACTION_DOWN` (0x03) or `KEY_ACTION_UP` (0x04).
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_sendKeyboardInput(
    _: JNIEnv,
    _: JClass,
    key: jshort,
    action: jbyte,
    modifiers: jbyte,
    flags: jbyte,
) {
    input(|q| q.send_keyboard(key as u16, action == 0x03, modifiers as u8, flags as u8));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_sendMouseHighResScroll(
    _: JNIEnv,
    _: JClass,
    amount: jshort,
) {
    input(|q| q.send_high_res_scroll(amount));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_sendMouseHighResHScroll(
    _: JNIEnv,
    _: JClass,
    amount: jshort,
) {
    input(|q| q.send_high_res_hscroll(amount));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_limelight_nvstream_jni_NightjarJni_sendUtf8Text(
    env: JNIEnv,
    _: JClass,
    text: JByteArray,
) {
    let Ok(bytes) = env.convert_byte_array(&text) else {
        return;
    };
    let text = String::from_utf8_lossy(&bytes);
    input(|q| q.send_text(&text));
}
