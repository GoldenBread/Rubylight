// Copyright (c) 2026 Chase Payne
// SPDX-License-Identifier: MIT
//
// Ported from Nonary/libvirtualgamepad driver/src/driver.cpp.
// Adapted for Rubylight by RamazanKara (https://github.com/RamazanKara/Rubylight).

//! Input, touch, motion, and battery IOCTL submission and report-pump
//! scheduling for [`super::DeviceState`].

use super::{
    ControllerSlot, DeviceState, DriverStatus, OwnerToken, PendingSubmit, SlotState, VhfHandleToken,
};
use crate::profiles::{
    dualsense, dualshock4, generic, is_playstation, is_steam_controller, steam_controller,
    switch_pro, xbox_one, xbox_series,
};
use crate::protocol::{
    BatteryStateRequest, InputStateRequest, MAX_CONTROLLERS, MotionStateRequest, Profile,
    TouchEvent, TouchStateRequest,
};
use crate::report_pump::{MAX_REPORT_BYTES, ReportKind};

impl DeviceState {
    pub(super) fn take_and_stamp(
        slot: &mut ControllerSlot,
        now_us: u64,
    ) -> Option<(u8, u32, VhfHandleToken, bool)> {
        let taken = slot.pump.take()?;
        let is_sc26_input = taken.report_id == steam_controller::INPUT_REPORT_ID
            && taken.length > 1
            && slot.selected_profile.is_some_and(is_steam_controller);
        if is_sc26_input {
            taken.data[1] = slot.sc26_wire_sequence;
            slot.sc26_wire_sequence = slot.sc26_wire_sequence.wrapping_add(1);
            slot.sc26.last_report_us = now_us;
        }
        let vhf = slot.vhf?;
        slot.submits_in_flight += 1;
        Some((taken.report_id, taken.length, vhf, is_sc26_input))
    }

    pub fn submit_finish(&mut self, controller_id: u32) {
        if let Some(slot) = self.slots.get_mut(controller_id as usize)
            && slot.submits_in_flight > 0
        {
            slot.submits_in_flight -= 1;
        }
    }

