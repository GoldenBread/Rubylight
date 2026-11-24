//! Night mode: a linked feed-forward compressor, a slow leveller and a soft
//! clipper. No lookahead: each output frame is the input frame times a gain
//! computed from it and earlier frames, so it adds no samples of delay.

const RATE: f32 = 48_000.;
/// Compressor threshold, ratio and knee width.
const THRESHOLD_DB: f32 = -30.;
const RATIO: f32 = 3.;
const KNEE_DB: f32 = 6.;
/// The peak detector falls this fast between peaks, so a waveform's troughs
/// don't read as quiet; the gain then moves with the attack and release.
const DETECT_SECONDS: f32 = 0.05;
const ATTACK_SECONDS: f32 = 0.003;
const RELEASE_SECONDS: f32 = 0.25;
/// The leveller's target, its 3 s RMS, range, slew and hold.
const TARGET_DB: f32 = -23.;
const RMS_SECONDS: f32 = 3.;
const LEVEL_MIN_DB: f32 = -6.;
const LEVEL_MAX_DB: f32 = 12.;
const SLEW_DB_PER_SECOND: f32 = 3.;
const HOLD_BELOW_DB: f32 = -60.;
/// The slow 3 s RMS leveller updates every 8 active frames (6 kHz at 48 kHz),
/// cutting its log/exp work by 8x while staying well within 0.05 dB of per-sample updates.
const LEVELLER_DECIM: u8 = 8;
/// `20 * log10(2)` and `10 * log10(2)` and `log2(10) / 20` for dB <-> linear conversions.
const DB20_PER_LOG2: f32 = 20. * std::f32::consts::LOG10_2;
const DB10_PER_LOG2: f32 = 10. * std::f32::consts::LOG10_2;
const LOG2_PER_DB20: f32 = std::f32::consts::LOG2_10 / 20.;
/// Turning night mode off ramps the gain back to unity over 10 ms.
const OFF_RAMP_FRAMES: u32 = 480;
/// The soft clipper is unity up to -3 dBFS and never reaches 0.98.
pub(crate) const CLIP_KNEE: f32 = 0.707_945_8;
pub(crate) const CLIP_CEILING: f32 = 0.98;

/// Values this small become 0, so neither samples nor decaying state reach
/// the subnormals, which are slow on x86.
pub(crate) const FLUSH_BELOW: f32 = 1e-30;

pub(crate) fn flush(x: f32) -> f32 {
    if x.abs() < FLUSH_BELOW { 0. } else { x }
}

/// The LFE of a 5.1 or 7.1 frame, which the leveller doesn't measure.
const LFE: usize = 3;

/// A frame's power for the leveller, BS.1770-style: without the LFE and
/// normalized to a stereo pair, so the same programme reads the same level in
/// 2, 6 or 8 channels.
fn power(frame: &[f32]) -> f32 {
    let square = |samples: &[f32]| samples.iter().map(|x| x * x).sum::<f32>();
    let sum = if frame.len() > LFE + 1 {
        square(&frame[..LFE]) + square(&frame[LFE + 1..])
    } else {
        square(frame)
    };
    sum / 2.
}

/// The off ramp's gain from `from` with `left` frames to go.
fn ramp(from: f32, left: u32) -> f32 {
    1. + (from - 1.) * (left as f32 / OFF_RAMP_FRAMES as f32)
}

fn coefficient(seconds: f32) -> f32 {
    (-1. / (seconds * RATE)).exp()
}

/// Fast IEEE-754 base-2 logarithm for positive normal `f32` values.
/// Range-reduces `x = 2^e * m` with `m in [sqrt(2)/2, sqrt(2))` and evaluates
/// `log2(m) = (2 / ln 2) * (t + t^3/3 + t^5/5)` where `t = (m - 1)/(m + 1)`
/// (`|t| <= 0.1716`): max error < 2e-6 (~0.00001 dB), exact at powers of two.
#[inline]
pub(crate) fn fast_log2(x: f32) -> f32 {
    let bits = x.to_bits();
    let exp = bits.wrapping_sub(0x3f35_04f3) as i32 >> 23;
    let mant = f32::from_bits(bits.wrapping_sub((exp.wrapping_shl(23)) as u32));
    let t = (mant - 1.0) / (mant + 1.0);
    let t2 = t * t;
    exp as f32 + t * (2.885_39 + t2 * (0.961_796_7 + t2 * 0.577_078))
}

