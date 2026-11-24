//! Pure Android/Nightjar rumble routing, gyro liveness/orientation/mapping, and
//! controller battery policies extracted from `ControllerHandler`, `GyroLivenessTracker`,
//! and `GyroAxisTransformer`.

use crate::rumble::{self, RumbleFeel};

use super::MAX_GAMEPADS;

/// Default all-zero window (1 second in nanoseconds) before declaring a gyroscope dead.
pub const DEFAULT_ZERO_NANOS: i64 = 1_000_000_000;

/// Maximum Android sensor sampling rate (Hz) without `HIGH_SAMPLING_RATE_SENSORS`.
pub const MAX_ANDROID_SENSOR_HZ: u16 = 200;

/// Radians-to-degrees factor (`180 / pi`) matching Java `57.2957795f`.
pub const RAD_TO_DEG: f32 = 57.295_78;

/// Minimum and maximum allowed custom gyroscope sensitivities.
pub const MIN_GYRO_SENSITIVITY: f32 = 0.1;
pub const MAX_GYRO_SENSITIVITY: f32 = 3.0;

/// Result of recording a host rumble/trigger update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RumbleRecordOutcome {
    pub allowed_to_record: bool,
    pub active: bool,
}

/// Action to execute on focus pause/resume transitions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PauseTransition {
    NoChange,
    Pause {
        slots: [i16; MAX_GAMEPADS],
        len: usize,
    },
    Resume {
        entries: [(i16, u16, u16, u16, u16); MAX_GAMEPADS],
        len: usize,
    },
}

/// Device vibrator routing outcome for player 1 fallback / OSC vibration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceVibratorAction {
    None,
    Osc {
        amplitude: u8,
        pwm_on_ms: i64,
        pwm_off_ms: i64,
    },
    PlayerOneFallback {
        amplitude: u8,
        pwm_on_ms: i64,
        pwm_off_ms: i64,
    },
}

/// Single-vibrator amplitude + PWM timing plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SingleVibratorPlan {
    pub amplitude: u8,
    pub pwm_on_ms: i64,
    pub pwm_off_ms: i64,
}

/// Per-handler rumble state (permission, feel, pause gate, and latest levels per slot).
#[derive(Clone, Debug)]
pub struct RumbleState {
    pub allowed: bool,
    pub feel: RumbleFeel,
    pub device_fallback_slot: i16,
    pub paused: bool,
    levels: [Option<[u16; 4]>; MAX_GAMEPADS],
}

impl RumbleState {
    pub fn new(allowed: bool, feel: RumbleFeel, device_fallback_slot: i16) -> Self {
        Self {
            allowed,
            feel,
            device_fallback_slot,
            paused: false,
            levels: [None; MAX_GAMEPADS],
        }
    }

    pub fn on_rumble(
        &mut self,
        slot: i16,
        low: u16,
        high: u16,
        stopped: bool,
    ) -> RumbleRecordOutcome {
        if stopped || !self.allowed {
            return RumbleRecordOutcome {
                allowed_to_record: false,
                active: false,
            };
        }
        if let Some(idx) = usize::try_from(slot).ok().filter(|&i| i < MAX_GAMEPADS) {
            let entry = self.levels[idx].get_or_insert([0; 4]);
            entry[0] = low;
            entry[1] = high;
        }
        RumbleRecordOutcome {
            allowed_to_record: true,
            active: !self.paused,
        }
    }

    pub fn on_rumble_triggers(
        &mut self,
        slot: i16,
        lt: u16,
        rt: u16,
        stopped: bool,
    ) -> RumbleRecordOutcome {
        if stopped || !self.allowed {
            return RumbleRecordOutcome {
                allowed_to_record: false,
                active: false,
            };
        }
        if let Some(idx) = usize::try_from(slot).ok().filter(|&i| i < MAX_GAMEPADS) {
            let entry = self.levels[idx].get_or_insert([0; 4]);
            entry[2] = lt;
            entry[3] = rt;
        }
        RumbleRecordOutcome {
            allowed_to_record: true,
            active: !self.paused,
        }
    }

