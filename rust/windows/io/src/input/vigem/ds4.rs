// The DualShock 4 report, its keepalive, the stale gyro and the lightbar
// follow Vibepollo's Windows input code (Nonary/Vibepollo, GPL-3.0), which
// carries LizardByte Sunshine's (GPL-3.0).
//! DualShock 4 extended report encoding and keepalive state for ViGEmBus targets.

use std::time::{Duration, Instant};

/// A DualShock 4 report goes out at least this often, so its 16-bit
/// timestamp keeps advancing: some games take DS4 input only while it does
/// (Vibepollo's `DS4_KEEPALIVE_INTERVAL`).
pub(super) const DS4_KEEPALIVE: Duration = Duration::from_millis(100);
/// A gyro reading older than this is stilled, so a client that stopped
/// sending motion doesn't leave the camera turning.
const DS4_GYRO_STALE: Duration = Duration::from_millis(750);
/// After this long without a gyro reading the client is asked for motion
/// again, at most once per [`DS4_MOTION_REQUEST_COOLDOWN`].
const DS4_MOTION_REQUEST_AFTER: Duration = Duration::from_secs(2);
const DS4_MOTION_REQUEST_COOLDOWN: Duration = Duration::from_secs(5);

/// DS4_REPORT_EX: sticks, buttons, triggers, timestamp, battery, gyro,
/// accelerometer and two touch contacts, with Vibepollo's conversions.
#[derive(Clone)]
pub(super) struct Ds4Report(pub(super) [u8; 63]);
impl Ds4Report {
    /// At rest, as ViGEmBus starts a DS4 (Ds4Pdo.cpp), held flat.
    pub(super) fn new() -> Self {
        let mut report = Self([0; 63]);
        report.0[..4].fill(0x80);
        report.0[4] = 8;
        report.0[11] = 0xff;
        report.0[29] = 0x1a;
        report.0[32] = 1;
        for offset in [34, 38, 43, 47, 52, 56] {
            report.0[offset] = 0x80;
        }
        report.motion(1, &[0., 9.80665, 0.]);
        report.motion(2, &[0.; 3]);
        report
    }
    pub(super) fn state(&mut self, flags: u32, left: u8, right: u8, sticks: &[i16; 4]) {
        let mut buttons = match (
            flags & 1 != 0,
            flags & 2 != 0,
            flags & 8 != 0,
            flags & 4 != 0,
        ) {
            (true, _, true, _) => 1,
            (true, _, false, true) => 7,
            (true, _, false, false) => 0,
            (_, true, true, _) => 3,
            (_, true, false, true) => 5,
            (_, true, false, false) => 4,
            (_, _, true, _) => 2,
            (_, _, _, true) => 6,
            _ => 8,
        };
        for (source, target) in [
            (0x40, 0x4000),
            (0x80, 0x8000),
            (0x100, 0x100),
            (0x200, 0x200),
            (0x10, 0x2000),
            (0x20, 0x1000),
            (0x1000, 0x20),
            (0x2000, 0x40),
            (0x4000, 0x10),
            (0x8000, 0x80),
        ] {
            if flags & source != 0 {
                buttons |= target;
            }
        }
        if left != 0 {
            buttons |= 0x400;
        }
        if right != 0 {
            buttons |= 0x800;
        }
        self.0[4..6].copy_from_slice(&(buttons as u16).to_le_bytes());
        // PS from Guide; the touchpad click from a touchpad or Share button.
        self.0[6] = u8::from(flags & 0x400 != 0) | (u8::from(flags & 0x300000 != 0) << 1);
        self.0[7..9].copy_from_slice(&[left, right]);
        for (i, stick) in sticks.iter().enumerate() {
            // Vibepollo's axis rounding, with its Y-axis inversion.
            self.0[i] = if i % 2 == 0 {
                ((i32::from(*stick) + 32768) / 257) as u8
            } else {
                let value = -(32766 + i32::from(*stick)) / 257;
                if value == 0 { 255 } else { value as u8 }
            };
        }
    }
    /// Moonlight motion: kind 1 the accelerometer in m/s², kind 2 the gyro in °/s.
    pub(super) fn motion(&mut self, kind: u8, xyz: &[f32; 3]) {
        let (offset, limit, calibration) = match kind {
            1 => (
                18,
                200.,
                [(-297., 1.010796), (-42., 1.014614), (-512., 1.024768)],
            ),
            2 => (12, 4000., [(1., 0.977596), (0., 0.972370), (0., 0.971550)]),
            _ => return,
        };
        for (axis, (&value, (bias, scale))) in xyz.iter().zip(calibration).enumerate() {
            let value = if value.is_finite() {
                value.clamp(-limit, limit)
            } else {
                0.
            };
            let raw = if kind == 1 {
                value / 9.80665 * 8192.
            } else {
                value * 16.
            } as i32;
            // Games undo ViGEmBus's fixed factory calibration; apply its inverse.
            let calibrated = ((raw as f32 + bias) / scale) as i32;
            let value = calibrated.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            let offset = offset + axis * 2;
            self.0[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        }
    }
    /// A contact (0 or 1) as the gamepads' touch mapping hands it over: event
    /// 1 down, 2 up, 3 move, 4 cancel, 5 cancel all; position in 0..1.
    pub(super) fn touch(&mut self, slot: u8, event: u8, position: [f32; 2]) {
        let offset = 34 + usize::from(slot.min(1)) * 4;
        match event {
            // A new touch gets the next 7-bit tracking number.
            1 => {
                if self.0[offset] & 0x80 != 0 {
                    self.0[offset] = self.0[offset].wrapping_add(1) & 0x7f;
                }
            }
            2 | 4 => self.0[offset] |= 0x80,
            3 => {}
            5 => {
                self.0[34] |= 0x80;
                self.0[38] |= 0x80;
            }
            _ => return,
        }
        if event != 5 {
            // ViGEmBus's touchpad is 1920 x 943.
            let x = (position[0].clamp(0., 1.) * 1920.) as u16;
            let y = (position[1].clamp(0., 1.) * 943.) as u16;
            self.0[offset + 1..offset + 4].copy_from_slice(&[
                x as u8,
                ((x >> 8) | ((y & 15) << 4)) as u8,
                (y >> 4) as u8,
            ]);
        }
        self.0[33] = self.0[33].wrapping_add(1);
    }
    /// Moonlight battery state (`1=NOT_PRESENT`, `2=DISCHARGING`, `3=CHARGING`,
    /// `4=NOT_CHARGING`, `5=FULL`) and percentage, in the DS4's battery fields
    /// (hid-playstation.c).
    pub(super) fn battery(&mut self, state: u8, percent: u8) {
        match state {
            2..=4 => {
                if state == 2 {
                    self.0[29] &= !0x10;
                } else {
                    self.0[29] |= 0x10;
                }
                if self.0[29] & 15 > 10 {
                    self.0[29] = (self.0[29] & !15) | 5;
                }
            }
            5 => {
                self.0[29] = 0x1b;
                self.0[11] = 255;
            }
            1 => self.0[29] = 0x1f,
            _ => {}
        }
        if percent != 255 {
            let percent = percent.min(100);
            self.0[11] = (u16::from(percent) * 255 / 100) as u8;
            if self.0[29] & 15 <= 10 {
                self.0[29] = (self.0[29] & !15) | ((percent + 5) / 10);
            }
        }
    }
    /// The timestamp counts 5.333 µs units.
    pub(super) fn advance_timestamp(&mut self, elapsed: Duration) {
        let previous = u16::from_le_bytes([self.0[9], self.0[10]]);
        self.0[9..11].copy_from_slice(
            &previous
                .wrapping_add((elapsed.as_nanos() / 5333) as u16)
                .to_le_bytes(),
        );
    }
}

/// A DualShock 4 target's report and when its parts last changed.
pub(super) struct Ds4 {
    pub(super) report: Ds4Report,
    pub(super) sent: Instant,
    pub(super) gyro: Instant,
    motion_requested: Instant,
}
impl Ds4 {
    pub(super) fn new(now: Instant) -> Self {
        Self {
            report: Ds4Report::new(),
            sent: now,
            gyro: now,
            motion_requested: now,
        }
    }
    /// DS4_SUBMIT_REPORT_EX, built on the stack: the report with a stale gyro
    /// stilled and the timestamp advanced to `now`.
    pub(super) fn packet(&mut self, serial: u32, now: Instant) -> [u8; 71] {
        if now.saturating_duration_since(self.gyro) > DS4_GYRO_STALE {
            self.report.motion(2, &[0.; 3]);
        }
        self.report
            .advance_timestamp(now.saturating_duration_since(self.sent));
        self.sent = now;
        let mut data = [0; 71];
        data[..4].copy_from_slice(&71u32.to_le_bytes());
        data[4..8].copy_from_slice(&serial.to_le_bytes());
        data[8..].copy_from_slice(&self.report.0);
        data
    }
    /// Whether to ask the client for motion again, as Vibepollo does when the
    /// gyro has been silent: at most once per cooldown.
    pub(super) fn wants_motion(&mut self, now: Instant) -> bool {
        let due = now.saturating_duration_since(self.gyro) > DS4_MOTION_REQUEST_AFTER
            && now.saturating_duration_since(self.motion_requested) > DS4_MOTION_REQUEST_COOLDOWN;
        if due {
            self.motion_requested = now;
        }
        due
    }
}
