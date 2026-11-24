//! JNI bridge for `com.limelight.nvstream.jni.NightjarInputJni`, exposing
//! `nightjar_policy::controller` deadzone, remapping, arrival/capability, Steam
//! Controls, slot reservation, and multi-context input fusion to Android.

mod arrival;
mod deadzone;
mod mapping;
mod slots;

use jni::JNIEnv;
use jni::objects::{JFloatArray, JIntArray};
use jni::sys::{jint, jstring};
use nightjar_policy::controller::slots::{SlotAllocator, SlotAssignment};

use crate::slab::MutexSlab;

pub(super) static SLOT_SLAB: MutexSlab<SlotAllocator> = MutexSlab::new();

pub(super) fn write_ints(env: &mut JNIEnv, out: &JIntArray, values: &[i32]) {
    if !out.is_null()
        && env
            .get_array_length(out)
            .is_ok_and(|n| n >= values.len() as i32)
    {
        let _ = env.set_int_array_region(out, 0, values);
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