    pub fn set_paused(&mut self, paused: bool, stopped: bool) -> PauseTransition {
        if self.paused == paused || stopped {
            return PauseTransition::NoChange;
        }
        self.paused = paused;
        if paused {
            let mut slots = [0i16; MAX_GAMEPADS];
            let mut len = 0usize;
            for (idx, level) in self.levels.iter().enumerate() {
                if level.is_some() {
                    slots[len] = idx as i16;
                    len += 1;
                }
            }
            PauseTransition::Pause { slots, len }
        } else {
            let mut entries = [(0i16, 0u16, 0u16, 0u16, 0u16); MAX_GAMEPADS];
            let mut len = 0usize;
            for (idx, level) in self.levels.iter().enumerate() {
                if let Some(l) = level {
                    entries[len] = (idx as i16, l[0], l[1], l[2], l[3]);
                    len += 1;
                }
            }
            PauseTransition::Resume { entries, len }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn device_vibrator_route(
        &self,
        slot: i16,
        low: u16,
        high: u16,
        found_matching: bool,
        vibrated: bool,
        osc: bool,
        only_l3_r3: bool,
        vibrate_osc: bool,
        fallback_to_device: bool,
        fallback_strength: i32,
    ) -> DeviceVibratorAction {
        if slot != 0 && slot != self.device_fallback_slot {
            return DeviceVibratorAction::None;
        }
        if slot == 0 && !found_matching && osc && !only_l3_r3 && vibrate_osc {
            let plan = single_vibrator_plan(low, high, self.feel, 100);
            return DeviceVibratorAction::Osc {
                amplitude: plan.amplitude,
                pwm_on_ms: plan.pwm_on_ms,
                pwm_off_ms: plan.pwm_off_ms,
            };
        }
        if slot == self.device_fallback_slot && found_matching && !vibrated && fallback_to_device {
            let plan = single_vibrator_plan(low, high, self.feel, fallback_strength);
            return DeviceVibratorAction::PlayerOneFallback {
                amplitude: plan.amplitude,
                pwm_on_ms: plan.pwm_on_ms,
                pwm_off_ms: plan.pwm_off_ms,
            };
        }
        DeviceVibratorAction::None
    }

    pub fn on_slot_released(
        &mut self,
        slot: i16,
        assigned: bool,
        slot_still_assigned: bool,
        dev_vib_for_p1: bool,
    ) -> (bool, bool) {
        let cancel_dev_vib = dev_vib_for_p1 && assigned && slot == self.device_fallback_slot;
        let mut cleared_slot_levels = false;
        if assigned
            && !slot_still_assigned
            && let Some(idx) = usize::try_from(slot).ok().filter(|&i| i < MAX_GAMEPADS)
        {
            self.levels[idx] = None;
            cleared_slot_levels = true;
        }
        (cancel_dev_vib, cleared_slot_levels)
    }
}

/// `[high, low]` 0-255 amplitudes for a dual-vibrator controller (`None` -> cancel).
pub fn dual_vibrator_amplitudes(low: u16, high: u16, feel: RumbleFeel) -> Option<[u8; 2]> {
    let low_amp = (rumble::shape(low, feel) >> 8) as u8;
    let high_amp = (rumble::shape(high, feel) >> 8) as u8;
    if low_amp == 0 && high_amp == 0 {
        None
    } else {
        Some([high_amp, low_amp])
    }
}

/// `[high, low, lt, rt]` 0-255 amplitudes for a quad-vibrator controller (`None` -> cancel).
pub fn quad_vibrator_amplitudes(
    low: u16,
    high: u16,
    lt: u16,
    rt: u16,
    feel: RumbleFeel,
) -> Option<[u8; 4]> {
    let low_amp = (rumble::shape(low, feel) >> 8) as u8;
    let high_amp = (rumble::shape(high, feel) >> 8) as u8;
    let lt_amp = (rumble::shape(lt, feel) >> 8) as u8;
    let rt_amp = (rumble::shape(rt, feel) >> 8) as u8;
    if low_amp == 0 && high_amp == 0 && lt_amp == 0 && rt_amp == 0 {
        None
    } else {
        Some([high_amp, low_amp, lt_amp, rt_amp])
    }
}

/// Single-motor amplitude and PWM fallback timings.
pub fn single_vibrator_plan(
    low: u16,
    high: u16,
    feel: RumbleFeel,
    strength_percent: i32,
) -> SingleVibratorPlan {
    let amp = rumble::single_motor_amplitude(low, high, feel, strength_percent).clamp(0, 255) as u8;
    let (pwm_on_ms, pwm_off_ms) = rumble::pwm_timings(i32::from(amp), feel);
    SingleVibratorPlan {
        amplitude: amp,
        pwm_on_ms,
        pwm_off_ms,
    }
}

/// Tracks whether a controller's gyroscope reports real data or only exact zeros.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GyroLivenessTracker {
    pub zero_nanos: i64,
    pub zero_run_start_nanos: i64,
    pub saw_motion: bool,
    pub tripped: bool,
}

impl Default for GyroLivenessTracker {
    fn default() -> Self {
        Self::new(DEFAULT_ZERO_NANOS)
    }
}

impl GyroLivenessTracker {
    pub fn new(zero_nanos: i64) -> Self {
        Self {
            zero_nanos,
            zero_run_start_nanos: 0,
            saw_motion: false,
            tripped: false,
        }
    }