/// Fast IEEE-754 base-2 exponential for `x in [-126, 126]`.
/// Splits `x = n + f` (`|f| <= 0.5`) and evaluates `2^f` via a degree-5 Horner
/// polynomial: max relative error < 3e-6 (~0.00003 dB), exact at integers.
#[inline]
pub(crate) fn fast_exp2(x: f32) -> f32 {
    let x = x.clamp(-126.0, 126.0);
    let n = x.round_ties_even();
    let f = x - n;
    let pow2_n = f32::from_bits(((n as i32 + 127) as u32) << 23);
    let poly = 1.0
        + f * (std::f32::consts::LN_2
            + f * (0.240_226_5 + f * (0.055_504_11 + f * (0.009_618_129 + f * 0.001_333_355_8))));
    pow2_n * poly
}

/// The compressor's static gain, in dB (0 or less), for a peak level in dBFS.
pub(crate) fn curve(level_db: f32) -> f32 {
    let over = level_db - THRESHOLD_DB;
    let slope = 1. / RATIO - 1.;
    if 2. * over < -KNEE_DB {
        0.
    } else if 2. * over <= KNEE_DB {
        slope * (over + KNEE_DB / 2.).powi(2) / (2. * KNEE_DB)
    } else {
        slope * over
    }
}

/// Unity below -3 dBFS, then a smooth approach to 0.98 with a continuous slope.
pub(crate) fn soft_clip(x: f32) -> f32 {
    let magnitude = x.abs();
    if magnitude <= CLIP_KNEE {
        return x;
    }
    let room = CLIP_CEILING - CLIP_KNEE;
    (CLIP_KNEE + room * ((magnitude - CLIP_KNEE) / room).tanh()).copysign(x)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    Off,
    On,
    /// Going back to unity from `from`, with `left` frames to go.
    Ending {
        from: f32,
        left: u32,
    },
}

pub struct Night {
    state: State,
    detector: f32,
    compressor_db: f32,
    mean_square: f32,
    leveller_db: f32,
    /// Cached linear leveller gain `10^(leveller_db / 20)`, updated every `LEVELLER_DECIM` active frames.
    leveller_lin: f32,
    /// Accumulated compressed power over the current decimation block.
    leveller_accum_power: f32,
    /// Active frames accumulated in the current decimation block (`0..LEVELLER_DECIM`).
    leveller_accum_count: u8,
    bad_samples: bool,
    detect: f32,
    attack: f32,
    release: f32,
    rms_decim: f32,
}

impl Default for Night {
    fn default() -> Self {
        Self {
            state: State::Off,
            detector: 0.,
            compressor_db: 0.,
            mean_square: 0.,
            leveller_db: 0.,
            leveller_lin: 1.,
            leveller_accum_power: 0.,
            leveller_accum_count: 0,
            bad_samples: false,
            detect: coefficient(DETECT_SECONDS),
            attack: coefficient(ATTACK_SECONDS),
            release: coefficient(RELEASE_SECONDS),
            rms_decim: (-(LEVELLER_DECIM as f32) / (RMS_SECONDS * RATE)).exp(),
        }
    }
}

impl Night {
    /// Processes interleaved `channels` in place and returns the frames it
    /// gained and clipped: all of them, or fewer when the off ramp ends within
    /// the packet. Off and fully ramped down, it leaves the samples untouched.
    pub fn process(&mut self, samples: &mut [f32], channels: usize, enabled: bool) -> usize {
        match (self.state, enabled) {
            (State::On, true) | (State::Off, false) => {}
            (State::Off, true) => self.start(),
            (State::Ending { from, left }, true) => self.resume(from, left),
            (State::On, false) => {
                self.state = State::Ending {
                    from: self.gain(),
                    left: OFF_RAMP_FRAMES,
                }
            }
            (State::Ending { .. }, false) => {}
        }
        let mut clipped = 0;
        for frame in samples.chunks_exact_mut(channels) {
            if self.state == State::Off {
                break;
            }
            for sample in frame.iter_mut() {
                if !sample.is_finite() {
                    *sample = 0.;
                    self.bad_samples = true;
                } else {
                    *sample = flush(*sample);
                }
            }
            let gain = if let State::Ending { from, left } = self.state {
                let gain = ramp(from, left);
                self.state = match left - 1 {
                    0 => State::Off,
                    left => State::Ending { from, left },
                };
                gain
            } else {
                self.track(frame)
            };
            for sample in frame.iter_mut() {
                *sample = soft_clip(*sample * gain);
            }
            clipped += 1;
        }
        clipped
    }

