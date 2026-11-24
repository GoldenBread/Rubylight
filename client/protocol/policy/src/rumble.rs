//! Rumble feel: a response curve for the host's 16-bit motor levels, so small handheld motors
//! feel right. Weak levels fall under a small motor's start threshold and do nothing; strong ones
//! saturate harshly. A feel reshapes a level before it is scaled or sent. Zero stays zero.
//! Android's `RumblePolicy.shapeMotor` is its Java port; both check the same table (`VECTORS`).

use serde::{Deserialize, Serialize};

/// Which curve the host's motor levels pass through.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RumbleFeel {
    /// The host's levels as they are.
    #[default]
    Standard,
    /// Weak levels are lifted above a small motor's start threshold.
    Strong,
    /// Strong levels are capped and weak ones softened.
    Subtle,
}

impl RumbleFeel {
    pub const ALL: [Self; 3] = [Self::Standard, Self::Strong, Self::Subtle];

    /// The feel stored under this key; anything unknown is `Standard`.
    pub fn from_key(key: Option<&str>) -> Self {
        match key {
            Some("strong") => Self::Strong,
            Some("subtle") => Self::Subtle,
            _ => Self::Standard,
        }
    }

    /// Maps an integer ordinal (`0=Standard, 1=Strong, 2=Subtle`), falling back to `Standard`.
    pub fn from_ordinal(ordinal: i32) -> Self {
        match ordinal {
            1 => Self::Strong,
            2 => Self::Subtle,
            _ => Self::Standard,
        }
    }

    /// Integer ordinal matching `RumblePolicy.Feel.ordinal()`.
    pub fn ordinal(self) -> i32 {
        match self {
            Self::Standard => 0,
            Self::Strong => 1,
            Self::Subtle => 2,
        }
    }
}

/// The PWM period a device without amplitude control is pulsed over, in milliseconds.
pub const PWM_PERIOD_MS: i64 = 20;

/// Whether host vibration requests may reach any motor (`!rumble_disabled`).
pub fn allows(rumble_disabled: bool) -> bool {
    !rumble_disabled
}

/// Strong: levels under this share of full strength stay silent (a motor cannot show them, and
/// lifting them would buzz on noise).
const STRONG_DEAD: f64 = 0.02;
/// Strong: the share of full strength every other level starts from.
const STRONG_FLOOR: f64 = 0.25;
const STRONG_GAMMA: f64 = 0.7;
/// Subtle: the share of full strength the strongest level reaches.
const SUBTLE_CAP: f64 = 0.6;
const SUBTLE_GAMMA: f64 = 1.5;

/// A motor level (0-65535) after the feel's curve.
pub fn shape(level: u16, feel: RumbleFeel) -> u16 {
    if level == 0 || feel == RumbleFeel::Standard {
        return level;
    }
    let f = f64::from(level) / 65535.0;
    let y = match feel {
        RumbleFeel::Standard => f,
        RumbleFeel::Strong if f < STRONG_DEAD => 0.0,
        RumbleFeel::Strong => STRONG_FLOOR + (1.0 - STRONG_FLOOR) * f.powf(STRONG_GAMMA),
        RumbleFeel::Subtle => SUBTLE_CAP * f.powf(SUBTLE_GAMMA),
    };
    (y * 65535.0).round().clamp(0.0, 65535.0) as u16
}

/// A 0-255 vibration amplitude after the feel's curve.
pub fn shape_amplitude(amplitude: i32, feel: RumbleFeel) -> i32 {
    if amplitude <= 0 {
        return 0;
    }
    let clamped = amplitude.min(255);
    if feel == RumbleFeel::Standard {
        return clamped;
    }
    let level = (clamped as u32 * 257) as u16;
    i32::from(shape(level, feel) >> 8).min(255)
}

/// Scales an unsigned 16-bit motor level by a percentage, capped at `0xFFFE` (`Short.MAX_VALUE * 2`).
pub fn scale_motor(motor: u16, strength_percent: i32) -> u16 {
    let pct = u64::from(strength_percent.max(0) as u32);
    let scaled = (u64::from(motor) * pct) / 100;
    scaled.min(0xFFFE) as u16
}