    pub fn on_sample(&mut self, x: f32, y: f32, z: f32, timestamp_nanos: i64) -> bool {
        if self.tripped || self.saw_motion {
            return false;
        }
        if x != 0.0 || y != 0.0 || z != 0.0 {
            self.saw_motion = true;
            return false;
        }
        if self.zero_run_start_nanos == 0 || timestamp_nanos < self.zero_run_start_nanos {
            self.zero_run_start_nanos = timestamp_nanos;
            return false;
        }
        if timestamp_nanos.saturating_sub(self.zero_run_start_nanos) < self.zero_nanos {
            return false;
        }
        self.tripped = true;
        true
    }
}

/// Whether player 1's controller gyro needs a liveness fallback check.
pub fn checks_gyro_liveness(
    slot: i16,
    device_fallback_slot: i16,
    using_device_sensors: bool,
    fallback_to_device: bool,
    has_device_gyro: bool,
) -> bool {
    slot == device_fallback_slot && !using_device_sensors && fallback_to_device && has_device_gyro
}

/// Restricts requested sensor rate to `<= 200 Hz` without `HIGH_SAMPLING_RATE_SENSORS`.
pub fn clamp_sensor_rate_hz(rate_hz: u16) -> u16 {
    rate_hz.min(MAX_ANDROID_SENSOR_HZ)
}

/// Converts a non-zero sensor rate in Hz into Android `samplingPeriodUs` (`0` if `rate_hz == 0`).
pub fn sampling_period_us(rate_hz: u16) -> i32 {
    if rate_hz == 0 {
        0
    } else {
        1_000_000 / i32::from(rate_hz)
    }
}

/// Effective motion report rate for a USB pad (`0` if disabled or dropped; clamped when using device sensors).
pub fn usb_motion_rate(
    report_rate_hz: u16,
    gamepad_motion_sensors: bool,
    drops_motion_and_touchpad: bool,
    uses_device_sensors: bool,
) -> u16 {
    let rate = if gamepad_motion_sensors && !drops_motion_and_touchpad {
        report_rate_hz
    } else {
        0
    };
    if uses_device_sensors {
        clamp_sensor_rate_hz(rate)
    } else {
        rate
    }
}

/// Remaps Android device sensor axes according to `Surface.ROTATION_{0,90,180,270}`.
pub fn apply_device_rotation(v: [f32; 3], rotation: i32) -> [f32; 3] {
    match rotation {
        0 => [v[0], v[2], -v[1]],
        1 => [-v[1], v[2], -v[0]],
        2 => [-v[0], v[2], v[1]],
        3 => [v[1], v[2], v[0]],
        _ => v,
    }
}

/// Validation failure kind for [`GyroMapping::validate`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GyroMappingError {
    InvalidSensitivity = 1,
    InvalidAxes = 2,
    InvalidSigns = 3,
}