    /// Whether NaN or infinite samples were replaced since the last call.
    pub fn take_bad_samples(&mut self) -> bool {
        std::mem::take(&mut self.bad_samples)
    }

    fn start(&mut self) {
        self.state = State::On;
        self.detector = 0.;
        self.compressor_db = 0.;
        self.leveller_db = 0.;
        self.leveller_lin = 1.;
        self.leveller_accum_power = 0.;
        self.leveller_accum_count = 0;
        // As if the programme were already at the target: the leveller starts
        // from unity and only moves once it has measured something else.
        self.mean_square = 10f32.powf(TARGET_DB / 10.);
    }

    /// Back on during the off ramp: from the ramp's gain where it is, with the
    /// measurements kept. The compressor and leveller keep their share of it.
    fn resume(&mut self, from: f32, left: u32) {
        self.state = State::On;
        let total_db = self.compressor_db + self.leveller_db;
        let share = if total_db == 0. {
            0.
        } else {
            20. * ramp(from, left).log10() / total_db
        };
        self.compressor_db *= share;
        self.leveller_db *= share;
        self.leveller_lin = fast_exp2(self.leveller_db * LOG2_PER_DB20);
    }

    fn gain(&self) -> f32 {
        fast_exp2((self.compressor_db + self.leveller_db) * LOG2_PER_DB20)
    }

    /// Updates the detectors with one frame and returns its gain.
    fn track(&mut self, frame: &[f32]) -> f32 {
        let peak = frame.iter().fold(0f32, |peak, x| peak.max(x.abs()));
        self.detector = flush(peak.max(self.detector * self.detect));
        let level_db = DB20_PER_LOG2 * fast_log2(self.detector.max(1e-6));
        let target = curve(level_db);
        let coefficient = if target < self.compressor_db {
            self.attack
        } else {
            self.release
        };
        self.compressor_db = flush(target + coefficient * (self.compressor_db - target));
        let comp_lin = fast_exp2(self.compressor_db * LOG2_PER_DB20);
        // Below -60 dBFS (pauses, hiss) the leveller holds its gain and its
        // measurement, so quiet passages don't pump the noise floor up.
        if level_db >= HOLD_BELOW_DB {
            // It measures what the compressor lets through (`10^(db/10) == comp_lin^2`).
            self.leveller_accum_power += power(frame) * (comp_lin * comp_lin);
            self.leveller_accum_count += 1;
            if self.leveller_accum_count == LEVELLER_DECIM {
                let avg_power = self.leveller_accum_power * (1. / LEVELLER_DECIM as f32);
                self.leveller_accum_power = 0.;
                self.leveller_accum_count = 0;
                self.mean_square = avg_power + self.rms_decim * (self.mean_square - avg_power);
                let rms_db = DB10_PER_LOG2 * fast_log2(self.mean_square.max(1e-12));
                let want = (TARGET_DB - rms_db).clamp(LEVEL_MIN_DB, LEVEL_MAX_DB);
                let step = SLEW_DB_PER_SECOND * (LEVELLER_DECIM as f32 / RATE);
                self.leveller_db += (want - self.leveller_db).clamp(-step, step);
                self.leveller_lin = fast_exp2(self.leveller_db * LOG2_PER_DB20);
            }
        }
        comp_lin * self.leveller_lin
    }

    #[cfg(test)]
    pub(crate) fn compressor_db(&self) -> f32 {
        self.compressor_db
    }

