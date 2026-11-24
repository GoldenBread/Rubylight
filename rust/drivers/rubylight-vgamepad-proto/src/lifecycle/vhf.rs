// Copyright (c) 2026 Chase Payne
// SPDX-License-Identifier: MIT
//
// Ported from Nonary/libvirtualgamepad driver/src/driver.cpp.
// Adapted for Rubylight by RamazanKara (https://github.com/RamazanKara/Rubylight).

//! VHF HID read/write/feature callbacks, PID force-feedback ticking, and
//! Steam Controller keep-alive scheduling for [`super::DeviceState`].

use super::{
    ControllerSlot, DeviceState, DriverStatus, PID_TICK_MS, PendingSubmit, SC26_BATTERY_PERIOD_US,
    SC26_RESEND_AFTER_US, SC26_TICK_MS, SlotState,
};
use crate::profiles::pid_ff;
use crate::profiles::{
    dualsense, dualshock4, generic, is_playstation, is_steam_controller, is_xbox, steam_controller,
    switch_pro, xbox_one, xbox_series,
};
use crate::protocol::{
    FeedbackEvent, FeedbackType, GenericRumbleRgbFeedback, MAX_CONTROLLERS,
    PlayStationOutputFeedback, Profile,
};
use crate::report_pump::{MAX_REPORT_BYTES, ReportKind};
use crate::steam_haptic_queue::SETTINGS_EVENTS_PER_WRITE;

impl DeviceState {
    /// Handles `EvtVhfAsyncOperationGetInputReport`. Returns written slice length on success.
    pub fn on_get_input_report(
        &mut self,
        controller_id: u32,
        requested_report_id: u8,
        out: &mut [u8],
        now_us: u64,
    ) -> Result<usize, DriverStatus> {
        let stopping = self.stopping;
        let powered = self.powered;
        let slot = &mut self.slots[controller_id as usize];
        if !slot.accepts_vhf_callbacks(stopping, powered) {
            return Err(DriverStatus::DeviceNotReady);
        }
        let profile = slot.selected_profile.ok_or(DriverStatus::DeviceNotReady)?;
        let mut buf = [0u8; MAX_REPORT_BYTES];
        let (length, report_id) = match profile {
            Profile::DualShock4 => {
                let r = dualshock4::encode_input(&slot.last_input, Some(&mut slot.ds4), now_us);
                buf[..r.len()].copy_from_slice(&r);
                (r.len(), dualshock4::INPUT_REPORT_ID)
            }
            Profile::DualSense => {
                let r = dualsense::encode_input(&slot.last_input, Some(&mut slot.ds5), now_us);
                buf[..r.len()].copy_from_slice(&r);
                (r.len(), dualsense::INPUT_REPORT_ID)
            }
            Profile::SwitchPro => {
                let r = switch_pro::encode_input(&slot.last_input, Some(&mut slot.switch_pro));
                buf[..r.len()].copy_from_slice(&r);
                (r.len(), switch_pro::INPUT_REPORT_ID)
            }
            Profile::XboxSeries => {
                let r = xbox_series::encode_input(&slot.last_input);
                buf[..r.len()].copy_from_slice(&r);
                (r.len(), xbox_series::INPUT_REPORT_ID)
            }
            Profile::XboxOne => {
                let r = xbox_one::encode_input(&slot.last_input);
                buf[..r.len()].copy_from_slice(&r);
                (r.len(), xbox_one::INPUT_REPORT_ID)
            }
            Profile::SteamController => {
                if requested_report_id == steam_controller::BATTERY_REPORT_ID {
                    let r = steam_controller::encode_battery(&slot.sc26);
                    buf[..r.len()].copy_from_slice(&r);
                    (r.len(), steam_controller::BATTERY_REPORT_ID)
                } else {
                    steam_controller::sc26_tick(&mut slot.sc26, now_us);
                    let mut r = steam_controller::encode_input(&slot.last_input, &mut slot.sc26);
                    r[1] = slot.sc26_wire_sequence;
                    buf[..r.len()].copy_from_slice(&r);
                    (r.len(), steam_controller::INPUT_REPORT_ID)
                }
            }
            Profile::GenericHid | Profile::GenericPid => {
                let r = generic::encode_generic_input(&slot.last_input);
                buf[..r.len()].copy_from_slice(&r);
                (r.len(), generic::GENERIC_INPUT_REPORT_ID)
            }
            Profile::Xbox360 => return Err(DriverStatus::InvalidDeviceRequest),
        };
        if requested_report_id != 0 && requested_report_id != report_id {
            return Err(DriverStatus::InvalidDeviceRequest);
        }
        if out.len() < length {
            return Err(DriverStatus::BufferTooSmall);
        }
        out[..length].copy_from_slice(&buf[..length]);
        Ok(length)
    }

