// Copyright (c) 2026 Chase Payne
// SPDX-License-Identifier: MIT
//
// Ported from Nonary/libvirtualgamepad driver/src/driver.cpp.
// Adapted for Rubylight by RamazanKara (https://github.com/RamazanKara/Rubylight).

//! Portable controller slot state machine and VHF/IOCTL protocol router.
//!
//! Keeps all controller bookkeeping, admission rules, feedback coalescing/rings,
//! D0 exit loss semantics, and report serialization independent of WDF/Win32 types
//! so every lifecycle invariant can be tested with ordinary `cargo test`.

use crate::profiles::pid_ff::{PidEngine, PidRumble};
use crate::profiles::{
    self, ProfileDefinition, dualsense, dualshock4, is_playstation, is_steam_controller,
    steam_controller, switch_pro,
};
use crate::protocol::{
    CONTROLLER_CONTAINER_BASE, ControllerIdRequest, CreateControllerRequest, FeedbackCounters,
    FeedbackEvent, InputStateRequest, MAX_CONTROLLERS, PROTOCOL_VERSION, PlayStationOutputFeedback,
    Profile, QueryInfoResponse, RequestHeader, SteamControllerAttributesRequest, feature_mask,
    format_instance_id_utf16, saturate_counter,
};
use crate::report_pump::{MAX_REPORT_BYTES, ReportBuffer, ReportPump};
use crate::steam_haptic_queue::{HapticRing, SettingsRing};

mod input;
#[cfg(test)]
mod tests;
mod vhf;

/// Windows NTSTATUS codes returned by the driver's IOCTL and VHF handlers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum DriverStatus {
    Success = 0x0000_0000u32 as i32,
    NotSupported = 0xC000_00BBu32 as i32,
    InvalidParameter = 0xC000_000Du32 as i32,
    InvalidDeviceRequest = 0xC000_0010u32 as i32,
    AccessDenied = 0xC000_0022u32 as i32,
    BufferTooSmall = 0xC000_0023u32 as i32,
    InvalidBufferSize = 0xC000_0206u32 as i32,
    DeviceNotReady = 0xC000_00A3u32 as i32,
    DevicePoweredOff = 0x8000_000Fu32 as i32,
    DeviceBusy = 0x8000_0011u32 as i32,
    Cancelled = 0xC000_0120u32 as i32,
    NoMoreEntries = 0x8000_001Au32 as i32,
    NotFound = 0xC000_0225u32 as i32,
    DeviceRemoved = 0xC000_02B6u32 as i32,
}

impl DriverStatus {
    pub const fn ntstatus(self) -> i32 {
        self as i32
    }

    pub const fn is_success(self) -> bool {
        (self as i32) >= 0
    }
}

pub const PID_TICK_MS: u32 = 10;
pub const SC26_TICK_MS: u32 = 4;
pub const SC26_RESEND_AFTER_US: u64 = 3_000;
pub const SC26_BATTERY_PERIOD_US: u64 = 3_500_000;

/// Opaque identity of a WDF file handle owning controller slots.
pub type OwnerToken = usize;

/// Opaque handle representing an active VHF child (e.g. `VHFHANDLE as usize`).
pub type VhfHandleToken = usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SlotState {
    #[default]
    Empty,
    Starting,
    Active,
    Stopping,
    Lost,
}

/// Complete mutable state of a single controller slot (owned under `state_lock`).
#[derive(Clone, Debug)]
pub struct ControllerSlot {
    pub controller_id: u32,
    pub generation: u64,
    pub vhf: Option<VhfHandleToken>,
    pub owner: OwnerToken,
    pub selected_profile: Option<Profile>,
    pub state: SlotState,

    pub feedback_pending: bool,
    pub feedback: FeedbackEvent,
    pub haptic_queue: HapticRing,
    pub settings_queue: SettingsRing,
    pub submits_in_flight: u32,

    pub playstation_feedback: PlayStationOutputFeedback,
    pub force_feedback: bool,
    pub pid_rumble_valid: bool,
    pub pid_rumble: PidRumble,

