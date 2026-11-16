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
        assert_eq!(serde_json::from_str::<RumbleFeel>("\"strong\"").unwrap(), RumbleFeel::Strong);
        assert_eq!(RumbleFeel::default(), RumbleFeel::Standard);
    }
}