    /// Handles `EvtVhfAsyncOperationGetFeature`. Returns written slice length on success.
    pub fn on_get_feature(
        &self,
        controller_id: u32,
        report_id: u8,
        out: &mut [u8],
    ) -> Result<usize, DriverStatus> {
        let slot = &self.slots[controller_id as usize];
        if !slot.accepts_vhf_callbacks(self.stopping, self.powered) {
            return Err(DriverStatus::DeviceNotReady);
        }
        let Some(profile) = slot.selected_profile else {
            return Err(DriverStatus::InvalidDeviceRequest);
        };
        if is_steam_controller(profile) {
            let written = steam_controller::get_feature(report_id, out, &slot.sc26);
            return if written != 0 {
                Ok(written)
            } else if report_id == steam_controller::FEATURES_REPORT_ID {
                Err(DriverStatus::BufferTooSmall)
            } else {
                Err(DriverStatus::InvalidDeviceRequest)
            };
        }
        if is_playstation(profile) {
            let written = if profile == Profile::DualShock4 {
                dualshock4::get_feature(report_id, out, &slot.ds4.features)
            } else {
                dualsense::get_feature(report_id, out, &slot.ds5.features)
            };
            return if written != 0 {
                Ok(written)
            } else {
                Err(DriverStatus::InvalidDeviceRequest)
            };
        }
        if slot.force_feedback {
            let (bytes, len): ([u8; 5], usize) = match report_id {
                pid_ff::PID_BLOCK_LOAD_REPORT_ID => (slot.pid.block_load(), 5),
                pid_ff::PID_POOL_REPORT_ID => (slot.pid.pool(), 5),
                pid_ff::PID_STATE_REPORT_ID => {
                    let s = slot.pid.state_report();
                    ([s[0], s[1], s[2], 0, 0], 3)
                }
                _ => return Err(DriverStatus::InvalidDeviceRequest),
            };
            if out.len() < len {
                return Err(DriverStatus::BufferTooSmall);
            }
            out[..len].copy_from_slice(&bytes[..len]);
            return Ok(len);
        }
        Err(DriverStatus::InvalidDeviceRequest)
    }