    pub have_last_input: bool,
    pub last_input: InputStateRequest,
    pub ds4: dualshock4::Ds4State,
    pub ds5: dualsense::Ds5State,
    pub switch_pro: switch_pro::SwitchState,
    pub sc26: steam_controller::Sc26State,
    pub sc26_wire_sequence: u8,

    pub pump: ReportPump,
    pub pid: PidEngine,

    pub container_id: u128,
    pub instance_id_utf16: [u16; 32],
    pub instance_id_utf16_len: usize,
}

impl ControllerSlot {
    pub fn new(controller_id: u32) -> Self {
        Self {
            controller_id,
            generation: 0,
            vhf: None,
            owner: 0,
            selected_profile: None,
            state: SlotState::Empty,
            feedback_pending: false,
            feedback: FeedbackEvent::default(),
            haptic_queue: HapticRing::new(),
            settings_queue: SettingsRing::new(),
            submits_in_flight: 0,
            playstation_feedback: PlayStationOutputFeedback::default(),
            force_feedback: false,
            pid_rumble_valid: false,
            pid_rumble: PidRumble::default(),
            have_last_input: false,
            last_input: InputStateRequest::default(),
            ds4: dualshock4::Ds4State::default(),
            ds5: dualsense::Ds5State::default(),
            switch_pro: switch_pro::SwitchState::default(),
            sc26: steam_controller::Sc26State::default(),
            sc26_wire_sequence: 0,
            pump: ReportPump::new(),
            pid: PidEngine::new(),
            container_id: 0,
            instance_id_utf16: [0; 32],
            instance_id_utf16_len: 0,
        }
    }

    pub fn reset(&mut self) {
        let controller_id = self.controller_id;
        let generation = self.generation.wrapping_add(1);
        *self = Self::new(controller_id);
        self.generation = generation;
    }

    pub fn is_owned_by(&self, owner: OwnerToken) -> bool {
        owner != 0 && self.owner == owner
    }

    pub fn accepts_vhf_callbacks(&self, device_stopping: bool, device_powered: bool) -> bool {
        !device_stopping
            && device_powered
            && self.vhf.is_some()
            && matches!(self.state, SlotState::Starting | SlotState::Active)
    }

    pub fn inactive_status(&self, owner: OwnerToken) -> DriverStatus {
        if self.is_owned_by(owner) && self.state == SlotState::Lost {
            DriverStatus::DeviceRemoved
        } else {
            DriverStatus::DeviceNotReady
        }
    }

    pub fn counters(&self) -> FeedbackCounters {
        FeedbackCounters {
            haptic_evicted: saturate_counter(self.haptic_queue.evicted()),
            settings_evicted: saturate_counter(self.settings_queue.evicted()),
            transitions_dropped: saturate_counter(self.pump.dropped_transitions()),
            unknown_commands: saturate_counter(self.sc26.features.unknown_commands),
        }
    }
}

/// Action returned after a report is taken out of a slot's `ReportPump` under `state_lock`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingSubmit {
    pub controller_id: u32,
    pub vhf: VhfHandleToken,
    pub report_id: u8,
    pub length: u32,
    pub rearm_keepalive: bool,
}

/// Result of preparing a controller creation under `state_lock`.
#[derive(Clone, Copy, Debug)]
pub struct CreatePlan {
    pub controller_id: u32,
    pub profile: &'static ProfileDefinition,
    pub container_id: u128,
    pub instance_id_utf16: [u16; 32],
    pub instance_id_utf16_len: usize,
    pub register_feature_callbacks: bool,
}

/// Core device state protected by `state_lock` (inner lock).
#[derive(Clone, Debug)]
pub struct DeviceState {
    pub stopping: bool,
    pub powered: bool,
    pub sc26_timer_running: bool,
    pub sc26_timer_parked: bool,
    pub slots: [ControllerSlot; MAX_CONTROLLERS as usize],
}

impl Default for DeviceState {
    fn default() -> Self {
        Self::new()
    }
}

impl DeviceState {
    pub fn new() -> Self {
        Self {
            stopping: false,
            powered: false,
            sc26_timer_running: false,
            sc26_timer_parked: false,
            slots: core::array::from_fn(|i| ControllerSlot::new(i as u32)),
        }
    }