/// Pure gyroscope axis/sign/sensitivity transform matching `GyroAxisTransformer`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GyroMapping {
    pub axes: [u8; 3],
    pub signs: [i8; 3],
    pub h_sens: f32,
    pub v_sens: f32,
    pub identity: bool,
}

impl Default for GyroMapping {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl GyroMapping {
    pub const IDENTITY: Self = Self {
        axes: [0, 1, 2],
        signs: [1, 1, 1],
        h_sens: 1.0,
        v_sens: 1.0,
        identity: true,
    };

    pub const FLYDIGI_TRANSFORM: Self = Self {
        axes: [1, 2, 0],
        signs: [-1, 1, 1],
        h_sens: 1.0,
        v_sens: 1.0,
        identity: false,
    };

    pub fn valid_sensitivity(value: f32) -> bool {
        value.is_finite() && (MIN_GYRO_SENSITIVITY..=MAX_GYRO_SENSITIVITY).contains(&value)
    }

    pub fn validate(
        axes: [i32; 3],
        signs: [i32; 3],
        h_sens: f32,
        v_sens: f32,
    ) -> Result<Self, GyroMappingError> {
        if !Self::valid_sensitivity(h_sens) || !Self::valid_sensitivity(v_sens) {
            return Err(GyroMappingError::InvalidSensitivity);
        }
        let mut seen = [false; 3];
        for axis in axes {
            if !(0..=2).contains(&axis) || seen[axis as usize] {
                return Err(GyroMappingError::InvalidAxes);
            }
            seen[axis as usize] = true;
        }
        for sign in signs {
            if sign != 1 && sign != -1 {
                return Err(GyroMappingError::InvalidSigns);
            }
        }
        let identity = axes == [0, 1, 2] && signs == [1, 1, 1] && h_sens == 1.0 && v_sens == 1.0;
        Ok(Self {
            axes: [axes[0] as u8, axes[1] as u8, axes[2] as u8],
            signs: [signs[0] as i8, signs[1] as i8, signs[2] as i8],
            h_sens,
            v_sens,
            identity,
        })
    }

    pub fn transform(&self, v: [f32; 3]) -> [f32; 3] {
        [
            v[self.axes[0] as usize] * f32::from(self.signs[0]) * self.v_sens,
            v[self.axes[1] as usize] * f32::from(self.signs[1]) * self.h_sens,
            v[self.axes[2] as usize] * f32::from(self.signs[2]),
        ]
    }
}

/// Outcome of one [`MotionFilter::on_sample`] call.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionSampleOutcome {
    pub tripped_liveness: bool,
    pub send: Option<[f32; 3]>,
}

/// Combined per-sensor filter: liveness check -> duplicate suppression -> rotation -> gyro scaling/mapping.
#[derive(Clone, Debug)]
pub struct MotionFilter {
    motion_type: u8,
    needs_device_orientation: bool,
    liveness: Option<GyroLivenessTracker>,
    last_values: [f32; 3],
    has_mapping: bool,
    mapping: GyroMapping,
}

impl MotionFilter {
    pub fn new(
        motion_type: u8,
        needs_device_orientation: bool,
        check_liveness: bool,
        has_mapping: bool,
        mapping: GyroMapping,
    ) -> Self {
        Self {
            motion_type,
            needs_device_orientation,
            liveness: check_liveness.then(GyroLivenessTracker::default),
            last_values: [0.0; 3],
            has_mapping,
            mapping,
        }
    }