    /// Handles `EvtVhfAsyncOperationSetFeature`.
    pub fn on_set_feature(
        &mut self,
        controller_id: u32,
        report_id: u8,
        data: &[u8],
    ) -> DriverStatus {
        let stopping = self.stopping;
        let powered = self.powered;
        let slot = &mut self.slots[controller_id as usize];
        if !slot.accepts_vhf_callbacks(stopping, powered) {
            return DriverStatus::DeviceNotReady;
        }
        let Some(profile) = slot.selected_profile else {
            return DriverStatus::InvalidDeviceRequest;
        };
        if is_steam_controller(profile) {
            let accepted = steam_controller::set_feature(report_id, data, &mut slot.sc26);
            if accepted && report_id == steam_controller::FEATURES_REPORT_ID {
                let mut events = [FeedbackEvent::default(); SETTINGS_EVENTS_PER_WRITE];
                let count = steam_controller::apply_settings(data, slot.controller_id, &mut events);
                for ev in &events[..count] {
                    slot.settings_queue.enqueue(*ev);
                }
            }
            return if accepted {
                DriverStatus::Success
            } else {
                DriverStatus::InvalidParameter
            };
        }
        if is_playstation(profile) {
            let accepted = if profile == Profile::DualShock4 {
                dualshock4::set_feature(report_id, data, &mut slot.ds4.features)
            } else {
                dualsense::set_feature(report_id, data, &mut slot.ds5.features)
            };
            return if accepted {
                DriverStatus::Success
            } else {
                DriverStatus::InvalidDeviceRequest
            };
        }
        if slot.force_feedback && report_id == pid_ff::PID_CREATE_NEW_EFFECT_REPORT_ID {
            if data.len() < 4 || data[0] != report_id {
                return DriverStatus::InvalidParameter;
            }
            let _ = slot.pid.create_new_effect(data);
            return DriverStatus::Success;
        }
        DriverStatus::InvalidDeviceRequest
    }