    pub fn fill_query_info(&self) -> QueryInfoResponse {
        QueryInfoResponse {
            header: RequestHeader::new(QueryInfoResponse::WIRE_SIZE as u32),
            minimum_protocol_version: PROTOCOL_VERSION,
            maximum_protocol_version: PROTOCOL_VERSION,
            available_profiles: profiles::available_profiles(),
            available_features: feature_mask::ALL,
            maximum_controllers: MAX_CONTROLLERS,
            counters: FeedbackCounters::default(),
        }
    }

    pub fn query_controller_info(
        &self,
        owner: OwnerToken,
        req: &ControllerIdRequest,
    ) -> Result<QueryInfoResponse, DriverStatus> {
        if req.controller_id >= MAX_CONTROLLERS {
            return Err(DriverStatus::InvalidParameter);
        }
        let slot = &self.slots[req.controller_id as usize];
        if slot.state == SlotState::Empty {
            return Err(DriverStatus::NotFound);
        }
        if !slot.is_owned_by(owner) {
            return Err(DriverStatus::AccessDenied);
        }
        let mut resp = self.fill_query_info();
        resp.counters = slot.counters();
        Ok(resp)
    }

    /// Pre-flight check outside `lifetime_gate` before calling OS Configuration Manager.
    pub fn check_create_preflight(
        &self,
        owner: OwnerToken,
        req: &CreateControllerRequest,
    ) -> Result<(&'static ProfileDefinition, u64), DriverStatus> {
        if req.controller_id >= MAX_CONTROLLERS || req.reserved != 0 {
            return Err(DriverStatus::InvalidParameter);
        }
        let Some(profile) = Profile::from_raw(req.requested_profile) else {
            return Err(DriverStatus::NotSupported);
        };
        let Some(def) = profiles::find_profile(profile) else {
            return Err(DriverStatus::NotSupported);
        };
        if self.stopping {
            return Err(DriverStatus::DeviceNotReady);
        }
        if !self.powered {
            return Err(DriverStatus::DevicePoweredOff);
        }
        let slot = &self.slots[req.controller_id as usize];
        let available = slot.state == SlotState::Empty
            || (slot.state == SlotState::Lost && slot.is_owned_by(owner));
        if !available {
            return Err(DriverStatus::DeviceBusy);
        }
        Ok((def, slot.generation))
    }

    /// Commits slot transition to `Starting` under `lifetime_gate + state_lock`.
    pub fn begin_create(
        &mut self,
        owner: OwnerToken,
        req: &CreateControllerRequest,
        expected_generation: u64,
        child_present: bool,
    ) -> Result<CreatePlan, DriverStatus> {
        let (def, _) = self.check_create_preflight(owner, req)?;
        let slot = &mut self.slots[req.controller_id as usize];
        if slot.generation != expected_generation || child_present {
            return Err(DriverStatus::DeviceBusy);
        }
        slot.reset();
        slot.owner = owner;
        slot.selected_profile = Some(def.id);
        slot.state = SlotState::Starting;
        slot.force_feedback = def.force_feedback;
        slot.ds4.features.address[0] = req.controller_id as u8;
        slot.ds5.features.address[0] = req.controller_id as u8;
        slot.sc26.stamp_slot_serial(req.controller_id);

        slot.container_id = CONTROLLER_CONTAINER_BASE | u128::from(req.controller_id as u8);
        slot.instance_id_utf16_len =
            format_instance_id_utf16(req.controller_id, &mut slot.instance_id_utf16);

        let register_feature_callbacks =
            def.force_feedback || is_playstation(def.id) || is_steam_controller(def.id);

        Ok(CreatePlan {
            controller_id: req.controller_id,
            profile: def,
            container_id: slot.container_id,
            instance_id_utf16: slot.instance_id_utf16,
            instance_id_utf16_len: slot.instance_id_utf16_len,
            register_feature_callbacks,
        })
    }