    pub fn on_sample(
        &mut self,
        v: [f32; 3],
        ts_ns: i64,
        display_rotation: i32,
        flydigi_forced: bool,
    ) -> MotionSampleOutcome {
        let tripped_liveness = self
            .liveness
            .as_mut()
            .is_some_and(|l| l.on_sample(v[0], v[1], v[2], ts_ns));

        if v[0] == self.last_values[0] && v[1] == self.last_values[1] && v[2] == self.last_values[2]
        {
            return MotionSampleOutcome {
                tripped_liveness,
                send: None,
            };
        }
        self.last_values = v;

        let rotated = if self.needs_device_orientation {
            apply_device_rotation(v, display_rotation)
        } else {
            v
        };

        if self.motion_type == 2 {
            let mut g = [
                rotated[0] * RAD_TO_DEG,
                rotated[1] * RAD_TO_DEG,
                rotated[2] * RAD_TO_DEG,
            ];
            if self.has_mapping {
                let map = if flydigi_forced {
                    GyroMapping::FLYDIGI_TRANSFORM
                } else {
                    self.mapping
                };
                if !map.identity {
                    g = map.transform(g);
                }
            }
            MotionSampleOutcome {
                tripped_liveness,
                send: Some(g),
            }
        } else {
            MotionSampleOutcome {
                tripped_liveness,
                send: Some(rotated),
            }
        }
    }
}

/// Compares battery capacities treating two `NaN` values as equal.
pub fn are_battery_capacities_equal(first: f32, second: f32) -> bool {
    if !first.is_nan() && !second.is_nan() {
        first == second
    } else {
        first.is_nan() == second.is_nan()
    }
}

/// Maps `android.hardware.BatteryState.STATUS_*` (`1..=5`) to `LI_BATTERY_STATE_*`.
pub fn map_battery_status(android_status: i32) -> Option<u8> {
    match android_status {
        1 => Some(0), // STATUS_UNKNOWN -> LI_BATTERY_STATE_UNKNOWN
        2 => Some(3), // STATUS_CHARGING -> LI_BATTERY_STATE_CHARGING
        3 => Some(2), // STATUS_DISCHARGING -> LI_BATTERY_STATE_DISCHARGING
        4 => Some(4), // STATUS_NOT_CHARGING -> LI_BATTERY_STATE_NOT_CHARGING
        5 => Some(5), // STATUS_FULL -> LI_BATTERY_STATE_FULL
        _ => None,
    }
}

/// Converts normalized capacity (`0.0..=1.0`, or `NaN`) to wire percentage byte (`0..=100`, `0xFF` for `NaN`).
pub fn battery_percentage_byte(capacity: f32) -> u8 {
    if capacity.is_nan() {
        0xFF
    } else {
        (capacity * 100.0).clamp(0.0, 100.0) as u8
    }
}

/// Maps SHIELD `SceConnectionType` (`0=UNKNOWN, 1=WIRED, 2=WIRELESS, 3=BOTH`) and
/// `SceChargingState` (`0=UNKNOWN, 1=NOT_CHARGING, 2=CHARGING`) into `(BatteryState.STATUS_*, capacity)`.
pub fn sce_battery_reading(
    percentage: i32,
    connection_type: i32,
    charging_state: i32,
) -> (i32, f32) {
    let capacity = if percentage < 0 {
        f32::NAN
    } else {
        (percentage as f32) / 100.0
    };
    let status = match connection_type {
        1 | 3 => {
            if percentage == 100 {
                5 // STATUS_FULL
            } else if charging_state == 1 {
                4 // STATUS_NOT_CHARGING
            } else {
                2 // STATUS_CHARGING
            }
        }
        2 => {
            if charging_state == 2 {
                2 // STATUS_CHARGING
            } else {
                3 // STATUS_DISCHARGING
            }
        }
        _ => {
            if percentage == 100 {
                5 // STATUS_FULL
            } else if charging_state == 1 {
                3 // STATUS_DISCHARGING
            } else if charging_state == 2 {
                2 // STATUS_CHARGING
            } else {
                1 // STATUS_UNKNOWN
            }
        }
    };
    (status, capacity)
}

/// Evaluates whether a controller battery packet should be sent, returning `(li_state, li_percentage_byte)`.
pub fn evaluate_battery_update(
    send_controllers: bool,
    status: i32,
    capacity: f32,
    last_status: i32,
    last_capacity: f32,
) -> Option<(u8, u8)> {
    if !send_controllers {
        return None;
    }
    if status == last_status && are_battery_capacities_equal(capacity, last_capacity) {
        return None;
    }
    let li_state = map_battery_status(status)?;
    Some((li_state, battery_percentage_byte(capacity)))
}

#[cfg(test)]
#[path = "haptics_sensor_tests.rs"]
mod tests;