    /// Handles `EvtVhfAsyncOperationWriteReport`.
    /// Returns `(status, optional_read_submit, arm_pid_timer)`.
    pub fn on_write_report(
        &mut self,
        controller_id: u32,
        report_id: u8,
        data: &[u8],
        now_us: u64,
    ) -> (DriverStatus, Option<PendingSubmit>, bool) {
        let stopping = self.stopping;
        let powered = self.powered;
        let slot = &mut self.slots[controller_id as usize];
        if !slot.accepts_vhf_callbacks(stopping, powered) {
            return (DriverStatus::DeviceNotReady, None, false);
        }
        let Some(profile) = slot.selected_profile else {
            return (DriverStatus::InvalidDeviceRequest, None, false);
        };

        if is_steam_controller(profile) {
            if let Some(ev) = steam_controller::apply_output(data, slot.controller_id) {
                let _ = slot.haptic_queue.enqueue(ev);
                let mut ack = [0u8; 6];
                let ack_len = steam_controller::encode_haptic_ack(data, &mut ack);
                let submit = if ack_len != 0 {
                    self.pump_enqueue_and_take(
                        controller_id,
                        Some((
                            &ack[..ack_len],
                            steam_controller::HAPTIC_ACK_REPORT_ID,
                            ReportKind::Aside,
                        )),
                        now_us,
                        true,
                    )
                    .ok()
                    .flatten()
                } else {
                    None
                };
                return (DriverStatus::Success, submit, false);
            } else if !data.is_empty()
                && data[0] > steam_controller::HAPTIC_SCRIPT_ID
                && steam_controller::is_output_report(data[0])
            {
                return (DriverStatus::Success, None, false);
            }
            return (DriverStatus::InvalidParameter, None, false);
        }

        if profile == Profile::SwitchPro {
            let mut rumble = PlayStationOutputFeedback::default();
            if switch_pro::decode_rumble(data, &mut rumble) {
                slot.feedback =
                    dualshock4::encode_playstation_feedback(slot.controller_id, &rumble);
                slot.feedback_pending = true;
            }
            let mut queued_reply = false;
            if report_id == switch_pro::USB_COMMAND_ID {
                let mut reply = [0u8; switch_pro::REPORT_SIZE];
                if switch_pro::handle_usb_command(data, Some(&mut slot.switch_pro), &mut reply) != 0
                {
                    queued_reply = slot.pump.enqueue(&reply, reply[0], ReportKind::Priority);
                }
            } else if report_id == switch_pro::RUMBLE_SUBCOMMAND_ID {
                queued_reply = switch_pro::queue_subcommand_reply(
                    data,
                    &slot.last_input,
                    &mut slot.switch_pro,
                    &mut slot.pump,
                );
            }
            let submit = if queued_reply {
                self.pump_enqueue_and_take(controller_id, None, now_us, true)
                    .ok()
                    .flatten()
            } else {
                None
            };
            return (DriverStatus::Success, submit, false);
        }

        if is_playstation(profile) {
            let mut feedback = slot.playstation_feedback;
            let decoded = if profile == Profile::DualShock4 {
                dualshock4::apply_output(data, &mut feedback)
            } else {
                dualsense::apply_output(data, &mut feedback)
            };
            if !decoded {
                return (DriverStatus::InvalidParameter, None, false);
            }
            slot.playstation_feedback = feedback;
            slot.feedback = dualshock4::encode_playstation_feedback(slot.controller_id, &feedback);
            slot.feedback_pending = true;
            return (DriverStatus::Success, None, false);
        }

        if is_xbox(profile) {
            if report_id != xbox_series::OUTPUT_REPORT_ID {
                return (DriverStatus::InvalidDeviceRequest, None, false);
            }
            let Some(rumble) = xbox_series::decode_output(data) else {
                return (DriverStatus::InvalidDeviceRequest, None, false);
            };
            slot.feedback = xbox_series::encode_feedback(slot.controller_id, &rumble);
            slot.feedback_pending = true;
            return (DriverStatus::Success, None, false);
        }

        if slot.force_feedback && report_id != generic::GENERIC_OUTPUT_REPORT_ID {
            let ok = match report_id {
                pid_ff::PID_SET_EFFECT_REPORT_ID => slot.pid.set_effect(data),
                pid_ff::PID_SET_ENVELOPE_REPORT_ID => slot.pid.set_envelope(data),
                pid_ff::PID_SET_CONDITION_REPORT_ID => slot.pid.set_condition(data),
                pid_ff::PID_SET_PERIODIC_REPORT_ID => slot.pid.set_periodic(data),
                pid_ff::PID_SET_CONSTANT_FORCE_REPORT_ID => slot.pid.set_constant_force(data),
                pid_ff::PID_SET_RAMP_FORCE_REPORT_ID => slot.pid.set_ramp_force(data),
                pid_ff::PID_EFFECT_OPERATION_REPORT_ID => slot.pid.effect_operation(data),
                pid_ff::PID_BLOCK_FREE_REPORT_ID => slot.pid.block_free(data),
                pid_ff::PID_DEVICE_CONTROL_REPORT_ID => slot.pid.device_control(data),
                pid_ff::PID_DEVICE_GAIN_REPORT_ID => slot.pid.device_gain(data),
                _ => return (DriverStatus::InvalidDeviceRequest, None, false),
            };
            if !ok {
                return (DriverStatus::InvalidParameter, None, false);
            }
            Self::publish_pid_rumble(slot);
            let arm = slot.pid.needs_tick();
            return (DriverStatus::Success, None, arm);
        }

        if report_id == generic::GENERIC_OUTPUT_REPORT_ID
            && matches!(profile, Profile::GenericHid | Profile::GenericPid)
        {
            if let Some(ev) = generic::encode_generic_feedback(slot.controller_id, data) {
                slot.feedback = ev;
                slot.feedback_pending = true;
                return (DriverStatus::Success, None, false);
            }
            return (DriverStatus::InvalidParameter, None, false);
        }

        (DriverStatus::InvalidDeviceRequest, None, false)
    }

    fn publish_pid_rumble(slot: &mut ControllerSlot) {
        let rumble = slot.pid.rumble();
        if slot.pid_rumble_valid && slot.pid_rumble == rumble {
            return;
        }
        slot.pid_rumble = rumble;
        slot.pid_rumble_valid = true;
        let payload = GenericRumbleRgbFeedback {
            low_frequency: rumble.low_frequency,
            high_frequency: rumble.high_frequency,
            red: 0,
            green: 0,
            blue: 0,
            reserved: 0,
        };
        slot.feedback = FeedbackEvent::new(
            slot.controller_id,
            FeedbackType::GenericRumble,
            &payload.to_bytes(),
        );
        slot.feedback_pending = true;
    }