/// The single 0-255 amplitude for a device with one motor: scales each motor, mixes 80% of low +
/// 33% of high, and applies the feel curve.
pub fn single_motor_amplitude(
    low_freq_motor: u16,
    high_freq_motor: u16,
    feel: RumbleFeel,
    strength_percent: i32,
) -> i32 {
    let low = i32::from(scale_motor(low_freq_motor, strength_percent) >> 8);
    let high = i32::from(scale_motor(high_freq_motor, strength_percent) >> 8);
    let mixed = ((f64::from(low) * 0.80) + (f64::from(high) * 0.33)) as i32;
    shape_amplitude(mixed.min(255), feel)
}

/// `{on, off}` milliseconds for a vibrator without amplitude control.
pub fn pwm_timings(amplitude: i32, feel: RumbleFeel) -> (i64, i64) {
    let clamped = amplitude.clamp(0, 255);
    let mut on = ((f64::from(clamped) / 255.0) * (PWM_PERIOD_MS as f64)) as i64;
    if feel != RumbleFeel::Standard && clamped > 0 {
        on = on.max(1);
    }
    (on, PWM_PERIOD_MS - on)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (feel, in, out): the same table is in Android's `RumblePolicyTest`.
    const VECTORS: [(RumbleFeel, u16, u16); 12] = [
        (RumbleFeel::Standard, 0, 0),
        (RumbleFeel::Standard, 1234, 1234),
        (RumbleFeel::Standard, 65535, 65535),
        (RumbleFeel::Strong, 0, 0),
        (RumbleFeel::Strong, 1000, 0),
        (RumbleFeel::Strong, 6554, 26_191),
        (RumbleFeel::Strong, 32768, 46_640),
        (RumbleFeel::Strong, 65535, 65_535),
        (RumbleFeel::Subtle, 0, 0),
        (RumbleFeel::Subtle, 6554, 1_244),
        (RumbleFeel::Subtle, 32768, 13_902),
        (RumbleFeel::Subtle, 65535, 39_321),
    ];

    #[test]
    fn the_curves_match_the_shared_table() {
        for (feel, input, want) in VECTORS {
            assert_eq!(shape(input, feel), want, "{feel:?} {input}");
        }
    }

    #[test]
    fn zero_stays_zero_and_levels_stay_in_order() {
        for feel in RumbleFeel::ALL {
            assert_eq!(shape(0, feel), 0);
            let mut last = 0;
            for level in (0..=65535u32).step_by(257) {
                let out = shape(level as u16, feel);
                assert!(out >= last, "{feel:?} falls at {level}");
                last = out;
            }
        }
    }

    #[test]
    fn strong_lifts_what_standard_leaves_and_subtle_never_exceeds_its_cap() {
        for level in (1311..=65535u32).step_by(101) {
            assert!(shape(level as u16, RumbleFeel::Strong) >= level as u16);
        }
        for level in (0..=65535u32).step_by(101) {
            assert!(shape(level as u16, RumbleFeel::Subtle) <= 39_321);
        }
    }

    #[test]
    fn a_stored_feel_reads_by_its_snake_case_name() {
        assert_eq!(
            serde_json::from_str::<RumbleFeel>("\"strong\"").unwrap(),
            RumbleFeel::Strong
        );
        assert_eq!(RumbleFeel::default(), RumbleFeel::Standard);
    }

    #[test]
    fn allows_from_key_and_ordinal_roundtrip() {
        assert!(allows(false));
        assert!(!allows(true));
        for feel in RumbleFeel::ALL {
            let key = match feel {
                RumbleFeel::Standard => "standard",
                RumbleFeel::Strong => "strong",
                RumbleFeel::Subtle => "subtle",
            };
            assert_eq!(RumbleFeel::from_key(Some(key)), feel);
            assert_eq!(RumbleFeel::from_ordinal(feel.ordinal()), feel);
        }
        assert_eq!(RumbleFeel::from_key(None), RumbleFeel::Standard);
        assert_eq!(RumbleFeel::from_key(Some("loud")), RumbleFeel::Standard);
        assert_eq!(RumbleFeel::from_ordinal(-1), RumbleFeel::Standard);
        assert_eq!(RumbleFeel::from_ordinal(99), RumbleFeel::Standard);
    }

    #[test]
    fn scale_motor_caps_at_0xfffe_and_clamps_negative_strength() {
        assert_eq!(scale_motor(0xFFFE, 50), 0x7FFF);
        assert_eq!(scale_motor(0xFFFF, 200), 0xFFFE);
        assert_eq!(scale_motor(0x1000, 200), 0x2000);
        assert_eq!(scale_motor(0xFFFF, 0), 0);
        assert_eq!(scale_motor(0xFFFF, -10), 0);
    }

    #[test]
    fn single_motor_amplitude_matches_java_mix_and_feel_order() {
        let levels = [
            (0xFF00u16, 0u16),
            (0, 0xFFFF),
            (0x8000, 0x4000),
            (0xFFFF, 0xFFFF),
            (0x0180, 0x0100),
            (0x1234, 0xABCD),
        ];
        for strength in 1..=200 {
            for (l0, l1) in levels {
                let low = i32::from(scale_motor(l0, strength) >> 8);
                let high = i32::from(scale_motor(l1, strength) >> 8);
                let old = (((f64::from(low) * 0.80) + (f64::from(high) * 0.33)) as i32).min(255);
                assert_eq!(
                    single_motor_amplitude(l0, l1, RumbleFeel::Standard, strength),
                    old,
                    "strength {strength}"
                );
            }
        }
        assert_eq!(
            single_motor_amplitude(0xFF00, 0, RumbleFeel::Standard, 100),
            204
        );
        assert_eq!(
            single_motor_amplitude(0xFFFF, 0xFFFF, RumbleFeel::Standard, 0),
            0
        );

        let standard = single_motor_amplitude(0x3000, 0, RumbleFeel::Standard, 50);
        let strong = single_motor_amplitude(0x3000, 0, RumbleFeel::Strong, 50);
        assert!(standard > 0 && standard < 64);
        assert!(strong >= 64);
    }

    #[test]
    fn shape_amplitude_and_pwm_timings_match_android_policy() {
        for feel in RumbleFeel::ALL {
            assert_eq!(shape_amplitude(0, feel), 0);
        }
        for amp in 0..=255 {
            assert_eq!(shape_amplitude(amp, RumbleFeel::Standard), amp);
        }
        assert!(shape_amplitude(20, RumbleFeel::Strong) >= 64);
        assert_eq!(shape_amplitude(255, RumbleFeel::Strong), 255);
        assert_eq!(shape_amplitude(256, RumbleFeel::Strong), 255);
        assert_eq!(shape_amplitude(300, RumbleFeel::Strong), 255);
        assert_eq!(shape_amplitude(300, RumbleFeel::Standard), 255);
        assert_eq!(shape_amplitude(-10, RumbleFeel::Standard), 0);
        assert_eq!(shape_amplitude(-10, RumbleFeel::Strong), 0);
        assert!(shape_amplitude(255, RumbleFeel::Subtle) <= 153);
        assert_eq!(
            shape_amplitude(300, RumbleFeel::Subtle),
            shape_amplitude(255, RumbleFeel::Subtle)
        );

        assert_eq!(pwm_timings(128, RumbleFeel::Standard), (10, 10));
        assert_eq!(pwm_timings(255, RumbleFeel::Standard), (20, 0));
        assert_eq!(pwm_timings(300, RumbleFeel::Standard), (20, 0));
        assert_eq!(pwm_timings(-10, RumbleFeel::Standard), (0, 20));
        assert_eq!(pwm_timings(-255, RumbleFeel::Strong), (0, 20));
        assert_eq!(pwm_timings(5, RumbleFeel::Standard).0, 0);
        assert_eq!(pwm_timings(5, RumbleFeel::Strong).0, 1);
        assert_eq!(pwm_timings(0, RumbleFeel::Strong).0, 0);
        let weak = single_motor_amplitude(0x1800, 0, RumbleFeel::Strong, 100);
        let plain = single_motor_amplitude(0x1800, 0, RumbleFeel::Standard, 100);
        assert!(
            pwm_timings(weak, RumbleFeel::Strong).0 > pwm_timings(plain, RumbleFeel::Standard).0
        );
    }
}