    pub fn release_starting_slot(&mut self, controller_id: u32, owner: OwnerToken) {
        let Some(slot) = self.slots.get_mut(controller_id as usize) else {
            return;
        };
        if slot.is_owned_by(owner)
            && matches!(slot.state, SlotState::Starting | SlotState::Stopping)
        {
            slot.reset();
        }
    }

    pub fn publish_vhf(&mut self, controller_id: u32, vhf: VhfHandleToken) {
        if let Some(slot) = self.slots.get_mut(controller_id as usize) {
            slot.vhf = Some(vhf);
        }
    }

    /// Finalizes `Starting -> Active` if still powered/unstopped; otherwise switches to
    /// `Stopping` and detaches `vhf` so caller can synchronously `VhfDelete`.
    pub fn commit_start(
        &mut self,
        controller_id: u32,
        owner: OwnerToken,
        start_succeeded: bool,
    ) -> bool {
        let slot = &mut self.slots[controller_id as usize];
        if start_succeeded
            && !self.stopping
            && self.powered
            && slot.is_owned_by(owner)
            && slot.state == SlotState::Starting
        {
            slot.state = SlotState::Active;
            true
        } else {
            slot.state = SlotState::Stopping;
            slot.feedback_pending = false;
            slot.vhf = None;
            false
        }
    }

    pub fn begin_destroy_checked(
        &mut self,
        owner: OwnerToken,
        controller_id: u32,
    ) -> Result<Option<VhfHandleToken>, DriverStatus> {
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
        Ok(self.begin_destroy_owned(owner, controller_id))
    }

    /// Transitions an owned slot toward deletion. Returns `Some(vhf)` if the caller
    /// must wait for `submits_in_flight == 0` and call `VhfDelete` + `finish_destroy_owned`.
    pub fn begin_destroy_owned(
        &mut self,
        owner: OwnerToken,
        controller_id: u32,
    ) -> Option<VhfHandleToken> {
        let slot = self.slots.get_mut(controller_id as usize)?;
        if !slot.is_owned_by(owner) || slot.state == SlotState::Empty {
            return None;
        }
        match slot.state {
            SlotState::Starting => {
                slot.state = SlotState::Stopping;
                None
            }
            SlotState::Lost => {
                slot.reset();
                None
            }
            SlotState::Active | SlotState::Stopping => {
                slot.state = SlotState::Stopping;
                slot.feedback_pending = false;
                slot.haptic_queue.clear();
                slot.settings_queue.clear();
                slot.vhf.take()
            }
            SlotState::Empty => None,
        }
    }

    pub fn finish_destroy_owned(&mut self, controller_id: u32, owner: OwnerToken) {
        if let Some(slot) = self.slots.get_mut(controller_id as usize)
            && slot.is_owned_by(owner)
            && slot.state == SlotState::Stopping
        {
            slot.reset();
        }
    }

    pub fn set_sc_attributes(
        &mut self,
        owner: OwnerToken,
        req: &SteamControllerAttributesRequest,
    ) -> DriverStatus {
        if req.controller_id >= MAX_CONTROLLERS {
            return DriverStatus::InvalidParameter;
        }
        let slot = &mut self.slots[req.controller_id as usize];
        if slot.state == SlotState::Empty {
            return DriverStatus::NotFound;
        }
        if !slot.is_owned_by(owner) {
            return DriverStatus::AccessDenied;
        }
        if !slot.selected_profile.is_some_and(is_steam_controller) {
            return DriverStatus::NotSupported;
        }
        steam_controller::apply_attributes(req, &mut slot.sc26);
        DriverStatus::Success
    }

    pub fn poll_feedback(
        &mut self,
        owner: OwnerToken,
        req: &ControllerIdRequest,
    ) -> Result<FeedbackEvent, DriverStatus> {
        if req.controller_id >= MAX_CONTROLLERS {
            return Err(DriverStatus::InvalidParameter);
        }
        let slot = &mut self.slots[req.controller_id as usize];
        if slot.state == SlotState::Empty {
            return Err(DriverStatus::NotFound);
        }
        if !slot.is_owned_by(owner) {
            return Err(DriverStatus::AccessDenied);
        }
        if slot.state != SlotState::Active {
            return Err(slot.inactive_status(owner));
        }
        if let Some(ev) = slot.settings_queue.pop() {
            return Ok(ev);
        }
        if let Some(ev) = slot.haptic_queue.pop() {
            return Ok(ev);
        }
        if !slot.feedback_pending {
            return Err(DriverStatus::NoMoreEntries);
        }
        slot.feedback_pending = false;
        Ok(slot.feedback)
    }