    pub fn pid_tick(&mut self) -> bool {
        let mut still_playing = false;
        for slot in &mut self.slots {
            if slot.state != SlotState::Active || !slot.force_feedback {
                continue;
            }
            slot.pid.advance(PID_TICK_MS);
            Self::publish_pid_rumble(slot);
            still_playing = still_playing || slot.pid.needs_tick();
        }
        still_playing && !self.stopping
    }

    pub fn any_pid_playing(&self) -> bool {
        !self.stopping
            && self.slots.iter().any(|slot| {
                slot.state == SlotState::Active && slot.force_feedback && slot.pid.needs_tick()
            })
    }

    /// Collects SC2026 keep-alive report submissions under `state_lock`.
    /// Returns `(submits, count, any_active, any_ready)`.
    pub fn sc26_keepalive_collect(
        &mut self,
        now_us: u64,
    ) -> (
        [Option<PendingSubmit>; MAX_CONTROLLERS as usize],
        usize,
        bool,
        bool,
    ) {
        let mut pending = [None; MAX_CONTROLLERS as usize];
        let mut count = 0usize;
        let mut any_active = false;
        let mut any_ready = false;

        if !self.stopping {
            for idx in 0..MAX_CONTROLLERS as usize {
                let slot = &mut self.slots[idx];
                if slot.state != SlotState::Active
                    || !slot.selected_profile.is_some_and(is_steam_controller)
                    || slot.vhf.is_none()
                    || !slot.have_last_input
                {
                    continue;
                }
                any_active = true;
                if !slot.pump.ready() {
                    slot.sc26.last_report_us = now_us;
                    continue;
                }
                any_ready = true;
                let battery_due = now_us >= slot.sc26.last_battery_us
                    && now_us - slot.sc26.last_battery_us >= SC26_BATTERY_PERIOD_US;
                if battery_due {
                    let battery = steam_controller::encode_battery(&slot.sc26);
                    let _ = slot.pump.enqueue(
                        &battery,
                        steam_controller::BATTERY_REPORT_ID,
                        ReportKind::Aside,
                    );
                    slot.sc26.last_battery_us = now_us;
                }
                let resend_due = now_us >= slot.sc26.last_report_us
                    && now_us - slot.sc26.last_report_us >= SC26_RESEND_AFTER_US;
                if !resend_due && !battery_due {
                    continue;
                }
                if resend_due {
                    steam_controller::sc26_tick(&mut slot.sc26, now_us);
                    let report = steam_controller::encode_input(&slot.last_input, &mut slot.sc26);
                    let _ = slot.pump.enqueue(
                        &report,
                        steam_controller::INPUT_REPORT_ID,
                        ReportKind::Continuous,
                    );
                }
                if let Some((report_id, length, vhf, is_sc26)) = Self::take_and_stamp(slot, now_us)
                {
                    pending[count] = Some(PendingSubmit {
                        controller_id: idx as u32,
                        vhf,
                        report_id,
                        length,
                        rearm_keepalive: is_sc26,
                    });
                    count += 1;
                }
            }
        }

        if !any_active {
            self.sc26_timer_running = false;
            self.sc26_timer_parked = false;
        } else {
            self.sc26_timer_parked = !any_ready;
        }

        (pending, count, any_active, any_ready)
    }

    pub fn sc26_next_deadline_us(&self, now_us: u64) -> u64 {
        let tick_us = u64::from(SC26_TICK_MS) * 1000;
        let mut delay_us = tick_us;
        for slot in &self.slots {
            if slot.state != SlotState::Active
                || !slot.selected_profile.is_some_and(is_steam_controller)
                || slot.vhf.is_none()
                || !slot.have_last_input
            {
                continue;
            }
            let elapsed = now_us.saturating_sub(slot.sc26.last_report_us);
            let remaining = if elapsed >= tick_us {
                1
            } else {
                tick_us - elapsed
            };
            if remaining < delay_us {
                delay_us = remaining;
            }
        }
        delay_us
    }
}