    #[cfg(test)]
    pub(crate) fn leveller_db(&self) -> f32 {
        self.leveller_db
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod fast_math_tests {
    use super::*;

    #[test]
    fn fast_log2_and_fast_exp2_are_within_one_thousandth_db() {
        assert_eq!(fast_log2(1.0), 0.0);
        assert_eq!(fast_log2(2.0), 1.0);
        assert_eq!(fast_exp2(0.0), 1.0);
        assert_eq!(fast_exp2(1.0), 2.0);
        assert_eq!(fast_exp2(-1.0), 0.5);
        // Sweep 120 dB of amplitude / power range (`1e-12..=2.0`).
        let mut max_log_db_err = 0f32;
        for step in -240..=20 {
            let db = step as f32 * 0.5;
            let x = 10f32.powf(db / 20.0);
            let exact = 20.0 * x.log10();
            let approx = DB20_PER_LOG2 * fast_log2(x);
            max_log_db_err = max_log_db_err.max((approx - exact).abs());
        }
        assert!(max_log_db_err < 0.001, "max log dB error: {max_log_db_err}");

        let mut max_exp_db_err = 0f32;
        for step in -120..=48 {
            let db = step as f32 * 0.5;
            let exact = 10f32.powf(db / 20.0);
            let approx = fast_exp2(db * LOG2_PER_DB20);
            let err_db = (20.0 * (approx / exact).log10()).abs();
            max_exp_db_err = max_exp_db_err.max(err_db);
        }
        assert!(max_exp_db_err < 0.001, "max exp dB error: {max_exp_db_err}");
    }

    #[test]
    fn fast_decimated_night_matches_libm_reference_within_0_05_db() {
        // Exact per-sample libm reference model for comparison.
        let (detect, attack, release, rms) = (
            coefficient(DETECT_SECONDS),
            coefficient(ATTACK_SECONDS),
            coefficient(RELEASE_SECONDS),
            coefficient(RMS_SECONDS),
        );
        let mut ref_detector = 0f32;
        let mut ref_comp_db = 0f32;
        let mut ref_ms = 10f32.powf(TARGET_DB / 10.);
        let mut ref_lev_db = 0f32;

        let frames = 48_000 * 6;
        let mut signal = Vec::with_capacity(frames * 2);
        for n in 0..frames {
            let amp = if n < 48_000 * 2 {
                10f32.powf(-36. / 20.)
            } else if n < 48_000 * 4 {
                10f32.powf(-8. / 20.)
            } else {
                10f32.powf(-24. / 20.)
            };
            let s = amp * (std::f32::consts::TAU * 440. * n as f32 / RATE).sin();
            signal.push(s);
            signal.push(s * 0.8);
        }
        let mut fast = Night::default();
        let mut fast_out = signal.clone();
        fast.process(&mut fast_out, 2, true);

        let mut max_gain_err_db = 0f32;
        for frame in signal.as_chunks::<2>().0 {
            let peak = frame[0].abs().max(frame[1].abs());
            ref_detector = flush(peak.max(ref_detector * detect));
            let level_db = 20. * ref_detector.max(1e-6).log10();
            let target = curve(level_db);
            let coeff = if target < ref_comp_db {
                attack
            } else {
                release
            };
            ref_comp_db = flush(target + coeff * (ref_comp_db - target));
            if level_db >= HOLD_BELOW_DB {
                let p = power(frame) * 10f32.powf(ref_comp_db / 10.);
                ref_ms = p + rms * (ref_ms - p);
                let rms_db = 10. * ref_ms.max(1e-12).log10();
                let want = (TARGET_DB - rms_db).clamp(LEVEL_MIN_DB, LEVEL_MAX_DB);
                let step = SLEW_DB_PER_SECOND / RATE;
                ref_lev_db += (want - ref_lev_db).clamp(-step, step);
            }
        }
        max_gain_err_db = max_gain_err_db
            .max((fast.compressor_db() - ref_comp_db).abs())
            .max((fast.leveller_db() - ref_lev_db).abs());
        assert!(
            max_gain_err_db < 0.05,
            "night mode drift {max_gain_err_db} dB (fast comp={} lev={}, ref comp={ref_comp_db} lev={ref_lev_db})",
            fast.compressor_db(),
            fast.leveller_db()
        );
    }
}