    /// Enqueues an encoded report on `controller_id` if callbacks are admitted and takes
    /// the next ready report if VHF is currently ready.
    pub fn pump_enqueue_and_take(
        &mut self,
        controller_id: u32,
        data: Option<(&[u8], u8, ReportKind)>,
        now_us: u64,
        rearm_keepalive: bool,
    ) -> Result<Option<PendingSubmit>, DriverStatus> {
        let stopping = self.stopping;
        let powered = self.powered;
        let slot = &mut self.slots[controller_id as usize];
        if !slot.accepts_vhf_callbacks(stopping, powered) {
            return Err(DriverStatus::DeviceNotReady);
        }
        if let Some((bytes, report_id, kind)) = data {
            let _ = slot.pump.enqueue(bytes, report_id, kind);
        }
        if let Some((report_id, length, vhf, is_sc26)) = Self::take_and_stamp(slot, now_us) {
            Ok(Some(PendingSubmit {
                controller_id,
                vhf,
                report_id,
                length,
                rearm_keepalive: rearm_keepalive && is_sc26,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn on_vhf_ready_for_next_report(
        &mut self,
        controller_id: u32,
        now_us: u64,
    ) -> (Option<PendingSubmit>, bool) {
        let stopping = self.stopping;
        let powered = self.powered;
        let timer_parked = self.sc26_timer_parked;
        let slot = &mut self.slots[controller_id as usize];
        if !slot.accepts_vhf_callbacks(stopping, powered) {
            return (None, false);
        }
        slot.pump.set_ready();
        let pending =
            Self::take_and_stamp(slot, now_us).map(|(report_id, length, vhf, is_sc26)| {
                PendingSubmit {
                    controller_id,
                    vhf,
                    report_id,
                    length,
                    rearm_keepalive: is_sc26,
                }
            });
        let unpark_keepalive = timer_parked
            && slot.have_last_input
            && slot.selected_profile.is_some_and(is_steam_controller);
        if unpark_keepalive {
            self.sc26_timer_parked = false;
        }
        (pending, unpark_keepalive)
    }

    /// Handles `IOCTL_SUBMIT_INPUT_STATE`. Returns `(Option<PendingSubmit>, start_sc26_keepalive)`.
    pub fn submit_input_state(
        &mut self,
        owner: OwnerToken,
        req: &InputStateRequest,
        now_us: u64,
        has_sc26_timer: bool,
    ) -> Result<(Option<PendingSubmit>, bool), DriverStatus> {
        if req.controller_id >= MAX_CONTROLLERS || req.reserved != 0 {
            return Err(DriverStatus::InvalidParameter);
        }
        let stopping = self.stopping;
        let slot = &mut self.slots[req.controller_id as usize];
        if slot.state == SlotState::Empty
            || !slot.is_owned_by(owner)
            || stopping
            || slot.state != SlotState::Active
            || slot.vhf.is_none()
        {
            return Err(slot.inactive_status(owner));
        }

        let Some(profile) = slot.selected_profile else {
            return Err(DriverStatus::DeviceNotReady);
        };

        let mut start_keepalive = false;
        let mut buf = [0u8; MAX_REPORT_BYTES];
        let (len, report_id, kind) = match profile {
            Profile::DualShock4 => {
                slot.last_input = *req;
                slot.have_last_input = true;
                let kind = slot
                    .pump
                    .classify(req.buttons, req.left_trigger, req.right_trigger);
                let r = dualshock4::encode_input(req, Some(&mut slot.ds4), now_us);
                buf[..r.len()].copy_from_slice(&r);
                (r.len(), dualshock4::INPUT_REPORT_ID, kind)
            }
            Profile::DualSense => {
                slot.last_input = *req;
                slot.have_last_input = true;
                let kind = slot
                    .pump
                    .classify(req.buttons, req.left_trigger, req.right_trigger);
                let r = dualsense::encode_input(req, Some(&mut slot.ds5), now_us);
                buf[..r.len()].copy_from_slice(&r);
                (r.len(), dualsense::INPUT_REPORT_ID, kind)
            }
            Profile::SteamController => {
                slot.last_input = *req;
                slot.have_last_input = true;
                steam_controller::sc26_tick(&mut slot.sc26, now_us);
                let r = steam_controller::encode_input(req, &mut slot.sc26);
                let enc_btns = u32::from_le_bytes([r[2], r[3], r[4], r[5]]);
                let cls_btns = steam_controller::classify_buttons(&slot.sc26.device, enc_btns);
                let kind = slot
                    .pump
                    .classify(cls_btns, req.left_trigger, req.right_trigger);
                if has_sc26_timer && !self.sc26_timer_running {
                    self.sc26_timer_running = true;
                    start_keepalive = true;
                }
                buf[..r.len()].copy_from_slice(&r);
                (r.len(), steam_controller::INPUT_REPORT_ID, kind)
            }
            Profile::SwitchPro => {
                slot.last_input = *req;
                slot.have_last_input = true;
                let kind = slot
                    .pump
                    .classify(req.buttons, req.left_trigger, req.right_trigger);
                let r = switch_pro::encode_input(req, Some(&mut slot.switch_pro));
                buf[..r.len()].copy_from_slice(&r);
                (r.len(), switch_pro::INPUT_REPORT_ID, kind)
            }
            Profile::XboxOne => {
                slot.last_input = *req;
                slot.have_last_input = true;
                let kind = slot
                    .pump
                    .classify(req.buttons, req.left_trigger, req.right_trigger);
                let r = xbox_one::encode_input(req);
                buf[..r.len()].copy_from_slice(&r);
                (r.len(), xbox_one::INPUT_REPORT_ID, kind)
            }
            Profile::XboxSeries => {
                slot.last_input = *req;
                slot.have_last_input = true;
                let kind = slot
                    .pump
                    .classify(req.buttons, req.left_trigger, req.right_trigger);
                let r = xbox_series::encode_input(req);
                buf[..r.len()].copy_from_slice(&r);
                (r.len(), xbox_series::INPUT_REPORT_ID, kind)
            }
            Profile::GenericHid | Profile::GenericPid => {
                slot.last_input = *req;
                slot.have_last_input = true;
                let kind = slot
                    .pump
                    .classify(req.buttons, req.left_trigger, req.right_trigger);
                let r = generic::encode_generic_input(req);
                buf[..r.len()].copy_from_slice(&r);
                (r.len(), generic::GENERIC_INPUT_REPORT_ID, kind)
            }
            Profile::Xbox360 => return Err(DriverStatus::NotSupported),
        };

        let submit = self.pump_enqueue_and_take(
            req.controller_id,
            Some((&buf[..len], report_id, kind)),
            now_us,
            true,
        )?;
        Ok((submit, start_keepalive))
    }

    fn check_state_update_target(
        &self,
        owner: OwnerToken,
        controller_id: u32,
    ) -> Result<Profile, DriverStatus> {
        if controller_id >= MAX_CONTROLLERS {
            return Err(DriverStatus::InvalidParameter);
        }
        let slot = &self.slots[controller_id as usize];
        if slot.state == SlotState::Empty {
            return Err(DriverStatus::NotFound);
        }
        if !slot.is_owned_by(owner) {
            return Err(DriverStatus::AccessDenied);
        }
        if self.stopping || slot.state != SlotState::Active || slot.vhf.is_none() {
            return Err(slot.inactive_status(owner));
        }
        let Some(profile) = slot.selected_profile else {
            return Err(DriverStatus::DeviceNotReady);
        };
        if !is_playstation(profile)
            && profile != Profile::SwitchPro
            && !is_steam_controller(profile)
        {
            return Err(DriverStatus::NotSupported);
        }
        Ok(profile)
    }

    fn rebuild_profile_report(
        &mut self,
        controller_id: u32,
        kind: ReportKind,
        now_us: u64,
    ) -> Result<Option<PendingSubmit>, DriverStatus> {
        let slot = &mut self.slots[controller_id as usize];
        if self.stopping || slot.state != SlotState::Active || slot.vhf.is_none() {
            return Err(DriverStatus::DeviceNotReady);
        }
        if !slot.have_last_input {
            return Ok(None);
        }
        let profile = slot.selected_profile.ok_or(DriverStatus::DeviceNotReady)?;
        let mut buf = [0u8; MAX_REPORT_BYTES];
        let (len, report_id) = match profile {
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
            Profile::SteamController => {
                steam_controller::sc26_tick(&mut slot.sc26, now_us);
                let r = steam_controller::encode_input(&slot.last_input, &mut slot.sc26);
                buf[..r.len()].copy_from_slice(&r);
                (r.len(), steam_controller::INPUT_REPORT_ID)
            }
            _ => return Ok(None),
        };
        self.pump_enqueue_and_take(
            controller_id,
            Some((&buf[..len], report_id, kind)),
            now_us,
            true,
        )
    }

    pub fn submit_touch_state(
        &mut self,
        owner: OwnerToken,
        req: &TouchStateRequest,
        now_us: u64,
    ) -> Result<Option<PendingSubmit>, DriverStatus> {
        if req.reserved != 0 {
            return Err(DriverStatus::InvalidParameter);
        }
        let profile = self.check_state_update_target(owner, req.controller_id)?;
        if profile == Profile::SwitchPro {
            return Err(DriverStatus::NotSupported);
        }
        let slot = &mut self.slots[req.controller_id as usize];
        let applied = match profile {
            Profile::DualShock4 => dualshock4::apply_touch(req, &mut slot.ds4),
            Profile::DualSense => dualsense::apply_touch(req, &mut slot.ds5),
            Profile::SteamController => steam_controller::apply_touch(req, &mut slot.sc26),
            _ => return Err(DriverStatus::NotSupported),
        };
        if !applied {
            return Err(DriverStatus::InvalidParameter);
        }
        let kind = match TouchEvent::from_raw(req.event_type) {
            Some(TouchEvent::Move | TouchEvent::Hover) => ReportKind::Continuous,
            _ => ReportKind::Transition,
        };
        self.rebuild_profile_report(req.controller_id, kind, now_us)
    }

    pub fn submit_motion_state(
        &mut self,
        owner: OwnerToken,
        req: &MotionStateRequest,
        now_us: u64,
        has_sc26_timer: bool,
    ) -> Result<Option<PendingSubmit>, DriverStatus> {
        if req.reserved0 != [0; 3] {
            return Err(DriverStatus::InvalidParameter);
        }
        let profile = self.check_state_update_target(owner, req.controller_id)?;
        let slot = &mut self.slots[req.controller_id as usize];
        let applied = match profile {
            Profile::DualShock4 => dualshock4::apply_motion(req, &mut slot.ds4),
            Profile::DualSense => dualsense::apply_motion(req, &mut slot.ds5),
            Profile::SteamController => steam_controller::apply_motion(req, &mut slot.sc26, now_us),
            Profile::SwitchPro => switch_pro::apply_motion(req, &mut slot.switch_pro),
            _ => return Err(DriverStatus::NotSupported),
        };
        if !applied {
            return Err(DriverStatus::InvalidParameter);
        }
        if profile == Profile::SteamController && has_sc26_timer && self.sc26_timer_running {
            return Ok(None);
        }
        self.rebuild_profile_report(req.controller_id, ReportKind::Continuous, now_us)
    }

    pub fn submit_battery_state(
        &mut self,
        owner: OwnerToken,
        req: &BatteryStateRequest,
        now_us: u64,
    ) -> Result<Option<PendingSubmit>, DriverStatus> {
        if req.reserved != 0 {
            return Err(DriverStatus::InvalidParameter);
        }
        let profile = self.check_state_update_target(owner, req.controller_id)?;
        let slot = &mut self.slots[req.controller_id as usize];
        let applied = match profile {
            Profile::DualShock4 => dualshock4::apply_battery(req, &mut slot.ds4),
            Profile::DualSense => dualsense::apply_battery(req, &mut slot.ds5),
            Profile::SteamController => steam_controller::apply_battery(req, &mut slot.sc26),
            Profile::SwitchPro => switch_pro::apply_battery(req, &mut slot.switch_pro),
            _ => return Err(DriverStatus::NotSupported),
        };
        if !applied {
            return Err(DriverStatus::InvalidParameter);
        }
        if profile == Profile::SteamController {
            let ready = slot.pump.ready();
            slot.sc26.last_battery_us = if ready { now_us } else { 0 };
            if !ready {
                return Ok(None);
            }
            let battery_report = steam_controller::encode_battery(&slot.sc26);
            return self.pump_enqueue_and_take(
                req.controller_id,
                Some((
                    &battery_report,
                    steam_controller::BATTERY_REPORT_ID,
                    ReportKind::Aside,
                )),
                now_us,
                true,
            );
        }
        self.rebuild_profile_report(req.controller_id, ReportKind::Continuous, now_us)
    }
}