    /// Retires active VHF handles on D0 exit, preserving `(owner, selected_profile)` as `Lost`.
    pub fn begin_power_down(&mut self) -> [Option<VhfHandleToken>; MAX_CONTROLLERS as usize] {
        self.powered = false;
        self.sc26_timer_running = false;
        self.sc26_timer_parked = false;
        let mut handles = [None; MAX_CONTROLLERS as usize];
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if slot.state == SlotState::Active && slot.vhf.is_some() {
                slot.state = SlotState::Stopping;
                slot.feedback_pending = false;
                handles[i] = slot.vhf.take();
            }
        }
        handles
    }

    pub fn finish_power_down(
        &mut self,
        retired: &[Option<VhfHandleToken>; MAX_CONTROLLERS as usize],
    ) {
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if retired[i].is_some() && slot.state == SlotState::Stopping {
                let owner = slot.owner;
                let profile = slot.selected_profile;
                slot.reset();
                slot.owner = owner;
                slot.selected_profile = profile;
                slot.state = SlotState::Lost;
            }
        }
    }

    pub fn begin_stop_all(&mut self) -> [Option<VhfHandleToken>; MAX_CONTROLLERS as usize] {
        self.stopping = true;
        self.powered = false;
        self.sc26_timer_running = false;
        self.sc26_timer_parked = false;
        let mut handles = [None; MAX_CONTROLLERS as usize];
        for (i, slot) in self.slots.iter_mut().enumerate() {
            slot.feedback_pending = false;
            slot.haptic_queue.clear();
            slot.settings_queue.clear();
            match slot.state {
                SlotState::Active | SlotState::Starting => {
                    slot.state = SlotState::Stopping;
                    handles[i] = slot.vhf.take();
                }
                SlotState::Stopping => {
                    handles[i] = slot.vhf.take();
                }
                SlotState::Lost => {
                    slot.reset();
                }
                SlotState::Empty => {}
            }
        }
        handles
    }

    pub fn finish_stop_slot(&mut self, controller_id: u32) {
        if let Some(slot) = self.slots.get_mut(controller_id as usize) {
            slot.reset();
        }
    }
}

/// Checks if `instance_id` (e.g. `"RubylightGamepad0"`) appears as an `&<instance>` suffix
/// in a double-null-terminated `CM_Get_Device_ID_ListW("HID", ...)` buffer.
pub fn matches_cm_child_instance_list(utf16_multi_sz: &[u16], target_instance_id: &str) -> bool {
    let len = utf16_multi_sz.len();
    if len == 0 || utf16_multi_sz[len - 1] != 0 || (len > 1 && utf16_multi_sz[len - 2] != 0) {
        // Malformed CM buffer must fail closed (`present = true`).
        return true;
    }
    let mut i = 0usize;
    while i < len && utf16_multi_sz[i] != 0 {
        let start = i;
        while i < len && utf16_multi_sz[i] != 0 {
            i += 1;
        }
        let item = &utf16_multi_sz[start..i];
        if let Some(amp_pos) = item.iter().rposition(|&c| c == u16::from(b'&')) {
            let suffix = &item[amp_pos + 1..];
            if ascii_eq_ignore_case(suffix, target_instance_id.as_bytes()) {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn ascii_eq_ignore_case(utf16: &[u16], ascii: &[u8]) -> bool {
    if utf16.len() != ascii.len() {
        return false;
    }
    utf16
        .iter()
        .zip(ascii.iter())
        .all(|(&w, &a)| w <= 0x7F && (w as u8).eq_ignore_ascii_case(&a))
}

/// Kept for `ReportBuffer` import cleanliness across crates.
pub const fn max_report_bytes() -> usize {
    let _ = size_of::<ReportBuffer>();
    MAX_REPORT_BYTES
}
