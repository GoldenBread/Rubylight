//! JNI bridge for `com.limelight.nvstream.jni.NightjarInputJni`, exposing
//! `nightjar_policy::controller` deadzone, remapping, arrival/capability, Steam
//! Controls, slot reservation, multi-context input fusion, combos/GuideHold/mouse/menu
//! state machines, and rumble/sensor/gyro/battery policies to Android.

mod arrival;
mod deadzone;
mod haptics_sensor;
mod mapping;
mod slots;
mod state;

use jni::JNIEnv;
use jni::objects::{JFloatArray, JIntArray, JLongArray};
use jni::sys::{jint, jstring};
use nightjar_policy::controller::haptics_sensor::{MotionFilter, RumbleState};
use nightjar_policy::controller::slots::{SlotAllocator, SlotAssignment};
use nightjar_policy::controller::state_machine::UsbInputStates;

use crate::slab::MutexSlab;

pub(super) use crate::policy::jbool;

pub(super) static SLOT_SLAB: MutexSlab<SlotAllocator> = MutexSlab::new();
pub(super) static USB_STATE_SLAB: MutexSlab<UsbInputStates> = MutexSlab::new();
pub(super) static RUMBLE_SLAB: MutexSlab<RumbleState> = MutexSlab::new();
pub(super) static MOTION_FILTER_SLAB: MutexSlab<MotionFilter> = MutexSlab::new();

pub(super) fn write_ints(env: &mut JNIEnv, out: &JIntArray, values: &[i32]) {
    if !out.is_null()
        && env
            .get_array_length(out)
            .is_ok_and(|n| n >= values.len() as i32)
    {
        let _ = env.set_int_array_region(out, 0, values);
    }
}

pub(super) fn write_longs(env: &mut JNIEnv, out: &JLongArray, values: &[i64]) {
    if !out.is_null()
        && env
            .get_array_length(out)
            .is_ok_and(|n| n >= values.len() as i32)
    {
        let _ = env.set_long_array_region(out, 0, values);
    }
}

pub(super) fn write_floats(env: &mut JNIEnv, out: &JFloatArray, values: &[f32]) {
    if !out.is_null()
        && env
            .get_array_length(out)
            .is_ok_and(|n| n >= values.len() as i32)
    {
        let _ = env.set_float_array_region(out, 0, values);
    }
}

pub(super) fn new_jstring(env: &mut JNIEnv, value: &str) -> jstring {
    match env.new_string(value) {
        Ok(s) => s.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

pub(super) fn pack_slot_assignment(res: SlotAssignment) -> jint {
    match res {
        SlotAssignment::Fixed(slot) => i32::from(slot) | (1 << 9),
        SlotAssignment::Reserved(slot) => i32::from(slot) | (1 << 8) | (1 << 9),
        SlotAssignment::LookupAssociated => 1 << 10,
        SlotAssignment::Full => 0,
    }
}
